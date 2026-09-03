//! Filesystem policy and descriptor-relative, bounded-memory metadata traversal.
//!
//! Paths are display names. Once a traversal starts, directory descriptors anchor
//! its reads; `openat(O_NOFOLLOW)` and `fstatat(AT_SYMLINK_NOFOLLOW)` never follow a
//! replaced link. This is a metadata fingerprint, deliberately not a content hash.
use crate::model::{Identity, Result, Root};
use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, CString, OsStr};
use std::fs::File;
use std::io::Read;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

pub(crate) const MAX_DEPTH: usize = 128;
const MAX_LINK_IDENTITIES: usize = 131_072;
const MAX_MANIFEST: u64 = 4 * 1024 * 1024;
const SCOPE_OBSERVATION_ATTEMPTS: usize = 3;
pub(crate) const SCOPE_CONTENTS_CHANGED: &str =
    "Files changed during refresh; this scope will be checked again.";

/// Only callers that have recognized an owned developer artifact may opt in to
/// generated application bundles and non-followed symbolic-link leaves. Folder
/// authorization and ordinary downloads always retain the strict policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MeasurementPolicy {
    #[default]
    Strict,
    Developer,
}

pub fn cancelled(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err("Cancelled".into())
    } else {
        Ok(())
    }
}

fn os_error(context: &str) -> String {
    format!("{context}: {}", std::io::Error::last_os_error())
}

fn c_name(name: &OsStr) -> Result<CString> {
    CString::new(name.as_bytes()).map_err(|_| "A path contains a NUL byte".into())
}

pub(crate) fn absolute_components(path: &Path) -> Result<Vec<&OsStr>> {
    if !path.is_absolute() {
        return Err("Choose an absolute local folder path".into());
    }
    path.components()
        .filter_map(|part| match part {
            Component::RootDir => None,
            Component::Normal(name) => Some(Ok(name)),
            _ => Some(Err(
                "Parent and relative path components are not allowed".into()
            )),
        })
        .collect()
}

/// Enumeration itself can materialize a dataless directory on macOS. Keep the
/// worker opted out for the whole operation, restoring the caller's thread policy.
/// The guard cannot move to another thread.
pub(crate) struct LocalOnlyIo {
    #[cfg(target_os = "macos")]
    previous: i32,
    _thread: std::marker::PhantomData<std::rc::Rc<()>>,
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn getiopolicy_np(kind: libc::c_int, scope: libc::c_int) -> libc::c_int;
    fn setiopolicy_np(kind: libc::c_int, scope: libc::c_int, policy: libc::c_int) -> libc::c_int;
}

#[cfg(target_os = "macos")]
const IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES: i32 = 3;
#[cfg(target_os = "macos")]
const IOPOL_SCOPE_THREAD: i32 = 1;
#[cfg(target_os = "macos")]
const IOPOL_MATERIALIZE_DATALESS_FILES_OFF: i32 = 1;

impl LocalOnlyIo {
    pub(crate) fn new() -> Result<Self> {
        #[cfg(target_os = "macos")]
        {
            // Constants from sys/resource.h, currently absent from libc's bindings.
            let previous = unsafe {
                getiopolicy_np(
                    IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES,
                    IOPOL_SCOPE_THREAD,
                )
            };
            if previous < 0
                || unsafe {
                    setiopolicy_np(
                        IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES,
                        IOPOL_SCOPE_THREAD,
                        IOPOL_MATERIALIZE_DATALESS_FILES_OFF,
                    )
                } != 0
            {
                return Err("Cannot disable cloud materialization for filesystem work".into());
            }
            Ok(Self {
                previous,
                _thread: std::marker::PhantomData,
            })
        }
        #[cfg(not(target_os = "macos"))]
        Ok(Self {
            _thread: std::marker::PhantomData,
        })
    }
}

impl Drop for LocalOnlyIo {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        unsafe {
            setiopolicy_np(
                IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES,
                IOPOL_SCOPE_THREAD,
                self.previous,
            );
        }
    }
}

/// Open a directory without following a symbolic link anywhere in its path.
pub(crate) fn open_directory(path: &Path) -> Result<OwnedFd> {
    open_directory_with_access(path, false, None)
}

fn open_search_directory(path: &Path) -> Result<OwnedFd> {
    open_directory_with_access(path, true, None)
}

fn open_directory_with_access(
    path: &Path,
    search_only: bool,
    cancel: Option<&AtomicBool>,
) -> Result<OwnedFd> {
    let names = absolute_components(path)?;
    if let Some(cancel) = cancel {
        cancelled(cancel)?;
    }
    #[cfg(target_os = "macos")]
    {
        let path = c_name(path.as_os_str())?;
        // The macOS 14 minimum supports rejecting every symlink in one lookup.
        // O_NOFOLLOW_ANY cannot be combined with O_NOFOLLOW or O_SYMLINK.
        // Keep legacy /.vol paths component-relative: an absolute open can
        // otherwise translate that spelling into a different physical path.
        if names.first().is_none_or(|name| name.as_bytes() != b".vol") {
            let access = if search_only {
                libc::O_SEARCH
            } else {
                libc::O_RDONLY | libc::O_DIRECTORY
            };
            if let Some(cancel) = cancel {
                cancelled(cancel)?;
            }
            let opened = unsafe {
                libc::open(
                    path.as_ptr(),
                    access | libc::O_NOFOLLOW_ANY | libc::O_CLOEXEC,
                )
            };
            if opened >= 0 {
                let fd = unsafe { OwnedFd::from_raw_fd(opened) };
                if let Some(cancel) = cancel {
                    cancelled(cancel)?;
                }
                return Ok(fd);
            }
            let error = std::io::Error::last_os_error();
            if let Some(cancel) = cancel {
                cancelled(cancel)?;
            }
            if error.raw_os_error() != Some(libc::ENAMETOOLONG) {
                return Err(format!(
                    "A folder is inaccessible, changed, or has a symbolic-link ancestor: {error}"
                ));
            }
        }
    }
    open_directory_components(&names, search_only, cancel)
}

fn open_directory_components(
    names: &[&OsStr],
    search_only: bool,
    cancel: Option<&AtomicBool>,
) -> Result<OwnedFd> {
    if let Some(cancel) = cancel {
        cancelled(cancel)?;
    }
    let initial = unsafe {
        libc::open(
            c"/".as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if initial < 0 {
        return Err(os_error("Cannot open filesystem root"));
    }
    let mut fd = unsafe { OwnedFd::from_raw_fd(initial) };
    let count = names.len();
    for (index, name) in names.iter().enumerate() {
        if let Some(cancel) = cancel {
            cancelled(cancel)?;
        }
        let name = c_name(name)?;
        #[cfg(target_os = "macos")]
        let access = if search_only || index + 1 < count {
            libc::O_SEARCH
        } else {
            libc::O_RDONLY | libc::O_DIRECTORY
        };
        #[cfg(not(target_os = "macos"))]
        let access = {
            let _ = (search_only, index, count);
            libc::O_RDONLY | libc::O_DIRECTORY
        };
        let next = unsafe {
            libc::openat(
                fd.as_raw_fd(),
                name.as_ptr(),
                access | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if next < 0 {
            return Err(os_error(
                "A folder is inaccessible, changed, or has a symbolic-link ancestor",
            ));
        }
        fd = unsafe { OwnedFd::from_raw_fd(next) };
    }
    if let Some(cancel) = cancel {
        cancelled(cancel)?;
    }
    Ok(fd)
}

pub(crate) fn stat_fd(fd: RawFd) -> Result<EntryMeta> {
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd, &mut stat) } != 0 {
        return Err(os_error("Cannot inspect an open file"));
    }
    Ok(EntryMeta::from_stat(&stat))
}

pub(crate) fn metadata(path: &Path) -> Result<EntryMeta> {
    absolute_components(path)?;
    if path == Path::new("/") {
        return stat_fd(open_directory(path)?.as_raw_fd());
    }
    let parent = open_search_directory(path.parent().ok_or("The file has no parent")?)?;
    stat_child(
        parent.as_raw_fd(),
        path.file_name().ok_or("The file has no name")?,
    )
}

/// Read a scope's first metadata without following links or enumerating siblings.
/// An observation is authoritative only while its pinned ancestry still reaches
/// the authorized path; a missing grant or an unsafe parent remains an error.
pub(crate) fn scope_metadata(
    root: &Root,
    path: &Path,
    cancel: &AtomicBool,
) -> Result<Option<EntryMeta>> {
    scope_metadata_impl(root, path, None, cancel, &|| {})
}

/// Bind a descendant observation to an already selected physical directory.
/// The real grant still anchors the lookup. The selected ancestor must retain
/// all metadata, not just its inode, throughout the same pinned ancestry proof.
/// The caller remains responsible for the leaf's type and recommendation policy.
pub(crate) fn scope_metadata_with_ancestor(
    root: &Root,
    path: &Path,
    artifact: &Entry,
    cancel: &AtomicBool,
    checkpoint: &impl Fn(),
) -> Result<Option<EntryMeta>> {
    cancelled(cancel)?;
    check_scope_policy(root, &artifact.path)?;
    if path == artifact.path || !path.starts_with(&artifact.path) {
        return Err("The witness must be a strict descendant of the selected artifact".into());
    }
    check_scope_parent(&artifact.meta, root.identity.device)?;
    scope_metadata_impl(root, path, Some(artifact), cancel, checkpoint)
}

fn scope_metadata_impl(
    root: &Root,
    path: &Path,
    ancestor: Option<&Entry>,
    cancel: &AtomicBool,
    checkpoint: &impl Fn(),
) -> Result<Option<EntryMeta>> {
    cancelled(cancel)?;
    let _local_io = LocalOnlyIo::new()?;
    check_scope_policy(root, path)?;
    let relative = path
        .strip_prefix(&root.path)
        .map_err(|_| "Scope is outside its grant")?;
    let names = relative
        .components()
        .map(|part| part.as_os_str())
        .take(MAX_DEPTH + 1)
        .collect::<Vec<_>>();
    if names.len() > MAX_DEPTH {
        return Err("Refresh scope ancestry exceeds the bounded directory depth".into());
    }
    let ancestor = ancestor.map(|entry| {
        // The public helper checked containment before any filesystem access.
        let depth = entry
            .path
            .strip_prefix(&root.path)
            .unwrap()
            .components()
            .count();
        (depth, &entry.meta)
    });
    scope_checkpoint(cancel, checkpoint)?;
    validate_root(root)?;
    #[cfg(test)]
    tests::observe_scope_metadata(tests::ScopeMetadataPhase::AfterGrantValidation);
    scope_checkpoint(cancel, checkpoint)?;
    let fd = open_directory_with_access(&root.path, false, Some(cancel))?;
    scope_checkpoint(cancel, checkpoint)?;
    let meta = scope_parent_metadata(fd.as_raw_fd(), root.identity.device, cancel)?;
    if !same_object(&root.identity, &meta.identity) {
        return Err("The authorized folder has been replaced; choose it again".into());
    }
    check_scope_ancestor(ancestor, 0, &meta)?;
    scope_checkpoint(cancel, checkpoint)?;
    check_local(fd.as_raw_fd())?;
    cancelled(cancel)?;
    let mut parents = Vec::with_capacity(names.len().max(1));
    parents.push(ScopeParent {
        fd,
        identity: meta.identity.clone(),
    });
    if names.is_empty() {
        return confirm_scope_observation(
            root,
            &names,
            &parents,
            Some(&meta),
            ancestor,
            cancel,
            checkpoint,
        );
    }
    for (index, name) in names.iter().enumerate() {
        let parent = parents.last().unwrap();
        scope_checkpoint(cancel, checkpoint)?;
        let Some(meta) = scope_child_metadata(parent.fd.as_raw_fd(), name, cancel)? else {
            return confirm_scope_observation(
                root,
                &names[..=index],
                &parents,
                None,
                ancestor,
                cancel,
                checkpoint,
            );
        };
        check_scope_ancestor(ancestor, index + 1, &meta)?;
        if index + 1 == names.len() {
            return confirm_scope_observation(
                root,
                &names,
                &parents,
                Some(&meta),
                ancestor,
                cancel,
                checkpoint,
            );
        }
        check_scope_parent(&meta, root.identity.device)?;
        let encoded = c_name(name)?;
        #[cfg(test)]
        tests::observe_scope_metadata(tests::ScopeMetadataPhase::BeforeParentOpen(index));
        scope_checkpoint(cancel, checkpoint)?;
        let raw = unsafe {
            libc::openat(
                parent.fd.as_raw_fd(),
                encoded.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        let opened = if raw < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(unsafe { OwnedFd::from_raw_fd(raw) })
        };
        cancelled(cancel)?;
        let fd = match opened {
            Ok(fd) => fd,
            Err(error) if error.raw_os_error() == Some(libc::ENOENT) => {
                return confirm_scope_observation(
                    root,
                    &names[..=index],
                    &parents,
                    None,
                    ancestor,
                    cancel,
                    checkpoint,
                );
            }
            Err(error) => return Err(format!("Cannot open refresh scope parent: {error}")),
        };
        scope_checkpoint(cancel, checkpoint)?;
        let opened = scope_parent_metadata(fd.as_raw_fd(), root.identity.device, cancel)?;
        if opened != meta {
            return Err("A refresh scope parent changed before opening".into());
        }
        parents.push(ScopeParent {
            fd,
            identity: opened.identity,
        });
    }
    unreachable!("Every scope lookup returns its leaf metadata or a verified absence")
}

fn scope_checkpoint(cancel: &AtomicBool, checkpoint: &impl Fn()) -> Result<()> {
    cancelled(cancel)?;
    checkpoint();
    cancelled(cancel)
}

fn check_scope_ancestor(
    ancestor: Option<(usize, &EntryMeta)>,
    depth: usize,
    meta: &EntryMeta,
) -> Result<()> {
    if ancestor
        .is_some_and(|(expected_depth, expected)| depth == expected_depth && meta != expected)
    {
        return Err("The selected artifact changed during witness verification".into());
    }
    Ok(())
}

struct ScopeParent {
    fd: OwnedFd,
    identity: Identity,
}

fn check_scope_parent(meta: &EntryMeta, device: u64) -> Result<()> {
    if !meta.is_dir() || meta.is_dataless() || meta.identity.device != device {
        return Err("A scope parent is protected or no longer a local directory".into());
    }
    Ok(())
}

fn scope_parent_metadata(fd: RawFd, device: u64, cancel: &AtomicBool) -> Result<EntryMeta> {
    cancelled(cancel)?;
    let meta = stat_fd(fd)?;
    cancelled(cancel)?;
    check_scope_parent(&meta, device)?;
    cloud_directory_check(fd)?;
    cancelled(cancel)?;
    Ok(meta)
}

/// This internal lookup preserves errno. The caller must validate the pinned
/// ancestry before using either its metadata or its observation of absence.
fn scope_child_metadata(
    parent: RawFd,
    name: &OsStr,
    cancel: &AtomicBool,
) -> Result<Option<EntryMeta>> {
    let name = c_name(name)?;
    cancelled(cancel)?;
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    let status =
        unsafe { libc::fstatat(parent, name.as_ptr(), &mut stat, libc::AT_SYMLINK_NOFOLLOW) };
    let error = (status != 0).then(std::io::Error::last_os_error);
    cancelled(cancel)?;
    match error {
        None => Ok(Some(EntryMeta::from_stat(&stat))),
        Some(error) if error.raw_os_error() == Some(libc::ENOENT) => Ok(None),
        Some(error) => Err(format!("Cannot inspect refresh scope: {error}")),
    }
}

fn confirm_scope_observation(
    root: &Root,
    names: &[&OsStr],
    parents: &[ScopeParent],
    expected: Option<&EntryMeta>,
    ancestor: Option<(usize, &EntryMeta)>,
    cancel: &AtomicBool,
    checkpoint: &impl Fn(),
) -> Result<Option<EntryMeta>> {
    confirm_scope_observation_attempt(
        root, names, parents, expected, ancestor, cancel, checkpoint, 0,
    )
}

#[allow(clippy::too_many_arguments)]
fn confirm_scope_observation_attempt(
    root: &Root,
    names: &[&OsStr],
    parents: &[ScopeParent],
    expected: Option<&EntryMeta>,
    ancestor: Option<(usize, &EntryMeta)>,
    cancel: &AtomicBool,
    checkpoint: &impl Fn(),
    attempt: usize,
) -> Result<Option<EntryMeta>> {
    debug_assert_eq!(names.len().max(1), parents.len());
    // A cleanup pause may invalidate any earlier observation. Pause before this
    // bounded proof, never after some of its final ancestry checks have passed.
    // Cancellation remains checked between its filesystem operations.
    scope_checkpoint(cancel, checkpoint)?;
    #[cfg(test)]
    tests::observe_scope_metadata(if expected.is_some() {
        tests::ScopeMetadataPhase::PresentObserved
    } else {
        tests::ScopeMetadataPhase::MissingObserved
    });
    // Capture after the observation: removing the requested child may have
    // legitimately changed its parent's timestamps. Pinned ancestors prevent
    // inode reuse from disguising a substituted path during this proof.
    let mut observed = Vec::with_capacity(parents.len());
    for (depth, parent) in parents.iter().enumerate() {
        let meta = scope_parent_metadata(parent.fd.as_raw_fd(), root.identity.device, cancel)?;
        if !same_object(&parent.identity, &meta.identity) {
            return Err("A refresh scope parent changed during metadata verification".into());
        }
        check_scope_ancestor(ancestor, depth, &meta)?;
        observed.push(meta);
    }
    for index in 1..parents.len() {
        let current =
            scope_child_metadata(parents[index - 1].fd.as_raw_fd(), names[index - 1], cancel)?;
        if current.as_ref() != Some(&observed[index]) {
            return Err(
                "A refresh scope parent moved or changed during metadata verification".into(),
            );
        }
    }
    let current = if let Some(name) = names.last() {
        scope_child_metadata(parents.last().unwrap().fd.as_raw_fd(), name, cancel)?
    } else {
        Some(scope_parent_metadata(
            parents[0].fd.as_raw_fd(),
            root.identity.device,
            cancel,
        )?)
    };
    if expected.is_some() && current.is_none() {
        // Before any traversal or publication, one disappearance can establish
        // a new observation. Re-capture after removal; the None proof cannot
        // enter this branch again or turn a substitution/error into absence.
        return confirm_scope_observation_attempt(
            root, names, parents, None, ancestor, cancel, checkpoint, attempt,
        );
    }
    // Initial discovery can observe an actively written file or directory. Its
    // contents changing is distinct from replacing the path or changing its
    // protection. Bound witnesses to their original full metadata as before.
    // A non-root leaf is not pinned: its parent's original timestamps must
    // still match, so unlink/recreate cannot disguise inode reuse as a write.
    // The root itself is pinned by parents[0].
    let contents_changed = ancestor.is_none()
        && (names.is_empty()
            || observed.last().unwrap().identity == parents.last().unwrap().identity)
        && matches!((expected, current.as_ref()), (Some(before), Some(after))
            if before != after
                && same_object(&before.identity, &after.identity)
                && before.uid == after.uid
                && before.flags == after.flags);
    if current.as_ref() != expected && !contents_changed {
        return Err("The refresh scope moved or changed during metadata verification".into());
    }
    #[cfg(test)]
    tests::observe_scope_metadata(if expected.is_some() {
        tests::ScopeMetadataPhase::BeforePresenceValidation
    } else {
        tests::ScopeMetadataPhase::BeforeAbsenceValidation
    });
    cancelled(cancel)?;
    // Resolve the grant again: an unchanged detached descriptor does not prove
    // that the authorized pathname still reaches it.
    validate_root(root)?;
    cancelled(cancel)?;
    for (parent, before) in parents.iter().zip(&observed) {
        let current = scope_parent_metadata(parent.fd.as_raw_fd(), root.identity.device, cancel)?;
        if &current != before {
            return Err("A refresh scope parent changed during metadata verification".into());
        }
    }
    if contents_changed {
        // Finish the strict grant/ancestry proof before either retrying or
        // reporting churn. Keep every parent pinned and require the leaf to
        // remain the same object across attempts; never retry a substitution.
        if attempt + 1 < SCOPE_OBSERVATION_ATTEMPTS {
            return confirm_scope_observation_attempt(
                root,
                names,
                parents,
                current.as_ref(),
                ancestor,
                cancel,
                checkpoint,
                attempt + 1,
            );
        }
        return Err(SCOPE_CONTENTS_CHANGED.into());
    }
    Ok(expected.cloned())
}

#[cfg(test)]
thread_local! {
    static CHILD_METADATA_CALLS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

fn stat_child(fd: RawFd, name: &OsStr) -> Result<EntryMeta> {
    #[cfg(test)]
    CHILD_METADATA_CALLS.with(|calls| calls.set(calls.get() + 1));
    let name = c_name(name)?;
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstatat(fd, name.as_ptr(), &mut stat, libc::AT_SYMLINK_NOFOLLOW) } != 0 {
        let error = std::io::Error::last_os_error();
        return Err(if error.raw_os_error() == Some(libc::ENOENT) {
            "A file or folder moved or disappeared. Scan again to refresh this location.".into()
        } else {
            format!("Cannot inspect a directory entry: {error}")
        });
    }
    Ok(EntryMeta::from_stat(&stat))
}

pub fn identity(path: &Path) -> Result<Identity> {
    let meta = metadata(path)?;
    if meta.is_symlink() {
        return Err("Symbolic links are excluded".into());
    }
    Ok(meta.identity)
}

pub(crate) fn same_object(left: &Identity, right: &Identity) -> bool {
    left.device == right.device && left.inode == right.inode && left.mode == right.mode
}

pub(crate) fn is_cloud_component(name: &OsStr) -> bool {
    let name = name.as_bytes();
    [
        "cloudstorage",
        "mobile documents",
        "icloud drive",
        "dropbox",
        "google drive",
        "googledrive",
        "box",
        "pcloud drive",
        "amazon drive",
        "proton drive",
        "nextcloud",
        "owncloud",
        "synologydrive",
    ]
    .iter()
    .any(|known| name.eq_ignore_ascii_case(known.as_bytes()))
        || name
            .get(..8)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"onedrive"))
        || name
            .get(..9)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"dropbox ("))
        || name
            .get(name.len().saturating_sub(7)..)
            .is_some_and(|suffix| suffix.eq_ignore_ascii_case(b".icloud"))
}

pub(crate) fn excluded_name(name: &OsStr, is_directory: bool) -> bool {
    if is_cloud_component(name) {
        return true;
    }
    if !is_directory {
        return false;
    }
    let name = name.as_bytes();
    [
        ".git",
        ".hg",
        ".svn",
        ".trash",
        ".trashes",
        ".spotlight-v100",
        ".fseventsd",
        ".documentrevisions-v100",
        "library",
        "backups.backupdb",
        ".timemachine",
        ".pnpm-store",
        ".pnpm",
        ".yarn",
        ".store",
    ]
    .iter()
    .any(|known| name.eq_ignore_ascii_case(known.as_bytes()))
        || [
            ".app",
            ".photoslibrary",
            ".photolibrary",
            ".musiclibrary",
            ".backupbundle",
            ".sparsebundle",
            ".bundle",
            ".framework",
        ]
        .iter()
        .any(|extension| {
            name.get(name.len().saturating_sub(extension.len())..)
                .is_some_and(|suffix| suffix.eq_ignore_ascii_case(extension.as_bytes()))
        })
}

fn excluded_measurement_name(name: &OsStr, is_directory: bool, policy: MeasurementPolicy) -> bool {
    if policy == MeasurementPolicy::Strict {
        return excluded_name(name, is_directory);
    }
    // A generated bundle can occur inside a recognized artifact; this does not
    // relax authorization or entry into application folders elsewhere. Cloud
    // names remain excluded even when they also have a bundle extension.
    let bytes = name.as_bytes();
    let generated_bundle =
        [b".app".as_slice(), b".framework", b".bundle"]
            .iter()
            .any(|extension| {
                bytes
                    .get(bytes.len().saturating_sub(extension.len())..)
                    .is_some_and(|suffix| suffix.eq_ignore_ascii_case(extension))
            });
    if generated_bundle && !is_cloud_component(name) {
        return false;
    }
    // Protected names also apply to developer link leaves and repository marker
    // files. No link named .git, a cloud root, or a shared store bypasses them.
    excluded_name(name, true)
}

pub(crate) fn check_path_policy(path: &Path) -> Result<()> {
    let names = absolute_components(path)?;
    if names.iter().any(|name| excluded_name(name, true)) {
        return Err(
            "Protected, shared-store, application, or cloud-managed locations are excluded".into(),
        );
    }
    for prefix in [
        "/System",
        "/Library",
        "/Applications",
        "/bin",
        "/sbin",
        "/usr",
        "/dev",
        "/etc",
        "/private/etc",
        "/private/var/db",
        "/private/var/vm",
    ] {
        if path.starts_with(prefix) {
            return Err("System and application locations are excluded".into());
        }
    }
    if matches!(
        path.to_str(),
        Some("/" | "/Users" | "/Volumes" | "/private" | "/private/var")
    ) {
        return Err("Choose a specific personal or project folder".into());
    }
    Ok(())
}

fn personal_media_name(name: &OsStr) -> bool {
    [b"Music".as_slice(), b"Pictures", b"Movies"]
        .iter()
        .any(|known| name.as_bytes().eq_ignore_ascii_case(known))
}

/// Only the immediate media folders of an explicit Home grant are omitted.
/// A project such as Projects/Music remains within ordinary discovery scope.
pub(crate) fn excluded_home_media(root: &Root, path: &Path) -> bool {
    root.kind == "home"
        && path.strip_prefix(&root.path).is_ok_and(|relative| {
            relative
                .components()
                .next()
                .is_some_and(|part| personal_media_name(part.as_os_str()))
        })
}

/// Lexical checks must precede metadata reads, including replayed event scopes
/// and candidates saved by an older recommendation policy.
pub(crate) fn check_scope_policy(root: &Root, path: &Path) -> Result<()> {
    if !path.starts_with(&root.path) {
        return Err("The path is outside its authorized folder".into());
    }
    if crate::recommendations::library_route_allowed(root, path) {
        // The Home grant, not a name anywhere on disk, authorizes these exact
        // user routes. All other protected components remain protected, even
        // inside a cache. Physical ancestry is verified by the callers as usual.
        check_path_policy(&root.path)?;
        let relative = path.strip_prefix(&root.path).unwrap();
        let names = relative.components().collect::<Vec<_>>();
        if names
            .first()
            .is_none_or(|part| part.as_os_str() != "Library")
            || names.iter().skip(1).any(|part| {
                !matches!(part, Component::Normal(_)) || excluded_name(part.as_os_str(), true)
            })
        {
            return Err("Protected content inside this Library route is excluded".into());
        }
    } else {
        check_path_policy(path)?;
    }
    if excluded_home_media(root, path) {
        return Err("Personal media folders are excluded from Home discovery".into());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
pub(crate) fn cloud_directory_check(fd: RawFd) -> Result<()> {
    // ATTR_CMNEXT_EXT_FLAGS returns a packed u32 length followed by a u64.
    let mut attributes: libc::attrlist = unsafe { std::mem::zeroed() };
    attributes.bitmapcount = 5;
    attributes.forkattr = 0x0000_0200;
    let mut result = [0u8; 12];
    let status = unsafe {
        libc::fgetattrlist(
            fd,
            (&mut attributes as *mut libc::attrlist).cast(),
            result.as_mut_ptr().cast(),
            result.len(),
            0x20,
        )
    };
    if status != 0 || u32::from_ne_bytes(result[0..4].try_into().unwrap()) != 12 {
        return Err("Cloud ownership could not be verified for this directory".into());
    }
    let flags = u64::from_ne_bytes(result[4..12].try_into().unwrap());
    if flags & 0x0000_0004 != 0 {
        return Err("Cloud synchronization roots are excluded".into());
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn cloud_directory_check(_fd: RawFd) -> Result<()> {
    Ok(())
}

pub(crate) fn check_local(fd: RawFd) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let mut info: libc::statfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstatfs(fd, &mut info) } != 0 {
            return Err(os_error("Cannot identify the storage volume"));
        }
        if info.f_flags & libc::MNT_LOCAL as u32 == 0 {
            return Err("Network and remote volumes are excluded".into());
        }
        if info.f_flags & libc::MNT_RDONLY as u32 != 0 {
            return Err("This volume is read-only".into());
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = fd;
    Ok(())
}

/// Cloud roots can have arbitrary names. Inspect every physical ancestor too.
pub(crate) fn validate_ancestors(path: &Path) -> Result<()> {
    let names = absolute_components(path)?;
    let mut current = PathBuf::from("/");
    for name in names {
        current.push(name);
        let fd = open_directory(&current)?;
        let meta = stat_fd(fd.as_raw_fd())?;
        if meta.is_dataless() {
            return Err("Cloud placeholders are excluded without downloading them".into());
        }
        cloud_directory_check(fd.as_raw_fd())?;
    }
    Ok(())
}

pub fn authorize(path: &Path, kind: &str) -> Result<Root> {
    let _local_io = LocalOnlyIo::new()?;
    if !matches!(kind, "projects" | "downloads" | "folder" | "home") {
        return Err("Choose a projects, downloads, folder, or home location kind".into());
    }
    check_path_policy(path)?;
    let fd = open_directory(path)?;
    check_local(fd.as_raw_fd())?;
    validate_ancestors(path)?;
    let meta = stat_fd(fd.as_raw_fd())?;
    if !meta.is_dir() {
        return Err("Choose a local directory".into());
    }
    let canonical =
        std::fs::canonicalize(path).map_err(|e| format!("Cannot resolve selected folder: {e}"))?;
    if canonical != path || !same_object(&meta.identity, &identity(&canonical)?) {
        return Err("The selected folder changed or contains a redirected path".into());
    }
    let mut hash = blake3::Hasher::new();
    hash.update(canonical.as_os_str().as_bytes());
    hash.update(&meta.identity.device.to_le_bytes());
    hash.update(&meta.identity.inode.to_le_bytes());
    Ok(Root {
        id: hash.finalize().to_hex()[..24].to_string(),
        path: canonical,
        kind: kind.into(),
        identity: meta.identity,
    })
}

pub fn validate_root(root: &Root) -> Result<()> {
    check_path_policy(&root.path)?;
    let fd = open_directory(&root.path)?;
    check_local(fd.as_raw_fd())?;
    if !same_object(&root.identity, &stat_fd(fd.as_raw_fd())?.identity) {
        return Err("The authorized folder has been replaced; choose it again".into());
    }
    validate_ancestors(&root.path)
}

/// Bytes and identity captured from the same validated file. Consumers must not
/// pair these bytes with a later, independently fetched pathname identity.
#[derive(Debug)]
pub(crate) struct RegularFile {
    pub bytes: Vec<u8>,
    pub identity: Identity,
}

/// Read only small, regular, owned manifests. Keep the parent pinned through the
/// read, then verify that the original pathname still reaches that parent/file.
pub(crate) fn read_regular(path: &Path, cancel: &AtomicBool) -> Result<RegularFile> {
    read_regular_bounded(path, cancel, MAX_MANIFEST)
}

/// A provider may impose a smaller evidence limit. Enforce it on the pinned
/// descriptor before allocating, not after reading a larger file into memory.
pub(crate) fn read_regular_bounded(
    path: &Path,
    cancel: &AtomicBool,
    maximum_bytes: u64,
) -> Result<RegularFile> {
    if maximum_bytes == 0 || maximum_bytes > MAX_MANIFEST {
        return Err("Invalid project evidence byte limit".into());
    }
    cancelled(cancel)?;
    let _local_io = LocalOnlyIo::new()?;
    absolute_components(path)?;
    let parent_path = path.parent().ok_or("No manifest parent")?;
    let name = path.file_name().ok_or("No manifest name")?;
    let parent = open_directory_with_access(parent_path, true, Some(cancel))?;
    cancelled(cancel)?;
    let parent_identity = stat_fd(parent.as_raw_fd())?.identity;
    let captured = read_regular_at(parent.as_fd(), name, cancel, maximum_bytes)?;
    #[cfg(test)]
    tests::observe_regular_read(tests::RegularReadPhase::BeforePathValidation);
    cancelled(cancel)?;
    let current_parent = open_directory_with_access(parent_path, true, Some(cancel))?;
    cancelled(cancel)?;
    if !same_object(
        &parent_identity,
        &stat_fd(current_parent.as_raw_fd())?.identity,
    ) {
        return Err("Project evidence parent changed while reading it".into());
    }
    cancelled(cancel)?;
    let current = stat_child(current_parent.as_raw_fd(), name)?;
    if current.identity != captured.identity || !regular_evidence_metadata(&current) {
        return Err("Project evidence pathname changed while reading it".into());
    }
    Ok(captured)
}

pub(crate) fn regular_evidence_metadata(meta: &EntryMeta) -> bool {
    meta.is_file()
        && !meta.is_dataless()
        && meta.links == 1
        && meta.uid == unsafe { libc::geteuid() }
        && meta.identity.size <= MAX_MANIFEST
}

fn read_regular_at(
    parent: BorrowedFd<'_>,
    name: &OsStr,
    cancel: &AtomicBool,
    maximum_bytes: u64,
) -> Result<RegularFile> {
    cancelled(cancel)?;
    let before = stat_child(parent.as_raw_fd(), name)?;
    if !regular_evidence_metadata(&before) || before.identity.size > maximum_bytes {
        return Err("Project evidence is not a small, independent, local regular file".into());
    }
    let name = c_name(name)?;
    #[cfg(test)]
    tests::observe_regular_read(tests::RegularReadPhase::BeforeOpen);
    cancelled(cancel)?;
    let raw = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
        )
    };
    if raw < 0 {
        return Err(os_error("Cannot open project evidence"));
    }
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    cancelled(cancel)?;
    if stat_fd(fd.as_raw_fd())? != before {
        return Err("Project evidence changed while opening it".into());
    }
    let mut file = File::from(fd);
    let mut bytes = vec![0; before.identity.size as usize];
    let mut read = 0;
    while read < bytes.len() {
        let end = bytes.len().min(read + 64 * 1024);
        let count = read_evidence_chunk(&mut file, &mut bytes[read..end], cancel)?;
        if count == 0 {
            return Err("Project evidence became shorter while reading it".into());
        }
        read += count;
        #[cfg(test)]
        tests::observe_regular_read(tests::RegularReadPhase::AfterChunk(read));
    }
    // Detect growth without allocating beyond the original, bounded file size.
    if read_evidence_chunk(&mut file, &mut [0u8; 1], cancel)? != 0 {
        return Err("Project evidence grew while reading it".into());
    }
    cancelled(cancel)?;
    if stat_fd(file.as_raw_fd())? != before {
        return Err("Project evidence changed while reading it".into());
    }
    Ok(RegularFile {
        bytes,
        identity: before.identity,
    })
}

fn read_evidence_chunk(file: &mut File, bytes: &mut [u8], cancel: &AtomicBool) -> Result<usize> {
    loop {
        cancelled(cancel)?;
        match file.read(bytes) {
            Ok(count) => return Ok(count),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(format!("Cannot read project evidence: {error}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EntryMeta {
    pub identity: Identity,
    pub allocated: u64,
    pub links: u64,
    pub uid: u32,
    pub flags: u32,
}

impl EntryMeta {
    pub(crate) fn from_stat(stat: &libc::stat) -> Self {
        #[cfg(target_os = "macos")]
        let flags = stat.st_flags;
        #[cfg(not(target_os = "macos"))]
        let flags = 0;
        Self {
            identity: Identity {
                device: stat.st_dev as u64,
                inode: stat.st_ino,
                mode: stat.st_mode as u32,
                size: stat.st_size.max(0) as u64,
                modified_ns: stat
                    .st_mtime
                    .saturating_mul(1_000_000_000)
                    .saturating_add(stat.st_mtime_nsec),
                changed_ns: stat
                    .st_ctime
                    .saturating_mul(1_000_000_000)
                    .saturating_add(stat.st_ctime_nsec),
            },
            allocated: (stat.st_blocks.max(0) as u64).saturating_mul(512),
            links: stat.st_nlink as u64,
            uid: stat.st_uid,
            flags,
        }
    }
    pub fn is_dir(&self) -> bool {
        self.identity.mode & libc::S_IFMT as u32 == libc::S_IFDIR as u32
    }
    pub fn is_file(&self) -> bool {
        self.identity.mode & libc::S_IFMT as u32 == libc::S_IFREG as u32
    }
    pub fn is_symlink(&self) -> bool {
        self.identity.mode & libc::S_IFMT as u32 == libc::S_IFLNK as u32
    }
    pub fn is_dataless(&self) -> bool {
        self.flags & 0x4000_0000 != 0
    }
}

#[derive(Debug)]
pub(crate) struct Entry {
    pub path: PathBuf,
    pub meta: EntryMeta,
}

/// A shallow, exact-byte namespace observation. A synthesized child path can
/// resolve a differently cased name on APFS, so metadata alone is insufficient.
/// Callers must bind the scan's starting metadata to this observation and keep
/// its candidates private until validation succeeds.
pub(crate) struct ExactChild {
    path: PathBuf,
    parent: Option<ExactChildParent>,
    metadata: Option<EntryMeta>,
}

struct ExactChildParent {
    directory: Directory,
    metadata: EntryMeta,
}

impl ExactChild {
    pub(crate) fn observe(
        root: &Root,
        path: &Path,
        cancel: &AtomicBool,
        checkpoint: &impl Fn(),
    ) -> Result<Self> {
        cancelled(cancel)?;
        check_scope_policy(root, path)?;
        if path == root.path {
            return Err("An exact child must be below its authorized folder".into());
        }
        if path
            .strip_prefix(&root.path)
            .map_err(|_| "The path is outside its authorized folder")?
            .components()
            .take(MAX_DEPTH + 1)
            .count()
            > MAX_DEPTH
        {
            return Err("Exact-child scope ancestry exceeds the bounded directory depth".into());
        }
        let parent_path = path.parent().ok_or("The child has no parent")?;
        let name = path.file_name().ok_or("The child has no name")?;
        if name.as_bytes().contains(&0) {
            return Err("A path contains a NUL byte".into());
        }
        checkpoint();
        cancelled(cancel)?;
        let _local_io = LocalOnlyIo::new()?;
        let Some(metadata) = scope_metadata(root, parent_path, cancel)? else {
            return Ok(Self {
                path: path.to_path_buf(),
                parent: None,
                metadata: None,
            });
        };
        check_scope_parent(&metadata, root.identity.device)?;
        let fd = open_directory_with_access(parent_path, false, Some(cancel))?;
        let opened = scope_parent_metadata(fd.as_raw_fd(), root.identity.device, cancel)?;
        if opened != metadata {
            return Err("The exact child's parent changed before opening".into());
        }
        let mut directory =
            Directory::from_fd(parent_path.to_path_buf(), fd, metadata.identity.clone())?;
        let child = directory.exact_child_metadata(name, cancel, checkpoint)?;
        let observation = Self {
            path: path.to_path_buf(),
            parent: Some(ExactChildParent {
                directory,
                metadata,
            }),
            metadata: child,
        };
        observation.validate_parent(root, cancel)?;
        Ok(observation)
    }

    pub(crate) fn metadata(&self) -> Option<&EntryMeta> {
        self.metadata.as_ref()
    }

    pub(crate) fn validate(
        &self,
        root: &Root,
        cancel: &AtomicBool,
        checkpoint: &impl Fn(),
    ) -> Result<()> {
        cancelled(cancel)?;
        check_scope_policy(root, &self.path)?;
        checkpoint();
        cancelled(cancel)?;
        let _local_io = LocalOnlyIo::new()?;
        self.validate_parent(root, cancel)?;
        if let Some(parent) = &self.parent {
            // An independent open file description starts at the first name;
            // neither dup nor a shared readdir/bulk offset can provide that.
            let mut fresh = parent.directory.reopen_names(cancel)?;
            if scope_parent_metadata(fresh.fd(), root.identity.device, cancel)? != parent.metadata {
                return Err("The exact child's parent changed before rechecking names".into());
            }
            let current = fresh.exact_child_metadata(
                self.path.file_name().ok_or("The child has no name")?,
                cancel,
                checkpoint,
            )?;
            if current != self.metadata {
                return Err("The exact child changed name, identity, or metadata".into());
            }
            self.validate_parent(root, cancel)?;
        }
        Ok(())
    }

    fn validate_parent(&self, root: &Root, cancel: &AtomicBool) -> Result<()> {
        let parent_path = self.path.parent().ok_or("The child has no parent")?;
        let current = scope_metadata(root, parent_path, cancel)?;
        let Some(parent) = &self.parent else {
            return if current.is_none() {
                Ok(())
            } else {
                Err("The exact child's missing parent has reappeared".into())
            };
        };
        if current.as_ref() != Some(&parent.metadata)
            || scope_parent_metadata(parent.directory.fd(), root.identity.device, cancel)?
                != parent.metadata
        {
            return Err("The exact child's parent moved or changed".into());
        }
        Ok(())
    }
}

pub(crate) struct Directory {
    pub path: PathBuf,
    reader: DirectoryReader,
    initial: Identity,
    #[cfg(test)]
    force_unknown_types: bool,
}

/// Until a names reader is requested, the descriptor has not enumerated any
/// contents. In particular, bulk traversal must not call fdopendir: on macOS it
/// eagerly reads names and would share an offset with getattrlistbulk.
enum DirectoryReader {
    #[cfg(target_os = "macos")]
    Bulk {
        fd: OwnedFd,
        reader: BulkReader,
    },
    #[cfg(not(target_os = "macos"))]
    Pending(OwnedFd),
    Names(NamesReader),
}

struct NamesReader(*mut libc::DIR);

impl Drop for NamesReader {
    fn drop(&mut self) {
        unsafe { libc::closedir(self.0) };
    }
}

/// A directory type from readdir is only a hint. The caller must open it with
/// O_NOFOLLOW and inspect that descriptor before treating it as an Entry.
pub(crate) enum DiscoveryEntry {
    Directory(PathBuf),
    Metadata(Entry),
}

/// A bounded name-only discovery step. Ordinary files cannot be suggestions
/// outside Downloads or a recognized artifact, so their metadata is unnecessary.
pub(crate) struct DiscoveryStep {
    pub entry: Option<DiscoveryEntry>,
    pub files: u64,
    pub skipped: u64,
    pub metadata_skipped: u64,
    pub finished: bool,
}

#[derive(Clone, Copy)]
enum DiscoveryFiles {
    None,
    Personal,
    Library { cache_root: bool },
}

impl DiscoveryFiles {
    fn includes(self, name: &OsStr) -> bool {
        match self {
            Self::None => false,
            Self::Personal => crate::recommendations::personal_file_name(name),
            Self::Library { .. } => true,
        }
    }

    fn excludes_before_metadata(self, name: &OsStr) -> bool {
        matches!(self, Self::Library { cache_root }
            if excluded_name(name, true)
                || (cache_root && crate::recommendations::managed_cache(name)))
    }
}

fn directory_entry_path(directory: &Path, name: &OsStr) -> PathBuf {
    let parent = directory.as_os_str().as_bytes();
    let separator = usize::from(!parent.is_empty() && !parent.ends_with(b"/"));
    let capacity = parent
        .len()
        .saturating_add(separator)
        .saturating_add(name.as_bytes().len());
    // Reserve for both components before copying the parent. Path::join can
    // otherwise allocate the parent and then grow it to append the entry name.
    let mut path = PathBuf::with_capacity(capacity);
    path.push(directory);
    path.push(name);
    path
}

impl Directory {
    pub fn open(path: &Path) -> Result<Self> {
        let fd = open_directory(path)?;
        let initial = stat_fd(fd.as_raw_fd())?.identity;
        Self::from_fd(path.to_path_buf(), fd, initial)
    }
    fn from_fd(path: PathBuf, fd: OwnedFd, initial: Identity) -> Result<Self> {
        cloud_directory_check(fd.as_raw_fd())?;
        Ok(Self {
            path,
            initial,
            #[cfg(target_os = "macos")]
            reader: DirectoryReader::Bulk {
                fd,
                reader: BulkReader::default(),
            },
            #[cfg(not(target_os = "macos"))]
            reader: DirectoryReader::Pending(fd),
            #[cfg(test)]
            force_unknown_types: false,
        })
    }
    fn fd(&self) -> RawFd {
        match &self.reader {
            #[cfg(target_os = "macos")]
            DirectoryReader::Bulk { fd, .. } => fd.as_raw_fd(),
            #[cfg(not(target_os = "macos"))]
            DirectoryReader::Pending(fd) => fd.as_raw_fd(),
            DirectoryReader::Names(reader) => unsafe { libc::dirfd(reader.0) },
        }
    }

    fn open_child_fd(&self, path: &Path) -> Result<OwnedFd> {
        let name = c_name(path.file_name().ok_or("No directory name")?)?;
        let raw = unsafe {
            libc::openat(
                self.fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if raw < 0 {
            return Err(os_error("A directory changed or could not be opened"));
        }
        Ok(unsafe { OwnedFd::from_raw_fd(raw) })
    }

    /// Fuse a known directory's metadata lookup with the descriptor we retain
    /// for traversal. Lexical exclusions must be checked before calling this.
    /// No contents are enumerated until next/next_discovery is requested.
    pub fn open_discovered(
        &self,
        path: PathBuf,
        device: u64,
        cancel: &AtomicBool,
    ) -> Result<(Entry, Option<Self>)> {
        cancelled(cancel)?;
        let fd = self.open_child_fd(&path)?;
        cancelled(cancel)?;
        let meta = stat_fd(fd.as_raw_fd())?;
        if !meta.is_dir() {
            return Err("The discovered directory changed type before opening".into());
        }
        cancelled(cancel)?;
        // A nested mount or placeholder still contributes an examined boundary,
        // but never receives an enumerator or cloud-provider metadata lookup.
        let directory = if meta.identity.device == device && !meta.is_dataless() {
            Some(Self::from_fd(path.clone(), fd, meta.identity.clone())?)
        } else {
            None
        };
        Ok((Entry { path, meta }, directory))
    }
    pub fn open_child(&self, entry: &Entry) -> Result<Self> {
        let fd = self.open_child_fd(&entry.path)?;
        let initial = stat_fd(fd.as_raw_fd())?.identity;
        if initial != entry.meta.identity {
            return Err("Directory identity or contents changed before traversal".into());
        }
        Self::from_fd(entry.path.clone(), fd, initial)
    }
    pub fn next(&mut self, cancel: &AtomicBool) -> Result<Option<Entry>> {
        cancelled(cancel)?;
        #[cfg(target_os = "macos")]
        {
            if matches!(&self.reader, DirectoryReader::Bulk { reader, .. } if reader.buffer.is_none())
            {
                self.unchanged()?;
            }
            if let DirectoryReader::Bulk { fd, reader } = &mut self.reader {
                match reader.next(fd.as_raw_fd(), &self.path, cancel) {
                    Ok(entry) => return Ok(entry),
                    Err(BulkError::Unsupported) => self.use_legacy_reader()?,
                    Err(BulkError::Failed(reason)) => return Err(reason),
                }
            }
        }
        self.initialize_names(cancel)?;
        self.next_legacy(cancel)
    }

    /// Enumerate names/types in libc's buffered reader. Known directory names
    /// let the caller prune before opening; unknown types retain no-follow stat.
    /// A short step bounds cancellation/progress latency even in a huge flat
    /// directory. Never mix an advanced bulk offset with readdir's buffer.
    pub fn next_discovery(
        &mut self,
        cancel: &AtomicBool,
        home_children: bool,
    ) -> Result<DiscoveryStep> {
        self.next_discovery_filtered(cancel, home_children, DiscoveryFiles::None)
    }

    /// Personal-file discovery keeps the same bounded names pass, requesting
    /// metadata only for relevant document, media and archive formats.
    pub(crate) fn next_personal_discovery(
        &mut self,
        cancel: &AtomicBool,
        home_children: bool,
    ) -> Result<DiscoveryStep> {
        self.next_discovery_filtered(cancel, home_children, DiscoveryFiles::Personal)
    }

    /// Targeted Library routes need regular-file metadata, but still reject
    /// protected names and manager-owned cache stores before a stat or open.
    pub(crate) fn next_library_discovery(
        &mut self,
        cancel: &AtomicBool,
        cache_root: bool,
    ) -> Result<DiscoveryStep> {
        self.next_discovery_filtered(cancel, false, DiscoveryFiles::Library { cache_root })
    }

    fn next_discovery_filtered(
        &mut self,
        cancel: &AtomicBool,
        home_children: bool,
        files: DiscoveryFiles,
    ) -> Result<DiscoveryStep> {
        self.initialize_names(cancel)?;
        let mut step = DiscoveryStep {
            entry: None,
            files: 0,
            skipped: 0,
            metadata_skipped: 0,
            finished: false,
        };
        for _ in 0..256 {
            cancelled(cancel)?;
            set_errno_zero();
            let DirectoryReader::Names(reader) = &self.reader else {
                unreachable!("Names reader was initialized above")
            };
            let record = unsafe { libc::readdir(reader.0) };
            if record.is_null() {
                if std::io::Error::last_os_error().raw_os_error().unwrap_or(0) != 0 {
                    return Err(os_error("Directory enumeration failed"));
                }
                step.finished = true;
                break;
            }
            let bytes = unsafe { CStr::from_ptr((*record).d_name.as_ptr()) }.to_bytes();
            if bytes == b"." || bytes == b".." {
                continue;
            }
            let name = OsStr::from_bytes(bytes);
            if files.excludes_before_metadata(name) {
                step.skipped += 1;
                step.metadata_skipped += 1;
                continue;
            }
            let entry_type = unsafe { (*record).d_type };
            #[cfg(test)]
            let entry_type = if self.force_unknown_types {
                libc::DT_UNKNOWN
            } else {
                entry_type
            };
            match entry_type {
                libc::DT_REG => {
                    if files.includes(name) {
                        step.entry = Some(DiscoveryEntry::Metadata(Entry {
                            path: directory_entry_path(&self.path, name),
                            meta: stat_child(self.fd(), name)?,
                        }));
                        break;
                    }
                    step.files += 1;
                    step.metadata_skipped += 1;
                }
                libc::DT_DIR | libc::DT_UNKNOWN => {
                    let name = OsStr::from_bytes(bytes);
                    // Filter before metadata, even for DT_UNKNOWN: the name
                    // proves these boundaries cannot be suggestions. Ordinary
                    // regular files retain their constant-work fast path above.
                    if excluded_name(name, true) || (home_children && personal_media_name(name)) {
                        step.skipped += 1;
                        step.metadata_skipped += 1;
                        continue;
                    }
                    if entry_type == libc::DT_DIR {
                        step.entry = Some(DiscoveryEntry::Directory(directory_entry_path(
                            &self.path, name,
                        )));
                        break;
                    }
                    let meta = stat_child(self.fd(), name)?;
                    if meta.is_dir() || (meta.is_file() && files.includes(name)) {
                        step.entry = Some(DiscoveryEntry::Metadata(Entry {
                            path: directory_entry_path(&self.path, name),
                            meta,
                        }));
                        break;
                    } else if meta.is_file() {
                        step.files += 1;
                    } else {
                        step.skipped += 1;
                    }
                }
                _ => {
                    step.skipped += 1;
                    step.metadata_skipped += 1;
                }
            }
        }
        Ok(step)
    }

    fn exact_child_metadata(
        &mut self,
        name: &OsStr,
        cancel: &AtomicBool,
        checkpoint: &impl Fn(),
    ) -> Result<Option<EntryMeta>> {
        self.initialize_names(cancel)?;
        loop {
            checkpoint();
            for _ in 0..256 {
                cancelled(cancel)?;
                set_errno_zero();
                let DirectoryReader::Names(reader) = &self.reader else {
                    unreachable!("Names reader was initialized above")
                };
                let record = unsafe { libc::readdir(reader.0) };
                if record.is_null() {
                    if std::io::Error::last_os_error().raw_os_error().unwrap_or(0) != 0 {
                        return Err(os_error("Exact-child enumeration failed"));
                    }
                    cancelled(cancel)?;
                    self.unchanged()?;
                    cancelled(cancel)?;
                    return Ok(None);
                }
                let bytes = unsafe { CStr::from_ptr((*record).d_name.as_ptr()) }.to_bytes();
                // Do not inspect d_type or stat unrelated siblings, including
                // DT_UNKNOWN entries, protected names, and symbolic links.
                if bytes != name.as_bytes() {
                    continue;
                }
                let metadata = stat_child(self.fd(), name)?;
                cancelled(cancel)?;
                self.unchanged()?;
                cancelled(cancel)?;
                return Ok(Some(metadata));
            }
        }
    }

    fn reopen_names(&self, cancel: &AtomicBool) -> Result<Self> {
        cancelled(cancel)?;
        let raw = unsafe {
            libc::openat(
                self.fd(),
                c".".as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if raw < 0 {
            return Err(os_error("Cannot reopen the exact child's parent"));
        }
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        cancelled(cancel)?;
        let initial = stat_fd(fd.as_raw_fd())?.identity;
        if initial != self.initial {
            return Err("The exact child's parent changed before rechecking names".into());
        }
        Self::from_fd(self.path.clone(), fd, initial)
    }

    fn initialize_names(&mut self, cancel: &AtomicBool) -> Result<()> {
        cancelled(cancel)?;
        if matches!(self.reader, DirectoryReader::Names(_)) {
            return Ok(());
        }
        #[cfg(target_os = "macos")]
        if matches!(&self.reader, DirectoryReader::Bulk { reader, .. } if reader.buffer.is_some()) {
            return Err("Cannot change enumeration policy after bulk traversal".into());
        }
        // Classification or a queued parent can delay first enumeration. Retain
        // the same identity barrier as opening a previously inspected Entry.
        self.unchanged()?;
        cancelled(cancel)?;
        #[cfg(test)]
        tests::observe_directory_read(&self.path);
        let handle = unsafe { libc::fdopendir(self.fd()) };
        if handle.is_null() {
            return Err(os_error("Cannot enumerate directory"));
        }
        // fdopendir owns the descriptor only after success. Transfer it without
        // closing it or keeping a second owner of the same file description.
        match std::mem::replace(
            &mut self.reader,
            DirectoryReader::Names(NamesReader(handle)),
        ) {
            #[cfg(target_os = "macos")]
            DirectoryReader::Bulk { fd, .. } => {
                let _ = fd.into_raw_fd();
            }
            #[cfg(not(target_os = "macos"))]
            DirectoryReader::Pending(fd) => {
                let _ = fd.into_raw_fd();
            }
            DirectoryReader::Names(_) => unreachable!("Reader was not initialized"),
        }
        Ok(())
    }

    /// getattrlistbulk and readdir must never share an enumeration offset. A new
    /// open file description, reached through the existing descriptor, restarts
    /// an unsupported bulk enumeration without following a pathname replacement.
    #[cfg(target_os = "macos")]
    fn use_legacy_reader(&mut self) -> Result<()> {
        let fd = unsafe {
            libc::openat(
                self.fd(),
                c".".as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(os_error("Cannot restart unsupported directory enumeration"));
        }
        let reopened = unsafe { OwnedFd::from_raw_fd(fd) };
        if stat_fd(fd)?.identity != self.initial {
            return Err("A directory changed before its fallback enumeration".into());
        }
        #[cfg(test)]
        tests::observe_directory_read(&self.path);
        let handle = unsafe { libc::fdopendir(fd) };
        if handle.is_null() {
            return Err(os_error("Cannot initialize fallback directory enumeration"));
        }
        let _ = reopened.into_raw_fd();
        self.reader = DirectoryReader::Names(NamesReader(handle));
        Ok(())
    }

    fn next_legacy(&mut self, cancel: &AtomicBool) -> Result<Option<Entry>> {
        loop {
            cancelled(cancel)?;
            set_errno_zero();
            let DirectoryReader::Names(reader) = &self.reader else {
                unreachable!("Legacy enumeration requires a names reader")
            };
            let record = unsafe { libc::readdir(reader.0) };
            if record.is_null() {
                if std::io::Error::last_os_error().raw_os_error().unwrap_or(0) != 0 {
                    return Err(os_error("Directory enumeration failed"));
                }
                return Ok(None);
            }
            let bytes = unsafe { CStr::from_ptr((*record).d_name.as_ptr()) }.to_bytes();
            if bytes == b"." || bytes == b".." {
                continue;
            }
            let name = OsStr::from_bytes(bytes);
            cancelled(cancel)?;
            let meta = stat_child(self.fd(), name)?;
            return Ok(Some(Entry {
                path: directory_entry_path(&self.path, name),
                meta,
            }));
        }
    }
    pub fn unchanged(&self) -> Result<()> {
        if stat_fd(self.fd())?.identity != self.initial {
            Err("A directory changed during measurement; refresh its review".into())
        } else {
            Ok(())
        }
    }
}

// SDK references: sys/attr.h and getattrlistbulk(2), macOS SDK. The syscall
// reports the symbolic link itself. FSOPT_PACK_INVAL_ATTRS gives fixed field
// positions; returned masks, rather than placeholder values, establish validity.
#[cfg(target_os = "macos")]
const BULK_BUFFER_BYTES: usize = 64 * 1024;
#[cfg(target_os = "macos")]
const BULK_ERROR_ATTRIBUTE: u32 = 0x2000_0000;
#[cfg(target_os = "macos")]
const BULK_COMMON: u32 = libc::ATTR_CMN_RETURNED_ATTRS
    | BULK_ERROR_ATTRIBUTE
    | libc::ATTR_CMN_NAME
    | libc::ATTR_CMN_DEVID
    | libc::ATTR_CMN_OBJTYPE
    | libc::ATTR_CMN_MODTIME
    | libc::ATTR_CMN_CHGTIME
    | libc::ATTR_CMN_OWNERID
    | libc::ATTR_CMN_ACCESSMASK
    | libc::ATTR_CMN_FLAGS
    | libc::ATTR_CMN_FILEID;
#[cfg(target_os = "macos")]
const BULK_FILE: u32 =
    libc::ATTR_FILE_LINKCOUNT | libc::ATTR_FILE_ALLOCSIZE | libc::ATTR_FILE_DATALENGTH;
#[cfg(target_os = "macos")]
const BULK_COMMON_BYTES: usize = 96;
#[cfg(target_os = "macos")]
const BULK_FILE_BYTES: usize = 116;

#[cfg(target_os = "macos")]
#[derive(Debug)]
enum BulkError {
    Unsupported,
    Failed(String),
}

#[cfg(target_os = "macos")]
#[derive(Default)]
struct BulkReader {
    // u64 backing guarantees the 8-byte alignment required by the syscall.
    buffer: Option<Box<[u64]>>,
    offset: usize,
    remaining: usize,
    emitted: u64,
    done: bool,
    #[cfg(test)]
    injected_errno: Option<i32>,
}

#[cfg(target_os = "macos")]
impl BulkReader {
    fn next(
        &mut self,
        fd: RawFd,
        directory: &Path,
        cancel: &AtomicBool,
    ) -> std::result::Result<Option<Entry>, BulkError> {
        cancelled(cancel).map_err(BulkError::Failed)?;
        if self.done {
            return Ok(None);
        }
        if self.remaining == 0 {
            #[cfg(test)]
            tests::observe_directory_read(directory);
            self.fetch(fd, cancel)?;
            if self.done {
                return Ok(None);
            }
        }
        let buffer = self
            .buffer
            .as_ref()
            .expect("bulk buffer exists after fetch");
        let bytes =
            unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), BULK_BUFFER_BYTES) };
        let record = parse_bulk_record(bytes.get(self.offset..).ok_or_else(|| {
            BulkError::Failed("Bulk directory offset exceeded its buffer".into())
        })?)
        .map_err(BulkError::Failed)?;
        let name = OsStr::from_bytes(record.name);
        cancelled(cancel).map_err(BulkError::Failed)?;
        // Directories need stat semantics (link counts and mounted-root identity
        // differ in the bulk API). Symlinks/specials and incomplete records also
        // retain the exact descriptor-relative no-follow metadata path.
        let meta = match record.metadata {
            Some(meta) => meta,
            None => stat_child(fd, name).map_err(BulkError::Failed)?,
        };
        let entry = Entry {
            path: directory_entry_path(directory, name),
            meta,
        };
        self.offset = self
            .offset
            .checked_add(record.length)
            .ok_or_else(|| BulkError::Failed("Bulk directory offset overflowed".into()))?;
        self.remaining -= 1;
        self.emitted += 1;
        Ok(Some(entry))
    }

    fn fetch(&mut self, fd: RawFd, cancel: &AtomicBool) -> std::result::Result<(), BulkError> {
        cancelled(cancel).map_err(BulkError::Failed)?;
        #[cfg(test)]
        if let Some(code) = self.injected_errno.take() {
            return Err(self.system_error(code));
        }
        let buffer = self
            .buffer
            .get_or_insert_with(|| vec![0u64; BULK_BUFFER_BYTES / 8].into_boxed_slice());
        let mut attributes: libc::attrlist = unsafe { std::mem::zeroed() };
        attributes.bitmapcount = 5;
        attributes.commonattr = BULK_COMMON;
        attributes.fileattr = BULK_FILE;
        let count = unsafe {
            libc::getattrlistbulk(
                fd,
                (&mut attributes as *mut libc::attrlist).cast(),
                buffer.as_mut_ptr().cast(),
                BULK_BUFFER_BYTES,
                0x8, // FSOPT_PACK_INVAL_ATTRS
            )
        };
        if count < 0 {
            return Err(self.system_error(
                std::io::Error::last_os_error()
                    .raw_os_error()
                    .unwrap_or(libc::EIO),
            ));
        }
        if count as usize > BULK_BUFFER_BYTES / BULK_COMMON_BYTES {
            return Err(BulkError::Failed(
                "Bulk directory returned an impossible record count".into(),
            ));
        }
        self.offset = 0;
        self.remaining = count as usize;
        self.done = count == 0;
        Ok(())
    }

    fn system_error(&self, code: i32) -> BulkError {
        if self.emitted == 0
            && [libc::ENOTSUP, libc::EOPNOTSUPP, libc::ENOSYS, libc::EINVAL].contains(&code)
        {
            BulkError::Unsupported
        } else {
            BulkError::Failed(format!(
                "Bulk directory enumeration failed; coverage is incomplete: {}",
                std::io::Error::from_raw_os_error(code)
            ))
        }
    }
}

#[cfg(target_os = "macos")]
struct BulkRecord<'a> {
    length: usize,
    name: &'a [u8],
    metadata: Option<EntryMeta>,
}

/// Parse only byte slices: packed timespec/u64 fields are not naturally aligned.
/// No field is trusted until its record length, masks, name and bounds validate.
#[cfg(target_os = "macos")]
fn parse_bulk_record(bytes: &[u8]) -> Result<BulkRecord<'_>> {
    let word = |offset: usize| -> Result<u32> {
        let field = bytes
            .get(offset..offset + 4)
            .ok_or("Truncated bulk directory field")?;
        Ok(u32::from_ne_bytes(field.try_into().unwrap()))
    };
    let length = word(0)? as usize;
    if length <= BULK_COMMON_BYTES || length > bytes.len() || !length.is_multiple_of(8) {
        return Err("Invalid bulk directory record length".into());
    }
    let bytes = &bytes[..length];
    let word = |offset: usize| -> u32 {
        u32::from_ne_bytes(bytes[offset..offset + 4].try_into().unwrap())
    };
    let wide = |offset: usize| -> u64 {
        u64::from_ne_bytes(bytes[offset..offset + 8].try_into().unwrap())
    };
    let common = word(4);
    let file = word(16);
    if common & (libc::ATTR_CMN_RETURNED_ATTRS | libc::ATTR_CMN_NAME)
        != (libc::ATTR_CMN_RETURNED_ATTRS | libc::ATTR_CMN_NAME)
        || common & !BULK_COMMON != 0
        || file & !BULK_FILE != 0
        || word(8) != 0
        || word(12) != 0
        || word(20) != 0
    {
        return Err("Bulk directory returned unsupported or missing attribute masks".into());
    }
    // attrreference offset is relative to the start of that reference (byte 28).
    let name_offset = i32::from_ne_bytes(bytes[28..32].try_into().unwrap()) as i64;
    let name_start = 28i64
        .checked_add(name_offset)
        .ok_or("Bulk name offset overflowed")?;
    let name_length = word(32) as usize;
    if name_start < BULK_COMMON_BYTES as i64 || name_length < 2 {
        return Err("Invalid bulk directory name reference".into());
    }
    let name_start =
        usize::try_from(name_start).map_err(|_| "Invalid bulk directory name offset")?;
    let name_end = name_start
        .checked_add(name_length)
        .ok_or("Bulk name length overflowed")?;
    let name = bytes
        .get(name_start..name_end)
        .ok_or("Bulk directory name exceeds its record")?;
    if name.last() != Some(&0)
        || name[..name.len() - 1].contains(&0)
        || name.contains(&b'/')
        || name == b".\0"
        || name == b"..\0"
    {
        return Err("Invalid bulk directory entry name".into());
    }
    let required = BULK_COMMON & !BULK_ERROR_ATTRIBUTE;
    // File-only groups are omitted for directories even with PACK_INVAL_ATTRS.
    // The first regular-file field may be read only when the full group is both
    // returned and present before the variable-length name.
    let fields_valid = common & required == required
        && file & BULK_FILE == BULK_FILE
        && word(24) == 0
        && name_start >= BULK_FILE_BYTES
        && length > BULK_FILE_BYTES;
    let objtype = word(40);
    let metadata = if fields_valid && objtype == 1 {
        // VREG, from sys/vnode.h
        let seconds = |offset| wide(offset) as i64;
        let modified_nanos = seconds(52);
        let changed_nanos = seconds(68);
        let allocated = seconds(100);
        let size = seconds(108);
        let links = word(96) as u64;
        if !(0..1_000_000_000).contains(&modified_nanos)
            || !(0..1_000_000_000).contains(&changed_nanos)
            || allocated < 0
            || size < 0
            || links == 0
        {
            None
        } else {
            Some(EntryMeta {
                identity: Identity {
                    device: word(36) as libc::dev_t as u64,
                    inode: wide(88),
                    mode: (word(80) & 0xffff & !(libc::S_IFMT as u32)) | libc::S_IFREG as u32,
                    size: size as u64,
                    modified_ns: seconds(44)
                        .saturating_mul(1_000_000_000)
                        .saturating_add(modified_nanos),
                    changed_ns: seconds(60)
                        .saturating_mul(1_000_000_000)
                        .saturating_add(changed_nanos),
                },
                allocated: allocated as u64,
                links,
                uid: word(76),
                flags: word(84),
            })
        }
    } else {
        None
    };
    Ok(BulkRecord {
        length,
        name: &name[..name.len() - 1],
        metadata,
    })
}

fn set_errno_zero() {
    #[cfg(target_os = "macos")]
    unsafe {
        *libc::__error() = 0;
    }
    #[cfg(target_os = "linux")]
    unsafe {
        *libc::__errno_location() = 0;
    }
}

#[derive(Debug, Clone, Default)]
pub struct Measurement {
    /// Suggestion-only early exit; never a complete fingerprint or mutation proof.
    pub pruned: bool,
    pub logical_bytes: u64,
    pub allocated_bytes: u64,
    pub files: u64,
    /// Newest modification anywhere in the visited tree, including exclusions.
    pub latest_modified_ns: i64,
    pub fingerprint: String,
    pub unsafe_reason: Option<String>,
    pub entries: u64,
    pub directories: u64,
    pub skipped: u64,
    pub errors: u64,
}

/// Only multiply-linked inodes need a set. Its strict cap keeps adversarial
/// fixtures bounded; once full, additional links receive zero size credit.
#[derive(Default)]
pub(crate) struct Hardlinks {
    seen: HashSet<(u64, u64)>,
    saturated: bool,
}
impl Hardlinks {
    pub fn first(&mut self, meta: &EntryMeta) -> bool {
        if meta.links <= 1 {
            return true;
        }
        let key = (meta.identity.device, meta.identity.inode);
        if self.seen.contains(&key) {
            return false;
        }
        if self.seen.len() >= MAX_LINK_IDENTITIES {
            self.saturated = true;
            return false;
        }
        self.seen.insert(key)
    }
    pub fn saturated(&self) -> bool {
        self.saturated
    }
}

struct RegularLinkGroup {
    original: EntryMeta,
    observed: u64,
}

/// Provisional closure evidence for one recognized developer artifact. Feed each
/// admitted regular path exactly once, never combining separate artifacts. A
/// caller may consume the groups only after the entire traversal and all other
/// policy, identity and fingerprint checks succeed.
///
/// Only multiply-linked inodes are retained. At the current 64-bit layout the
/// key and value occupy 96 bytes: 12 MiB of entries at MAX_LINK_IDENTITIES, about
/// 25 MiB including HashMap capacity and control bytes. Allocator overhead and
/// transient growth are additional; no paths, file contents or FDs are retained.
#[derive(Default)]
pub(crate) struct RegularLinkClosure {
    groups: HashMap<(u64, u64), RegularLinkGroup>,
    failure: Option<&'static str>,
}

impl RegularLinkClosure {
    fn reject(&mut self, reason: &'static str) -> Result<()> {
        let reason = *self.failure.get_or_insert(reason);
        Err(reason.into())
    }

    pub(crate) fn observe(&mut self, meta: &EntryMeta) -> Result<()> {
        if let Some(reason) = self.failure {
            return Err(reason.into());
        }
        let key = (meta.identity.device, meta.identity.inode);
        // Check an existing group before the single-link fast path: a later
        // alias with a reduced link count must not escape consistency checks.
        if let Some(group) = self.groups.get_mut(&key) {
            if group.original != *meta {
                return self.reject("A hard-linked file changed during measurement");
            }
            if group.observed >= meta.links {
                return self
                    .reject("A hard-linked file was observed more times than its link count");
            }
            group.observed += 1;
            return Ok(());
        }
        if !meta.is_file() {
            return Ok(());
        }
        if meta.links == 0 {
            return self.reject("A regular file lost its directory links during measurement");
        }
        if meta.links == 1 {
            return Ok(());
        }
        if self.groups.len() >= MAX_LINK_IDENTITIES {
            return self
                .reject("Hard-link closure limit reached; internal ownership is unverified");
        }
        if self.groups.try_reserve(1).is_err() {
            return self.reject("Hard-link closure storage is unavailable; cleanup is excluded");
        }
        self.groups.insert(
            key,
            RegularLinkGroup {
                original: meta.clone(),
                observed: 1,
            },
        );
        Ok(())
    }

    pub(crate) fn verify(&self) -> Result<()> {
        if let Some(reason) = self.failure {
            return Err(reason.into());
        }
        if self
            .groups
            .values()
            .any(|group| group.observed != group.original.links)
        {
            return Err(
                "Contains hard-linked files that may be shared outside this artifact".into(),
            );
        }
        Ok(())
    }

    pub(crate) fn into_closed(self) -> Result<impl Iterator<Item = EntryMeta>> {
        self.verify()?;
        Ok(self.groups.into_values().map(|group| group.original))
    }
}

#[derive(Default)]
struct Digest {
    sum: [u64; 4],
    xor: [u64; 4],
    count: u64,
}
impl Digest {
    fn add(&mut self, relative: &Path, meta: &EntryMeta, link: Option<&[u8]>) {
        let mut hash = blake3::Hasher::new();
        hash.update(relative.as_os_str().as_bytes());
        // Renaming the reviewed root into the engine's reserved sibling changes
        // only its ctime. Root object identity is separately checked before and
        // after staging; descendants retain their full timestamp evidence.
        let changed_ns = if relative.as_os_str().is_empty() {
            0
        } else {
            meta.identity.changed_ns
        };
        for value in [
            meta.identity.device,
            meta.identity.inode,
            meta.identity.mode as u64,
            meta.identity.size,
            meta.identity.modified_ns as u64,
            changed_ns as u64,
            meta.allocated,
            meta.links,
            meta.flags as u64,
            meta.uid as u64,
        ] {
            hash.update(&value.to_le_bytes());
        }
        if let Some(target) = link {
            hash.update(target);
        }
        let digest = hash.finalize();
        for (i, chunk) in digest.as_bytes().as_chunks::<8>().0.iter().enumerate() {
            let value = u64::from_le_bytes(*chunk);
            self.sum[i] = self.sum[i].wrapping_add(value);
            self.xor[i] ^= value;
        }
        self.count += 1;
    }
    fn finish(&self) -> String {
        let mut hash = blake3::Hasher::new();
        hash.update(b"chippytea-metadata-v1");
        for value in self.sum.into_iter().chain(self.xor).chain([self.count]) {
            hash.update(&value.to_le_bytes());
        }
        hash.finalize().to_hex().to_string()
    }
}

/// npm/Yarn create command links in `.bin`. These are safe leaf entries only
/// when a relative target remains inside this artifact and every target ancestor
/// is physical. We never traverse a link or count its target twice.
fn internal_bin_link(path: &Path, artifact: &Path, device: u64) -> Result<Vec<u8>> {
    if path.parent() != Some(artifact.join(".bin").as_path()) {
        return Err("Only contained .bin command links are supported".into());
    }
    let parent = open_directory(path.parent().ok_or("No symbolic-link parent")?)?;
    let name = c_name(path.file_name().ok_or("No symbolic-link name")?)?;
    let mut bytes = vec![0u8; 4096];
    let length = unsafe {
        libc::readlinkat(
            parent.as_raw_fd(),
            name.as_ptr(),
            bytes.as_mut_ptr().cast(),
            bytes.len(),
        )
    };
    if length < 0 || length as usize >= bytes.len() {
        return Err("The symbolic-link target is unavailable or too long".into());
    }
    bytes.truncate(length as usize);
    let target = Path::new(OsStr::from_bytes(&bytes));
    if target.is_absolute() || bytes.is_empty() {
        return Err("External or empty command links are excluded".into());
    }
    let mut resolved = path.parent().unwrap().to_path_buf();
    for component in target.components() {
        match component {
            Component::Normal(name) => resolved.push(name),
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            _ => return Err("An ambiguous command link is excluded".into()),
        }
        if !resolved.starts_with(artifact) {
            return Err("A command link leaves its artifact directory".into());
        }
    }
    let target_meta = metadata(&resolved)?;
    if !target_meta.is_file()
        || target_meta.identity.device != device
        || target_meta.is_dataless()
        || target_meta.links != 1
    {
        return Err("The command link does not name an independent local regular file".into());
    }
    Ok(bytes)
}

/// Read the link object through its already-open physical parent. The payload
/// may be absolute, dangling, or cyclic: it is fingerprinted, never resolved.
/// Rechecking metadata catches replacement between enumeration and readlinkat.
fn developer_link_payload(
    entry: &Entry,
    parent: Option<&Directory>,
    cancel: &AtomicBool,
) -> Result<Vec<u8>> {
    let opened_parent;
    let fd = match parent {
        Some(parent) => parent.fd(),
        None => {
            opened_parent =
                open_search_directory(entry.path.parent().ok_or("No symbolic-link parent")?)?;
            opened_parent.as_raw_fd()
        }
    };
    let name = entry.path.file_name().ok_or("No symbolic-link name")?;
    let encoded = c_name(name)?;
    cancelled(cancel)?;
    if stat_child(fd, name)? != entry.meta {
        return Err("A symbolic link changed before its payload was read".into());
    }
    cancelled(cancel)?;
    let mut bytes = vec![0u8; 4096];
    let length =
        unsafe { libc::readlinkat(fd, encoded.as_ptr(), bytes.as_mut_ptr().cast(), bytes.len()) };
    if length <= 0 || length as usize >= bytes.len() {
        return Err("A symbolic-link payload is unavailable or too long".into());
    }
    cancelled(cancel)?;
    if stat_child(fd, name)? != entry.meta {
        return Err("A symbolic link changed while its payload was read".into());
    }
    bytes.truncate(length as usize);
    Ok(bytes)
}

pub fn measure(path: &Path, device: u64, cancel: &AtomicBool) -> Result<Measurement> {
    measure_observing(path, device, cancel, |_, _| {})
}

pub fn measure_with_policy(
    path: &Path,
    device: u64,
    cancel: &AtomicBool,
    policy: MeasurementPolicy,
) -> Result<Measurement> {
    measure_observing_with_policy(path, device, cancel, policy, |_, _| {})
}

pub(crate) fn measure_observing(
    path: &Path,
    device: u64,
    cancel: &AtomicBool,
    mut observe: impl FnMut(&Entry, &Measurement),
) -> Result<Measurement> {
    measure_try_observing_with_policy(
        path,
        device,
        cancel,
        MeasurementPolicy::Strict,
        |entry, measurement| {
            observe(entry, measurement);
            Ok(())
        },
    )
}

pub(crate) fn measure_observing_with_policy(
    path: &Path,
    device: u64,
    cancel: &AtomicBool,
    policy: MeasurementPolicy,
    mut observe: impl FnMut(&Entry, &Measurement),
) -> Result<Measurement> {
    measure_try_observing_with_policy(path, device, cancel, policy, |entry, measurement| {
        observe(entry, measurement);
        Ok(())
    })
}

/// Streams the same full policy-aware measurement used for mutation checks.
/// Observer errors stop traversal immediately. Persisted observations must not
/// be committed until the caller has accepted the complete measurement.
pub(crate) fn measure_try_observing_with_policy(
    path: &Path,
    device: u64,
    cancel: &AtomicBool,
    policy: MeasurementPolicy,
    observe: impl FnMut(&Entry, &Measurement) -> Result<()>,
) -> Result<Measurement> {
    measure_observing_impl(path, device, cancel, policy, true, None, observe)
}

/// Cheap coverage for artifacts already excluded by suggestion policy. This is
/// never mutation evidence: the fingerprint is deliberately empty.
#[cfg(test)]
pub(crate) fn measure_metadata_observing(
    path: &Path,
    device: u64,
    cancel: &AtomicBool,
    mut observe: impl FnMut(&Entry, &Measurement),
) -> Result<Measurement> {
    measure_observing_impl(
        path,
        device,
        cancel,
        MeasurementPolicy::Strict,
        false,
        None,
        |entry, measurement| {
            observe(entry, measurement);
            Ok(())
        },
    )
}

#[cfg(test)]
pub(crate) fn measure_metadata_observing_with_policy(
    path: &Path,
    device: u64,
    cancel: &AtomicBool,
    policy: MeasurementPolicy,
    mut observe: impl FnMut(&Entry, &Measurement),
) -> Result<Measurement> {
    match policy {
        MeasurementPolicy::Strict => measure_metadata_observing(path, device, cancel, observe),
        MeasurementPolicy::Developer => measure_observing_impl(
            path,
            device,
            cancel,
            policy,
            false,
            None,
            |entry, measurement| {
                observe(entry, measurement);
                Ok(())
            },
        ),
    }
}

/// Once a descendant proves an artifact ineligible, stop reading its siblings.
/// Cleanup and metadata benchmarks never use this abbreviated measurement.
#[cfg(test)]
pub(crate) fn measure_suggestion_observing_with_policy(
    path: &Path,
    device: u64,
    cancel: &AtomicBool,
    policy: MeasurementPolicy,
    max_modified_ns: i64,
    mut observe: impl FnMut(&Entry, &Measurement),
) -> Result<Measurement> {
    measure_observing_impl(
        path,
        device,
        cancel,
        policy,
        true,
        Some(max_modified_ns),
        |entry, measurement| {
            observe(entry, measurement);
            Ok(())
        },
    )
}

fn measure_observing_impl(
    path: &Path,
    device: u64,
    cancel: &AtomicBool,
    policy: MeasurementPolicy,
    fingerprint: bool,
    stop_after_modified_ns: Option<i64>,
    mut observe: impl FnMut(&Entry, &Measurement) -> Result<()>,
) -> Result<Measurement> {
    let mut cursor = MeasurementCursor::new(
        path,
        device,
        policy,
        fingerprint,
        stop_after_modified_ns,
        cancel,
    )?;
    match cursor.advance_unbounded(cancel, &mut observe)? {
        MeasurementProgress::Pending => unreachable!("an unbounded measurement must finish"),
        MeasurementProgress::Complete(result) => Ok(result),
    }
}

/// One bounded slice of a complete measurement. A pending cursor owns all
/// provisional state; only `Complete` exposes the finalized fingerprint and
/// hard-link closure decision.
#[derive(Debug)]
pub(crate) enum MeasurementProgress {
    Pending,
    Complete(Measurement),
}

/// Resumable metadata measurement for one artifact. It deliberately cannot move
/// between threads: directory readers and the local-only I/O policy are tied to
/// the worker that advances it.
pub(crate) struct MeasurementCursor {
    path: PathBuf,
    device: u64,
    policy: MeasurementPolicy,
    stop_after_modified_ns: Option<i64>,
    initial: Identity,
    result: Measurement,
    digest: Option<Digest>,
    links: Hardlinks,
    regular_links: Option<RegularLinkClosure>,
    stack: Vec<Directory>,
    next: Option<Entry>,
    terminal: bool,
    _thread: std::marker::PhantomData<std::rc::Rc<()>>,
}

impl MeasurementCursor {
    pub(crate) fn full(
        path: &Path,
        device: u64,
        policy: MeasurementPolicy,
        cancel: &AtomicBool,
    ) -> Result<Self> {
        Self::new(path, device, policy, true, None, cancel)
    }

    pub(crate) fn metadata(
        path: &Path,
        device: u64,
        policy: MeasurementPolicy,
        cancel: &AtomicBool,
    ) -> Result<Self> {
        Self::new(path, device, policy, false, None, cancel)
    }

    pub(crate) fn suggestion(
        path: &Path,
        device: u64,
        policy: MeasurementPolicy,
        max_modified_ns: i64,
        cancel: &AtomicBool,
    ) -> Result<Self> {
        Self::new(path, device, policy, true, Some(max_modified_ns), cancel)
    }

    fn new(
        path: &Path,
        device: u64,
        policy: MeasurementPolicy,
        fingerprint: bool,
        stop_after_modified_ns: Option<i64>,
        cancel: &AtomicBool,
    ) -> Result<Self> {
        let _local_io = LocalOnlyIo::new()?;
        cancelled(cancel)?;
        let root = Entry {
            path: path.to_path_buf(),
            meta: metadata(path)?,
        };
        let initial = root.meta.identity.clone();
        Ok(Self {
            path: path.to_path_buf(),
            device,
            policy,
            stop_after_modified_ns,
            initial,
            result: Measurement::default(),
            digest: fingerprint.then(Digest::default),
            links: Hardlinks::default(),
            regular_links: (policy == MeasurementPolicy::Developer)
                .then(RegularLinkClosure::default),
            stack: Vec::new(),
            next: Some(root),
            terminal: false,
            _thread: std::marker::PhantomData,
        })
    }

    /// Advances by no more than `max_entries` and stops before beginning another
    /// entry step after `deadline`. One step can require multiple descriptor-
    /// relative observations, and an individual OS call can still overrun the
    /// deadline; process isolation is required to contain such a blocked call.
    pub(crate) fn advance(
        &mut self,
        cancel: &AtomicBool,
        max_entries: usize,
        deadline: Instant,
        mut observe: impl FnMut(&Entry, &Measurement) -> Result<()>,
    ) -> Result<MeasurementProgress> {
        if max_entries == 0 {
            return Err("A measurement quantum must allow at least one entry".into());
        }
        self.advance_guarded(cancel, max_entries, Some(deadline), &mut observe)
    }

    fn advance_unbounded(
        &mut self,
        cancel: &AtomicBool,
        observe: &mut impl FnMut(&Entry, &Measurement) -> Result<()>,
    ) -> Result<MeasurementProgress> {
        self.advance_guarded(cancel, usize::MAX, None, observe)
    }

    fn advance_guarded(
        &mut self,
        cancel: &AtomicBool,
        max_entries: usize,
        deadline: Option<Instant>,
        observe: &mut impl FnMut(&Entry, &Measurement) -> Result<()>,
    ) -> Result<MeasurementProgress> {
        if self.terminal {
            return Err("Measurement cursor is already terminal".into());
        }
        let _local_io = LocalOnlyIo::new()?;
        let outcome = self.advance_inner(cancel, max_entries, deadline, observe);
        if outcome.is_err() {
            self.terminal = true;
        }
        outcome
    }

    fn advance_inner(
        &mut self,
        cancel: &AtomicBool,
        max_entries: usize,
        deadline: Option<Instant>,
        observe: &mut impl FnMut(&Entry, &Measurement) -> Result<()>,
    ) -> Result<MeasurementProgress> {
        let mut processed = 0;
        loop {
            cancelled(cancel)?;
            if let Some(latest_allowed) = self.stop_after_modified_ns
                && self.result.entries > 0
                && (self.result.unsafe_reason.is_some()
                    || self.result.latest_modified_ns > latest_allowed)
            {
                self.result.pruned = true;
                self.result.unsafe_reason.get_or_insert(
                    "Artifact contents changed within the required quiet period".into(),
                );
                return self.finish(cancel).map(MeasurementProgress::Complete);
            }
            if processed >= max_entries
                || deadline.is_some_and(|deadline| Instant::now() >= deadline)
            {
                return Ok(MeasurementProgress::Pending);
            }
            let entry = if let Some(entry) = self.next.take() {
                entry
            } else {
                let Some(directory) = self.stack.last_mut() else {
                    return self.finish(cancel).map(MeasurementProgress::Complete);
                };
                match Directory::next(directory, cancel) {
                    Ok(Some(entry)) => entry,
                    Ok(None) => {
                        if let Err(reason) = directory.unchanged() {
                            self.result.unsafe_reason.get_or_insert(reason);
                        }
                        self.stack.pop();
                        continue;
                    }
                    Err(reason) => {
                        cancelled(cancel)?;
                        self.result.errors += 1;
                        self.result.unsafe_reason.get_or_insert(reason);
                        // A failed read cannot safely be assumed to advance.
                        self.stack.pop();
                        continue;
                    }
                }
            };
            processed += 1;
            let meta = &entry.meta;
            self.result.entries += 1;
            self.result.latest_modified_ns = self
                .result
                .latest_modified_ns
                .max(meta.identity.modified_ns);
            let excluded_name = excluded_measurement_name(
                entry.path.file_name().unwrap_or_default(),
                meta.is_dir(),
                self.policy,
            );
            let mut link_error = None;
            let safe_link = if meta.is_symlink()
                && meta.identity.device == self.device
                && !meta.is_dataless()
                && !excluded_name
            {
                let payload = match self.policy {
                    MeasurementPolicy::Strict => {
                        internal_bin_link(&entry.path, &self.path, self.device)
                    }
                    MeasurementPolicy::Developer => {
                        developer_link_payload(&entry, self.stack.last(), cancel)
                    }
                };
                match payload {
                    Ok(bytes) => Some(bytes),
                    Err(reason) => {
                        cancelled(cancel)?;
                        if self.policy == MeasurementPolicy::Developer {
                            self.result.errors += 1;
                        }
                        link_error = Some(reason);
                        None
                    }
                }
            } else {
                None
            };
            if let Some(digest) = &mut self.digest {
                digest.add(
                    entry.path.strip_prefix(&self.path).unwrap_or(&entry.path),
                    meta,
                    safe_link.as_deref(),
                );
            }
            let exclusion = if meta.identity.device != self.device {
                Some("A nested mount belongs to a different volume")
            } else if meta.is_dataless() {
                Some("Contains cloud placeholders; their contents were not downloaded")
            } else if excluded_name {
                Some("Contains protected, cloud-managed, or shared-store data")
            } else if meta.is_symlink() && safe_link.is_none() {
                Some(match self.policy {
                    MeasurementPolicy::Strict => {
                        "Contains symbolic links outside supported internal .bin commands; linked content has uncertain ownership"
                    }
                    MeasurementPolicy::Developer => link_error
                        .as_deref()
                        .unwrap_or("A symbolic-link payload could not be verified"),
                })
            } else if !meta.is_dir() && !meta.is_file() && safe_link.is_none() {
                Some("Contains special files; cleanup is unsupported")
            } else {
                None
            };
            if let Some(reason) = exclusion {
                self.result.skipped += 1;
                self.result.unsafe_reason.get_or_insert(reason.into());
                observe(&entry, &self.result)?;
                continue;
            }
            if meta.uid != unsafe { libc::geteuid() } {
                self.result
                    .unsafe_reason
                    .get_or_insert("Contains items owned by another account".into());
            }
            if meta.is_symlink() && meta.links != 1 {
                self.result.unsafe_reason.get_or_insert(
                    "Contains hard-linked symbolic links that may be shared outside this artifact"
                        .into(),
                );
            }
            if meta.is_file() {
                self.result.files += 1;
                if let Some(closure) = &mut self.regular_links {
                    if let Err(reason) = closure.observe(meta) {
                        self.result.unsafe_reason.get_or_insert(reason);
                    }
                } else if meta.links > 1 {
                    self.result.unsafe_reason.get_or_insert(
                        "Contains hard-linked files that may be shared outside this artifact"
                            .into(),
                    );
                }
                if self.links.first(meta) {
                    self.result.logical_bytes =
                        self.result.logical_bytes.saturating_add(meta.identity.size);
                    self.result.allocated_bytes =
                        self.result.allocated_bytes.saturating_add(meta.allocated);
                }
            } else if meta.is_dir() {
                self.result.directories += 1;
                if self.stack.len() >= MAX_DEPTH {
                    self.result.skipped += 1;
                    self.result.unsafe_reason.get_or_insert(
                        "Directory depth exceeds the bounded traversal limit".into(),
                    );
                } else {
                    let opened = match self.stack.last() {
                        Some(parent) => Directory::open_child(parent, &entry),
                        None => Directory::open(&entry.path),
                    };
                    match opened {
                        Ok(directory) => self.stack.push(directory),
                        Err(reason) => {
                            self.result.errors += 1;
                            self.result.unsafe_reason.get_or_insert(reason);
                        }
                    }
                }
            }
            observe(&entry, &self.result)?;
        }
    }

    fn finish(&mut self, cancel: &AtomicBool) -> Result<Measurement> {
        cancelled(cancel)?;
        if identity(&self.path)? != self.initial {
            self.result
                .unsafe_reason
                .get_or_insert("The selected item changed during measurement".into());
        }
        if self.links.saturated() {
            self.result.errors += 1;
            self.result.unsafe_reason = Some("Hard-link identity limit reached; size accounting is incomplete and cleanup is excluded".into());
        }
        if !self.result.pruned
            && let Some(closure) = self.regular_links.take()
            && let Err(reason) = closure.verify()
        {
            self.result.unsafe_reason.get_or_insert(reason);
        }
        self.result.fingerprint = if self.result.pruned {
            String::new()
        } else {
            self.digest
                .take()
                .map(|digest| digest.finish())
                .unwrap_or_default()
        };
        self.terminal = true;
        Ok(std::mem::take(&mut self.result))
    }
}

/// Reproducible raw traversal using exactly the production Directory primitive.
/// It has no recommendation-name exclusions. Like `du -x`, it does not follow
/// links or enter another mounted volume. Allocated totals include directory and
/// symlink blocks; logical totals describe regular file data only.
pub fn traverse_metadata(path: &Path, cancel: &AtomicBool) -> Result<crate::model::ScanStats> {
    let _local_io = LocalOnlyIo::new()?;
    use crate::model::ScanStats;
    let started = std::time::Instant::now();
    let mut stats = ScanStats::default();
    if cancelled(cancel).is_err() {
        stats.cancelled = true;
        stats.message = "Metadata traversal cancelled before reading the root".into();
        return Ok(stats);
    }
    let root_meta = metadata(path)?;
    let device = root_meta.identity.device;
    let mut next = Some(Entry {
        path: path.to_path_buf(),
        meta: root_meta,
    });
    let mut stack: Vec<Directory> = Vec::new();
    let mut links = Hardlinks::default();
    loop {
        if cancelled(cancel).is_err() {
            stats.cancelled = true;
            break;
        }
        let entry = if let Some(entry) = next.take() {
            entry
        } else {
            let Some(directory) = stack.last_mut() else {
                break;
            };
            match directory.next(cancel) {
                Ok(Some(entry)) => entry,
                Ok(None) => {
                    if directory.unchanged().is_err() {
                        stats.errors += 1;
                    }
                    stack.pop();
                    continue;
                }
                Err(_) if cancelled(cancel).is_err() => {
                    stats.cancelled = true;
                    break;
                }
                Err(_) => {
                    stats.errors += 1;
                    stack.pop();
                    continue;
                }
            }
        };
        let meta = &entry.meta;
        stats.entries += 1;
        if meta.is_dir() {
            stats.directories += 1;
        }
        if meta.is_file() {
            stats.files += 1;
        }
        if meta.identity.device != device {
            stats.skipped += 1;
            continue;
        }
        if meta.is_dir() || links.first(meta) {
            stats.allocated_bytes = stats.allocated_bytes.saturating_add(meta.allocated);
            if meta.is_file() {
                stats.logical_bytes = stats.logical_bytes.saturating_add(meta.identity.size);
            }
        }
        if meta.is_dir() {
            if stack.len() >= MAX_DEPTH {
                stats.skipped += 1;
                stats.errors += 1;
                continue;
            }
            let opened = match stack.last() {
                Some(parent) => parent.open_child(&entry),
                None => Directory::open(&entry.path),
            };
            match opened {
                Ok(directory) => stack.push(directory),
                Err(_) => {
                    stats.errors += 1;
                    stats.skipped += 1;
                }
            }
        }
    }
    if links.saturated() {
        stats.errors += 1;
    }
    stats.elapsed_ms = started.elapsed().as_millis() as u64;
    stats.complete = !stats.cancelled && stats.errors == 0;
    stats.message = if stats.cancelled {
        "Metadata traversal cancelled; coverage is partial".into()
    } else if links.saturated() {
        "Metadata traversal reached its hard-link accounting limit; totals are partial".into()
    } else if stats.errors > 0 {
        "Metadata traversal encountered inaccessible or changed entries; coverage is partial".into()
    } else {
        "Metadata traversal complete within one local volume".into()
    };
    Ok(stats)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::os::unix::fs::symlink;

    type DirectoryReadObserver = Box<dyn FnMut(&Path)>;
    thread_local! {
        static DIRECTORY_READ_OBSERVER: RefCell<Option<DirectoryReadObserver>> = const { RefCell::new(None) };
    }

    pub(super) fn observe_directory_read(path: &Path) {
        DIRECTORY_READ_OBSERVER.with(|observer| {
            if let Some(callback) = observer.borrow_mut().as_mut() {
                callback(path);
            }
        });
    }

    pub(crate) fn with_directory_read_observer<T>(
        observer: impl FnMut(&Path) + 'static,
        run: impl FnOnce() -> T,
    ) -> T {
        struct RestoreObserver(Option<DirectoryReadObserver>);
        impl Drop for RestoreObserver {
            fn drop(&mut self) {
                DIRECTORY_READ_OBSERVER.with(|observer| observer.replace(self.0.take()));
            }
        }
        let previous = DIRECTORY_READ_OBSERVER.with(|slot| slot.replace(Some(Box::new(observer))));
        let _restore = RestoreObserver(previous);
        run()
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum ScopeMetadataPhase {
        AfterGrantValidation,
        BeforeParentOpen(usize),
        PresentObserved,
        BeforePresenceValidation,
        MissingObserved,
        BeforeAbsenceValidation,
    }

    type ScopeMetadataObserver = Box<dyn FnMut(ScopeMetadataPhase)>;
    thread_local! {
        static SCOPE_METADATA_OBSERVER: RefCell<Option<ScopeMetadataObserver>> = const { RefCell::new(None) };
    }

    pub(super) fn observe_scope_metadata(phase: ScopeMetadataPhase) {
        SCOPE_METADATA_OBSERVER.with(|observer| {
            if let Some(callback) = observer.borrow_mut().as_mut() {
                callback(phase);
            }
        });
    }

    pub(crate) fn with_scope_metadata_observer<T>(
        observer: impl FnMut(ScopeMetadataPhase) + 'static,
        run: impl FnOnce() -> T,
    ) -> T {
        struct RestoreObserver(Option<ScopeMetadataObserver>);
        impl Drop for RestoreObserver {
            fn drop(&mut self) {
                SCOPE_METADATA_OBSERVER.with(|observer| observer.replace(self.0.take()));
            }
        }
        let previous = SCOPE_METADATA_OBSERVER.with(|slot| slot.replace(Some(Box::new(observer))));
        let _restore = RestoreObserver(previous);
        run()
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum RegularReadPhase {
        BeforeOpen,
        AfterChunk(usize),
        BeforePathValidation,
    }

    type RegularReadObserver = Box<dyn FnMut(RegularReadPhase)>;
    thread_local! {
        static REGULAR_READ_OBSERVER: RefCell<Option<RegularReadObserver>> = const { RefCell::new(None) };
    }

    pub(super) fn observe_regular_read(phase: RegularReadPhase) {
        REGULAR_READ_OBSERVER.with(|observer| {
            if let Some(callback) = observer.borrow_mut().as_mut() {
                callback(phase);
            }
        });
    }

    pub(crate) fn with_regular_read_observer<T>(
        observer: impl FnMut(RegularReadPhase) + 'static,
        run: impl FnOnce() -> T,
    ) -> T {
        struct RestoreObserver(Option<RegularReadObserver>);
        impl Drop for RestoreObserver {
            fn drop(&mut self) {
                REGULAR_READ_OBSERVER.with(|observer| observer.replace(self.0.take()));
            }
        }
        let previous = REGULAR_READ_OBSERVER.with(|slot| slot.replace(Some(Box::new(observer))));
        let _restore = RestoreObserver(previous);
        run()
    }

    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let path = std::fs::canonicalize(temp.path()).unwrap();
        (temp, path)
    }

    #[test]
    fn directory_entry_paths_match_join_byte_for_byte() {
        let long_parent = b"/segment".repeat(256);
        let long_name = vec![b'n'; 1023];
        for parent in [
            b"".as_slice(),
            b"/",
            b"/projects",
            b"/projects/",
            b"/projects//./",
            b"relative",
            b"/parent-\xff",
            long_parent.as_slice(),
        ] {
            for name in [
                b"file".as_slice(),
                "café".as_bytes(),
                b"child-\xfe",
                b"",
                b".",
                b"..",
                b"/absolute",
                long_name.as_slice(),
            ] {
                let parent = Path::new(OsStr::from_bytes(parent));
                let name = OsStr::from_bytes(name);
                let actual = directory_entry_path(parent, name);
                let expected = parent.join(name);
                // Path equality normalizes some components; compare the bytes
                // used by fingerprints and the filesystem instead.
                assert_eq!(actual.as_os_str(), expected.as_os_str());
            }
        }
    }

    #[test]
    fn exact_child_reads_only_the_matched_name_and_rechecks_with_a_fresh_reader() {
        let (_temp, base) = fixture();
        let parent = base.join("project");
        std::fs::create_dir(&parent).unwrap();
        for index in 0..300 {
            std::fs::write(parent.join(format!("unrelated-{index}")), b"preserve").unwrap();
        }
        std::fs::create_dir(parent.join("Dropbox")).unwrap();
        std::fs::write(parent.join("Dropbox/preserved"), b"protected sibling").unwrap();
        std::fs::write(parent.join("unrelated-é"), b"unicode name").unwrap();
        symlink("Dropbox", parent.join("unrelated-link")).unwrap();
        let child = parent.join("target");
        std::fs::create_dir(&child).unwrap();
        std::fs::write(child.join("preserved"), b"target contents").unwrap();
        let root = authorize(&base, "projects").unwrap();
        let expected = metadata(&child).unwrap();
        let cancel = AtomicBool::new(false);
        let mut directory = Directory::open(&parent).unwrap();
        directory.force_unknown_types = true;
        let before = CHILD_METADATA_CALLS.with(std::cell::Cell::get);
        assert_eq!(
            directory
                .exact_child_metadata(OsStr::new("target"), &cancel, &|| {})
                .unwrap(),
            Some(expected.clone())
        );
        assert_eq!(CHILD_METADATA_CALLS.with(std::cell::Cell::get) - before, 1);
        let mut fresh = directory.reopen_names(&cancel).unwrap();
        let before = CHILD_METADATA_CALLS.with(std::cell::Cell::get);
        assert!(
            fresh
                .exact_child_metadata(OsStr::new("absent"), &cancel, &|| {})
                .unwrap()
                .is_none()
        );
        assert_eq!(CHILD_METADATA_CALLS.with(std::cell::Cell::get), before);
        let observed_parent = parent.clone();
        with_directory_read_observer(
            move |path| assert_eq!(path, observed_parent, "Only the parent may be enumerated"),
            || {
                let observation = ExactChild::observe(&root, &child, &cancel, &|| {}).unwrap();
                assert_eq!(observation.metadata(), Some(&expected));
                observation.validate(&root, &cancel, &|| {}).unwrap();
                observation.validate(&root, &cancel, &|| {}).unwrap();
            },
        );
        assert_eq!(
            std::fs::read(child.join("preserved")).unwrap(),
            b"target contents"
        );
        assert_eq!(
            std::fs::read(parent.join("Dropbox/preserved")).unwrap(),
            b"protected sibling"
        );
    }

    #[test]
    fn exact_child_case_alias_is_absent_and_case_renames_invalidate_the_guard() {
        let (_temp, base) = fixture();
        let upper = base.join("Target");
        let lower = base.join("target");
        let temporary = base.join("renaming");
        std::fs::create_dir(&upper).unwrap();
        std::fs::write(upper.join("preserved"), b"case-sensitive proof").unwrap();
        let root = authorize(&base, "projects").unwrap();
        let cancel = AtomicBool::new(false);
        let absent = ExactChild::observe(&root, &lower, &cancel, &|| {}).unwrap();
        assert!(absent.metadata().is_none());
        absent.validate(&root, &cancel, &|| {}).unwrap();
        std::fs::rename(&upper, &temporary).unwrap();
        std::fs::rename(&temporary, &lower).unwrap();
        assert!(absent.validate(&root, &cancel, &|| {}).is_err());
        let present = ExactChild::observe(&root, &lower, &cancel, &|| {}).unwrap();
        assert!(present.metadata().unwrap().is_dir());
        std::fs::rename(&lower, &temporary).unwrap();
        std::fs::rename(&temporary, &upper).unwrap();
        assert!(present.validate(&root, &cancel, &|| {}).is_err());
        assert_eq!(
            std::fs::read(upper.join("preserved")).unwrap(),
            b"case-sensitive proof"
        );
    }

    #[test]
    fn exact_child_preserves_file_and_link_types_and_detects_metadata_changes() {
        let (_temp, base) = fixture();
        let parent = base.join("project");
        std::fs::create_dir(&parent).unwrap();
        let child = parent.join("target");
        std::fs::write(&child, b"original").unwrap();
        let root = authorize(&base, "downloads").unwrap();
        let cancel = AtomicBool::new(false);
        let file = ExactChild::observe(&root, &child, &cancel, &|| {}).unwrap();
        assert!(file.metadata().unwrap().is_file());
        file.validate(&root, &cancel, &|| {}).unwrap();
        let parent_before = metadata(&parent).unwrap();
        std::fs::write(&child, b"changed same-inode contents").unwrap();
        assert_eq!(metadata(&parent).unwrap(), parent_before);
        assert!(file.validate(&root, &cancel, &|| {}).is_err());
        std::fs::rename(&child, parent.join("retained")).unwrap();
        let destination = base.join("destination");
        std::fs::write(&destination, b"outside the link").unwrap();
        symlink(&destination, &child).unwrap();
        let link = ExactChild::observe(&root, &child, &cancel, &|| {}).unwrap();
        assert!(link.metadata().unwrap().is_symlink());
        std::fs::write(&destination, b"destination changes do not change the link").unwrap();
        link.validate(&root, &cancel, &|| {}).unwrap();
        assert_eq!(
            std::fs::read(parent.join("retained")).unwrap(),
            b"changed same-inode contents"
        );
    }

    #[test]
    fn exact_child_rejects_detached_or_replaced_ancestry() {
        for moved_component in ["grant", "ancestor", "parent"] {
            let (_temp, base) = fixture();
            let grant = base.join("chosen");
            let ancestor = grant.join("ancestor");
            let parent = ancestor.join("project");
            std::fs::create_dir_all(&parent).unwrap();
            let child = parent.join("target");
            std::fs::write(&child, b"original target").unwrap();
            let root = authorize(&grant, "projects").unwrap();
            let cancel = AtomicBool::new(false);
            let observation = ExactChild::observe(&root, &child, &cancel, &|| {}).unwrap();
            let moved = match moved_component {
                "grant" => &grant,
                "ancestor" => &ancestor,
                _ => &parent,
            };
            let retained = base.join("retained");
            let suffix = child.strip_prefix(moved).unwrap();
            std::fs::rename(moved, &retained).unwrap();
            std::fs::create_dir_all(&parent).unwrap();
            std::fs::write(&child, b"replacement target").unwrap();
            assert!(
                observation.validate(&root, &cancel, &|| {}).is_err(),
                "Accepted replaced {moved_component}"
            );
            assert_eq!(
                std::fs::read(retained.join(suffix)).unwrap(),
                b"original target"
            );
            assert_eq!(std::fs::read(&child).unwrap(), b"replacement target");
        }
    }

    #[test]
    fn exact_child_rejects_namespace_changes_during_initial_and_final_enumeration() {
        for during_validation in [false, true] {
            let (_temp, base) = fixture();
            let parent = base.join("project");
            std::fs::create_dir(&parent).unwrap();
            let child = parent.join("target");
            std::fs::write(&child, b"preserved target").unwrap();
            let root = authorize(&base, "projects").unwrap();
            let cancel = AtomicBool::new(false);
            let observation = during_validation
                .then(|| ExactChild::observe(&root, &child, &cancel, &|| {}).unwrap());
            let moved = child.clone();
            let renamed = parent.join("Target");
            let destination = renamed.clone();
            let result = with_directory_read_observer(
                move |_| std::fs::rename(&moved, &destination).unwrap(),
                || match &observation {
                    Some(observation) => observation.validate(&root, &cancel, &|| {}),
                    None => ExactChild::observe(&root, &child, &cancel, &|| {}).map(|_| ()),
                },
            );
            assert!(result.is_err());
            assert_eq!(std::fs::read(renamed).unwrap(), b"preserved target");
        }
    }

    #[test]
    fn exact_child_missing_parent_requires_a_fresh_absence_proof() {
        let (_temp, base) = fixture();
        let root = authorize(&base, "projects").unwrap();
        let parent = base.join("missing");
        let child = parent.join("target");
        let cancel = AtomicBool::new(false);
        let absent = with_directory_read_observer(
            |_| panic!("A missing parent must not enumerate siblings"),
            || {
                let observation = ExactChild::observe(&root, &child, &cancel, &|| {}).unwrap();
                observation.validate(&root, &cancel, &|| {}).unwrap();
                observation
            },
        );
        assert!(absent.metadata().is_none());
        std::fs::create_dir(&parent).unwrap();
        assert!(absent.validate(&root, &cancel, &|| {}).is_err());
        let empty = ExactChild::observe(&root, &child, &cancel, &|| {}).unwrap();
        assert!(empty.metadata().is_none());
        empty.validate(&root, &cancel, &|| {}).unwrap();
        std::fs::remove_dir(&parent).unwrap();
        assert!(empty.validate(&root, &cancel, &|| {}).is_err());
    }

    #[test]
    fn exact_child_protected_link_and_denied_parents_do_not_prove_absence() {
        use std::os::unix::fs::PermissionsExt;
        let (_temp, base) = fixture();
        let actual = base.join("actual");
        std::fs::create_dir(&actual).unwrap();
        std::fs::write(actual.join("preserved"), b"private contents").unwrap();
        symlink(&actual, base.join("alias")).unwrap();
        std::fs::write(base.join("regular"), b"not a directory").unwrap();
        let root = authorize(&base, "projects").unwrap();
        let cancel = AtomicBool::new(false);
        with_scope_metadata_observer(
            |_| panic!("Protected exact scopes must fail before opening the grant"),
            || {
                assert!(
                    ExactChild::observe(&root, &base.join("Dropbox/target"), &cancel, &|| {})
                        .is_err()
                );
                let invalid = base.join(OsStr::from_bytes(b"target\0suffix"));
                assert!(ExactChild::observe(&root, &invalid, &cancel, &|| {}).is_err());
                assert!(ExactChild::observe(&root, &base, &cancel, &|| {}).is_err());
                let mut deep = base.clone();
                for _ in 0..=MAX_DEPTH {
                    deep.push("component");
                }
                assert!(ExactChild::observe(&root, &deep, &cancel, &|| {}).is_err());
            },
        );
        with_directory_read_observer(
            |_| panic!("Unsafe parents must not receive an enumerator"),
            || {
                for parent in ["alias", "regular"] {
                    assert!(
                        ExactChild::observe(
                            &root,
                            &base.join(parent).join("target"),
                            &cancel,
                            &|| {}
                        )
                        .is_err()
                    );
                }
            },
        );
        let pinned = File::open(&actual).unwrap();
        let permissions = pinned.metadata().unwrap().permissions();
        pinned
            .set_permissions(std::fs::Permissions::from_mode(0o0))
            .unwrap();
        let denied = ExactChild::observe(&root, &actual.join("target"), &cancel, &|| {});
        pinned.set_permissions(permissions).unwrap();
        if unsafe { libc::geteuid() } != 0 {
            assert!(
                denied.is_err(),
                "A denied directory cannot prove child absence"
            );
        }
        assert_eq!(
            std::fs::read(actual.join("preserved")).unwrap(),
            b"private contents"
        );
    }

    #[test]
    fn exact_child_cancellation_is_checked_before_io_and_between_name_chunks() {
        let (_temp, base) = fixture();
        for index in 0..600 {
            std::fs::write(base.join(format!("file-{index}")), b"preserve").unwrap();
        }
        let root = authorize(&base, "projects").unwrap();
        let child = base.join("target");
        with_scope_metadata_observer(
            |_| panic!("Pre-cancelled exact lookup must not open the grant"),
            || {
                let result = ExactChild::observe(&root, &child, &AtomicBool::new(true), &|| {});
                assert_eq!(result.err().as_deref(), Some("Cancelled"));
            },
        );
        let cancel = AtomicBool::new(false);
        let checkpoints = std::cell::Cell::new(0);
        let result = ExactChild::observe(&root, &child, &cancel, &|| {
            let count = checkpoints.get() + 1;
            checkpoints.set(count);
            if count == 3 {
                cancel.store(true, Ordering::Release);
            }
        });
        assert_eq!(checkpoints.get(), 3);
        assert_eq!(result.err().as_deref(), Some("Cancelled"));
        cancel.store(false, Ordering::Release);
        let observation = ExactChild::observe(&root, &child, &cancel, &|| {}).unwrap();
        with_scope_metadata_observer(
            |_| panic!("Pre-cancelled validation must not open the grant"),
            || {
                assert_eq!(
                    observation
                        .validate(&root, &AtomicBool::new(true), &|| {})
                        .unwrap_err(),
                    "Cancelled"
                );
            },
        );
    }

    #[test]
    fn bounded_evidence_read_rejects_over_limit_before_opening_or_allocating() {
        let (_temp, base) = fixture();
        let path = base.join("package.json");
        let cancel = AtomicBool::new(false);
        std::fs::write(&path, b"12345678").unwrap();
        assert_eq!(
            read_regular_bounded(&path, &cancel, 8).unwrap().bytes,
            b"12345678"
        );
        with_regular_read_observer(
            |_| panic!("over-limit evidence must be rejected before open/read"),
            || assert!(read_regular_bounded(&path, &cancel, 7).is_err()),
        );
        assert!(read_regular_bounded(&path, &cancel, 0).is_err());
        assert!(read_regular_bounded(&path, &cancel, MAX_MANIFEST + 1).is_err());
        let changed_path = path.clone();
        let result = with_regular_read_observer(
            move |phase| {
                if matches!(phase, RegularReadPhase::BeforeOpen) {
                    std::fs::write(&changed_path, b"123456789").unwrap();
                }
            },
            || read_regular_bounded(&path, &cancel, 8),
        );
        assert!(
            result.is_err(),
            "growth after the size check must fail closed"
        );
    }

    #[test]
    fn scope_metadata_reads_existing_and_missing_paths_without_enumeration() {
        let (_temp, base) = fixture();
        let parent = base.join("parent");
        std::fs::create_dir(&parent).unwrap();
        let preserved = parent.join("preserved");
        std::fs::write(&preserved, b"unchanged sibling").unwrap();
        let root = authorize(&base, "projects").unwrap();
        let cancel = AtomicBool::new(false);
        let expected = metadata(&preserved).unwrap();
        with_directory_read_observer(
            |_| panic!("Scope metadata must not enumerate any directory"),
            || {
                assert_eq!(
                    scope_metadata(&root, &base, &cancel).unwrap(),
                    Some(metadata(&base).unwrap())
                );
                assert_eq!(
                    scope_metadata(&root, &preserved, &cancel).unwrap(),
                    Some(expected)
                );
                assert!(
                    scope_metadata(&root, &parent.join("absent"), &cancel)
                        .unwrap()
                        .is_none()
                );
                assert!(
                    scope_metadata(&root, &base.join("absent/child/leaf"), &cancel)
                        .unwrap()
                        .is_none()
                );
            },
        );
        assert_eq!(std::fs::read(&preserved).unwrap(), b"unchanged sibling");
    }

    #[test]
    fn scope_metadata_retries_brief_contents_changes_without_enumeration() {
        for directory in [false, true] {
            let (_temp, base) = fixture();
            let requested = base.join("active");
            if directory {
                std::fs::create_dir(&requested).unwrap();
            } else {
                std::fs::write(&requested, b"active file").unwrap();
            }
            let preserved = base.join("preserved");
            std::fs::write(&preserved, b"unchanged sibling").unwrap();
            let root = authorize(&base, "projects").unwrap();
            let changed = requested.clone();
            let attempts = std::rc::Rc::new(std::cell::Cell::new(0));
            let observed = attempts.clone();
            let result = with_directory_read_observer(
                |_| panic!("Retrying scope metadata must not enumerate directories"),
                || {
                    with_scope_metadata_observer(
                        move |phase| {
                            if phase == ScopeMetadataPhase::PresentObserved {
                                observed.set(observed.get() + 1);
                                if observed.get() == 1 {
                                    File::open(&changed)
                                        .unwrap()
                                        .set_times(std::fs::FileTimes::new().set_modified(
                                            std::time::UNIX_EPOCH
                                                + std::time::Duration::from_secs(1),
                                        ))
                                        .unwrap();
                                }
                            }
                        },
                        || scope_metadata(&root, &requested, &AtomicBool::new(false)),
                    )
                },
            );
            assert_eq!(attempts.get(), 2);
            assert_eq!(result.unwrap(), Some(metadata(&requested).unwrap()));
            assert_eq!(std::fs::read(preserved).unwrap(), b"unchanged sibling");
        }
    }

    #[test]
    fn scope_metadata_bounds_continuous_contents_changes() {
        let (_temp, base) = fixture();
        let requested = base.join("active");
        std::fs::create_dir(&requested).unwrap();
        let root = authorize(&base, "projects").unwrap();
        let changed = requested.clone();
        let attempts = std::rc::Rc::new(std::cell::Cell::new(0));
        let observed = attempts.clone();
        let result = with_scope_metadata_observer(
            move |phase| {
                if phase == ScopeMetadataPhase::PresentObserved {
                    observed.set(observed.get() + 1);
                    File::open(&changed)
                        .unwrap()
                        .set_times(std::fs::FileTimes::new().set_modified(
                            std::time::UNIX_EPOCH
                                + std::time::Duration::from_secs(observed.get() as u64),
                        ))
                        .unwrap();
                }
            },
            || scope_metadata(&root, &requested, &AtomicBool::new(false)),
        );
        assert_eq!(result.unwrap_err(), SCOPE_CONTENTS_CHANGED);
        assert_eq!(attempts.get(), SCOPE_OBSERVATION_ATTEMPTS);
    }

    #[test]
    fn scope_metadata_does_not_retry_contents_changes_with_namespace_changes() {
        let (_temp, base) = fixture();
        let requested = base.join("requested");
        std::fs::write(&requested, b"original contents").unwrap();
        let root = authorize(&base, "projects").unwrap();
        let changed = requested.clone();
        let parent = base.clone();
        let result = with_scope_metadata_observer(
            move |phase| {
                if phase == ScopeMetadataPhase::PresentObserved {
                    std::fs::write(&changed, b"updated contents").unwrap();
                    // Even unchanged dev/inode on the leaf is not sufficient
                    // when its containing namespace changed after opening.
                    std::fs::write(parent.join("new-sibling"), b"preserved sibling").unwrap();
                    File::open(&parent)
                        .unwrap()
                        .set_times(std::fs::FileTimes::new().set_modified(
                            std::time::UNIX_EPOCH + std::time::Duration::from_secs(1),
                        ))
                        .unwrap();
                }
            },
            || scope_metadata(&root, &requested, &AtomicBool::new(false)),
        );
        assert_ne!(result.unwrap_err(), SCOPE_CONTENTS_CHANGED);
        assert_eq!(std::fs::read(requested).unwrap(), b"updated contents");
        assert_eq!(
            std::fs::read(base.join("new-sibling")).unwrap(),
            b"preserved sibling"
        );
    }

    #[test]
    fn scope_metadata_retry_keeps_replacement_and_cancellation_strict() {
        for cancel_retry in [false, true] {
            let (_temp, base) = fixture();
            let requested = base.join("requested");
            std::fs::write(&requested, b"original contents").unwrap();
            let original = base.join("original");
            let root = authorize(&base, "projects").unwrap();
            let changed = requested.clone();
            let destination = original.clone();
            let cancel = std::sync::Arc::new(AtomicBool::new(false));
            let requested_cancel = cancel.clone();
            let mut attempts = 0;
            let result = with_scope_metadata_observer(
                move |phase| {
                    if phase == ScopeMetadataPhase::PresentObserved {
                        attempts += 1;
                        if attempts == 1 {
                            std::fs::write(&changed, b"original contents, updated").unwrap();
                        } else if cancel_retry {
                            requested_cancel.store(true, Ordering::Release);
                        } else {
                            std::fs::rename(&changed, &destination).unwrap();
                            std::fs::write(&changed, b"replacement contents").unwrap();
                        }
                    }
                },
                || scope_metadata(&root, &requested, &cancel),
            );
            let error = result.unwrap_err();
            assert_ne!(error, SCOPE_CONTENTS_CHANGED);
            if cancel_retry {
                assert_eq!(error, "Cancelled");
                assert_eq!(
                    std::fs::read(requested).unwrap(),
                    b"original contents, updated"
                );
            } else {
                assert_eq!(
                    std::fs::read(original).unwrap(),
                    b"original contents, updated"
                );
                assert_eq!(std::fs::read(requested).unwrap(), b"replacement contents");
            }
        }
    }

    #[test]
    fn ancestor_scope_metadata_binds_existing_and_missing_descendants_without_enumeration() {
        let (_temp, base) = fixture();
        let artifact_path = base.join("project/target");
        std::fs::create_dir_all(artifact_path.join("deep")).unwrap();
        let witness = artifact_path.join("deep/preserved");
        std::fs::write(&witness, b"unchanged descendant").unwrap();
        let root = authorize(&base, "projects").unwrap();
        let artifact = Entry {
            path: artifact_path,
            meta: metadata(&base.join("project/target")).unwrap(),
        };
        let cancel = AtomicBool::new(false);
        let checkpoints = std::cell::Cell::new(0);
        let checkpoint = || checkpoints.set(checkpoints.get() + 1);
        with_directory_read_observer(
            |_| panic!("A witness proof must not enumerate siblings or artifact contents"),
            || {
                assert_eq!(
                    scope_metadata_with_ancestor(&root, &witness, &artifact, &cancel, &checkpoint)
                        .unwrap(),
                    Some(metadata(&witness).unwrap())
                );
                for absent in ["deep/absent", "absent/child"] {
                    assert!(
                        scope_metadata_with_ancestor(
                            &root,
                            &artifact.path.join(absent),
                            &artifact,
                            &cancel,
                            &checkpoint,
                        )
                        .unwrap()
                        .is_none()
                    );
                }
            },
        );
        assert!(
            checkpoints.get() > 1,
            "The bounded walk must offer cleanup checkpoints"
        );
        with_scope_metadata_observer(
            |_| panic!("An invalid witness boundary must fail before opening the grant"),
            || {
                for path in [&artifact.path, &base.join("outside/preserved")] {
                    assert!(
                        scope_metadata_with_ancestor(&root, path, &artifact, &cancel, &|| {})
                            .is_err()
                    );
                }
            },
        );
        assert_eq!(std::fs::read(witness).unwrap(), b"unchanged descendant");
    }

    #[test]
    fn ancestor_scope_metadata_requires_full_metadata_at_open_and_final_proof() {
        for phase in [
            ScopeMetadataPhase::BeforeParentOpen(1),
            ScopeMetadataPhase::PresentObserved,
            ScopeMetadataPhase::BeforePresenceValidation,
        ] {
            let (_temp, base) = fixture();
            let path = base.join("project/target");
            std::fs::create_dir_all(path.join("deep")).unwrap();
            let witness = path.join("deep/preserved");
            std::fs::write(&witness, b"same file and ancestor inode").unwrap();
            let root = authorize(&base, "projects").unwrap();
            let artifact = Entry {
                path: path.clone(),
                meta: metadata(&path).unwrap(),
            };
            let changed = path.clone();
            let result = with_scope_metadata_observer(
                move |observed| {
                    if observed == phase {
                        File::open(&changed)
                            .unwrap()
                            .set_times(std::fs::FileTimes::new().set_modified(
                                std::time::UNIX_EPOCH + std::time::Duration::from_secs(1),
                            ))
                            .unwrap();
                    }
                },
                || {
                    scope_metadata_with_ancestor(
                        &root,
                        &witness,
                        &artifact,
                        &AtomicBool::new(false),
                        &|| {},
                    )
                },
            );
            assert!(same_object(
                &artifact.meta.identity,
                &metadata(&path).unwrap().identity
            ));
            assert_ne!(artifact.meta, metadata(&path).unwrap());
            assert!(
                result.is_err(),
                "Accepted changed ancestor metadata at {phase:?}"
            );
            assert_eq!(
                scope_metadata(&root, &witness, &AtomicBool::new(false)).unwrap(),
                Some(metadata(&witness).unwrap())
            );
            assert_eq!(
                std::fs::read(witness).unwrap(),
                b"same file and ancestor inode"
            );
        }

        let (_temp, base) = fixture();
        let path = base.join("target");
        std::fs::create_dir(&path).unwrap();
        let witness = path.join("preserved");
        std::fs::write(&witness, b"full metadata binding").unwrap();
        let root = authorize(&base, "projects").unwrap();
        let mut artifact = Entry {
            path: path.clone(),
            meta: metadata(&path).unwrap(),
        };
        // Allocated bytes are outside Identity; comparing only Identity would
        // accept this mismatched selection even though the contract is EntryMeta.
        artifact.meta.allocated = artifact.meta.allocated.wrapping_add(512);
        assert!(
            scope_metadata_with_ancestor(
                &root,
                &witness,
                &artifact,
                &AtomicBool::new(false),
                &|| {}
            )
            .is_err()
        );
        assert_eq!(std::fs::read(witness).unwrap(), b"full metadata binding");
    }

    #[test]
    fn ancestor_scope_metadata_rejects_replaced_and_detached_ancestry() {
        for (moved_relative, phase, replace) in [
            (
                "project/target",
                ScopeMetadataPhase::BeforeParentOpen(1),
                true,
            ),
            ("project", ScopeMetadataPhase::PresentObserved, true),
            (
                "project",
                ScopeMetadataPhase::BeforePresenceValidation,
                false,
            ),
            (
                "project/target/deep",
                ScopeMetadataPhase::PresentObserved,
                true,
            ),
        ] {
            let (_temp, base) = fixture();
            let path = base.join("project/target");
            std::fs::create_dir_all(path.join("deep")).unwrap();
            let witness = path.join("deep/preserved");
            std::fs::write(&witness, b"original descendant").unwrap();
            let root = authorize(&base, "projects").unwrap();
            let artifact = Entry {
                path: path.clone(),
                meta: metadata(&path).unwrap(),
            };
            let moved = base.join(moved_relative);
            let suffix = witness.strip_prefix(&moved).unwrap().to_path_buf();
            let detached = base.join("detached");
            let preserved = detached.join(&suffix);
            let replacement = witness.clone();
            let result = with_scope_metadata_observer(
                move |observed| {
                    if observed == phase {
                        std::fs::rename(&moved, &detached).unwrap();
                        if replace {
                            std::fs::create_dir_all(replacement.parent().unwrap()).unwrap();
                            std::fs::write(&replacement, b"replacement descendant").unwrap();
                        }
                    }
                },
                || {
                    scope_metadata_with_ancestor(
                        &root,
                        &witness,
                        &artifact,
                        &AtomicBool::new(false),
                        &|| {},
                    )
                },
            );
            assert!(result.is_err(), "Accepted detached ancestry at {phase:?}");
            assert_eq!(std::fs::read(preserved).unwrap(), b"original descendant");
            if replace {
                assert_eq!(std::fs::read(witness).unwrap(), b"replacement descendant");
            }
        }
    }

    #[test]
    fn ancestor_scope_metadata_revalidates_after_a_cleanup_checkpoint() {
        let (_temp, base) = fixture();
        let project = base.join("project");
        let path = project.join("target");
        std::fs::create_dir_all(path.join("deep")).unwrap();
        let witness = path.join("deep/preserved");
        std::fs::write(&witness, b"original during pause").unwrap();
        let root = authorize(&base, "projects").unwrap();
        let artifact = Entry {
            path: path.clone(),
            meta: metadata(&path).unwrap(),
        };
        let pending = std::rc::Rc::new(std::cell::Cell::new(false));
        let arm_pause = pending.clone();
        let paused = std::cell::Cell::new(false);
        let detached = base.join("detached");
        let result = with_scope_metadata_observer(
            move |phase| {
                // The selected artifact is pinned before opening its child.
                if phase == ScopeMetadataPhase::BeforeParentOpen(2) {
                    arm_pause.set(true);
                }
            },
            || {
                scope_metadata_with_ancestor(
                    &root,
                    &witness,
                    &artifact,
                    &AtomicBool::new(false),
                    &|| {
                        if pending.replace(false) {
                            paused.set(true);
                            std::fs::rename(&project, &detached).unwrap();
                            std::fs::create_dir_all(path.join("deep")).unwrap();
                            std::fs::write(&witness, b"replacement during pause").unwrap();
                        }
                    },
                )
            },
        );
        assert!(paused.get());
        assert!(
            result.is_err(),
            "A pause must not return a detached witness"
        );
        assert_eq!(
            std::fs::read(detached.join("target/deep/preserved")).unwrap(),
            b"original during pause"
        );
        assert_eq!(std::fs::read(witness).unwrap(), b"replacement during pause");
    }

    #[test]
    fn ancestor_scope_metadata_keeps_links_and_protected_names_out_of_ancestry() {
        let (_temp, base) = fixture();
        let path = base.join("target");
        let outside = base.join("outside");
        std::fs::create_dir(&path).unwrap();
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("preserved"), b"outside linked data").unwrap();
        symlink(&outside, path.join("directory-link")).unwrap();
        symlink(outside.join("preserved"), path.join("leaf-link")).unwrap();
        std::fs::write(path.join("regular"), b"not a directory").unwrap();
        let root = authorize(&base, "projects").unwrap();
        let artifact = Entry {
            path: path.clone(),
            meta: metadata(&path).unwrap(),
        };
        let cancel = AtomicBool::new(false);
        for suffix in ["directory-link/preserved", "regular/preserved"] {
            assert!(
                scope_metadata_with_ancestor(&root, &path.join(suffix), &artifact, &cancel, &|| {})
                    .is_err()
            );
        }
        let leaf = scope_metadata_with_ancestor(
            &root,
            &path.join("leaf-link"),
            &artifact,
            &cancel,
            &|| {},
        )
        .unwrap()
        .unwrap();
        assert!(
            leaf.is_symlink(),
            "The caller must receive the link's metadata, never its target's"
        );
        with_scope_metadata_observer(
            |_| panic!("Protected witness names must fail before opening the grant"),
            || {
                for suffix in ["Dropbox/preserved", ".git/preserved", "placeholder.icloud"] {
                    assert!(
                        scope_metadata_with_ancestor(
                            &root,
                            &path.join(suffix),
                            &artifact,
                            &cancel,
                            &|| {}
                        )
                        .is_err()
                    );
                }
            },
        );
        let mut unsafe_artifact = Entry {
            path: path.clone(),
            meta: artifact.meta.clone(),
        };
        unsafe_artifact.meta.flags |= 0x4000_0000;
        assert!(
            scope_metadata_with_ancestor(
                &root,
                &path.join("leaf-link"),
                &unsafe_artifact,
                &cancel,
                &|| {}
            )
            .is_err()
        );
        unsafe_artifact.meta = artifact.meta.clone();
        unsafe_artifact.meta.identity.device = root.identity.device.wrapping_add(1);
        assert!(
            scope_metadata_with_ancestor(
                &root,
                &path.join("leaf-link"),
                &unsafe_artifact,
                &cancel,
                &|| {}
            )
            .is_err()
        );
        assert_eq!(
            std::fs::read(outside.join("preserved")).unwrap(),
            b"outside linked data"
        );
    }

    #[test]
    fn ancestor_scope_metadata_cancels_during_walk_and_final_proof() {
        let (_temp, base) = fixture();
        let path = base.join("project/target");
        std::fs::create_dir_all(path.join("deep")).unwrap();
        let witness = path.join("deep/preserved");
        std::fs::write(&witness, b"preserved across cancellation").unwrap();
        let root = authorize(&base, "projects").unwrap();
        let artifact = Entry {
            path: path.clone(),
            meta: metadata(&path).unwrap(),
        };
        assert_eq!(
            scope_metadata_with_ancestor(
                &root,
                &witness,
                &artifact,
                &AtomicBool::new(true),
                &|| panic!("Pre-cancelled proof must not pause")
            )
            .unwrap_err(),
            "Cancelled"
        );
        for phase in [
            ScopeMetadataPhase::BeforeParentOpen(1),
            ScopeMetadataPhase::BeforePresenceValidation,
        ] {
            let cancel = std::sync::Arc::new(AtomicBool::new(false));
            let requested = cancel.clone();
            let confirming = std::rc::Rc::new(std::cell::Cell::new(false));
            let observed_confirmation = confirming.clone();
            let result = with_scope_metadata_observer(
                move |observed| {
                    if observed == ScopeMetadataPhase::PresentObserved {
                        observed_confirmation.set(true);
                    }
                    if observed == phase {
                        requested.store(true, Ordering::Release);
                    }
                },
                || {
                    scope_metadata_with_ancestor(&root, &witness, &artifact, &cancel, &|| {
                        assert!(
                            !confirming.get(),
                            "Cleanup must not pause partway through the final proof"
                        );
                    })
                },
            );
            assert_eq!(result.unwrap_err(), "Cancelled");
        }
        assert_eq!(
            std::fs::read(witness).unwrap(),
            b"preserved across cancellation"
        );
    }

    #[test]
    fn scope_metadata_confirms_parent_removed_between_stat_and_open() {
        let (_temp, base) = fixture();
        let parent = base.join("transient");
        std::fs::create_dir(&parent).unwrap();
        let preserved = base.join("preserved");
        std::fs::write(&preserved, b"unchanged sibling").unwrap();
        let root = authorize(&base, "projects").unwrap();
        let remove = parent.clone();
        let result = with_scope_metadata_observer(
            move |phase| {
                if phase == ScopeMetadataPhase::BeforeParentOpen(0) {
                    std::fs::remove_dir(&remove).unwrap();
                }
            },
            || scope_metadata(&root, &parent.join("child"), &AtomicBool::new(false)),
        );
        assert!(result.unwrap().is_none());
        assert!(!parent.exists());
        assert_eq!(std::fs::read(&preserved).unwrap(), b"unchanged sibling");
    }

    #[test]
    fn scope_metadata_rejects_absence_inside_a_detached_or_replaced_parent() {
        for (phase, replace) in [
            (ScopeMetadataPhase::MissingObserved, false),
            (ScopeMetadataPhase::MissingObserved, true),
            (ScopeMetadataPhase::BeforeAbsenceValidation, true),
        ] {
            let (_temp, base) = fixture();
            let parent = base.join("parent");
            std::fs::create_dir(&parent).unwrap();
            std::fs::write(parent.join("preserved"), b"original parent").unwrap();
            let root = authorize(&base, "projects").unwrap();
            let detached = base.join("detached");
            let moved = parent.clone();
            let destination = detached.clone();
            let result = with_scope_metadata_observer(
                move |observed| {
                    if observed == phase {
                        std::fs::rename(&moved, &destination).unwrap();
                        if replace {
                            std::fs::create_dir(&moved).unwrap();
                            std::fs::write(moved.join("requested"), b"replacement child").unwrap();
                        }
                    }
                },
                || scope_metadata(&root, &parent.join("requested"), &AtomicBool::new(false)),
            );
            assert!(result.is_err(), "Accepted a detached parent at {phase:?}");
            assert_eq!(
                std::fs::read(detached.join("preserved")).unwrap(),
                b"original parent"
            );
            if replace {
                assert_eq!(
                    std::fs::read(parent.join("requested")).unwrap(),
                    b"replacement child"
                );
            }
        }
    }

    #[test]
    fn scope_metadata_rejects_replaced_or_missing_grants() {
        for phase in [
            ScopeMetadataPhase::AfterGrantValidation,
            ScopeMetadataPhase::MissingObserved,
            ScopeMetadataPhase::BeforeAbsenceValidation,
        ] {
            let (_temp, base) = fixture();
            let chosen = base.join("chosen");
            std::fs::create_dir(&chosen).unwrap();
            std::fs::write(chosen.join("preserved"), b"original grant").unwrap();
            let root = authorize(&chosen, "projects").unwrap();
            let moved = chosen.clone();
            let detached = base.join("detached");
            let destination = detached.clone();
            let result = with_scope_metadata_observer(
                move |observed| {
                    if observed == phase {
                        std::fs::rename(&moved, &destination).unwrap();
                        std::fs::create_dir(&moved).unwrap();
                        std::fs::write(moved.join("requested"), b"replacement grant").unwrap();
                    }
                },
                || scope_metadata(&root, &chosen.join("requested"), &AtomicBool::new(false)),
            );
            assert!(result.is_err(), "Accepted a replaced grant at {phase:?}");
            assert_eq!(
                std::fs::read(detached.join("preserved")).unwrap(),
                b"original grant"
            );
            assert_eq!(
                std::fs::read(chosen.join("requested")).unwrap(),
                b"replacement grant"
            );
        }
        let (_temp, base) = fixture();
        let chosen = base.join("chosen");
        std::fs::create_dir(&chosen).unwrap();
        let root = authorize(&chosen, "projects").unwrap();
        std::fs::remove_dir(&chosen).unwrap();
        assert!(scope_metadata(&root, &chosen, &AtomicBool::new(false)).is_err());
        assert!(scope_metadata(&root, &chosen.join("child"), &AtomicBool::new(false)).is_err());
    }

    #[test]
    fn scope_metadata_revalidates_existing_leaf_against_a_moved_grant() {
        for phase in [
            ScopeMetadataPhase::BeforeParentOpen(0),
            ScopeMetadataPhase::PresentObserved,
            ScopeMetadataPhase::BeforePresenceValidation,
        ] {
            let (_temp, base) = fixture();
            let chosen = base.join("chosen");
            std::fs::create_dir_all(chosen.join("parent")).unwrap();
            std::fs::write(chosen.join("parent/requested"), b"original contents").unwrap();
            let root = authorize(&chosen, "projects").unwrap();
            let moved = chosen.clone();
            let detached = base.join("detached");
            let destination = detached.clone();
            let result = with_scope_metadata_observer(
                move |observed| {
                    if observed == phase {
                        std::fs::rename(&moved, &destination).unwrap();
                    }
                },
                || {
                    scope_metadata(
                        &root,
                        &chosen.join("parent/requested"),
                        &AtomicBool::new(false),
                    )
                },
            );
            assert!(
                result.is_err(),
                "Accepted detached grant metadata at {phase:?}"
            );
            assert_eq!(
                std::fs::read(detached.join("parent/requested")).unwrap(),
                b"original contents"
            );
        }
    }

    #[test]
    fn scope_metadata_revalidates_existing_leaf_against_a_moved_parent() {
        for phase in [
            ScopeMetadataPhase::PresentObserved,
            ScopeMetadataPhase::BeforePresenceValidation,
        ] {
            for replace in [false, true] {
                let (_temp, base) = fixture();
                let parent = base.join("parent");
                std::fs::create_dir(&parent).unwrap();
                std::fs::write(parent.join("requested"), b"original contents").unwrap();
                let root = authorize(&base, "projects").unwrap();
                let moved = parent.clone();
                let detached = base.join("detached");
                let destination = detached.clone();
                let result = with_scope_metadata_observer(
                    move |observed| {
                        if observed == phase {
                            std::fs::rename(&moved, &destination).unwrap();
                            if replace {
                                std::fs::create_dir(&moved).unwrap();
                                std::fs::write(moved.join("requested"), b"replacement contents")
                                    .unwrap();
                            }
                        }
                    },
                    || scope_metadata(&root, &parent.join("requested"), &AtomicBool::new(false)),
                );
                assert!(
                    result.is_err(),
                    "Accepted detached parent metadata at {phase:?}"
                );
                assert_eq!(
                    std::fs::read(detached.join("requested")).unwrap(),
                    b"original contents"
                );
                if replace {
                    assert_eq!(
                        std::fs::read(parent.join("requested")).unwrap(),
                        b"replacement contents"
                    );
                }
            }
        }
    }

    #[test]
    fn scope_metadata_rejects_existing_leaf_substitution() {
        for phase in [
            ScopeMetadataPhase::PresentObserved,
            ScopeMetadataPhase::BeforePresenceValidation,
        ] {
            let (_temp, base) = fixture();
            let requested = base.join("requested");
            std::fs::write(&requested, b"original contents").unwrap();
            let root = authorize(&base, "projects").unwrap();
            let replaced = requested.clone();
            let original = base.join("original");
            let destination = original.clone();
            let result = with_scope_metadata_observer(
                move |observed| {
                    if observed == phase {
                        std::fs::rename(&replaced, &destination).unwrap();
                        std::fs::write(&replaced, b"replacement contents").unwrap();
                    }
                },
                || scope_metadata(&root, &requested, &AtomicBool::new(false)),
            );
            assert!(
                result.is_err(),
                "Accepted substituted leaf metadata at {phase:?}"
            );
            assert_eq!(std::fs::read(&requested).unwrap(), b"replacement contents");
            assert_eq!(std::fs::read(original).unwrap(), b"original contents");
        }
    }

    #[test]
    fn scope_metadata_confirms_one_disappearance_after_existing_observation() {
        for reappear in [false, true] {
            let (_temp, base) = fixture();
            let requested = base.join("requested");
            let preserved = base.join("preserved");
            std::fs::write(&requested, b"transient contents").unwrap();
            std::fs::write(&preserved, b"unchanged sibling").unwrap();
            let root = authorize(&base, "projects").unwrap();
            let changed = requested.clone();
            let missing = std::rc::Rc::new(std::cell::Cell::new(0));
            let observed_missing = missing.clone();
            let result = with_scope_metadata_observer(
                move |phase| match phase {
                    ScopeMetadataPhase::PresentObserved => std::fs::remove_file(&changed).unwrap(),
                    ScopeMetadataPhase::MissingObserved => {
                        observed_missing.set(observed_missing.get() + 1);
                        if reappear {
                            std::fs::write(&changed, b"new contents").unwrap();
                        }
                    }
                    _ => {}
                },
                || scope_metadata(&root, &requested, &AtomicBool::new(false)),
            );
            assert_eq!(
                missing.get(),
                1,
                "A disappearance must not trigger a retry loop"
            );
            if reappear {
                assert!(result.is_err());
                assert_eq!(std::fs::read(requested).unwrap(), b"new contents");
            } else {
                assert!(result.unwrap().is_none());
                assert!(!requested.exists());
            }
            assert_eq!(std::fs::read(preserved).unwrap(), b"unchanged sibling");
        }
    }

    #[test]
    fn scope_metadata_rejects_a_scope_that_reappears_during_absence_proof() {
        for phase in [
            ScopeMetadataPhase::MissingObserved,
            ScopeMetadataPhase::BeforeAbsenceValidation,
        ] {
            let (_temp, base) = fixture();
            let root = authorize(&base, "projects").unwrap();
            let requested = base.join("requested");
            let created = requested.clone();
            let result = with_scope_metadata_observer(
                move |observed| {
                    if observed == phase {
                        std::fs::write(&created, b"new contents").unwrap();
                    }
                },
                || scope_metadata(&root, &requested, &AtomicBool::new(false)),
            );
            assert!(result.is_err(), "Accepted reappearance at {phase:?}");
            assert_eq!(std::fs::read(requested).unwrap(), b"new contents");
        }
    }

    #[test]
    fn scope_metadata_preserves_link_cloud_and_parent_type_boundaries() {
        let (_temp, base) = fixture();
        let actual = base.join("actual");
        std::fs::create_dir(&actual).unwrap();
        std::fs::write(actual.join("preserved"), b"outside link contents").unwrap();
        symlink(&actual, base.join("alias")).unwrap();
        symlink("absent-target", base.join("leaf-link")).unwrap();
        std::fs::write(base.join("regular"), b"regular parent").unwrap();
        let root = authorize(&base, "projects").unwrap();
        let cancel = AtomicBool::new(false);
        assert!(scope_metadata(&root, &base.join("alias/absent"), &cancel).is_err());
        assert!(scope_metadata(&root, &base.join("regular/absent"), &cancel).is_err());
        assert!(
            scope_metadata(&root, &base.join("leaf-link"), &cancel)
                .unwrap()
                .unwrap()
                .is_symlink()
        );
        with_scope_metadata_observer(
            |_| panic!("Protected scope names must fail before opening the grant"),
            || assert!(scope_metadata(&root, &base.join("Dropbox/absent"), &cancel).is_err()),
        );
        let mut parent = metadata(&actual).unwrap();
        parent.flags |= 0x4000_0000;
        assert!(check_scope_parent(&parent, root.identity.device).is_err());
        parent.flags = 0;
        assert!(check_scope_parent(&parent, root.identity.device.wrapping_add(1)).is_err());
        assert_eq!(
            std::fs::read(actual.join("preserved")).unwrap(),
            b"outside link contents"
        );
        assert_eq!(
            std::fs::read(base.join("regular")).unwrap(),
            b"regular parent"
        );
    }

    #[test]
    fn scope_metadata_rejects_a_parent_replaced_by_a_link_before_open() {
        let (_temp, base) = fixture();
        let parent = base.join("parent");
        let actual = base.join("actual");
        std::fs::create_dir(&parent).unwrap();
        std::fs::create_dir(&actual).unwrap();
        std::fs::write(actual.join("requested"), b"preserved link target").unwrap();
        let root = authorize(&base, "projects").unwrap();
        let replaced = parent.clone();
        let target = actual.clone();
        let result = with_scope_metadata_observer(
            move |phase| {
                if phase == ScopeMetadataPhase::BeforeParentOpen(0) {
                    std::fs::remove_dir(&replaced).unwrap();
                    symlink(&target, &replaced).unwrap();
                }
            },
            || scope_metadata(&root, &parent.join("requested"), &AtomicBool::new(false)),
        );
        assert!(result.is_err());
        assert_eq!(
            std::fs::read(actual.join("requested")).unwrap(),
            b"preserved link target"
        );
    }

    #[test]
    fn scope_metadata_keeps_permission_failures_distinct_from_absence() {
        use std::os::unix::fs::PermissionsExt;
        let (_temp, base) = fixture();
        let parent = base.join("denied");
        std::fs::create_dir(&parent).unwrap();
        std::fs::write(parent.join("preserved"), b"private contents").unwrap();
        let root = authorize(&base, "projects").unwrap();
        let pinned = File::open(&parent).unwrap();
        let permissions = pinned.metadata().unwrap().permissions();
        pinned
            .set_permissions(std::fs::Permissions::from_mode(0o0))
            .unwrap();
        let result = scope_metadata(&root, &parent.join("absent"), &AtomicBool::new(false));
        pinned.set_permissions(permissions).unwrap();
        if unsafe { libc::geteuid() } != 0 {
            assert!(
                result.is_err(),
                "A denied parent cannot establish child absence"
            );
        }
        assert_eq!(
            std::fs::read(parent.join("preserved")).unwrap(),
            b"private contents"
        );
    }

    #[test]
    fn scope_metadata_cancellation_and_ancestry_limits_fail_before_proving_absence() {
        let (_temp, base) = fixture();
        let root = authorize(&base, "projects").unwrap();
        let requested = base.join("absent/child");
        with_scope_metadata_observer(
            |_| panic!("A pre-cancelled lookup must not open the grant"),
            || {
                assert_eq!(
                    scope_metadata(&root, &requested, &AtomicBool::new(true)).unwrap_err(),
                    "Cancelled"
                )
            },
        );
        for phase in [
            ScopeMetadataPhase::MissingObserved,
            ScopeMetadataPhase::BeforeAbsenceValidation,
        ] {
            let cancel = std::sync::Arc::new(AtomicBool::new(false));
            let requested_cancel = cancel.clone();
            let result = with_scope_metadata_observer(
                move |observed| {
                    if observed == phase {
                        requested_cancel.store(true, Ordering::Release);
                    }
                },
                || scope_metadata(&root, &requested, &cancel),
            );
            assert_eq!(result.unwrap_err(), "Cancelled");
        }
        let mut deep = base.clone();
        for _ in 0..=MAX_DEPTH {
            deep.push("component");
        }
        with_scope_metadata_observer(
            |_| panic!("Depth must be bounded before opening the grant"),
            || {
                assert!(
                    scope_metadata(&root, &deep, &AtomicBool::new(false))
                        .unwrap_err()
                        .contains("bounded directory depth")
                )
            },
        );
    }

    #[test]
    fn missing_entry_remains_an_error_without_changing_other_metadata_diagnostics() {
        let (_temp, base) = fixture();
        let preserved = base.join("preserved.txt");
        let contents = b"Preserve this disposable file.";
        std::fs::write(&preserved, contents).unwrap();
        let directory = open_directory(&base).unwrap();
        let before = stat_child(directory.as_raw_fd(), OsStr::new("preserved.txt")).unwrap();

        let transient = base.join("transient");
        std::fs::create_dir(&transient).unwrap();
        assert!(stat_child(directory.as_raw_fd(), OsStr::new("transient")).is_ok());
        std::fs::remove_dir(&transient).unwrap();
        let missing = stat_child(directory.as_raw_fd(), OsStr::new("transient")).unwrap_err();
        assert_eq!(
            missing,
            "A file or folder moved or disappeared. Scan again to refresh this location."
        );
        assert!(!missing.contains("os error"));

        let file = std::fs::File::open(&preserved).unwrap();
        let not_directory = stat_child(file.as_raw_fd(), OsStr::new("child")).unwrap_err();
        assert_eq!(
            not_directory,
            format!(
                "Cannot inspect a directory entry: {}",
                std::io::Error::from_raw_os_error(libc::ENOTDIR)
            )
        );
        assert_eq!(
            stat_child(directory.as_raw_fd(), OsStr::new("preserved.txt")).unwrap(),
            before
        );
        assert_eq!(std::fs::read(&preserved).unwrap(), contents);
    }

    #[test]
    fn absolute_directory_open_preserves_identity_and_close_on_exec() {
        let (_temp, base) = fixture();
        let path = base.join("directory");
        std::fs::create_dir(&path).unwrap();
        let expected = metadata(&path).unwrap();
        for search_only in [false, true] {
            let fd = open_directory_with_access(&path, search_only, None).unwrap();
            assert_eq!(stat_fd(fd.as_raw_fd()).unwrap(), expected);
            let descriptor_flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFD) };
            assert!(descriptor_flags >= 0);
            assert_ne!(descriptor_flags & libc::FD_CLOEXEC, 0);
            #[cfg(target_os = "macos")]
            {
                let access = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
                assert!(access >= 0);
                assert_eq!(access & libc::O_EXEC != 0, search_only);
            }
        }
    }

    #[test]
    fn absolute_directory_open_rejects_ancestor_leaf_and_trailing_slash_links() {
        let (_temp, base) = fixture();
        let actual = base.join("actual");
        std::fs::create_dir_all(actual.join("child")).unwrap();
        let alias = base.join("alias");
        symlink(&actual, &alias).unwrap();
        let leaf = actual.join("leaf");
        symlink("child", &leaf).unwrap();
        let mut trailing = leaf.as_os_str().to_owned();
        trailing.push("/");
        let mut dotted = alias.as_os_str().to_owned();
        dotted.push("/.");
        for path in [
            alias.join("child"),
            alias,
            leaf,
            PathBuf::from(trailing),
            PathBuf::from(dotted),
        ] {
            for search_only in [false, true] {
                assert!(
                    open_directory_with_access(&path, search_only, None).is_err(),
                    "Accepted a symbolic link in {path:?}"
                );
            }
        }
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn absolute_directory_search_open_preserves_search_only_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let (_temp, base) = fixture();
        let directory = base.join("search-only");
        std::fs::create_dir(&directory).unwrap();
        let manifest = directory.join("manifest");
        std::fs::write(&manifest, b"readable evidence").unwrap();
        let permissions = std::fs::metadata(&directory).unwrap().permissions();
        let pinned = File::open(&directory).unwrap();
        pinned
            .set_permissions(std::fs::Permissions::from_mode(0o100))
            .unwrap();
        let search = open_directory_with_access(&directory, true, None);
        let listing = open_directory_with_access(&directory, false, None);
        let evidence = read_regular(&manifest, &AtomicBool::new(false));
        // Restore before assertions so a failed check cannot strand the fixture.
        pinned.set_permissions(permissions).unwrap();
        assert!(search.is_ok());
        if unsafe { libc::geteuid() } != 0 {
            assert!(listing.is_err());
        }
        assert_eq!(evidence.unwrap().bytes, b"readable evidence");
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn absolute_directory_open_handles_long_paths_without_following_links() {
        struct CreatedDirectories(Vec<(OwnedFd, CString)>);
        impl Drop for CreatedDirectories {
            fn drop(&mut self) {
                for (parent, name) in self.0.iter().rev() {
                    unsafe {
                        libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR);
                    }
                }
            }
        }

        let (_temp, base) = fixture();
        let mut created = CreatedDirectories(Vec::new());
        let mut parent = open_directory(&base).unwrap();
        let mut path = base.clone();
        while path.as_os_str().as_bytes().len() <= libc::PATH_MAX as usize + 256 {
            let name = format!("directory-{}-{}", created.0.len(), "x".repeat(180));
            let name_c = c_name(OsStr::new(&name)).unwrap();
            assert_eq!(
                unsafe { libc::mkdirat(parent.as_raw_fd(), name_c.as_ptr(), 0o700) },
                0
            );
            let next = unsafe {
                libc::openat(
                    parent.as_raw_fd(),
                    name_c.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            created.0.push((parent, name_c));
            assert!(next >= 0, "Cannot open a disposable directory component");
            parent = unsafe { OwnedFd::from_raw_fd(next) };
            path.push(name);
        }
        let expected = stat_fd(parent.as_raw_fd()).unwrap();
        for search_only in [false, true] {
            let opened = open_directory_with_access(&path, search_only, None).unwrap();
            assert_eq!(stat_fd(opened.as_raw_fd()).unwrap(), expected);
        }
        let alias = base.join("alias");
        symlink(".", &alias).unwrap();
        let redirected = alias.join(path.strip_prefix(&base).unwrap());
        for search_only in [false, true] {
            assert!(open_directory_with_access(&redirected, search_only, None).is_err());
        }
    }

    #[test]
    fn absolute_directory_open_rejects_invalid_paths_and_precancellation() {
        let (_temp, base) = fixture();
        let cancelled = AtomicBool::new(true);
        for search_only in [false, true] {
            assert_eq!(
                open_directory_with_access(&base.join("missing"), search_only, Some(&cancelled))
                    .unwrap_err(),
                "Cancelled"
            );
            for path in [
                PathBuf::from("relative"),
                base.join("../outside"),
                base.join(OsStr::from_bytes(b"nul\0name")),
            ] {
                assert!(open_directory_with_access(&path, search_only, None).is_err());
            }
        }
    }

    #[test]
    fn regular_file_read_returns_bytes_with_their_validated_identity() {
        let (_temp, base) = fixture();
        let path = base.join("manifest");
        for payload in [
            Vec::new(),
            b"project evidence".to_vec(),
            vec![0x35; MAX_MANIFEST as usize],
        ] {
            std::fs::write(&path, &payload).unwrap();
            let before = identity(&path).unwrap();
            let captured = read_regular(&path, &AtomicBool::new(false)).unwrap();
            assert_eq!(captured.bytes, payload);
            assert_eq!(captured.identity, before);
            assert_eq!(captured.identity, identity(&path).unwrap());
        }
    }

    #[test]
    fn regular_file_read_rejects_links_specials_and_oversized_files_before_opening() {
        let (_temp, base) = fixture();
        let regular = base.join("regular");
        std::fs::write(&regular, b"preserve").unwrap();
        std::fs::hard_link(&regular, base.join("hardlink")).unwrap();
        symlink(&regular, base.join("symlink")).unwrap();
        std::fs::create_dir(base.join("directory")).unwrap();
        let fifo = c_name(base.join("fifo").as_os_str()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        std::fs::File::create(base.join("large"))
            .unwrap()
            .set_len(MAX_MANIFEST + 1)
            .unwrap();
        symlink(&base, base.join("parent-link")).unwrap();
        with_regular_read_observer(
            |phase| {
                assert_ne!(
                    phase,
                    RegularReadPhase::BeforeOpen,
                    "Unsafe evidence reached openat"
                )
            },
            || {
                for name in [
                    "hardlink",
                    "symlink",
                    "directory",
                    "fifo",
                    "large",
                    "parent-link/regular",
                ] {
                    assert!(
                        read_regular(&base.join(name), &AtomicBool::new(false)).is_err(),
                        "{name}"
                    );
                }
            },
        );
        assert_eq!(std::fs::read(&regular).unwrap(), b"preserve");
    }

    #[test]
    fn regular_file_read_cancels_before_probes_and_between_bounded_reads() {
        use std::sync::Arc;
        use std::sync::atomic::AtomicUsize;
        let (_temp, base) = fixture();
        let path = base.join("manifest");
        std::fs::write(&path, vec![0x35; 3 * 64 * 1024]).unwrap();
        let before = CHILD_METADATA_CALLS.with(std::cell::Cell::get);
        assert_eq!(
            read_regular(&path, &AtomicBool::new(true)).unwrap_err(),
            "Cancelled"
        );
        assert_eq!(CHILD_METADATA_CALLS.with(std::cell::Cell::get), before);
        let cancel = Arc::new(AtomicBool::new(false));
        let read_bytes = Arc::new(AtomicUsize::new(0));
        let observed_cancel = Arc::clone(&cancel);
        let observed_bytes = Arc::clone(&read_bytes);
        let result = with_regular_read_observer(
            move |phase| {
                if let RegularReadPhase::AfterChunk(bytes) = phase {
                    observed_bytes.store(bytes, Ordering::Relaxed);
                    observed_cancel.store(true, Ordering::Relaxed);
                }
            },
            || read_regular(&path, &cancel),
        );
        assert_eq!(result.unwrap_err(), "Cancelled");
        assert!((1..=64 * 1024).contains(&read_bytes.load(Ordering::Relaxed)));
    }

    #[test]
    fn regular_file_read_rejects_a_leaf_swapped_before_open() {
        let (_temp, base) = fixture();
        let path = base.join("manifest");
        let sentinel = base.join("sentinel");
        std::fs::write(&path, b"original").unwrap();
        std::fs::write(&sentinel, b"must not be read as evidence").unwrap();
        let swapped = path.clone();
        let target = sentinel.clone();
        let result = with_regular_read_observer(
            move |phase| {
                if phase == RegularReadPhase::BeforeOpen {
                    std::fs::remove_file(&swapped).unwrap();
                    symlink(&target, &swapped).unwrap();
                }
            },
            || read_regular(&path, &AtomicBool::new(false)),
        );
        assert!(result.is_err());
        assert_eq!(
            std::fs::read(&sentinel).unwrap(),
            b"must not be read as evidence"
        );
    }

    #[test]
    fn regular_file_read_rejects_a_replacement_after_capturing_bytes() {
        let (_temp, base) = fixture();
        let path = base.join("manifest");
        std::fs::write(&path, b"original").unwrap();
        let replaced = path.clone();
        let retained = base.join("retained");
        let retained_by_observer = retained.clone();
        let result = with_regular_read_observer(
            move |phase| {
                if phase == RegularReadPhase::BeforePathValidation {
                    std::fs::rename(&replaced, &retained_by_observer).unwrap();
                    std::fs::write(&replaced, b"new file").unwrap();
                }
            },
            || read_regular(&path, &AtomicBool::new(false)),
        );
        assert!(result.unwrap_err().contains("pathname changed"));
        assert_eq!(std::fs::read(&retained).unwrap(), b"original");
    }

    #[test]
    fn regular_file_read_rejects_in_place_changes_after_capturing_bytes() {
        let (_temp, base) = fixture();
        let path = base.join("manifest");
        std::fs::write(&path, b"original").unwrap();
        std::fs::File::open(&path)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new()
                    .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1)),
            )
            .unwrap();
        let modified = path.clone();
        let result = with_regular_read_observer(
            move |phase| {
                if phase == RegularReadPhase::BeforePathValidation {
                    std::fs::write(&modified, b"new data").unwrap();
                }
            },
            || read_regular(&path, &AtomicBool::new(false)),
        );
        assert!(result.unwrap_err().contains("pathname changed"));
    }

    #[test]
    fn regular_file_read_rejects_a_renamed_or_redirected_parent() {
        let (_temp, base) = fixture();
        for redirected in [false, true] {
            let parent = base.join(if redirected { "redirected" } else { "replaced" });
            std::fs::create_dir(&parent).unwrap();
            let path = parent.join("manifest");
            std::fs::write(&path, b"original").unwrap();
            let moved = parent.with_extension("retained");
            let moved_by_observer = moved.clone();
            let result = with_regular_read_observer(
                move |phase| {
                    if phase == RegularReadPhase::BeforePathValidation {
                        std::fs::rename(&parent, &moved_by_observer).unwrap();
                        if redirected {
                            symlink(&moved_by_observer, &parent).unwrap();
                        } else {
                            std::fs::create_dir(&parent).unwrap();
                            std::fs::write(parent.join("manifest"), b"new file").unwrap();
                        }
                    }
                },
                || read_regular(&path, &AtomicBool::new(false)),
            );
            assert!(result.is_err());
            assert_eq!(std::fs::read(moved.join("manifest")).unwrap(), b"original");
        }
    }

    #[test]
    fn names_first_discovery_is_bounded_and_never_follows_links() {
        let (_temp, base) = fixture();
        for index in 0..600 {
            std::fs::write(base.join(format!("file-{index}")), b"unrelated").unwrap();
        }
        std::fs::create_dir(base.join("nested-project")).unwrap();
        symlink("nested-project", base.join("linked-project")).unwrap();
        let cancel = AtomicBool::new(false);
        let mut directory = Directory::open(&base).unwrap();
        let (mut files, mut skipped, mut directories, mut steps) = (0, 0, 0, 0);
        loop {
            let step = directory.next_discovery(&cancel, false).unwrap();
            steps += 1;
            assert!(step.files + step.skipped + u64::from(step.entry.is_some()) <= 256);
            assert!(step.metadata_skipped <= step.files + step.skipped);
            files += step.files;
            skipped += step.skipped;
            if let Some(entry) = step.entry {
                let path = match entry {
                    DiscoveryEntry::Directory(path) => path,
                    DiscoveryEntry::Metadata(entry) => {
                        assert!(entry.meta.is_dir());
                        entry.path
                    }
                };
                assert_eq!(path, base.join("nested-project"));
                let (entry, _) = directory
                    .open_discovered(path, directory.initial.device, &cancel)
                    .unwrap();
                assert!(entry.meta.is_dir());
                directories += 1;
            }
            if step.finished {
                break;
            }
        }
        assert_eq!((files, skipped, directories), (600, 1, 1));
        assert!(steps >= 3);
        directory.unchanged().unwrap();
        assert!(
            directory
                .next_discovery(&AtomicBool::new(true), false)
                .is_err()
        );
    }

    #[test]
    fn personal_names_only_request_metadata_for_reviewable_formats() {
        let (_temp, base) = fixture();
        for index in 0..600 {
            std::fs::write(base.join(format!("source-{index}.rs")), b"source").unwrap();
        }
        for name in ["report.pdf", "recording.MOV", ".hidden.zip"] {
            std::fs::write(base.join(name), b"fixture").unwrap();
        }
        std::fs::create_dir(base.join("Library")).unwrap();
        symlink("report.pdf", base.join("linked.pdf")).unwrap();
        let cancel = AtomicBool::new(false);
        let mut directory = Directory::open(&base).unwrap();
        let before = CHILD_METADATA_CALLS.with(std::cell::Cell::get);
        let (mut files, mut skipped) = (0, 0);
        let mut selected = Vec::new();
        loop {
            let step = directory.next_personal_discovery(&cancel, false).unwrap();
            assert!(step.files + step.skipped + u64::from(step.entry.is_some()) <= 256);
            files += step.files;
            skipped += step.skipped;
            if let Some(entry) = step.entry {
                let DiscoveryEntry::Metadata(entry) = entry else {
                    panic!("Only the two ordinary personal files should be selected");
                };
                assert!(entry.meta.is_file() && !entry.meta.is_symlink());
                selected.push(entry.path.file_name().unwrap().to_owned());
            }
            if step.finished {
                break;
            }
        }
        selected.sort();
        assert_eq!(
            selected,
            [OsStr::new("recording.MOV"), OsStr::new("report.pdf")]
        );
        assert_eq!((files, skipped), (601, 2));
        assert_eq!(CHILD_METADATA_CALLS.with(std::cell::Cell::get) - before, 2);
        assert!(
            directory
                .next_personal_discovery(&AtomicBool::new(true), false)
                .is_err()
        );
    }

    #[test]
    fn library_names_exclude_protected_and_managed_entries_before_metadata() {
        let (_temp, base) = fixture();
        let excluded = [".git", "Library", "Dropbox", "Homebrew", "uv", "pip"];
        for name in excluded {
            std::fs::create_dir(base.join(name)).unwrap();
        }
        std::fs::create_dir(base.join("com.example.browser")).unwrap();
        std::fs::write(base.join("ordinary.log"), b"fixture").unwrap();
        symlink("ordinary.log", base.join("linked.log")).unwrap();
        for unknown_types in [false, true] {
            let mut directory = Directory::open(&base).unwrap();
            directory.force_unknown_types = unknown_types;
            let before = CHILD_METADATA_CALLS.with(std::cell::Cell::get);
            let mut selected = Vec::new();
            let mut skipped = 0;
            loop {
                let step = directory
                    .next_library_discovery(&AtomicBool::new(false), true)
                    .unwrap();
                skipped += step.skipped;
                if let Some(entry) = step.entry {
                    selected.push(match entry {
                        DiscoveryEntry::Directory(path) => path,
                        DiscoveryEntry::Metadata(entry) => entry.path,
                    });
                }
                if step.finished {
                    break;
                }
            }
            selected.sort();
            assert_eq!(
                selected,
                [base.join("com.example.browser"), base.join("ordinary.log")]
            );
            assert_eq!(skipped, excluded.len() as u64 + 1);
            // Unknown types need a no-follow stat for the ordinary directory
            // and link too; protected names never need one in either mode.
            assert_eq!(
                CHILD_METADATA_CALLS.with(std::cell::Cell::get) - before,
                if unknown_types { 3 } else { 1 }
            );
        }
    }

    #[test]
    fn names_first_discovery_excludes_media_before_metadata() {
        let (_temp, base) = fixture();
        let names = [
            "Music",
            "Pictures",
            "Movies",
            "Library",
            "Collection.PHOTOSLIBRARY",
            "Collection.musiclibrary",
        ];
        for name in names {
            std::fs::create_dir(base.join(name)).unwrap();
            std::fs::write(base.join(name).join("preserve"), b"untouched").unwrap();
        }
        std::fs::create_dir(base.join("Projects")).unwrap();
        let root = authorize(&base, "home").unwrap();
        let mut directory = Directory::open(&base).unwrap();
        let before = CHILD_METADATA_CALLS.with(std::cell::Cell::get);
        let mut returned = Vec::new();
        let mut skipped = 0;
        loop {
            let step = directory
                .next_discovery(&AtomicBool::new(false), true)
                .unwrap();
            skipped += step.skipped;
            assert_eq!(step.metadata_skipped, step.skipped);
            if let Some(entry) = step.entry {
                returned.push(match entry {
                    DiscoveryEntry::Directory(path) => path,
                    DiscoveryEntry::Metadata(entry) => entry.path,
                });
            }
            if step.finished {
                break;
            }
        }
        assert_eq!(returned, vec![base.join("Projects")]);
        assert_eq!(skipped, names.len() as u64);
        assert_eq!(CHILD_METADATA_CALLS.with(std::cell::Cell::get), before);
        let before = CHILD_METADATA_CALLS.with(std::cell::Cell::get);
        for name in names {
            let path = base.join(name).join("preserve");
            assert!(check_scope_policy(&root, &path).is_err());
            assert!(scope_metadata(&root, &path, &AtomicBool::new(false)).is_err());
        }
        assert_eq!(CHILD_METADATA_CALLS.with(std::cell::Cell::get), before);
        for name in names {
            assert_eq!(
                std::fs::read(base.join(name).join("preserve")).unwrap(),
                b"untouched"
            );
        }
    }

    #[test]
    fn directory_construction_and_fused_open_do_not_enumerate_contents() {
        let (_temp, base) = fixture();
        let path = base.join("project");
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("payload"), b"preserve").unwrap();
        let expected = metadata(&path).unwrap();
        let before = CHILD_METADATA_CALLS.with(std::cell::Cell::get);
        with_directory_read_observer(
            |_| panic!("Opening a descriptor must not enumerate its contents"),
            || {
                let parent = Directory::open(&base).unwrap();
                let (entry, directory) = parent
                    .open_discovered(path, expected.identity.device, &AtomicBool::new(false))
                    .unwrap();
                assert_eq!(entry.meta, expected);
                assert!(directory.is_some());
                directory.unwrap().unchanged().unwrap();
            },
        );
        assert_eq!(CHILD_METADATA_CALLS.with(std::cell::Cell::get), before);
    }

    #[test]
    fn unknown_discovery_types_retain_nofollow_metadata_and_identity_checks() {
        let (_temp, base) = fixture();
        let child = base.join("project");
        std::fs::create_dir(&child).unwrap();
        std::fs::create_dir(base.join("Dropbox")).unwrap();
        std::fs::write(base.join("file"), b"preserve").unwrap();
        symlink(&child, base.join("alias")).unwrap();
        let mut directory = Directory::open(&base).unwrap();
        directory.force_unknown_types = true;
        let before = CHILD_METADATA_CALLS.with(std::cell::Cell::get);
        let (mut files, mut skipped) = (0, 0);
        let mut discovered = None;
        loop {
            let step = directory
                .next_discovery(&AtomicBool::new(false), false)
                .unwrap();
            files += step.files;
            skipped += step.skipped;
            if let Some(entry) = step.entry {
                let DiscoveryEntry::Metadata(entry) = entry else {
                    panic!("An unknown type must have authoritative no-follow metadata")
                };
                assert_eq!(entry.path, child);
                discovered = Some(entry);
            }
            if step.finished {
                break;
            }
        }
        assert_eq!((files, skipped), (1, 2));
        assert_eq!(CHILD_METADATA_CALLS.with(std::cell::Cell::get) - before, 3);
        let entry = discovered.unwrap();
        directory.open_child(&entry).unwrap();
        std::fs::rename(&child, base.join("retained")).unwrap();
        std::fs::create_dir(&child).unwrap();
        assert!(directory.open_child(&entry).is_err());
    }

    #[test]
    fn fused_discovery_rejects_symlink_and_regular_file_replacements() {
        for link in [false, true] {
            let (_temp, base) = fixture();
            let path = base.join("project");
            std::fs::create_dir(&path).unwrap();
            std::fs::write(path.join("payload"), b"preserve").unwrap();
            let mut parent = Directory::open(&base).unwrap();
            let Some(DiscoveryEntry::Directory(discovered)) = parent
                .next_discovery(&AtomicBool::new(false), false)
                .unwrap()
                .entry
            else {
                panic!("The disposable local directory must supply a directory hint")
            };
            assert_eq!(discovered, path);
            let retained = base.join("retained");
            std::fs::rename(&path, &retained).unwrap();
            if link {
                symlink(&retained, &path).unwrap();
            } else {
                std::fs::write(&path, b"replacement").unwrap();
            }
            with_directory_read_observer(
                |_| panic!("A substituted entry must never be enumerated"),
                || {
                    assert!(
                        parent
                            .open_discovered(
                                discovered,
                                parent.initial.device,
                                &AtomicBool::new(false)
                            )
                            .is_err()
                    );
                },
            );
            assert!(parent.unchanged().is_err());
            assert_eq!(
                std::fs::read(retained.join("payload")).unwrap(),
                b"preserve"
            );
        }
    }

    #[test]
    fn fused_discovery_uses_opened_identity_and_excludes_other_devices() {
        let (_temp, base) = fixture();
        let path = base.join("project");
        std::fs::create_dir(&path).unwrap();
        let mut parent = Directory::open(&base).unwrap();
        let Some(DiscoveryEntry::Directory(discovered)) = parent
            .next_discovery(&AtomicBool::new(false), false)
            .unwrap()
            .entry
        else {
            panic!("Expected a directory hint")
        };
        std::fs::rename(&path, base.join("retained")).unwrap();
        std::fs::create_dir(&path).unwrap();
        let replacement = metadata(&path).unwrap();
        with_directory_read_observer(
            |_| panic!("Inspecting a replacement or volume boundary must not enumerate"),
            || {
                let (entry, opened) = parent
                    .open_discovered(
                        discovered.clone(),
                        replacement.identity.device,
                        &AtomicBool::new(false),
                    )
                    .unwrap();
                assert_eq!(entry.meta, replacement);
                assert_eq!(opened.unwrap().initial, replacement.identity);
                let (entry, opened) = parent
                    .open_discovered(
                        discovered,
                        replacement.identity.device.wrapping_add(1),
                        &AtomicBool::new(false),
                    )
                    .unwrap();
                assert_eq!(entry.meta, replacement);
                assert!(opened.is_none());
            },
        );
        assert!(parent.unchanged().is_err());
    }

    #[test]
    fn lazy_readers_reject_changes_before_first_enumeration() {
        for names in [false, true] {
            let (_temp, base) = fixture();
            std::fs::File::open(&base)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH))
                .unwrap();
            let mut directory = Directory::open(&base).unwrap();
            std::fs::write(base.join("new-entry"), b"preserve").unwrap();
            with_directory_read_observer(
                |_| panic!("The changed directory must be rejected before its first read"),
                || {
                    let error = if names {
                        directory
                            .next_discovery(&AtomicBool::new(false), false)
                            .err()
                            .unwrap()
                    } else {
                        directory.next(&AtomicBool::new(false)).unwrap_err()
                    };
                    assert!(error.contains("changed"));
                },
            );
        }
    }

    #[test]
    fn cancellation_prevents_fused_open_and_lazy_reader_initialization() {
        let (_temp, base) = fixture();
        let path = base.join("project");
        std::fs::create_dir(&path).unwrap();
        with_directory_read_observer(
            |_| panic!("A cancelled traversal must not initialize an enumerator"),
            || {
                let mut directory = Directory::open(&base).unwrap();
                let cancel = AtomicBool::new(true);
                let error = directory
                    .open_discovered(path, directory.initial.device, &cancel)
                    .err()
                    .unwrap();
                assert_eq!(error, "Cancelled");
                assert!(directory.next_discovery(&cancel, false).is_err());
                assert!(directory.next(&cancel).is_err());
                directory.unchanged().unwrap();
            },
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn local_only_io_restores_the_calling_threads_policy() {
        let original = unsafe { getiopolicy_np(3, 1) };
        {
            let _guard = LocalOnlyIo::new().unwrap();
            assert_eq!(unsafe { getiopolicy_np(3, 1) }, 1);
            {
                let _nested = LocalOnlyIo::new().unwrap();
            }
            assert_eq!(unsafe { getiopolicy_np(3, 1) }, 1);
        }
        assert_eq!(unsafe { getiopolicy_np(3, 1) }, original);
    }

    #[test]
    fn authorization_rejects_links_protected_and_cloud_names() {
        let (_temp, base) = fixture();
        let actual = base.join("actual");
        std::fs::create_dir(&actual).unwrap();
        symlink(&actual, base.join("alias")).unwrap();
        assert!(authorize(&base.join("alias"), "projects").is_err());
        assert!(authorize(Path::new("/System"), "folder").is_err());
        let cloud = base.join("Dropbox");
        std::fs::create_dir(&cloud).unwrap();
        assert!(authorize(&cloud, "folder").is_err());
    }

    #[test]
    fn root_identity_change_revokes_authorization() {
        let (_temp, base) = fixture();
        let chosen = base.join("chosen");
        std::fs::create_dir(&chosen).unwrap();
        let root = authorize(&chosen, "projects").unwrap();
        std::fs::rename(&chosen, base.join("old")).unwrap();
        std::fs::create_dir(&chosen).unwrap();
        assert!(validate_root(&root).is_err());
    }

    #[test]
    fn links_are_not_followed_and_hard_links_are_counted_once() {
        let (_temp, base) = fixture();
        let tree = base.join("tree");
        std::fs::create_dir(&tree).unwrap();
        std::fs::write(tree.join("first"), [7u8; 8192]).unwrap();
        std::fs::hard_link(tree.join("first"), tree.join("second")).unwrap();
        std::fs::write(base.join("outside"), [5u8; 4096]).unwrap();
        symlink(base.join("outside"), tree.join("link")).unwrap();
        let measurement = measure(
            &tree,
            identity(&tree).unwrap().device,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(measurement.logical_bytes, 8192);
        assert_eq!(measurement.files, 2);
        assert_eq!(measurement.skipped, 1);
        assert!(measurement.unsafe_reason.is_some());
    }

    fn assert_same_measurement(actual: &Measurement, expected: &Measurement) {
        assert_eq!(actual.pruned, expected.pruned);
        assert_eq!(actual.logical_bytes, expected.logical_bytes);
        assert_eq!(actual.allocated_bytes, expected.allocated_bytes);
        assert_eq!(actual.files, expected.files);
        assert_eq!(actual.latest_modified_ns, expected.latest_modified_ns);
        assert_eq!(actual.fingerprint, expected.fingerprint);
        assert_eq!(actual.unsafe_reason, expected.unsafe_reason);
        assert_eq!(actual.entries, expected.entries);
        assert_eq!(actual.directories, expected.directories);
        assert_eq!(actual.skipped, expected.skipped);
        assert_eq!(actual.errors, expected.errors);
    }

    #[test]
    fn bounded_measurement_cursor_matches_the_synchronous_wrapper() {
        let (_temp, base) = fixture();
        let (tree, first, _) = internal_regular_links(&base);
        let cancel = AtomicBool::new(false);
        let device = identity(&tree).unwrap().device;
        let expected =
            measure_with_policy(&tree, device, &cancel, MeasurementPolicy::Developer).unwrap();
        let mut cursor =
            MeasurementCursor::full(&tree, device, MeasurementPolicy::Developer, &cancel).unwrap();
        let mut observed = 0;
        let mut yields = 0;
        let actual = loop {
            let progress = cursor
                .advance(
                    &cancel,
                    1,
                    Instant::now() + std::time::Duration::from_secs(1),
                    |_, partial| {
                        observed += 1;
                        assert!(
                            partial.fingerprint.is_empty(),
                            "a resumable partial must never expose mutation evidence"
                        );
                        Ok(())
                    },
                )
                .unwrap();
            match progress {
                MeasurementProgress::Pending => yields += 1,
                MeasurementProgress::Complete(measurement) => break measurement,
            }
        };
        assert!(yields > 1);
        assert_eq!(observed, actual.entries);
        assert_same_measurement(&actual, &expected);
        assert_eq!(std::fs::read(first).unwrap(), [7u8; 8192]);
        assert!(
            cursor
                .advance(
                    &cancel,
                    1,
                    Instant::now() + std::time::Duration::from_secs(1),
                    |_, _| Ok(()),
                )
                .unwrap_err()
                .contains("terminal")
        );
    }

    #[test]
    fn measurement_cursor_deadline_and_cancellation_are_sticky() {
        let (_temp, base) = fixture();
        std::fs::write(base.join("payload"), b"preserve").unwrap();
        let cancel = AtomicBool::new(false);
        let device = identity(&base).unwrap().device;
        let mut cursor =
            MeasurementCursor::metadata(&base, device, MeasurementPolicy::Strict, &cancel).unwrap();
        let mut observed = 0;
        assert!(matches!(
            cursor
                .advance(&cancel, 1, Instant::now(), |_, _| {
                    observed += 1;
                    Ok(())
                })
                .unwrap(),
            MeasurementProgress::Pending
        ));
        assert_eq!(observed, 0);
        assert!(matches!(
            cursor
                .advance(
                    &cancel,
                    1,
                    Instant::now() + std::time::Duration::from_secs(1),
                    |_, partial| {
                        observed += 1;
                        assert!(partial.fingerprint.is_empty());
                        Ok(())
                    },
                )
                .unwrap(),
            MeasurementProgress::Pending
        ));
        assert_eq!(observed, 1);
        cancel.store(true, Ordering::Release);
        assert_eq!(
            cursor
                .advance(
                    &cancel,
                    1,
                    Instant::now() + std::time::Duration::from_secs(1),
                    |_, _| Ok(()),
                )
                .unwrap_err(),
            "Cancelled"
        );
        cancel.store(false, Ordering::Release);
        assert!(
            cursor
                .advance(
                    &cancel,
                    1,
                    Instant::now() + std::time::Duration::from_secs(1),
                    |_, _| Ok(()),
                )
                .unwrap_err()
                .contains("terminal")
        );
    }

    #[test]
    fn measurement_cursor_revalidates_a_root_replaced_while_yielded() {
        let (_temp, base) = fixture();
        let tree = base.join("tree");
        std::fs::create_dir(&tree).unwrap();
        std::fs::write(tree.join("payload"), b"old output").unwrap();
        let cancel = AtomicBool::new(false);
        let device = identity(&tree).unwrap().device;
        let mut cursor =
            MeasurementCursor::full(&tree, device, MeasurementPolicy::Strict, &cancel).unwrap();
        assert!(matches!(
            cursor
                .advance(
                    &cancel,
                    1,
                    Instant::now() + std::time::Duration::from_secs(1),
                    |_, partial| {
                        assert!(partial.fingerprint.is_empty());
                        Ok(())
                    },
                )
                .unwrap(),
            MeasurementProgress::Pending
        ));
        let old = base.join("old-tree");
        std::fs::rename(&tree, &old).unwrap();
        std::fs::create_dir(&tree).unwrap();
        std::fs::write(tree.join("replacement"), b"must not be measured").unwrap();
        let measured = loop {
            match cursor
                .advance(
                    &cancel,
                    1,
                    Instant::now() + std::time::Duration::from_secs(1),
                    |_, partial| {
                        assert!(partial.fingerprint.is_empty());
                        Ok(())
                    },
                )
                .unwrap()
            {
                MeasurementProgress::Pending => {}
                MeasurementProgress::Complete(measurement) => break measurement,
            }
        };
        assert!(measured.unsafe_reason.is_some());
        assert_eq!(std::fs::read(old.join("payload")).unwrap(), b"old output");
        assert_eq!(
            std::fs::read(tree.join("replacement")).unwrap(),
            b"must not be measured"
        );
    }

    fn internal_regular_links(base: &Path) -> (PathBuf, PathBuf, PathBuf) {
        let tree = base.join("target");
        let first = tree.join("left/first");
        let second = tree.join("right/second");
        std::fs::create_dir_all(first.parent().unwrap()).unwrap();
        std::fs::create_dir_all(second.parent().unwrap()).unwrap();
        std::fs::write(&first, [7u8; 8192]).unwrap();
        std::fs::hard_link(&first, &second).unwrap();
        (tree, first, second)
    }

    #[test]
    fn developer_internal_regular_links_close_without_changing_counts_or_fingerprint() {
        let (_temp, base) = fixture();
        let (tree, first, second) = internal_regular_links(&base);
        let original = metadata(&first).unwrap();
        assert_eq!(metadata(&second).unwrap(), original);
        assert_eq!(original.links, 2);
        let cancel = AtomicBool::new(false);
        let mut closure = RegularLinkClosure::default();
        let measured = measure_try_observing_with_policy(
            &tree,
            original.identity.device,
            &cancel,
            MeasurementPolicy::Developer,
            |entry, _| closure.observe(&entry.meta),
        )
        .unwrap();
        assert!(
            measured.unsafe_reason.is_none(),
            "{:?}",
            measured.unsafe_reason
        );
        assert!(!measured.pruned);
        assert_eq!(
            (measured.entries, measured.directories, measured.files),
            (5, 3, 2)
        );
        assert_eq!(measured.logical_bytes, original.identity.size);
        assert_eq!(measured.allocated_bytes, original.allocated);
        assert_eq!(
            closure.into_closed().unwrap().collect::<Vec<_>>(),
            vec![original.clone()]
        );

        let strict = measure(&tree, original.identity.device, &cancel).unwrap();
        assert!(strict.unsafe_reason.unwrap().contains("hard-linked"));
        assert_eq!(strict.fingerprint, measured.fingerprint);
        assert_eq!(strict.logical_bytes, measured.logical_bytes);
        assert_eq!(strict.allocated_bytes, measured.allocated_bytes);
        let cheap = measure_metadata_observing_with_policy(
            &tree,
            original.identity.device,
            &cancel,
            MeasurementPolicy::Developer,
            |_, _| {},
        )
        .unwrap();
        assert!(cheap.fingerprint.is_empty());
        assert!(cheap.unsafe_reason.is_none());
        assert_eq!(cheap.entries, measured.entries);
        assert_eq!(cheap.logical_bytes, measured.logical_bytes);
        assert_eq!(cheap.allocated_bytes, measured.allocated_bytes);

        let stage = base.join(".chippytea-stage");
        std::fs::rename(&tree, &stage).unwrap();
        let staged = measure_with_policy(
            &stage,
            original.identity.device,
            &cancel,
            MeasurementPolicy::Developer,
        )
        .unwrap();
        assert!(staged.unsafe_reason.is_none());
        assert_eq!(staged.fingerprint, measured.fingerprint);
        assert_eq!(
            std::fs::read(stage.join("left/first")).unwrap(),
            [7u8; 8192]
        );
        assert_eq!(
            std::fs::read(stage.join("right/second")).unwrap(),
            [7u8; 8192]
        );
    }

    #[test]
    fn developer_link_closure_never_combines_separate_artifacts() {
        let (_temp, base) = fixture();
        let (tree, first, second) = internal_regular_links(&base);
        let sibling = base.join("node_modules");
        std::fs::create_dir(&sibling).unwrap();
        let external = sibling.join("shared");
        std::fs::hard_link(&first, &external).unwrap();
        let original = metadata(&first).unwrap();
        assert_eq!(original.links, 3);
        let cancel = AtomicBool::new(false);
        for (artifact, paths) in [(&tree, 2), (&sibling, 1)] {
            let measured = measure_with_policy(
                artifact,
                original.identity.device,
                &cancel,
                MeasurementPolicy::Developer,
            )
            .unwrap();
            assert!(!measured.pruned);
            assert_eq!(measured.files, paths);
            assert_eq!(measured.logical_bytes, original.identity.size);
            assert!(measured.unsafe_reason.unwrap().contains("hard-linked"));
        }
        assert_eq!(metadata(&first).unwrap(), original);
        assert_eq!(metadata(&second).unwrap(), original);
        assert_eq!(metadata(&external).unwrap(), original);
        assert_eq!(std::fs::read(&external).unwrap(), [7u8; 8192]);
    }

    #[test]
    fn regular_link_closure_rejects_metadata_and_count_changes_permanently() {
        let (_temp, base) = fixture();
        let (_, first, _) = internal_regular_links(&base);
        let original = metadata(&first).unwrap();
        let changes: [fn(&mut EntryMeta); 9] = [
            |meta| meta.identity.mode ^= 0o100,
            |meta| meta.identity.mode = libc::S_IFLNK as u32 | 0o777,
            |meta| meta.identity.size += 1,
            |meta| meta.identity.modified_ns += 1,
            |meta| meta.identity.changed_ns += 1,
            |meta| meta.allocated += 512,
            |meta| meta.links = 1,
            |meta| meta.uid = meta.uid.wrapping_add(1),
            |meta| meta.flags ^= 1,
        ];
        for change in changes {
            let mut closure = RegularLinkClosure::default();
            closure.observe(&original).unwrap();
            assert!(
                closure.verify().is_err(),
                "One of two aliases is incomplete evidence"
            );
            let mut changed = original.clone();
            change(&mut changed);
            assert!(closure.observe(&changed).is_err());
            assert!(
                closure.observe(&original).is_err(),
                "A later matching alias cannot erase failure"
            );
            assert!(closure.verify().is_err());
            assert!(closure.into_closed().is_err());
        }
        let mut duplicate = RegularLinkClosure::default();
        duplicate.observe(&original).unwrap();
        duplicate.observe(&original).unwrap();
        duplicate.verify().unwrap();
        assert!(duplicate.observe(&original).is_err());
        assert!(duplicate.into_closed().is_err());

        let mut zero = original;
        zero.links = 0;
        let mut closure = RegularLinkClosure::default();
        assert!(closure.observe(&zero).is_err());
        assert!(closure.into_closed().is_err());
    }

    #[test]
    fn developer_link_closure_detects_an_alias_added_between_nested_observations() {
        let (_temp, base) = fixture();
        let (tree, first, second) = internal_regular_links(&base);
        let outside = base.join("outside-alias");
        let device = identity(&tree).unwrap().device;
        let mut changed = false;
        let measured = measure_observing_with_policy(
            &tree,
            device,
            &AtomicBool::new(false),
            MeasurementPolicy::Developer,
            |entry, _| {
                if entry.meta.is_file() && !changed {
                    std::fs::hard_link(&first, &outside).unwrap();
                    changed = true;
                }
            },
        )
        .unwrap();
        assert!(changed);
        assert_eq!(measured.files, 2);
        assert!(
            measured
                .unsafe_reason
                .unwrap()
                .contains("changed during measurement")
        );
        for path in [&first, &second, &outside] {
            assert_eq!(metadata(path).unwrap().links, 3);
            assert_eq!(std::fs::read(path).unwrap(), [7u8; 8192]);
        }
    }

    #[test]
    fn regular_link_closure_bound_cannot_turn_overflow_into_closed_evidence() {
        let (_temp, base) = fixture();
        let (_, first, _) = internal_regular_links(&base);
        let mut meta = metadata(&first).unwrap();
        let mut closure = RegularLinkClosure::default();
        // Exercise the actual production cap without creating 262,144 files.
        for inode in 0..MAX_LINK_IDENTITIES as u64 {
            meta.identity.inode = inode;
            closure.observe(&meta).unwrap();
            closure.observe(&meta).unwrap();
        }
        assert_eq!(closure.groups.len(), MAX_LINK_IDENTITIES);
        closure.verify().unwrap();
        let capacity = closure.groups.capacity();
        meta.identity.inode = MAX_LINK_IDENTITIES as u64;
        assert!(closure.observe(&meta).unwrap_err().contains("limit"));
        assert_eq!(closure.groups.len(), MAX_LINK_IDENTITIES);
        assert_eq!(closure.groups.capacity(), capacity);
        assert!(closure.verify().is_err());
        assert!(closure.into_closed().is_err());
    }

    #[test]
    fn developer_link_closure_stays_provisional_after_recent_pruning_or_cancellation() {
        let (_temp, base) = fixture();
        let (tree, first, second) = internal_regular_links(&base);
        for path in [
            tree.as_path(),
            first.parent().unwrap(),
            second.parent().unwrap(),
        ] {
            File::open(path)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH))
                .unwrap();
        }
        let device = identity(&tree).unwrap().device;
        let cancel = AtomicBool::new(false);
        let mut closure = RegularLinkClosure::default();
        let pruned = measure_suggestion_observing_with_policy(
            &tree,
            device,
            &cancel,
            MeasurementPolicy::Developer,
            1_000_000_000,
            |entry, _| closure.observe(&entry.meta).unwrap(),
        )
        .unwrap();
        assert!(pruned.pruned);
        assert!(pruned.fingerprint.is_empty());
        assert!(pruned.unsafe_reason.is_some());
        assert_eq!(pruned.files, 1);
        assert!(closure.into_closed().is_err());

        let mut closure = RegularLinkClosure::default();
        let cancelled = measure_try_observing_with_policy(
            &tree,
            device,
            &cancel,
            MeasurementPolicy::Developer,
            |entry, _| {
                closure.observe(&entry.meta)?;
                if entry.meta.is_file() {
                    cancel.store(true, Ordering::Relaxed);
                }
                Ok(())
            },
        );
        assert_eq!(cancelled.unwrap_err(), "Cancelled");
        assert!(closure.into_closed().is_err());
        assert_eq!(std::fs::read(first).unwrap(), [7u8; 8192]);
        assert_eq!(std::fs::read(second).unwrap(), [7u8; 8192]);
    }

    #[test]
    fn developer_regular_link_closure_preserves_protected_and_symbolic_link_exclusions() {
        let (_temp, base) = fixture();
        let (tree, first, _) = internal_regular_links(&base);
        let protected = tree.join(".pnpm");
        std::fs::create_dir(&protected).unwrap();
        std::fs::hard_link(&first, protected.join("shared")).unwrap();
        let device = identity(&tree).unwrap().device;
        let cancel = AtomicBool::new(false);
        let measured =
            measure_with_policy(&tree, device, &cancel, MeasurementPolicy::Developer).unwrap();
        assert_eq!(measured.files, 2);
        assert_eq!(measured.skipped, 1);
        assert!(measured.unsafe_reason.unwrap().contains("protected"));
        assert_eq!(
            std::fs::read(protected.join("shared")).unwrap(),
            [7u8; 8192]
        );

        let linked = base.join("linked-symbols");
        std::fs::create_dir(&linked).unwrap();
        symlink("../target/left/first", linked.join("first")).unwrap();
        std::fs::hard_link(linked.join("first"), linked.join("second")).unwrap();
        let link = metadata(&linked.join("first")).unwrap();
        assert!(link.is_symlink());
        assert_eq!(link.links, 2);
        let measured =
            measure_with_policy(&linked, device, &cancel, MeasurementPolicy::Developer).unwrap();
        assert_eq!(measured.files, 0);
        assert_eq!(measured.logical_bytes, 0);
        assert!(
            measured
                .unsafe_reason
                .unwrap()
                .contains("hard-linked symbolic links")
        );
        assert_eq!(std::fs::read(first).unwrap(), [7u8; 8192]);
    }

    #[test]
    fn metadata_fingerprint_changes_for_replacement_or_nested_edit() {
        let (_temp, base) = fixture();
        std::fs::create_dir(base.join("nested")).unwrap();
        std::fs::write(base.join("nested/file"), b"old").unwrap();
        let cancel = AtomicBool::new(false);
        let device = identity(&base).unwrap().device;
        let before = measure(&base, device, &cancel).unwrap();
        let repeat = measure(&base, device, &cancel).unwrap();
        assert_eq!(before.fingerprint, repeat.fingerprint);
        std::fs::write(base.join("nested/file"), b"new content").unwrap();
        assert_ne!(
            before.fingerprint,
            measure(&base, device, &cancel).unwrap().fingerprint
        );
    }

    #[test]
    fn cancellation_stops_before_a_filesystem_operation() {
        let (_temp, base) = fixture();
        assert_eq!(
            measure(&base, 0, &AtomicBool::new(true)).unwrap_err(),
            "Cancelled"
        );
    }

    #[test]
    fn contained_bin_links_are_leaf_entries_and_external_links_block() {
        let (_temp, base) = fixture();
        let tree = base.join("node_modules");
        std::fs::create_dir_all(tree.join(".bin")).unwrap();
        std::fs::create_dir(tree.join("package")).unwrap();
        std::fs::write(tree.join("package/cli"), b"command").unwrap();
        symlink("../package/cli", tree.join(".bin/command")).unwrap();
        let device = identity(&tree).unwrap().device;
        let measurement = measure(&tree, device, &AtomicBool::new(false)).unwrap();
        assert!(
            measurement.unsafe_reason.is_none(),
            "{:?}",
            measurement.unsafe_reason
        );
        assert_eq!(measurement.logical_bytes, 7);
        assert_eq!(measurement.files, 1);
        symlink("../../../outside", tree.join(".bin/external")).unwrap();
        assert!(
            measure(&tree, device, &AtomicBool::new(false))
                .unwrap()
                .unsafe_reason
                .is_some()
        );
    }

    #[test]
    fn strict_measurement_still_excludes_application_bundle_contents() {
        let (_temp, base) = fixture();
        for name in ["Generated.app", "Generated.framework", "Generated.bundle"] {
            let tree = base.join(name.replace('.', "-"));
            let bundle = tree.join(name);
            std::fs::create_dir_all(&bundle).unwrap();
            std::fs::write(bundle.join("payload"), b"generated content").unwrap();
            let device = identity(&tree).unwrap().device;
            let default = measure(&tree, device, &AtomicBool::new(false)).unwrap();
            let explicit = measure_with_policy(
                &tree,
                device,
                &AtomicBool::new(false),
                MeasurementPolicy::Strict,
            )
            .unwrap();
            assert_eq!(default.fingerprint, explicit.fingerprint);
            assert_eq!((default.entries, default.files, default.skipped), (2, 0, 1));
            assert_eq!(default.logical_bytes, 0);
            assert!(default.unsafe_reason.is_some(), "{name}");
            assert_eq!(
                std::fs::read(bundle.join("payload")).unwrap(),
                b"generated content"
            );
        }
    }

    #[test]
    fn fallible_observer_preserves_measurement_and_stops_on_failure() {
        let (_temp, base) = fixture();
        let tree = base.join("artifact");
        std::fs::create_dir(&tree).unwrap();
        std::fs::write(tree.join("first"), b"first payload").unwrap();
        std::fs::write(tree.join("second"), b"second payload").unwrap();
        let device = identity(&tree).unwrap().device;
        let cancel = AtomicBool::new(false);
        for policy in [MeasurementPolicy::Strict, MeasurementPolicy::Developer] {
            let expected = measure_with_policy(&tree, device, &cancel, policy).unwrap();
            let mut observed = Vec::new();
            let measured =
                measure_try_observing_with_policy(&tree, device, &cancel, policy, |entry, _| {
                    observed.push((entry.path.clone(), entry.meta.identity.clone()));
                    Ok(())
                })
                .unwrap();
            assert_eq!(observed.len() as u64, expected.entries);
            assert_eq!(observed[0].0, tree);
            assert_eq!(measured.fingerprint, expected.fingerprint);
            assert_eq!(measured.logical_bytes, expected.logical_bytes);
            assert_eq!(measured.allocated_bytes, expected.allocated_bytes);
            assert_eq!(measured.files, expected.files);
            assert_eq!(measured.unsafe_reason, expected.unsafe_reason);

            let mut visits = 0;
            let reason =
                measure_try_observing_with_policy(&tree, device, &cancel, policy, |_, _| {
                    visits += 1;
                    if visits == 2 {
                        Err("Manifest persistence failed".into())
                    } else {
                        Ok(())
                    }
                })
                .unwrap_err();
            assert_eq!(reason, "Manifest persistence failed");
            assert_eq!(
                visits, 2,
                "Observer failure must stop subsequent entry visits"
            );
        }
        assert_eq!(std::fs::read(tree.join("first")).unwrap(), b"first payload");
        assert_eq!(
            std::fs::read(tree.join("second")).unwrap(),
            b"second payload"
        );
    }

    #[test]
    fn developer_bundles_and_framework_links_are_measured_without_following_links() {
        let (_temp, base) = fixture();
        let tree = base.join("target");
        let app = tree.join("Generated.app/Contents");
        let framework = app.join("Frameworks/Generated.framework");
        let bundle = app.join("PlugIns/Theme.bundle");
        std::fs::create_dir_all(app.join("MacOS")).unwrap();
        std::fs::create_dir_all(framework.join("Versions/A")).unwrap();
        std::fs::create_dir_all(&bundle).unwrap();
        std::fs::write(app.join("MacOS/launcher"), b"binary").unwrap();
        std::fs::write(framework.join("Versions/A/Generated"), b"framework").unwrap();
        std::fs::write(bundle.join("payload"), b"theme").unwrap();
        symlink("A", framework.join("Versions/Current")).unwrap();
        symlink("Versions/Current/Generated", framework.join("Generated")).unwrap();
        let device = identity(&tree).unwrap().device;
        let cancel = AtomicBool::new(false);
        let mut visited = Vec::new();
        let full = measure_observing_with_policy(
            &tree,
            device,
            &cancel,
            MeasurementPolicy::Developer,
            |entry, _| visited.push(entry.path.clone()),
        )
        .unwrap();
        assert!(full.unsafe_reason.is_none(), "{:?}", full.unsafe_reason);
        assert_eq!((full.files, full.logical_bytes, full.skipped), (3, 20, 0));
        assert!(!visited.contains(&framework.join("Versions/Current/Generated")));
        let cheap = measure_metadata_observing_with_policy(
            &tree,
            device,
            &cancel,
            MeasurementPolicy::Developer,
            |_, _| {},
        )
        .unwrap();
        assert!(cheap.fingerprint.is_empty());
        assert!(cheap.unsafe_reason.is_none());
        assert_eq!(cheap.logical_bytes, full.logical_bytes);
        assert_eq!(cheap.allocated_bytes, full.allocated_bytes);
        assert_eq!(cheap.files, full.files);
        assert_eq!(cheap.entries, full.entries);
        let staged = base.join(".chippytea-stage");
        std::fs::rename(&tree, &staged).unwrap();
        let staged =
            measure_with_policy(&staged, device, &cancel, MeasurementPolicy::Developer).unwrap();
        assert!(staged.unsafe_reason.is_none());
        assert_eq!(staged.fingerprint, full.fingerprint);
    }

    #[test]
    fn developer_external_dangling_and_cyclic_links_only_fingerprint_their_payloads() {
        let (_temp, base) = fixture();
        let tree = base.join("node_modules");
        std::fs::create_dir(&tree).unwrap();
        let outside = base.join("outside");
        let outside_directory = base.join("external-directory");
        std::fs::create_dir(&outside_directory).unwrap();
        std::fs::write(&outside, [7u8; 8192]).unwrap();
        std::fs::write(outside_directory.join("payload"), [8u8; 4096]).unwrap();
        std::fs::write(tree.join("owned"), b"own").unwrap();
        symlink("../outside", tree.join("external-file")).unwrap();
        symlink("../external-directory", tree.join("external-directory")).unwrap();
        symlink("cycle", tree.join("cycle")).unwrap();
        symlink(OsStr::from_bytes(b"missing-\xff"), tree.join("dangling")).unwrap();
        let device = identity(&tree).unwrap().device;
        let cancel = AtomicBool::new(false);
        let before =
            measure_with_policy(&tree, device, &cancel, MeasurementPolicy::Developer).unwrap();
        assert!(before.unsafe_reason.is_none(), "{:?}", before.unsafe_reason);
        assert_eq!(
            (before.files, before.logical_bytes, before.entries),
            (1, 3, 6)
        );
        assert_eq!(before.skipped, 0);
        assert_eq!(std::fs::read(&outside).unwrap(), [7u8; 8192]);
        assert_eq!(
            std::fs::read(outside_directory.join("payload")).unwrap(),
            [8u8; 4096]
        );
        // A target edit is outside the artifact: it must not affect its size or
        // fingerprint. Changing the link's own payload must affect the review.
        std::fs::write(&outside, b"changed outside the artifact").unwrap();
        let target_changed =
            measure_with_policy(&tree, device, &cancel, MeasurementPolicy::Developer).unwrap();
        assert_eq!(target_changed.fingerprint, before.fingerprint);
        assert_eq!(target_changed.logical_bytes, before.logical_bytes);
        std::fs::remove_file(tree.join("external-file")).unwrap();
        symlink("../external-directory/payload", tree.join("external-file")).unwrap();
        let link_changed =
            measure_with_policy(&tree, device, &cancel, MeasurementPolicy::Developer).unwrap();
        assert_ne!(link_changed.fingerprint, before.fingerprint);
        assert_eq!(link_changed.logical_bytes, before.logical_bytes);
    }

    #[test]
    fn developer_policy_preserves_git_cloud_photo_backup_and_shared_store_exclusions() {
        let (_temp, base) = fixture();
        for (index, name) in [
            ".git",
            "Dropbox",
            "OneDrive.app",
            "Pictures.photoslibrary",
            "Pictures.photolibrary",
            "Collection.musiclibrary",
            "Backups.backupdb",
            "Backup.backupbundle",
            "Disk.sparsebundle",
            "Library",
            ".pnpm",
            ".pnpm-store",
            ".yarn",
            ".store",
        ]
        .into_iter()
        .enumerate()
        {
            let tree = base.join(format!("artifact-{index}"));
            let protected = tree.join(name);
            std::fs::create_dir_all(&protected).unwrap();
            std::fs::write(protected.join("preserve"), b"untouched").unwrap();
            let measured = measure_with_policy(
                &tree,
                identity(&tree).unwrap().device,
                &AtomicBool::new(false),
                MeasurementPolicy::Developer,
            )
            .unwrap();
            assert!(measured.unsafe_reason.is_some(), "{name} must stay blocked");
            assert_eq!(
                (measured.entries, measured.skipped, measured.files),
                (2, 1, 0),
                "{name}"
            );
            assert_eq!(measured.logical_bytes, 0, "{name}");
            assert_eq!(
                std::fs::read(protected.join("preserve")).unwrap(),
                b"untouched"
            );
        }
        let tree = base.join("linked-repository");
        std::fs::create_dir(&tree).unwrap();
        symlink("../artifact-0/.git", tree.join(".git")).unwrap();
        let linked = measure_with_policy(
            &tree,
            identity(&tree).unwrap().device,
            &AtomicBool::new(false),
            MeasurementPolicy::Developer,
        )
        .unwrap();
        assert!(linked.unsafe_reason.is_some());
        assert_eq!(
            (linked.entries, linked.skipped, linked.logical_bytes),
            (2, 1, 0)
        );
    }

    #[test]
    fn developer_bundle_hardlinks_remain_ineligible() {
        let (_temp, base) = fixture();
        let tree = base.join("node_modules");
        let bundle = tree.join("Generated.app");
        std::fs::create_dir_all(&bundle).unwrap();
        std::fs::write(base.join("shared"), b"shared store data").unwrap();
        std::fs::hard_link(base.join("shared"), bundle.join("linked")).unwrap();
        let measured = measure_with_policy(
            &tree,
            identity(&tree).unwrap().device,
            &AtomicBool::new(false),
            MeasurementPolicy::Developer,
        )
        .unwrap();
        assert!(
            measured
                .unsafe_reason
                .as_deref()
                .unwrap()
                .contains("hard-linked")
        );
        assert_eq!(
            std::fs::read(base.join("shared")).unwrap(),
            b"shared store data"
        );
    }

    #[test]
    fn developer_link_payload_rejects_a_replacement_after_enumeration() {
        let (_temp, base) = fixture();
        symlink("first-unresolved-target", base.join("link")).unwrap();
        let entry = Entry {
            path: base.join("link"),
            meta: metadata(&base.join("link")).unwrap(),
        };
        let directory = Directory::open(&base).unwrap();
        symlink("second-unresolved-target", base.join("replacement")).unwrap();
        std::fs::rename(base.join("replacement"), base.join("link")).unwrap();
        assert!(
            developer_link_payload(&entry, Some(&directory), &AtomicBool::new(false))
                .unwrap_err()
                .contains("changed before")
        );
        assert_eq!(
            std::fs::read_link(base.join("link")).unwrap(),
            PathBuf::from("second-unresolved-target")
        );
    }

    #[test]
    fn fingerprint_survives_exclusive_sibling_staging() {
        let (_temp, base) = fixture();
        let tree = base.join("artifact");
        std::fs::create_dir(&tree).unwrap();
        std::fs::write(tree.join("payload"), b"disposable").unwrap();
        let device = identity(&tree).unwrap().device;
        let before = measure(&tree, device, &AtomicBool::new(false)).unwrap();
        let staged = base.join(".chippytea-stage");
        std::fs::rename(&tree, &staged).unwrap();
        let after = measure(&staged, device, &AtomicBool::new(false)).unwrap();
        assert_eq!(before.fingerprint, after.fingerprint);
    }

    #[cfg(target_os = "macos")]
    fn bulk_record_fixture() -> Vec<u8> {
        let name = b"fixture\0";
        let length = (BULK_FILE_BYTES + name.len() + 7) & !7;
        let mut record = vec![0u8; length];
        for (offset, value) in [
            (0, length as u32),
            (4, BULK_COMMON),
            (16, BULK_FILE),
            (28, (BULK_FILE_BYTES - 28) as u32),
            (32, name.len() as u32),
            (36, 123),
            (40, 1),
            (76, 501),
            (80, 0o640),
            (84, 0x4000_0000),
            (96, 2),
        ] {
            record[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
        }
        for (offset, value) in [
            (44, 101u64),
            (52, 123),
            (60, 202),
            (68, 456),
            (88, 999),
            (100, 8192),
            (108, 4097),
        ] {
            record[offset..offset + 8].copy_from_slice(&value.to_ne_bytes());
        }
        record[BULK_FILE_BYTES..BULK_FILE_BYTES + name.len()].copy_from_slice(name);
        record
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn bulk_parser_requires_valid_returned_fields_and_checked_name_references() {
        let valid = bulk_record_fixture();
        let parsed = parse_bulk_record(&valid).unwrap();
        assert_eq!(parsed.name, b"fixture");
        let meta = parsed.metadata.unwrap();
        assert_eq!(meta.identity.inode, 999);
        assert_eq!(meta.identity.device, 123);
        assert_eq!(meta.identity.modified_ns, 101_000_000_123);
        assert_eq!(meta.identity.changed_ns, 202_000_000_456);
        assert_eq!(meta.identity.mode, libc::S_IFREG as u32 | 0o640);
        assert_eq!(
            (meta.identity.size, meta.allocated, meta.links, meta.uid),
            (4097, 8192, 2, 501)
        );
        assert!(meta.is_dataless());

        let mut missing = valid.clone();
        missing[16..20].copy_from_slice(&(BULK_FILE & !libc::ATTR_FILE_LINKCOUNT).to_ne_bytes());
        assert!(parse_bulk_record(&missing).unwrap().metadata.is_none());
        let mut failed = valid.clone();
        failed[24..28].copy_from_slice(&(libc::EACCES as u32).to_ne_bytes());
        assert!(parse_bulk_record(&failed).unwrap().metadata.is_none());
        let mut invalid_nanos = valid.clone();
        invalid_nanos[52..60].copy_from_slice(&1_000_000_000u64.to_ne_bytes());
        assert!(
            parse_bulk_record(&invalid_nanos)
                .unwrap()
                .metadata
                .is_none()
        );
        for (offset, value) in [
            (0, u32::MAX),
            (28, i32::MAX as u32),
            (28, 0),
            (32, u32::MAX),
            (4, 0),
        ] {
            let mut invalid = valid.clone();
            invalid[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
            assert!(parse_bulk_record(&invalid).is_err());
        }
        let mut unterminated = valid;
        unterminated[BULK_FILE_BYTES + 7] = b'x';
        assert!(parse_bulk_record(&unterminated).is_err());
    }

    #[cfg(target_os = "macos")]
    fn bulk_reader(directory: &mut Directory) -> &mut BulkReader {
        let DirectoryReader::Bulk { reader, .. } = &mut directory.reader else {
            panic!("Native bulk must be exercised, not masked by fallback")
        };
        reader
    }

    #[cfg(target_os = "macos")]
    fn directory_entries(
        path: &Path,
        legacy: bool,
    ) -> std::collections::BTreeMap<PathBuf, EntryMeta> {
        let mut directory = Directory::open(path).unwrap();
        if legacy {
            directory.use_legacy_reader().unwrap();
        }
        let mut entries = std::collections::BTreeMap::new();
        while let Some(entry) = directory.next(&AtomicBool::new(false)).unwrap() {
            assert!(
                entries.insert(entry.path, entry.meta).is_none(),
                "No entry may be emitted twice"
            );
        }
        assert_eq!(
            matches!(directory.reader, DirectoryReader::Names(_)),
            legacy,
            "Native bulk must be exercised, not masked by fallback"
        );
        directory.unchanged().unwrap();
        entries
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_bulk_matches_nofollow_stat_for_sparse_linked_and_forked_files() {
        let (_temp, base) = fixture();
        std::fs::write(base.join("regular"), [9u8; 4096]).unwrap();
        std::fs::hard_link(base.join("regular"), base.join("hard-link")).unwrap();
        std::fs::File::create(base.join("sparse"))
            .unwrap()
            .set_len(4 * 1024 * 1024)
            .unwrap();
        std::fs::create_dir(base.join("directory")).unwrap();
        symlink("regular", base.join("file-link")).unwrap();
        symlink("directory", base.join("directory-link")).unwrap();
        std::fs::write(base.join("forked"), [3u8; 4096]).unwrap();
        std::fs::write(base.join("forked/..namedfork/rsrc"), [4u8; 8192]).unwrap();
        let pipe = c_name(base.join("pipe").as_os_str()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(pipe.as_ptr(), 0o600) }, 0);
        let bulk = directory_entries(&base, false);
        let legacy = directory_entries(&base, true);
        assert_eq!(bulk, legacy);
        assert!(bulk[&base.join("file-link")].is_symlink());
        assert_eq!(bulk[&base.join("sparse")].allocated, 0);
        assert_eq!(bulk[&base.join("hard-link")].links, 2);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn unsupported_bulk_restarts_on_a_fresh_descriptor_without_missing_entries() {
        let (_temp, base) = fixture();
        for index in 0..700 {
            std::fs::write(base.join(format!("file-{index:04}")), b"x").unwrap();
        }
        let expected = directory_entries(&base, true);
        let mut directory = Directory::open(&base).unwrap();
        bulk_reader(&mut directory).injected_errno = Some(libc::ENOTSUP);
        let mut actual = std::collections::BTreeMap::new();
        while let Some(entry) = directory.next(&AtomicBool::new(false)).unwrap() {
            assert!(actual.insert(entry.path, entry.meta).is_none());
        }
        assert!(matches!(directory.reader, DirectoryReader::Names(_)));
        assert_eq!(actual, expected);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn bulk_failure_after_progress_is_partial_and_never_restarts_with_duplicates() {
        let (_temp, base) = fixture();
        for index in 0..700 {
            std::fs::write(base.join(format!("file-{index:04}")), b"x").unwrap();
        }
        let mut directory = Directory::open(&base).unwrap();
        let mut emitted = 0;
        loop {
            assert!(directory.next(&AtomicBool::new(false)).unwrap().is_some());
            emitted += 1;
            if bulk_reader(&mut directory).remaining == 0 {
                break;
            }
        }
        assert!(emitted < 700);
        bulk_reader(&mut directory).injected_errno = Some(libc::ENOTSUP);
        assert!(
            directory
                .next(&AtomicBool::new(false))
                .unwrap_err()
                .contains("incomplete")
        );
        assert!(matches!(directory.reader, DirectoryReader::Bulk { .. }));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn cancellation_checks_before_bulk_fetch_and_between_buffered_records() {
        let (_temp, base) = fixture();
        std::fs::write(base.join("first"), b"one").unwrap();
        std::fs::write(base.join("second"), b"two").unwrap();
        let mut directory = Directory::open(&base).unwrap();
        assert!(directory.next(&AtomicBool::new(true)).is_err());
        assert!(bulk_reader(&mut directory).buffer.is_none());
        assert!(directory.next(&AtomicBool::new(false)).unwrap().is_some());
        let offset = bulk_reader(&mut directory).offset;
        assert!(directory.next(&AtomicBool::new(true)).is_err());
        assert_eq!(bulk_reader(&mut directory).offset, offset);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn names_reader_cannot_share_a_fetched_bulk_offset_before_first_emission() {
        let (_temp, base) = fixture();
        std::fs::write(base.join("payload"), b"preserve").unwrap();
        let mut directory = Directory::open(&base).unwrap();
        let fd = directory.fd();
        bulk_reader(&mut directory)
            .fetch(fd, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(bulk_reader(&mut directory).emitted, 0);
        assert!(
            directory
                .next_discovery(&AtomicBool::new(false), false)
                .is_err()
        );
        assert_eq!(
            directory
                .next(&AtomicBool::new(false))
                .unwrap()
                .unwrap()
                .path,
            base.join("payload")
        );
        assert!(directory.next(&AtomicBool::new(false)).unwrap().is_none());
        directory.unchanged().unwrap();
    }

    #[test]
    fn metadata_only_measurement_keeps_counts_and_newest_excluded_boundary() {
        let (_temp, base) = fixture();
        std::fs::write(base.join("payload"), b"one").unwrap();
        std::fs::create_dir(base.join(".git")).unwrap();
        let device = identity(&base).unwrap().device;
        let full = measure(&base, device, &AtomicBool::new(false)).unwrap();
        let cheap =
            measure_metadata_observing(&base, device, &AtomicBool::new(false), |_, _| {}).unwrap();
        assert!(cheap.fingerprint.is_empty());
        assert!(!full.fingerprint.is_empty());
        assert_eq!(cheap.logical_bytes, full.logical_bytes);
        assert_eq!(cheap.allocated_bytes, full.allocated_bytes);
        assert_eq!(cheap.entries, full.entries);
        assert_eq!(cheap.skipped, full.skipped);
        assert_eq!(cheap.latest_modified_ns, full.latest_modified_ns);
        assert!(cheap.latest_modified_ns >= identity(&base.join(".git")).unwrap().modified_ns);
    }

    #[test]
    fn raw_traversal_includes_protected_names_and_directory_allocations() {
        let (_temp, base) = fixture();
        std::fs::create_dir(base.join(".git")).unwrap();
        std::fs::create_dir(base.join("Library")).unwrap();
        std::fs::create_dir(base.join("Collection.musiclibrary")).unwrap();
        std::fs::write(base.join(".git/a"), [1u8; 4096]).unwrap();
        std::fs::write(base.join("Library/b"), [2u8; 8192]).unwrap();
        std::fs::write(base.join("Collection.musiclibrary/c"), [3u8; 4096]).unwrap();
        symlink(".git/a", base.join("link")).unwrap();
        let stats = traverse_metadata(&base, &AtomicBool::new(false)).unwrap();
        let allocated: u64 = [
            "",
            ".git",
            "Library",
            "Collection.musiclibrary",
            ".git/a",
            "Library/b",
            "Collection.musiclibrary/c",
            "link",
        ]
        .iter()
        .map(|relative| metadata(&base.join(relative)).unwrap().allocated)
        .sum();
        assert!(stats.complete, "{}", stats.message);
        assert_eq!((stats.entries, stats.files, stats.directories), (8, 3, 4));
        assert_eq!(stats.logical_bytes, 16_384);
        assert_eq!(stats.allocated_bytes, allocated);
    }
}
