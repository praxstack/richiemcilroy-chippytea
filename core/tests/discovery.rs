use chippytea_core::{Engine, model::*};
use serde_json::json;
use std::{
    fs,
    io::Write,
    os::unix::fs::MetadataExt,
    path::Path,
    sync::Arc,
    time::{Duration, Instant, SystemTime},
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
    let ignored=engine.request(json!({"action":"dirty","root_id":root.id,"path":root.path.join("Library/Application Support/chippytea/library.sqlite-wal")})).unwrap();
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

fn allocated_file(path: &Path, bytes: u64) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut file = fs::File::create(path).unwrap();
    let block = [0x63; 64 * 1024];
    let mut remaining = bytes;
    while remaining > 0 {
        let count = remaining.min(block.len() as u64) as usize;
        file.write_all(&block[..count]).unwrap();
        remaining -= count as u64;
    }
    file.sync_all().unwrap();
    assert!(file.metadata().unwrap().blocks() * 512 >= bytes);
}

fn age_fixture_tree(path: &Path, days: u64) {
    if path.is_dir() {
        for child in fs::read_dir(path).unwrap() {
            age_fixture_tree(&child.unwrap().path(), days);
        }
    }
    fs::File::open(path)
        .unwrap()
        .set_times(
            fs::FileTimes::new()
                .set_modified(SystemTime::now() - Duration::from_secs(days * 86_400)),
        )
        .unwrap();
}

#[test]
fn explicit_duplicate_check_verifies_contents_includes_kept_copies_and_expires_on_events() {
    use std::io::{Seek, SeekFrom};

    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let downloads = base.join("Downloads");
    let size = 21_000_000;
    let paths = ["keeper.dmg", "copy.dmg", "different.dmg"].map(|name| downloads.join(name));
    for path in &paths {
        allocated_file(path, size);
    }
    // Preserve the first, middle and last sample while changing real content.
    let mut different = fs::OpenOptions::new().write(true).open(&paths[2]).unwrap();
    different.seek(SeekFrom::Start(2 * 64 * 1024)).unwrap();
    different.write_all(b"not actually equal").unwrap();
    different.sync_all().unwrap();
    drop(different);
    age_fixture_tree(&downloads, 15);
    let engine = Engine::open(&base.join("library.sqlite"), None).unwrap();
    let root: Root = serde_json::from_value(
        engine
            .request(json!({
                "action":"authorize", "path":downloads, "kind":"downloads"
            }))
            .unwrap(),
    )
    .unwrap();
    engine.request(json!({"action":"scan"})).unwrap();
    let initial = wait(&engine);
    assert!(initial.stats.complete, "{initial:?}");
    assert_eq!(initial.candidates.len(), 3);
    assert!(
        initial
            .candidates
            .iter()
            .all(|file| file.kind == "installer")
    );
    assert!(
        engine
            .request(json!({"action":"duplicate_progress"}))
            .unwrap()
            .is_null()
    );
    let keeper = initial
        .candidates
        .iter()
        .find(|file| file.path == paths[0])
        .unwrap();
    let copy = initial
        .candidates
        .iter()
        .find(|file| file.path == paths[1])
        .unwrap();
    engine
        .request(json!({"action":"keep", "id":keeper.id}))
        .unwrap();
    assert_eq!(engine.snapshot().unwrap().candidates.len(), 2);
    let report = engine
        .request(json!({"action":"check_duplicates"}))
        .unwrap();
    assert_eq!(report["indexed_files"], 3);
    assert_eq!(report["progress"]["complete"], true, "{report}");
    assert_eq!(report["progress"]["files_compared"], 3);
    assert_eq!(report["progress"]["bytes_read"], 5 * size + 9 * 64 * 1024);
    let groups = report["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 1, "{report}");
    let files = groups[0]["files"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    assert!(
        files
            .iter()
            .any(|file| file["candidate"]["id"] == keeper.id && file["keeper_only"] == true)
    );
    assert!(
        files
            .iter()
            .any(|file| file["candidate"]["id"] == copy.id && file["keeper_only"] == false)
    );
    let request = json!({"action":"prepare_duplicate", "operation":"trash",
        "report_token":report["token"], "group_id":groups[0]["id"],
        "keeper_id":keeper.id, "copy_id":copy.id});
    let mut permanent = request.clone();
    permanent["operation"] = json!("permanent");
    assert!(
        engine
            .request(permanent)
            .unwrap_err()
            .contains("Trash-only")
    );
    let mut reversed = request.clone();
    reversed["keeper_id"] = json!(copy.id);
    reversed["copy_id"] = json!(keeper.id);
    assert!(
        engine.request(reversed).is_err(),
        "Keep is not permission to remove the keeper"
    );
    assert!(engine.request(request.clone()).unwrap()["token"].is_string());
    // A kept file may be filtered from normal event refresh. Its raw event must
    // still invalidate the content report before another review can be prepared.
    engine
        .request(json!({"action":"dirty", "root_id":root.id, "path":keeper.path}))
        .unwrap();
    assert!(engine.request(request).unwrap_err().contains("expired"));
    let final_snapshot = wait(&engine);
    assert!(final_snapshot.history.is_empty());
    assert_eq!(final_snapshot.wallet.credited_bytes, 0);
    assert_eq!(final_snapshot.kept_paths, [keeper.path.to_str().unwrap()]);
    for path in &paths {
        assert_eq!(fs::metadata(path).unwrap().len(), size);
    }
}

#[test]
fn home_everyday_recommendations_are_scoped_freshness_checked_and_trash_only() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let home = base.join("Home");
    let cache = home.join("Library/Caches/com.example.fixture");
    let cached_payload = cache.join("nested/payload");
    let log = home.join("Library/Logs/fixture/rotated.log");
    let report = home.join("Library/Logs/DiagnosticReports/fixture.ips");
    let installer = home.join("Downloads/fixture.DMG");
    for (path, bytes) in [
        (&cached_payload, 51_000_000),
        (&log, 11_000_000),
        (&report, 1_100_000),
        (&installer, 21_000_000),
    ] {
        allocated_file(path, bytes);
    }
    let protected = [
        "Library/Application Support/fixture/state",
        "Library/Keychains/preserve",
        "Library/Containers/fixture/Data/Library/Caches/preserve",
        "Library/Developer/Xcode/Archives/preserve",
        "Library/Developer/CoreSimulator/preserve",
        "Library/Caches/uv/preserve",
        "Library/Caches/Homebrew/preserve",
    ]
    .map(|relative| home.join(relative));
    for path in &protected {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"preserve unrelated application data").unwrap();
    }
    age_fixture_tree(&home, 31);
    // Installers use their own 14-day policy, not the generic 30-day one.
    age_fixture_tree(&installer, 15);
    let current_log = log.with_file_name("current.log");
    fs::write(&current_log, b"currently logging").unwrap();

    let db = base.join("db");
    let engine = Engine::open(&db, None).unwrap();
    let root: Root = serde_json::from_value(
        engine
            .request(json!({"action":"authorize", "path":home, "kind":"home"}))
            .unwrap(),
    )
    .unwrap();
    engine
        .request(json!({"action":"scan", "metadata_coverage":true}))
        .unwrap();
    let initial = wait(&engine);
    assert!(initial.stats.complete, "{initial:?}");
    let cached_review = indexed_candidate(&db, &cache);
    assert_eq!(cached_review.kind, "cache");
    assert!(cached_review.allocated_bytes >= 51_000_000);
    assert!(!cached_review.provisional && !cached_review.eligible_permanent);
    let cache_offered = cached_review.suggestion_eligible;
    if !cache_offered {
        // Some macOS hosts contain a live process whose executable cannot be
        // inspected. Keep that production fail-closed policy, but still prove
        // cache recognition and measurement; no other exclusion is accepted.
        let activity_unavailable = if cfg!(target_os = "macos") {
            "A running executable could not be identified; cleanup is withheld"
        } else {
            "Reliable activity checks are supported only by the native macOS engine"
        };
        assert_eq!(
            cached_review.blocked_reason.as_deref(),
            Some(activity_unavailable)
        );
    }
    assert_eq!(
        initial.candidates.len(),
        3 + usize::from(cache_offered),
        "{initial:?}"
    );
    for (path, kind) in [
        (&cache, "cache"),
        (&log, "log"),
        (&report, "crashreport"),
        (&installer, "installer"),
    ] {
        if kind == "cache" && !cache_offered {
            continue;
        }
        let candidate = initial
            .candidates
            .iter()
            .find(|candidate| &candidate.path == path)
            .unwrap_or_else(|| panic!("Missing {kind} recommendation at {path:?}"));
        assert_eq!(candidate.kind, kind);
        assert!(candidate.suggestion_eligible && !candidate.provisional);
        assert!(!candidate.eligible_permanent);
        assert!(
            engine
                .request(json!({"action":"prepare", "operation":"trash", "items":[candidate]}))
                .unwrap()["token"]
                .as_str()
                .is_some()
        );
        assert!(
            engine
                .request(json!({"action":"prepare", "operation":"permanent", "items":[candidate]}))
                .is_err()
        );
    }
    assert_eq!(initial.wallet.credited_bytes, 0);
    assert!(initial.history.is_empty());

    if cache_offered {
        chippytea_core::scanner::revalidate(
            &root,
            &cached_review,
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap();
    }
    let cache_modified = fs::metadata(&cache).unwrap().modified().unwrap();
    let active_payload = cache.join("nested/current");
    fs::write(&active_payload, b"new work must remain").unwrap();
    assert_eq!(
        fs::metadata(&cache).unwrap().modified().unwrap(),
        cache_modified,
        "A fresh descendant must invalidate even an unchanged cache boundary"
    );
    assert!(
        chippytea_core::scanner::revalidate(
            &root,
            &cached_review,
            &std::sync::atomic::AtomicBool::new(false),
        )
        .is_err()
    );
    age_fixture_tree(&installer, 0);
    engine
        .request(json!({
            "action":"dirty", "root_id":root.id,
            "events":[
                {"path":active_payload, "kind":"file", "recursive":false},
                {"path":installer, "kind":"file", "recursive":false},
                {"path":current_log, "kind":"file", "recursive":false}
            ]
        }))
        .unwrap();
    let refreshed = wait(&engine);
    assert!(refreshed.stats.complete, "{refreshed:?}");
    assert_eq!(refreshed.candidates.len(), 2, "{refreshed:?}");
    assert_eq!(
        (
            refreshed.stats.entries - initial.stats.entries,
            refreshed.stats.files - initial.stats.files,
            refreshed.stats.directories - initial.stats.directories,
        ),
        (6, 4, 2),
        "Refresh must visit only the four-entry cache unit and two changed files"
    );
    assert!(refreshed.candidates.iter().any(|row| row.path == log));
    assert!(refreshed.candidates.iter().any(|row| row.path == report));
    let changed_cache = indexed_candidate(&db, &cache);
    assert!(changed_cache.modified_ns > cached_review.modified_ns);
    assert!(
        changed_cache.explanation.contains("30 quiet days"),
        "Freshness must independently exclude the measured cache: {changed_cache:?}"
    );
    assert_eq!(fs::read(&active_payload).unwrap(), b"new work must remain");
    assert_eq!(fs::metadata(&installer).unwrap().len(), 21_000_000);

    let ignored = engine
        .request(json!({
            "action":"dirty", "root_id":root.id,
            "events":protected.iter().map(|path| json!({
                "path":path, "kind":"file", "recursive":false
            })).collect::<Vec<_>>()
        }))
        .unwrap();
    assert_eq!(ignored["ignored"], true);
    let after_ignored = engine.snapshot().unwrap();
    assert!(!after_ignored.scanning);
    assert_eq!(after_ignored.stats.entries, refreshed.stats.entries);
    for path in &protected {
        assert_eq!(
            fs::read(path).unwrap(),
            b"preserve unrelated application data"
        );
    }
    assert_eq!(after_ignored.wallet.credited_bytes, 0);
    assert!(after_ignored.history.is_empty());
}

#[cfg(target_os = "macos")]
#[test]
fn live_cache_owner_blocks_discovery_and_a_prepared_cleanup_until_exit() {
    use std::{
        os::unix::ffi::OsStrExt,
        process::{Child, Command, Stdio},
        sync::atomic::AtomicBool,
    };

    // Only this test's child may be stopped. An unreaped owned child cannot
    // have its PID reused; try_wait also avoids signalling an already reaped PID.
    struct OwnedChild(Child);
    impl OwnedChild {
        fn stop(&mut self) -> std::io::Result<()> {
            if self.0.try_wait()?.is_some() {
                return Ok(());
            }
            self.0.kill()?;
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if self.0.try_wait()?.is_some() {
                    return Ok(());
                }
                if Instant::now() >= deadline {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "The owned cache-owner fixture did not exit",
                    ));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            let _ = self.stop();
        }
    }

    const OWNER_RUNNING: &str =
        "The app that owns this cache is running; close it before reviewing cleanup";
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let home = base.join("Home");
    let bundle_id = format!("com.example.chippytea.cache-owner.{}", unique_id());
    let cache = home.join("Library/Caches").join(&bundle_id);
    let payload = cache.join("nested/payload");
    allocated_file(&payload, 51_000_000);
    age_fixture_tree(&home, 31);
    let payload_identity = chippytea_core::safety::identity(&payload).unwrap();
    let payload_digest = || {
        let mut hasher = blake3::Hasher::new();
        hasher
            .update_reader(fs::File::open(&payload).unwrap())
            .unwrap();
        hasher.finalize()
    };
    let original_digest = payload_digest();

    // Copy the existing system executable; do not compile, open, register or
    // launch a user's app. Both its executable and cwd stay outside the cache,
    // so only the exact bundle-ID rule can supply OWNER_RUNNING.
    let app = base.join("Owner.app");
    let executable = app.join("Contents/MacOS/fixture-sleep");
    fs::create_dir_all(executable.parent().unwrap()).unwrap();
    fs::copy("/bin/sleep", &executable).unwrap();
    fs::write(
        app.join("Contents/Info.plist"),
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <plist version=\"1.0\"><dict>\
             <key>CFBundlePackageType</key><string>APPL</string>\
             <key>CFBundleExecutable</key><string>fixture-sleep</string>\
             <key>CFBundleIdentifier</key><string>{bundle_id}</string>\
             </dict></plist>"
        ),
    )
    .unwrap();

    let db = base.join("db");
    // There is deliberately no Trash callback. Even if the activity guard
    // regresses, this test cannot move anything into the user's native Trash.
    let engine = Engine::open(&db, None).unwrap();
    let root: Root = serde_json::from_value(
        engine
            .request(json!({"action":"authorize", "path":home, "kind":"home"}))
            .unwrap(),
    )
    .unwrap();
    engine.request(json!({"action":"scan"})).unwrap();
    let idle = wait(&engine);
    assert!(idle.stats.complete && idle.error.is_none(), "{idle:?}");
    assert_eq!(idle.candidates.len(), 1, "{idle:?}");
    let candidate = &idle.candidates[0];
    assert_eq!(candidate.path, cache);
    assert_eq!(candidate.kind, "cache");
    assert!(candidate.suggestion_eligible && !candidate.provisional);
    assert!(candidate.blocked_reason.is_none() && !candidate.eligible_permanent);
    assert!(candidate.allocated_bytes >= 51_000_000);
    chippytea_core::scanner::revalidate(&root, candidate, &AtomicBool::new(false)).unwrap();
    let prepared = engine
        .request(json!({"action":"prepare", "operation":"trash", "items":[candidate]}))
        .unwrap();
    assert!(prepared["token"].is_string());
    assert!(idle.history.is_empty());
    let wallet = serde_json::to_value(&idle.wallet).unwrap();

    // The finite duration is a fallback if the entire test process aborts and
    // cannot run Drop. Normal completion and panic unwinding stop/reap it early.
    let mut child = OwnedChild(
        Command::new(&executable)
            .arg("60")
            .current_dir(&base)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "The owned cache-owner fixture exited before inspection"
        );
        let mut path = [0u8; 4096];
        let length = unsafe {
            libc::proc_pidpath(
                child.0.id() as libc::pid_t,
                path.as_mut_ptr().cast(),
                path.len() as u32,
            )
        };
        if length > 0 {
            let end = path.iter().position(|byte| *byte == 0).unwrap();
            if &path[..end] == executable.as_os_str().as_bytes() {
                break;
            }
        }
        assert!(
            Instant::now() < deadline,
            "The owned fixture's executable path was not observable"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    // Do not refresh the index before executing the old token: startup after
    // prepare must be caught by fresh mutation-time activity evidence itself.
    assert_eq!(
        chippytea_core::scanner::revalidate(&root, candidate, &AtomicBool::new(false)).unwrap_err(),
        OWNER_RUNNING
    );
    let receipts: Vec<Receipt> = serde_json::from_value(
        engine
            .request(json!({
                "action":"execute", "token":prepared["token"], "confirmed":true
            }))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(receipts.len(), 1);
    let receipt = &receipts[0];
    assert_eq!(receipt.outcome, "skipped");
    assert!(receipt.detail.contains("before staging"), "{receipt:?}");
    assert!(receipt.detail.contains(OWNER_RUNNING), "{receipt:?}");
    assert_eq!(
        (
            receipt.observed_bytes,
            receipt.credited_bytes,
            receipt.coins
        ),
        (0, 0, 0)
    );
    assert!(receipt.trash_path.is_none() && !receipt.can_restore);
    assert_eq!(
        chippytea_core::safety::identity(&cache).unwrap(),
        candidate.identity
    );

    // Mutation schedules a fresh scoped scan. Also request an ordinary scan
    // explicitly, so positive/negative discovery uses the production mode.
    wait(&engine);
    engine.request(json!({"action":"scan"})).unwrap();
    let running = wait(&engine);
    assert!(
        running.stats.complete && running.error.is_none(),
        "{running:?}"
    );
    assert!(running.candidates.is_empty(), "{running:?}");
    let blocked = indexed_candidate(&db, &cache);
    assert_eq!(blocked.blocked_reason.as_deref(), Some(OWNER_RUNNING));
    assert!(!blocked.suggestion_eligible);
    assert!(
        engine
            .request(json!({"action":"prepare", "operation":"trash", "items":[blocked]}))
            .unwrap_err()
            .contains("ineligible")
    );
    // One refused execution is an audit receipt, not a removal or reward.
    assert_eq!(running.history.len(), 1);
    assert_eq!(running.history[0].id, receipt.id);
    assert_eq!(serde_json::to_value(&running.wallet).unwrap(), wallet);
    let refused_history = serde_json::to_value(&running.history).unwrap();
    assert!(child.0.try_wait().unwrap().is_none());
    child.stop().unwrap();

    engine.request(json!({"action":"scan"})).unwrap();
    let after_exit = wait(&engine);
    assert!(
        after_exit.stats.complete && after_exit.error.is_none(),
        "{after_exit:?}"
    );
    assert_eq!(after_exit.candidates.len(), 1, "{after_exit:?}");
    let available = &after_exit.candidates[0];
    assert_eq!(available.id, candidate.id);
    assert!(available.suggestion_eligible && available.blocked_reason.is_none());
    assert!(!available.eligible_permanent);
    chippytea_core::scanner::revalidate(&root, available, &AtomicBool::new(false)).unwrap();
    assert!(
        engine
            .request(json!({"action":"prepare", "operation":"trash", "items":[available]}))
            .unwrap()["token"]
            .is_string()
    );
    let final_state = engine.snapshot().unwrap();
    assert_eq!(serde_json::to_value(&final_state.wallet).unwrap(), wallet);
    assert_eq!(
        serde_json::to_value(&final_state.history).unwrap(),
        refused_history
    );
    assert_eq!(
        chippytea_core::safety::identity(&cache).unwrap(),
        candidate.identity
    );
    assert_eq!(
        chippytea_core::safety::identity(&payload).unwrap(),
        payload_identity
    );
    assert_eq!(payload_digest(), original_digest);
    assert!(
        fs::read_dir(cache.parent().unwrap())
            .unwrap()
            .all(|entry| entry.unwrap().path() == cache)
    );
}
