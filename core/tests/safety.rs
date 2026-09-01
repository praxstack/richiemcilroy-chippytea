use chippytea_core::{Engine, model::*, safety, scanner};
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
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
