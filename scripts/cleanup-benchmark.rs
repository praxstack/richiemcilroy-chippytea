//! Creates its own disposable artifact, then times the public cleanup engine.
//! No argument can name a deletion target. Failed and completed fixtures remain.
#[cfg(not(target_os = "macos"))]
compile_error!("This benchmark requires macOS and physical /private/tmp");

use chippytea_core::{
    Engine,
    cleanup::{self, CleanupPhase},
    model::{COIN_BYTES, Candidate, Root},
    store::Store,
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    ffi::CString,
    fs::{self, DirBuilder, File, FileTimes, OpenOptions},
    io::{BufWriter, Read, Write},
    os::unix::{
        ffi::OsStrExt,
        fs::{DirBuilderExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const MAGIC: &[u8] = b"chippytea-cleanup-benchmark-v1\n";
const PAYLOAD_BYTES: u64 = 100 * 1024 * 1024;
const GROUP_SIZE: usize = 256;
const LEAF: &[u8] = b"Disposable compiled output; no user data.\n";
const TAG: &[u8] = b"Signature: 8a477f597d28d172789f06886806bc55\n";

fn require(ok: bool, message: &str) -> Result<()> {
    if ok { Ok(()) } else { Err(message.into()) }
}

fn create_file(path: &Path) -> Result<File> {
    Ok(OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW_ANY)
        .open(path)?)
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = create_file(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn write_json(path: &Path, value: &Value) -> Result<()> {
    let mut file = create_file(path)?;
    serde_json::to_writer_pretty(&mut file, value)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}

fn open_file(path: &Path) -> Result<File> {
    Ok(OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW_ANY)
        .open(path)?)
}

fn metadata(path: &Path, device: Option<u64>) -> Result<Value> {
    let path = CString::new(path.as_os_str().as_bytes())?;
    let mut value: libc::stat = unsafe { std::mem::zeroed() };
    require(
        unsafe { libc::lstat(path.as_ptr(), &mut value) } == 0,
        "Cannot audit fixture metadata",
    )?;
    let kind = value.st_mode & libc::S_IFMT;
    require(
        (kind == libc::S_IFREG || kind == libc::S_IFDIR)
            && value.st_uid == unsafe { libc::geteuid() }
            && value.st_flags == 0
            && value.st_size >= 0
            && value.st_blocks >= 0
            && device.is_none_or(|expected| value.st_dev as u64 == expected),
        "Fixture must contain only owned local regular files and directories on one device",
    )?;
    Ok(
        json!({"device":value.st_dev as u64,"inode":value.st_ino,"mode":value.st_mode,
        "size":value.st_size,"allocated_bytes":value.st_blocks as u64 * 512,"links":value.st_nlink,
        "uid":value.st_uid,"flags":value.st_flags,"directory":kind == libc::S_IFDIR,
        "modified_ns":value.st_mtime * 1_000_000_000 + value.st_mtime_nsec,
        "changed_ns":value.st_ctime * 1_000_000_000 + value.st_ctime_nsec}),
    )
}

fn hash_file(path: &Path) -> Result<String> {
    let mut file = open_file(path)?;
    let mut hash = blake3::Hasher::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(hash.finalize().to_hex().to_string())
}

fn sentinel(path: &Path, device: u64) -> Result<Value> {
    let before = metadata(path, Some(device))?;
    require(
        before["directory"] == false && before["links"] == 1,
        "Sentinel must remain a single-link regular file",
    )?;
    let hash = hash_file(path)?;
    require(
        metadata(path, Some(device))? == before,
        "Sentinel changed during audit",
    )?;
    Ok(json!({"metadata":before,"blake3":hash}))
}

fn reserve(leaves: usize) -> Result<()> {
    let mut value: libc::statfs = unsafe { std::mem::zeroed() };
    require(
        unsafe { libc::statfs(c"/private/tmp".as_ptr(), &mut value) } == 0,
        "Cannot check fixture capacity",
    )?;
    let required = 3 * 1024u64.pow(3) + PAYLOAD_BYTES + (leaves as u64 + 1024) * 16_384;
    require(
        value.f_bavail.saturating_mul(value.f_bsize as u64) >= required,
        "Keep a 3 GiB reserve in addition to payload, metadata, and audit space",
    )
}

fn make_fixture(case: &str, leaves: usize) -> Result<(PathBuf, Vec<PathBuf>)> {
    require(
        fs::canonicalize("/private/tmp")? == Path::new("/private/tmp"),
        "Temporary parent must be physical /private/tmp",
    )?;
    reserve(leaves)?;
    let mut random = [0u8; 16];
    unsafe { libc::arc4random_buf(random.as_mut_ptr().cast(), random.len()) };
    let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
    let base = Path::new("/private/tmp").join(format!("chippytea-cleanup-{suffix}"));
    DirBuilder::new().mode(0o700).create(&base)?;
    let base_meta = metadata(&base, None)?;
    require(
        base_meta["mode"].as_u64().unwrap() & 0o7777 == 0o700,
        "Disposable directory must have mode 0700",
    )?;
    write_new(&base.join(".chippytea-cleanup-fixture"), MAGIC)?;
    eprintln!(
        "{}",
        json!({"phase":"fixture_created","pid":std::process::id(),"fixture":base})
    );
    for relative in [
        "Projects",
        "Projects/Disposable",
        "Projects/Disposable/target",
        "Projects/Disposable/target/debug",
        "Projects/Sibling",
        "State",
    ] {
        DirBuilder::new().mode(0o700).create(base.join(relative))?;
    }
    let project = base.join("Projects/Disposable");
    let artifact = project.join("target");
    write_new(
        &project.join("Cargo.toml"),
        b"[package]\nname=\"disposable-cleanup\"\nversion=\"0.1.0\"\n",
    )?;
    write_new(&artifact.join("CACHEDIR.TAG"), TAG)?;
    let mut directories = vec![
        artifact.clone(),
        artifact.join("debug"),
        project.clone(),
        base.join("Projects"),
    ];
    let mut files = vec![project.join("Cargo.toml"), artifact.join("CACHEDIR.TAG")];
    {
        let path = artifact.join("payload.bin");
        let mut file = create_file(&path)?;
        let mut block = vec![0u8; 1024 * 1024];
        let mut hash = blake3::Hasher::new();
        hash.update(b"chippytea-cleanup-benchmark-payload-v1");
        hash.finalize_xof().fill(&mut block);
        for _ in 0..100 {
            file.write_all(&block)?;
        }
        file.sync_all()?;
        files.push(path);
    }
    for group in 0..leaves / GROUP_SIZE {
        let directory = artifact.join(format!("debug/group-{group:04}"));
        DirBuilder::new().mode(0o700).create(&directory)?;
        for member in 0..GROUP_SIZE {
            let path = directory.join(format!("leaf-{member:04}.o"));
            if case == "hardlinks" && member % 2 == 1 {
                fs::hard_link(directory.join(format!("leaf-{:04}.o", member - 1)), &path)?;
            } else {
                let mut file = create_file(&path)?;
                file.write_all(LEAF)?;
            }
            files.push(path);
        }
        directories.push(directory);
    }
    let preserved = vec![
        project.join("Cargo.toml"),
        project.join("source.rs"),
        base.join("Projects/Sibling/preserve.txt"),
        base.join("outside-authorized-root.txt"),
        base.join(".chippytea-cleanup-fixture"),
    ];
    for path in &preserved[1..4] {
        write_new(path, b"Preserve this disposable sentinel exactly.\n")?;
    }
    let old = SystemTime::now() - Duration::from_secs(9 * 86_400);
    for path in files
        .iter()
        .chain(directories.iter())
        .chain(preserved.iter())
    {
        open_file(path)?.set_times(FileTimes::new().set_modified(old))?;
    }
    // Sync generation before the untimed audits and discovery warm this fixture.
    open_file(&artifact)?.sync_all()?;
    Ok((base, preserved))
}

fn audit_artifact(base: &Path, case: &str, leaves: usize, device: u64) -> Result<Value> {
    let artifact = base.join("Projects/Disposable/target");
    let mut output = BufWriter::new(create_file(&base.join("input-metadata.jsonl"))?);
    let mut stack = vec![artifact.clone()];
    let mut hardlinks: HashMap<(u64, u64), (u64, u64, Value)> = HashMap::new();
    let (mut entries, mut files, mut directories, mut logical, mut allocated) =
        (0u64, 0u64, 0u64, 0u64, 0u64);
    let cutoff = (SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
        - 8 * 86_400 * 1_000_000_000) as i64;
    while let Some(path) = stack.pop() {
        let value = metadata(&path, Some(device))?;
        require(
            value["modified_ns"].as_i64().unwrap() < cutoff,
            "Artifact must be older than eight days",
        )?;
        entries += 1;
        let mut record = json!({"path":path.strip_prefix(base)?,"metadata":value});
        if value["directory"] == true {
            directories += 1;
            for entry in fs::read_dir(&path)? {
                stack.push(entry?.path());
            }
        } else {
            files += 1;
            let links = value["links"].as_u64().unwrap();
            require(
                links == 1
                    || (case == "hardlinks"
                        && links == 2
                        && path.extension().is_some_and(|name| name == "o")),
                "Unexpected external or unsupported hard link",
            )?;
            let first = if links == 1 {
                true
            } else {
                let key = (device, value["inode"].as_u64().unwrap());
                let seen = hardlinks.entry(key).or_insert((0, links, value.clone()));
                require(
                    seen.1 == links && seen.2 == value,
                    "Linked-file metadata is inconsistent",
                )?;
                seen.0 += 1;
                seen.0 == 1
            };
            if first {
                logical += value["size"].as_u64().unwrap();
                allocated += value["allocated_bytes"].as_u64().unwrap();
            }
            record["blake3"] = json!(hash_file(&path)?);
            require(
                metadata(&path, Some(device))? == value,
                "Input changed during audit",
            )?;
        }
        serde_json::to_writer(&mut output, &record)?;
        output.write_all(b"\n")?;
    }
    require(
        hardlinks
            .values()
            .all(|(seen, expected, _)| seen == expected),
        "Hard links must be closed inside the artifact",
    )?;
    let unique_leaves = if case == "hardlinks" {
        leaves / 2
    } else {
        leaves
    } as u64;
    require(
        files == leaves as u64 + 2
            && directories == leaves as u64 / GROUP_SIZE as u64 + 2
            && logical == PAYLOAD_BYTES + unique_leaves * LEAF.len() as u64 + TAG.len() as u64,
        "Fixture counts or unique logical bytes do not match the generated workload",
    )?;
    output.flush()?;
    output.get_ref().sync_all()?;
    Ok(
        json!({"entries":entries,"files":files,"directories":directories,
        "unique_regular_files":unique_leaves+2,"hardlink_groups":hardlinks.len(),
        "logical_bytes":logical,"allocated_bytes":allocated,"metadata_file":"input-metadata.jsonl"}),
    )
}

fn discover(database: &Path, projects: &Path, artifact: &Path) -> Result<(Root, Candidate, Value)> {
    create_file(database)?;
    let engine = Engine::open(database, None)?;
    let root: Root = serde_json::from_value(
        engine.request(json!({"action":"authorize","path":projects,"kind":"projects"}))?,
    )?;
    engine.request(json!({"action":"scan"}))?;
    let deadline = Instant::now() + Duration::from_secs(90);
    let snapshot = loop {
        let snapshot = engine.snapshot()?;
        require(snapshot.error.is_none(), "Discovery reported an error")?;
        if !snapshot.scanning {
            break snapshot;
        }
        require(Instant::now() < deadline, "Discovery did not finish")?;
        std::thread::sleep(Duration::from_millis(5));
    };
    require(
        snapshot.stats.complete
            && !snapshot.stats.cancelled
            && snapshot.stats.errors == 0
            && snapshot.candidates.len() == 1
            && snapshot.history.is_empty()
            && snapshot.wallet.credited_bytes == 0
            && snapshot.wallet.pending_coins == 0,
        "Discovery must finish cleanly with one genuine suggestion and no rewards",
    )?;
    let candidate = snapshot.candidates[0].clone();
    require(
        candidate.path == artifact
            && candidate.root_id == root.id
            && candidate.kind == "cargo"
            && candidate.suggestion_eligible
            && candidate.eligible_permanent
            && !candidate.provisional
            && candidate.blocked_reason.is_none()
            && !candidate.fingerprint.is_empty()
            && !candidate.evidence.is_empty(),
        "The generated artifact did not pass ordinary scanner eligibility",
    )?;
    while Arc::strong_count(&engine) != 1 {
        require(
            Instant::now() < deadline,
            "Discovery worker did not release its engine",
        )?;
        std::thread::sleep(Duration::from_millis(1));
    }
    drop(engine);
    Ok((root, candidate, serde_json::to_value(snapshot.stats)?))
}

#[derive(Clone, Copy)]
struct Usage {
    at: Instant,
    user: f64,
    system: f64,
    rss: u64,
}
impl Usage {
    fn read() -> Result<Self> {
        let mut value: libc::rusage = unsafe { std::mem::zeroed() };
        require(
            unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut value) } == 0,
            "getrusage failed",
        )?;
        Ok(Self {
            at: Instant::now(),
            user: value.ru_utime.tv_sec as f64 + value.ru_utime.tv_usec as f64 / 1_000_000.0,
            system: value.ru_stime.tv_sec as f64 + value.ru_stime.tv_usec as f64 / 1_000_000.0,
            rss: value.ru_maxrss as u64,
        })
    }
    fn since(self, before: Self) -> Value {
        json!({"wall_seconds":self.at.duration_since(before.at).as_secs_f64(),
            "user_seconds":self.user-before.user,"system_seconds":self.system-before.system,
            "cpu_seconds":self.user+self.system-before.user-before.system,
            "lifetime_peak_rss_bytes":self.rss})
    }
}

fn ledger(store: &Store) -> Result<Value> {
    let count = |table: &str| -> Result<u64> {
        Ok(store
            .conn
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })?)
    };
    let mut counts = serde_json::Map::new();
    for table in [
        "operations",
        "earnings",
        "allocations",
        "windows",
        "cleanup_entries",
        "operation_parents",
        "candidates",
    ] {
        counts.insert(table.into(), json!(count(table)?));
    }
    let mut earnings = Vec::new();
    let mut statement = store
        .conn
        .prepare("SELECT operation_id,coins,collected FROM earnings ORDER BY operation_id")?;
    for row in statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, u64>(1)?,
            row.get::<_, u64>(2)?,
        ))
    })? {
        earnings.push(row?);
    }
    let mut allocations = Vec::new();
    let mut statement = store
        .conn
        .prepare("SELECT operation_id,bytes FROM allocations ORDER BY operation_id")?;
    for row in statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?))
    })? {
        allocations.push(row?);
    }
    let mut operations = Vec::new();
    let mut statement = store
        .conn
        .prepare("SELECT id,state FROM operations ORDER BY id")?;
    for row in statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })? {
        operations.push(row?);
    }
    let mut windows = Vec::new();
    let mut statement = store
        .conn
        .prepare("SELECT id,state,private_bound,credited_bytes FROM windows ORDER BY id")?;
    for row in statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, u64>(2)?,
            row.get::<_, u64>(3)?,
        ))
    })? {
        windows.push(row?);
    }
    Ok(
        json!({"counts":counts,"wallet":store.wallet()?,"history":store.history()?,"earnings":earnings,"allocations":allocations,
        "operation_states":operations,"windows":windows}),
    )
}

fn run(case: &str, leaves: usize, ready_delay_ms: u64) -> Result<Value> {
    let (base, preserved) = make_fixture(case, leaves)?;
    let artifact = base.join("Projects/Disposable/target");
    let project = artifact.parent().unwrap();
    let device = metadata(&base, None)?["device"].as_u64().unwrap();
    let input = audit_artifact(&base, case, leaves, device)?;
    let before: Vec<Value> = preserved
        .iter()
        .map(|path| sentinel(path, device))
        .collect::<Result<_>>()?;
    let database = base.join("State/library.sqlite");
    let (root, candidate, scan) = discover(&database, &base.join("Projects"), &artifact)?;
    require(
        candidate.file_count == input["files"].as_u64().unwrap()
            && candidate.logical_bytes == input["logical_bytes"].as_u64().unwrap()
            && candidate.allocated_bytes == input["allocated_bytes"].as_u64().unwrap(),
        "Scanner counts must agree with the independent hard-link-aware audit",
    )?;
    write_json(
        &base.join("input.json"),
        &json!({"case":case,"leaves":leaves,"artifact":input,
        "root":root,"candidate":candidate,"scan":scan,"sentinels":before}),
    )?;
    let mut store = Store::open(&database)?;
    require(
        store.candidate(&candidate.id)? == candidate,
        "Stored candidate differs from the ordinary scan",
    )?;
    eprintln!(
        "{}",
        json!({"phase":"ready","pid":std::process::id(),"fixture":base,
        "case":case,"leaves":leaves,"cleanup_entries":input["entries"],"delay_ms":ready_delay_ms})
    );
    std::io::stderr().flush()?;
    if ready_delay_ms > 0 {
        std::thread::sleep(Duration::from_millis(ready_delay_ms));
    }
    let start = Usage::read()?;
    let mut phase_start = start;
    let mut current = CleanupPhase::Checking;
    let mut label = "preflight";
    let mut callbacks = 0u64;
    let (mut completed, mut total) = (0u64, 0u64);
    let mut phases = Vec::with_capacity(5);
    let mut timing_error = None;
    let result = cleanup::execute_with_progress(
        &mut store,
        &root,
        &candidate,
        "permanent",
        None,
        &AtomicBool::new(false),
        |phase, done, expected| {
            if phase != current {
                match Usage::read() {
                    Ok(now) => {
                        phases.push(json!({"phase":label,"timing":now.since(phase_start),"callbacks":callbacks,"completed":completed,"total":total}));
                        phase_start = now;
                    }
                    Err(error) => {
                        timing_error = Some(error.to_string());
                    }
                }
                current = phase;
                label = match phase {
                    CleanupPhase::Checking => "staged_checking",
                    CleanupPhase::Preparing => "preparing",
                    CleanupPhase::Removing => "removing",
                    CleanupPhase::Accounting => "accounting",
                };
                callbacks = 0;
            }
            callbacks += 1;
            completed = done;
            total = expected;
        },
    );
    let end = Usage::read()?;
    phases.push(json!({"phase":label,"timing":end.since(phase_start),"callbacks":callbacks,"completed":completed,"total":total}));
    if let Some(error) = timing_error {
        return Err(error.into());
    }
    let receipt = result?;
    require(
        receipt.outcome == "removed"
            && receipt.operation == "permanent"
            && receipt.trash_path.is_none()
            && !receipt.can_restore,
        &format!("Cleanup did not finish: {}", receipt.detail),
    )?;
    require(
        phases
            .iter()
            .map(|value| value["phase"].as_str().unwrap())
            .collect::<Vec<_>>()
            == [
                "preflight",
                "preparing",
                "staged_checking",
                "removing",
                "accounting",
            ],
        "Unexpected cleanup phase sequence",
    )?;
    require(
        fs::symlink_metadata(&artifact)
            .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound),
        "Artifact remains after cleanup",
    )?;
    for entry in fs::read_dir(project)? {
        require(
            !entry?.file_name().as_bytes().starts_with(b".chippytea-"),
            "A stage or recovery directory remains",
        )?;
    }
    for (path, expected) in preserved.iter().zip(&before) {
        require(
            sentinel(path, device)? == *expected,
            "Source or sentinel changed",
        )?;
    }
    let state = ledger(&store)?;
    let wallet = store.wallet()?;
    require(
        state["counts"]["operations"] == 1
            && state["counts"]["cleanup_entries"] == 0
            && state["counts"]["candidates"] == 0
            && state["history"] == json!([receipt])
            && state["operation_states"] == json!([[receipt.id, "removed"]])
            && receipt.reported_bytes == candidate.allocated_bytes
            && receipt.credited_bytes <= receipt.reported_bytes
            && receipt.credited_bytes <= receipt.observed_bytes
            && receipt.coins == receipt.credited_bytes / COIN_BYTES
            && wallet.credited_bytes == receipt.credited_bytes
            && wallet.fractional_bytes == receipt.credited_bytes % COIN_BYTES
            && wallet.pending_coins == receipt.coins
            && wallet.collected_coins == 0,
        "Receipt, manifest, wallet or conservative credit bounds are inconsistent",
    )?;
    let earnings = state["counts"]["earnings"].as_u64().unwrap();
    require(
        earnings <= 1
            && state["counts"]["allocations"] == earnings
            && (earnings != 0 || receipt.credited_bytes == 0),
        "Accounting produced duplicate or unsupported rewards",
    )?;
    if earnings == 1 {
        require(
            state["earnings"] == json!([[receipt.id, receipt.coins, 0]])
                && state["allocations"] == json!([[receipt.id, receipt.credited_bytes]])
                && state["counts"]["windows"] == 1
                && state["windows"][0][1] == "closed"
                && receipt.credited_bytes <= state["windows"][0][2].as_u64().unwrap()
                && state["windows"][0][3] == receipt.credited_bytes,
            "Accounting rows disagree with receipt",
        )?;
    }
    require(
        cleanup::execute(
            &mut store,
            &root,
            &candidate,
            "permanent",
            None,
            &AtomicBool::new(false),
        )
        .is_err()
            && ledger(&store)? == state,
        "Repeating the removed review must not create another operation or reward",
    )?;
    drop(store);
    let mut reopened = Store::open(&database)?;
    reopened.reconcile()?;
    require(
        ledger(&reopened)? == state,
        "Restart changed the completed receipt or reward state",
    )?;
    let collected = reopened.collect()?;
    require(
        collected == (0, receipt.coins, receipt.coins),
        "First collection differs from the pending reward",
    )?;
    let after_collection = ledger(&reopened)?;
    require(
        reopened.collect()? == (receipt.coins, receipt.coins, 0)
            && ledger(&reopened)? == after_collection,
        "Reward collection was not idempotent",
    )?;
    drop(reopened);
    let mut restarted = Store::open(&database)?;
    restarted.reconcile()?;
    require(
        ledger(&restarted)? == after_collection && restarted.collect()?.2 == 0,
        "Restart after collection duplicated rewards",
    )?;
    let output = json!({"protocol":1,"verified":true,"fixture":base,"database":database,"pid":std::process::id(),
        "case":case,"leaves":leaves,"input":input,"discovery":scan,"timing":end.since(start),"phases":phases,
        "receipt":receipt,"ledger_after_cleanup":state,"ledger_after_restart_and_collection":after_collection,
        "source_and_sentinels_preserved":true,"artifact_absent":true,"staging_and_recovery_absent":true,
        "restart_and_duplicate_reward_checks":true,
        "scope":"Direct public Rust permanent cleanup only; excludes fixture creation, full input audits, discovery, result audits, restart and collection. Generated local fixtures and audits warm caches; first pair is not cold disk I/O. No Swift, FSEvents, user library or real user files. Phase intervals follow actual progress callbacks, including work until the next boundary. RSS is process lifetime peak, including untimed setup. Storage accounting varies with ambient APFS activity; zero coins is a valid measured result."});
    write_json(&base.join("result.json"), &output)?;
    Ok(output)
}

fn main() {
    let result = (|| -> Result<Value> {
        let mut case = String::from("regular");
        let mut leaves = 16_384usize;
        let mut delay = 0u64;
        let mut arguments = std::env::args().skip(1);
        while let Some(option) = arguments.next() {
            let value = arguments
                .next()
                .ok_or("Each option requires a value; filesystem targets are never accepted")?;
            match option.as_str() {
                "--case" => case = value,
                "--leaves" => leaves = value.parse()?,
                "--ready-delay-ms" => delay = value.parse()?,
                _ => return Err("Only --case, --leaves and --ready-delay-ms are accepted; no deletion path may be supplied".into()),
            }
        }
        require(
            matches!(case.as_str(), "regular" | "hardlinks")
                && (256..=131_072).contains(&leaves)
                && leaves % GROUP_SIZE == 0
                && delay <= 5_000,
            "Use regular/hardlinks, 256..131072 leaves in groups of256, and delay at most5000ms",
        )?;
        run(&case, leaves, delay)
    })();
    match result {
        Ok(value) => println!("{value}"),
        Err(error) => {
            eprintln!(
                "FAIL cleanup benchmark: {error}. Disposable evidence is retained; no fallback cleanup runs."
            );
            std::process::exit(1);
        }
    }
}
