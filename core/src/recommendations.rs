//! Shared, lexical recommendation policy. Recognizing a location never grants
//! filesystem access or deletion permission; the scanner and cleanup still
//! validate the original grant, identities and complete measurements.
use crate::model::Root;
use std::ffi::OsStr;
use std::fmt::Write;
use std::path::{Path, PathBuf};

pub(crate) const HOME_LIBRARY_ROUTES: [&str; 3] = [
    "Library/Caches",
    "Library/Logs",
    "Library/Developer/Xcode/DerivedData",
];

pub(crate) fn permanent_kind(kind: &str) -> bool {
    matches!(kind, "cargo" | "node" | "venv" | "webcache")
}

pub(crate) fn review_project_kind(kind: &str) -> bool {
    matches!(
        kind,
        "swiftpm" | "dotnet" | "gradle" | "dart" | "flutter" | "zig"
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
        "cache" | "archive" => 50_000_000,
        "log" => 10_000_000,
        "crashreport" => 1_000_000,
        "installer" => 20_000_000,
        "xcode" => 250_000_000,
        "largefile" => 500_000_000,
        "cargo" | "node" | "venv" | "webcache" | "download" => 100_000_000,
        kind if review_project_kind(kind) => 100_000_000,
        _ => u64::MAX,
    }
}

pub(crate) fn quiet_days(kind: &str) -> i64 {
    match kind {
        "cargo" | "node" | "venv" | "webcache" => 7,
        kind if review_project_kind(kind) => 7,
        "installer" | "xcode" => 14,
        "largefile" => 90,
        _ => 30,
    }
}

const KINDS: [&str; 18] = [
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
        WHEN 'cache' THEN 1 WHEN 'webcache' THEN 1 WHEN 'dart' THEN 1 \
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
}

/// Only an explicit Home grant opts into these routes. A project containing a
/// folder named Library is not an application-data authorization.
pub(crate) fn library_area<'a>(root: &Root, path: &'a Path) -> Option<(LibraryArea, &'a Path)> {
    if root.kind != "home" {
        return None;
    }
    let relative = path.strip_prefix(&root.path).ok()?;
    for (route, area) in HOME_LIBRARY_ROUTES.into_iter().zip([
        LibraryArea::Caches,
        LibraryArea::Logs,
        LibraryArea::Xcode,
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
                Some("Library" | "Library/Developer" | "Library/Developer/Xcode")
            )
        })
}

/// Package caches need their manager's own preview/prune protocol. Do not
/// reinterpret a familiar manager's opaque store as an ordinary app cache.
pub(crate) fn managed_cache(name: &OsStr) -> bool {
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

pub(crate) fn library_route_allowed(root: &Root, path: &Path) -> bool {
    if library_corridor(root, path) {
        return true;
    }
    library_area(root, path).is_some_and(|(area, suffix)| {
        if area != LibraryArea::Caches {
            return true;
        }
        let mut components = suffix.components();
        let Some(first) = components.next().map(|component| component.as_os_str()) else {
            return true;
        };
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

/// Return the bundle identifier for a recognized browser profile cache. The
/// caller must have already validated this as a Home Library/Caches location;
/// the suffix match here only prevents an unrelated cache from being treated
/// as browser-owned activity.
pub(crate) fn browser_cache_owner(location: &Path) -> Option<&'static str> {
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
        _ => None,
    }
}

/// Changes inside a cache/build unit invalidate that exact unit. Logs stay
/// file-scoped, so a logging app cannot continually rescan all user logs.
pub(crate) fn library_event_scope(root: &Root, path: &Path) -> Option<PathBuf> {
    let (area, suffix) = library_area(root, path)?;
    if !library_route_allowed(root, path) {
        return None;
    }
    if area == LibraryArea::Logs || suffix.as_os_str().is_empty() {
        return Some(path.to_path_buf());
    }
    let route = match area {
        LibraryArea::Caches => HOME_LIBRARY_ROUTES[0],
        LibraryArea::Xcode => HOME_LIBRARY_ROUTES[2],
        LibraryArea::Logs => unreachable!(),
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
            "Library/Caches/uv/item",
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
