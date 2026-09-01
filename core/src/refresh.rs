//! Bounded filesystem-event work. Event receipt is purely lexical; filesystem
//! probes and index reconciliation belong to the discovery worker.
use crate::{model::*, safety};
use serde::Deserialize;
use std::collections::VecDeque;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// File events carry their type even when the item has already disappeared.
/// Unknown events retain an exact, conservative refresh scope.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum EventKind {
    File,
    Directory,
    #[default]
    Unknown,
}

#[derive(Debug, Deserialize)]
pub(crate) struct FsEvent {
    pub path: PathBuf,
    #[serde(default)]
    pub kind: EventKind,
    /// Set for structural directory changes or coalesced subtree events.
    #[serde(default)]
    pub recursive: bool,
}

const RECENT_FILE_HINTS_MAX_COUNT: usize = 64;
// The owned root IDs and paths share this budget; entry overhead is separately
// bounded by MAX_COUNT. No file contents or filesystem observations are cached.
const RECENT_FILE_HINTS_MAX_BYTES: usize = 64 * 1024;

#[derive(Clone)]
struct RecentFileHint {
    root_id: String,
    artifact: PathBuf,
    leaf: PathBuf,
    bytes: usize,
}

/// Disposable paths that may help discovery prove an artifact is still active.
/// Event receipt and successful discovery can supply paths, never observations.
/// A worker edits a bounded clone and returns it only if no later receipt or
/// invalidation changed the runtime pool. Every hit needs fresh validation.
#[derive(Clone, Default)]
pub(crate) struct RecentFileHints {
    entries: VecDeque<RecentFileHint>,
    bytes: usize,
    revision: u64,
}

impl RecentFileHints {
    pub(crate) fn insert(&mut self, root: &Root, event: &FsEvent, scope: &Path) {
        if event.kind == EventKind::File && !event.recursive {
            self.remember(root, scope, &event.path);
        }
    }

    /// Called after every relevant dirty batch commits, including batches that
    /// supply no usable file hint. It protects later events from worker writeback.
    pub(crate) fn received_batch(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    pub(crate) fn snapshot(&self) -> (u64, Self) {
        (self.revision, self.clone())
    }

    pub(crate) fn replace_if_unchanged(&mut self, revision: u64, next: Self) -> bool {
        if self.revision != revision {
            return false;
        }
        *self = next;
        true
    }

    pub(crate) fn get(&self, root_id: &str, artifact: &Path) -> Option<&Path> {
        self.entries
            .iter()
            .find(|hint| hint.root_id == root_id && hint.artifact == artifact)
            .map(|hint| hint.leaf.as_path())
    }

    /// Learning has the same lexical restrictions as an event-derived path.
    /// The caller must first observe a safe recent regular file in a successfully
    /// pruned Suggestions measurement; later reuse still proves everything anew.
    pub(crate) fn remember(&mut self, root: &Root, scope: &Path, leaf: &Path) {
        let Some(bytes) = root
            .id
            .len()
            .checked_add(scope.as_os_str().len())
            .and_then(|bytes| bytes.checked_add(leaf.as_os_str().len()))
            .filter(|bytes| *bytes <= RECENT_FILE_HINTS_MAX_BYTES)
        else {
            return;
        };
        if scope == root.path
            || !scope
                .file_name()
                .is_some_and(crate::scanner::artifact_component)
            || leaf == scope
            || !leaf.starts_with(scope)
            || leaf.as_os_str().as_encoded_bytes().contains(&0)
            || safety::check_scope_policy(root, leaf).is_err()
            || event_scope_with_kind(root, leaf, EventKind::File, false)
                .ok()
                .flatten()
                .as_deref()
                != Some(scope)
        {
            return;
        }
        if let Some(index) = self
            .entries
            .iter()
            .position(|hint| hint.root_id == root.id && hint.artifact == scope)
        {
            self.bytes -= self.entries.remove(index).unwrap().bytes;
        }
        while self.entries.len() >= RECENT_FILE_HINTS_MAX_COUNT
            || self.bytes > RECENT_FILE_HINTS_MAX_BYTES - bytes
        {
            self.bytes -= self.entries.pop_front().unwrap().bytes;
        }
        self.entries.push_back(RecentFileHint {
            root_id: root.id.clone(),
            artifact: scope.to_path_buf(),
            leaf: leaf.to_path_buf(),
            bytes,
        });
        self.bytes += bytes;
        self.received_batch();
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
        self.received_batch();
    }

    pub(crate) fn remove(&mut self, root_id: &str, artifact: &Path) {
        if let Some(index) = self
            .entries
            .iter()
            .position(|hint| hint.root_id == root_id && hint.artifact == artifact)
        {
            self.bytes -= self.entries.remove(index).unwrap().bytes;
            self.received_batch();
        }
    }
}

/// Known excluded directories cannot affect a cleanup recommendation. Ignore
/// their churn (including this app's own Library database), but invalidate a
/// repository when its tracking/configuration evidence changes.
pub(crate) fn event_scope(root: &Root, path: &Path) -> Result<Option<PathBuf>> {
    event_scope_with_kind(root, path, EventKind::Unknown, false)
}

pub(crate) fn event_scope_with_kind(
    root: &Root,
    path: &Path,
    kind: EventKind,
    recursive: bool,
) -> Result<Option<PathBuf>> {
    safety::absolute_components(path)?;
    let relative = path
        .strip_prefix(&root.path)
        .map_err(|_| "Filesystem event is outside its authorized root")?;
    if safety::excluded_home_media(root, path) {
        return Ok(None);
    }
    let mut current = root.path.clone();
    for component in relative.components() {
        let name = component.as_os_str();
        if name == ".cargo" {
            let suffix = path
                .strip_prefix(current.join(".cargo"))
                .unwrap_or(Path::new(""));
            if suffix.as_os_str().is_empty()
                || suffix == Path::new("config")
                || suffix == Path::new("config.toml")
            {
                if suffix.as_os_str().is_empty() && kind == EventKind::Directory && !recursive {
                    return Ok(None);
                }
                return Ok(Some(current));
            }
        }
        if name == ".git" {
            // Object writes and logs do not change which working files are tracked.
            let suffix = path
                .strip_prefix(current.join(".git"))
                .unwrap_or(Path::new(""));
            // Commit text is not tracking or ownership evidence. Keep unknown
            // and recursive events conservative, including this file's parent.
            if kind == EventKind::File && !recursive && suffix == Path::new("COMMIT_EDITMSG") {
                return Ok(None);
            }
            if suffix.starts_with("objects")
                || suffix.starts_with("logs")
                || (kind == EventKind::Directory && !recursive)
            {
                return Ok(None);
            }
            return Ok(Some(current));
        }
        if safety::excluded_name(name, true) || name.to_string_lossy().starts_with(".chippytea-") {
            return Ok(None);
        }
        current.push(name);
        if crate::scanner::artifact_component(name) {
            return Ok(Some(current));
        }
    }
    // Cargo reads only the immediate project's lockfile for a default target.
    // Keep its exact path as the durable selector for a compound refresh of the
    // original item and its sibling target. Unknown/recursive events still
    // reconcile the whole project, including possible structural changes.
    if path != root.path
        && path.file_name() == Some(OsStr::new("Cargo.lock"))
        && kind == EventKind::File
        && !recursive
    {
        return Ok(Some(path.to_path_buf()));
    }
    // Other ownership evidence can change eligibility throughout a workspace. These
    // invalidations are deliberate; an ordinary log or shell-history write is
    // not a reason to traverse its parent, which may be the entire Home folder.
    if path != root.path && path.file_name().is_some_and(ownership_evidence) {
        return Ok(path.parent().map(Path::to_path_buf));
    }
    match kind {
        EventKind::File if !recursive && !in_downloads(root, path) => return Ok(None),
        // FileEvents reports children individually. A directory metadata-only
        // notification does not require revisiting all of those descendants.
        EventKind::Directory if !recursive => return Ok(None),
        _ => {}
    }
    Ok(Some(path.to_path_buf()))
}

/// An exact lockfile claim replays both affected footprints after interruption.
/// This identifies the dependency, not the child's existence or spelling on a
/// case-insensitive volume; discovery must establish those at execution time.
pub(crate) fn cargo_lock_target(root: &Root, origin: &Path) -> Result<Option<PathBuf>> {
    let normalized = normalize_scope(root, origin)?;
    if origin == root.path
        || origin.file_name() != Some(OsStr::new("Cargo.lock"))
        || normalized != origin
        || safety::check_scope_policy(root, origin).is_err()
    {
        return Ok(None);
    }
    Ok(origin.parent().map(|parent| parent.join("target")))
}

fn ownership_evidence(name: &OsStr) -> bool {
    matches!(
        name.to_str(),
        Some(
            "package.json"
                | "npm-shrinkwrap.json"
                | "package-lock.json"
                | "yarn.lock"
                | "bun.lock"
                | "bun.lockb"
                | "pnpm-lock.yaml"
                | "pnpm-workspace.yaml"
                | "bunfig.toml"
                | ".npmrc"
                | ".yarnrc"
                | ".yarnrc.yml"
                | "Cargo.toml"
                | "Cargo.lock"
                | "pyvenv.cfg"
        )
    )
}

fn in_downloads(root: &Root, path: &Path) -> bool {
    match root.kind.as_str() {
        "downloads" => true,
        "folder" if root.path.file_name() == Some(OsStr::new("Downloads")) => true,
        "folder" | "home" => path.starts_with(root.path.join("Downloads")),
        _ => false,
    }
}

pub(crate) fn resolve_scope(root: &Root, path: &Path, indexed: Option<PathBuf>) -> Result<PathBuf> {
    safety::absolute_components(path)?;
    if !path.starts_with(&root.path) {
        return Err("Refresh is outside its authorized root".into());
    }
    if let Some(indexed) = indexed.as_deref() {
        safety::absolute_components(indexed)?;
        if !indexed.starts_with(&root.path) || !path.starts_with(indexed) {
            return Err("The indexed refresh scope does not enclose this event".into());
        }
    }
    // An older indexed ancestor must not erase a protected suffix. Keep that
    // exact scope for the worker's reconciliation without filesystem access.
    if safety::check_scope_policy(root, path).is_err() {
        return Ok(path.to_path_buf());
    }
    normalize_scope(root, indexed.as_deref().unwrap_or(path))
}

/// The scanner and refresh journal must cover the same artifact. This is purely
/// lexical: ordinary missing paths stay exact, and protected inputs stay intact
/// so callers can reject or reconcile them without probing their contents.
pub(crate) fn normalize_scope(root: &Root, path: &Path) -> Result<PathBuf> {
    safety::absolute_components(path)?;
    if !path.starts_with(&root.path) {
        return Err("Refresh is outside its authorized root".into());
    }
    if safety::check_scope_policy(root, path).is_err() {
        return Ok(path.to_path_buf());
    }
    let selected = path
        .ancestors()
        .take_while(|ancestor| ancestor.starts_with(&root.path))
        .filter(|ancestor| {
            *ancestor != root.path
                && ancestor
                    .file_name()
                    .is_some_and(crate::scanner::artifact_component)
        })
        .last()
        .unwrap_or(path);
    Ok(selected.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn root() -> Root {
        Root {
            id: "root".into(),
            path: "/Users/test".into(),
            kind: "folder".into(),
            identity: Identity {
                device: 1,
                inode: 1,
                mode: 0,
                size: 0,
                modified_ns: 0,
                changed_ns: 0,
            },
        }
    }

    fn file_event(path: PathBuf) -> FsEvent {
        FsEvent {
            path,
            kind: EventKind::File,
            recursive: false,
        }
    }

    #[test]
    fn recent_hints_keep_only_the_latest_leaf_for_the_outer_artifact() {
        let root = root();
        let artifact = root.path.join("project/node_modules");
        let nested = artifact.join("package/target");
        let first = file_event(artifact.join("package/first"));
        let latest = file_event(nested.join("debug/latest"));
        let mut hints = RecentFileHints::default();
        hints.insert(&root, &first, &artifact);
        hints.insert(&root, &latest, &artifact);
        assert_eq!(hints.entries.len(), 1);
        assert_eq!(hints.get(&root.id, &nested), None);
        assert_eq!(hints.get(&root.id, &artifact), Some(latest.path.as_path()));
        assert_eq!(hints.get(&root.id, &artifact), Some(latest.path.as_path()));
        hints.remove(&root.id, &artifact);
        assert_eq!(hints.get(&root.id, &artifact), None);
        assert_eq!(hints.bytes, 0);
    }

    #[test]
    fn recent_hints_reject_unsafe_or_mismatched_events_without_replacing_a_hint() {
        let root = root();
        let artifact = root.path.join("project/target");
        let valid = file_event(artifact.join("debug/valid"));
        let mut hints = RecentFileHints::default();
        hints.insert(&root, &valid, &artifact);
        for (kind, recursive) in [
            (EventKind::Unknown, false),
            (EventKind::Directory, false),
            (EventKind::File, true),
        ] {
            let event = FsEvent {
                path: artifact.join("debug/other"),
                kind,
                recursive,
            };
            hints.insert(&root, &event, &artifact);
        }
        for (path, scope) in [
            (artifact.clone(), artifact.clone()),
            (artifact.join("../other/file"), artifact.clone()),
            (artifact.join(".git/index"), artifact.clone()),
            (artifact.join("Dropbox/file"), artifact.clone()),
            (artifact.join("Library/file"), artifact.clone()),
            (
                artifact.join("Generated.app/Contents/file"),
                artifact.clone(),
            ),
            (artifact.join("debug/invalid\0name"), artifact.clone()),
            (root.path.join("project/Cargo.lock"), artifact.clone()),
            (artifact.join("debug/other"), root.path.join("project")),
            (
                artifact.join("member/node_modules/package/file"),
                artifact.join("member/node_modules"),
            ),
            (
                root.path.join("project/Target/file"),
                root.path.join("project/Target"),
            ),
            (
                PathBuf::from("/Users/test-neighbor/project/target/file"),
                PathBuf::from("/Users/test-neighbor/project/target"),
            ),
            (
                PathBuf::from("relative/project/target/file"),
                PathBuf::from("relative/project/target"),
            ),
        ] {
            hints.insert(&root, &file_event(path), &scope);
        }
        let mut home = root.clone();
        home.kind = "home".into();
        let media = home.path.join("Music/project/target");
        hints.insert(&home, &file_event(media.join("file")), &media);
        let mut artifact_grant = root.clone();
        artifact_grant.path = artifact.clone();
        hints.insert(&artifact_grant, &valid, &artifact);
        assert_eq!(hints.entries.len(), 1);
        assert_eq!(hints.get(&root.id, &artifact), Some(valid.path.as_path()));
        hints.remove(&root.id, &artifact);
        assert_eq!(hints.bytes, 0);
    }

    #[test]
    fn recent_hints_evict_the_oldest_artifact_after_replacement() {
        let root = root();
        let mut hints = RecentFileHints::default();
        let artifacts = (0..=RECENT_FILE_HINTS_MAX_COUNT)
            .map(|index| root.path.join(format!("project-{index}/target")))
            .collect::<Vec<_>>();
        for artifact in &artifacts[..RECENT_FILE_HINTS_MAX_COUNT] {
            hints.insert(&root, &file_event(artifact.join("file")), artifact);
        }
        let latest = file_event(artifacts[0].join("newest"));
        hints.insert(&root, &latest, &artifacts[0]);
        let last = artifacts.last().unwrap();
        hints.insert(&root, &file_event(last.join("file")), last);
        assert_eq!(hints.entries.len(), RECENT_FILE_HINTS_MAX_COUNT);
        assert!(hints.bytes <= RECENT_FILE_HINTS_MAX_BYTES);
        assert_eq!(hints.get(&root.id, &artifacts[1]), None);
        assert_eq!(
            hints.get(&root.id, &artifacts[0]),
            Some(latest.path.as_path())
        );
        for artifact in &artifacts[2..] {
            assert_eq!(
                hints.get(&root.id, artifact),
                Some(artifact.join("file").as_path())
            );
        }
        hints.clear();
        assert_eq!(hints.bytes, 0);
    }

    #[test]
    fn recent_hints_bound_retained_bytes_and_ignore_oversized_replacements() {
        let root = root();
        let artifact = root.path.join("project/target");
        let prefix_bytes = root.id.len() + 2 * artifact.as_os_str().len() + 1;
        // These paths are lexical inputs only: no filesystem access is needed
        // to establish either the exact budget or an oversized event's refusal.
        let exact =
            file_event(artifact.join("x".repeat(RECENT_FILE_HINTS_MAX_BYTES - prefix_bytes)));
        let oversized = file_event(artifact.join("x".repeat(RECENT_FILE_HINTS_MAX_BYTES)));
        let mut hints = RecentFileHints::default();
        hints.insert(&root, &exact, &artifact);
        assert_eq!(hints.bytes, RECENT_FILE_HINTS_MAX_BYTES);
        hints.insert(&root, &oversized, &artifact);
        assert_eq!(hints.bytes, RECENT_FILE_HINTS_MAX_BYTES);
        assert_eq!(hints.get(&root.id, &artifact), Some(exact.path.as_path()));
        let sibling = root.path.join("other/node_modules");
        let next = file_event(sibling.join("package/file"));
        hints.insert(&root, &next, &sibling);
        assert_eq!(hints.get(&root.id, &artifact), None);
        assert_eq!(hints.get(&root.id, &sibling), Some(next.path.as_path()));
        hints.remove(&root.id, &sibling);
        assert_eq!(hints.bytes, 0);
    }

    #[test]
    fn recent_hints_lookup_and_removal_are_exact_and_root_scoped() {
        let root = root();
        let mut other = root.clone();
        other.id = "other-root".into();
        let project = root.path.join("project");
        let artifacts = [
            project.join("target"),
            project.join("node_modules"),
            root.path.join("project-neighbor/target"),
        ];
        let mut hints = RecentFileHints::default();
        for grant in [&root, &other] {
            for artifact in &artifacts {
                hints.insert(grant, &file_event(artifact.join("file")), artifact);
            }
        }
        assert_eq!(hints.get(&root.id, &project), None);
        hints.remove(&root.id, &project);
        assert_eq!(hints.entries.len(), 6);
        hints.remove(&root.id, &artifacts[0]);
        assert_eq!(hints.get(&root.id, &artifacts[0]), None);
        assert_eq!(
            hints.get(&root.id, &artifacts[1]),
            Some(artifacts[1].join("file").as_path())
        );
        assert_eq!(
            hints.get(&other.id, &artifacts[0]),
            Some(artifacts[0].join("file").as_path())
        );
        hints.clear();
        assert!(hints.entries.is_empty());
        assert_eq!(hints.bytes, 0);
    }

    #[test]
    fn recent_hints_writeback_requires_the_same_receipt_revision() {
        let root = root();
        let artifact = root.path.join("project/target");
        let first = artifact.join("first");
        let next = artifact.join("next");
        let mut hints = RecentFileHints::default();
        let (revision, mut local) = hints.snapshot();
        local.remember(&root, &artifact, &first);
        assert!(hints.replace_if_unchanged(revision, local));
        assert_eq!(hints.get(&root.id, &artifact), Some(first.as_path()));

        for change in ["event", "receipt", "clear"] {
            let (revision, mut local) = hints.snapshot();
            local.remove(&root.id, &artifact);
            match change {
                "event" => hints.insert(&root, &file_event(next.clone()), &artifact),
                "receipt" => hints.received_batch(),
                "clear" => hints.clear(),
                _ => unreachable!(),
            }
            assert!(!hints.replace_if_unchanged(revision, local));
            assert_eq!(
                hints.get(&root.id, &artifact),
                (change != "clear").then_some(next.as_path())
            );
        }
    }

    #[test]
    fn protected_churn_is_ignored_but_tracking_changes_refresh_the_project() {
        let root = root();
        assert_eq!(
            event_scope(
                &root,
                Path::new("/Users/test/Library/Application Support/chippytea/library.sqlite-wal")
            )
            .unwrap(),
            None
        );
        assert_eq!(
            event_scope(&root, Path::new("/Users/test/project/.git/objects/aa/file")).unwrap(),
            None
        );
        assert_eq!(
            event_scope(&root, Path::new("/Users/test/project/.git/index")).unwrap(),
            Some("/Users/test/project".into())
        );
        assert_eq!(
            event_scope(
                &root,
                Path::new("/Users/test/project/node_modules/pkg/file")
            )
            .unwrap(),
            Some("/Users/test/project/node_modules".into())
        );
        assert!(event_scope(&root, Path::new("/Users/test-neighbor/file")).is_err());
        assert_eq!(
            event_scope(&root, Path::new("/Users/test/project/.cargo/config.toml")).unwrap(),
            Some("/Users/test/project".into())
        );
    }

    #[test]
    fn ordinary_file_events_do_not_rescan_home_or_application_state() {
        let root = root();
        for path in [
            "/Users/test/.zsh_history",
            "/Users/test/.tool/state.sqlite-wal",
            "/Users/test/.tool/already-removed.tmp",
            "/Users/test/project/src/lib.rs",
        ] {
            assert_eq!(
                event_scope_with_kind(&root, Path::new(path), EventKind::File, false).unwrap(),
                None,
                "{path}"
            );
        }
        assert_eq!(
            event_scope_with_kind(
                &root,
                Path::new("/Users/test/Downloads/archive.zip"),
                EventKind::File,
                false,
            )
            .unwrap(),
            Some("/Users/test/Downloads/archive.zip".into())
        );
    }

    #[test]
    fn only_exact_nonrecursive_commit_message_file_events_are_ignored() {
        let root = root();
        let project = Path::new("/Users/test/project");
        let message = project.join(".git/COMMIT_EDITMSG");
        assert_eq!(
            event_scope_with_kind(&root, &message, EventKind::File, false).unwrap(),
            None
        );
        for (kind, recursive) in [
            (EventKind::Unknown, false),
            (EventKind::Unknown, true),
            (EventKind::File, true),
            (EventKind::Directory, true),
        ] {
            assert_eq!(
                event_scope_with_kind(&root, &message, kind, recursive).unwrap(),
                Some(project.to_path_buf())
            );
        }
        for name in [
            ".git",
            ".git/index",
            ".git/config",
            ".git/config.worktree",
            ".git/HEAD",
            ".git/commondir",
            ".git/COMMIT_EDITMSG.lock",
            ".git/COMMIT_EDITMSG/child",
            ".git/refs/heads/COMMIT_EDITMSG",
            ".git/worktrees/linked/COMMIT_EDITMSG",
        ] {
            assert_eq!(
                event_scope_with_kind(&root, &project.join(name), EventKind::File, false).unwrap(),
                Some(project.to_path_buf()),
                "{name}"
            );
        }
        // A protected entry arriving inside an artifact still invalidates it.
        assert_eq!(
            event_scope_with_kind(
                &root,
                &project.join("target/.git/COMMIT_EDITMSG"),
                EventKind::File,
                false,
            )
            .unwrap(),
            Some(project.join("target"))
        );
    }

    #[test]
    fn home_media_events_are_ignored_without_hiding_projects_named_music() {
        let mut root = root();
        root.kind = "home".into();
        for path in [
            "/Users/test/Music",
            "/Users/test/Pictures/Photos.photoslibrary",
            "/Users/test/Movies/project/target",
            "/Users/test/Music/project/package.json",
        ] {
            for kind in [EventKind::File, EventKind::Directory, EventKind::Unknown] {
                assert_eq!(
                    event_scope_with_kind(&root, Path::new(path), kind, true).unwrap(),
                    None,
                    "{path} ({kind:?})"
                );
            }
        }
        assert_eq!(
            event_scope(
                &root,
                Path::new("/Users/test/Projects/Music/target/debug/output")
            )
            .unwrap(),
            Some("/Users/test/Projects/Music/target".into())
        );
        assert_eq!(
            event_scope_with_kind(
                &root,
                Path::new("/Users/test/Downloads/review.dmg"),
                EventKind::File,
                false,
            )
            .unwrap(),
            Some("/Users/test/Downloads/review.dmg".into())
        );
        root.kind = "projects".into();
        assert_eq!(
            event_scope(&root, Path::new("/Users/test/Music/target/debug/output")).unwrap(),
            Some("/Users/test/Music/target".into())
        );
    }

    #[test]
    fn structural_events_keep_their_subtree_but_directory_metadata_does_not() {
        let root = root();
        for path in [
            "/Users/test",
            "/Users/test/project",
            "/Users/test/.cargo",
            "/Users/test/project/.git",
        ] {
            assert_eq!(
                event_scope_with_kind(&root, Path::new(path), EventKind::Directory, false).unwrap(),
                None,
                "{path}"
            );
        }
        assert_eq!(
            event_scope_with_kind(
                &root,
                Path::new("/Users/test/new-project"),
                EventKind::Directory,
                true,
            )
            .unwrap(),
            Some("/Users/test/new-project".into())
        );
        assert_eq!(
            event_scope_with_kind(&root, &root.path, EventKind::Directory, true).unwrap(),
            Some(root.path.clone())
        );
        assert_eq!(
            event_scope_with_kind(
                &root,
                Path::new("/Users/test/Library"),
                EventKind::Directory,
                true,
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn ownership_evidence_and_artifact_changes_still_invalidate_recommendations() {
        let root = root();
        for name in [
            "package.json",
            "package-lock.json",
            "pnpm-workspace.yaml",
            "bun.lock",
            "bunfig.toml",
            "Cargo.toml",
            ".cargo/config.toml",
            ".git/index",
        ] {
            assert_eq!(
                event_scope_with_kind(
                    &root,
                    &Path::new("/Users/test/project").join(name),
                    EventKind::File,
                    false,
                )
                .unwrap(),
                Some("/Users/test/project".into()),
                "{name}"
            );
        }
        for path in [
            "/Users/test/project/target/debug/output",
            "/Users/test/project/target/debug/Generated.app/Contents/MacOS/binary",
        ] {
            assert_eq!(
                event_scope_with_kind(&root, Path::new(path), EventKind::File, false).unwrap(),
                Some("/Users/test/project/target".into())
            );
        }
        assert_eq!(
            event_scope_with_kind(
                &root,
                Path::new("/Users/test/project/node_modules/package"),
                EventKind::Directory,
                false,
            )
            .unwrap(),
            Some("/Users/test/project/node_modules".into())
        );
    }

    #[test]
    fn only_exact_file_lock_events_use_the_compound_refresh() {
        let root = root();
        for base in [
            root.path.clone(),
            root.path.join("project"),
            root.path.join("Downloads"),
        ] {
            let lock = base.join("Cargo.lock");
            assert_eq!(
                event_scope_with_kind(&root, &lock, EventKind::File, false).unwrap(),
                Some(lock.clone())
            );
            assert_eq!(
                cargo_lock_target(&root, &lock).unwrap(),
                Some(base.join("target"))
            );
            for (kind, recursive) in [
                (EventKind::Unknown, false),
                (EventKind::Directory, false),
                (EventKind::File, true),
                (EventKind::Unknown, true),
            ] {
                assert_eq!(
                    event_scope_with_kind(&root, &lock, kind, recursive).unwrap(),
                    Some(base.clone())
                );
            }
        }
        for suffix in ["node_modules/package/Cargo.lock", "target/debug/Cargo.lock"] {
            let path = root.path.join(suffix);
            assert_eq!(cargo_lock_target(&root, &path).unwrap(), None);
            assert_ne!(
                event_scope_with_kind(&root, &path, EventKind::File, false).unwrap(),
                Some(path)
            );
        }
        for suffix in [
            "Library/Cargo.lock",
            "Dropbox/Cargo.lock",
            "cache.app/Cargo.lock",
        ] {
            let path = root.path.join(suffix);
            assert_eq!(cargo_lock_target(&root, &path).unwrap(), None);
            assert_eq!(
                event_scope_with_kind(&root, &path, EventKind::File, false).unwrap(),
                None
            );
        }
        assert_eq!(cargo_lock_target(&root, &root.path).unwrap(), None);
        assert_eq!(
            cargo_lock_target(&root, &root.path.join("cargo.lock")).unwrap(),
            None
        );
        assert!(cargo_lock_target(&root, Path::new("/outside/Cargo.lock")).is_err());
        let mut home = root.clone();
        home.kind = "home".into();
        assert_eq!(
            cargo_lock_target(&home, &home.path.join("Music/Cargo.lock")).unwrap(),
            None
        );
    }

    #[test]
    fn existing_and_removed_leaf_scopes_never_promote_to_their_parent() {
        let fixture = tempfile::tempdir().unwrap();
        let folder = fixture.path().canonicalize().unwrap();
        let root = safety::authorize(&folder, "folder").unwrap();
        let leaf = folder.join("history");
        std::fs::write(&leaf, b"disposable history").unwrap();
        assert_eq!(resolve_scope(&root, &leaf, None).unwrap(), leaf);
        std::fs::remove_file(&leaf).unwrap();
        assert_eq!(resolve_scope(&root, &leaf, None).unwrap(), leaf);
        let missing = folder.join("removed-directory/removed-file");
        assert_eq!(resolve_scope(&root, &missing, None).unwrap(), missing);
        let artifact = folder.join("project/target");
        assert_eq!(
            resolve_scope(
                &root,
                &artifact.join("debug/output"),
                Some(artifact.clone())
            )
            .unwrap(),
            artifact
        );
        assert!(resolve_scope(&root, Path::new("/outside"), None).is_err());
        assert!(resolve_scope(&root, &leaf, Some(folder.join("other"))).is_err());
        assert!(resolve_scope(&root, &leaf, Some(PathBuf::from("/"))).is_err());
    }

    #[test]
    fn normalized_artifact_scope_is_outermost_and_idempotent() {
        let root = root();
        for (outer, inner) in [("target", "node_modules"), ("node_modules", "target")] {
            let artifact = root.path.join("project").join(outer);
            let nested = artifact.join("nested").join(inner);
            let leaf = nested.join("member/file");
            assert_eq!(normalize_scope(&root, &leaf).unwrap(), artifact);
            assert_eq!(resolve_scope(&root, &leaf, None).unwrap(), artifact);
            assert_eq!(resolve_scope(&root, &leaf, Some(nested)).unwrap(), artifact);
            assert_eq!(normalize_scope(&root, &artifact).unwrap(), artifact);
        }
        for name in ["target-other", "node_modules-old", "ordinary"] {
            let leaf = root.path.join("project").join(name).join("removed/leaf");
            assert_eq!(normalize_scope(&root, &leaf).unwrap(), leaf);
        }
        assert!(normalize_scope(&root, Path::new("/Users/test-neighbor/target/a")).is_err());
        assert!(normalize_scope(&root, &root.path.join("project/../target/a")).is_err());
    }

    #[test]
    fn venv_and_web_cache_events_collapse_to_their_artifact_boundary() {
        let root = root();
        for name in [".venv", "venv", ".next", ".nuxt", ".turbo", ".parcel-cache"] {
            let artifact = root.path.join("project").join(name);
            let leaf = artifact.join("lib/deep/changed");
            assert_eq!(
                event_scope(&root, &leaf).unwrap(),
                Some(artifact.clone()),
                "{name}"
            );
            assert_eq!(normalize_scope(&root, &leaf).unwrap(), artifact);
            assert_eq!(
                event_scope_with_kind(&root, &leaf, EventKind::File, false).unwrap(),
                Some(artifact)
            );
        }
        // Environment configuration writes invalidate their parent scope, the
        // same as the other ownership evidence files.
        let config = root.path.join("project/pyvenv.cfg");
        assert_eq!(
            event_scope_with_kind(&root, &config, EventKind::File, false).unwrap(),
            Some(root.path.join("project"))
        );
        // Similar names never borrow the artifact collapse.
        for name in ["venv-old", ".next-export", "parcel-cache"] {
            let leaf = root.path.join("project").join(name).join("changed");
            assert_eq!(
                event_scope_with_kind(&root, &leaf, EventKind::Unknown, false).unwrap(),
                Some(leaf)
            );
        }
    }

    #[test]
    fn normalized_scope_never_promotes_through_the_grant_boundary() {
        for name in ["target", "node_modules"] {
            let mut root = root();
            root.path = root.path.join("project").join(name);
            assert_eq!(normalize_scope(&root, &root.path).unwrap(), root.path);
            let leaf = root.path.join("package/ordinary");
            assert_eq!(normalize_scope(&root, &leaf).unwrap(), leaf);
            let inner = root.path.join("package/target");
            assert_eq!(
                normalize_scope(&root, &inner.join("debug/output")).unwrap(),
                inner
            );

            root.path.push("subgrant");
            let leaf = root.path.join("ordinary");
            assert_eq!(normalize_scope(&root, &leaf).unwrap(), leaf);
        }
    }

    #[test]
    fn protected_scope_suffixes_survive_indexed_ancestors() {
        let mut root = root();
        let artifact = root.path.join("project/target");
        for suffix in [
            "Library/cache",
            "Dropbox/cache",
            "collection.photoslibrary/item",
            "collection.musiclibrary/item",
            "debug/Generated.app/Contents/file",
        ] {
            let path = artifact.join(suffix);
            assert!(safety::check_scope_policy(&root, &path).is_err());
            assert_eq!(normalize_scope(&root, &path).unwrap(), path);
            assert_eq!(resolve_scope(&root, &path, None).unwrap(), path);
            assert_eq!(
                resolve_scope(&root, &path, Some(artifact.clone())).unwrap(),
                path
            );
            assert_eq!(
                resolve_scope(&root, &path, Some(root.path.clone())).unwrap(),
                path
            );
            // A protected input must not hide a malformed index enclosure.
            assert!(resolve_scope(&root, &path, Some(root.path.join("other"))).is_err());
            assert!(resolve_scope(&root, &path, Some(PathBuf::from("/outside"))).is_err());
        }
        root.kind = "home".into();
        let media = root.path.join("Music/target/cache");
        assert_eq!(
            resolve_scope(&root, &media, Some(root.path.clone())).unwrap(),
            media
        );
        let project_artifact = root.path.join("Projects/Music/target");
        assert_eq!(
            normalize_scope(&root, &project_artifact.join("debug/output")).unwrap(),
            project_artifact
        );
    }

    #[test]
    fn normalization_does_not_apply_filesystem_event_policy() {
        let root = root();
        for suffix in [
            "package.json",
            "Cargo.lock",
            ".cargo/config.toml",
            ".git/index",
            ".git/objects/object",
            "removed-directory/removed-file",
        ] {
            let path = root.path.join("project").join(suffix);
            assert_eq!(normalize_scope(&root, &path).unwrap(), path);
            assert_eq!(resolve_scope(&root, &path, None).unwrap(), path);
        }
    }

    #[test]
    fn missing_event_details_remain_conservative() {
        let event: FsEvent =
            serde_json::from_str(r#"{"path":"/Users/test/unknown-item"}"#).unwrap();
        assert_eq!(event.kind, EventKind::Unknown);
        assert!(!event.recursive);
        assert_eq!(
            event_scope_with_kind(&root(), &event.path, event.kind, event.recursive).unwrap(),
            Some(event.path)
        );
        assert!(
            serde_json::from_str::<FsEvent>(r#"{"path":"/Users/test/item","kind":"unrecognized"}"#)
                .is_err()
        );
    }
}
