//! Disposable public-Engine benchmark; compile unchanged against each frozen rlib.
//! The production reader is included only for untimed fixed-ordinal selection.
#![allow(dead_code)]

use chippytea_core::{Engine, model};
#[path = "../core/src/safety.rs"]
mod selection_safety;

use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, atomic::AtomicBool};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const MAGIC: &[u8] = b"chippytea-recent-file-v1\n";
const ARTIFACT: &str = "tree/changed-project/node_modules";
const CURSOR: i64 = 1000;

fn require(condition: bool, message: &str) -> Result<()> {
    if condition { Ok(()) } else { Err(message.into()) }
}

fn open(path: &Path, directory: bool, write: bool) -> Result<File> {
    Ok(OpenOptions::new().read(true).write(write)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW_ANY
            | if directory { libc::O_DIRECTORY } else { 0 })
        .open(path)?)
}

fn facts(file: &File) -> Result<Value> {
    let mut value: libc::stat = unsafe { std::mem::zeroed() };
    require(unsafe { libc::fstat(file.as_raw_fd(), &mut value) } == 0, "fstat failed")?;
    require(value.st_size >= 0 && value.st_blocks >= 0, "Invalid fixture metadata")?;
    Ok(json!({"device":value.st_dev as u64,"inode":value.st_ino,
        "mode":value.st_mode as u32,"size":value.st_size as u64,
        "modified_ns":value.st_mtime * 1_000_000_000 + value.st_mtime_nsec,
        "changed_ns":value.st_ctime * 1_000_000_000 + value.st_ctime_nsec,
        "uid":value.st_uid,"links":value.st_nlink,"flags":value.st_flags,
        "allocated_bytes":value.st_blocks as u64 * 512}))
}

fn local_owned(value: &Value, directory: bool) -> Result<()> {
    let mode = value["mode"].as_u64().ok_or("Missing mode")? as u32;
    require(value["uid"] == unsafe { libc::geteuid() }
        && value["flags"] == 0
        && mode & libc::S_IFMT as u32 == if directory { libc::S_IFDIR as u32 } else { libc::S_IFREG as u32 }
        && (directory || value["links"] == 1), "Fixture must be owned, local, unlinked regular data")
}

fn fixture(path: &Path) -> Result<()> {
    let name = path.file_name().and_then(|value| value.to_str()).ok_or("Invalid fixture name")?;
    let suffix = name.strip_prefix("chippytea-recent-file-").ok_or("Unmarked fixture name")?;
    require(path.parent() == Some(Path::new("/private/tmp")) && suffix.len() == 32
        && suffix.bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()), "Use an exact physical disposable fixture path")?;
    let value = facts(&open(path, true, false)?)?;
    local_owned(&value, true)?;
    require(value["mode"].as_u64().unwrap() & 0o7777 == 0o700, "Fixture must have mode 0700")?;
    let mut marker = open(&path.join(".chippytea-recent-fixture"), false, false)?;
    local_owned(&facts(&marker)?, false)?;
    let mut bytes = Vec::new();
    (&mut marker).take(64).read_to_end(&mut bytes)?;
    require(bytes == MAGIC, "Missing disposable fixture marker")?;
    local_owned(&facts(&open(&path.join("tree"), true, false)?)?, true)
}

fn leaf(fixture: &Path, relative: &str) -> Result<PathBuf> {
    let path = Path::new(relative);
    require(path.components().all(|part| matches!(part, Component::Normal(_))), "Leaf must be a strict relative path")?;
    let full = fixture.join(path);
    let suffix = full.strip_prefix(fixture.join(ARTIFACT))?;
    require(suffix.components().count() == 4
        && suffix.starts_with("deep/nested")
        && suffix.parent().and_then(Path::file_name).and_then(|name| name.to_str())
            .is_some_and(|name| name.len() == 10 && name.starts_with("group-") && name[6..].bytes().all(|b| b.is_ascii_digit()))
        && suffix.file_name().and_then(|name| name.to_str())
            .is_some_and(|name| name.len() == 13 && name.starts_with("leaf-") && name.ends_with(".txt") && name[5..9].bytes().all(|b| b.is_ascii_digit())),
        "Use an existing generated leaf strictly inside the changed artifact")?;
    Ok(full)
}

fn ancestors(fixture: &Path, leaf: &Path) -> Result<Vec<(PathBuf, Value)>> {
    let mut values = Vec::new();
    let mut path = leaf.parent().ok_or("Leaf has no parent")?;
    loop {
        require(path.starts_with(fixture), "Ancestor left fixture")?;
        let value = facts(&open(path, true, false)?)?;
        local_owned(&value, true)?;
        values.push((path.to_owned(), value));
        if path == fixture.join("tree") { break; }
        path = path.parent().ok_or("Missing ancestor")?;
    }
    require(values.iter().all(|(_, value)| value["device"] == values[0].1["device"]), "Fixture crosses devices")?;
    Ok(values)
}

fn usage() -> Result<(f64, u64)> {
    let mut value: libc::rusage = unsafe { std::mem::zeroed() };
    require(unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut value) } == 0, "getrusage failed")?;
    Ok((value.ru_utime.tv_sec as f64 + value.ru_stime.tv_sec as f64
        + (value.ru_utime.tv_usec + value.ru_stime.tv_usec) as f64 / 1_000_000.0,
        value.ru_maxrss as u64))
}

fn settled(engine: &Arc<Engine>) -> Result<Value> {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let snapshot = engine.snapshot()?;
        require(!snapshot.cleaning && snapshot.error.is_none(), "Engine reported an operation or scan error")?;
        if !snapshot.scanning {
            require(snapshot.stats.complete && !snapshot.stats.cancelled && snapshot.stats.errors == 0,
                "Engine stopped without complete error-free coverage")?;
            return Ok(serde_json::to_value(snapshot)?);
        }
        require(Instant::now() < deadline, "Scan did not settle")?;
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn library(path: &Path) -> Result<Value> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW)?;
    connection.execute_batch("BEGIN")?;
    let mut counts = serde_json::Map::new();
    for table in ["roots", "scans", "pending_scopes", "active_scopes", "refreshes", "refresh_seen",
        "incomplete_roots", "kept", "candidate_tombstones", "operations", "earnings", "windows", "allocations"] {
        let count: i64 = connection.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row.get(0))?;
        counts.insert(table.into(), json!(count));
    }
    let candidates: Vec<Value> = connection.prepare("SELECT json FROM candidates ORDER BY path")?
        .query_map([], |row| row.get::<_, String>(0))?
        .map(|row| Ok(serde_json::from_str(&row?)?)).collect::<Result<_>>()?;
    let scan: String = connection.query_row("SELECT json FROM scans", [], |row| row.get(0))?;
    let foreground: String = connection.query_row("SELECT summary_json FROM foreground_state WHERE id=1", [], |row| row.get(0))?;
    let cursor: i64 = connection.query_row("SELECT cursor FROM event_cursor WHERE id=1", [], |row| row.get(0))?;
    let wallet: (i64, i64, i64) = connection.query_row("SELECT collected,remainder,credited FROM wallet WHERE id=1", [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
    connection.execute_batch("COMMIT")?;
    Ok(json!({"counts":counts,"candidates":candidates,"scope_stats":serde_json::from_str::<Value>(&scan)?,
        "foreground_scan":serde_json::from_str::<Value>(&foreground)?,"cursor":cursor,"wallet":wallet}))
}

fn checked_state(snapshot: &Value, state: &Value, cursor: i64) -> Result<()> {
    for (table, count) in state["counts"].as_object().ok_or("Missing table counts")? {
        require(*count == if table == "roots" || table == "scans" { 1 } else { 0 },
            "Durable work, incomplete coverage, or accounting state remains")?;
    }
    require(state["wallet"] == json!([0, 0, 0]) && state["cursor"] == cursor
        && snapshot["wallet"] == json!({"collected_coins":0,"pending_coins":0,"fractional_bytes":0,"credited_bytes":0})
        && snapshot["history"] == json!([]) && snapshot["kept_paths"] == json!([])
        && snapshot["foreground_scan"]["active"] == false
        && snapshot["foreground_scan"]["stats"]["complete"] == true
        && snapshot["foreground_scan"]["stats"]["cancelled"] == false
        && snapshot["foreground_scan"]["stats"]["errors"] == 0
        && state["foreground_scan"] == snapshot["foreground_scan"]
        && state["scope_stats"]["complete"] == true && state["scope_stats"]["cancelled"] == false
        && state["scope_stats"]["errors"] == 0, "Endpoint lost saved foreground, cursor, coverage, or zero ledger")
}

fn candidate<'a>(state: &'a Value, path: &Path) -> Result<&'a Value> {
    state["candidates"].as_array().ok_or("Missing index")?.iter()
        .find(|value| value["path"] == path.to_string_lossy().as_ref()).ok_or_else(|| "Missing indexed artifact".into())
}

fn select(fixture: &Path, count: u64) -> Result<Value> {
    require(count >= 256 && count <= 131_072 && count % 256 == 0, "Use 256..131072 leaves in groups of 256")?;
    let artifact = fixture.join(ARTIFACT);
    let metadata = selection_safety::metadata(&artifact)?;
    let mut ordinal = 0;
    let mut selected = Vec::with_capacity(3);
    let measurement = selection_safety::measure_observing_with_policy(&artifact, metadata.identity.device,
        &AtomicBool::new(false), selection_safety::MeasurementPolicy::Developer, |entry, measurement| {
            if entry.meta.is_file() && entry.path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.starts_with("leaf-")) {
                ordinal += 1;
                if [1, count / 2, count].contains(&ordinal) {
                    selected.push(json!({"ordinal":ordinal,"entry":measurement.entries,
                        "path":entry.path,"identity":entry.meta.identity}));
                }
            }
        })?;
    require(ordinal == count && selected.len() == 3 && measurement.unsafe_reason.is_none()
        && measurement.errors == 0 && !measurement.pruned, "Selection did not cover the intended safe artifact")?;
    Ok(json!({"selected":selected,"entries":measurement.entries,"files":measurement.files}))
}

fn run(fixture: &Path, relative: &str, id: &str, mode: &str, old_ns: i64, rescan: bool) -> Result<Value> {
    require(id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()), "Use a fresh 32-hex database ID")?;
    require(mode == "recent" || mode == "control", "Unknown workload")?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos() as i64;
    require(old_ns > 0 && now - old_ns > 8 * 86_400 * 1_000_000_000, "Fixture must be older than eight days")?;
    let path = leaf(fixture, relative)?;
    let database = fixture.join(format!("state-{id}.sqlite"));
    for path in [database.clone(), database.with_extension("lock"),
        PathBuf::from(format!("{}-wal", database.display())), PathBuf::from(format!("{}-shm", database.display())),
        PathBuf::from(format!("{}-journal", database.display()))] {
        match path.symlink_metadata() {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
            _ => return Err("Refusing an existing database or sidecar".into()),
        }
    }
    OpenOptions::new().write(true).create_new(true).mode(0o600)
        .custom_flags(libc::O_NOFOLLOW_ANY | libc::O_CLOEXEC).open(&database)?;
    let engine = Engine::open(&database, None)?;
    let root = engine.request(json!({"action":"authorize","path":fixture.join("tree"),"kind":"projects"}))?;
    engine.request(json!({"action":"scan"}))?;
    let before = settled(&engine)?;
    require(before["candidates"].as_array().is_some_and(|items| items.len() == 2), "Initial scan must find two eligible artifacts")?;
    let state_before = library(&database)?;
    checked_state(&before, &state_before, 0)?;
    let parent_before = ancestors(fixture, &path)?;
    let mutates = mode == "recent" || rescan;
    let file = open(&path, false, mutates)?;
    let leaf_before = facts(&file)?;
    local_owned(&leaf_before, false)?;
    require(leaf_before["modified_ns"] == old_ns && leaf_before["size"].as_u64().unwrap() > 0
        && leaf_before["device"] == parent_before[0].1["device"], "Witness is not an old local fixture leaf")?;
    let mut byte = [0];
    file.read_exact_at(&mut byte, 0)?;
    let old_byte = byte[0];
    if mutates {
        file.write_all_at(&[old_byte ^ 1], 0)?;
        file.sync_all()?;
    }
    let mut leaf_after = facts(&file)?;
    local_owned(&leaf_after, false)?;
    for key in ["device", "inode", "mode", "size", "uid", "links", "flags", "allocated_bytes"] {
        require(leaf_after[key] == leaf_before[key], "Mutation changed more than one byte and timestamps")?;
    }
    require(!mutates || leaf_after["modified_ns"].as_i64().unwrap() > now - 60 * 1_000_000_000,
        "Write did not produce a recent filesystem modification time")?;
    drop(file);
    require(facts(&open(&path, false, false)?)? == leaf_after && ancestors(fixture, &path)? == parent_before,
        "Mutation changed a path identity or ancestor")?;
    let leaf_seeded = leaf_after.clone();
    let mut seeded = Value::Null;
    let mut state_seeded = Value::Null;
    if rescan {
        // Learn through ordinary discovery, without delivering a file event.
        // Neither this seed scan nor the optional aging is in the timed interval.
        engine.request(json!({"action":"scan"}))?;
        seeded = settled(&engine)?;
        state_seeded = library(&database)?;
        checked_state(&seeded, &state_seeded, 0)?;
        require(seeded["candidates"].as_array().is_some_and(|items| items.len() == 1)
            && candidate(&state_seeded, &fixture.join(ARTIFACT))?["suggestion_eligible"] == false,
            "Seed scan must independently discover the recently modified artifact")?;
        if mode == "control" {
            let file = open(&path, false, false)?;
            require(facts(&file)? == leaf_after, "Witness changed before aging")?;
            file.set_times(std::fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_nanos(old_ns as u64)))?;
            leaf_after = facts(&file)?;
            for key in ["device", "inode", "mode", "size", "uid", "links", "flags", "allocated_bytes"] {
                require(leaf_after[key] == leaf_seeded[key], "Aging changed the fixture identity or allocation")?;
            }
            require(leaf_after["modified_ns"] == old_ns
                && leaf_after["changed_ns"].as_i64() >= leaf_seeded["changed_ns"].as_i64(), "Aging did not restore only mtime")?;
        }
        require(facts(&open(&path, false, false)?)? == leaf_after && ancestors(fixture, &path)? == parent_before,
            "Seed scan or aging changed an unrelated path")?;
    }
    let dirty = json!({"action":"dirty","root_id":root["id"],"events":[{"path":path,"kind":"file","recursive":false}]});
    let request = if rescan { json!({"action":"scan"}) } else { dirty.clone() };
    let cursor = if rescan { 0 } else { CURSOR };
    let started = Instant::now();
    let (cpu_start, _) = usage()?;
    drop(engine.request(request.clone())?);
    let ack_wall = started.elapsed().as_secs_f64();
    let (ack_cpu, _) = usage()?;
    if !rescan {
        engine.request(json!({"action":"cursor","value":CURSOR}))?;
    }
    let after = settled(&engine)?;
    let wall = started.elapsed().as_secs_f64();
    let (cpu_end, rss) = usage()?;
    let state_after = library(&database)?;
    checked_state(&after, &state_after, cursor)?;
    let changed_path = fixture.join(ARTIFACT);
    let control_path = fixture.join("tree/control-project/node_modules");
    require(state_before["candidates"].as_array().unwrap().len() == 2
        && state_after["candidates"].as_array().unwrap().len() == 2
        && candidate(&state_before, &control_path)? == candidate(&state_after, &control_path)?
        && (rescan || before["foreground_scan"] == after["foreground_scan"]), "Refresh changed independent proof or saved foreground")?;
    if mode == "recent" {
        let changed = candidate(&state_after, &changed_path)?;
        require(changed["suggestion_eligible"] == false && changed["eligible_permanent"] == false
            && changed["blocked_reason"].as_str().is_some_and(|reason| reason.contains("quiet"))
            && after["candidates"] == json!([candidate(&state_before, &control_path)?]),
            "Recent changed artifact did not become an ineligible quiet-period diagnostic")?;
    } else if !rescan {
        require(state_after["candidates"] == state_before["candidates"]
            && after["candidates"] == before["candidates"], "Old-file event changed an eligible proof")?;
    } else {
        require(after["candidates"].as_array().is_some_and(|items| items.len() == 2)
            && candidate(&state_after, &changed_path)?["eligible_permanent"] == true,
            "Aged witness did not fall back to complete eligible discovery")?;
    }
    // The measured endpoint and its durable proof are captured before repair.
    engine.request(json!({"action":"scan","metadata_coverage":true}))?;
    let full = settled(&engine)?;
    let state_full = library(&database)?;
    checked_state(&full, &state_full, cursor)?;
    require(facts(&open(&path, false, false)?)? == leaf_after && ancestors(fixture, &path)? == parent_before,
        "Fixture changed during engine verification")?;
    let mut record = json!({"protocol":if rescan { 2 } else { 1 },"mode":mode,"fixture":fixture,"database":database,"leaf":path,
        "wall_seconds":wall,"cpu_seconds":cpu_end-cpu_start,
        "lifetime_peak_rss_bytes":rss,"before":before,"after":after,"full":full,
        "library_states":{"before":state_before,"after":state_after,"full":state_full},
        "mutation":{"before":leaf_before,"after":leaf_after,"old_byte":old_byte,
            "new_byte":if mutates { old_byte ^ 1 } else { old_byte },"ancestors":parent_before}});
    let action = if rescan { "scan" } else { "dirty" };
    record[format!("{action}_request")] = request;
    record[format!("{action}_acknowledgment_seconds")] = json!(ack_wall);
    record[format!("{action}_acknowledgment_cpu_seconds")] = json!(ack_cpu - cpu_start);
    if rescan {
        record["seeded"] = seeded;
        record["library_states"]["seeded"] = state_seeded;
        record["mutation"]["seeded"] = leaf_seeded;
    }
    Ok(record)
}

fn main() -> Result<()> {
    let arguments: Vec<String> = std::env::args().collect();
    require(cfg!(target_os = "macos") && arguments.len() >= 4, "Usage: select FIXTURE LEAVES | run|rescan FIXTURE RELATIVE_LEAF ID recent|control OLD_NS")?;
    let path = Path::new(&arguments[2]);
    fixture(path)?;
    let result = match arguments[1].as_str() {
        "select" if arguments.len() == 4 => select(path, arguments[3].parse()?)?,
        action @ ("run" | "rescan") if arguments.len() == 7 => run(path, &arguments[3], &arguments[4], &arguments[5], arguments[6].parse()?, action == "rescan")?,
        _ => return Err("Invalid benchmark arguments".into()),
    };
    println!("{result}");
    Ok(())
}
