//! Shared, lexical recommendation policy. Recognizing a location never grants
//! filesystem access or deletion permission; the scanner and cleanup still
//! validate the original grant, identities and complete measurements.
use crate::model::Root;
use std::ffi::OsStr;
use std::fmt::Write;
use std::path::{Path, PathBuf};

pub(crate) const SWIFTPM_CACHE_ROUTE: &str = "Library/org.swift.swiftpm/cache";
pub(crate) const HOME_LIBRARY_ROUTES: [&str; 5] = [
    "Library/Caches",
    "Library/Logs",
    "Library/Developer/Xcode/DerivedData",
    "Library/Application Support",
    SWIFTPM_CACHE_ROUTE,
];

/// Only these generated leaves may be reviewed inside Application Support.
/// In particular, browser profiles, offline storage, sessions, databases and
/// all other siblings remain outside discovery and cleanup authorization.
const SUPPORT_CACHE_APPS: &[(&str, &str)] = &[
    ("Slack", "com.tinyspeck.slackmacgap"),
    ("Claude", "com.anthropic.claudefordesktop"),
    ("discord", "com.hnc.Discord"),
    ("Code", "com.microsoft.VSCode"),
    ("Cursor", "com.todesktop.230313mzl4w4u92"),
];
const SUPPORT_CACHE_LEAVES: &[&str] = &[
    "Cache",
    "Code Cache",
    "GPUCache",
    "DawnCache",
    "DawnGraphiteCache",
    "DawnWebGPUCache",
];

/// Generated application caches retain their review-and-Trash policy.
const HOME_CACHE_ROUTES: &[(&str, &str)] = &[
    (".cache/opencode", "OpenCode"),
    (".cache/ghostty", "Ghostty"),
    (".oh-my-zsh/cache", "Oh My Zsh"),
];
const HOME_LOG_ROUTES: &[&str] = &[".npm/_logs", ".config/gcloud/logs", ".azure/logs"];

pub(crate) struct DeveloperCacheRoute {
    pub path: &'static str,
    pub title: &'static str,
    pub owner: &'static str,
}

/// Exact generated stores under an explicit Home grant. Configuration,
/// persistent credentials, installed toolchains and runtime versions are excluded.
/// A matching location still needs full identity, activity, Git, link and
/// manifest validation before either cleanup operation can be offered.
pub(crate) const DEVELOPER_CACHE_ROUTES: &[DeveloperCacheRoute] = &[
    DeveloperCacheRoute {
        path: ".npm/_cacache",
        title: "npm package cache",
        owner: "node",
    },
    DeveloperCacheRoute {
        path: ".npm/_npx",
        title: "npx temporary packages",
        owner: "node",
    },
    DeveloperCacheRoute {
        path: ".cache/node/corepack",
        title: "Corepack package cache",
        owner: "node",
    },
    DeveloperCacheRoute {
        path: "Library/Caches/node/corepack",
        title: "Corepack package cache",
        owner: "node",
    },
    DeveloperCacheRoute {
        path: ".bun/install/cache",
        title: "Bun package cache",
        owner: "bun",
    },
    DeveloperCacheRoute {
        path: ".cache/pip",
        title: "pip download cache",
        owner: "pip",
    },
    DeveloperCacheRoute {
        path: "Library/Caches/pip",
        title: "pip download cache",
        owner: "pip",
    },
    DeveloperCacheRoute {
        path: ".cache/uv",
        title: "uv package cache",
        owner: "uv",
    },
    DeveloperCacheRoute {
        path: "Library/Caches/uv",
        title: "uv package cache",
        owner: "uv",
    },
    DeveloperCacheRoute {
        path: ".cache/mise",
        title: "mise download cache",
        owner: "mise",
    },
    DeveloperCacheRoute {
        path: "Library/Caches/mise",
        title: "mise download cache",
        owner: "mise",
    },
    DeveloperCacheRoute {
        path: ".cargo/registry/cache",
        title: "Cargo crate downloads",
        owner: "cargo",
    },
    DeveloperCacheRoute {
        path: ".cargo/registry/src",
        title: "Cargo unpacked crates",
        owner: "cargo",
    },
    DeveloperCacheRoute {
        path: ".cargo/git/db",
        title: "Cargo Git downloads",
        owner: "cargo",
    },
    DeveloperCacheRoute {
        path: ".cargo/git/checkouts",
        title: "Cargo Git checkouts",
        owner: "cargo",
    },
    DeveloperCacheRoute {
        path: "Library/Caches/org.swift.swiftpm",
        title: "SwiftPM package cache",
        owner: "swiftpm",
    },
    DeveloperCacheRoute {
        path: SWIFTPM_CACHE_ROUTE,
        title: "SwiftPM package cache",
        owner: "swiftpm",
    },
    DeveloperCacheRoute {
        path: "Library/Caches/Homebrew/downloads",
        title: "Homebrew downloads",
        owner: "homebrew",
    },
    DeveloperCacheRoute {
        path: ".aws/cli/cache",
        title: "AWS CLI session cache",
        owner: "aws",
    },
    DeveloperCacheRoute {
        path: ".cache/zig",
        title: "Zig build cache",
        owner: "zig",
    },
    DeveloperCacheRoute {
        path: "Library/Caches/zig",
        title: "Zig build cache",
        owner: "zig",
    },
    DeveloperCacheRoute {
        path: ".cache/ruff",
        title: "Ruff cache",
        owner: "ruff",
    },
    DeveloperCacheRoute {
        path: ".cache/mypy",
        title: "MyPy cache",
        owner: "mypy",
    },
    DeveloperCacheRoute {
        path: ".cache/typescript",
        title: "TypeScript cache",
        owner: "node",
    },
    DeveloperCacheRoute {
        path: ".cache/eslint",
        title: "ESLint cache",
        owner: "node",
    },
    DeveloperCacheRoute {
        path: ".cache/prettier",
        title: "Prettier cache",
        owner: "node",
    },
    DeveloperCacheRoute {
        path: ".expo/native-modules-cache",
        title: "Expo native modules cache",
        owner: "node",
    },
    DeveloperCacheRoute {
        path: ".expo/versions-cache",
        title: "Expo versions cache",
        owner: "node",
    },
    DeveloperCacheRoute {
        path: ".expo/schema-cache",
        title: "Expo schema cache",
        owner: "node",
    },
    DeveloperCacheRoute {
        path: ".expo/template-cache",
        title: "Expo templates cache",
        owner: "node",
    },
];

pub(crate) fn developer_cache_route(
    root: &Root,
    path: &Path,
) -> Option<&'static DeveloperCacheRoute> {
    if root.kind != "home" {
        return None;
    }
    let relative = path.strip_prefix(&root.path).ok()?;
    DEVELOPER_CACHE_ROUTES
        .iter()
        .find(|route| relative == Path::new(route.path))
}

pub(crate) fn developer_cache_route_allowed(root: &Root, path: &Path) -> bool {
    root.kind == "home"
        && path.strip_prefix(&root.path).is_ok_and(|relative| {
            !relative.as_os_str().is_empty()
                && DEVELOPER_CACHE_ROUTES.iter().any(|route| {
                    relative.starts_with(route.path) || Path::new(route.path).starts_with(relative)
                })
        })
}

pub(crate) fn developer_cache_event_scope(root: &Root, path: &Path) -> Option<PathBuf> {
    if root.kind != "home" {
        return None;
    }
    let relative = path.strip_prefix(&root.path).ok()?;
    DEVELOPER_CACHE_ROUTES.iter().find_map(|route| {
        relative
            .starts_with(route.path)
            .then(|| root.path.join(route.path))
    })
}

fn support_cache_unit(suffix: &Path) -> Option<(PathBuf, &'static str)> {
    let mut components = suffix.components();
    if let Some(app) = components.next().map(|part| part.as_os_str())
        && let Some((_, owner)) = SUPPORT_CACHE_APPS
            .iter()
            .find(|(known, _)| app == OsStr::new(known))
        && let Some(leaf) = components.next().map(|part| part.as_os_str())
        && SUPPORT_CACHE_LEAVES
            .iter()
            .any(|known| leaf == OsStr::new(known))
    {
        return Some((Path::new(app).join(leaf), owner));
    }
    // The updater's download cache is separate from installed versions and
    // registration state. No other Google application support is admitted.
    let updater = Path::new("Google/GoogleUpdater/crx_cache");
    suffix
        .starts_with(updater)
        .then(|| (updater.to_path_buf(), "com.google.GoogleUpdater"))
}

fn support_route_allowed(suffix: &Path) -> bool {
    suffix.as_os_str().is_empty()
        || SUPPORT_CACHE_APPS
            .iter()
            .any(|(app, _)| suffix == Path::new(app))
        || matches!(suffix.to_str(), Some("Google" | "Google/GoogleUpdater"))
        || support_cache_unit(suffix).is_some()
}

pub(crate) fn home_cache_candidate(
    root: &Root,
    path: &Path,
    directory: bool,
) -> Option<&'static str> {
    if root.kind != "home" {
        return None;
    }
    if directory && developer_cache_route(root, path).is_some() {
        return Some("devcache");
    }
    let relative = path.strip_prefix(&root.path).ok()?;
    if directory
        && HOME_CACHE_ROUTES
            .iter()
            .any(|(route, _)| relative == Path::new(route))
    {
        Some("cache")
    } else if !directory && home_log_scope(root, path) {
        Some("log")
    } else {
        None
    }
}

pub(crate) fn home_log_scope(root: &Root, path: &Path) -> bool {
    root.kind == "home"
        && path.strip_prefix(&root.path).is_ok_and(|relative| {
            HOME_LOG_ROUTES
                .iter()
                .any(|route| relative.starts_with(route))
        })
}

pub(crate) fn home_cache_event_scope(root: &Root, path: &Path) -> Option<PathBuf> {
    if root.kind != "home" {
        return None;
    }
    let relative = path.strip_prefix(&root.path).ok()?;
    if let Some(scope) = developer_cache_event_scope(root, path) {
        return Some(scope);
    }
    HOME_CACHE_ROUTES
        .iter()
        .find_map(|(route, _)| relative.starts_with(route).then(|| root.path.join(route)))
        .or_else(|| home_log_scope(root, path).then(|| path.to_path_buf()))
}

pub(crate) fn cache_title(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_string_lossy();
    if let Some((app, _)) = path.parent().and_then(Path::file_name).and_then(|name| {
        SUPPORT_CACHE_APPS
            .iter()
            .find(|(app, _)| name == OsStr::new(app))
    }) && SUPPORT_CACHE_LEAVES.contains(&name.as_ref())
    {
        return Some(format!("{app} {name}"));
    }
    HOME_CACHE_ROUTES
        .iter()
        .find_map(|(route, title)| path.ends_with(route).then(|| format!("{title} cache")))
        .or_else(|| {
            path.ends_with("Google/GoogleUpdater/crx_cache")
                .then(|| "GoogleUpdater download cache".into())
        })
}

pub(crate) fn permanent_kind(kind: &str) -> bool {
    matches!(kind, "cargo" | "node" | "venv" | "webcache" | "devcache")
}

pub(crate) fn review_project_kind(kind: &str) -> bool {
    matches!(
        kind,
        "swiftpm" | "dotnet" | "gradle" | "dart" | "flutter" | "zig" | "pythoncache"
    )
}

pub(crate) fn developer_measurement(kind: &str) -> bool {
    permanent_kind(kind) || review_project_kind(kind) || kind == "xcode"
}

pub(crate) fn checks_git(kind: &str) -> bool {
    permanent_kind(kind) || review_project_kind(kind) || kind == "largefile"
}

pub(crate) fn checks_activity(kind: &str) -> bool {
    permanent_kind(kind) || review_project_kind(kind) || matches!(kind, "cache" | "xcode")
}

pub(crate) fn minimum_bytes(kind: &str) -> u64 {
    match kind {
        "cache" => 1_000_000,
        "archive" => 50_000_000,
        "log" | "crashreport" | "pythoncache" | "devcache" => 4_096,
        "installer" => 20_000_000,
        "xcode" => 1_000_000,
        "largefile" => 500_000_000,
        "cargo" | "node" | "venv" | "webcache" | "download" => 100_000_000,
        kind if review_project_kind(kind) => 100_000_000,
        _ => u64::MAX,
    }
}

pub(crate) fn minimum_size_label(kind: &str) -> String {
    let bytes = minimum_bytes(kind);
    if bytes >= 1_000_000 {
        format!("{} MB", bytes / 1_000_000)
    } else {
        format!("{} KB", bytes / 1_024)
    }
}

pub(crate) fn quiet_days(kind: &str) -> i64 {
    match kind {
        "devcache" => 0,
        "cargo" | "node" | "venv" | "webcache" => 7,
        "cache" => 1,
        "log" | "crashreport" | "xcode" => 7,
        kind if review_project_kind(kind) => 7,
        "installer" => 14,
        "largefile" => 90,
        _ => 30,
    }
}

const KINDS: [&str; 20] = [
    "devcache",
    "cache",
    "log",
    "crashreport",
    "cargo",
    "node",
    "venv",
    "webcache",
    "xcode",
    "installer",
    "archive",
    "download",
    "largefile",
    "swiftpm",
    "dotnet",
    "gradle",
    "dart",
    "flutter",
    "zig",
    "pythoncache",
];

/// These SQL expressions are built once per query/index, from the same policy
/// as scan-time eligibility. The column argument is internal, never user input.
pub(crate) fn minimum_size_sql(column: &str) -> String {
    let mut sql = format!("CASE json_extract({column},'$.kind')");
    for kind in KINDS {
        write!(&mut sql, " WHEN '{kind}' THEN {}", minimum_bytes(kind)).unwrap();
    }
    // NULL makes unknown kinds fail the comparison at every reported size,
    // including malformed rows larger than SQLite's signed integer range.
    sql.push_str(" ELSE NULL END");
    sql
}

pub(crate) fn priority_sql(column: &str) -> String {
    // Prefer lower recreation cost before bytes. This is a conservative policy
    // class, not an invented estimate of rebuild time or network consumption.
    // Personal files remain explicit review decisions after generated data.
    format!(
        "CASE json_extract({column},'$.kind') \
        WHEN 'log' THEN 0 WHEN 'crashreport' THEN 0 \
        WHEN 'cache' THEN 1 WHEN 'devcache' THEN 1 WHEN 'webcache' THEN 1 WHEN 'dart' THEN 1 WHEN 'pythoncache' THEN 1 \
        WHEN 'cargo' THEN 2 WHEN 'xcode' THEN 2 WHEN 'zig' THEN 2 \
        WHEN 'dotnet' THEN 2 WHEN 'flutter' THEN 2 WHEN 'gradle' THEN 2 \
        WHEN 'swiftpm' THEN 3 WHEN 'node' THEN 3 WHEN 'venv' THEN 3 \
        WHEN 'installer' THEN 4 WHEN 'archive' THEN 5 WHEN 'download' THEN 5 \
        WHEN 'largefile' THEN 6 ELSE 7 END"
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LibraryArea {
    Caches,
    Logs,
    Xcode,
    ApplicationSupport,
}

/// Only an explicit Home grant opts into these routes. A project containing a
/// folder named Library is not an application-data authorization.
pub(crate) fn library_area<'a>(root: &Root, path: &'a Path) -> Option<(LibraryArea, &'a Path)> {
    if root.kind != "home" {
        return None;
    }
    let relative = path.strip_prefix(&root.path).ok()?;
    // The fifth start is an exact candidate, not an open-ended Library area.
    for (route, area) in HOME_LIBRARY_ROUTES.into_iter().take(4).zip([
        LibraryArea::Caches,
        LibraryArea::Logs,
        LibraryArea::Xcode,
        LibraryArea::ApplicationSupport,
    ]) {
        if let Ok(suffix) = relative.strip_prefix(route) {
            return Some((area, suffix));
        }
    }
    None
}

pub(crate) fn library_corridor(root: &Root, path: &Path) -> bool {
    root.kind == "home"
        && path.strip_prefix(&root.path).is_ok_and(|relative| {
            matches!(
                relative.to_str(),
                Some(
                    "Library"
                        | "Library/Developer"
                        | "Library/Developer/Xcode"
                        | "Library/org.swift.swiftpm"
                )
            )
        })
}

/// Unrecognized manager stores never fall through to the app-cache adapter.
/// Only the exact developer-cache table can admit a generated manager route.
pub(crate) fn managed_cache(name: &OsStr) -> bool {
    // The Library name filter has no Root, but it is used only after the
    // caller enters an authorized Library/Caches lane. Admit table corridors;
    // library_route_allowed still rejects every unsupported child below them.
    if developer_library_cache_name(name) {
        return false;
    }
    let Some(name) = name.to_str() else {
        return true;
    };
    [
        "homebrew",
        "pip",
        "uv",
        "npm",
        "pnpm",
        "yarn",
        "bun",
        "cocoapods",
        "org.swift.swiftpm",
        "go-build",
        "ms-playwright",
    ]
    .iter()
    .any(|known| name.eq_ignore_ascii_case(known))
}

fn developer_library_cache_name(name: &OsStr) -> bool {
    DEVELOPER_CACHE_ROUTES.iter().any(|route| {
        Path::new(route.path)
            .strip_prefix("Library/Caches")
            .is_ok_and(|suffix| {
                suffix
                    .components()
                    .next()
                    .is_some_and(|part| part.as_os_str() == name)
            })
    })
}

pub(crate) fn library_route_allowed(root: &Root, path: &Path) -> bool {
    if library_corridor(root, path) {
        return true;
    }
    if root.kind == "home"
        && path
            .strip_prefix(&root.path)
            .is_ok_and(|relative| relative.starts_with("Library"))
        && developer_cache_route_allowed(root, path)
    {
        return true;
    }
    library_area(root, path).is_some_and(|(area, suffix)| {
        if area == LibraryArea::ApplicationSupport {
            return support_route_allowed(suffix);
        }
        if area != LibraryArea::Caches {
            return true;
        }
        if developer_cache_route_allowed(root, path) {
            return true;
        }
        let mut components = suffix.components();
        let Some(first) = components.next().map(|component| component.as_os_str()) else {
            return true;
        };
        if developer_library_cache_name(first) {
            return false;
        }
        if managed_cache(first) {
            return false;
        }
        if first.eq_ignore_ascii_case("Google") {
            match components.next().map(|component| component.as_os_str()) {
                // Google is a corridor; Chrome is a nested corridor. A visible
                // profile is the smallest browser cleanup unit.
                None => true,
                Some(browser) if browser.eq_ignore_ascii_case("Chrome") => components
                    .next()
                    .is_none_or(|profile| browser_component_allowed(profile.as_os_str())),
                Some(child) => browser_component_allowed(child),
            }
        } else if first.eq_ignore_ascii_case("Chromium") {
            match components.next().map(|component| component.as_os_str()) {
                None => true,
                Some(profile) => browser_component_allowed(profile),
            }
        } else {
            true
        }
    })
}

fn browser_component_allowed(component: &OsStr) -> bool {
    let bytes = component.as_encoded_bytes();
    !bytes.is_empty() && bytes[0] != b'.' && bytes != b".."
}

fn path_ends_with_ascii_case(path: &Path, suffix: &[&str]) -> bool {
    let mut components = path.components().rev();
    suffix.iter().rev().all(|expected| {
        components
            .next()
            .is_some_and(|component| component.as_os_str().eq_ignore_ascii_case(expected))
    })
}

/// Return the owner of an exact browser profile or known Application Support
/// cache leaf. The caller must already have validated its Home cache location;
/// lexical recognition never authorizes an arbitrary path or sibling data.
pub(crate) fn browser_cache_owner(location: &Path) -> Option<&'static str> {
    for ancestor in location.ancestors() {
        if path_ends_with_ascii_case(ancestor, &["Library", "Application Support"]) {
            let suffix = location.strip_prefix(ancestor).ok()?;
            return support_cache_unit(suffix)
                .filter(|(unit, _)| suffix == unit)
                .map(|(_, owner)| owner);
        }
    }
    let profile = location.file_name()?;
    if !browser_component_allowed(profile) {
        return None;
    }
    let browser = location.parent()?;
    if path_ends_with_ascii_case(browser, &["Library", "Caches", "Google", "Chrome"]) {
        Some("com.google.Chrome")
    } else if path_ends_with_ascii_case(browser, &["Library", "Caches", "Chromium"]) {
        Some("org.chromium.Chromium")
    } else {
        None
    }
}

pub(crate) fn library_candidate(root: &Root, path: &Path, directory: bool) -> Option<&'static str> {
    if developer_cache_route(root, path).is_some() {
        return directory.then_some("devcache");
    }
    if developer_cache_route_allowed(root, path) {
        return None;
    }
    let (area, suffix) = library_area(root, path)?;
    if suffix.as_os_str().is_empty() || !library_route_allowed(root, path) {
        return None;
    }
    match area {
        LibraryArea::Caches => {
            let mut components = suffix.components();
            let vendor = components.next().map(|component| component.as_os_str())?;
            let second = components.next().map(|component| component.as_os_str());
            match (vendor, second) {
                // Keep vendor and browser directories as traversal corridors.
                (vendor, None)
                    if vendor.eq_ignore_ascii_case("Google")
                        || vendor.eq_ignore_ascii_case("Chromium") =>
                {
                    None
                }
                (vendor, Some(browser))
                    if vendor.eq_ignore_ascii_case("Google")
                        && browser.eq_ignore_ascii_case("Chrome") =>
                {
                    let profile = components.next().map(|component| component.as_os_str());
                    (directory
                        && profile.is_some_and(browser_component_allowed)
                        && components.next().is_none())
                    .then_some("cache")
                }
                (vendor, Some(profile)) if vendor.eq_ignore_ascii_case("Chromium") => {
                    (directory && browser_component_allowed(profile) && components.next().is_none())
                        .then_some("cache")
                }
                (vendor, Some(child))
                    if vendor.eq_ignore_ascii_case("Google")
                        && !child.eq_ignore_ascii_case("Chrome") =>
                {
                    (directory && browser_component_allowed(child) && components.next().is_none())
                        .then_some("cache")
                }
                (_, None) => Some("cache"),
                _ => None,
            }
        }
        // Log directories are traversal scopes, not cleanup units. A newly
        // written current log must not hide an old rotated log beside it.
        LibraryArea::Logs if !directory => Some(if suffix.starts_with("DiagnosticReports") {
            "crashreport"
        } else {
            "log"
        }),
        LibraryArea::Xcode if directory && suffix.components().count() == 1 => Some("xcode"),
        LibraryArea::ApplicationSupport if directory => support_cache_unit(suffix)
            .filter(|(unit, _)| suffix == unit)
            .map(|_| "cache"),
        _ => None,
    }
}

/// Changes inside a cache/build unit invalidate that exact unit. Logs stay
/// file-scoped, so a logging app cannot continually rescan all user logs.
pub(crate) fn library_event_scope(root: &Root, path: &Path) -> Option<PathBuf> {
    if let Some(scope) = developer_cache_event_scope(root, path) {
        return Some(scope);
    }
    let (area, suffix) = library_area(root, path)?;
    if !library_route_allowed(root, path) {
        return None;
    }
    if area == LibraryArea::Logs || suffix.as_os_str().is_empty() {
        return Some(path.to_path_buf());
    }
    if developer_cache_route_allowed(root, path) {
        return Some(path.to_path_buf());
    }
    if area == LibraryArea::ApplicationSupport {
        return Some(
            root.path.join(HOME_LIBRARY_ROUTES[3]).join(
                support_cache_unit(suffix)
                    .map(|(unit, _)| unit)
                    .unwrap_or_else(|| suffix.to_path_buf()),
            ),
        );
    }
    let route = match area {
        LibraryArea::Caches => HOME_LIBRARY_ROUTES[0],
        LibraryArea::Xcode => HOME_LIBRARY_ROUTES[2],
        LibraryArea::Logs => unreachable!(),
        LibraryArea::ApplicationSupport => unreachable!(),
    };
    if area == LibraryArea::Caches {
        let mut components = suffix.components();
        let Some(first) = components.next().map(|component| component.as_os_str()) else {
            return Some(path.to_path_buf());
        };
        if first.eq_ignore_ascii_case("Google") || first.eq_ignore_ascii_case("Chromium") {
            let second = components.next().map(|component| component.as_os_str());
            let third = if first.eq_ignore_ascii_case("Google")
                && second.is_some_and(|name| name.eq_ignore_ascii_case("Chrome"))
            {
                components.next().map(|component| component.as_os_str())
            } else {
                None
            };
            let mut path = root.path.join(route);
            path.push(first);
            if let Some(second) = second {
                path.push(second);
            }
            if let Some(third) = third {
                path.push(third);
            }
            return Some(path);
        }
    }
    Some(root.path.join(route).join(suffix.components().next()?))
}

pub(crate) fn personal_scope(root: &Root, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(&root.path) else {
        return false;
    };
    if relative.components().any(|part| {
        part.as_os_str()
            .as_encoded_bytes()
            .first()
            .is_some_and(|byte| *byte == b'.')
    }) {
        return false;
    }
    match root.kind.as_str() {
        "home" => relative.starts_with("Desktop") || relative.starts_with("Documents"),
        // An individually chosen folder is also an explicit personal-file scope.
        // The Projects choice remains a cheap developer-only discovery mode.
        "folder" => true,
        _ => false,
    }
}

pub(crate) fn downloads_boundary(root: &Root) -> Option<PathBuf> {
    match root.kind.as_str() {
        "downloads" => Some(root.path.clone()),
        "folder" if root.path.file_name() == Some(OsStr::new("Downloads")) => {
            Some(root.path.clone())
        }
        "home" => Some(root.path.join("Downloads")),
        _ => None,
    }
}

const INSTALLER_EXTENSIONS: &[&str] = &["dmg", "pkg"];
const ARCHIVE_EXTENSIONS: &[&str] = &[
    "zip", "tar", "gz", "tgz", "bz2", "tbz", "tbz2", "xz", "txz", "zst", "7z", "rar", "iso", "xip",
];

fn extension_is(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| {
            extensions
                .iter()
                .any(|known| extension.eq_ignore_ascii_case(known))
        })
}

pub(crate) fn installer(path: &Path) -> bool {
    extension_is(path, INSTALLER_EXTENSIONS)
}

pub(crate) fn archive(path: &Path) -> bool {
    extension_is(path, ARCHIVE_EXTENSIONS)
}

/// A names-only pass ignores ordinary source/configuration files. Full metadata
/// is requested only for formats useful to a personal-file review. No file
/// contents or speculative duplicate hashes are read during discovery.
pub(crate) fn personal_file_name(name: &OsStr) -> bool {
    if name.as_encoded_bytes().first() == Some(&b'.') {
        return false;
    }
    let Some(extension) = Path::new(name).extension().and_then(OsStr::to_str) else {
        return false;
    };
    INSTALLER_EXTENSIONS
        .iter()
        .chain(ARCHIVE_EXTENSIONS)
        .chain(
            [
                "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "pages", "numbers", "key",
                "csv", "tsv", "mp4", "mov", "m4v", "avi", "mkv", "webm", "wav", "aif", "aiff",
                "flac", "mp3", "m4a", "jpg", "jpeg", "png", "heic", "gif", "tiff", "tif", "raw",
                "psd", "ai", "sketch", "fig", "blend",
            ]
            .iter(),
        )
        .any(|known| extension.eq_ignore_ascii_case(known))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Identity;

    fn home() -> Root {
        Root {
            id: "home".into(),
            path: "/Users/test".into(),
            kind: "home".into(),
            identity: Identity {
                device: 0,
                inode: 0,
                mode: 0,
                size: 0,
                modified_ns: 0,
                changed_ns: 0,
            },
        }
    }

    #[test]
    fn library_routes_are_exact_home_scopes_not_general_library_access() {
        let mut root = home();
        for route in HOME_LIBRARY_ROUTES {
            assert!(library_route_allowed(&root, &root.path.join(route)));
        }
        for relative in [
            "Library/Application Support/app",
            "Library/Keychains",
            "Library/Containers/app/Data/Library/Caches",
            "Library/Developer/Xcode/Archives",
            "Library/Developer/CoreSimulator",
            "Projects/Library/Caches/app",
            "Library/Caches-old/app",
            "Library/Caches/pnpm/item",
            "Library/Caches/Homebrew/item",
        ] {
            assert!(
                !library_route_allowed(&root, &root.path.join(relative)),
                "{relative}"
            );
        }
        root.kind = "projects".into();
        assert!(!library_route_allowed(
            &root,
            &root.path.join("Library/Caches/app")
        ));
    }

    #[test]
    fn support_and_hidden_cache_routes_are_exact_generated_leaves() {
        let mut root = home();
        for relative in [
            "Library/Application Support/Slack/Cache",
            "Library/Application Support/Slack/Code Cache",
            "Library/Application Support/Claude/GPUCache",
            "Library/Application Support/Code/DawnGraphiteCache",
            "Library/Application Support/Google/GoogleUpdater/crx_cache",
        ] {
            let path = root.path.join(relative);
            assert!(library_route_allowed(&root, &path));
            assert_eq!(library_candidate(&root, &path, true), Some("cache"));
            assert_eq!(library_candidate(&root, &path, false), None);
            assert_eq!(
                library_event_scope(&root, &path.join("nested/payload")),
                Some(path)
            );
        }
        for relative in [
            "Library/Application Support/Slack/Local Storage",
            "Library/Application Support/Slack/Service Worker/CacheStorage",
            "Library/Application Support/Slack/Cache-backup",
            "Library/Application Support/Claude/claude-code",
            "Library/Application Support/Google/Chrome/Default/Cache",
            "Library/Application Support/Google/GoogleUpdater/Current",
            "Library/Application Support/Unknown/Cache",
        ] {
            assert!(
                !library_route_allowed(&root, &root.path.join(relative)),
                "{relative}"
            );
        }
        for (relative, kind) in [
            (".cache/zig", "devcache"),
            (".expo/versions-cache", "devcache"),
            (".oh-my-zsh/cache", "cache"),
        ] {
            let path = root.path.join(relative);
            assert_eq!(home_cache_candidate(&root, &path, true), Some(kind));
            assert_eq!(
                home_cache_candidate(&root, &path.join("nested"), true),
                None
            );
            assert_eq!(
                home_cache_event_scope(&root, &path.join("payload")),
                Some(path)
            );
        }
        assert_eq!(
            home_cache_candidate(&root, &root.path.join(".npm/_logs/old.log"), false),
            Some("log")
        );
        for relative in [
            ".cache",
            ".cache/unknown",
            ".npm/_update-notifier-last-checked",
            ".cargo/git",
            ".aws/credentials",
        ] {
            assert_eq!(
                home_cache_candidate(&root, &root.path.join(relative), true),
                None
            );
        }
        assert_eq!(
            browser_cache_owner(&root.path.join("Library/Application Support/Slack/Cache")),
            Some("com.tinyspeck.slackmacgap")
        );
        assert_eq!(
            browser_cache_owner(
                &root
                    .path
                    .join("Library/Application Support/Slack/Local Storage")
            ),
            None
        );
        root.kind = "folder".into();
        assert!(!library_route_allowed(
            &root,
            &root.path.join("Library/Application Support/Slack/Cache")
        ));
        assert_eq!(
            home_cache_candidate(&root, &root.path.join(".cache/zig"), true),
            None
        );
    }

    #[test]
    fn generated_data_has_useful_floors_without_changing_personal_or_permanent_policy() {
        assert_eq!(
            (minimum_bytes("devcache"), quiet_days("devcache")),
            (4096, 0)
        );
        assert!(
            permanent_kind("devcache") && checks_git("devcache") && checks_activity("devcache")
        );
        assert_eq!(
            (minimum_bytes("cache"), quiet_days("cache")),
            (1_000_000, 1)
        );
        assert_eq!((minimum_bytes("log"), quiet_days("log")), (4_096, 7));
        assert_eq!(
            (minimum_bytes("xcode"), quiet_days("xcode")),
            (1_000_000, 7)
        );
        assert_eq!(minimum_size_label("log"), "4 KB");
        assert_eq!(minimum_size_label("cache"), "1 MB");
        assert_eq!(
            (minimum_bytes("largefile"), quiet_days("largefile")),
            (500_000_000, 90)
        );
        assert_eq!(
            (minimum_bytes("cargo"), quiet_days("cargo")),
            (100_000_000, 7)
        );
        assert!(!permanent_kind("pythoncache"));
        assert!(checks_git("pythoncache") && checks_activity("pythoncache"));
    }

    #[test]
    fn devcache_routes_require_exact_home_leaves_and_never_cover_adjacent_state() {
        let root = home();
        for route in DEVELOPER_CACHE_ROUTES {
            let path = root.path.join(route.path);
            assert_eq!(
                developer_cache_route(&root, &path).unwrap().path,
                route.path
            );
            assert_eq!(home_cache_candidate(&root, &path, true), Some("devcache"));
            assert_eq!(home_cache_candidate(&root, &path, false), None);
            assert!(developer_cache_route(&root, &path.join("child")).is_none());
            assert_eq!(
                developer_cache_event_scope(&root, &path.join("child/nested")),
                Some(path.clone())
            );
            if route.path.starts_with("Library/") {
                assert!(library_route_allowed(&root, &path));
                assert_eq!(library_candidate(&root, &path, true), Some("devcache"));
            }
            for kind in ["projects", "folder", "downloads"] {
                let mut other = root.clone();
                other.kind = kind.into();
                assert!(developer_cache_route(&other, &path).is_none());
                assert!(home_cache_candidate(&other, &path, true).is_none());
            }
        }
        for path in [
            ".cargo",
            ".cargo/bin",
            ".cargo/registry",
            ".npm",
            ".bun/install",
            ".rustup/toolchains",
            ".local/share/mise/installs",
            ".aws/credentials",
            ".aws/config",
            ".expo/state.json",
            "Library/Caches/Homebrew",
            "Library/Caches/Homebrew/locks",
            "Library/Caches/node/other",
            "Library/org.swift.swiftpm/configuration",
        ] {
            assert!(
                developer_cache_route(&root, &root.path.join(path)).is_none(),
                "{path}"
            );
            assert!(
                home_cache_candidate(&root, &root.path.join(path), true).is_none(),
                "{path}"
            );
            if path.starts_with("Library/") && !matches!(path, "Library/Caches/Homebrew") {
                assert!(
                    !library_route_allowed(&root, &root.path.join(path)),
                    "{path}"
                );
            }
        }
    }

    #[test]
    fn library_refreshes_match_cleanup_units_without_widening_log_writes() {
        let root = home();
        for (relative, expected) in [
            (
                "Library/Caches/com.example.app/deep/file",
                "Library/Caches/com.example.app",
            ),
            (
                "Library/Developer/Xcode/DerivedData/project/Build/item",
                "Library/Developer/Xcode/DerivedData/project",
            ),
            ("Library/Logs/app/old.log", "Library/Logs/app/old.log"),
        ] {
            assert_eq!(
                library_event_scope(&root, &root.path.join(relative)),
                Some(root.path.join(expected))
            );
        }
    }

    #[test]
    fn browser_cache_profiles_are_units_and_vendor_paths_are_corridors() {
        let root = home();
        for (relative, directory, expected) in [
            ("Library/Caches/Google", true, None),
            ("Library/Caches/Google/Chrome", true, None),
            ("Library/Caches/Google/Chrome/Default", true, Some("cache")),
            (
                "Library/Caches/Google/Chrome/Profile 1",
                true,
                Some("cache"),
            ),
            ("Library/Caches/Google/Drive", true, Some("cache")),
            ("Library/Caches/Chromium", true, None),
            ("Library/Caches/Chromium/Default", true, Some("cache")),
            ("Library/Caches/Google/Chrome/Default", false, None),
            ("Library/Caches/Google/Chrome/Default/data", true, None),
            ("Library/Caches/Google/.hidden", true, None),
            ("Library/Caches/Chromium/.hidden", true, None),
            ("Library/Caches/GoogleChrome", true, Some("cache")),
        ] {
            assert_eq!(
                library_candidate(&root, &root.path.join(relative), directory),
                expected,
                "{relative}"
            );
        }
        assert_eq!(
            browser_cache_owner(&root.path.join("Library/Caches/Google/Chrome/Default")),
            Some("com.google.Chrome")
        );
        assert!(library_route_allowed(
            &root,
            &root
                .path
                .join("Library/Caches/Google/Chrome/Default/Cache/data")
        ));
        assert!(library_route_allowed(
            &root,
            &root.path.join("Library/Caches/Google/Drive/deep/item")
        ));
        assert!(!library_route_allowed(
            &root,
            &root.path.join("Library/Caches/Google/Chrome/.hidden/deep")
        ));
        assert!(!library_route_allowed(
            &root,
            &root.path.join("Library/Caches/Chromium/.hidden/deep")
        ));
        assert_eq!(
            browser_cache_owner(&root.path.join("library/caches/google/chrome/Default")),
            Some("com.google.Chrome")
        );
        assert_eq!(
            browser_cache_owner(&root.path.join("Library/Caches/Chromium/Default")),
            Some("org.chromium.Chromium")
        );
        assert_eq!(
            browser_cache_owner(&root.path.join("Library/Caches/Google/Chrome/.hidden")),
            None
        );
        assert_eq!(
            browser_cache_owner(&root.path.join("Library/Caches/GoogleChrome/Default")),
            None
        );
        let mut other = root.clone();
        other.kind = "projects".into();
        assert_eq!(
            browser_cache_owner(&other.path.join("Library/Caches/Google/Chrome/Default")),
            Some("com.google.Chrome"),
            "owner matching is lexical; the caller's Home route validation supplies authorization"
        );
        assert!(
            library_candidate(
                &other,
                &other.path.join("Library/Caches/Google/Chrome/Default"),
                true
            )
            .is_none()
        );
    }

    #[test]
    fn browser_cache_events_keep_profiles_and_unknown_google_products_separate() {
        let root = home();
        for (relative, expected) in [
            (
                "Library/Caches/Google/Chrome",
                "Library/Caches/Google/Chrome",
            ),
            (
                "Library/Caches/Google/Chrome/Default/Cache/data",
                "Library/Caches/Google/Chrome/Default",
            ),
            (
                "Library/Caches/Google/Drive/deep/item",
                "Library/Caches/Google/Drive",
            ),
            (
                "Library/Caches/Chromium/Profile 1/Cache/data",
                "Library/Caches/Chromium/Profile 1",
            ),
        ] {
            assert_eq!(
                library_event_scope(&root, &root.path.join(relative)),
                Some(root.path.join(expected)),
                "{relative}"
            );
        }
    }

    #[test]
    fn personal_discovery_ignores_source_hidden_files_and_implicit_project_scopes() {
        let mut root = home();
        assert!(personal_scope(&root, &root.path.join("Documents/Exports")));
        assert!(!personal_scope(
            &root,
            &root.path.join("Documents/.private")
        ));
        assert!(!personal_scope(&root, &root.path.join("Music")));
        for name in [
            "recording.MOV",
            "document.pDf",
            "archive.tAr.Gz",
            "Installer.DMG",
            "package.XIP",
            "package.TGZ",
            "package.tbz2",
            "package.TXZ",
            "package.zst",
        ] {
            assert!(personal_file_name(OsStr::new(name)), "{name}");
        }
        for name in [
            "source.rs",
            "view.tsx",
            "config.json",
            ".hidden.zip",
            "database.sqlite",
            "extensionless",
            "trailing.",
            "document.pdf.tmp",
            "document.ＰＤＦ",
        ] {
            assert!(!personal_file_name(OsStr::new(name)), "{name}");
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            assert!(personal_file_name(OsStr::from_bytes(b"name-\xff.PDF")));
            assert!(!personal_file_name(OsStr::from_bytes(b"name.pdf\xff")));
        }
        root.kind = "projects".into();
        assert!(!personal_scope(&root, &root.path.join("Documents")));
    }

    #[test]
    fn broader_kinds_never_inherit_permanent_eligibility() {
        for kind in [
            "cache",
            "log",
            "crashreport",
            "xcode",
            "installer",
            "archive",
            "largefile",
            "unknown",
        ] {
            assert!(!permanent_kind(kind), "{kind}");
        }
        assert!(developer_measurement("xcode"));
        assert!(!developer_measurement("cache"));
        assert_eq!(minimum_bytes("unknown"), u64::MAX);
    }
}
