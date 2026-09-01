use chippytea_core::{Engine, model::*};
use serde_json::json;
use std::{
    fs,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

fn wait(engine: &Arc<Engine>) -> Snapshot {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let snapshot = engine.snapshot().unwrap();
        if !snapshot.scanning {
            return snapshot;
        }
        assert!(Instant::now() < deadline, "Discovery failed to become idle");
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn fixture() -> (tempfile::TempDir, Arc<Engine>, Root) {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let projects = base.join("Projects");
    fs::create_dir(&projects).unwrap();
    for (name, files) in [("a", 2), ("b", 3), ("unrelated", 500)] {
        let dir = projects.join(name);
        fs::create_dir(&dir).unwrap();
        for i in 0..files {
            fs::write(dir.join(format!("file-{i}")), b"fixture").unwrap();
        }
    }
    let engine = Engine::open(&base.join("db"), None).unwrap();
    let root: Root = serde_json::from_value(
        engine
            .request(json!({"action":"authorize","path":projects,"kind":"projects"}))
            .unwrap(),
    )
    .unwrap();
    engine.request(json!({"action":"scan"})).unwrap();
    assert!(wait(&engine).stats.complete);
    (temp, engine, root)
}

#[test]
fn a_broader_explicit_grant_replaces_scopes_without_touching_files() {
    let (temp, engine, root) = fixture();
    let broader = temp.path().canonicalize().unwrap();
    assert!(
        engine
            .request(json!({"action":"authorize","path":broader,"kind":"folder"}))
            .is_err()
    );
    assert_eq!(engine.snapshot().unwrap().roots.len(), 1);
    assert_eq!(engine.snapshot().unwrap().roots[0].id, root.id);
    // Authorization changes do not mutate any of the existing fixture files.
    let content = fs::read(root.path.join("a/file-0")).unwrap();
    let new_root: Root = serde_json::from_value(
        engine
            .request(json!({
                "action":"authorize","path":broader,"kind":"folder","replace_contained":true
            }))
            .unwrap(),
    )
    .unwrap();
    let snapshot = engine.snapshot().unwrap();
    assert_eq!(snapshot.roots.len(), 1);
    assert_eq!(snapshot.roots[0].id, new_root.id);
    assert!(!snapshot.stats.complete);
    assert!(snapshot.candidates.is_empty());
    assert_eq!(fs::read(root.path.join("a/file-0")).unwrap(), content);
    assert!(engine.request(json!({"action":"authorize","path":root.path,"kind":"projects","replace_contained":true})).is_err());
    assert_eq!(engine.snapshot().unwrap().roots[0].id, new_root.id);
}

#[test]
fn sibling_events_refresh_only_their_scopes_and_ignored_churn_stays_idle() {
    let (_temp, engine, root) = fixture();
    let initial = engine.snapshot().unwrap().stats.entries;
    for path in [
        root.path.join("a/file-0"),
        root.path.join("b/file-0"),
        root.path.join("a/file-1"),
    ] {
        engine
            .request(json!({"action":"dirty","root_id":root.id,"path":path}))
            .unwrap();
    }
    let refreshed = wait(&engine);
    assert!(refreshed.stats.complete);
    assert_eq!(refreshed.stats.entries - initial, 3);
    assert!(refreshed.stats.entries - initial < initial / 10);
    let ignored=engine.request(json!({"action":"dirty","root_id":root.id,"path":root.path.join("Library/Application Support/Chippytea/library.sqlite-wal")})).unwrap();
    assert_eq!(ignored["ignored"], true);
    assert!(!engine.snapshot().unwrap().scanning);
    let noise = (0..100)
        .map(|i| root.path.join(format!("Library/Cache/item-{i}")))
        .collect::<Vec<_>>();
    let ignored = engine
        .request(json!({"action":"dirty","root_id":root.id,"paths":noise}))
        .unwrap();
    assert_eq!(ignored["ignored"], true);
    assert!(!engine.snapshot().unwrap().scanning);
}

#[test]
fn cancellation_defers_work_but_durably_received_events_can_be_acknowledged() {
    let (_temp, engine, root) = fixture();
    engine
        .request(json!({"action":"dirty","root_id":root.id,"path":root.path.join("a")}))
        .unwrap();
    let start = Instant::now();
    engine.cancel_scan();
    let cancelled = wait(&engine);
    assert!(start.elapsed() < Duration::from_millis(200));
    assert!(cancelled.stats.cancelled);
    engine
        .request(json!({"action":"dirty","root_id":root.id,"path":root.path.join("b")}))
        .unwrap();
    assert!(!engine.snapshot().unwrap().scanning);
    assert_eq!(
        engine
            .request(json!({"action":"cursor","value":99}))
            .unwrap()["cursor"],
        99
    );
    engine.request(json!({"action":"scan"})).unwrap();
    assert!(wait(&engine).stats.complete);
    assert_eq!(
        engine
            .request(json!({"action":"cursor","value":99}))
            .unwrap()["cursor"],
        99
    );
}

#[test]
fn failed_scope_remains_incomplete_until_a_full_reconciliation() {
    let (_temp, engine, root) = fixture();
    let moved = root.path.with_file_name("Moved");
    fs::rename(&root.path, &moved).unwrap();
    engine
        .request(json!({"action":"dirty","root_id":root.id,"path":root.path.join("a")}))
        .unwrap();
    let failed = wait(&engine);
    assert!(!failed.stats.complete);
    assert!(failed.error.is_some());
    assert_eq!(
        engine
            .request(json!({"action":"cursor","value":100}))
            .unwrap()["cursor"],
        100
    );
    fs::rename(&moved, &root.path).unwrap();
    let before = failed.stats.entries;
    engine
        .request(json!({"action":"dirty","root_id":root.id,"path":root.path.join("a")}))
        .unwrap();
    let partial = wait(&engine);
    assert!(
        !partial.stats.complete,
        "A narrow pass cannot repair unknown root coverage"
    );
    assert_eq!(
        partial.stats.entries - before,
        3,
        "Partial Home coverage must not widen a project event"
    );
    engine.request(json!({"action":"scan"})).unwrap();
    assert!(wait(&engine).stats.complete);
}

#[test]
fn unkeep_schedules_fresh_discovery_without_mutating_files() {
    let (_temp, engine, root) = fixture();
    let before = engine.snapshot().unwrap().stats.entries;
    engine
        .request(json!({"action":"unkeep","path":root.path.join("a")}))
        .unwrap();
    let refreshed = wait(&engine);
    assert_eq!(refreshed.stats.entries - before, 3);
    assert!(Path::new(&root.path.join("a/file-0")).exists());
}

#[test]
fn concurrent_event_submission_cannot_resume_a_cancelled_scan() {
    let (_temp, engine, root) = fixture();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let events = Arc::clone(&engine);
    let start = Arc::clone(&barrier);
    let id = root.id;
    let path = root.path.join("a");
    let submit = std::thread::spawn(move || {
        start.wait();
        events
            .request(json!({"action":"dirty","root_id":id,"path":path}))
            .unwrap();
    });
    barrier.wait();
    engine.cancel_scan();
    submit.join().unwrap();
    let state = wait(&engine);
    assert!(state.stats.cancelled);
    assert!(!state.scanning);
    assert_eq!(
        engine
            .request(json!({"action":"cursor","value":50}))
            .unwrap()["cursor"],
        50
    );
}

#[test]
fn received_scopes_survive_restart_without_repeating_the_full_scan() {
    let (temp, engine, root) = fixture();
    engine.cancel_scan();
    engine
        .request(json!({"action":"dirty","root_id":root.id,"path":root.path.join("a")}))
        .unwrap();
    engine
        .request(json!({"action":"cursor","value":101}))
        .unwrap();
    drop(engine);
    let engine = Engine::open(&temp.path().canonicalize().unwrap().join("db"), None).unwrap();
    let before = engine.snapshot().unwrap().stats.entries;
    engine.request(json!({"action":"resume"})).unwrap();
    let refreshed = wait(&engine);
    assert_eq!(refreshed.stats.entries - before, 3);
    assert_eq!(
        engine.request(json!({"action":"cursor"})).unwrap()["cursor"],
        101
    );
}

#[test]
fn commit_message_churn_stays_idle_after_cursor_acknowledgment_and_restart() {
    let (temp, engine, root) = fixture();
    let before = engine.snapshot().unwrap().stats.entries;
    let metadata = root.path.join("a/.git");
    fs::create_dir(&metadata).unwrap();
    let message = metadata.join("COMMIT_EDITMSG");
    for index in 0..20 {
        fs::write(&message, format!("Disposable commit draft {index}\n")).unwrap();
        let result = engine
            .request(json!({
                "action":"dirty", "root_id":root.id,
                "events":[{"path":message, "kind":"file", "recursive":false}]
            }))
            .unwrap();
        assert_eq!(result["ignored"], true);
        assert!(!engine.snapshot().unwrap().scanning);
    }
    engine
        .request(json!({"action":"cursor", "value":201}))
        .unwrap();
    drop(engine);

    let engine = Engine::open(&temp.path().canonicalize().unwrap().join("db"), None).unwrap();
    engine.request(json!({"action":"resume"})).unwrap();
    let after = wait(&engine);
    assert_eq!(after.stats.entries, before);
    assert!(after.stats.complete);
    assert_eq!(
        engine.request(json!({"action":"cursor"})).unwrap()["cursor"],
        201
    );
    assert_eq!(
        fs::read_to_string(message).unwrap(),
        "Disposable commit draft 19\n"
    );
}

#[test]
fn mixed_commit_message_and_artifact_events_survive_restart_with_exact_scopes() {
    fn aged_project(project: &Path) -> std::path::PathBuf {
        let artifact = project.join("node_modules");
        fs::create_dir_all(&artifact).unwrap();
        fs::write(project.join("package.json"), br#"{"name":"fixture"}"#).unwrap();
        fs::write(
            project.join("package-lock.json"),
            br#"{"lockfileVersion":3,"packages":{}}"#,
        )
        .unwrap();
        fs::write(artifact.join("payload"), b"original disposable payload").unwrap();
        let modified = std::time::SystemTime::now() - Duration::from_secs(8 * 86_400);
        for path in [
            project.join("package.json"),
            project.join("package-lock.json"),
            artifact.join("payload"),
            artifact.clone(),
        ] {
            fs::File::open(path)
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(modified))
                .unwrap();
        }
        artifact
    }

    fn indexed_candidate(db: &Path, path: &Path) -> Candidate {
        let connection =
            rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        let encoded: String = connection
            .query_row(
                "SELECT json FROM candidates WHERE path=?1",
                [path.to_str().unwrap()],
                |row| row.get(0),
            )
            .unwrap();
        serde_json::from_str(&encoded).unwrap()
    }

    let (temp, engine, root) = fixture();
    let db = temp.path().canonicalize().unwrap().join("db");
    let artifact = aged_project(&root.path.join("b"));
    engine
        .request(json!({
            "action":"dirty", "root_id":root.id,
            "events":[{"path":root.path.join("b"), "kind":"directory", "recursive":true}]
        }))
        .unwrap();
    assert!(wait(&engine).stats.complete);
    let before = indexed_candidate(&db, &artifact);

    let metadata = root.path.join("a/.git");
    fs::create_dir(&metadata).unwrap();
    let message = metadata.join("COMMIT_EDITMSG");
    fs::write(&message, b"Disposable draft\n").unwrap();
    fs::write(artifact.join("payload"), b"changed disposable payload").unwrap();
    let new_project = root.path.join("new-project");
    let new_artifact = aged_project(&new_project);
    engine.cancel_scan();
    let response = engine
        .request(json!({
            "action":"dirty", "root_id":root.id,
            "events":[
                {"path":message, "kind":"file", "recursive":false},
                {"path":artifact.join("payload"), "kind":"file", "recursive":false},
                {"path":new_project, "kind":"directory", "recursive":true}
            ]
        }))
        .unwrap();
    assert_ne!(response["ignored"], true);
    assert!(!engine.snapshot().unwrap().scanning);
    engine
        .request(json!({"action":"cursor", "value":202}))
        .unwrap();
    drop(engine);

    {
        let connection =
            rusqlite::Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        let mut query = connection
            .prepare("SELECT path FROM pending_scopes ORDER BY path")
            .unwrap();
        let pending: Vec<String> = query
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            pending,
            vec![
                artifact.to_str().unwrap().to_owned(),
                new_project.to_str().unwrap().to_owned(),
            ],
            "The message event must not queue its project or suppress real work"
        );
    }

    let engine = Engine::open(&db, None).unwrap();
    let entries = engine.snapshot().unwrap().stats.entries;
    engine.request(json!({"action":"resume"})).unwrap();
    let after = wait(&engine);
    assert!(after.stats.complete, "{after:?}");
    assert!(after.stats.entries - entries < 20);
    let changed = indexed_candidate(&db, &artifact);
    assert!(changed.modified_ns > before.modified_ns);
    assert_ne!(changed.fingerprint, before.fingerprint);
    assert_eq!(indexed_candidate(&db, &new_artifact).path, new_artifact);
    assert_eq!(
        engine.request(json!({"action":"cursor"})).unwrap()["cursor"],
        202
    );
    assert_eq!(fs::read(message).unwrap(), b"Disposable draft\n");
    assert_eq!(
        fs::read(root.path.join("unrelated/file-0")).unwrap(),
        b"fixture"
    );
}

#[test]
fn ordinary_home_files_and_removed_transients_never_restart_discovery() {
    let (_temp, engine, root) = fixture();
    let before = engine.snapshot().unwrap().stats.entries;
    let path = root.path.join(".zsh_history");
    fs::write(&path, "disposable history").unwrap();
    for _ in 0..100 {
        let result = engine.request(json!({"action":"dirty","root_id":root.id,"events":[{"path":path,"kind":"file","recursive":false}]})).unwrap();
        assert_eq!(result["ignored"], true);
    }
    fs::remove_file(&path).unwrap();
    // Already persisted legacy events also stay exact when their path is gone.
    engine
        .request(json!({"action":"dirty","root_id":root.id,"path":path}))
        .unwrap();
    let after = wait(&engine);
    assert!(after.stats.complete, "{after:?}");
    assert_eq!(after.stats.entries, before);
    assert!(after.error.is_none());
}

#[test]
fn a_removed_subtree_reconciles_without_reading_its_siblings() {
    let (_temp, engine, root) = fixture();
    let before = engine.snapshot().unwrap().stats.entries;
    fs::remove_dir_all(root.path.join("a")).unwrap(); // disposable fixture only
    engine.request(json!({"action":"dirty","root_id":root.id,"events":[{"path":root.path.join("a"),"kind":"directory","recursive":true}]})).unwrap();
    let after = wait(&engine);
    assert!(after.stats.complete);
    assert_eq!(after.stats.entries, before);
    assert_eq!(fs::read(root.path.join("b/file-0")).unwrap(), b"fixture");
}

#[test]
fn a_substituted_scope_parent_cannot_be_followed_for_absence_checks() {
    let (temp, engine, root) = fixture();
    let outside = temp.path().canonicalize().unwrap().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("preserve"), "outside grant").unwrap();
    fs::rename(root.path.join("a"), root.path.join("old-a")).unwrap();
    std::os::unix::fs::symlink(&outside, root.path.join("a")).unwrap();
    engine
        .request(json!({"action":"dirty","root_id":root.id,"path":root.path.join("a/missing")}))
        .unwrap();
    let after = wait(&engine);
    assert!(!after.stats.complete);
    assert!(after.error.is_some());
    assert_eq!(
        fs::read_to_string(outside.join("preserve")).unwrap(),
        "outside grant"
    );
}
