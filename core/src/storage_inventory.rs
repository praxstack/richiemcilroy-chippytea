//! Automatic, metadata-only inventory of specific storage locations.
//!
//! These observations are deliberately not candidates: they cannot authorize
//! cleanup, report recovery, or earn chips. No installed commands run and no file
//! contents are read. Defaults are inspected only when they exist; custom tool
//! paths must be reviewed with the owning tool.

use crate::model::{Identity, Root};
use crate::safety::{self, EntryMeta};
use serde::Serialize;
use std::collections::HashSet;
use std::ffi::{CStr, CString, OsStr, OsString};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

const MAX_ISSUES: usize = 64;
const MAX_PATH_BYTES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum InventoryState {
    Complete,
    Partial,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum CleanupAuthority {
    ReviewOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum IssueKind {
    PermissionDenied,
    Symlink,
    Changed,
    DepthLimit,
    EntryLimit,
    TimeLimit,
    CloudPlaceholder,
    MountBoundary,
    Unavailable,
}

#[derive(Debug, Clone, Serialize)]
pub struct InventoryIssue {
    pub path: PathBuf,
    pub kind: IssueKind,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct StorageInventoryRow {
    pub id: String,
    pub title: String,
    pub category: String,
    pub path: PathBuf,
    pub state: InventoryState,
    /// Observed allocation, deduplicated by inode within this location. APFS
    /// clones and shared stores can share blocks: this is not recoverable space.
    /// For a partial row this is only the measured portion, never a full total.
    pub allocated_bytes: Option<u64>,
    pub logical_bytes: Option<u64>,
    pub files: u64,
    pub directories: u64,
    pub detail: String,
    pub owner_followup: String,
    /// An existing fixed owner-review destination, never an executable command.
    pub provider: Option<String>,
    pub cleanup_authority: CleanupAuthority,
}

#[derive(Debug, Clone, Serialize)]
pub struct StorageInventoryReport {
    pub rows: Vec<StorageInventoryRow>,
    pub issues: Vec<InventoryIssue>,
    pub omitted_issues: u64,
    pub examined_entries: u64,
    pub elapsed_ms: u64,
    /// Complete coverage of these fixed routes only, not the whole disk.
    pub complete: bool,
}

#[derive(Clone, Copy)]
struct Limits {
    entries_per_route: u64,
    total_entries: u64,
    depth: usize,
    route_time: Duration,
    total_time: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            entries_per_route: 24_000,
            total_entries: 160_000,
            depth: 32,
            route_time: Duration::from_millis(250),
            total_time: Duration::from_secs(4),
        }
    }
}

#[derive(Clone, Copy)]
struct Route {
    id: &'static str,
    title: &'static str,
    category: &'static str,
    relative: &'static str,
    detail: &'static str,
    followup: &'static str,
    provider: Option<&'static str>,
    /// Some locations are a single VM image. Never enumerate their data parent.
    file_only: bool,
    skip_children: &'static [&'static str],
    /// A shallow metadata inventory, used for launch-agent plist filenames.
    child_suffix: Option<&'static str>,
}

const fn route(
    id: &'static str,
    title: &'static str,
    relative: &'static str,
    followup: &'static str,
) -> Route {
    Route {
        id,
        title,
        category: "Developer tools",
        relative,
        detail: "Storage at this standard location may still be in use. A custom tool location is not included.",
        followup,
        provider: None,
        file_only: false,
        skip_children: &[],
        child_suffix: None,
    }
}

const XCODE_REVIEW: &str = "Review in Xcode Settings and Devices and Simulators. Keep runtimes and build data needed by current projects.";
const SYSTEM_REVIEW: &str = "macOS and the owning app manage these files. Review with the owner; this overview does not remove system files.";
const VM_DETAIL: &str = "This disk image includes active containers, images, volumes, or machines. Its allocated size is not unused space; its logical size may be much larger.";

// Review-only defaults outside the scanner's cleanup candidates. Do not add
// cache or log routes already measured by discovery, or replace these with
// recursive scans of .config, .local/share, Application Support, Containers,
// or media folders.
const USER_ROUTES: &[Route] = &[
    route(
        "rustup-downloads",
        "Rust toolchain downloads",
        ".rustup/downloads",
        "Review downloads with rustup before removing anything an installation may still need.",
    ),
    Route {
        detail: "These are installed Rust toolchains, including versions that projects may still require. No toolchain is classified as unused.",
        ..route(
            "rust-toolchains",
            "Rust toolchains",
            ".rustup/toolchains",
            "Use rustup toolchain list and review project overrides before uninstalling a specific toolchain.",
        )
    },
    Route {
        category: "Containers",
        detail: VM_DETAIL,
        provider: Some("docker"),
        file_only: true,
        ..route(
            "docker-disk",
            "Docker Desktop disk image",
            "Library/Containers/com.docker.docker/Data/vms/0/data/Docker.raw",
            "Review Docker's disk usage to identify unused data. Never remove the VM disk image to clear a cache.",
        )
    },
    Route {
        category: "Containers",
        detail: VM_DETAIL,
        provider: Some("docker"),
        file_only: true,
        ..route(
            "docker-disk-legacy",
            "Docker Desktop disk image",
            "Library/Containers/com.docker.docker/Data/vms/0/data/Docker.qcow2",
            "Review Docker's disk usage to identify unused data. Never remove the VM disk image to clear a cache.",
        )
    },
    Route {
        category: "Containers",
        detail: VM_DETAIL,
        file_only: true,
        ..route(
            "orbstack-disk",
            "OrbStack disk image",
            "Library/Group Containers/HUAQ24HBR6.dev.orbstack/data/data.img",
            "Use OrbStack to review containers and machines. Its virtual disk may contain important volumes and databases.",
        )
    },
    Route {
        category: "Containers",
        detail: VM_DETAIL,
        file_only: true,
        ..route(
            "orbstack-disk-raw",
            "OrbStack disk image",
            "Library/Group Containers/HUAQ24HBR6.dev.orbstack/data/data.img.raw",
            "Use OrbStack to review containers and machines. Its virtual disk may contain important volumes and databases.",
        )
    },
    Route {
        category: "Containers",
        detail: VM_DETAIL,
        file_only: true,
        ..route(
            "orbstack-disk-legacy",
            "OrbStack disk image",
            ".orbstack/data/data.img",
            "Use OrbStack to review containers and machines. Its virtual disk may contain important volumes and databases.",
        )
    },
    Route {
        detail: "Versioned Claude Code installations may include the current and previous versions. This inventory does not identify which versions are unused.",
        ..route(
            "claude-versions",
            "Claude Code versions",
            ".local/share/claude/versions",
            "Review the active Claude Code version and its installation before changing versioned binaries.",
        )
    },
    Route {
        category: "Apps",
        child_suffix: Some(".plist"),
        detail: "These are user login-agent configuration files. This metadata-only overview does not inspect their configuration or establish that an app is missing.",
        ..route(
            "login-agents",
            "Login items to review",
            "Library/LaunchAgents",
            "Review Login Items in System Settings and uninstall unwanted helpers through their owning apps. These entries are not classified as stale.",
        )
    },
    route(
        "simulator-cache",
        "Simulator cache",
        "Library/Developer/CoreSimulator/Caches",
        XCODE_REVIEW,
    ),
    Route {
        category: "Apps",
        detail: "This exact CacheClip location may contain generated DaVinci Resolve render cache. Original media folders are not scanned by this overview.",
        ..route(
            "resolve-cache",
            "DaVinci Resolve CacheClip",
            "Movies/CacheClip",
            "Use DaVinci Resolve's Delete Render Cache controls after reviewing the projects that use this cache.",
        )
    },
];

const SYSTEM_ROUTES: &[Route] = &[
    Route {
        category: "System",
        ..route(
            "system-caches",
            "System caches",
            "Library/Caches",
            SYSTEM_REVIEW,
        )
    },
    Route {
        category: "System",
        skip_children: &["DiagnosticReports"],
        ..route(
            "system-logs",
            "System app logs",
            "Library/Logs",
            SYSTEM_REVIEW,
        )
    },
    Route {
        category: "System",
        ..route(
            "system-diagnostics",
            "System diagnostic reports",
            "Library/Logs/DiagnosticReports",
            SYSTEM_REVIEW,
        )
    },
    Route {
        category: "System",
        ..route(
            "system-service-logs",
            "System service logs",
            "private/var/log",
            SYSTEM_REVIEW,
        )
    },
    route(
        "simulator-system-cache",
        "Simulator system cache",
        "Library/Developer/CoreSimulator/Caches",
        XCODE_REVIEW,
    ),
    route(
        "simulator-images",
        "Simulator runtime images",
        "Library/Developer/CoreSimulator/Images",
        XCODE_REVIEW,
    ),
    Route {
        detail: "Mounted runtime volumes are excluded from traversal. Use Xcode to review installed runtimes and their storage.",
        ..route(
            "simulator-volumes",
            "Simulator runtime volumes",
            "Library/Developer/CoreSimulator/Volumes",
            XCODE_REVIEW,
        )
    },
    Route {
        provider: Some("homebrew"),
        ..route(
            "homebrew-locks-arm",
            "Homebrew lock files",
            "opt/homebrew/var/homebrew/locks",
            "Lock files may coordinate a running Homebrew operation. Review with Homebrew; age or a zero-byte size does not establish that a lock is stale.",
        )
    },
    Route {
        provider: Some("homebrew"),
        ..route(
            "homebrew-locks-intel",
            "Homebrew lock files",
            "usr/local/var/homebrew/locks",
            "Lock files may coordinate a running Homebrew operation. Review with Homebrew; age or a zero-byte size does not establish that a lock is stale.",
        )
    },
];

/// The caller must validate an existing Home grant before this call and again
/// before publishing its result. The opened descriptor must match that grant
/// before any storage route is inspected. `include_system` is permitted only
/// for the user's real Home grant. Fixtures pass false and touch no system path.
pub fn scan(root: &Root, include_system: bool) -> StorageInventoryReport {
    if root.kind != "home" {
        let mut scan = Scan::new(Limits::default());
        scan.issue(
            &root.path,
            Failure::new(
                IssueKind::PermissionDenied,
                "A home-folder grant is required for the storage overview.",
            ),
        );
        return scan.finish();
    }
    scan_bound(
        &root.path,
        &root.identity,
        include_system,
        Limits::default(),
    )
}

fn scan_bound(
    home: &Path,
    expected_home: &Identity,
    include_system: bool,
    limits: Limits,
) -> StorageInventoryReport {
    let mut scan = Scan::new(limits);
    let _local_io = match safety::LocalOnlyIo::new() {
        Ok(guard) => guard,
        Err(detail) => {
            scan.issue(home, Failure::new(IssueKind::Unavailable, detail));
            return scan.finish();
        }
    };
    let home_fd = match open_absolute_directory(home) {
        Ok(fd) => fd,
        Err(failure) => {
            scan.issue(home, failure);
            return scan.finish();
        }
    };
    let home_meta = match stat_fd(home_fd.as_raw_fd()) {
        Ok(meta) if meta.uid == unsafe { libc::geteuid() } => meta,
        Ok(_) => {
            scan.issue(
                home,
                Failure::new(
                    IssueKind::PermissionDenied,
                    "The home folder is owned by another user.",
                ),
            );
            return scan.finish();
        }
        Err(failure) => {
            scan.issue(home, failure);
            return scan.finish();
        }
    };
    if home_meta.identity.device != expected_home.device
        || home_meta.identity.inode != expected_home.inode
        || home_meta.identity.mode != expected_home.mode
    {
        scan.issue(
            home,
            Failure::new(
                IssueKind::Changed,
                "The authorized home folder was replaced. Choose the folder again before scanning.",
            ),
        );
        return scan.finish();
    }
    for route in USER_ROUTES {
        scan.measure(
            home_fd.as_raw_fd(),
            home,
            route,
            Some(home_meta.identity.device),
            false,
        );
    }
    // Production system routes exist only on macOS. Linux tests must never
    // accidentally inspect the runner's /Library or /private/var directories.
    #[cfg(target_os = "macos")]
    if include_system {
        match open_absolute_directory(Path::new("/")) {
            Ok(root) => {
                for route in SYSTEM_ROUTES {
                    scan.measure(root.as_raw_fd(), Path::new("/"), route, None, true);
                }
            }
            Err(failure) => scan.issue(Path::new("/"), failure),
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = include_system;
    // A detached descriptor does not establish that the authorized display
    // path still names the same Home. Remove observations on replacement.
    if open_absolute_directory(home)
        .and_then(|fd| stat_fd(fd.as_raw_fd()))
        .map_or(true, |current| !same_object(&home_meta, &current))
    {
        scan.report.rows.clear();
        scan.issue(
            home,
            Failure::new(
                IssueKind::Changed,
                "The home folder moved or changed during the overview. Scan it again.",
            ),
        );
    }
    scan.finish()
}

struct Scan {
    report: StorageInventoryReport,
    start: Instant,
    limits: Limits,
}

impl Scan {
    fn new(limits: Limits) -> Self {
        Self {
            report: StorageInventoryReport {
                rows: Vec::new(),
                issues: Vec::new(),
                omitted_issues: 0,
                examined_entries: 0,
                elapsed_ms: 0,
                complete: true,
            },
            start: Instant::now(),
            limits,
        }
    }

    fn finish(mut self) -> StorageInventoryReport {
        self.report.elapsed_ms = self.start.elapsed().as_millis().min(u64::MAX as u128) as u64;
        self.report
    }

    fn issue(&mut self, path: &Path, failure: Failure) {
        self.report.complete = false;
        if self.report.issues.len() < MAX_ISSUES {
            self.report.issues.push(InventoryIssue {
                path: path.to_path_buf(),
                kind: failure.kind,
                detail: failure.detail,
            });
        } else {
            self.report.omitted_issues += 1;
        }
    }

    fn checkpoint(&self, started: Instant, entries: u64) -> Result<(), Failure> {
        // Deadlines bound cooperative filesystem work; a kernel call itself is
        // not preempted. The paths are local and cloud materialization is off.
        if self.start.elapsed() >= self.limits.total_time
            || started.elapsed() >= self.limits.route_time
        {
            return Err(Failure::new(
                IssueKind::TimeLimit,
                "The overview reached its time limit. Only the measured portion is shown.",
            ));
        }
        if self.report.examined_entries >= self.limits.total_entries
            || entries >= self.limits.entries_per_route
        {
            return Err(Failure::new(
                IssueKind::EntryLimit,
                "The overview reached its entry limit. Only the measured portion is shown.",
            ));
        }
        Ok(())
    }

    fn measure(
        &mut self,
        base: RawFd,
        base_path: &Path,
        route: &Route,
        device: Option<u64>,
        system: bool,
    ) {
        let path = base_path.join(route.relative);
        let started = Instant::now();
        if let Err(failure) = self.checkpoint(started, 0) {
            self.issue(&path, failure);
            return;
        }
        let location = match locate(base, Path::new(route.relative), device) {
            Ok(Some(location)) => location,
            Ok(None) => return,
            Err(failure) => {
                self.issue(&path, failure);
                return;
            }
        };
        self.report.examined_entries += 1;
        let mut row = StorageInventoryRow {
            id: route.id.into(),
            title: route.title.into(),
            category: route.category.into(),
            path: path.clone(),
            state: InventoryState::Complete,
            allocated_bytes: Some(0),
            logical_bytes: Some(0),
            files: 0,
            directories: 0,
            detail: route.detail.into(),
            owner_followup: route.followup.into(),
            provider: route.provider.map(str::to_owned),
            cleanup_authority: CleanupAuthority::ReviewOnly,
        };
        let valid_owner =
            |meta: &EntryMeta| meta.uid == unsafe { libc::geteuid() } || (system && meta.uid == 0);
        if !valid_owner(&location.meta) {
            self.issue(
                &path,
                Failure::new(
                    IssueKind::PermissionDenied,
                    "This storage belongs to another user.",
                ),
            );
            row.state = InventoryState::Unavailable;
        } else if route.file_only {
            if location.meta.is_file() {
                row.files = 1;
                row.allocated_bytes = Some(location.meta.allocated);
                row.logical_bytes = Some(location.meta.identity.size);
                if stat_at(location.parent.as_raw_fd(), &location.name)
                    .map_or(true, |meta| meta != location.meta)
                {
                    self.issue(
                        &path,
                        Failure::new(
                            IssueKind::Changed,
                            "The file changed while its size was being read.",
                        ),
                    );
                    row.state = InventoryState::Partial;
                }
            } else {
                self.issue(
                    &path,
                    Failure::new(
                        IssueKind::Unavailable,
                        "The expected disk-image file has a different type.",
                    ),
                );
                row.state = InventoryState::Unavailable;
            }
        } else if location.meta.is_dir() {
            match Directory::open(
                location.parent.as_raw_fd(),
                &location.name,
                &location.meta,
                path.clone(),
            ) {
                Ok(root) => self.walk(root, &location, route, started, &valid_owner, &mut row),
                Err(failure) => {
                    self.issue(&path, failure);
                    row.state = InventoryState::Unavailable;
                }
            }
        } else {
            self.issue(
                &path,
                Failure::new(
                    IssueKind::Unavailable,
                    "The expected storage folder has a different type.",
                ),
            );
            row.state = InventoryState::Unavailable;
        }
        // The terminal parent descriptor can remain valid after an ancestor is
        // renamed. Re-resolve the route from its original anchor before using
        // that pathname as the label for this measurement.
        if let Err(failure) =
            verify_location(base, Path::new(route.relative), device, &location.meta)
        {
            self.issue(&path, failure);
            row.state = InventoryState::Partial;
            row.allocated_bytes = None;
            row.logical_bytes = None;
        }
        if row.state == InventoryState::Unavailable
            || (row.state == InventoryState::Partial && row.files == 0)
        {
            row.allocated_bytes = None;
            row.logical_bytes = None;
        }
        // An empty, successfully inspected directory is not a suggestion. A
        // zero-byte lock file is an actual observation and remains review-only.
        if row.state != InventoryState::Complete || row.files > 0 {
            self.report.rows.push(row);
        }
    }

    fn walk(
        &mut self,
        root: Directory,
        location: &Location,
        route: &Route,
        started: Instant,
        valid_owner: &impl Fn(&EntryMeta) -> bool,
        row: &mut StorageInventoryRow,
    ) {
        let device = location.meta.identity.device;
        let mut stack = vec![root];
        let mut inodes = HashSet::new();
        let mut entries = 1;
        while !stack.is_empty() {
            if let Err(failure) = self.checkpoint(started, entries) {
                self.issue(&row.path, failure);
                row.state = InventoryState::Partial;
                break;
            }
            let current = stack.last_mut().unwrap();
            let name = match current.next_name() {
                Ok(Some(name)) => name,
                Ok(None) => {
                    let finished = stack.pop().unwrap();
                    let parent = stack
                        .last()
                        .map_or(location.parent.as_raw_fd(), Directory::fd);
                    let current_name = if stack.is_empty() {
                        &location.name
                    } else {
                        &finished.name
                    };
                    if stat_fd(finished.fd()).map_or(true, |meta| meta != finished.initial)
                        || stat_at(parent, current_name)
                            .map_or(true, |meta| meta != finished.initial)
                    {
                        self.issue(&finished.path, Failure::new(IssueKind::Changed, "This folder changed during the overview. Scan again for a fresh measurement."));
                        row.state = InventoryState::Partial;
                    }
                    continue;
                }
                Err(failure) => {
                    let path = current.path.clone();
                    self.issue(&path, failure);
                    row.state = InventoryState::Partial;
                    stack.pop();
                    continue;
                }
            };
            entries += 1;
            self.report.examined_entries += 1;
            if stack.len() == 1
                && route
                    .skip_children
                    .iter()
                    .any(|skip| name == OsStr::new(skip))
            {
                continue;
            }
            if route
                .child_suffix
                .is_some_and(|suffix| !name.as_bytes().ends_with(suffix.as_bytes()))
            {
                continue;
            }
            let current = stack.last().unwrap();
            let path = current.path.join(&name);
            if path.as_os_str().len() > MAX_PATH_BYTES {
                self.issue(
                    &current.path,
                    Failure::new(
                        IssueKind::DepthLimit,
                        "A nested path exceeded the overview's path limit.",
                    ),
                );
                row.state = InventoryState::Partial;
                continue;
            }
            let name_c =
                CString::new(name.as_bytes()).expect("A directory name cannot contain NUL");
            let metadata = match stat_at(current.fd(), &name_c)
                .and_then(|meta| boundary(meta, Some(device)))
            {
                Ok(metadata) => metadata,
                Err(failure) => {
                    self.issue(&path, failure);
                    row.state = InventoryState::Partial;
                    continue;
                }
            };
            if !valid_owner(&metadata) {
                self.issue(
                    &path,
                    Failure::new(
                        IssueKind::PermissionDenied,
                        "An entry is owned by another user and was excluded.",
                    ),
                );
                row.state = InventoryState::Partial;
                continue;
            }
            if metadata.is_file() {
                // The set is bounded by entries_per_route. Count aliases as
                // entries but never double-count a hardlinked inode's bytes.
                row.files += 1;
                if inodes.insert((metadata.identity.device, metadata.identity.inode)) {
                    let allocated = row.allocated_bytes.unwrap().checked_add(metadata.allocated);
                    let logical = row
                        .logical_bytes
                        .unwrap()
                        .checked_add(metadata.identity.size);
                    match (allocated, logical) {
                        (Some(allocated), Some(logical)) => {
                            row.allocated_bytes = Some(allocated);
                            row.logical_bytes = Some(logical);
                        }
                        _ => {
                            self.issue(
                                &path,
                                Failure::new(
                                    IssueKind::Unavailable,
                                    "The storage measurement exceeded the supported size.",
                                ),
                            );
                            row.state = InventoryState::Unavailable;
                            break;
                        }
                    }
                }
            } else if metadata.is_dir() {
                if route.child_suffix.is_some() {
                    continue;
                }
                row.directories += 1;
                if stack.len() >= self.limits.depth || safety::is_cloud_component(&name) {
                    let kind = if safety::is_cloud_component(&name) {
                        IssueKind::CloudPlaceholder
                    } else {
                        IssueKind::DepthLimit
                    };
                    self.issue(
                        &path,
                        Failure::new(
                            kind,
                            "This nested folder was excluded from the bounded local overview.",
                        ),
                    );
                    row.state = InventoryState::Partial;
                    continue;
                }
                match Directory::open(current.fd(), &name_c, &metadata, path.clone()) {
                    Ok(directory) => stack.push(directory),
                    Err(failure) => {
                        self.issue(&path, failure);
                        row.state = InventoryState::Partial;
                    }
                }
            } else {
                self.issue(
                    &path,
                    Failure::new(
                        IssueKind::Unavailable,
                        "A special filesystem entry was excluded.",
                    ),
                );
                row.state = InventoryState::Partial;
            }
        }
    }
}

#[derive(Debug)]
struct Failure {
    kind: IssueKind,
    detail: String,
}

impl Failure {
    fn new(kind: IssueKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }
    fn io(error: io::Error) -> Self {
        let kind = match error.raw_os_error() {
            Some(libc::EACCES | libc::EPERM) => IssueKind::PermissionDenied,
            Some(libc::ELOOP) => IssueKind::Symlink,
            Some(libc::ENOENT | libc::ENOTDIR) => IssueKind::Changed,
            _ => IssueKind::Unavailable,
        };
        let detail = match kind {
            IssueKind::PermissionDenied => {
                "macOS did not allow this location to be read. Check folder access and Full Disk Access."
            }
            IssueKind::Symlink => "Symbolic links are not followed by the overview.",
            IssueKind::Changed => "This location moved or changed during the overview.",
            _ => "This location could not be measured.",
        };
        Self::new(kind, detail)
    }
}

fn same_object(left: &EntryMeta, right: &EntryMeta) -> bool {
    left.identity.device == right.identity.device
        && left.identity.inode == right.identity.inode
        && left.identity.mode == right.identity.mode
        && left.uid == right.uid
        && left.flags == right.flags
}

fn stat_fd(fd: RawFd) -> Result<EntryMeta, Failure> {
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd, &mut stat) } != 0 {
        return Err(Failure::io(io::Error::last_os_error()));
    }
    Ok(EntryMeta::from_stat(&stat))
}

fn stat_at(fd: RawFd, name: &CStr) -> Result<EntryMeta, Failure> {
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstatat(fd, name.as_ptr(), &mut stat, libc::AT_SYMLINK_NOFOLLOW) } != 0 {
        return Err(Failure::io(io::Error::last_os_error()));
    }
    Ok(EntryMeta::from_stat(&stat))
}

fn boundary(meta: EntryMeta, device: Option<u64>) -> Result<EntryMeta, Failure> {
    if meta.is_symlink() {
        return Err(Failure::new(
            IssueKind::Symlink,
            "Symbolic links are not followed by the overview.",
        ));
    }
    if meta.is_dataless() {
        return Err(Failure::new(
            IssueKind::CloudPlaceholder,
            "Cloud placeholders are excluded without downloading them.",
        ));
    }
    if device.is_some_and(|device| meta.identity.device != device) {
        return Err(Failure::new(
            IssueKind::MountBoundary,
            "A different storage volume was excluded. Review it with its owning app.",
        ));
    }
    Ok(meta)
}

fn open_at(fd: RawFd, name: &CStr, search: bool) -> Result<OwnedFd, Failure> {
    #[cfg(target_os = "macos")]
    let access = if search {
        libc::O_SEARCH
    } else {
        libc::O_RDONLY | libc::O_DIRECTORY
    };
    #[cfg(not(target_os = "macos"))]
    let access = {
        let _ = search;
        libc::O_RDONLY | libc::O_DIRECTORY
    };
    let raw = unsafe {
        libc::openat(
            fd,
            name.as_ptr(),
            access | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if raw < 0 {
        return Err(Failure::io(io::Error::last_os_error()));
    }
    Ok(unsafe { OwnedFd::from_raw_fd(raw) })
}

fn validate_directory(fd: RawFd, expected: &EntryMeta) -> Result<(), Failure> {
    let actual = boundary(stat_fd(fd)?, Some(expected.identity.device))?;
    if !actual.is_dir() || actual != *expected {
        return Err(Failure::new(
            IssueKind::Changed,
            "A folder changed before it could be measured.",
        ));
    }
    safety::cloud_directory_check(fd)
        .map_err(|detail| Failure::new(IssueKind::CloudPlaceholder, detail))?;
    #[cfg(target_os = "macos")]
    {
        let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstatfs(fd, &mut stat) } != 0 {
            return Err(Failure::io(io::Error::last_os_error()));
        }
        if stat.f_flags & libc::MNT_LOCAL as u32 == 0 {
            return Err(Failure::new(
                IssueKind::MountBoundary,
                "Network and remote storage are excluded.",
            ));
        }
    }
    Ok(())
}

struct Location {
    parent: OwnedFd,
    name: CString,
    meta: EntryMeta,
}

fn verify_location(
    base: RawFd,
    relative: &Path,
    device: Option<u64>,
    expected: &EntryMeta,
) -> Result<(), Failure> {
    if locate(base, relative, device).is_ok_and(|location| {
        location.is_some_and(|location| same_object(&location.meta, expected))
    }) {
        Ok(())
    } else {
        Err(Failure::new(
            IssueKind::Changed,
            "This storage location moved or changed. Its previous measurement is no longer associated with this path.",
        ))
    }
}

fn locate(base: RawFd, relative: &Path, device: Option<u64>) -> Result<Option<Location>, Failure> {
    if relative.as_os_str().len() > MAX_PATH_BYTES {
        return Err(Failure::new(
            IssueKind::DepthLimit,
            "This path exceeds the overview's path limit.",
        ));
    }
    let names = relative
        .components()
        .map(|component| match component {
            Component::Normal(name) => CString::new(name.as_bytes()).map_err(|_| {
                Failure::new(IssueKind::Unavailable, "The path contains an invalid name.")
            }),
            _ => Err(Failure::new(
                IssueKind::Unavailable,
                "The overview requires an exact local path.",
            )),
        })
        .collect::<Result<Vec<_>, _>>()?;
    if names.is_empty() {
        return Err(Failure::new(
            IssueKind::Unavailable,
            "An empty storage route is not allowed.",
        ));
    }
    let mut parent = open_at(base, c".", true)?;
    for (index, name) in names.iter().enumerate() {
        // ENOENT is absence only on the initial, no-follow metadata lookup.
        // Any failure after a presence observation is a changed/partial result.
        let mut raw: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe {
            libc::fstatat(
                parent.as_raw_fd(),
                name.as_ptr(),
                &mut raw,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ENOENT) {
                return Ok(None);
            }
            return Err(Failure::io(error));
        }
        let meta = boundary(EntryMeta::from_stat(&raw), device)?;
        if index + 1 == names.len() {
            return Ok(Some(Location {
                parent,
                name: name.clone(),
                meta,
            }));
        }
        if !meta.is_dir() {
            return Err(Failure::new(
                IssueKind::Changed,
                "A storage path component is no longer a folder.",
            ));
        }
        let next = open_at(parent.as_raw_fd(), name, true)?;
        validate_directory(next.as_raw_fd(), &meta)?;
        parent = next;
    }
    unreachable!("Nonempty route returns its final location")
}

fn open_absolute_directory(path: &Path) -> Result<OwnedFd, Failure> {
    if !path.is_absolute() || path.as_os_str().len() > MAX_PATH_BYTES {
        return Err(Failure::new(
            IssueKind::Unavailable,
            "An absolute local home folder is required.",
        ));
    }
    let root = open_at(libc::AT_FDCWD, c"/", true)?;
    if path == Path::new("/") {
        return Ok(root);
    }
    let relative = path
        .strip_prefix("/")
        .map_err(|_| Failure::new(IssueKind::Unavailable, "Invalid absolute folder path."))?;
    let location = locate(root.as_raw_fd(), relative, None)?.ok_or_else(|| {
        Failure::new(
            IssueKind::Unavailable,
            "The home folder is no longer available.",
        )
    })?;
    let result = open_at(location.parent.as_raw_fd(), &location.name, true)?;
    validate_directory(result.as_raw_fd(), &location.meta)?;
    Ok(result)
}

struct Directory {
    reader: *mut libc::DIR,
    name: CString,
    path: PathBuf,
    initial: EntryMeta,
}

impl Directory {
    fn open(
        parent: RawFd,
        name: &CStr,
        expected: &EntryMeta,
        path: PathBuf,
    ) -> Result<Self, Failure> {
        let fd = open_at(parent, name, false)?;
        validate_directory(fd.as_raw_fd(), expected)?;
        let reader = unsafe { libc::fdopendir(fd.as_raw_fd()) };
        if reader.is_null() {
            return Err(Failure::io(io::Error::last_os_error()));
        }
        let _ = fd.into_raw_fd(); // Ownership transfers only after fdopendir succeeds.
        Ok(Self {
            reader,
            name: name.into(),
            path,
            initial: expected.clone(),
        })
    }
    fn fd(&self) -> RawFd {
        unsafe { libc::dirfd(self.reader) }
    }
    fn next_name(&mut self) -> Result<Option<OsString>, Failure> {
        loop {
            #[cfg(target_os = "macos")]
            unsafe {
                *libc::__error() = 0;
            }
            #[cfg(not(target_os = "macos"))]
            unsafe {
                *libc::__errno_location() = 0;
            }
            let entry = unsafe { libc::readdir(self.reader) };
            if entry.is_null() {
                let error = io::Error::last_os_error();
                return if error.raw_os_error().unwrap_or(0) == 0 {
                    Ok(None)
                } else {
                    Err(Failure::io(error))
                };
            }
            let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
            if name != b"." && name != b".." {
                return Ok(Some(OsString::from_vec(name.to_vec())));
            }
        }
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        unsafe {
            libc::closedir(self.reader);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
    use tempfile::TempDir;

    fn fixture() -> TempDir {
        tempfile::tempdir().unwrap()
    }
    fn home(fixture: &TempDir) -> PathBuf {
        fixture.path().canonicalize().unwrap()
    }
    fn put(home: &Path, path: &str, bytes: usize) -> PathBuf {
        let path = home.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, vec![b'x'; bytes]).unwrap();
        path
    }
    fn relaxed() -> Limits {
        Limits {
            route_time: Duration::from_secs(5),
            total_time: Duration::from_secs(10),
            ..Limits::default()
        }
    }

    fn scan_with_limits(
        home: &Path,
        include_system: bool,
        limits: Limits,
    ) -> StorageInventoryReport {
        let expected = safety::metadata(home).unwrap().identity;
        scan_bound(home, &expected, include_system, limits)
    }

    #[test]
    fn replaced_home_cannot_be_scanned_under_an_earlier_grant() {
        let fixture = fixture();
        let parent = home(&fixture);
        let home = parent.join("authorized-home");
        fs::create_dir(&home).unwrap();
        let root = Root {
            id: "fixture-home".into(),
            path: home.clone(),
            kind: "home".into(),
            identity: safety::metadata(&home).unwrap().identity,
        };
        fs::rename(&home, parent.join("original-home")).unwrap();
        put(&home, ".rustup/toolchains/replacement-data", 1_000_000);
        let report = scan(&root, false);
        assert!(report.rows.is_empty());
        assert_eq!(report.examined_entries, 0);
        assert_eq!(report.issues.len(), 1);
        assert_eq!(report.issues[0].kind, IssueKind::Changed);
    }

    #[test]
    fn a_non_home_grant_cannot_authorize_inventory_routes() {
        let fixture = fixture();
        let home = home(&fixture);
        put(&home, ".rustup/toolchains/data", 1_000_000);
        let root = Root {
            id: "fixture-folder".into(),
            path: home.clone(),
            kind: "folder".into(),
            identity: safety::metadata(&home).unwrap().identity,
        };
        let report = scan(&root, false);
        assert!(report.rows.is_empty());
        assert_eq!(report.examined_entries, 0);
        assert_eq!(report.issues[0].kind, IssueKind::PermissionDenied);
    }

    #[test]
    fn cloud_and_volume_boundaries_are_rejected_before_directory_enumeration() {
        let fixture = fixture();
        let home = home(&fixture);
        let metadata = safety::metadata(&home).unwrap();
        let mut placeholder = metadata.clone();
        placeholder.flags |= 0x4000_0000;
        assert_eq!(
            boundary(placeholder, None).unwrap_err().kind,
            IssueKind::CloudPlaceholder
        );
        assert_eq!(
            boundary(
                metadata.clone(),
                Some(metadata.identity.device.wrapping_add(1))
            )
            .unwrap_err()
            .kind,
            IssueKind::MountBoundary
        );
    }

    #[test]
    fn exact_defaults_measure_existing_files_without_inventorying_siblings() {
        let fixture = fixture();
        let home = home(&fixture);
        let toolchains = put(&home, ".rustup/toolchains/content/file", 7777);
        put(&home, ".aws/credentials", 1_000_000);
        put(&home, ".local/share/opencode/sessions/private", 1_000_000);
        put(&home, "Movies/Originals/movie.mp4", 1_000_000);
        put(
            &home,
            "Library/org.swift.swiftpm/security/private",
            1_000_000,
        );
        let report = scan_with_limits(&home, false, relaxed());
        assert!(report.complete, "{:?}", report.issues);
        assert_eq!(report.rows.len(), 1);
        let row = &report.rows[0];
        assert_eq!(row.id, "rust-toolchains");
        assert_eq!(row.logical_bytes, Some(7777));
        assert_eq!(
            row.allocated_bytes,
            Some(fs::metadata(toolchains).unwrap().blocks() * 512)
        );
        assert_eq!(row.cleanup_authority, CleanupAuthority::ReviewOnly);
    }

    #[test]
    fn scanner_owned_cache_and_log_routes_are_not_inventoried() {
        let fixture = fixture();
        let home = home(&fixture);
        for route in crate::recommendations::DEVELOPER_CACHE_ROUTES {
            put(&home, &format!("{}/preserved", route.path), 1024);
        }
        for path in [
            "Library/Caches/com.example.app",
            "Library/Caches/com.google.GoogleUpdater",
            "Library/Application Support/Google/GoogleUpdater/crx_cache",
            "Library/Application Support/Slack/Cache",
            "Library/Logs/com.example.app",
            "Library/Developer/Xcode/DerivedData/example",
            "Library/Caches/com.apple.dt.Xcode",
            ".npm/_logs",
            ".config/gcloud/logs",
            ".azure/logs",
            ".oh-my-zsh/cache",
            ".cache/opencode",
            ".cache/ghostty",
        ] {
            put(&home, &format!("{path}/preserved.log"), 1024);
        }
        let report = scan_with_limits(&home, false, relaxed());
        assert!(report.complete, "{:?}", report.issues);
        assert!(report.rows.is_empty());
        assert!(report.issues.is_empty());
        assert_eq!(report.examined_entries, 0);
    }

    #[test]
    fn missing_and_empty_locations_do_not_create_suggestions_or_errors() {
        let fixture = fixture();
        let home = home(&fixture);
        fs::create_dir_all(home.join(".rustup/toolchains")).unwrap();
        let report = scan_with_limits(&home, false, relaxed());
        assert!(report.complete);
        assert!(report.rows.is_empty());
        assert!(report.issues.is_empty());
    }

    #[test]
    fn symlink_roots_and_ancestors_cannot_escape_the_home() {
        let fixture = fixture();
        let home = home(&fixture);
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().canonicalize().unwrap();
        put(&target, "secret/file", 32_000);
        fs::create_dir_all(home.join(".rustup")).unwrap();
        symlink(target.join("secret"), home.join(".rustup/toolchains")).unwrap();
        symlink(&target, home.join(".local")).unwrap();
        let report = scan_with_limits(&home, false, relaxed());
        assert!(report.rows.is_empty());
        assert!(!report.complete);
        assert!(
            report
                .issues
                .iter()
                .all(|issue| issue.kind == IssueKind::Symlink)
        );
    }

    #[test]
    fn nested_symlink_is_partial_and_never_contributes_target_bytes() {
        let fixture = fixture();
        let home = home(&fixture);
        put(&home, ".rustup/toolchains/local", 17);
        let target = put(&home, "unrelated/secret", 8_000_000);
        symlink(target, home.join(".rustup/toolchains/link")).unwrap();
        let report = scan_with_limits(&home, false, relaxed());
        let row = &report.rows[0];
        assert_eq!(row.state, InventoryState::Partial);
        assert_eq!(row.logical_bytes, Some(17));
        assert_eq!(row.files, 1);
        assert_eq!(report.issues[0].kind, IssueKind::Symlink);
    }

    #[test]
    fn hardlinks_are_counted_once_and_sparse_file_allocation_is_not_logical_size() {
        let fixture = fixture();
        let home = home(&fixture);
        let first = put(&home, ".local/share/claude/versions/a", 8192);
        fs::hard_link(&first, home.join(".local/share/claude/versions/b")).unwrap();
        let image = put(
            &home,
            "Library/Containers/com.docker.docker/Data/vms/0/data/Docker.raw",
            0,
        );
        fs::OpenOptions::new()
            .write(true)
            .open(&image)
            .unwrap()
            .set_len(8_000_000_000)
            .unwrap();
        let report = scan_with_limits(&home, false, relaxed());
        let versions = report
            .rows
            .iter()
            .find(|row| row.id == "claude-versions")
            .unwrap();
        assert_eq!(versions.logical_bytes, Some(8192));
        assert_eq!(versions.files, 2);
        let docker = report
            .rows
            .iter()
            .find(|row| row.id == "docker-disk")
            .unwrap();
        assert_eq!(docker.logical_bytes, Some(8_000_000_000));
        assert_eq!(
            docker.allocated_bytes,
            Some(fs::metadata(image).unwrap().blocks() * 512)
        );
        assert!(docker.allocated_bytes.unwrap() < docker.logical_bytes.unwrap());
        assert_eq!(docker.cleanup_authority, CleanupAuthority::ReviewOnly);
    }

    #[test]
    fn entry_and_depth_limits_publish_only_partial_measurements() {
        let fixture = fixture();
        let home = home(&fixture);
        for n in 0..20 {
            put(&home, &format!(".rustup/toolchains/{n}"), 100);
        }
        put(&home, ".local/share/claude/versions/a/b/c/d", 10_000);
        let report = scan_with_limits(
            &home,
            false,
            Limits {
                entries_per_route: 3,
                depth: 1,
                ..relaxed()
            },
        );
        assert!(!report.complete);
        let toolchains = report
            .rows
            .iter()
            .find(|row| row.id == "rust-toolchains")
            .unwrap();
        assert_eq!(toolchains.state, InventoryState::Partial);
        assert!(toolchains.logical_bytes.unwrap() <= 200);
        let versions = report
            .rows
            .iter()
            .find(|row| row.id == "claude-versions")
            .unwrap();
        assert_eq!(versions.state, InventoryState::Partial);
        assert_eq!(versions.allocated_bytes, None);
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.kind == IssueKind::DepthLimit)
        );
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.kind == IssueKind::EntryLimit)
        );
    }

    #[test]
    fn global_entry_and_time_limits_do_not_turn_unvisited_routes_into_zeroes() {
        let fixture = fixture();
        let home = home(&fixture);
        put(&home, ".rustup/toolchains/one", 10);
        put(&home, ".local/share/claude/versions/one", 10);
        let entries = scan_with_limits(
            &home,
            false,
            Limits {
                total_entries: 1,
                ..relaxed()
            },
        );
        assert!(entries.examined_entries <= 1);
        assert!(entries.rows.iter().all(|row| row.allocated_bytes.is_none()));
        assert!(
            entries
                .issues
                .iter()
                .any(|issue| issue.kind == IssueKind::EntryLimit)
        );
        let time = scan_with_limits(
            &home,
            false,
            Limits {
                total_time: Duration::ZERO,
                ..relaxed()
            },
        );
        assert!(time.rows.is_empty());
        assert_eq!(time.examined_entries, 0);
        assert!(
            time.issues
                .iter()
                .all(|issue| issue.kind == IssueKind::TimeLimit)
        );
    }

    #[test]
    fn inaccessible_root_is_unavailable_and_never_a_zero_byte_measurement() {
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let fixture = fixture();
        let home = home(&fixture);
        let file = put(&home, ".rustup/toolchains/secret", 1000);
        let toolchains = file.parent().unwrap();
        fs::set_permissions(toolchains, fs::Permissions::from_mode(0o000)).unwrap();
        let report = scan_with_limits(&home, false, relaxed());
        fs::set_permissions(toolchains, fs::Permissions::from_mode(0o700)).unwrap();
        let row = report
            .rows
            .iter()
            .find(|row| row.id == "rust-toolchains")
            .unwrap();
        assert_eq!(row.state, InventoryState::Unavailable);
        assert_eq!(row.allocated_bytes, None);
        assert_eq!(report.issues[0].kind, IssueKind::PermissionDenied);
    }

    #[test]
    fn denied_descendant_keeps_other_measurement_partial() {
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let fixture = fixture();
        let home = home(&fixture);
        put(&home, ".rustup/toolchains/available", 11);
        let secret = put(&home, ".rustup/toolchains/denied/secret", 9999);
        fs::set_permissions(secret.parent().unwrap(), fs::Permissions::from_mode(0o000)).unwrap();
        let report = scan_with_limits(&home, false, relaxed());
        fs::set_permissions(secret.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(report.rows[0].state, InventoryState::Partial);
        assert_eq!(report.rows[0].logical_bytes, Some(11));
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.kind == IssueKind::PermissionDenied)
        );
    }

    #[test]
    fn issue_memory_is_bounded_even_for_many_excluded_entries() {
        let fixture = fixture();
        let home = home(&fixture);
        fs::create_dir_all(home.join(".rustup/toolchains")).unwrap();
        for n in 0..100 {
            symlink(
                "/does-not-exist",
                home.join(format!(".rustup/toolchains/{n}")),
            )
            .unwrap();
        }
        let report = scan_with_limits(&home, false, relaxed());
        assert_eq!(report.issues.len(), MAX_ISSUES);
        assert_eq!(report.omitted_issues, 100 - MAX_ISSUES as u64);
        assert_eq!(report.rows[0].allocated_bytes, None);
    }

    #[test]
    fn a_directory_replaced_by_a_link_cannot_be_opened_from_old_metadata() {
        let fixture = fixture();
        let home = home(&fixture);
        fs::create_dir(home.join("cache")).unwrap();
        let parent = open_absolute_directory(&home).unwrap();
        let before = stat_at(parent.as_raw_fd(), c"cache").unwrap();
        fs::rename(home.join("cache"), home.join("old-cache")).unwrap();
        symlink("/", home.join("cache")).unwrap();
        assert!(
            Directory::open(parent.as_raw_fd(), c"cache", &before, home.join("cache")).is_err()
        );
    }

    #[test]
    fn a_renamed_intermediate_ancestor_invalidates_the_display_path() {
        let fixture = fixture();
        let home = home(&fixture);
        put(&home, ".rustup/toolchains/original", 100);
        let base = open_absolute_directory(&home).unwrap();
        let before = locate(base.as_raw_fd(), Path::new(".rustup/toolchains"), None)
            .unwrap()
            .unwrap();
        fs::rename(home.join(".rustup"), home.join(".rustup-moved")).unwrap();
        put(&home, ".rustup/toolchains/replacement", 10_000);
        // The detached parent still resolves the original leaf, so checking
        // that descriptor alone would falsely validate the display pathname.
        assert_eq!(
            stat_at(before.parent.as_raw_fd(), &before.name).unwrap(),
            before.meta
        );
        let result = verify_location(
            base.as_raw_fd(),
            Path::new(".rustup/toolchains"),
            None,
            &before.meta,
        );
        assert_eq!(result.unwrap_err().kind, IssueKind::Changed);
    }

    #[test]
    fn launch_agents_are_shallow_plist_observations_without_stale_claims() {
        let fixture = fixture();
        let home = home(&fixture);
        put(&home, "Library/LaunchAgents/com.example.helper.plist", 23);
        put(&home, "Library/LaunchAgents/.DS_Store", 900);
        put(&home, "Library/LaunchAgents/unrelated/nested.plist", 900);
        let report = scan_with_limits(&home, false, relaxed());
        assert!(report.complete);
        assert_eq!(report.rows[0].id, "login-agents");
        assert_eq!(report.rows[0].logical_bytes, Some(23));
        assert_eq!(report.rows[0].files, 1);
        assert!(!report.rows[0].title.contains("stale"));
    }

    #[test]
    fn fixed_routes_are_unique_narrow_and_serialized_without_cleanup_authority() {
        let mut ids = HashSet::new();
        for routes in [USER_ROUTES, SYSTEM_ROUTES] {
            for route in routes {
                assert!(ids.insert(route.id));
                assert!(
                    Path::new(route.relative)
                        .components()
                        .all(|part| matches!(part, Component::Normal(_)))
                );
                assert!(!matches!(
                    route.relative,
                    "Library"
                        | "Library/Application Support"
                        | "Library/Containers"
                        | "Movies"
                        | ".aws"
                        | ".config"
                        | ".local/share"
                ));
                assert!(
                    route
                        .provider
                        .is_none_or(|provider| matches!(provider, "homebrew" | "uv" | "docker"))
                );
            }
        }
        let fixture = fixture();
        let home = home(&fixture);
        for route in USER_ROUTES {
            for candidate in crate::recommendations::DEVELOPER_CACHE_ROUTES {
                let observed = Path::new(route.relative);
                let candidate = Path::new(candidate.path);
                assert!(
                    !observed.starts_with(candidate) && !candidate.starts_with(observed),
                    "Inventory must not duplicate a scanner-owned developer cache: {}",
                    route.relative
                );
            }
            if route.file_only {
                put(&home, route.relative, 1);
            } else {
                put(
                    &home,
                    &format!(
                        "{}/fixture{}",
                        route.relative,
                        route.child_suffix.unwrap_or("")
                    ),
                    1,
                );
            }
        }
        let report = scan_with_limits(&home, false, relaxed());
        assert!(report.complete, "{:?}", report.issues);
        assert_eq!(report.rows.len(), USER_ROUTES.len());
        assert!(
            report
                .rows
                .iter()
                .all(|row| row.cleanup_authority == CleanupAuthority::ReviewOnly)
        );
        assert!(report.rows.iter().all(|row| row.path.starts_with(&home)));
        let json = serde_json::to_value(&report).unwrap();
        assert!(json.get("recovery_bytes").is_none());
        assert!(json.get("cleanup").is_none());
        assert!(
            json["rows"]
                .as_array()
                .unwrap()
                .iter()
                .all(|row| row["cleanup_authority"] == "ReviewOnly")
        );
    }
}
