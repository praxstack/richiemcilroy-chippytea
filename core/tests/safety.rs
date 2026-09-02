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
