use chippytea_core::{Engine, cleanup, model::*, safety, scanner, store::Store};
use serde_json::json;
use std::{
    fs,
    io::Write,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::{Duration, SystemTime},
};

fn project(root: &Path) -> PathBuf {
    let project = root.join("sample");
    fs::create_dir_all(project.join("target/debug")).unwrap();
    fs::write(
        project.join("Cargo.toml"),
        "[package]\nname=\"sample\"\nversion=\"0.1.0\"\n",
    )
    .unwrap();
    fs::write(
        project.join("target/CACHEDIR.TAG"),
        "Signature: 8a477f597d28d172789f06886806bc55\n",
    )
    .unwrap();
    fs::write(project.join("target/debug/binary"), vec![42u8; 16384]).unwrap();
    project
}
fn candidate(root: &Path) -> (Root, Candidate) {
    let root = safety::authorize(root, "projects").unwrap();
    let mut candidates = Vec::new();
    scanner::scan_with_checkpoint_mode(
        &root,
        None,
        &[],
        &AtomicBool::new(false),
        scanner::ScanMode::MetadataCoverage,
        || {},
        |b| {
            candidates.extend(
                b.candidates
                    .into_iter()
                    .filter(|c| c.blocked_reason.is_none()),
            )
        },
    )
    .unwrap();
    let c = candidates
        .into_iter()
        .find(|c| c.kind == "cargo")
        .expect("eligible Cargo artifact");
    (root, c)
}
#[test]
fn changed_descendant_is_never_mutated() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let p = project(&root);
    let (r, c) = candidate(&root);
    fs::write(p.join("target/debug/new-work"), "preserve this").unwrap();
    assert!(scanner::revalidate(&r, &c, &AtomicBool::new(false)).is_err());
    assert_eq!(
        fs::read_to_string(p.join("target/debug/new-work")).unwrap(),
        "preserve this"
    );
}
#[test]
fn replacement_and_symlink_do_not_escape_authorization() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let p = project(&root);
    let (r, c) = candidate(&root);
    fs::rename(p.join("target"), p.join("old-target")).unwrap();
    std::os::unix::fs::symlink(p.join("old-target"), p.join("target")).unwrap();
    assert!(scanner::revalidate(&r, &c, &AtomicBool::new(false)).is_err());
    assert!(p.join("old-target/debug/binary").exists());
}
#[test]
fn engine_library_excludes_a_second_writer() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("db");
    let first = Engine::open(&db, None).unwrap();
    assert!(Engine::open(&db, None).is_err());
    drop(first);
    assert!(Engine::open(&db, None).is_ok());
}
#[test]
fn no_confirmation_no_mutation_and_no_rewards_for_scans() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let p = project(&root);
    let engine = Engine::open(&root.join("db"), None).unwrap();
    engine
        .request(json!({"action":"authorize","path":root,"kind":"projects"}))
        .unwrap();
    assert!(
        engine
            .request(json!({"action":"execute","token":"unknown","confirmed":false}))
            .is_err()
    );
    assert!(p.join("target/debug/binary").exists());
    assert_eq!(engine.snapshot().unwrap().wallet.credited_bytes, 0);
}

fn home_report() -> (tempfile::TempDir, Root, Candidate) {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap().join("Home");
    let path = home.join("Library/Logs/DiagnosticReports/disposable.ips");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut file = fs::File::create(&path).unwrap();
    let block = [0x52; 64 * 1024];
    for _ in 0..16 {
        file.write_all(&block).unwrap();
    }
    file.sync_all().unwrap();
    assert!(file.metadata().unwrap().blocks() * 512 >= 1_000_000);
    file.set_times(
        fs::FileTimes::new().set_modified(SystemTime::now() - Duration::from_secs(31 * 86_400)),
    )
    .unwrap();
    drop(file);
    let root = safety::authorize(&home, "home").unwrap();
    let mut rows = std::collections::BTreeMap::new();
    let stats = scanner::scan(&root, None, &AtomicBool::new(false), |batch| {
        for row in batch.candidates {
            rows.insert(row.id.clone(), row);
        }
    })
    .unwrap();
    assert!(stats.complete, "{stats:?}");
    let report = rows
        .into_values()
        .find(|candidate| candidate.path == path && candidate.suggestion_eligible)
        .expect("An allocated, old diagnostic report must be offered for review");
    assert_eq!(report.kind, "crashreport");
    assert!(!report.eligible_permanent);
    scanner::revalidate(&root, &report, &AtomicBool::new(false)).unwrap();
    (temp, root, report)
}

#[test]
fn changed_home_report_is_rejected_without_mutating_its_new_contents() {
    let (_temp, root, report) = home_report();
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(&report.path)
        .unwrap();
    file.write_all(b"new diagnostic data").unwrap();
    file.sync_all().unwrap();
    assert!(scanner::revalidate(&root, &report, &AtomicBool::new(false)).is_err());
    assert!(
        fs::read(&report.path)
            .unwrap()
            .ends_with(b"new diagnostic data")
    );
}

#[test]
fn replaced_or_redirected_home_report_cannot_reuse_a_review() {
    let (_temp, root, report) = home_report();
    let preserved = report.path.with_file_name("preserved.ips");
    let original_modified = fs::metadata(&report.path).unwrap().modified().unwrap();
    fs::rename(&report.path, &preserved).unwrap();
    let replacement = fs::File::create(&report.path).unwrap();
    replacement.set_len(report.logical_bytes).unwrap();
    replacement
        .set_times(fs::FileTimes::new().set_modified(original_modified))
        .unwrap();
    drop(replacement);
    assert!(scanner::revalidate(&root, &report, &AtomicBool::new(false)).is_err());
    fs::remove_file(&report.path).unwrap(); // Only the disposable replacement.
    std::os::unix::fs::symlink(&preserved, &report.path).unwrap();
    assert!(scanner::revalidate(&root, &report, &AtomicBool::new(false)).is_err());
    assert_eq!(fs::read(&preserved).unwrap(), vec![0x52; 1024 * 1024]);
}

#[test]
fn everyday_kinds_reject_a_forged_permanent_flag_before_any_cleanup() {
    let (temp, root, report) = home_report();
    let mut forged = report.clone();
    forged.eligible_permanent = true;
    let error = scanner::revalidate(&root, &forged, &AtomicBool::new(false)).unwrap_err();
    assert!(error.contains("Trash-only"), "{error}");

    let mut store = Store::open(&temp.path().join("cleanup-db")).unwrap();
    // The operation boundary must enforce its kind allowlist independently of
    // a persisted or client-provided eligibility flag, including larger kinds
    // whose threshold need not be allocated again to exercise this gate.
    for kind in [
        "cache",
        "log",
        "crashreport",
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
    ] {
        forged.kind = kind.into();
        let error = cleanup::execute(
            &mut store,
            &root,
            &forged,
            "permanent",
            None,
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(
            error.contains("Permanent cleanup is restricted"),
            "{kind}: {error}"
        );
    }
    assert!(store.history().unwrap().is_empty());
    assert_eq!(store.wallet().unwrap().credited_bytes, 0);
    assert_eq!(fs::read(&report.path).unwrap(), vec![0x52; 1024 * 1024]);
}

#[test]
fn library_cleanup_requires_the_original_explicit_home_grant() {
    let (_temp, root, report) = home_report();
    for kind in ["projects", "folder", "downloads"] {
        let changed = safety::authorize(&root.path, kind).unwrap();
        assert!(scanner::revalidate(&changed, &report, &AtomicBool::new(false)).is_err());
        let mut suggestions = Vec::new();
        let stats = scanner::scan(&changed, None, &AtomicBool::new(false), |batch| {
            suggestions.extend(batch.candidates);
        })
        .unwrap();
        assert!(stats.complete, "{kind}: {stats:?}");
        assert!(suggestions.is_empty(), "{kind}: {suggestions:?}");
    }
    assert_eq!(fs::read(&report.path).unwrap(), vec![0x52; 1024 * 1024]);
}

// This callback moves only a generated test stage to a sibling fixture path.
// It never calls Finder or the user's actual Trash.
unsafe extern "C" fn fixture_project_trash(
    source: *const libc::c_char,
    output: *mut libc::c_char,
    capacity: usize,
) -> libc::c_int {
    let result = (|| -> std::result::Result<(), ()> {
        let source = unsafe { std::ffi::CStr::from_ptr(source) }
            .to_str()
            .map_err(|_| ())?;
        let source = Path::new(source);
        if source
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            != Some("disposable-provider-project")
            || !source
                .file_name()
                .is_some_and(|name| name.as_encoded_bytes().starts_with(b".chippytea-"))
        {
            return Err(());
        }
        let destination = source.with_file_name("fixture-trash-item");
        let bytes = destination.as_os_str().as_encoded_bytes();
        if bytes.len() + 1 > capacity || output.is_null() {
            return Err(());
        }
        fs::rename(source, &destination).map_err(|_| ())?;
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), output.cast(), bytes.len());
            *output.add(bytes.len()) = 0;
        }
        Ok(())
    })();
    if result.is_ok() { 0 } else { 1 }
}

#[test]
fn project_ecosystems_are_verified_reversible_and_never_coin_eligible() {
    let cases = [
        (
            "swiftpm",
            ".build",
            "Package.swift",
            "// swift-tools-version: 6.0\nimport PackageDescription\nlet package = Package(name: \"fixture\")\n",
        ),
        (
            "dotnet",
            "obj",
            "Fixture.csproj",
            "<Project Sdk=\"Microsoft.NET.Sdk\"></Project>\n",
        ),
        ("gradle", "build", "build.gradle", "plugins { id 'java' }\n"),
        (
            "dart",
            ".dart_tool",
            "pubspec.yaml",
            "name: fixture\nenvironment:\n  sdk: '>=3.0.0'\n",
        ),
        (
            "flutter",
            "build",
            "pubspec.yaml",
            "name: fixture\ndependencies:\n  flutter:\n    sdk: flutter\n",
        ),
        (
            "zig",
            ".zig-cache",
            "build.zig",
            "const std = @import(\"std\");\npub fn build(b: *std.Build) void {}\n",
        ),
    ];
    for (kind, output, manifest, contents) in cases {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let projects = base.join("Projects");
        let project = projects.join("disposable-provider-project");
        let artifact = project.join(output);
        fs::create_dir_all(&artifact).unwrap();
        fs::write(project.join(manifest), contents).unwrap();
        let payload = artifact.join("generated-output");
        let mut file = fs::File::create(&payload).unwrap();
        let block = [0x53; 1024 * 1024];
        for _ in 0..100 {
            file.write_all(&block).unwrap();
        }
        file.sync_all().unwrap();
        assert!(file.metadata().unwrap().blocks() * 512 >= 100_000_000);
        drop(file);
        let old = SystemTime::now() - Duration::from_secs(31 * 86_400);
        for path in [&payload, &project.join(manifest), &artifact, &project] {
            fs::File::open(path)
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(old))
                .unwrap();
        }
        let root = safety::authorize(&projects, "projects").unwrap();
        let mut rows = std::collections::BTreeMap::new();
        let stats = scanner::scan(&root, None, &AtomicBool::new(false), |batch| {
            for candidate in batch.candidates {
                rows.insert(candidate.id.clone(), candidate);
            }
        })
        .unwrap();
        assert!(stats.complete && stats.errors == 0, "{kind}: {stats:?}");
        let candidate = rows
            .into_values()
            .find(|candidate| candidate.path == artifact)
            .unwrap_or_else(|| panic!("Missing verified {kind} fixture"));
        assert_eq!(candidate.kind, kind);
        assert!(
            candidate.suggestion_eligible && !candidate.provisional,
            "{kind}: {candidate:?}"
        );
        assert!(!candidate.eligible_permanent);
        scanner::revalidate(&root, &candidate, &AtomicBool::new(false)).unwrap();
        let mut store = Store::open(&base.join("fixture-ledger.sqlite")).unwrap();
        store.add_root(&root).unwrap();
        let receipt = cleanup::execute(
            &mut store,
            &root,
            &candidate,
            "trash",
            Some(fixture_project_trash),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(!artifact.exists());
        assert_eq!((receipt.credited_bytes, receipt.coins), (0, 0));
        assert_eq!(
            fs::read_to_string(project.join(manifest)).unwrap(),
            contents
        );
        cleanup::restore(&mut store, &receipt.id, &AtomicBool::new(false)).unwrap();
        assert_eq!(fs::metadata(&payload).unwrap().len(), 100 * 1024 * 1024);
        assert_eq!(store.wallet().unwrap().credited_bytes, 0);
        // Rename/restore changes identity metadata. Establish a *fresh* valid
        // review, so the following rejection really tests owner evidence.
        let mut restored = None;
        scanner::scan(&root, None, &AtomicBool::new(false), |batch| {
            for row in batch.candidates {
                if row.path == artifact && !row.provisional {
                    restored = Some(row);
                }
            }
        })
        .unwrap();
        let restored = restored.expect("restored artifact can be reviewed again");
        scanner::revalidate(&root, &restored, &AtomicBool::new(false)).unwrap();
        // An accepted review cannot survive ownership/configuration replacement.
        fs::write(project.join(manifest), "new user configuration").unwrap();
        assert!(scanner::revalidate(&root, &restored, &AtomicBool::new(false)).is_err());
        assert!(payload.exists());
    }
}
