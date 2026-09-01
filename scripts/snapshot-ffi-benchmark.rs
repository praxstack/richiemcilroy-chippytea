//! Synthetic, read-only snapshot workload. No scan or cleanup request is made.
use chippytea_core::{
    Engine, ct_close, ct_free_string, ct_open, ct_request,
    model::{Candidate, Identity, Receipt, Root, ScanBatch, ScanStats},
    store::Store,
};
use rusqlite::{Connection, OpenFlags, params, types::ValueRef};
use serde_json::{Value, json};
use std::{
    error::Error,
    ffi::{CStr, CString},
    fs::{self, DirBuilder},
    os::unix::fs::DirBuilderExt,
    path::Path,
    sync::Arc,
    time::Instant,
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const LARGE: u64 = 9_007_199_254_740_993;
const RECEIPTS: usize = 100;

struct Handle(*mut Arc<Engine>);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { ct_close(self.0) };
    }
}

fn candidate(root: &Root, index: usize) -> Candidate {
    Candidate {
        id: format!("synthetic-candidate-{index:04}"),
        root_id: root.id.clone(),
        path: root.path.join(format!("project-{index:04}/.venv")),
        title: format!("Synthetic environment {index:04}: café · 🪙"),
        kind: "venv".into(),
        logical_bytes: u64::MAX,
        allocated_bytes: LARGE + index as u64,
        file_count: 100_000 + index as u64,
        modified_ns: -1,
        explanation:
            "Synthetic serialized fixture; no filesystem discovery occurred.\n\"quoted\"\0".into(),
        consequence: "No scan, review or cleanup command is allowed in this probe.".into(),
        eligible_permanent: false,
        blocked_reason: None,
        identity: root.identity.clone(),
        fingerprint: format!("synthetic-fingerprint-{index:04}"),
        evidence: "Synthetic fixture, not a real cleanup recommendation".into(),
        suggestion_eligible: true,
        provisional: false,
    }
}

fn seed(path: &Path, count: usize) -> Result<()> {
    let root = Root {
        id: "synthetic-root".into(),
        path: "/synthetic/chippytea-ffi-snapshot".into(),
        kind: "folder".into(),
        identity: Identity {
            device: LARGE,
            inode: u64::MAX,
            mode: 0o40700,
            size: 0,
            modified_ns: i64::MIN,
            changed_ns: i64::MAX,
        },
    };
    let mut store = Store::open(path)?;
    store.add_root(&root)?;
    let stats = ScanStats {
        entries: 1_000_001,
        files: 1_000_000,
        directories: 1,
        candidates: count as u64,
        elapsed_ms: 1,
        first_finding_ms: (count != 0).then_some(1),
        complete: true,
        message: "Synthetic counters, not measured scan performance".into(),
        ..Default::default()
    };
    store.save_batch(&ScanBatch {
        candidates: (0..count).map(|index| candidate(&root, index)).collect(),
        stats: stats.clone(),
    })?;
    store.save_stats(&root.id, &stats)?;
    let tx = store.conn.transaction()?;
    tx.execute("DELETE FROM incomplete_roots", [])?;
    let root_json = serde_json::to_string(&root)?;
    for index in 0..RECEIPTS {
        let item = candidate(&root, 1_000 + index);
        let receipt = Receipt {
            id: format!("synthetic-receipt-{index:04}"),
            path: item.path.to_str().unwrap().into(),
            title: item.title.clone(),
            operation: "permanent".into(),
            outcome: "removed".into(),
            detail: "Synthetic receipt, no deletion or reward took place.\n\"quoted\"\0".into(),
            created_at: 1_700_000_000 + index as i64,
            reported_bytes: u64::MAX,
            observed_bytes: LARGE,
            credited_bytes: LARGE,
            coins: 1,
            trash_path: None,
            can_restore: false,
            seq: None,
        };
        tx.execute(
            "INSERT INTO operations VALUES(?1,?2,?3,?4,NULL,NULL,'removed')",
            params![
                receipt.id,
                root_json,
                serde_json::to_string(&item)?,
                serde_json::to_string(&receipt)?
            ],
        )?;
        tx.execute("INSERT INTO earnings VALUES(?1,1,0)", [&receipt.id])?;
    }
    tx.execute(
        "UPDATE wallet SET collected=?1,remainder=17,credited=?1 WHERE id=1",
        [LARGE],
    )?;
    tx.commit()?;
    Ok(())
}

fn feed(hash: &mut blake3::Hasher, bytes: &[u8]) {
    hash.update(&(bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
}

// Logical contents, not WAL/checkpoint layout. Capture after startup, then
// again after all snapshot calls; every table must be unchanged within a run.
fn database_hash(path: &Path) -> Result<String> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut tables =
        conn.prepare("SELECT name,sql FROM sqlite_master WHERE type='table' ORDER BY name")?;
    let tables = tables
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut hash = blake3::Hasher::new();
    for (name, schema) in tables {
        feed(&mut hash, name.as_bytes());
        feed(&mut hash, schema.as_bytes());
        let quoted = name.replace('"', "\"\"");
        let mut query = conn.prepare(&format!("SELECT * FROM \"{quoted}\" ORDER BY rowid"))?;
        let columns = query.column_count();
        let mut rows = query.query([])?;
        while let Some(row) = rows.next()? {
            hash.update(&[0xff]);
            for index in 0..columns {
                let (tag, bytes) = match row.get_ref(index)? {
                    ValueRef::Null => (0, Vec::new()),
                    ValueRef::Integer(value) => (1, value.to_le_bytes().to_vec()),
                    ValueRef::Real(value) => (2, value.to_bits().to_le_bytes().to_vec()),
                    ValueRef::Text(bytes) => (3, bytes.to_vec()),
                    ValueRef::Blob(bytes) => (4, bytes.to_vec()),
                };
                hash.update(&[tag]);
                feed(&mut hash, &bytes);
            }
        }
        hash.update(&[0xfe]);
    }
    Ok(hash.finalize().to_hex().to_string())
}

fn response(handle: &Handle) -> Result<(Value, Vec<u8>)> {
    let raw = unsafe { ct_request(handle.0, c"{\"action\":\"snapshot\"}".as_ptr()) };
    assert!(!raw.is_null(), "Null FFI response");
    let bytes = unsafe { CStr::from_ptr(raw) }.to_bytes().to_vec();
    let parsed = serde_json::from_slice(&bytes);
    unsafe { ct_free_string(raw) };
    Ok((parsed?, bytes))
}

fn validate(handle: &Handle, count: usize) -> Result<(String, Vec<u8>)> {
    let engine = unsafe { &*handle.0 };
    let expected = engine.request(json!({"action":"snapshot"}))?;
    let (actual, bytes) = response(handle)?;
    assert_eq!(
        actual,
        json!({"ok":true,"data":expected}),
        "FFI/public API mismatch"
    );
    let data = &actual["data"];
    assert_eq!(data["candidates"].as_array().unwrap().len(), count);
    assert_eq!(data["history"].as_array().unwrap().len(), RECEIPTS);
    assert_eq!(data["roots"].as_array().unwrap().len(), 1);
    assert_eq!(data["kept_paths"], json!([]));
    assert_eq!(data["scanning"], false);
    assert_eq!(data["cleaning"], false);
    assert!(data["error"].is_null());
    assert_eq!(data["wallet"]["credited_bytes"].as_u64(), Some(LARGE));
    assert_eq!(
        data["wallet"]["pending_coins"].as_u64(),
        Some(RECEIPTS as u64)
    );
    assert_eq!(
        data["history"][0]["reported_bytes"].as_u64(),
        Some(u64::MAX)
    );
    assert!(data["history"][0].get("seq").is_none());
    if count != 0 {
        assert_eq!(
            data["candidates"][0]["logical_bytes"].as_u64(),
            Some(u64::MAX)
        );
    }
    Ok((
        blake3::hash(&serde_json::to_vec(&actual)?)
            .to_hex()
            .to_string(),
        bytes,
    ))
}

fn usage() -> libc::rusage {
    let mut usage = std::mem::MaybeUninit::uninit();
    assert_eq!(
        unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) },
        0
    );
    unsafe { usage.assume_init() }
}

fn seconds(value: libc::timeval) -> f64 {
    value.tv_sec as f64 + value.tv_usec as f64 / 1_000_000.0
}

fn peak_rss_bytes(usage: &libc::rusage) -> u64 {
    #[cfg(target_os = "macos")]
    {
        usage.ru_maxrss as u64
    }
    #[cfg(not(target_os = "macos"))]
    {
        usage.ru_maxrss as u64 * 1024
    }
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().collect();
    assert_eq!(
        args.len(),
        4,
        "driver NEW_PRIVATE_DIRECTORY CANDIDATES ITERATIONS"
    );
    let directory = Path::new(&args[1]);
    let count: usize = args[2].to_str().unwrap().parse()?;
    let iterations: usize = args[3].to_str().unwrap().parse()?;
    assert!(
        matches!(count, 0 | 500),
        "Only the declared workloads are supported"
    );
    assert!((2..=10_000).contains(&iterations));
    assert!(
        directory.is_absolute(),
        "Use an absolute new disposable directory"
    );
    DirBuilder::new().mode(0o700).create(directory)?;
    fs::write(
        directory.join("SYNTHETIC-FIXTURE.txt"),
        "Synthetic FFI snapshot probe only. No real files scanned or deleted.\n",
    )?;
    let path = directory.join("library.sqlite");
    seed(&path, count)?;
    let c_path = CString::new(path.to_str().unwrap())?;
    let handle = Handle(unsafe { ct_open(c_path.as_ptr(), None) });
    assert!(!handle.0.is_null(), "Unable to open synthetic library");
    let (before_json, expected_response) = validate(&handle, count)?;
    let before_size = expected_response.len();
    let before_database = database_hash(&path)?;
    // Setup and parsed JSON validation are outside the timers. One validated
    // raw response remains allocated as the deterministic reference. Every
    // timed call includes C-string access and a full byte comparison before
    // releasing its response; errors cannot masquerade as fast snapshots.
    let mut validated_responses = 0usize;
    let before_usage = usage();
    let start = Instant::now();
    for _ in 0..iterations {
        let raw = unsafe { ct_request(handle.0, c"{\"action\":\"snapshot\"}".as_ptr()) };
        assert!(!raw.is_null());
        let matches = unsafe { CStr::from_ptr(raw) }.to_bytes() == expected_response.as_slice();
        std::hint::black_box(raw);
        unsafe { ct_free_string(raw) };
        assert!(
            matches,
            "A timed FFI response differed from the validated successful snapshot"
        );
        validated_responses += 1;
    }
    let wall = start.elapsed().as_secs_f64();
    let after_usage = usage();
    assert_eq!(validated_responses, iterations);
    let (after_json, after_response) = validate(&handle, count)?;
    assert_eq!(
        (after_json.as_str(), after_response.len()),
        (before_json.as_str(), before_size)
    );
    assert_eq!(
        after_response, expected_response,
        "The reference FFI response changed after timing"
    );
    let after_database = database_hash(&path)?;
    assert_eq!(
        before_database, after_database,
        "Snapshot workload mutated library contents"
    );
    let user = seconds(after_usage.ru_utime) - seconds(before_usage.ru_utime);
    let system = seconds(after_usage.ru_stime) - seconds(before_usage.ru_stime);
    let record = json!({
        "protocol": 2, "verified": true, "synthetic": true,
        "candidates": count, "receipts": RECEIPTS, "iterations": iterations,
        "directory": directory, "response_bytes": before_size,
        "timed_responses_validated": validated_responses,
        "timed_response_validation": "Every response matched this variant's validated successful reference byte-for-byte before being freed",
        "canonical_response_blake3": before_json,
        "database_blake3_before": before_database, "database_blake3_after": after_database,
        "wall_seconds": wall, "user_seconds": user, "system_seconds": system,
        "cpu_seconds": user + system,
        "peak_rss_before_bytes": peak_rss_bytes(&before_usage),
        "peak_rss_after_bytes": peak_rss_bytes(&after_usage),
        "memory_note": "Process-lifetime peak RSS including setup, first JSON validation and one retained raw reference response; not timed-only or live heap",
        "method": "Warm repeated ct_request(snapshot) + C-string access + byte-for-byte response validation + ct_free_string + validation counting; seeding, startup, public API comparison and first/last JSON parsing excluded"
    });
    fs::write(
        directory.join("result.json"),
        serde_json::to_vec_pretty(&record)?,
    )?;
    println!("{}", serde_json::to_string(&record)?);
    Ok(())
}
