use crate::model::*;
use rusqlite::{Connection, OptionalExtension, params};
use std::cell::RefCell;
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

/// Derived findings are intentionally bounded independently of the durable
/// cleanup and event ledgers. These defaults are policy limits, not a claim
/// about the physical size of the SQLite database.
pub(crate) const DEFAULT_MAX_CANDIDATE_ROWS: u64 = 50_000;
pub(crate) const DEFAULT_MAX_CANDIDATE_PAYLOAD_BYTES: u64 = 32 * 1024 * 1024;
pub(crate) const DEFAULT_MAX_CANDIDATE_JSON_BYTES: usize = 16 * 1024;
const CANDIDATE_PAYLOAD_OVERHEAD: u64 = 64;
const DERIVED_USAGE_VERSION: u32 = 1;
const WAL_AUTOCHECKPOINT_PAGES: u32 = 1_000;
const WAL_SIZE_HINT_BYTES: u32 = 4 * 1024 * 1024;
const DERIVED_MAINTENANCE_ROWS: usize = 256;

pub struct Store {
    pub conn: Connection,
    visible_candidates: RefCell<Option<Vec<Candidate>>>,
    database_path: PathBuf,
    derived_limits: DerivedLimits,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct DerivedLimits {
    pub max_candidate_rows: u64,
    pub max_candidate_payload_bytes: u64,
    pub max_candidate_json_bytes: usize,
}

impl Default for DerivedLimits {
    fn default() -> Self {
        Self {
            max_candidate_rows: DEFAULT_MAX_CANDIDATE_ROWS,
            max_candidate_payload_bytes: DEFAULT_MAX_CANDIDATE_PAYLOAD_BYTES,
            max_candidate_json_bytes: DEFAULT_MAX_CANDIDATE_JSON_BYTES,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
pub(crate) struct DerivedUsage {
    pub candidate_rows: u64,
    pub candidate_payload_bytes: u64,
    /// Size of the SQLite database file. This is telemetry only: it includes
    /// durable truth and therefore is not a derived-storage quota.
    pub database_bytes: u64,
    pub wal_bytes: u64,
    pub wal_shm_bytes: u64,
    pub wal_frames: u64,
    pub wal_checkpointed: u64,
    pub wal_busy: bool,
}

pub type StoredOperation = (Root, Candidate, Receipt, Option<Identity>, Option<PathBuf>);

pub(crate) struct DuplicateInputs {
    pub files: Vec<crate::duplicates::Input>,
    pub indexed_files: u64,
    pub skipped_buckets: u64,
    pub skipped_bucket_files: u64,
    pub bucket_limit_reached: bool,
}

/// Binds one committed refresh generation to its separately durable scope claim.
pub(crate) struct ScopeRefresh {
    root: String,
    claimed: String,
    scope: Option<String>,
    generation: String,
    was_incomplete: bool,
}

// A terminal summary contains fixed counters and one human-readable message.
// Bound both storage and decoding of this optional cache to 64 KiB of JSON.
const MAX_FOREGROUND_SUMMARY_BYTES: usize = 64 * 1024;
const FOREGROUND_STATE_SQL: &str = "SELECT revision,
    CASE WHEN typeof(summary_json)='text' AND length(CAST(summary_json AS BLOB))<=?1
         THEN summary_json ELSE NULL END
    FROM foreground_state WHERE id=1";

// The expression and ordering match the partial index below. SQLite can stop
// after the visible page instead of decoding and sorting the entire disk index.
static SUGGESTIONS_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "SELECT c.json FROM candidates c
    WHERE json_extract(c.json,'$.suggestion_eligible')=1
      AND json_extract(c.json,'$.blocked_reason') IS NULL
      AND json_extract(c.json,'$.allocated_bytes')>={}
      AND NOT EXISTS(SELECT 1 FROM kept k WHERE c.path=k.path
          OR substr(c.path,1,length(k.path)+1)=k.path||'/'
          OR substr(k.path,1,length(c.path)+1)=c.path||'/')
    ORDER BY {},
             json_extract(c.json,'$.allocated_bytes') DESC, c.path
    LIMIT 500",
        crate::recommendations::minimum_size_sql("c.json"),
        crate::recommendations::priority_sql("c.json")
    )
});

/// One page of the operations ledger, newest first. `?1` NULL starts at the
/// newest receipt; a `next_before` cursor value continues strictly older ones.
const HISTORY_PAGE_SQL: &str = "SELECT rowid, receipt_json FROM operations
    WHERE (?1 IS NULL OR rowid < ?1) ORDER BY rowid DESC LIMIT ?2";

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let conn = Connection::open(path).map_err(|e| e.to_string())?;
        conn.busy_timeout(Duration::from_secs(3))
            .map_err(|e| e.to_string())?;
        conn.execute_batch(&format!("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;
            PRAGMA wal_autocheckpoint={WAL_AUTOCHECKPOINT_PAGES}; PRAGMA journal_size_limit={WAL_SIZE_HINT_BYTES};
            CREATE TABLE IF NOT EXISTS roots(id TEXT PRIMARY KEY, path TEXT UNIQUE NOT NULL, json TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS candidates(id TEXT PRIMARY KEY, root_id TEXT NOT NULL, path TEXT UNIQUE NOT NULL, json TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS candidate_roots ON candidates(root_id);
            CREATE TABLE IF NOT EXISTS kept(path TEXT PRIMARY KEY);
            CREATE TABLE IF NOT EXISTS scans(root_id TEXT PRIMARY KEY, json TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS incomplete_roots(root_id TEXT PRIMARY KEY);
            CREATE TABLE IF NOT EXISTS operations(id TEXT PRIMARY KEY, root_json TEXT NOT NULL, candidate_json TEXT NOT NULL, receipt_json TEXT NOT NULL, stage_path TEXT, trash_identity TEXT, state TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS windows(id TEXT PRIMARY KEY, domain TEXT NOT NULL, before_bytes INTEGER NOT NULL, after_bytes INTEGER, private_bound INTEGER NOT NULL DEFAULT 0, credited_bytes INTEGER NOT NULL DEFAULT 0, state TEXT NOT NULL);
            CREATE UNIQUE INDEX IF NOT EXISTS open_window_domain ON windows(domain) WHERE state='open';
            CREATE TABLE IF NOT EXISTS allocations(operation_id TEXT PRIMARY KEY REFERENCES operations(id), window_id TEXT NOT NULL REFERENCES windows(id), bytes INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS earnings(operation_id TEXT PRIMARY KEY REFERENCES operations(id), coins INTEGER NOT NULL, collected INTEGER NOT NULL DEFAULT 0);
            CREATE TABLE IF NOT EXISTS wallet(id INTEGER PRIMARY KEY CHECK(id=1), collected INTEGER NOT NULL, remainder INTEGER NOT NULL, credited INTEGER NOT NULL);
            INSERT OR IGNORE INTO wallet VALUES(1,0,0,0);
            PRAGMA user_version=2;")).map_err(|e| e.to_string())?;
        conn.execute_batch(&format!(
            "PRAGMA cache_size=-8192;
            DROP INDEX IF EXISTS candidate_suggestions_v2;
            DROP INDEX IF EXISTS candidate_suggestions_v3;
            DROP INDEX IF EXISTS candidate_suggestions_v4;
            CREATE INDEX IF NOT EXISTS candidate_suggestions_v5 ON candidates(
                {},
                json_extract(json,'$.allocated_bytes') DESC, path)
                WHERE json_extract(json,'$.suggestion_eligible')=1
                  AND json_extract(json,'$.blocked_reason') IS NULL
                  AND json_extract(json,'$.allocated_bytes')>={};",
            crate::recommendations::priority_sql("json"),
            crate::recommendations::minimum_size_sql("json")
        ))
        .map_err(err)?;
        conn.execute_batch("CREATE INDEX IF NOT EXISTS candidate_paths ON candidates(root_id,path);
            CREATE TABLE IF NOT EXISTS index_version(id INTEGER PRIMARY KEY CHECK(id=1), version INTEGER NOT NULL);
            INSERT OR IGNORE INTO index_version VALUES(1,0);
            CREATE TABLE IF NOT EXISTS foreground_state(
                id INTEGER PRIMARY KEY CHECK(id=1),
                revision INTEGER NOT NULL CHECK(typeof(revision)='integer' AND revision>=0),
                summary_json TEXT,
                completed_at INTEGER);
            INSERT OR IGNORE INTO foreground_state(id,revision,summary_json,completed_at) VALUES(1,0,NULL,NULL);
            CREATE TABLE IF NOT EXISTS pending_scopes(root_id TEXT NOT NULL, path TEXT NOT NULL, PRIMARY KEY(root_id,path));
            CREATE TABLE IF NOT EXISTS active_scopes(root_id TEXT NOT NULL, path TEXT NOT NULL, PRIMARY KEY(root_id,path));
            CREATE TABLE IF NOT EXISTS refreshes(root_id TEXT PRIMARY KEY, scope TEXT, generation TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS refresh_seen(root_id TEXT NOT NULL, generation TEXT NOT NULL, candidate_id TEXT NOT NULL, seen INTEGER NOT NULL CHECK(seen IN (0,1)), PRIMARY KEY(root_id,generation,candidate_id));
            CREATE INDEX IF NOT EXISTS refresh_unseen ON refresh_seen(root_id,generation,seen,candidate_id);
            CREATE TABLE IF NOT EXISTS candidate_tombstones(id TEXT PRIMARY KEY, root_id TEXT, path TEXT);
            CREATE INDEX IF NOT EXISTS tombstone_paths ON candidate_tombstones(root_id,path);
            CREATE TABLE IF NOT EXISTS derived_usage(
                id INTEGER PRIMARY KEY CHECK(id=1),
                candidate_rows INTEGER NOT NULL CHECK(candidate_rows>=0),
                candidate_payload_bytes INTEGER NOT NULL CHECK(candidate_payload_bytes>=0));
            INSERT OR IGNORE INTO derived_usage VALUES(1,0,0);
            CREATE TABLE IF NOT EXISTS derived_admission_pressure(
                id INTEGER PRIMARY KEY CHECK(id=1),
                row_headroom INTEGER NOT NULL CHECK(row_headroom>=0),
                byte_headroom INTEGER NOT NULL CHECK(byte_headroom>=0));
            INSERT OR IGNORE INTO derived_admission_pressure VALUES(1,0,0);
            CREATE TABLE IF NOT EXISTS derived_usage_version(
                id INTEGER PRIMARY KEY CHECK(id=1), version INTEGER NOT NULL);
            INSERT OR IGNORE INTO derived_usage_version VALUES(1,0);
            CREATE TABLE IF NOT EXISTS derived_limited_roots(
                root_id TEXT PRIMARY KEY,
                omitted_rows INTEGER NOT NULL CHECK(omitted_rows>=0),
                omitted_bytes INTEGER NOT NULL CHECK(omitted_bytes>=0),
                reason TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS refresh_admission(
                root_id TEXT NOT NULL,
                generation TEXT NOT NULL,
                omitted_rows INTEGER NOT NULL CHECK(omitted_rows>=0),
                omitted_bytes INTEGER NOT NULL CHECK(omitted_bytes>=0),
                reason TEXT NOT NULL,
                PRIMARY KEY(root_id,generation));
            CREATE TRIGGER IF NOT EXISTS candidates_usage_insert AFTER INSERT ON candidates BEGIN
                UPDATE derived_usage SET candidate_rows=candidate_rows+1,
                    candidate_payload_bytes=candidate_payload_bytes+
                        length(CAST(NEW.json AS BLOB))+length(CAST(NEW.path AS BLOB))+64
                    WHERE id=1;
            END;
            CREATE TRIGGER IF NOT EXISTS candidates_usage_delete AFTER DELETE ON candidates BEGIN
                UPDATE derived_usage SET candidate_rows=candidate_rows-1,
                    candidate_payload_bytes=candidate_payload_bytes-
                        length(CAST(OLD.json AS BLOB))-length(CAST(OLD.path AS BLOB))-64
                    WHERE id=1;
            END;
            CREATE TRIGGER IF NOT EXISTS candidates_usage_update AFTER UPDATE OF path,json ON candidates
                WHEN OLD.path != NEW.path OR OLD.json != NEW.json BEGIN
                UPDATE derived_usage SET candidate_payload_bytes=candidate_payload_bytes-
                        length(CAST(OLD.json AS BLOB))-length(CAST(OLD.path AS BLOB))-64+
                        length(CAST(NEW.json AS BLOB))+length(CAST(NEW.path AS BLOB))+64
                    WHERE id=1;
            END;").map_err(err)?;
        {
            let version: u32 = conn
                .query_row(
                    "SELECT version FROM derived_usage_version WHERE id=1",
                    [],
                    |r| r.get(0),
                )
                .map_err(err)?;
            if version != DERIVED_USAGE_VERSION {
                let tx = conn.unchecked_transaction().map_err(err)?;
                recount_derived_usage_in(&tx)?;
                tx.execute(
                    "UPDATE derived_usage_version SET version=?1 WHERE id=1",
                    [DERIVED_USAGE_VERSION],
                )
                .map_err(err)?;
                tx.commit().map_err(err)?;
            }
        }
        let version: u32 = conn
            .query_row("SELECT version FROM index_version WHERE id=1", [], |r| {
                r.get(0)
            })
            .map_err(err)?;
        if version != RULE_VERSION {
            let tx = conn.unchecked_transaction().map_err(err)?;
            // Classification changes rebuild only the derived index. Event receipt
            // may already have a cursor, so the rebuild itself must be durable.
            tx.execute_batch(
                "DELETE FROM candidates; DELETE FROM pending_scopes;
                INSERT INTO pending_scopes SELECT id,path FROM roots;",
            )
            .map_err(err)?;
            tx.execute(
                "INSERT OR IGNORE INTO incomplete_roots SELECT id FROM roots",
                [],
            )
            .map_err(err)?;
            tx.execute(
                "UPDATE index_version SET version=?1 WHERE id=1",
                [RULE_VERSION],
            )
            .map_err(err)?;
            invalidate_foreground_summary_in(&tx)?;
            tx.commit().map_err(err)?;
        }
        let mut store = Self {
            conn,
            visible_candidates: RefCell::new(None),
            database_path: path.to_path_buf(),
            derived_limits: DerivedLimits::default(),
        };
        store.recover_discovery()?;
        Ok(store)
    }

    #[cfg(test)]
    pub(crate) fn with_derived_limits(mut self, limits: DerivedLimits) -> Self {
        self.derived_limits = limits;
        self
    }

    /// Return O(1) derived counters together with physical SQLite side-file
    /// sizes. Durable tables are deliberately excluded from these counters.
    pub(crate) fn derived_usage(&self) -> Result<DerivedUsage> {
        let (candidate_rows, candidate_payload_bytes): (u64, u64) = self
            .conn
            .query_row(
                "SELECT candidate_rows,candidate_payload_bytes FROM derived_usage WHERE id=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(err)?;
        let wal = self.wal_sizes()?;
        Ok(DerivedUsage {
            candidate_rows,
            candidate_payload_bytes,
            database_bytes: wal.0,
            wal_bytes: wal.1,
            wal_shm_bytes: wal.2,
            ..Default::default()
        })
    }

    /// A non-blocking checkpoint for maintenance telemetry. PASSIVE never
    /// waits for readers; `wal_busy` reports that a later truncate is needed.
    pub(crate) fn passive_checkpoint(&self) -> Result<DerivedUsage> {
        self.checkpoint_wal("PASSIVE")
    }

    /// TRUNCATE is only for an explicit idle maintenance call. It is never
    /// part of discovery, cleanup, accounting, or crash recovery.
    pub(crate) fn truncate_wal(&self) -> Result<DerivedUsage> {
        self.checkpoint_wal("TRUNCATE")
    }

    fn wal_sizes(&self) -> Result<(u64, u64, u64)> {
        fn size(path: PathBuf) -> Result<u64> {
            match std::fs::metadata(path) {
                Ok(metadata) => Ok(metadata.len()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
                Err(error) => Err(error.to_string()),
            }
        }
        let database = size(self.database_path.clone())?;
        let mut wal = self.database_path.as_os_str().to_os_string();
        wal.push("-wal");
        let mut shm = self.database_path.as_os_str().to_os_string();
        shm.push("-shm");
        Ok((
            database,
            size(PathBuf::from(wal))?,
            size(PathBuf::from(shm))?,
        ))
    }

    fn checkpoint_wal(&self, mode: &str) -> Result<DerivedUsage> {
        let old_timeout: i64 = self
            .conn
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .map_err(err)?;
        self.conn.busy_timeout(Duration::ZERO).map_err(err)?;
        let checkpoint =
            self.conn
                .query_row(&format!("PRAGMA wal_checkpoint({mode})"), [], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                });
        let restore = self
            .conn
            .busy_timeout(Duration::from_millis(old_timeout.max(0) as u64));
        restore.map_err(err)?;
        let (busy, frames, checkpointed) = match checkpoint {
            Ok((busy, frames, checkpointed)) => (busy != 0, frames, checkpointed),
            Err(error) if error.to_string().contains("locked") => (true, 0, 0),
            Err(error) => return Err(err(error)),
        };
        let (database_bytes, wal_bytes, wal_shm_bytes) = self.wal_sizes()?;
        let (candidate_rows, candidate_payload_bytes): (u64, u64) = self
            .conn
            .query_row(
                "SELECT candidate_rows,candidate_payload_bytes FROM derived_usage WHERE id=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(err)?;
        Ok(DerivedUsage {
            candidate_rows,
            candidate_payload_bytes,
            database_bytes,
            wal_bytes,
            wal_shm_bytes,
            wal_frames: frames.max(0) as u64,
            wal_checkpointed: checkpointed.max(0) as u64,
            wal_busy: busy,
        })
    }

    /// Recover abandoned read claims after process exit or after the sole
    /// coordinator has stopped and discarded every helper receiver. Never call
    /// this while another reader still has publication authority. No findings
    /// are pruned, and a failed transaction leaves the replay journal intact.
    pub(crate) fn recover_discovery(&mut self) -> Result<()> {
        let tx = self.conn.transaction().map_err(err)?;
        loop {
            let active: Option<(String, String)> = tx
                .query_row(
                    "SELECT a.root_id,a.path FROM active_scopes a JOIN roots r ON r.id=a.root_id LIMIT 1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(err)?;
            let Some((root, path)) = active else { break };
            enqueue_scope_in(&tx, &root, Path::new(&path))?;
            tx.execute("INSERT OR IGNORE INTO incomplete_roots VALUES(?1)", [&root])
                .map_err(err)?;
            // The persistent derived_limited_roots warning survives restart;
            // only this abandoned generation's temporary marker is discarded.
            tx.execute("DELETE FROM refresh_admission WHERE root_id=?1", [&root])
                .map_err(err)?;
            tx.execute(
                "DELETE FROM active_scopes WHERE root_id=?1 AND path=?2",
                params![root, path],
            )
            .map_err(err)?;
        }
        // A failed scanner can leave a refresh marker even after its queue item
        // was released. Recover that scope too, without pruning unseen rows.
        loop {
            let refresh: Option<(String, String)> = tx
                .query_row(
                    "SELECT f.root_id,coalesce(f.scope,r.path) FROM refreshes f JOIN roots r ON r.id=f.root_id LIMIT 1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(err)?;
            let Some((root, path)) = refresh else { break };
            enqueue_scope_in(&tx, &root, Path::new(&path))?;
            tx.execute("INSERT OR IGNORE INTO incomplete_roots VALUES(?1)", [&root])
                .map_err(err)?;
            tx.execute("DELETE FROM refresh_admission WHERE root_id=?1", [&root])
                .map_err(err)?;
            tx.execute("DELETE FROM refreshes WHERE root_id=?1", [&root])
                .map_err(err)?;
        }
        tx.execute_batch(
            "DELETE FROM active_scopes;
             DELETE FROM refreshes;
             DELETE FROM refresh_seen;
             DELETE FROM refresh_admission;
             DELETE FROM pending_scopes WHERE NOT EXISTS(SELECT 1 FROM roots WHERE id=pending_scopes.root_id);",
        )
        .map_err(err)?;
        tx.commit().map_err(err)
    }

    pub fn reconcile(&mut self) -> Result<()> {
        // Never replay a filesystem mutation or invent a missing observation after a crash.
        let mut unfinished = Vec::new();
        {
            let mut query = self.conn.prepare("SELECT id,receipt_json,stage_path FROM operations WHERE state IN ('prepared','mutating','restoring')").map_err(err)?;
            for row in query
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
                    ))
                })
                .map_err(err)?
            {
                unfinished.push(row.map_err(err)?);
            }
        }
        for (id, json, stage) in unfinished {
            let mut receipt: Receipt = serde_json::from_str(&json).map_err(err)?;
            receipt.outcome = "interrupted".into();
            let prepared_detail = std::mem::take(&mut receipt.detail);
            receipt.detail = "Interrupted before a durable result. No deletion was replayed and no space was credited. Inspect the original location before trying again.".into();
            if !prepared_detail.is_empty() {
                receipt.detail.push(' ');
                receipt.detail.push_str(&prepared_detail);
            }
            if let Some(p) = stage {
                receipt
                    .detail
                    .push_str(&format!(" A staged item may remain at {p}."));
            }
            receipt.can_restore = false;
            self.conn
                .execute(
                    "UPDATE operations SET state='interrupted',receipt_json=?2 WHERE id=?1",
                    params![id, serde_json::to_string(&receipt).map_err(err)?],
                )
                .map_err(err)?;
        }
        self.conn
            .execute(
                "UPDATE windows SET state='interrupted' WHERE state='open'",
                [],
            )
            .map_err(err)?;
        Ok(())
    }

    pub fn roots(&self) -> Result<Vec<Root>> {
        self.json_rows("SELECT json FROM roots ORDER BY path")
    }

    /// Reconcile an event-history loss boundary atomically. The caller must
    /// have created `event_cursor` during engine initialization. Every root is
    /// re-enqueued before the cursor is advanced, so a failed queue write
    /// leaves the old cursor intact and the event source will retry.
    pub(crate) fn reconcile_events(&mut self, cursor: u64) -> Result<()> {
        // Capture grants before opening the write transaction; this avoids
        // holding a mutable transaction while deserializing arbitrary roots.
        let roots = self.roots()?;
        let tx = self.conn.transaction().map_err(err)?;
        for root in roots {
            enqueue_scope_in(&tx, &root.id, &root.path)?;
        }
        let updated = tx
            .execute("UPDATE event_cursor SET cursor=?1 WHERE id=1", [cursor])
            .map_err(err)?;
        if updated != 1 {
            return Err("The durable event cursor row is missing".into());
        }
        tx.commit().map_err(err)
    }

    pub fn root(&self, id: &str) -> Result<Root> {
        let json: String = self
            .conn
            .query_row("SELECT json FROM roots WHERE id=?1", [id], |r| r.get(0))
            .map_err(err)?;
        serde_json::from_str(&json).map_err(err)
    }

    /// Grant/rule context for a foreground request. Callers also guard their
    /// in-process request generation before saving a terminal result.
    pub fn foreground_context(&self) -> Result<i64> {
        let revision: i64 = self
            .conn
            .query_row(
                "SELECT revision FROM foreground_state WHERE id=1",
                [],
                |row| row.get(0),
            )
            .map_err(err)?;
        if revision < 0 {
            return Err("The foreground summary context is invalid".into());
        }
        Ok(revision)
    }

    /// Optional presentation cache, never coverage or authorization evidence.
    /// Invalid cache content is ignored without changing the library; actual
    /// SQLite read errors still propagate to the caller.
    pub fn load_foreground_summary(&self) -> Result<Option<ForegroundScan>> {
        let (revision, scan) = self
            .conn
            .query_row(
                FOREGROUND_STATE_SQL,
                [MAX_FOREGROUND_SUMMARY_BYTES as i64],
                |row| {
                    let scan = match row.get_ref(1)? {
                        rusqlite::types::ValueRef::Text(bytes)
                            if bytes.len() <= MAX_FOREGROUND_SUMMARY_BYTES =>
                        {
                            serde_json::from_slice::<ForegroundScan>(bytes)
                                .ok()
                                .filter(|scan| validate_foreground_summary(scan).is_ok())
                        }
                        _ => None,
                    };
                    Ok((row.get::<_, i64>(0)?, scan))
                },
            )
            .map_err(err)?;
        if revision < 0 {
            return Err("The foreground summary context is invalid".into());
        }
        Ok(scan)
    }

    /// A stale grant/rule context cannot overwrite the current summary. An
    /// identical repeat is accepted without another update or completion time.
    pub fn save_foreground_summary(
        &mut self,
        expected_context: i64,
        scan: &ForegroundScan,
    ) -> Result<bool> {
        validate_foreground_summary(scan)?;
        if expected_context < 0 {
            return Err("The foreground summary context is invalid".into());
        }
        if scan.stats.message.len() > MAX_FOREGROUND_SUMMARY_BYTES {
            return Err("The foreground summary exceeds 64 KiB".into());
        }
        let summary = serde_json::to_string(scan).map_err(err)?;
        if summary.len() > MAX_FOREGROUND_SUMMARY_BYTES {
            return Err("The foreground summary exceeds 64 KiB".into());
        }
        let tx = self.conn.transaction().map_err(err)?;
        let (revision, unchanged) = tx
            .query_row(
                FOREGROUND_STATE_SQL,
                [MAX_FOREGROUND_SUMMARY_BYTES as i64],
                |row| {
                    let unchanged = matches!(row.get_ref(1)?, rusqlite::types::ValueRef::Text(previous) if previous == summary.as_bytes());
                    Ok((row.get::<_, i64>(0)?, unchanged))
                },
            )
            .map_err(err)?;
        if revision < 0 {
            return Err("The foreground summary context is invalid".into());
        }
        if revision != expected_context {
            return Ok(false);
        }
        if !unchanged {
            let changed = tx.execute(
                "UPDATE foreground_state SET summary_json=?1,completed_at=?2 WHERE id=1 AND revision=?3",
                params![summary, now(), expected_context],
            ).map_err(err)?;
            if changed != 1 {
                return Err("The foreground summary context changed during its save".into());
            }
        }
        tx.commit().map_err(err)?;
        Ok(true)
    }

    pub fn add_root(&mut self, root: &Root) -> Result<()> {
        self.authorize_root(root, false).map(|_| ())
    }

    /// An explicit broader authorization replaces its contained scan roots atomically.
    /// History, kept paths and the reward ledger retain their original identities.
    pub fn authorize_root(&mut self, root: &Root, replace_contained: bool) -> Result<Vec<String>> {
        let mut contained = Vec::new();
        let mut narrowing_home = false;
        for existing in self.roots()? {
            // Reconfirming a legacy Home grant narrows its media policy. Replace
            // only the same physical grant, in the existing atomic transaction;
            // old candidate rows must not remain actionable during the refresh.
            if replace_contained
                && root.path == existing.path
                && root.id == existing.id
                && root.identity.device == existing.identity.device
                && root.identity.inode == existing.identity.inode
                && existing.kind == "folder"
                && root.kind == "home"
            {
                contained.push(existing.id);
                narrowing_home = true;
                continue;
            }
            if root.path.starts_with(&existing.path) {
                return Err("This folder overlaps an already authorized location.".into());
            }
            if existing.path.starts_with(&root.path) {
                // Traversal stays on one filesystem. A separately authorized mount
                // beneath the new root must remain an independent scan location.
                if existing.identity.device != root.identity.device {
                    continue;
                }
                if !replace_contained {
                    return Err("This folder overlaps an already authorized location.".into());
                }
                contained.push(existing.id);
            }
        }
        let tx = self.conn.transaction().map_err(err)?;
        for id in &contained {
            remove_discovery_for_root(&tx, id)?;
            tx.execute("DELETE FROM candidates WHERE root_id=?1", [id])
                .map_err(err)?;
            tx.execute("DELETE FROM scans WHERE root_id=?1", [id])
                .map_err(err)?;
            tx.execute("DELETE FROM incomplete_roots WHERE root_id=?1", [id])
                .map_err(err)?;
            tx.execute("DELETE FROM derived_limited_roots WHERE root_id=?1", [id])
                .map_err(err)?;
            tx.execute("DELETE FROM refresh_admission WHERE root_id=?1", [id])
                .map_err(err)?;
            tx.execute("DELETE FROM roots WHERE id=?1", [id])
                .map_err(err)?;
        }
        tx.execute(
            "INSERT INTO roots VALUES(?1,?2,?3)",
            params![
                root.id,
                root.path.to_string_lossy(),
                serde_json::to_string(root).map_err(err)?
            ],
        )
        .map_err(err)?;
        tx.execute(
            "INSERT OR IGNORE INTO incomplete_roots VALUES(?1)",
            [&root.id],
        )
        .map_err(err)?;
        if narrowing_home {
            // The replacement scan belongs to the same commit as invalidation.
            // A restart between authorization and the native resume must not
            // leave the narrowed grant empty with no durable work to finish.
            enqueue_scope_in(&tx, &root.id, &root.path)?;
        }
        invalidate_foreground_summary_in(&tx)?;
        tx.commit().map_err(err)?;
        self.invalidate_candidates();
        Ok(contained)
    }
    pub fn remove_root(&mut self, id: &str) -> Result<bool> {
        self.invalidate_candidates();
        let tx = self.conn.transaction().map_err(err)?;
        remove_discovery_for_root(&tx, id)?;
        tx.execute("DELETE FROM candidates WHERE root_id=?1", [id])
            .map_err(err)?;
        tx.execute("DELETE FROM scans WHERE root_id=?1", [id])
            .map_err(err)?;
        tx.execute("DELETE FROM incomplete_roots WHERE root_id=?1", [id])
            .map_err(err)?;
        tx.execute("DELETE FROM derived_limited_roots WHERE root_id=?1", [id])
            .map_err(err)?;
        tx.execute("DELETE FROM refresh_admission WHERE root_id=?1", [id])
            .map_err(err)?;
        let removed = tx
            .execute("DELETE FROM roots WHERE id=?1", [id])
            .map_err(err)?;
        if removed != 0 {
            invalidate_foreground_summary_in(&tx)?;
        }
        tx.commit().map_err(err)?;
        Ok(removed != 0)
    }

    /// Persist a refresh request before acknowledging its filesystem event.
    /// Pending ancestors subsume descendants; an active traversal never does.
    pub fn enqueue_scope(&mut self, root: &str, path: &Path) -> Result<()> {
        let tx = self.conn.transaction().map_err(err)?;
        enqueue_scope_in(&tx, root, path)?;
        tx.commit().map_err(err)
    }

    /// One durable commit per bounded bridge batch, rather than one fsync per path.
    pub fn enqueue_scopes(&mut self, root: &str, paths: &[PathBuf]) -> Result<()> {
        if paths.len() > 512 {
            return Err("Too many event scopes in one batch".into());
        }
        if let [path] = paths {
            return self.enqueue_scope(root, path);
        }
        let tx = self.conn.transaction().map_err(err)?;
        if paths.is_empty() {
            return tx.commit().map_err(err);
        }
        let root_path: String = tx
            .query_row("SELECT path FROM roots WHERE id=?1", [root], |r| r.get(0))
            .map_err(err)?;
        let mut scopes = Vec::with_capacity(paths.len());
        for (order, path) in paths.iter().enumerate() {
            // Validate every input before pruning. A valid ancestor must never
            // hide a malformed or unauthorized descendant in the same batch.
            let path = scope_path(path)?;
            if !Path::new(&path).starts_with(&root_path) {
                return Err("The discovery scope is outside its authorized location".into());
            }
            scopes.push((order, path));
        }
        // Component ordering keeps descendants beside their ancestor, unlike
        // string ordering where siblings such as `a-other` can intervene.
        scopes.sort_unstable_by(|(left_order, left), (right_order, right)| {
            Path::new(left)
                .cmp(Path::new(right))
                .then(left_order.cmp(right_order))
        });
        scopes.dedup_by(|(_, path), (_, previous)| {
            Path::new(path.as_str()).starts_with(Path::new(previous.as_str()))
        });
        // Intermediate descendant inserts would be removed before this commit.
        // Keep the first occurrence of each surviving scope to preserve FIFO.
        scopes.sort_unstable_by_key(|(order, _)| *order);
        for (_, path) in scopes {
            enqueue_validated_scope_in(&tx, root, &root_path, &path)?;
        }
        tx.commit().map_err(err)
    }

    pub fn has_pending_scopes(&self) -> Result<bool> {
        self.conn
            .query_row("SELECT EXISTS(SELECT 1 FROM pending_scopes)", [], |r| {
                r.get(0)
            })
            .map_err(err)
    }

    pub(crate) fn has_active_scopes(&self) -> Result<bool> {
        self.conn
            .query_row("SELECT EXISTS(SELECT 1 FROM active_scopes)", [], |row| {
                row.get(0)
            })
            .map_err(err)
    }

    /// Claim one durable work item. Only one scanner owns an active scope; a
    /// second claim leaves pending work untouched until the first is finished.
    pub fn take_scope(&mut self) -> Result<Option<(String, PathBuf)>> {
        self.take_scope_bounded(1)
    }

    /// Bounded parallel readers may own distinct roots, never two generations
    /// for the same root. Root authorization already excludes same-volume
    /// overlapping grants; separately granted mounts remain separate readers.
    pub(crate) fn take_scope_bounded(&mut self, limit: usize) -> Result<Option<(String, PathBuf)>> {
        if !(1..=2).contains(&limit) {
            return Err("Invalid read-only discovery admission limit".into());
        }
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(err)?;
        let next: Option<(String, String)> = tx
            .query_row(
                "SELECT p.root_id,p.path FROM pending_scopes p
                 WHERE (SELECT count(*) FROM active_scopes) < ?1
                   AND NOT EXISTS(SELECT 1 FROM active_scopes a WHERE a.root_id=p.root_id)
                 ORDER BY p.rowid LIMIT 1",
                [limit as u32],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(err)?;
        let Some((root, path)) = next else {
            return Ok(None);
        };
        tx.execute(
            "INSERT INTO active_scopes VALUES(?1,?2)",
            params![root, path],
        )
        .map_err(err)?;
        tx.execute(
            "DELETE FROM pending_scopes WHERE root_id=?1 AND path=?2",
            params![root, path],
        )
        .map_err(err)?;
        tx.commit().map_err(err)?;
        Ok(Some((root, PathBuf::from(path))))
    }

    pub fn finish_scope(&mut self, root: &str, path: &Path) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM active_scopes WHERE root_id=?1 AND path=?2",
                params![root, scope_path(path)?],
            )
            .map_err(err)?;
        Ok(())
    }

    /// Scope resolution can widen a file event to its artifact or parent. Drop
    /// already-queued work covered by that pass before it starts; later events
    /// remain pending, and the claimed scope stays active for crash recovery.
    pub fn discard_pending_scope(&mut self, root: &str, scope: &Path) -> Result<()> {
        let path = scope_path(scope)?;
        let tx = self.conn.transaction().map_err(err)?;
        discard_pending_scope_in(&tx, root, &path)?;
        tx.commit().map_err(err)
    }

    /// Keep the last completed result visible while its replacement is measured.
    /// The disk-backed generation tracks absence without loading old rows in RAM.
    pub fn begin_refresh(&mut self, root: &str, scope: Option<&Path>) -> Result<()> {
        let scope = scope.map(scope_path).transpose()?;
        let generation = unique_id();
        let tx = self.conn.transaction().map_err(err)?;
        begin_refresh_in(&tx, root, scope.as_deref(), &generation)?;
        tx.commit().map_err(err)
    }

    pub fn finish_refresh(
        &mut self,
        root: &str,
        scope: Option<&Path>,
        complete: bool,
    ) -> Result<()> {
        let scope = scope.map(scope_path).transpose()?;
        let tx = self.conn.transaction().map_err(err)?;
        let generation: Option<String> = tx
            .query_row(
                "SELECT generation FROM refreshes WHERE root_id=?1 AND scope IS ?2",
                params![root, scope],
                |r| r.get(0),
            )
            .optional()
            .map_err(err)?;
        let Some(generation) = generation else {
            return Err("The completed refresh does not match its active scope".into());
        };
        let removed = finish_refresh_in(&tx, root, &generation, complete)?;
        tx.commit().map_err(err)?;
        if removed != 0 {
            self.invalidate_candidates();
        }
        Ok(())
    }

    /// Called at the traversal/adoption boundary while runtime and store are
    /// locked. The claim predates this transaction; later events remain queued.
    pub(crate) fn begin_scope_refresh(
        &mut self,
        root: &Root,
        claimed: &Path,
        scope: Option<&Path>,
    ) -> Result<ScopeRefresh> {
        self.begin_scope_refresh_with_sibling(root, claimed, scope, None)
    }

    /// The exact lock path is the durable replay marker. Its refresh covers
    /// only that subtree and its immediate target sibling, never the project.
    /// The scanner must prove both footprints before reporting completion.
    pub(crate) fn begin_cargo_lock_refresh(
        &mut self,
        root: &Root,
        origin: &Path,
    ) -> Result<ScopeRefresh> {
        let origin = scope_path(origin)?;
        let origin = Path::new(&origin);
        let target = crate::refresh::cargo_lock_target(root, origin)?
            .ok_or("A Cargo lock refresh requires an eligible exact descendant Cargo.lock scope")?;
        let parent = origin
            .parent()
            .ok_or("The Cargo lock scope has no parent")?;
        // An equal-origin download row belongs to this footprint. A strictly
        // enclosing indexed artifact must instead retain its ordinary refresh.
        if self.enclosing_candidate(root, parent)?.is_some() {
            return Err(
                "An enclosing indexed candidate requires its ordinary subtree refresh".into(),
            );
        }
        self.begin_scope_refresh_with_sibling(root, origin, Some(origin), Some(&target))
    }

    fn begin_scope_refresh_with_sibling(
        &mut self,
        root: &Root,
        claimed: &Path,
        scope: Option<&Path>,
        sibling: Option<&Path>,
    ) -> Result<ScopeRefresh> {
        let resolved = scope.unwrap_or(&root.path);
        if !resolved.starts_with(&root.path) || !claimed.starts_with(resolved) {
            return Err("The refresh scope does not cover its authorized claim".into());
        }
        let claimed = scope_path(claimed)?;
        let resolved = scope_path(resolved)?;
        let scope = scope.map(scope_path).transpose()?;
        let sibling = sibling.map(scope_path).transpose()?;
        let generation = unique_id();
        let tx = self.conn.transaction().map_err(err)?;
        let active: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM active_scopes WHERE root_id=?1 AND path=?2)",
                params![root.id, claimed],
                |row| row.get(0),
            )
            .map_err(err)?;
        if !active {
            return Err("The refresh has no matching durable scope claim".into());
        }
        let was_incomplete: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM incomplete_roots WHERE root_id=?1)",
                [&root.id],
                |row| row.get(0),
            )
            .map_err(err)?;
        tx.execute(
            "INSERT OR IGNORE INTO incomplete_roots VALUES(?1)",
            [&root.id],
        )
        .map_err(err)?;
        discard_pending_scope_in(&tx, &root.id, &resolved)?;
        if let Some(sibling) = sibling {
            discard_pending_scope_in(&tx, &root.id, &sibling)?;
            initialize_refresh_in(&tx, &root.id, scope.as_deref(), &generation)?;
            capture_scope_in(&tx, &root.id, &resolved, &generation, false)?;
            capture_scope_in(&tx, &root.id, &sibling, &generation, false)?;
        } else {
            begin_refresh_in(&tx, &root.id, scope.as_deref(), &generation)?;
        }
        tx.commit().map_err(err)?;
        Ok(ScopeRefresh {
            root: root.id.clone(),
            claimed,
            scope,
            generation,
            was_incomplete,
        })
    }

    /// Commit the result and acknowledge its claim together. Any failure rolls
    /// back pruning, statistics, coverage, requeue, and acknowledgement together.
    pub(crate) fn finish_scope_refresh(
        &mut self,
        refresh: &ScopeRefresh,
        stats: &ScanStats,
        requeue: bool,
    ) -> Result<()> {
        if stats.complete && (stats.cancelled || stats.errors != 0) {
            return Err("Incomplete discovery cannot commit complete coverage".into());
        }
        let tx = self.conn.transaction().map_err(err)?;
        let matches: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM refreshes WHERE root_id=?1 AND scope IS ?2 AND generation=?3)",
            params![refresh.root, refresh.scope, refresh.generation], |row| row.get(0),
        ).map_err(err)?;
        if !matches {
            return Err("The completed refresh does not match its active generation".into());
        }
        let limited: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM refresh_admission
                 WHERE root_id=?1 AND generation=?2)",
                params![refresh.root, refresh.generation],
                |row| row.get(0),
            )
            .map_err(err)?;
        let complete = stats.complete && !limited;
        let removed = finish_refresh_in(&tx, &refresh.root, &refresh.generation, complete)?;
        save_stats_in(&tx, &refresh.root, stats)?;
        tx.execute(
            "DELETE FROM refresh_admission WHERE root_id=?1 AND generation=?2",
            params![refresh.root, refresh.generation],
        )
        .map_err(err)?;
        if complete && (refresh.scope.is_none() || !refresh.was_incomplete) {
            tx.execute(
                "DELETE FROM incomplete_roots WHERE root_id=?1",
                [&refresh.root],
            )
            .map_err(err)?;
            // A successful full pass is the only operation that clears a
            // persistent admission warning. Scoped passes cannot prove that
            // omitted observations elsewhere in this root are now present.
            if refresh.scope.is_none() {
                tx.execute(
                    "DELETE FROM derived_limited_roots WHERE root_id=?1",
                    [&refresh.root],
                )
                .map_err(err)?;
            }
        }
        if requeue {
            enqueue_scope_in(&tx, &refresh.root, Path::new(&refresh.claimed))?;
        }
        acknowledge_scope_in(&tx, &refresh.root, &refresh.claimed)?;
        tx.commit().map_err(err)?;
        if removed != 0 {
            self.invalidate_candidates();
        }
        Ok(())
    }

    /// Cancellation before begin-refresh still leaves incomplete coverage and
    /// durable replay work, with no gap between requeue and acknowledgement.
    #[cfg(test)]
    pub(crate) fn cancel_claimed_scope(&mut self, root: &str, claimed: &Path) -> Result<()> {
        let claimed = scope_path(claimed)?;
        let tx = self.conn.transaction().map_err(err)?;
        tx.execute("INSERT OR IGNORE INTO incomplete_roots VALUES(?1)", [root])
            .map_err(err)?;
        enqueue_scope_in(&tx, root, Path::new(&claimed))?;
        acknowledge_scope_in(&tx, root, &claimed)?;
        tx.commit().map_err(err)
    }

    pub fn clear_scope(&mut self, root: &str, scope: Option<&Path>) -> Result<()> {
        self.invalidate_candidates();
        if let Some(scope) = scope {
            let path = scope
                .to_str()
                .ok_or("This path cannot be represented in the native interface")?;
            let tx = self.conn.transaction().map_err(err)?;
            // '/' sorts immediately before '0' in UTF-8. This exact binary range
            // does not interpret '%' or '_' as wildcard characters.
            tx.execute(
                "DELETE FROM candidates WHERE root_id=?1 AND path>=?2 AND path<?3",
                params![root, format!("{path}/"), format!("{path}0")],
            )
            .map_err(err)?;
            for ancestor in scope.ancestors() {
                tx.execute(
                    "DELETE FROM candidates WHERE root_id=?1 AND path=?2",
                    params![root, ancestor.to_string_lossy()],
                )
                .map_err(err)?;
            }
            tx.commit().map_err(err)
        } else {
            self.conn
                .execute("DELETE FROM candidates WHERE root_id=?1", [root])
                .map_err(err)?;
            Ok(())
        }
    }
    pub fn save_batch(&mut self, batch: &ScanBatch) -> Result<()> {
        if batch.candidates.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction().map_err(err)?;
        let (mut candidate_rows, mut candidate_payload_bytes): (u64, u64) = tx
            .query_row(
                "SELECT candidate_rows,candidate_payload_bytes FROM derived_usage WHERE id=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(err)?;
        let mut changed = false;
        {
            let mut remove_path_conflict = tx
                .prepare_cached(
                    "DELETE FROM candidates
                     WHERE path=?1 AND id!=?2
                       AND NOT EXISTS(SELECT 1 FROM candidate_tombstones WHERE id=?2)",
                )
                .map_err(err)?;
            let mut insert = tx
                .prepare_cached(
                    "INSERT INTO candidates(id,root_id,path,json)
                     SELECT ?1,?2,?3,?4
                     WHERE NOT EXISTS(SELECT 1 FROM candidate_tombstones WHERE id=?1)
                     ON CONFLICT(id) DO UPDATE SET
                       root_id=excluded.root_id,path=excluded.path,json=excluded.json",
                )
                .map_err(err)?;
            let mut existing = tx
                .prepare_cached("SELECT path,json FROM candidates WHERE id=?1 OR path=?2")
                .map_err(err)?;
            let mut seen = tx
                .prepare_cached(
                    "INSERT OR REPLACE INTO refresh_seen
                     SELECT root_id,generation,?1,1 FROM refreshes WHERE root_id=?2",
                )
                .map_err(err)?;
            let mut locate_tombstone = tx
                .prepare_cached(
                    "UPDATE candidate_tombstones SET root_id=?2,path=?3 WHERE id=?1 AND root_id IS NULL",
                )
                .map_err(err)?;
            let mut limited = tx
                .prepare_cached(
                    "INSERT INTO refresh_admission(root_id,generation,omitted_rows,omitted_bytes,reason)
                     VALUES(?1,?2,?3,?4,?5)
                     ON CONFLICT(root_id,generation) DO UPDATE SET
                       omitted_rows=omitted_rows+excluded.omitted_rows,
                       omitted_bytes=omitted_bytes+excluded.omitted_bytes,
                       reason=excluded.reason",
                )
                .map_err(err)?;
            let mut root_limited = tx
                .prepare_cached(
                    "INSERT INTO derived_limited_roots(root_id,omitted_rows,omitted_bytes,reason)
                     VALUES(?1,?2,?3,?4)
                     ON CONFLICT(root_id) DO UPDATE SET
                       omitted_rows=omitted_rows+excluded.omitted_rows,
                       omitted_bytes=omitted_bytes+excluded.omitted_bytes,
                       reason=excluded.reason",
                )
                .map_err(err)?;
            let mut mark_incomplete = tx
                .prepare_cached("INSERT OR IGNORE INTO incomplete_roots VALUES(?1)")
                .map_err(err)?;
            for c in &batch.candidates {
                let path = scope_path(&c.path)?;
                // A defensive suppression of an already-missing ID can acquire
                // its scope from a late batch, while still rejecting that batch.
                locate_tombstone
                    .execute(params![c.id, c.root_id, path])
                    .map_err(err)?;
                if c.provisional {
                    seen.execute(params![c.id, c.root_id]).map_err(err)?;
                    continue;
                }
                let json = serde_json::to_string(c).map_err(err)?;
                let weight = candidate_payload_weight(json.as_bytes(), Path::new(&path))?;
                let mut old_rows = 0u64;
                let mut old_weight = 0u64;
                let mut unchanged = false;
                let mut rows = existing.query(params![c.id, path]).map_err(err)?;
                while let Some(row) = rows.next().map_err(err)? {
                    let old_path: String = row.get(0).map_err(err)?;
                    let old_json: String = row.get(1).map_err(err)?;
                    old_rows = old_rows.saturating_add(1);
                    old_weight = old_weight
                        .checked_add(candidate_payload_weight(
                            old_json.as_bytes(),
                            Path::new(&old_path),
                        )?)
                        .ok_or("Derived candidate payload overflow")?;
                    unchanged |= old_path == path && old_json.as_bytes() == json.as_bytes();
                }
                let projected_rows = candidate_rows.saturating_sub(old_rows).saturating_add(1);
                let projected_bytes = candidate_payload_bytes
                    .saturating_sub(old_weight)
                    .checked_add(weight)
                    .ok_or("Derived candidate payload overflow")?;
                // An unchanged row consumes no additional budget and must be
                // acknowledged by the active refresh even when retention has
                // left the existing derived set temporarily over limit.
                if unchanged {
                    seen.execute(params![c.id, c.root_id]).map_err(err)?;
                    continue;
                }
                if json.len() > self.derived_limits.max_candidate_json_bytes
                    || projected_rows > self.derived_limits.max_candidate_rows
                    || projected_bytes > self.derived_limits.max_candidate_payload_bytes
                {
                    // Reserve headroom only for a finding that could fit on
                    // its own. Oversized rows must not evict useful findings.
                    // This pressure is separate from the persistent coverage
                    // warning and is consumed after one low-water target.
                    if json.len() <= self.derived_limits.max_candidate_json_bytes
                        && weight <= self.derived_limits.max_candidate_payload_bytes
                        && self.derived_limits.max_candidate_rows != 0
                    {
                        let row_headroom =
                            if projected_rows > self.derived_limits.max_candidate_rows {
                                projected_rows.saturating_sub(candidate_rows)
                            } else {
                                0
                            };
                        let byte_headroom =
                            if projected_bytes > self.derived_limits.max_candidate_payload_bytes {
                                weight.saturating_sub(old_weight)
                            } else {
                                0
                            };
                        tx.execute(
                            "UPDATE derived_admission_pressure SET
                             row_headroom=MAX(row_headroom,?1),byte_headroom=MAX(byte_headroom,?2)
                             WHERE id=1",
                            params![row_headroom, byte_headroom],
                        )
                        .map_err(err)?;
                    }
                    let reason = if json.len() > self.derived_limits.max_candidate_json_bytes {
                        "candidate row exceeds the derived JSON limit"
                    } else {
                        "derived candidate budget is full"
                    };
                    let omitted_bytes = weight;
                    let refresh_generation: Option<String> = tx
                        .query_row(
                            "SELECT generation FROM refreshes WHERE root_id=?1",
                            [&c.root_id],
                            |row| row.get(0),
                        )
                        .optional()
                        .map_err(err)?;
                    if let Some(generation) = refresh_generation {
                        limited
                            .execute(params![c.root_id, generation, 1u64, omitted_bytes, reason])
                            .map_err(err)?;
                    }
                    root_limited
                        .execute(params![c.root_id, 1u64, omitted_bytes, reason])
                        .map_err(err)?;
                    mark_incomplete.execute([&c.root_id]).map_err(err)?;
                    continue;
                }
                seen.execute(params![c.id, c.root_id]).map_err(err)?;
                remove_path_conflict
                    .execute(params![path, c.id])
                    .map_err(err)?;
                let inserted = insert
                    .execute(params![c.id, c.root_id, path, json])
                    .map_err(err)?;
                changed |= inserted != 0;
                if inserted != 0 {
                    candidate_rows = projected_rows;
                    candidate_payload_bytes = projected_bytes;
                }
            }
        }
        tx.commit().map_err(err)?;
        if changed {
            self.invalidate_candidates();
        }
        Ok(())
    }

    /// Remove only expendable candidate rows, in a small bounded transaction.
    /// Eviction itself invalidates the affected roots; no refresh is enqueued
    /// here, so a full budget cannot create a retry storm.
    pub(crate) fn maintain_derived(
        &mut self,
        protected_candidate_ids: &[String],
    ) -> Result<DerivedUsage> {
        let limits = self.derived_limits;
        let tx = self.conn.transaction().map_err(err)?;
        let usage: (u64, u64) = tx
            .query_row(
                "SELECT candidate_rows,candidate_payload_bytes FROM derived_usage WHERE id=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(err)?;
        let pressure: (u64, u64) = tx
            .query_row(
                "SELECT row_headroom,byte_headroom FROM derived_admission_pressure WHERE id=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(err)?;
        let row_target = limits.max_candidate_rows.saturating_sub(pressure.0);
        let byte_target = limits
            .max_candidate_payload_bytes
            .saturating_sub(pressure.1);
        if usage.0 <= row_target && usage.1 <= byte_target {
            if pressure != (0, 0) {
                tx.execute("UPDATE derived_admission_pressure SET row_headroom=0,byte_headroom=0 WHERE id=1", []).map_err(err)?;
            }
            tx.commit().map_err(err)?;
            return self.derived_usage();
        }
        let mut query = tx
            .prepare(
                "SELECT c.id,c.root_id,c.path,
                         length(CAST(c.json AS BLOB))+length(CAST(c.path AS BLOB))+64 FROM candidates c
                 WHERE NOT EXISTS(SELECT 1 FROM active_scopes a
                     WHERE a.root_id=c.root_id AND (a.path=c.path
                         OR c.path>=a.path||'/' AND c.path<a.path||'0'))
                   AND NOT EXISTS(SELECT 1 FROM kept k WHERE c.path=k.path
                     OR substr(c.path,1,length(k.path)+1)=k.path||'/'
                     OR substr(k.path,1,length(c.path)+1)=c.path||'/')
                 ORDER BY c.rowid LIMIT ?1",
            )
            .map_err(err)?;
        let mut rows = query
            .query([DERIVED_MAINTENANCE_ROWS as u64])
            .map_err(err)?;
        let mut candidates = Vec::new();
        while let Some(row) = rows.next().map_err(err)? {
            candidates.push((
                row.get::<_, String>(0).map_err(err)?,
                row.get::<_, String>(1).map_err(err)?,
                row.get::<_, String>(2).map_err(err)?,
                row.get::<_, u64>(3).map_err(err)?,
            ));
        }
        drop(rows);
        drop(query);
        let mut evicted = 0usize;
        let mut affected_roots = std::collections::BTreeSet::new();
        let mut remaining_bytes = usage.1;
        for (id, root, path, weight) in candidates {
            let over = usage.0.saturating_sub(evicted as u64) > row_target
                || remaining_bytes > byte_target;
            if !over {
                break;
            }
            if protected_candidate_ids
                .iter()
                .any(|protected| protected == &id)
            {
                affected_roots.insert(root);
                continue;
            }
            let removed = tx
                .execute("DELETE FROM candidates WHERE id=?1", [&id])
                .map_err(err)?;
            if removed != 0 {
                evicted += 1;
                remaining_bytes = remaining_bytes.saturating_sub(weight);
                affected_roots.insert(root);
            }
            let _ = path;
        }
        for root in affected_roots {
            tx.execute("INSERT OR IGNORE INTO incomplete_roots VALUES(?1)", [&root])
                .map_err(err)?;
            tx.execute(
                "INSERT INTO derived_limited_roots(root_id,omitted_rows,omitted_bytes,reason)
                 VALUES(?1,0,0,?2)
                 ON CONFLICT(root_id) DO UPDATE SET reason=excluded.reason",
                params![root, "derived retention protected or evicted observations"],
            )
            .map_err(err)?;
        }
        if pressure != (0, 0)
            && usage.0.saturating_sub(evicted as u64) <= row_target
            && remaining_bytes <= byte_target
        {
            tx.execute(
                "UPDATE derived_admission_pressure SET row_headroom=0,byte_headroom=0 WHERE id=1",
                [],
            )
            .map_err(err)?;
        }
        tx.commit().map_err(err)?;
        if evicted != 0 {
            self.invalidate_candidates();
        }
        self.derived_usage()
    }
    pub fn save_stats(&self, id: &str, stats: &ScanStats) -> Result<()> {
        save_stats_in(&self.conn, id, stats)
    }
    pub fn latest_stats(&self) -> Result<ScanStats> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT json FROM scans ORDER BY rowid DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(err)?;
        let mut stats = json
            .map(|v| serde_json::from_str(&v).map_err(err))
            .unwrap_or(Ok(ScanStats::default()))?;
        self.apply_coverage(&mut stats)?;
        Ok(stats)
    }
    pub fn candidates(&self) -> Result<Vec<Candidate>> {
        if let Some(cached) = self.visible_candidates.borrow().as_ref() {
            return Ok(cached.clone());
        }
        let candidates = self.json_rows::<Candidate>(&SUGGESTIONS_SQL)?;
        *self.visible_candidates.borrow_mut() = Some(candidates.clone());
        Ok(candidates)
    }

    /// Explicit content checks use their own bounded query, never the visible
    /// all-category page. A size bucket is either included whole or reported as
    /// skipped; a truncated bucket must not look like a complete duplicate set.
    pub(crate) fn duplicate_inputs(
        &self,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<DuplicateInputs> {
        const MAX_BUCKETS: usize = 1024;
        crate::safety::cancelled(cancel)?;
        let predicate = format!(
            "json_extract(c.json,'$.kind') IN ('download','installer','archive','largefile')
            AND json_extract(c.json,'$.suggestion_eligible')=1
            AND json_extract(c.json,'$.blocked_reason') IS NULL
            AND COALESCE(json_extract(c.json,'$.provisional'),0)=0
            AND json_extract(c.json,'$.eligible_permanent')=0
            AND json_extract(c.json,'$.allocated_bytes')>={}
            AND typeof(json_extract(c.json,'$.identity.size'))='integer'
            AND json_extract(c.json,'$.identity.size')>0
            AND length(CAST(c.json AS BLOB))<=16384",
            crate::recommendations::minimum_size_sql("c.json")
        );
        let indexed_files = self.conn.query_row(
            &format!("SELECT COUNT(*) FROM candidates c JOIN roots r ON r.id=c.root_id WHERE {predicate}"),
            [], |row| row.get(0),
        ).map_err(err)?;
        let mut result = DuplicateInputs {
            files: Vec::new(),
            indexed_files,
            skipped_buckets: 0,
            skipped_bucket_files: 0,
            bucket_limit_reached: false,
        };
        let mut statement = self.conn.prepare(&format!(
            "SELECT json_extract(c.json,'$.identity.device'),json_extract(c.json,'$.identity.size'),COUNT(*)
            FROM candidates c JOIN roots r ON r.id=c.root_id WHERE {predicate}
            GROUP BY json_extract(c.json,'$.identity.device'),json_extract(c.json,'$.identity.size')
            HAVING COUNT(*)>1
            ORDER BY SUM(json_extract(c.json,'$.allocated_bytes')) DESC,
                     json_extract(c.json,'$.identity.device'),json_extract(c.json,'$.identity.size') DESC
            LIMIT {}", MAX_BUCKETS + 1
        )).map_err(err)?;
        let mut buckets = statement.query([]).map_err(err)?;
        let mut bucket_index = 0;
        while let Some(bucket) = buckets.next().map_err(err)? {
            crate::safety::cancelled(cancel)?;
            if bucket_index == MAX_BUCKETS {
                result.bucket_limit_reached = true;
                break;
            }
            bucket_index += 1;
            let device: u64 = bucket.get(0).map_err(err)?;
            let size: u64 = bucket.get(1).map_err(err)?;
            let count: u64 = bucket.get(2).map_err(err)?;
            if count > (crate::duplicates::MAX_FILES - result.files.len()) as u64 {
                result.skipped_buckets += 1;
                result.skipped_bucket_files = result.skipped_bucket_files.saturating_add(count);
                continue;
            }
            let mut members = self
                .conn
                .prepare(&format!(
                    "SELECT r.json,c.json,EXISTS(SELECT 1 FROM kept k WHERE c.path=k.path
                    OR substr(c.path,1,length(k.path)+1)=k.path||'/'
                    OR substr(k.path,1,length(c.path)+1)=c.path||'/')
                FROM candidates c JOIN roots r ON r.id=c.root_id WHERE {predicate}
                    AND json_extract(c.json,'$.identity.device')=?1
                    AND json_extract(c.json,'$.identity.size')=?2 ORDER BY c.path"
                ))
                .map_err(err)?;
            let mut rows = members.query(params![device, size]).map_err(err)?;
            let before = result.files.len();
            while let Some(row) = rows.next().map_err(err)? {
                crate::safety::cancelled(cancel)?;
                if result.files.len() - before >= count as usize {
                    return Err(
                        "The indexed size bucket changed during selection; check again".into(),
                    );
                }
                result.files.push(crate::duplicates::Input {
                    root: serde_json::from_str(&row.get::<_, String>(0).map_err(err)?)
                        .map_err(err)?,
                    candidate: serde_json::from_str(&row.get::<_, String>(1).map_err(err)?)
                        .map_err(err)?,
                    keeper_only: row.get(2).map_err(err)?,
                });
            }
            if result.files.len() - before != count as usize {
                return Err("The indexed size bucket changed during selection; check again".into());
            }
        }
        Ok(result)
    }

    fn invalidate_candidates(&self) {
        self.visible_candidates.borrow_mut().take();
    }

    pub fn apply_coverage(&self, stats: &mut ScanStats) -> Result<()> {
        let count: u64 = self
            .conn
            .query_row("SELECT count(*) FROM incomplete_roots", [], |r| r.get(0))
            .map_err(err)?;
        if count > 0 {
            stats.complete = false;
            if !stats.cancelled {
                stats.message = format!(
                    "{count} authorized location(s) still need complete reconciliation. {} entries checked in the latest pass.",
                    stats.entries
                );
            }
        }
        let limited: Option<(u64, u64)> = self
            .conn
            .query_row(
                "SELECT coalesce(sum(omitted_rows),0),coalesce(sum(omitted_bytes),0)
                 FROM derived_limited_roots HAVING count(*)>0",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(err)?;
        if let Some((omitted_rows, omitted_bytes)) = limited {
            stats.complete = false;
            if !stats.cancelled {
                stats.message = format!(
                    "Derived findings are storage-limited ({omitted_rows} observations / {omitted_bytes} payload bytes omitted); complete reconciliation is required."
                );
            }
        }
        Ok(())
    }

    /// A storage-limited result cannot report complete findings. Ordinary
    /// root-wide incompleteness is overlaid only on the aggregate snapshot: an
    /// incremental scope can finish correctly while the rest of its root is
    /// still awaiting reconciliation.
    pub(crate) fn apply_root_coverage(&self, root: &str, stats: &mut ScanStats) -> Result<()> {
        let limited: Option<(u64, u64)> = self
            .conn
            .query_row(
                "SELECT omitted_rows,omitted_bytes FROM derived_limited_roots WHERE root_id=?1",
                [root],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(err)?;
        if let Some((omitted_rows, omitted_bytes)) = limited {
            stats.complete = false;
            if !stats.cancelled {
                stats.message = format!(
                    "Derived findings are storage-limited for {root} ({omitted_rows} observations / {omitted_bytes} payload bytes omitted); complete reconciliation is required."
                );
            }
        }
        Ok(())
    }

    pub fn incomplete(&self, root: &str) -> Result<bool> {
        self.conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM incomplete_roots WHERE root_id=?1)",
                [root],
                |r| r.get(0),
            )
            .map_err(err)
    }

    /// Find an enclosing artifact using at most one indexed lookup per path
    /// component; no candidate JSON or unrelated rows are loaded.
    pub fn enclosing_candidate(&self, root: &Root, path: &Path) -> Result<Option<PathBuf>> {
        let mut query = self
            .conn
            .prepare_cached("SELECT path FROM candidates WHERE root_id=?1 AND path=?2")
            .map_err(err)?;
        for ancestor in path.ancestors().take_while(|p| p.starts_with(&root.path)) {
            let found: Option<String> = query
                .query_row(params![root.id, ancestor.to_string_lossy()], |r| r.get(0))
                .optional()
                .map_err(err)?;
            if let Some(found) = found {
                return Ok(Some(PathBuf::from(found)));
            }
        }
        Ok(None)
    }
    pub fn candidates_for_root(&self, root: &str) -> Result<Vec<Candidate>> {
        let mut q = self
            .conn
            .prepare("SELECT json FROM candidates WHERE root_id=?1")
            .map_err(err)?;
        q.query_map([root], |r| r.get::<_, String>(0))
            .map_err(err)?
            .map(|r| serde_json::from_str(&r.map_err(err)?).map_err(err))
            .collect()
    }
    pub fn candidate(&self, id: &str) -> Result<Candidate> {
        let json: String = self
            .conn
            .query_row("SELECT json FROM candidates WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .map_err(err)?;
        serde_json::from_str(&json).map_err(err)
    }
    pub fn discard_candidate(&self, id: &str) -> Result<()> {
        self.invalidate_candidates();
        self.conn
            .execute("DELETE FROM candidates WHERE id=?1", [id])
            .map_err(err)?;
        Ok(())
    }

    /// Suppress buffered discoveries before a cleanup changes the filesystem.
    /// A new refresh of this location explicitly permits its next observation.
    pub fn suppress_candidate(&mut self, id: &str) -> Result<()> {
        let tx = self.conn.transaction().map_err(err)?;
        tx.execute(
            "INSERT OR REPLACE INTO candidate_tombstones
             SELECT id,root_id,path FROM candidates WHERE id=?1",
            [id],
        )
        .map_err(err)?;
        // Keep an unknown ID suppressed as well; save_batch can later record its
        // location without allowing that delayed result to resurrect the row.
        tx.execute(
            "INSERT OR IGNORE INTO candidate_tombstones(id) VALUES(?1)",
            [id],
        )
        .map_err(err)?;
        tx.execute("DELETE FROM candidates WHERE id=?1", [id])
            .map_err(err)?;
        tx.commit().map_err(err)?;
        self.invalidate_candidates();
        Ok(())
    }

    pub fn kept(&self) -> Result<Vec<String>> {
        let mut q = self
            .conn
            .prepare("SELECT path FROM kept ORDER BY path")
            .map_err(err)?;
        q.query_map([], |r| r.get(0))
            .map_err(err)?
            .map(|r| r.map_err(err))
            .collect()
    }
    pub fn keep(&self, path: &str, value: bool) -> Result<()> {
        self.invalidate_candidates();
        self.conn
            .execute(
                if value {
                    "INSERT OR IGNORE INTO kept VALUES(?1)"
                } else {
                    "DELETE FROM kept WHERE path=?1"
                },
                [path],
            )
            .map_err(err)?;
        Ok(())
    }
    pub fn wallet(&self) -> Result<Wallet> {
        self.conn.query_row("SELECT collected,remainder,credited,(SELECT coalesce(sum(coins),0) FROM earnings WHERE collected=0) FROM wallet WHERE id=1",[],|r|Ok(Wallet {collected_coins:r.get(0)?,fractional_bytes:r.get(1)?,credited_bytes:r.get(2)?,pending_coins:r.get(3)?})).map_err(err)
    }
    pub fn collect(&mut self) -> Result<(u64, u64, u64)> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(err)?;
        let before: u64 = tx
            .query_row("SELECT collected FROM wallet WHERE id=1", [], |r| r.get(0))
            .map_err(err)?;
        let pending: u64 = tx
            .query_row(
                "SELECT coalesce(sum(coins),0) FROM earnings WHERE collected=0",
                [],
                |r| r.get(0),
            )
            .map_err(err)?;
        if pending == 0 {
            return Ok((before, before, 0));
        }
        tx.execute("UPDATE earnings SET collected=1 WHERE collected=0", [])
            .map_err(err)?;
        tx.execute(
            "UPDATE wallet SET collected=collected+?1 WHERE id=1",
            [pending],
        )
        .map_err(err)?;
        tx.commit().map_err(err)?;
        Ok((before, before + pending, pending))
    }
    pub fn history(&self) -> Result<Vec<Receipt>> {
        self.json_rows("SELECT receipt_json FROM operations ORDER BY rowid DESC LIMIT 100")
    }
    /// One page of receipts, newest first, each carrying its ledger sequence,
    /// plus the cursor for the next-older page and the ledger's total count.
    /// Read-only: paging never rewrites stored receipt JSON.
    pub fn history_page(
        &self,
        before: Option<i64>,
        limit: u32,
    ) -> Result<(Vec<Receipt>, Option<i64>, u64)> {
        let mut query = self.conn.prepare_cached(HISTORY_PAGE_SQL).map_err(err)?;
        let rows = query
            .query_map(params![before, limit], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(err)?;
        let mut receipts = Vec::new();
        let mut oldest = None;
        for row in rows {
            let (seq, json) = row.map_err(err)?;
            let mut receipt: Receipt = serde_json::from_str(&json).map_err(err)?;
            receipt.seq = Some(seq);
            oldest = Some(seq);
            receipts.push(receipt);
        }
        let total: u64 = self
            .conn
            .query_row("SELECT count(*) FROM operations", [], |row| row.get(0))
            .map_err(err)?;
        // A short page proves the ledger's oldest receipt was reached.
        let next_before = (receipts.len() as u64 == u64::from(limit))
            .then_some(oldest)
            .flatten();
        Ok((receipts, next_before, total))
    }
    pub fn prepare_operation(
        &self,
        root: &Root,
        c: &Candidate,
        receipt: &Receipt,
        stage: &Path,
    ) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO operations VALUES(?1,?2,?3,?4,?5,NULL,'prepared')",
                params![
                    receipt.id,
                    serde_json::to_string(root).map_err(err)?,
                    serde_json::to_string(c).map_err(err)?,
                    serde_json::to_string(receipt).map_err(err)?,
                    stage.to_string_lossy()
                ],
            )
            .map_err(err)?;
        Ok(())
    }
    pub fn finish_operation(
        &self,
        receipt: &Receipt,
        trash_identity: Option<&Identity>,
    ) -> Result<()> {
        self.conn
            .execute(
                "UPDATE operations SET receipt_json=?2,state=?3,trash_identity=?4 WHERE id=?1",
                params![
                    receipt.id,
                    serde_json::to_string(receipt).map_err(err)?,
                    receipt.outcome,
                    trash_identity
                        .map(serde_json::to_string)
                        .transpose()
                        .map_err(err)?
                ],
            )
            .map_err(err)?;
        Ok(())
    }
    pub fn operation(&self, id: &str) -> Result<StoredOperation> {
        let row=self.conn.query_row("SELECT root_json,candidate_json,receipt_json,trash_identity,stage_path FROM operations WHERE id=?1",[id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,Option<String>>(3)?,r.get::<_,Option<String>>(4)?))).map_err(err)?;
        Ok((
            serde_json::from_str(&row.0).map_err(err)?,
            serde_json::from_str(&row.1).map_err(err)?,
            serde_json::from_str(&row.2).map_err(err)?,
            row.3
                .map(|s| serde_json::from_str(&s))
                .transpose()
                .map_err(err)?,
            row.4.map(PathBuf::from),
        ))
    }
    fn json_rows<T: serde::de::DeserializeOwned>(&self, sql: &str) -> Result<Vec<T>> {
        let mut q = self.conn.prepare(sql).map_err(err)?;
        q.query_map([], |r| r.get::<_, String>(0))
            .map_err(err)?
            .map(|r| serde_json::from_str(&r.map_err(err)?).map_err(err))
            .collect()
    }
}

fn scope_path(path: &Path) -> Result<String> {
    if !path.is_absolute() || path.components().any(|c| c == Component::ParentDir) {
        return Err("A discovery scope must be an absolute path without parent traversal".into());
    }
    let path: PathBuf = path.components().collect();
    path.into_os_string()
        .into_string()
        .map_err(|_| "This path cannot be represented in the native interface".into())
}

// A path component separator is byte 0x2f. Its exclusive upper bound, 0x30,
// excludes siblings such as `project-other` and treats '%' and '_' literally.
fn descendant_range(path: &str) -> (String, String) {
    let parent = path.trim_end_matches('/');
    (format!("{parent}/"), format!("{parent}0"))
}

fn validate_foreground_summary(scan: &ForegroundScan) -> Result<()> {
    if scan.active {
        return Err("An active foreground scan cannot be a saved summary".into());
    }
    if scan.stats.complete && (scan.stats.cancelled || scan.stats.errors != 0) {
        return Err("The foreground summary has inconsistent completion status".into());
    }
    Ok(())
}

fn invalidate_foreground_summary_in(conn: &Connection) -> Result<()> {
    // Restrict addition before it occurs: SQLite otherwise promotes overflowing
    // INTEGER arithmetic to REAL, which could allow a context to be reused.
    let changed = conn
        .execute(
            "UPDATE foreground_state SET revision=revision+1,summary_json=NULL,completed_at=NULL
         WHERE id=1 AND typeof(revision)='integer' AND revision>=0 AND revision<?1",
            [i64::MAX],
        )
        .map_err(err)?;
    if changed != 1 {
        return Err("The foreground summary context is missing or exhausted".into());
    }
    Ok(())
}

// These helpers never open transactions. Both standalone index APIs and the
// discovery journal's compound operations supply their own transaction boundary.
fn discard_pending_scope_in(conn: &Connection, root: &str, path: &str) -> Result<()> {
    let (lower, upper) = descendant_range(path);
    conn.execute(
        "DELETE FROM pending_scopes WHERE root_id=?1 AND path=?2",
        params![root, path],
    )
    .map_err(err)?;
    conn.execute(
        "DELETE FROM pending_scopes WHERE root_id=?1 AND path>=?2 AND path<?3",
        params![root, lower, upper],
    )
    .map_err(err)?;
    Ok(())
}

fn initialize_refresh_in(
    conn: &Connection,
    root: &str,
    scope: Option<&str>,
    generation: &str,
) -> Result<()> {
    // An abandoned generation must never remove candidates. Superseding it
    // discards only bookkeeping, then captures this pass's exact scope.
    conn.execute("DELETE FROM refresh_seen WHERE root_id=?1", [root])
        .map_err(err)?;
    conn.execute(
        "INSERT OR REPLACE INTO refreshes VALUES(?1,?2,?3)",
        params![root, scope, generation],
    )
    .map_err(err)?;
    Ok(())
}

fn begin_refresh_in(
    conn: &Connection,
    root: &str,
    scope: Option<&str>,
    generation: &str,
) -> Result<()> {
    initialize_refresh_in(conn, root, scope, generation)?;
    if let Some(path) = scope {
        capture_scope_in(conn, root, path, generation, true)?;
    } else {
        conn.execute(
            "INSERT INTO refresh_seen SELECT root_id,?2,id,0 FROM candidates WHERE root_id=?1",
            params![root, generation],
        )
        .map_err(err)?;
        conn.execute("DELETE FROM candidate_tombstones WHERE root_id=?1", [root])
            .map_err(err)?;
    }
    Ok(())
}

fn capture_scope_in(
    conn: &Connection,
    root: &str,
    path: &str,
    generation: &str,
    include_ancestors: bool,
) -> Result<()> {
    let (lower, upper) = descendant_range(path);
    conn.execute(
        "INSERT INTO refresh_seen SELECT root_id,?2,id,0 FROM candidates
         WHERE root_id=?1 AND path>=?3 AND path<?4",
        params![root, generation, lower, upper],
    )
    .map_err(err)?;
    conn.execute(
        "DELETE FROM candidate_tombstones WHERE root_id=?1 AND path>=?2 AND path<?3",
        params![root, lower, upper],
    )
    .map_err(err)?;
    let mut capture_ancestor = conn
        .prepare_cached(
            "INSERT OR IGNORE INTO refresh_seen SELECT root_id,?2,id,0 FROM candidates
             WHERE root_id=?1 AND path=?3",
        )
        .map_err(err)?;
    let mut clear_ancestor_tombstones = conn
        .prepare_cached("DELETE FROM candidate_tombstones WHERE root_id=?1 AND path=?2")
        .map_err(err)?;
    for ancestor in Path::new(path).ancestors() {
        let ancestor = ancestor.to_str().expect("The scope was checked as UTF-8");
        capture_ancestor
            .execute(params![root, generation, ancestor])
            .map_err(err)?;
        clear_ancestor_tombstones
            .execute(params![root, ancestor])
            .map_err(err)?;
        // Ordinary scopes also invalidate enclosing artifact rows. Compound
        // lock refreshes must leave their common parent and its other rows alone.
        if !include_ancestors {
            break;
        }
    }
    Ok(())
}

fn finish_refresh_in(
    conn: &Connection,
    root: &str,
    generation: &str,
    complete: bool,
) -> Result<usize> {
    let removed = if complete {
        conn.execute(
            "DELETE FROM candidates WHERE root_id=?1 AND id IN
             (SELECT candidate_id FROM refresh_seen WHERE root_id=?1 AND generation=?2 AND seen=0)",
            params![root, generation],
        )
        .map_err(err)?
    } else {
        0
    };
    conn.execute("DELETE FROM refresh_seen WHERE root_id=?1", [root])
        .map_err(err)?;
    conn.execute("DELETE FROM refreshes WHERE root_id=?1", [root])
        .map_err(err)?;
    Ok(removed)
}

fn save_stats_in(conn: &Connection, root: &str, stats: &ScanStats) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO scans VALUES(?1,?2)",
        params![root, serde_json::to_string(stats).map_err(err)?],
    )
    .map_err(err)?;
    Ok(())
}

fn recount_derived_usage_in(conn: &Connection) -> Result<()> {
    conn.execute(
        "UPDATE derived_usage SET candidate_rows=(SELECT count(*) FROM candidates),
            candidate_payload_bytes=(SELECT coalesce(sum(length(CAST(json AS BLOB))+
                length(CAST(path AS BLOB))+64),0) FROM candidates) WHERE id=1",
        [],
    )
    .map_err(err)?;
    Ok(())
}

fn candidate_payload_weight(json: &[u8], path: &Path) -> Result<u64> {
    let json = u64::try_from(json.len()).map_err(err)?;
    let path = u64::try_from(path.as_os_str().len()).map_err(err)?;
    json.checked_add(path)
        .and_then(|bytes| bytes.checked_add(CANDIDATE_PAYLOAD_OVERHEAD))
        .ok_or_else(|| "Derived candidate payload overflow".into())
}

fn acknowledge_scope_in(conn: &Connection, root: &str, claimed: &str) -> Result<()> {
    let removed = conn
        .execute(
            "DELETE FROM active_scopes WHERE root_id=?1 AND path=?2",
            params![root, claimed],
        )
        .map_err(err)?;
    if removed != 1 {
        return Err("The completed refresh has no matching durable scope claim".into());
    }
    Ok(())
}

fn enqueue_scope_in(conn: &Connection, root: &str, path: &Path) -> Result<()> {
    let path = scope_path(path)?;
    let root_path: String = conn
        .query_row("SELECT path FROM roots WHERE id=?1", [root], |r| r.get(0))
        .map_err(err)?;
    if !Path::new(&path).starts_with(&root_path) {
        return Err("The discovery scope is outside its authorized location".into());
    }
    enqueue_validated_scope_in(conn, root, &root_path, &path)
}

// The caller supplies a canonical path validated against this transaction's
// grant. Only pending scopes subsume work; active scopes never participate.
fn enqueue_validated_scope_in(
    conn: &Connection,
    root: &str,
    root_path: &str,
    path: &str,
) -> Result<()> {
    let mut ancestor_query = conn
        .prepare_cached("SELECT EXISTS(SELECT 1 FROM pending_scopes WHERE root_id=?1 AND path=?2)")
        .map_err(err)?;
    for ancestor in Path::new(path)
        .ancestors()
        .take_while(|p| p.starts_with(root_path))
    {
        let exists: bool = ancestor_query
            .query_row(params![root, ancestor.to_string_lossy()], |r| r.get(0))
            .map_err(err)?;
        if exists {
            return Ok(());
        }
    }
    let (lower, upper) = descendant_range(path);
    conn.execute(
        "DELETE FROM pending_scopes WHERE root_id=?1 AND path>=?2 AND path<?3",
        params![root, lower, upper],
    )
    .map_err(err)?;
    conn.execute(
        "INSERT INTO pending_scopes VALUES(?1,?2)",
        params![root, path],
    )
    .map_err(err)?;
    Ok(())
}

fn remove_discovery_for_root(conn: &Connection, root: &str) -> Result<()> {
    for table in [
        "pending_scopes",
        "active_scopes",
        "refreshes",
        "refresh_seen",
        "candidate_tombstones",
    ] {
        conn.execute(&format!("DELETE FROM {table} WHERE root_id=?1"), [root])
            .map_err(err)?;
    }
    Ok(())
}

pub fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::atomic::AtomicBool;

    fn item(id: &str, path: &str, kind: &str, bytes: u64, eligible: bool) -> Candidate {
        Candidate {
            id: id.into(),
            root_id: "root".into(),
            path: path.into(),
            title: id.into(),
            kind: kind.into(),
            logical_bytes: bytes,
            allocated_bytes: bytes,
            file_count: 1,
            modified_ns: 0,
            explanation: "Verified fixture".into(),
            consequence: "Rebuild".into(),
            eligible_permanent: kind != "download",
            blocked_reason: None,
            identity: Identity {
                device: 1,
                inode: 1,
                mode: 0,
                size: bytes,
                modified_ns: 0,
                changed_ns: 0,
            },
            fingerprint: "fixture".into(),
            evidence: "fixture".into(),
            suggestion_eligible: eligible,
            provisional: false,
        }
    }
    fn save(store: &mut Store, candidates: Vec<Candidate>) {
        store
            .save_batch(&ScanBatch {
                candidates,
                stats: ScanStats::default(),
            })
            .unwrap();
    }

    fn discovery_root(store: &mut Store) {
        store
            .add_root(&Root {
                id: "root".into(),
                path: "/root".into(),
                kind: "projects".into(),
                identity: item("root", "/root", "cargo", 1, false).identity,
            })
            .unwrap();
    }

    fn duplicate_bucket_counts(inputs: &DuplicateInputs) -> BTreeMap<(u64, u64), usize> {
        let mut counts = BTreeMap::new();
        for input in &inputs.files {
            let identity = &input.candidate.identity;
            *counts.entry((identity.device, identity.size)).or_insert(0) += 1;
        }
        counts
    }

    fn terminal_summary(entries: u64) -> ForegroundScan {
        ForegroundScan {
            active: false,
            stats: ScanStats {
                entries,
                complete: true,
                elapsed_ms: 125,
                first_finding_ms: Some(17),
                message: "Disposable completed scan".into(),
                ..Default::default()
            },
        }
    }

    fn saved_summary(store: &Store) -> Option<serde_json::Value> {
        store
            .load_foreground_summary()
            .unwrap()
            .map(|scan| serde_json::to_value(scan).unwrap())
    }

    #[test]
    fn foreground_summary_roundtrips_complete_cancelled_and_failed_results() {
        for outcome in ["complete", "cancelled", "failed"] {
            let temp = tempfile::tempdir().unwrap();
            let db = temp.path().join("db");
            let mut store = Store::open(&db).unwrap();
            discovery_root(&mut store);
            let context = store.foreground_context().unwrap();
            let mut scan = terminal_summary(9_007_199_254_740_993);
            scan.stats.complete = outcome == "complete";
            scan.stats.cancelled = outcome == "cancelled";
            scan.stats.errors = u64::from(outcome == "failed");
            assert!(store.save_foreground_summary(context, &scan).unwrap());
            let expected = serde_json::to_value(scan).unwrap();
            assert_eq!(saved_summary(&store), Some(expected.clone()));
            drop(store);
            let store = Store::open(&db).unwrap();
            assert_eq!(store.foreground_context().unwrap(), context);
            assert_eq!(saved_summary(&store), Some(expected));
            assert!(
                store.incomplete("root").unwrap(),
                "A display summary cannot manufacture index coverage"
            );
            assert!(store.history().unwrap().is_empty());
            assert_eq!(store.wallet().unwrap().credited_bytes, 0);
        }
    }

    #[test]
    fn foreground_summary_rejects_invalid_saves_and_ignores_invalid_cache_content() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("db");
        let mut store = Store::open(&db).unwrap();
        discovery_root(&mut store);
        store
            .conn
            .execute(
                "UPDATE wallet SET collected=17,remainder=42,credited=1700000042",
                [],
            )
            .unwrap();
        let context = store.foreground_context().unwrap();
        let good = terminal_summary(3);
        assert!(store.save_foreground_summary(context, &good).unwrap());
        let mut active = good.clone();
        active.active = true;
        let mut cancelled_complete = good.clone();
        cancelled_complete.stats.cancelled = true;
        let mut failed_complete = good.clone();
        failed_complete.stats.errors = 1;
        for invalid in [active, cancelled_complete, failed_complete] {
            assert!(store.save_foreground_summary(context, &invalid).is_err());
            assert_eq!(
                saved_summary(&store),
                Some(serde_json::to_value(&good).unwrap())
            );
            store
                .conn
                .execute(
                    "UPDATE foreground_state SET summary_json=?1",
                    [serde_json::to_string(&invalid).unwrap()],
                )
                .unwrap();
            assert!(store.load_foreground_summary().unwrap().is_none());
            assert!(store.save_foreground_summary(context, &good).unwrap());
        }
        assert!(store.save_foreground_summary(-1, &good).is_err());
        for invalid in [
            "{",
            "null",
            "{}",
            r#"{"active":false,"stats":{"complete":true}}"#,
        ] {
            store
                .conn
                .execute("UPDATE foreground_state SET summary_json=?1", [invalid])
                .unwrap();
            assert!(store.load_foreground_summary().unwrap().is_none());
            let unchanged: String = store
                .conn
                .query_row("SELECT summary_json FROM foreground_state", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(
                unchanged, invalid,
                "Reading invalid presentation data must not mutate the library"
            );
        }
        assert!(store.save_foreground_summary(context, &good).unwrap());
        for message in [
            "x".repeat(MAX_FOREGROUND_SUMMARY_BYTES),
            "\0".repeat(MAX_FOREGROUND_SUMMARY_BYTES / 2),
        ] {
            let mut oversized = good.clone();
            oversized.stats.message = message;
            assert!(store.save_foreground_summary(context, &oversized).is_err());
            assert_eq!(
                saved_summary(&store),
                Some(serde_json::to_value(&good).unwrap())
            );
            store
                .conn
                .execute(
                    "UPDATE foreground_state SET summary_json=?1",
                    [serde_json::to_string(&oversized).unwrap()],
                )
                .unwrap();
            assert!(
                store.load_foreground_summary().unwrap().is_none(),
                "Oversized derived cache must not be decoded"
            );
            assert!(store.save_foreground_summary(context, &good).unwrap());
        }
        let blob = serde_json::to_vec(&good).unwrap();
        store
            .conn
            .execute("UPDATE foreground_state SET summary_json=?1", [&blob])
            .unwrap();
        assert!(
            store.load_foreground_summary().unwrap().is_none(),
            "A BLOB is not a saved JSON text record"
        );
        assert!(
            store.save_foreground_summary(context, &good).unwrap(),
            "Valid results must replace malformed cache types"
        );
        store
            .conn
            .execute_batch("UPDATE foreground_state SET summary_json=CAST(X'FF' AS TEXT)")
            .unwrap();
        assert!(store.load_foreground_summary().unwrap().is_none());
        drop(store);
        let mut store = Store::open(&db).unwrap();
        assert!(store.load_foreground_summary().unwrap().is_none());
        assert!(store.root("root").is_ok());
        assert_eq!(store.wallet().unwrap().collected_coins, 17);
        assert_eq!(store.wallet().unwrap().fractional_bytes, 42);
        assert!(
            store.save_foreground_summary(context, &good).unwrap(),
            "Invalid UTF-8 in the old cache must not prevent a new terminal save"
        );
        store
            .conn
            .execute_batch("DROP TABLE foreground_state")
            .unwrap();
        assert!(
            store.load_foreground_summary().is_err(),
            "Actual SQLite failures must not masquerade as an empty cache"
        );
    }

    #[test]
    fn foreground_summary_context_tracks_only_successful_grant_changes() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::open(&temp.path().join("db")).unwrap();
        discovery_root(&mut store);
        let root = store.root("root").unwrap();
        let context = store.foreground_context().unwrap();
        let scan = terminal_summary(8);
        assert!(store.save_foreground_summary(context, &scan).unwrap());
        assert!(store.authorize_root(&root, false).is_err());
        assert_eq!(store.foreground_context().unwrap(), context);
        assert!(store.load_foreground_summary().unwrap().is_some());
        let mut duplicate_id = root.clone();
        duplicate_id.path = "/unrelated".into();
        assert!(store.authorize_root(&duplicate_id, false).is_err());
        assert_eq!(store.foreground_context().unwrap(), context);
        assert!(store.load_foreground_summary().unwrap().is_some());
        let mut another = duplicate_id;
        another.id = "another".into();
        store.authorize_root(&another, false).unwrap();
        assert_eq!(store.foreground_context().unwrap(), context + 1);
        assert!(store.load_foreground_summary().unwrap().is_none());
        assert!(!store.save_foreground_summary(context, &scan).unwrap());
        assert!(store.save_foreground_summary(context + 1, &scan).unwrap());
        assert!(!store.remove_root("does-not-exist").unwrap());
        assert_eq!(store.foreground_context().unwrap(), context + 1);
        assert!(store.load_foreground_summary().unwrap().is_some());
        assert!(store.remove_root("another").unwrap());
        assert_eq!(store.foreground_context().unwrap(), context + 2);
        assert!(store.load_foreground_summary().unwrap().is_none());
        assert!(!store.save_foreground_summary(context + 1, &scan).unwrap());
        assert!(store.root("root").is_ok());
    }

    #[test]
    fn foreground_summary_invalidation_failure_rolls_back_grants_and_never_overflows() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::open(&temp.path().join("db")).unwrap();
        discovery_root(&mut store);
        let root = store.root("root").unwrap();
        let old = item("old", "/root/target", "cargo", 100_000_000, true);
        save(&mut store, vec![old.clone()]);
        let context = store.foreground_context().unwrap();
        let scan = terminal_summary(3);
        assert!(store.save_foreground_summary(context, &scan).unwrap());
        store.conn.execute_batch("CREATE TEMP TRIGGER fail_summary_invalidation BEFORE UPDATE ON foreground_state BEGIN SELECT RAISE(ABORT,'disposable context failure'); END;").unwrap();
        let mut another = root.clone();
        another.id = "another".into();
        another.path = "/unrelated".into();
        assert!(store.authorize_root(&another, false).is_err());
        assert!(store.root("another").is_err());
        assert!(store.remove_root("root").is_err());
        assert!(store.root("root").is_ok());
        assert_eq!(store.candidate("old").unwrap(), old);
        assert_eq!(store.foreground_context().unwrap(), context);
        assert_eq!(
            saved_summary(&store),
            Some(serde_json::to_value(&scan).unwrap())
        );
        store
            .conn
            .execute_batch("DROP TRIGGER fail_summary_invalidation")
            .unwrap();
        store
            .conn
            .execute("UPDATE foreground_state SET revision=?1", [i64::MAX])
            .unwrap();
        assert!(store.authorize_root(&another, false).is_err());
        assert!(store.root("another").is_err());
        assert_eq!(store.foreground_context().unwrap(), i64::MAX);
        assert_eq!(
            saved_summary(&store),
            Some(serde_json::to_value(scan).unwrap())
        );
    }

    #[test]
    fn foreground_summary_save_failure_and_background_refreshes_do_not_change_last_result() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::open(&temp.path().join("db")).unwrap();
        discovery_root(&mut store);
        let root = store.root("root").unwrap();
        let context = store.foreground_context().unwrap();
        let scan = terminal_summary(31);
        assert!(store.save_foreground_summary(context, &scan).unwrap());
        store.conn.execute_batch("CREATE TEMP TRIGGER reject_summary_write BEFORE UPDATE ON foreground_state BEGIN SELECT RAISE(ABORT,'disposable summary write failure'); END;").unwrap();
        assert!(
            store.save_foreground_summary(context, &scan).unwrap(),
            "An identical terminal retry must not rewrite its summary"
        );
        assert!(!store.save_foreground_summary(context - 1, &scan).unwrap());
        assert!(
            store
                .save_foreground_summary(context, &terminal_summary(32))
                .is_err()
        );
        for index in 0..3 {
            let path = PathBuf::from(format!("/root/changed-{index}"));
            claim(&mut store, &path);
            let refresh = store
                .begin_scope_refresh(&root, &path, Some(&path))
                .unwrap();
            store
                .finish_scope_refresh(
                    &refresh,
                    &ScanStats {
                        entries: 1,
                        complete: true,
                        ..Default::default()
                    },
                    false,
                )
                .unwrap();
        }
        assert_eq!(
            saved_summary(&store),
            Some(serde_json::to_value(scan).unwrap())
        );
        assert_eq!(store.foreground_context().unwrap(), context);
        assert!(store.history().unwrap().is_empty());
        assert_eq!(store.wallet().unwrap().credited_bytes, 0);
    }

    #[test]
    fn a_thousand_pending_siblings_stay_scoped_and_coalesce_only_with_an_ancestor() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("db")).unwrap();
        discovery_root(&mut store);
        for index in 0..1000 {
            store
                .enqueue_scope("root", Path::new(&format!("/root/project-{index}/target")))
                .unwrap();
        }
        let pending: u64 = store
            .conn
            .query_row("SELECT count(*) FROM pending_scopes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(pending, 1000);
        assert!(store.incomplete("root").unwrap());
        assert_eq!(
            store.take_scope().unwrap(),
            Some(("root".into(), "/root/project-0/target".into()))
        );
        store
            .finish_scope("root", Path::new("/root/project-0/target"))
            .unwrap();
        store.enqueue_scope("root", Path::new("/root")).unwrap();
        assert_eq!(
            store.take_scope().unwrap(),
            Some(("root".into(), "/root".into()))
        );
        assert!(!store.has_pending_scopes().unwrap());
    }

    #[test]
    fn pending_ancestors_preserve_literal_sibling_names_and_reject_escaping_paths() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("db")).unwrap();
        discovery_root(&mut store);
        for path in ["/root/a%_/one", "/root/a%_/two", "/root/a%_other/three"] {
            store.enqueue_scope("root", Path::new(path)).unwrap();
        }
        store.enqueue_scope("root", Path::new("/root/a%_")).unwrap();
        store
            .enqueue_scope("root", Path::new("/root/a%_/another/deep/path"))
            .unwrap();
        let count: u64 = store
            .conn
            .query_row("SELECT count(*) FROM pending_scopes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
        assert_eq!(
            store.take_scope().unwrap(),
            Some(("root".into(), "/root/a%_other/three".into()))
        );
        for path in ["relative", "/root/../other", "/root-other"] {
            assert!(store.enqueue_scope("root", Path::new(path)).is_err());
        }
    }

    #[test]
    fn scope_batches_match_sequential_coalescing_and_fifo() {
        struct Case {
            existing: &'static [&'static str],
            batch: &'static [&'static str],
            expected: &'static [&'static str],
        }
        let cases = [
            Case {
                existing: &["/root/old"],
                batch: &[],
                expected: &["/root/old"],
            },
            Case {
                existing: &["/root/old"],
                batch: &["/root//one/./"],
                expected: &["/root/old", "/root/one"],
            },
            Case {
                existing: &["/root/z"],
                batch: &["/root//a/./", "/root/a", "/root/b", "/root/a/child"],
                expected: &["/root/z", "/root/a", "/root/b"],
            },
            Case {
                existing: &["/root/a/old", "/root/z"],
                batch: &[
                    "/root/a/child",
                    "/root/b",
                    "/root/a",
                    "/root/b/child",
                    "/root/a/",
                ],
                expected: &["/root/z", "/root/b", "/root/a"],
            },
            Case {
                existing: &["/root/a", "/root/z"],
                batch: &["/root/a/child", "/root/b", "/root/a/other", "/root/a"],
                expected: &["/root/a", "/root/z", "/root/b"],
            },
            Case {
                existing: &[],
                batch: &[
                    "/root/a",
                    "/root/a-other",
                    "/root/a/child",
                    "/root/a%_",
                    "/root/a%_/child",
                    "/root/a%_other/child",
                ],
                expected: &[
                    "/root/a",
                    "/root/a-other",
                    "/root/a%_",
                    "/root/a%_other/child",
                ],
            },
            Case {
                existing: &[],
                batch: &[
                    "/root/café/child",
                    "/root/🪙/child",
                    "/root/cafe\u{301}",
                    "/root/café",
                    "/root/🪙//./child/",
                ],
                expected: &["/root/🪙/child", "/root/cafe\u{301}", "/root/café"],
            },
            Case {
                existing: &["/root/old"],
                batch: &["/root/a", "/root/b", "/root", "/root/after"],
                expected: &["/root"],
            },
        ];
        let temporary = tempfile::tempdir().unwrap();
        for (index, case) in cases.iter().enumerate() {
            let mut batched =
                Store::open(&temporary.path().join(format!("batch-{index}"))).unwrap();
            let mut sequential =
                Store::open(&temporary.path().join(format!("sequential-{index}"))).unwrap();
            for store in [&mut batched, &mut sequential] {
                discovery_root(store);
                for path in case.existing {
                    store.enqueue_scope("root", Path::new(path)).unwrap();
                }
            }
            let paths: Vec<PathBuf> = case.batch.iter().map(PathBuf::from).collect();
            batched.enqueue_scopes("root", &paths).unwrap();
            for path in &paths {
                sequential.enqueue_scope("root", path).unwrap();
            }
            for path in case.expected {
                let batch_claim = batched.take_scope().unwrap();
                assert_eq!(
                    batch_claim,
                    sequential.take_scope().unwrap(),
                    "Case {index}"
                );
                assert_eq!(
                    batch_claim,
                    Some(("root".into(), PathBuf::from(path))),
                    "Case {index}"
                );
                for store in [&mut batched, &mut sequential] {
                    store.finish_scope("root", Path::new(path)).unwrap();
                }
            }
            for store in [&mut batched, &mut sequential] {
                assert!(store.take_scope().unwrap().is_none(), "Case {index}");
                assert!(!store.has_pending_scopes().unwrap());
                assert!(store.history().unwrap().is_empty());
                assert_eq!(store.wallet().unwrap().credited_bytes, 0);
            }
        }
    }

    #[test]
    fn duplicate_scope_batches_preserve_active_claims_and_later_events() {
        fn claim_next(store: &mut Store, path: &str) {
            assert_eq!(
                store.take_scope().unwrap(),
                Some(("root".into(), path.into()))
            );
        }

        let temporary = tempfile::tempdir().unwrap();
        let mut store = Store::open(&temporary.path().join("db")).unwrap();
        discovery_root(&mut store);
        claim(&mut store, Path::new("/root"));
        let paths: Vec<_> = (0..512)
            .map(|index| {
                PathBuf::from(if index % 2 == 0 {
                    "/root/project/target"
                } else {
                    "/root//project/./target/"
                })
            })
            .collect();
        store.enqueue_scopes("root", &paths).unwrap();
        store
            .enqueue_scope("root", Path::new("/root/later"))
            .unwrap();
        assert!(store.take_scope().unwrap().is_none());
        assert_eq!(
            queued_paths(&store),
            ["/root/later", "/root/project/target"]
        );
        store.finish_scope("root", Path::new("/root")).unwrap();
        claim_next(&mut store, "/root/project/target");
        // A later event beneath an already-active artifact must remain pending.
        store.enqueue_scopes("root", &paths).unwrap();
        store
            .finish_scope("root", Path::new("/root/project/target"))
            .unwrap();
        for path in ["/root/later", "/root/project/target"] {
            claim_next(&mut store, path);
            store.finish_scope("root", Path::new(path)).unwrap();
        }
        assert!(!store.has_pending_scopes().unwrap());
    }

    #[test]
    fn invalid_scope_batches_preserve_the_queue_and_do_not_hide_invalid_paths() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let temporary = tempfile::tempdir().unwrap();
        let mut store = Store::open(&temporary.path().join("db")).unwrap();
        discovery_root(&mut store);
        claim(&mut store, Path::new("/root/active"));
        store.enqueue_scope("root", Path::new("/root/old")).unwrap();
        let before = queued_paths(&store);
        for invalid in [
            PathBuf::from("relative"),
            PathBuf::from("/root/../escape"),
            PathBuf::from("/root-other"),
            PathBuf::from(OsString::from_vec(b"/root/invalid-\xff".to_vec())),
        ] {
            let paths = [PathBuf::from("/root/new"), PathBuf::from("/root"), invalid];
            assert!(store.enqueue_scopes("root", &paths).is_err());
            assert_eq!(queued_paths(&store), before);
        }
        assert!(
            store
                .enqueue_scopes("root", &vec![PathBuf::from("/root"); 513])
                .is_err()
        );
        assert!(
            store
                .enqueue_scopes("missing", &[PathBuf::from("/root")])
                .is_err()
        );
        store.enqueue_scopes("missing", &[]).unwrap();
        assert_eq!(queued_paths(&store), before);

        store.conn.execute_batch("CREATE TEMP TRIGGER fail_batch_enqueue BEFORE INSERT ON pending_scopes WHEN NEW.path='/root/reject' BEGIN SELECT RAISE(ABORT,'disposable enqueue failure'); END;").unwrap();
        assert!(
            store
                .enqueue_scopes(
                    "root",
                    &[PathBuf::from("/root/new"), PathBuf::from("/root/reject")]
                )
                .is_err()
        );
        assert_eq!(queued_paths(&store), before);
        assert!(store.take_scope().unwrap().is_none());
        store
            .conn
            .execute_batch("DROP TRIGGER fail_batch_enqueue")
            .unwrap();
        store
            .finish_scope("root", Path::new("/root/active"))
            .unwrap();
        assert_eq!(
            store.take_scope().unwrap(),
            Some(("root".into(), "/root/old".into()))
        );
        assert!(store.history().unwrap().is_empty());
        assert_eq!(store.wallet().unwrap().credited_bytes, 0);
    }

    #[test]
    fn events_during_an_active_full_scan_remain_child_scopes() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("db")).unwrap();
        discovery_root(&mut store);
        store.enqueue_scope("root", Path::new("/root")).unwrap();
        assert_eq!(
            store.take_scope().unwrap(),
            Some(("root".into(), "/root".into()))
        );
        store
            .enqueue_scope("root", Path::new("/root/project/target"))
            .unwrap();
        assert!(store.has_pending_scopes().unwrap());
        assert!(store.take_scope().unwrap().is_none());
        store.finish_scope("root", Path::new("/root")).unwrap();
        assert_eq!(
            store.take_scope().unwrap(),
            Some(("root".into(), "/root/project/target".into()))
        );
    }

    #[test]
    fn resolving_a_scope_coalesces_older_events_but_preserves_active_and_new_work() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("db");
        let mut store = Store::open(&db).unwrap();
        discovery_root(&mut store);
        for path in ["/root/a/0", "/root/b/0", "/root/a/1"] {
            store.enqueue_scope("root", Path::new(path)).unwrap();
        }
        assert_eq!(
            store.take_scope().unwrap(),
            Some(("root".into(), "/root/a/0".into()))
        );
        store
            .discard_pending_scope("root", Path::new("/root/a"))
            .unwrap();
        store.enqueue_scope("root", Path::new("/root/a/2")).unwrap();
        assert!(store.take_scope().unwrap().is_none());
        store.finish_scope("root", Path::new("/root/a/0")).unwrap();
        assert_eq!(
            store.take_scope().unwrap(),
            Some(("root".into(), "/root/b/0".into()))
        );
        store.finish_scope("root", Path::new("/root/b/0")).unwrap();
        assert_eq!(
            store.take_scope().unwrap(),
            Some(("root".into(), "/root/a/2".into()))
        );
        assert!(!store.has_pending_scopes().unwrap());
    }

    fn queued_paths(store: &Store) -> Vec<String> {
        let mut query = store
            .conn
            .prepare("SELECT path FROM pending_scopes ORDER BY path")
            .unwrap();
        query
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|row| row.unwrap())
            .collect()
    }

    fn claim(store: &mut Store, path: &Path) {
        store.enqueue_scope("root", path).unwrap();
        assert_eq!(
            store.take_scope().unwrap(),
            Some(("root".into(), path.into()))
        );
    }

    #[test]
    fn cargo_lock_refresh_prunes_only_its_two_footprints_and_tombstones() {
        for already_incomplete in [false, true] {
            let temporary = tempfile::tempdir().unwrap();
            let mut store = Store::open(&temporary.path().join("db")).unwrap();
            discovery_root(&mut store);
            let root = store.root("root").unwrap();
            if !already_incomplete {
                store
                    .conn
                    .execute("DELETE FROM incomplete_roots", [])
                    .unwrap();
            }
            let summary = terminal_summary(17);
            store
                .save_foreground_summary(store.foreground_context().unwrap(), &summary)
                .unwrap();
            let origin = Path::new("/root/a%_/Cargo.lock");
            let original = item(
                "origin",
                origin.to_str().unwrap(),
                "download",
                100_000_000,
                true,
            );
            let candidates = [
                original.clone(),
                item(
                    "origin-child",
                    "/root/a%_/Cargo.lock/old/target",
                    "cargo",
                    1,
                    false,
                ),
                item("target", "/root/a%_/target", "cargo", 100_000_000, true),
                item(
                    "target-child",
                    "/root/a%_/target/old/node_modules",
                    "node",
                    1,
                    false,
                ),
                item(
                    "unrelated",
                    "/root/a%_/node_modules",
                    "node",
                    100_000_000,
                    true,
                ),
                item(
                    "prefix-sibling",
                    "/root/a%_/target-other",
                    "cargo",
                    1,
                    false,
                ),
                item("neighbor", "/root/a%_other/target", "cargo", 1, false),
            ];
            let suppressed = [
                item(
                    "hidden-origin",
                    "/root/a%_/Cargo.lock/new/target",
                    "cargo",
                    1,
                    false,
                ),
                item(
                    "hidden-target",
                    "/root/a%_/target/new/node_modules",
                    "node",
                    1,
                    false,
                ),
                item(
                    "hidden-sibling",
                    "/root/a%_/other/target",
                    "cargo",
                    1,
                    false,
                ),
                item("hidden-parent", "/root/a%_", "cargo", 1, false),
            ];
            save(&mut store, candidates.to_vec());
            save(&mut store, suppressed.to_vec());
            for candidate in &suppressed {
                store.suppress_candidate(&candidate.id).unwrap();
            }
            let visible = store.candidates().unwrap();
            claim(&mut store, origin);
            let refresh = store.begin_cargo_lock_refresh(&root, origin).unwrap();
            assert_eq!(refresh.scope.as_deref(), origin.to_str());
            assert_eq!(store.candidates().unwrap(), visible);
            let mut updated = original;
            updated.fingerprint = "new lock evidence".into();
            save(&mut store, vec![updated.clone()]);
            save(&mut store, suppressed.to_vec());
            store
                .finish_scope_refresh(
                    &refresh,
                    &ScanStats {
                        complete: true,
                        ..Default::default()
                    },
                    false,
                )
                .unwrap();
            assert_eq!(store.candidate("origin").unwrap(), updated);
            for id in ["origin-child", "target", "target-child"] {
                assert!(
                    store.candidate(id).is_err(),
                    "{id} was not observed in either footprint"
                );
            }
            for candidate in &candidates[4..] {
                assert_eq!(store.candidate(&candidate.id).unwrap(), *candidate);
            }
            for candidate in &suppressed[..2] {
                assert_eq!(store.candidate(&candidate.id).unwrap(), *candidate);
            }
            for candidate in &suppressed[2..] {
                assert!(
                    store.candidate(&candidate.id).is_err(),
                    "Unrelated cleanup suppression must survive"
                );
            }
            assert_eq!(store.incomplete("root").unwrap(), already_incomplete);
            assert_eq!(
                saved_summary(&store),
                Some(serde_json::to_value(summary).unwrap())
            );
            assert!(!store.has_pending_scopes().unwrap());
            assert!(store.history().unwrap().is_empty());
            assert_eq!(store.wallet().unwrap().credited_bytes, 0);
        }
    }

    #[test]
    fn cargo_lock_refresh_replays_the_origin_and_preserves_late_scope_order() {
        for interruption in ["claimed", "begun", "cancelled"] {
            let temporary = tempfile::tempdir().unwrap();
            let database = temporary.path().join("db");
            let origin = Path::new("/root/project/Cargo.lock");
            let target = item("target", "/root/project/target", "cargo", 100_000_000, true);
            {
                let mut store = Store::open(&database).unwrap();
                discovery_root(&mut store);
                let root = store.root("root").unwrap();
                save(&mut store, vec![target.clone()]);
                claim(&mut store, origin);
                let refresh = if interruption == "claimed" {
                    None
                } else {
                    Some(store.begin_cargo_lock_refresh(&root, origin).unwrap())
                };
                for path in [
                    "/root/other",
                    origin.to_str().unwrap(),
                    "/root/project/target/late",
                ] {
                    store.enqueue_scope("root", Path::new(path)).unwrap();
                }
                if interruption == "cancelled" {
                    store
                        .finish_scope_refresh(
                            refresh.as_ref().unwrap(),
                            &ScanStats {
                                cancelled: true,
                                ..Default::default()
                            },
                            true,
                        )
                        .unwrap();
                }
                assert_eq!(store.candidate("target").unwrap(), target);
            }
            let mut store = Store::open(&database).unwrap();
            let root = store.root("root").unwrap();
            assert_eq!(
                queued_paths(&store),
                [
                    "/root/other",
                    "/root/project/Cargo.lock",
                    "/root/project/target/late"
                ]
            );
            assert!(store.incomplete("root").unwrap());
            for path in ["/root/other", origin.to_str().unwrap()] {
                assert_eq!(
                    store.take_scope().unwrap(),
                    Some(("root".into(), path.into()))
                );
                if path != origin.to_str().unwrap() {
                    store.finish_scope("root", Path::new(path)).unwrap();
                }
            }
            let refresh = store.begin_cargo_lock_refresh(&root, origin).unwrap();
            assert!(
                !store.has_pending_scopes().unwrap(),
                "Only the older target event is coalesced"
            );
            for path in [
                "/root/project/target/new",
                origin.to_str().unwrap(),
                "/root/after",
            ] {
                store.enqueue_scope("root", Path::new(path)).unwrap();
            }
            save(&mut store, vec![target.clone()]);
            store
                .finish_scope_refresh(
                    &refresh,
                    &ScanStats {
                        complete: true,
                        ..Default::default()
                    },
                    false,
                )
                .unwrap();
            assert_eq!(store.candidate("target").unwrap(), target);
            assert!(
                store.incomplete("root").unwrap(),
                "A recovered partial root stays incomplete"
            );
            for path in [
                "/root/project/target/new",
                origin.to_str().unwrap(),
                "/root/after",
            ] {
                assert_eq!(
                    store.take_scope().unwrap(),
                    Some(("root".into(), path.into()))
                );
                store.finish_scope("root", Path::new(path)).unwrap();
            }
            assert!(!store.has_pending_scopes().unwrap());
            assert!(store.history().unwrap().is_empty());
            assert_eq!(store.wallet().unwrap().credited_bytes, 0);
        }
    }

    #[test]
    fn cargo_lock_refresh_failures_are_atomic_and_partial_results_do_not_prune() {
        let temporary = tempfile::tempdir().unwrap();
        let mut store = Store::open(&temporary.path().join("db")).unwrap();
        discovery_root(&mut store);
        let root = store.root("root").unwrap();
        store
            .conn
            .execute("DELETE FROM incomplete_roots", [])
            .unwrap();
        let origin = Path::new("/root/project/Cargo.lock");
        let candidates = vec![
            item(
                "origin",
                origin.to_str().unwrap(),
                "download",
                100_000_000,
                true,
            ),
            item("target", "/root/project/target", "cargo", 100_000_000, true),
        ];
        save(&mut store, candidates.clone());
        store
            .save_stats(
                "root",
                &ScanStats {
                    entries: 17,
                    complete: true,
                    ..Default::default()
                },
            )
            .unwrap();
        let cached = store.candidates().unwrap();
        claim(&mut store, origin);
        store
            .enqueue_scope("root", Path::new("/root/project/target/older"))
            .unwrap();
        store
            .enqueue_scope("root", Path::new("/root/other"))
            .unwrap();
        let pending = queued_paths(&store);
        store.conn.execute_batch("CREATE TEMP TRIGGER fail_cargo_start BEFORE INSERT ON refresh_seen WHEN NEW.candidate_id='target' BEGIN SELECT RAISE(ABORT,'disposable second footprint failure'); END;").unwrap();
        assert!(store.begin_cargo_lock_refresh(&root, origin).is_err());
        assert_eq!(queued_paths(&store), pending);
        assert!(!store.incomplete("root").unwrap());
        assert!(store.take_scope().unwrap().is_none());
        assert_eq!(store.candidates().unwrap(), cached);
        store
            .conn
            .execute_batch("DROP TRIGGER fail_cargo_start")
            .unwrap();
        let stale = store.begin_cargo_lock_refresh(&root, origin).unwrap();
        let current = store.begin_cargo_lock_refresh(&root, origin).unwrap();
        for path in [origin.to_str().unwrap(), "/root/project/target/late"] {
            store.enqueue_scope("root", Path::new(path)).unwrap();
        }
        let pending = queued_paths(&store);
        let complete = ScanStats {
            entries: 23,
            complete: true,
            ..Default::default()
        };
        assert!(
            store
                .finish_scope_refresh(&stale, &complete, false)
                .is_err()
        );
        store.conn.execute_batch("CREATE TEMP TRIGGER fail_cargo_finish BEFORE DELETE ON active_scopes BEGIN SELECT RAISE(ABORT,'disposable acknowledgement failure'); END;").unwrap();
        assert!(
            store
                .finish_scope_refresh(&current, &complete, false)
                .is_err()
        );
        assert_eq!(store.candidates().unwrap(), cached);
        for candidate in &candidates {
            assert_eq!(store.candidate(&candidate.id).unwrap(), *candidate);
        }
        assert_eq!(queued_paths(&store), pending);
        assert_eq!(store.latest_stats().unwrap().entries, 17);
        assert!(store.incomplete("root").unwrap());
        assert!(store.take_scope().unwrap().is_none());
        store
            .conn
            .execute_batch("DROP TRIGGER fail_cargo_finish")
            .unwrap();
        store
            .finish_scope_refresh(
                &current,
                &ScanStats {
                    entries: 23,
                    errors: 1,
                    ..Default::default()
                },
                false,
            )
            .unwrap();
        assert_eq!(store.candidates().unwrap(), cached);
        assert_eq!(queued_paths(&store), pending);
        assert!(store.incomplete("root").unwrap());
        assert_eq!(store.latest_stats().unwrap().errors, 1);
        assert!(store.take_scope().unwrap().is_some());
    }

    #[test]
    fn cargo_lock_refresh_requires_its_exact_claim_and_preserves_parent_subsumption() {
        let temporary = tempfile::tempdir().unwrap();
        let mut store = Store::open(&temporary.path().join("validation")).unwrap();
        discovery_root(&mut store);
        let root = store.root("root").unwrap();
        for path in [
            "/root",
            "/root/project/Cargo.toml",
            "/root/project/cargo.lock",
            "/root/target/project/Cargo.lock",
            "/root/node_modules/Cargo.lock",
            "/root/Library/project/Cargo.lock",
            "/root/../other/Cargo.lock",
            "/elsewhere/Cargo.lock",
            "relative/Cargo.lock",
        ] {
            assert!(
                store
                    .begin_cargo_lock_refresh(&root, Path::new(path))
                    .is_err(),
                "{path}"
            );
        }
        let origin = Path::new("/root/project/Cargo.lock");
        assert!(
            store.begin_cargo_lock_refresh(&root, origin).is_err(),
            "A durable claim is required"
        );
        claim(&mut store, origin);
        save(
            &mut store,
            vec![
                item(
                    "origin",
                    origin.to_str().unwrap(),
                    "download",
                    100_000_000,
                    true,
                ),
                item("enclosing", "/root/project", "cargo", 1, false),
            ],
        );
        assert!(store.begin_cargo_lock_refresh(&root, origin).is_err());
        store.discard_candidate("enclosing").unwrap();
        let refresh = store.begin_cargo_lock_refresh(&root, origin).unwrap();
        store
            .finish_scope_refresh(&refresh, &ScanStats::default(), false)
            .unwrap();

        for (index, parent) in ["/root/project", "/root"].into_iter().enumerate() {
            let mut store = Store::open(&temporary.path().join(format!("parent-{index}"))).unwrap();
            discovery_root(&mut store);
            let root = store.root("root").unwrap();
            store.enqueue_scope("root", origin).unwrap();
            store
                .enqueue_scope("root", Path::new("/root/other"))
                .unwrap();
            store.enqueue_scope("root", Path::new(parent)).unwrap();
            store.enqueue_scope("root", origin).unwrap();
            if parent != "/root" {
                assert_eq!(
                    store.take_scope().unwrap(),
                    Some(("root".into(), "/root/other".into()))
                );
                store
                    .finish_scope("root", Path::new("/root/other"))
                    .unwrap();
            }
            assert_eq!(
                store.take_scope().unwrap(),
                Some(("root".into(), parent.into()))
            );
            assert!(store.begin_cargo_lock_refresh(&root, origin).is_err());
            assert!(!store.has_pending_scopes().unwrap());
            store.finish_scope("root", Path::new(parent)).unwrap();
        }
    }

    #[test]
    fn compound_start_rolls_back_and_interrupted_generation_recovers_exact_scopes() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("db");
        let requested = Path::new("/root/a/first");
        let resolved = Path::new("/root/a");
        let old = item("old", "/root/a/target", "cargo", 100_000_000, true);
        let hidden = item("hidden", "/root/a/node_modules", "node", 100_000_000, true);
        {
            let mut store = Store::open(&db).unwrap();
            discovery_root(&mut store);
            let root = store.root("root").unwrap();
            store
                .conn
                .execute("DELETE FROM incomplete_roots", [])
                .unwrap();
            save(&mut store, vec![old.clone(), hidden.clone()]);
            store.suppress_candidate("hidden").unwrap();
            claim(&mut store, requested);
            store
                .enqueue_scope("root", Path::new("/root/a/second"))
                .unwrap();
            store.enqueue_scope("root", Path::new("/root/b")).unwrap();
            store.conn.execute_batch("CREATE TEMP TRIGGER fail_refresh_start BEFORE DELETE ON candidate_tombstones BEGIN SELECT RAISE(ABORT,'disposable start failure'); END;").unwrap();
            assert!(
                store
                    .begin_scope_refresh(&root, requested, Some(resolved))
                    .is_err()
            );
            assert!(
                !store.incomplete("root").unwrap(),
                "A failed start must not partially change coverage"
            );
            assert_eq!(queued_paths(&store), ["/root/a/second", "/root/b"]);
            assert!(
                store.take_scope().unwrap().is_none(),
                "The original claim must remain active"
            );
            assert_eq!(store.candidate("old").unwrap(), old);
            save(&mut store, vec![hidden.clone()]);
            assert!(
                store.candidate("hidden").is_err(),
                "A rolled-back start must retain cleanup suppression"
            );
            store
                .conn
                .execute_batch("DROP TRIGGER fail_refresh_start")
                .unwrap();
            let _refresh = store
                .begin_scope_refresh(&root, requested, Some(resolved))
                .unwrap();
            assert!(store.incomplete("root").unwrap());
            assert_eq!(queued_paths(&store), ["/root/b"]);
            save(&mut store, vec![hidden.clone()]);
            assert_eq!(store.candidate("hidden").unwrap(), hidden);
            store
                .enqueue_scope("root", Path::new("/root/a/late"))
                .unwrap();
            // Drop without finalization, as after an interrupted traversal.
        }
        let mut recovered = Store::open(&db).unwrap();
        assert_eq!(queued_paths(&recovered), ["/root/a", "/root/b"]);
        assert!(recovered.incomplete("root").unwrap());
        assert_eq!(recovered.candidate("old").unwrap(), old);
        assert!(recovered.take_scope().unwrap().is_some());
        assert!(recovered.history().unwrap().is_empty());
        assert_eq!(recovered.wallet().unwrap().credited_bytes, 0);
    }

    #[test]
    fn compound_finalize_failure_preserves_rows_stats_cache_and_recoverable_claim() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("db");
        let old = item("old", "/root/a/target", "cargo", 100_000_000, true);
        let mut store = Store::open(&db).unwrap();
        discovery_root(&mut store);
        let root = store.root("root").unwrap();
        store
            .conn
            .execute("DELETE FROM incomplete_roots", [])
            .unwrap();
        save(&mut store, vec![old.clone()]);
        store
            .save_stats(
                "root",
                &ScanStats {
                    entries: 17,
                    complete: true,
                    ..Default::default()
                },
            )
            .unwrap();
        let cached = store.candidates().unwrap();
        claim(&mut store, &root.path);
        let refresh = store.begin_scope_refresh(&root, &root.path, None).unwrap();
        store
            .enqueue_scope("root", Path::new("/root/late"))
            .unwrap();
        store.conn.execute_batch("CREATE TEMP TRIGGER fail_scope_ack BEFORE DELETE ON active_scopes BEGIN SELECT RAISE(ABORT,'disposable acknowledgement failure'); END;").unwrap();
        let complete = ScanStats {
            entries: 23,
            complete: true,
            ..Default::default()
        };
        assert!(
            store
                .finish_scope_refresh(&refresh, &complete, false)
                .is_err()
        );
        assert_eq!(store.candidate("old").unwrap(), old);
        assert_eq!(store.candidates().unwrap(), cached);
        assert_eq!(store.latest_stats().unwrap().entries, 17);
        assert!(store.incomplete("root").unwrap());
        assert_eq!(queued_paths(&store), ["/root/late"]);
        assert!(store.take_scope().unwrap().is_none());
        // A second connection after dropping the failed process recovers the
        // original full scope; neither pruning nor acknowledgement escaped.
        drop(store);
        let mut store = Store::open(&db).unwrap();
        assert_eq!(queued_paths(&store), ["/root"]);
        assert_eq!(store.candidate("old").unwrap(), old);
        assert_eq!(store.latest_stats().unwrap().entries, 17);
        assert_eq!(
            store.take_scope().unwrap(),
            Some(("root".into(), root.path.clone()))
        );
        let refresh = store.begin_scope_refresh(&root, &root.path, None).unwrap();
        store
            .enqueue_scope("root", Path::new("/root/after-restart"))
            .unwrap();
        assert_eq!(store.candidates().unwrap(), cached);
        store
            .finish_scope_refresh(&refresh, &complete, false)
            .unwrap();
        assert!(
            store.candidates().unwrap().is_empty(),
            "Committed pruning must invalidate the visible cache"
        );
        assert_eq!(store.latest_stats().unwrap().entries, 23);
        assert!(!store.incomplete("root").unwrap());
        assert_eq!(queued_paths(&store), ["/root/after-restart"]);
        assert!(
            store
                .finish_scope_refresh(&refresh, &complete, false)
                .is_err(),
            "A completed generation cannot acknowledge new work twice"
        );
    }

    #[test]
    fn compound_partial_and_cancelled_results_preserve_findings_and_late_events() {
        for cancelled in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let mut store = Store::open(&temp.path().join("db")).unwrap();
            discovery_root(&mut store);
            let root = store.root("root").unwrap();
            store
                .conn
                .execute("DELETE FROM incomplete_roots", [])
                .unwrap();
            let old = item("old", "/root/a/target", "cargo", 100_000_000, true);
            save(&mut store, vec![old.clone()]);
            let requested = Path::new("/root/a/first");
            claim(&mut store, requested);
            let refresh = store
                .begin_scope_refresh(&root, requested, Some(Path::new("/root/a")))
                .unwrap();
            store
                .enqueue_scope("root", Path::new("/root/a/late"))
                .unwrap();
            let stats = ScanStats {
                entries: 11,
                cancelled,
                errors: u64::from(!cancelled),
                ..Default::default()
            };
            store
                .finish_scope_refresh(&refresh, &stats, cancelled)
                .unwrap();
            assert_eq!(store.candidate("old").unwrap(), old);
            assert!(store.incomplete("root").unwrap());
            let saved = store.latest_stats().unwrap();
            assert_eq!(saved.entries, 11);
            assert_eq!(saved.cancelled, cancelled);
            assert_eq!(saved.errors, u64::from(!cancelled));
            assert!(!saved.complete);
            let expected = if cancelled {
                vec!["/root/a/first", "/root/a/late"]
            } else {
                vec!["/root/a/late"]
            };
            assert_eq!(queued_paths(&store), expected);
            assert!(
                store.take_scope().unwrap().is_some(),
                "Finalization must release the active claim"
            );
        }
    }

    #[test]
    fn compound_success_preserves_prior_partial_coverage() {
        for already_incomplete in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let mut store = Store::open(&temp.path().join("db")).unwrap();
            discovery_root(&mut store);
            let root = store.root("root").unwrap();
            if !already_incomplete {
                store
                    .conn
                    .execute("DELETE FROM incomplete_roots", [])
                    .unwrap();
            }
            let old = item("old", "/root/a/target", "cargo", 100_000_000, true);
            save(&mut store, vec![old.clone()]);
            let path = Path::new("/root/a");
            claim(&mut store, path);
            let refresh = store.begin_scope_refresh(&root, path, Some(path)).unwrap();
            let complete = ScanStats {
                complete: true,
                ..Default::default()
            };
            store
                .finish_scope_refresh(&refresh, &complete, false)
                .unwrap();
            assert_eq!(store.incomplete("root").unwrap(), already_incomplete);
            assert!(store.candidates().unwrap().is_empty());
        }
    }

    #[test]
    fn compound_finalize_rejects_a_superseded_same_scope_generation() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::open(&temp.path().join("db")).unwrap();
        discovery_root(&mut store);
        let root = store.root("root").unwrap();
        let old = item("old", "/root/a/target", "cargo", 100_000_000, true);
        save(&mut store, vec![old.clone()]);
        let path = Path::new("/root/a");
        claim(&mut store, path);
        let stale = store.begin_scope_refresh(&root, path, Some(path)).unwrap();
        let current = store.begin_scope_refresh(&root, path, Some(path)).unwrap();
        store
            .enqueue_scope("root", Path::new("/root/a/late"))
            .unwrap();
        let complete = ScanStats {
            complete: true,
            ..Default::default()
        };
        assert!(
            store
                .finish_scope_refresh(&stale, &complete, false)
                .is_err()
        );
        assert!(store.take_scope().unwrap().is_none());
        assert_eq!(store.candidate("old").unwrap(), old);
        assert!(store.incomplete("root").unwrap());
        assert_eq!(queued_paths(&store), ["/root/a/late"]);
        // The current generation remains usable; acknowledging its partial
        // result retains the old finding and leaves the late event pending.
        store
            .finish_scope_refresh(&current, &ScanStats::default(), false)
            .unwrap();
        assert_eq!(store.candidate("old").unwrap(), old);
        assert_eq!(
            store.take_scope().unwrap(),
            Some(("root".into(), "/root/a/late".into()))
        );
    }

    #[test]
    fn cancellation_before_refresh_atomically_requeues_and_marks_incomplete() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::open(&temp.path().join("db")).unwrap();
        discovery_root(&mut store);
        let root = store.root("root").unwrap();
        store
            .conn
            .execute("DELETE FROM incomplete_roots", [])
            .unwrap();
        claim(&mut store, &root.path);
        store
            .enqueue_scope("root", Path::new("/root/late"))
            .unwrap();
        store.conn.execute_batch("CREATE TEMP TRIGGER fail_cancel_ack BEFORE DELETE ON active_scopes BEGIN SELECT RAISE(ABORT,'disposable cancellation failure'); END;").unwrap();
        assert!(store.cancel_claimed_scope("root", &root.path).is_err());
        assert!(!store.incomplete("root").unwrap());
        assert_eq!(queued_paths(&store), ["/root/late"]);
        assert!(store.take_scope().unwrap().is_none());
        store
            .conn
            .execute_batch("DROP TRIGGER fail_cancel_ack")
            .unwrap();
        store.cancel_claimed_scope("root", &root.path).unwrap();
        assert!(store.incomplete("root").unwrap());
        assert_eq!(queued_paths(&store), ["/root"]);
        assert!(store.cancel_claimed_scope("root", &root.path).is_err());
        assert_eq!(
            store.take_scope().unwrap(),
            Some(("root".into(), root.path))
        );
    }

    #[test]
    fn interrupted_active_scope_and_refresh_recover_without_losing_old_findings() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("db");
        let path = Path::new("/root/project");
        let old = item("old", "/root/project/target", "cargo", 100_000_000, true);
        {
            let mut store = Store::open(&db).unwrap();
            discovery_root(&mut store);
            save(&mut store, vec![old.clone()]);
            store.enqueue_scope("root", path).unwrap();
            assert!(store.take_scope().unwrap().is_some());
            store.begin_refresh("root", Some(path)).unwrap();
            store
                .enqueue_scope("root", Path::new("/root/project/new-child"))
                .unwrap();
        }
        let mut store = Store::open(&db).unwrap();
        assert_eq!(store.candidate("old").unwrap(), old);
        assert!(store.incomplete("root").unwrap());
        assert_eq!(
            store.take_scope().unwrap(),
            Some(("root".into(), path.into()))
        );
        assert!(!store.has_pending_scopes().unwrap());
        store.begin_refresh("root", Some(path)).unwrap();
        store.finish_refresh("root", Some(path), true).unwrap();
        store.finish_scope("root", path).unwrap();
        assert!(store.candidate("old").is_err());
        drop(store);
        assert!(!Store::open(&db).unwrap().has_pending_scopes().unwrap());
    }

    #[test]
    fn provisional_and_partial_refreshes_keep_completed_findings_visible() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("db")).unwrap();
        let path = Path::new("/root/project");
        let old = item("old", "/root/project/target", "cargo", 100_000_000, true);
        let missing = item(
            "missing",
            "/root/project/node_modules",
            "node",
            200_000_000,
            true,
        );
        save(&mut store, vec![old.clone(), missing]);
        let visible = store.candidates().unwrap();
        store.begin_refresh("root", Some(path)).unwrap();
        assert_eq!(store.candidates().unwrap(), visible);
        let mut provisional = old.clone();
        provisional.provisional = true;
        provisional.allocated_bytes = 0;
        provisional.suggestion_eligible = false;
        provisional.fingerprint.clear();
        save(&mut store, vec![provisional]);
        assert_eq!(store.candidate("old").unwrap(), old);
        assert_eq!(store.candidates().unwrap(), visible);
        store.finish_refresh("root", Some(path), false).unwrap();
        assert!(store.candidate("missing").is_ok());
        store.begin_refresh("root", Some(path)).unwrap();
        let mut updated = old;
        updated.allocated_bytes = 300_000_000;
        save(&mut store, vec![updated.clone()]);
        store.finish_refresh("root", Some(path), true).unwrap();
        assert_eq!(store.candidate("old").unwrap(), updated);
        assert!(store.candidate("missing").is_err());
    }

    #[test]
    fn scoped_refresh_only_removes_unseen_rows_in_the_exact_scope() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("db")).unwrap();
        save(
            &mut store,
            vec![
                item("a", "/root/a%_/target", "cargo", 100_000_000, true),
                item("b", "/root/abc/target", "cargo", 100_000_000, true),
                item("c", "/root/a%_other/target", "cargo", 100_000_000, true),
            ],
        );
        let scope = Some(Path::new("/root/a%_/target/descendant"));
        store.begin_refresh("root", scope).unwrap();
        assert!(store.finish_refresh("root", None, true).is_err());
        store.finish_refresh("root", scope, true).unwrap();
        assert!(store.candidate("a").is_err());
        assert!(store.candidate("b").is_ok());
        assert!(store.candidate("c").is_ok());
        store.begin_refresh("root", None).unwrap();
        store.finish_refresh("root", None, true).unwrap();
        assert!(store.candidates().unwrap().is_empty());
    }

    #[test]
    fn cleanup_tombstones_reject_late_batches_until_that_scope_is_refreshed() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("db");
        let candidate = item("old", "/root/project/target", "cargo", 100_000_000, true);
        let mut store = Store::open(&db).unwrap();
        discovery_root(&mut store);
        save(&mut store, vec![candidate.clone()]);
        store.begin_refresh("root", None).unwrap();
        store.suppress_candidate("old").unwrap();
        save(&mut store, vec![candidate.clone()]);
        assert!(store.candidate("old").is_err());
        store.finish_refresh("root", None, false).unwrap();
        drop(store);
        let mut store = Store::open(&db).unwrap();
        store
            .begin_refresh("root", Some(Path::new("/root/project-other")))
            .unwrap();
        save(&mut store, vec![candidate.clone()]);
        assert!(store.candidate("old").is_err());
        store
            .finish_refresh("root", Some(Path::new("/root/project-other")), true)
            .unwrap();
        let scope = Some(Path::new("/root/project"));
        store.begin_refresh("root", scope).unwrap();
        save(&mut store, vec![candidate.clone()]);
        store.finish_refresh("root", scope, true).unwrap();
        assert_eq!(store.candidate("old").unwrap(), candidate);
    }

    #[test]
    fn suppressing_an_already_missing_id_still_blocks_its_late_batch() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("db")).unwrap();
        let candidate = item("old", "/root/project/target", "cargo", 100_000_000, true);
        store.suppress_candidate("old").unwrap();
        save(&mut store, vec![candidate.clone()]);
        assert!(store.candidate("old").is_err());
        store
            .begin_refresh("root", Some(Path::new("/root/project")))
            .unwrap();
        save(&mut store, vec![candidate]);
        assert!(store.candidate("old").is_ok());
    }

    #[test]
    fn removing_a_root_removes_its_durable_discovery_state() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("db");
        let mut store = Store::open(&db).unwrap();
        discovery_root(&mut store);
        save(
            &mut store,
            vec![item("old", "/root/a/target", "cargo", 100_000_000, true)],
        );
        store.enqueue_scope("root", Path::new("/root/a")).unwrap();
        assert!(store.take_scope().unwrap().is_some());
        store.enqueue_scope("root", Path::new("/root/b")).unwrap();
        store.begin_refresh("root", None).unwrap();
        store.suppress_candidate("old").unwrap();
        store.remove_root("root").unwrap();
        for table in [
            "pending_scopes",
            "active_scopes",
            "refreshes",
            "refresh_seen",
            "candidate_tombstones",
        ] {
            let count: u64 = store
                .conn
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 0, "{table}");
        }
        drop(store);
        assert!(!Store::open(&db).unwrap().has_pending_scopes().unwrap());
    }

    #[test]
    fn ranking_only_returns_verified_meaningful_suggestions_and_respects_keep() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("db")).unwrap();
        let mut blocked = item("blocked", "/root/blocked", "cargo", 900_000_000, true);
        blocked.blocked_reason = Some("Active project".into());
        save(
            &mut store,
            vec![
                item(
                    "personal",
                    "/root/Downloads/big.dmg",
                    "download",
                    9_000_000_000,
                    true,
                ),
                item("node", "/root/node/node_modules", "node", 200_000_000, true),
                item("cargo", "/root/cargo/target", "cargo", 100_000_000, true),
                item("tiny", "/root/tiny/target", "cargo", 999, true),
                item("fresh", "/root/fresh/target", "cargo", 300_000_000, false),
                item("venv", "/root/python/.venv", "venv", 800_000_000, true),
                item(
                    "webcache",
                    "/root/site/.next",
                    "webcache",
                    700_000_000,
                    true,
                ),
                blocked,
            ],
        );
        // Prefer cheap regenerated caches, then rebuild output, then artifacts
        // likely to need network downloads. Size orders within each cost class.
        assert_eq!(
            store
                .candidates()
                .unwrap()
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            ["webcache", "cargo", "venv", "node", "personal"]
        );
        store.keep("/root/cargo", true).unwrap();
        assert_eq!(store.candidates().unwrap().len(), 4);
        store.keep("/root/cargo", false).unwrap();
        assert_eq!(store.candidates().unwrap()[0].id, "webcache");
        let mut changed = store.candidate("cargo").unwrap();
        changed.suggestion_eligible = false;
        save(&mut store, vec![changed]);
        assert_eq!(store.candidates().unwrap().len(), 4);
    }

    #[test]
    fn category_thresholds_and_priority_are_applied_by_the_indexed_query() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("db")).unwrap();
        let kinds = [
            "cache",
            "log",
            "crashreport",
            "xcode",
            "installer",
            "archive",
            "download",
            "largefile",
        ];
        let mut rows = Vec::new();
        for kind in kinds {
            let minimum = crate::recommendations::minimum_bytes(kind);
            rows.push(item(kind, &format!("/root/{kind}"), kind, minimum, true));
            rows.push(item(
                &format!("small-{kind}"),
                &format!("/root/small-{kind}"),
                kind,
                minimum - 1,
                true,
            ));
        }
        rows.push(item(
            "unknown",
            "/root/unknown",
            "unrecognized",
            u64::MAX,
            true,
        ));
        save(&mut store, rows);
        assert_eq!(
            store
                .candidates()
                .unwrap()
                .iter()
                .map(|row| row.kind.as_str())
                .collect::<Vec<_>>(),
            [
                "log",
                "crashreport",
                "cache",
                "xcode",
                "installer",
                "download",
                "archive",
                "largefile"
            ]
        );
        // The 1 MB report and 10 MB log survive the query's size filter, while
        // a 500 MB personal file cannot displace first-group cleanup artifacts.
        assert_eq!(
            store.candidate("crashreport").unwrap().allocated_bytes,
            1_000_000
        );
    }

    #[test]
    fn duplicate_inputs_admit_whole_buckets_without_exceeding_500_files() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("db")).unwrap();
        discovery_root(&mut store);
        let mut rows = Vec::new();
        // These are indexed sizes only; no corresponding large files are made.
        // Aggregate allocated size orders the buckets exactly as listed.
        for (prefix, bytes, count) in [
            ("oversized", 1_000_000_000, 501),
            ("large-fit", 900_000_000, 498),
            ("cannot-fit-remainder", 800_000_000, 3),
            ("small-fit", 700_000_000, 2),
            ("after-full", 600_000_000, 2),
        ] {
            for index in 0..count {
                let id = format!("{prefix}-{index:03}");
                rows.push(item(&id, &format!("/root/{id}"), "download", bytes, true));
            }
        }
        save(&mut store, rows);

        let inputs = store.duplicate_inputs(&AtomicBool::new(false)).unwrap();
        assert_eq!(inputs.files.len(), 500);
        assert_eq!(inputs.indexed_files, 1006);
        assert_eq!(
            duplicate_bucket_counts(&inputs),
            BTreeMap::from([((1, 900_000_000), 498), ((1, 700_000_000), 2)]),
            "Oversized buckets must be skipped whole, leaving room for later complete buckets"
        );
        assert_eq!(inputs.skipped_buckets, 3);
        assert_eq!(inputs.skipped_bucket_files, 506);
        assert!(!inputs.bucket_limit_reached);
    }

    #[test]
    fn duplicate_inputs_keep_protected_rows_as_keeper_only() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("db")).unwrap();
        discovery_root(&mut store);
        save(
            &mut store,
            vec![
                item(
                    "protected",
                    "/root/café%_/clip.mov",
                    "download",
                    600_000_000,
                    true,
                ),
                item(
                    "copy",
                    "/root/other/copy.mov",
                    "download",
                    600_000_000,
                    true,
                ),
            ],
        );
        for (kept, overlaps) in [
            ("/root/café%_", true),
            ("/root/café%_/clip.mov", true),
            ("/root/café%_/clip.mov/previously-kept-child", true),
            ("/root/café%_/clip.mov-other", false),
            ("/root/caféXX/clip.mov", false),
        ] {
            store.keep(kept, true).unwrap();
            // Populate the ordinary page after Keep has hidden any protected row.
            assert_eq!(
                store.candidates().unwrap().len(),
                if overlaps { 1 } else { 2 }
            );
            let inputs = store.duplicate_inputs(&AtomicBool::new(false)).unwrap();
            assert_eq!(inputs.indexed_files, 2);
            assert_eq!(
                inputs
                    .files
                    .iter()
                    .map(|input| (input.candidate.id.as_str(), input.keeper_only))
                    .collect::<BTreeMap<_, _>>(),
                BTreeMap::from([("copy", false), ("protected", overlaps)]),
                "{kept}"
            );
            store.keep(kept, false).unwrap();
        }
        store.keep("/root", true).unwrap();
        assert!(store.candidates().unwrap().is_empty());
        let inputs = store.duplicate_inputs(&AtomicBool::new(false)).unwrap();
        assert_eq!(inputs.files.len(), 2);
        assert!(inputs.files.iter().all(|input| input.keeper_only));
        assert_eq!(store.kept().unwrap(), ["/root"]);
    }

    #[test]
    fn duplicate_inputs_are_independent_of_visible_limit_and_category_ranks() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("db")).unwrap();
        discovery_root(&mut store);
        let mut personal = Vec::new();
        // Each same-size pair crosses ordinary presentation categories.
        for (kind, bytes) in [
            ("installer", 500_000_000),
            ("archive", 500_000_000),
            ("download", 600_000_000),
            ("largefile", 600_000_000),
        ] {
            let mut candidate = item(kind, &format!("/root/{kind}"), kind, bytes, true);
            candidate.eligible_permanent = false;
            personal.push(candidate);
        }
        save(&mut store, personal);
        assert_eq!(store.candidates().unwrap().len(), 4);
        save(
            &mut store,
            (0..500)
                .map(|index| {
                    let id = format!("cargo-{index:03}");
                    item(
                        &id,
                        &format!("/root/{id}/target"),
                        "cargo",
                        100_000_000,
                        true,
                    )
                })
                .collect(),
        );
        let visible = store.candidates().unwrap();
        assert_eq!(visible.len(), 500);
        assert!(visible.iter().all(|candidate| candidate.kind == "cargo"));

        let inputs = store.duplicate_inputs(&AtomicBool::new(false)).unwrap();
        assert_eq!(inputs.indexed_files, 4);
        assert_eq!(
            inputs
                .files
                .iter()
                .map(|input| input.candidate.id.as_str())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["installer", "archive", "download", "largefile"])
        );
        assert_eq!(
            duplicate_bucket_counts(&inputs),
            BTreeMap::from([((1, 500_000_000), 2), ((1, 600_000_000), 2)])
        );
    }

    #[test]
    fn duplicate_inputs_exclude_unverified_ineligible_and_nonpersonal_rows() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("db")).unwrap();
        discovery_root(&mut store);
        let mut rows = vec![
            item("match-a", "/root/match-a", "download", 600_000_000, true),
            item("match-b", "/root/match-b", "download", 600_000_000, true),
            item(
                "singleton",
                "/root/singleton",
                "download",
                700_000_000,
                true,
            ),
        ];
        for reason in [
            "provisional",
            "ineligible",
            "blocked",
            "nonpersonal",
            "permanent",
            "zero-size",
            "below-minimum",
            "oversized-json",
            "missing-root",
        ] {
            for index in 0..2 {
                let id = format!("{reason}-{index}");
                let mut candidate =
                    item(&id, &format!("/root/{id}"), "download", 600_000_000, true);
                match reason {
                    "provisional" => {} // Inject the stored flag below, after normal saving.
                    "ineligible" => candidate.suggestion_eligible = false,
                    "blocked" => candidate.blocked_reason = Some("Fixture activity guard".into()),
                    "nonpersonal" => candidate.kind = "cargo".into(),
                    "permanent" => candidate.eligible_permanent = true,
                    "zero-size" => candidate.identity.size = 0,
                    "below-minimum" => {
                        candidate.allocated_bytes =
                            crate::recommendations::minimum_bytes("download") - 1;
                    }
                    "oversized-json" => candidate.evidence = "x".repeat(17_000),
                    "missing-root" => candidate.root_id = "missing".into(),
                    _ => unreachable!(),
                }
                rows.push(candidate);
            }
        }
        save(&mut store, rows);
        // save_batch intentionally refuses provisional rows. Directly mark these
        // disposable records to exercise the query's own defensive predicate.
        store
            .conn
            .execute(
                "UPDATE candidates SET json=json_set(json,'$.provisional',json('true'))
                 WHERE id IN ('provisional-0','provisional-1')",
                [],
            )
            .unwrap();

        let inputs = store.duplicate_inputs(&AtomicBool::new(false)).unwrap();
        assert_eq!(
            inputs.indexed_files, 3,
            "Only eligible indexed rows are counted"
        );
        assert_eq!(
            inputs
                .files
                .iter()
                .map(|input| input.candidate.id.as_str())
                .collect::<Vec<_>>(),
            ["match-a", "match-b"],
            "Invalid rows cannot join an otherwise eligible size bucket"
        );
        assert_eq!(inputs.skipped_buckets, 0);
        assert_eq!(inputs.skipped_bucket_files, 0);
        assert!(!inputs.bucket_limit_reached);
    }

    #[test]
    fn duplicate_inputs_do_not_form_buckets_across_devices() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("db")).unwrap();
        discovery_root(&mut store);
        let mut other = store.root("root").unwrap();
        other.id = "other".into();
        other.path = "/other".into();
        other.identity.device = 2;
        store.add_root(&other).unwrap();
        let mut rows = Vec::new();
        for (root_id, root_path, device) in [("root", "/root", 1), ("other", "/other", 2)] {
            for (suffix, bytes) in [
                ("pair-a", 500_000_000),
                ("pair-b", 500_000_000),
                ("singleton", 800_000_000),
            ] {
                let id = format!("{root_id}-{suffix}");
                let mut candidate = item(
                    &id,
                    &format!("{root_path}/{suffix}"),
                    "download",
                    bytes,
                    true,
                );
                candidate.root_id = root_id.into();
                candidate.identity.device = device;
                rows.push(candidate);
            }
        }
        save(&mut store, rows);

        let inputs = store.duplicate_inputs(&AtomicBool::new(false)).unwrap();
        assert_eq!(inputs.indexed_files, 6);
        assert_eq!(
            duplicate_bucket_counts(&inputs),
            BTreeMap::from([((1, 500_000_000), 2), ((2, 500_000_000), 2)]),
            "One same-size file on each device is not a comparable bucket"
        );
        assert!(inputs.files.iter().all(|input| {
            input.root.id == input.candidate.root_id
                && input.root.identity.device == input.candidate.identity.device
        }));
        assert_eq!(inputs.skipped_buckets, 0);
    }

    #[test]
    fn duplicate_inputs_report_the_bucket_limit_without_partial_buckets() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("db")).unwrap();
        discovery_root(&mut store);
        let pair = |bucket: usize| {
            let bytes = 100_000_000 + bucket as u64;
            (0..2)
                .map(|member| {
                    let id = format!("bucket-{bucket:04}-{member}");
                    item(&id, &format!("/root/{id}"), "download", bytes, true)
                })
                .collect::<Vec<_>>()
        };
        let mut rows = Vec::new();
        for bucket in 0..1024 {
            rows.extend(pair(bucket));
        }
        save(&mut store, rows);
        for extra_bucket in [false, true] {
            if extra_bucket {
                save(&mut store, pair(1024));
            }
            let inputs = store.duplicate_inputs(&AtomicBool::new(false)).unwrap();
            assert_eq!(inputs.indexed_files, if extra_bucket { 2050 } else { 2048 });
            assert_eq!(inputs.files.len(), 500);
            let counts = duplicate_bucket_counts(&inputs);
            assert_eq!(counts.len(), 250);
            assert!(counts.values().all(|count| *count == 2));
            assert_eq!(inputs.skipped_buckets, 774);
            assert_eq!(inputs.skipped_bucket_files, 1548);
            assert_eq!(inputs.bucket_limit_reached, extra_bucket);
        }
    }

    #[test]
    fn kept_descendants_hide_enclosing_candidates_without_hiding_sibling_prefixes() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("db")).unwrap();
        let selected = item(
            "selected",
            "/root/café%_/project/.venv",
            "venv",
            100_000_000,
            true,
        );
        save(
            &mut store,
            vec![
                selected.clone(),
                item("other", "/root/other/target", "cargo", 100_000_000, true),
            ],
        );
        for (kept, overlaps) in [
            ("/root/café%_/project", true),
            ("/root/café%_/project/.venv", true),
            ("/root/café%_/project/.venv/kept_%.txt", true),
            ("/root/café%_/project/.venv-other/kept_%.txt", false),
            ("/root/café%_/project/.ven", false),
            ("/root/caféXX/project/.venv/kept_%.txt", false),
        ] {
            // Populate the cache first: Keep must invalidate the old presentation.
            assert_eq!(store.candidates().unwrap().len(), 2);
            store.keep(kept, true).unwrap();
            let visible = store.candidates().unwrap();
            assert_eq!(
                visible.iter().any(|candidate| candidate.id == selected.id),
                !overlaps,
                "{kept}"
            );
            assert!(visible.iter().any(|candidate| candidate.id == "other"));
            assert_eq!(store.kept().unwrap(), [kept]);
            assert_eq!(store.candidate(&selected.id).unwrap(), selected);
            store.keep(kept, false).unwrap();
        }
        assert_eq!(store.candidates().unwrap().len(), 2);
    }

    #[test]
    fn history_paging_returns_cursors_totals_and_monotonic_sequences() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("db")).unwrap();
        let root = Root {
            id: "root".into(),
            path: "/root".into(),
            kind: "projects".into(),
            identity: item("root", "/root", "cargo", 1, false).identity,
        };
        let candidate = item("artifact", "/root/target", "cargo", 100_000_000, true);
        for index in 1..=7 {
            let receipt = Receipt {
                id: format!("op-{index}"),
                path: "/root/target".into(),
                title: "Build artifacts".into(),
                operation: "permanent".into(),
                outcome: "removed".into(),
                detail: String::new(),
                created_at: now(),
                reported_bytes: 0,
                observed_bytes: 0,
                credited_bytes: 0,
                coins: 0,
                trash_path: None,
                can_restore: false,
                seq: None,
            };
            store
                .prepare_operation(&root, &candidate, &receipt, Path::new("/root/.stage"))
                .unwrap();
        }
        let mut cursor = None;
        let mut ids = Vec::new();
        let mut sequences = Vec::new();
        for expected_len in [3usize, 3, 1] {
            let (receipts, next_before, total) = store.history_page(cursor, 3).unwrap();
            assert_eq!(total, 7);
            assert_eq!(receipts.len(), expected_len);
            for receipt in &receipts {
                let seq = receipt.seq.expect("Paged receipts carry a sequence");
                assert!(
                    sequences.last().is_none_or(|last| seq < *last),
                    "Sequences must strictly decrease across pages"
                );
                sequences.push(seq);
                ids.push(receipt.id.clone());
            }
            assert_eq!(
                next_before,
                (expected_len == 3).then(|| *sequences.last().unwrap())
            );
            cursor = next_before;
        }
        assert_eq!(cursor, None);
        assert_eq!(
            ids,
            (1..=7)
                .rev()
                .map(|index| format!("op-{index}"))
                .collect::<Vec<_>>(),
            "Paging walks the complete ledger newest first without overlap"
        );
        // The bounded snapshot history is unchanged and never serializes seq.
        let unpaged = store.history().unwrap();
        assert_eq!(unpaged.len(), 7);
        assert!(unpaged.iter().all(|receipt| receipt.seq.is_none()));
        let encoded = serde_json::to_value(&unpaged[0]).unwrap();
        assert!(encoded.get("seq").is_none());
        // A full final page hands out one more cursor; it then proves empty.
        let (receipts, next_before, _) = store.history_page(None, 7).unwrap();
        assert_eq!(receipts.len(), 7);
        let (empty, none, total) = store.history_page(next_before, 7).unwrap();
        assert!(empty.is_empty() && none.is_none());
        assert_eq!(total, 7);
    }
    #[test]
    fn candidate_query_uses_the_rank_index_without_a_temporary_sort() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("db")).unwrap();
        let mut query = store
            .conn
            .prepare(&format!("EXPLAIN QUERY PLAN {}", SUGGESTIONS_SQL.as_str()))
            .unwrap();
        let details = query
            .query_map([], |r| r.get::<_, String>(3))
            .unwrap()
            .map(|r| r.unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(details.contains("candidate_suggestions_v5"), "{details}");
        assert!(!details.contains("USE TEMP B-TREE"), "{details}");
    }
    #[test]
    fn replacing_contained_roots_is_atomic_and_preserves_keep_and_rewards() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("db")).unwrap();
        let identity = item("x", "/root/projects/target", "cargo", 100_000_000, true).identity;
        let make_root = |id: &str, path: &str| Root {
            id: id.into(),
            path: path.into(),
            kind: "folder".into(),
            identity: identity.clone(),
        };
        store
            .add_root(&make_root("projects", "/root/projects"))
            .unwrap();
        store.add_root(&make_root("external", "/other")).unwrap();
        let mut mount = make_root("mount", "/root/mount");
        mount.identity.device += 1;
        store.add_root(&mount).unwrap();
        store
            .enqueue_scope("projects", Path::new("/root/projects/a"))
            .unwrap();
        assert!(store.take_scope().unwrap().is_some());
        store
            .enqueue_scope("projects", Path::new("/root/projects/b"))
            .unwrap();
        store.begin_refresh("projects", None).unwrap();
        store.keep("/root/projects/target", true).unwrap();
        store
            .conn
            .execute(
                "UPDATE wallet SET collected=17,remainder=42,credited=1700000042",
                [],
            )
            .unwrap();
        // The conflicting ID fails after scoped DELETEs; the transaction must roll back.
        assert!(
            store
                .authorize_root(&make_root("external", "/root"), true)
                .is_err()
        );
        assert!(store.root("projects").is_ok());
        assert_eq!(store.roots().unwrap().len(), 3);
        assert!(store.has_pending_scopes().unwrap());
        let replaced = store
            .authorize_root(&make_root("home", "/root"), true)
            .unwrap();
        assert_eq!(replaced, ["projects"]);
        assert!(store.root("projects").is_err());
        assert!(store.root("external").is_ok());
        assert!(store.root("mount").is_ok());
        assert!(store.incomplete("home").unwrap());
        assert_eq!(store.kept().unwrap(), ["/root/projects/target"]);
        assert_eq!(store.wallet().unwrap().collected_coins, 17);
        assert_eq!(store.wallet().unwrap().fractional_bytes, 42);
        assert!(!store.has_pending_scopes().unwrap());
        assert!(store.take_scope().unwrap().is_none());
        for table in ["active_scopes", "refreshes", "refresh_seen"] {
            let count: u64 = store
                .conn
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 0, "{table}");
        }
    }
    #[test]
    fn legacy_home_policy_upgrade_preserves_grant_identity_and_rewards() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("db");
        let mut store = Store::open(&db).unwrap();
        let candidate = item(
            "old-media",
            "/root/Music/target",
            "cargo",
            100_000_000,
            true,
        );
        let root = Root {
            id: "root".into(),
            path: "/root".into(),
            kind: "folder".into(),
            identity: candidate.identity.clone(),
        };
        store.add_root(&root).unwrap();
        save(&mut store, vec![candidate]);
        store.keep("/root/Projects/keep", true).unwrap();
        store
            .conn
            .execute(
                "UPDATE wallet SET collected=17,remainder=42,credited=1700000042",
                [],
            )
            .unwrap();
        let mut home = root.clone();
        home.kind = "home".into();
        let context = store.foreground_context().unwrap();
        let summary = terminal_summary(10);
        assert!(store.save_foreground_summary(context, &summary).unwrap());
        assert!(store.authorize_root(&home, false).is_err());
        let mut replaced_object = home.clone();
        replaced_object.identity.inode += 1;
        assert!(store.authorize_root(&replaced_object, true).is_err());
        assert_eq!(store.foreground_context().unwrap(), context);
        assert!(store.load_foreground_summary().unwrap().is_some());
        assert_eq!(store.root("root").unwrap().kind, "folder");
        assert_eq!(store.candidates().unwrap().len(), 1);
        assert_eq!(store.authorize_root(&home, true).unwrap(), ["root"]);
        assert_eq!(store.foreground_context().unwrap(), context + 1);
        assert!(store.load_foreground_summary().unwrap().is_none());
        assert!(!store.save_foreground_summary(context, &summary).unwrap());
        assert_eq!(store.root("root").unwrap().kind, "home");
        assert!(store.candidates().unwrap().is_empty());
        assert!(store.incomplete("root").unwrap());
        assert_eq!(store.kept().unwrap(), ["/root/Projects/keep"]);
        assert_eq!(store.wallet().unwrap().collected_coins, 17);
        assert_eq!(store.wallet().unwrap().fractional_bytes, 42);
        drop(store);
        let mut restarted = Store::open(&db).unwrap();
        assert_eq!(restarted.root("root").unwrap().kind, "home");
        assert_eq!(
            restarted.take_scope().unwrap(),
            Some(("root".into(), "/root".into()))
        );
    }

    #[test]
    fn scoped_deletion_never_matches_sibling_prefixes_or_sql_wildcards() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("db")).unwrap();
        save(
            &mut store,
            vec![
                item("a", "/root/a%_/target", "cargo", 100_000_000, true),
                item("b", "/root/abc/target", "cargo", 100_000_000, true),
                item("c", "/root/a%_other/target", "cargo", 100_000_000, true),
            ],
        );
        store
            .clear_scope("root", Some(Path::new("/root/a%_")))
            .unwrap();
        assert!(store.candidate("a").is_err());
        assert!(store.candidate("b").is_ok());
        assert!(store.candidate("c").is_ok());
    }
    #[test]
    fn legacy_index_is_hidden_and_requires_reconciliation_without_touching_rewards() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("db");
        let mut store = Store::open(&db).unwrap();
        let root = Root {
            id: "root".into(),
            path: "/root".into(),
            kind: "projects".into(),
            identity: item("x", "/root/x", "cargo", 1, false).identity,
        };
        store.add_root(&root).unwrap();
        let mut legacy =
            serde_json::to_value(item("old", "/root/old", "cargo", 100_000_000, true)).unwrap();
        legacy
            .as_object_mut()
            .unwrap()
            .remove("suggestion_eligible");
        store
            .conn
            .execute(
                "INSERT INTO candidates VALUES('old','root','/root/old',?1)",
                [legacy.to_string()],
            )
            .unwrap();
        store
            .conn
            .execute("DELETE FROM incomplete_roots", [])
            .unwrap();
        store
            .conn
            .execute("UPDATE index_version SET version=1", [])
            .unwrap();
        store
            .conn
            .execute(
                "UPDATE wallet SET collected=17,remainder=42,credited=1700000042",
                [],
            )
            .unwrap();
        let context = store.foreground_context().unwrap();
        let summary = terminal_summary(7);
        assert!(store.save_foreground_summary(context, &summary).unwrap());
        store.conn.execute_batch("CREATE TRIGGER fail_rule_summary BEFORE UPDATE ON foreground_state BEGIN SELECT RAISE(ABORT,'disposable rule invalidation failure'); END;").unwrap();
        drop(store);
        assert!(Store::open(&db).is_err());
        let unchanged = Connection::open(&db).unwrap();
        let (version, candidates): (u32, u64) = unchanged
            .query_row(
                "SELECT (SELECT version FROM index_version),(SELECT count(*) FROM candidates)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            (version, candidates),
            (1, 1),
            "A failed rule invalidation must roll back the index rebuild"
        );
        unchanged
            .execute_batch("DROP TRIGGER fail_rule_summary")
            .unwrap();
        drop(unchanged);
        let mut store = Store::open(&db).unwrap();
        assert_eq!(store.foreground_context().unwrap(), context + 1);
        assert!(store.load_foreground_summary().unwrap().is_none());
        assert!(!store.save_foreground_summary(context, &summary).unwrap());
        assert!(store.candidates().unwrap().is_empty());
        assert!(store.incomplete("root").unwrap());
        assert_eq!(
            store.take_scope().unwrap(),
            Some(("root".into(), "/root".into()))
        );
        assert_eq!(store.wallet().unwrap().collected_coins, 17);
        assert_eq!(store.wallet().unwrap().fractional_bytes, 42);
    }

    #[test]
    fn interrupted_cleanup_preserves_recovery_evidence_across_restarts() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("db");
        let candidate = item("artifact", "/root/target", "cargo", 100_000_000, true);
        let root = Root {
            id: "root".into(),
            path: "/root".into(),
            kind: "projects".into(),
            identity: candidate.identity.clone(),
        };
        let receipt = Receipt {
            id: "operation".into(),
            path: "/root/target".into(),
            title: "Build artifacts".into(),
            operation: "permanent".into(),
            outcome: "prepared".into(),
            detail: "Captured leaves may remain at /root/.chippytea-operation-recovery.".into(),
            created_at: now(),
            reported_bytes: 100_000_000,
            observed_bytes: 0,
            credited_bytes: 0,
            coins: 0,
            trash_path: None,
            can_restore: false,
            seq: None,
        };
        let store = Store::open(&db).unwrap();
        store
            .prepare_operation(
                &root,
                &candidate,
                &receipt,
                Path::new("/root/.chippytea-operation"),
            )
            .unwrap();
        drop(store);
        let mut store = Store::open(&db).unwrap();
        store.reconcile().unwrap();
        let recovered = store.history().unwrap().remove(0);
        assert_eq!(recovered.outcome, "interrupted");
        assert!(recovered.detail.contains(&receipt.detail));
        assert_eq!(recovered.credited_bytes, 0);
        assert_eq!(recovered.coins, 0);
        drop(store);
        let mut store = Store::open(&db).unwrap();
        store.reconcile().unwrap();
        assert_eq!(store.history().unwrap()[0].detail, recovered.detail);
        assert_eq!(store.wallet().unwrap().pending_coins, 0);
    }

    #[test]
    fn derived_usage_counts_utf8_payload_and_unchanged_batches_do_not_write() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("db")).unwrap();
        discovery_root(&mut store);
        let candidate = item("🪙", "/root/🪙", "cargo", 100_000_000, true);
        save(&mut store, vec![candidate.clone()]);
        let usage = store.derived_usage().unwrap();
        assert_eq!(usage.candidate_rows, 1);
        assert_eq!(
            usage.candidate_payload_bytes,
            candidate_payload_weight(
                serde_json::to_string(&candidate).unwrap().as_bytes(),
                &candidate.path
            )
            .unwrap()
        );
        let before = store.conn.total_changes();
        save(&mut store, vec![candidate]);
        assert_eq!(store.conn.total_changes(), before);

        let mut changed = item("🪙", "/root/🪙", "cargo", 200_000_000, true);
        changed.title = "changed".into();
        let before = store.conn.total_changes();
        save(&mut store, vec![changed.clone()]);
        assert!(store.conn.total_changes() > before);
        let usage = store.derived_usage().unwrap();
        assert_eq!(usage.candidate_rows, 1);
        assert_eq!(
            usage.candidate_payload_bytes,
            candidate_payload_weight(
                serde_json::to_string(&changed).unwrap().as_bytes(),
                &changed.path
            )
            .unwrap()
        );
    }

    #[test]
    fn limited_refresh_retains_old_findings_and_marks_coverage_incomplete() {
        for byte_pressure in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let old = item("old", "/root/old", "cargo", 100_000_000, true);
            let weight = candidate_payload_weight(
                serde_json::to_string(&old).unwrap().as_bytes(),
                &old.path,
            )
            .unwrap();
            let limits = DerivedLimits {
                max_candidate_rows: if byte_pressure { 50_000 } else { 1 },
                max_candidate_payload_bytes: if byte_pressure { weight } else { u64::MAX },
                max_candidate_json_bytes: DEFAULT_MAX_CANDIDATE_JSON_BYTES,
            };
            let mut store = Store::open(&dir.path().join("db"))
                .unwrap()
                .with_derived_limits(limits);
            discovery_root(&mut store);
            save(&mut store, vec![old.clone()]);
            let root = store.root("root").unwrap();
            claim(&mut store, &root.path);
            let refresh = store.begin_scope_refresh(&root, &root.path, None).unwrap();
            let newer = item("new", "/root/new", "cargo", 100_000_000, true);
            store
                .save_batch(&ScanBatch {
                    candidates: vec![newer.clone()],
                    stats: ScanStats::default(),
                })
                .unwrap();
            store
                .finish_scope_refresh(
                    &refresh,
                    &ScanStats {
                        complete: true,
                        ..Default::default()
                    },
                    false,
                )
                .unwrap();
            assert_eq!(store.candidate("old").unwrap(), old);
            assert!(store.incomplete("root").unwrap());
            let mut stats = ScanStats {
                complete: true,
                ..Default::default()
            };
            store.apply_coverage(&mut stats).unwrap();
            assert!(!stats.complete);
            assert!(stats.message.contains("storage-limited"));
            drop(store);
            let mut restarted = Store::open(&dir.path().join("db"))
                .unwrap()
                .with_derived_limits(limits);
            let mut restarted_stats = ScanStats {
                complete: true,
                ..Default::default()
            };
            restarted.apply_coverage(&mut restarted_stats).unwrap();
            assert!(!restarted_stats.complete);
            assert!(restarted_stats.message.contains("storage-limited"));
            // A durable admission-pressure signal permits one bounded Tidy slice
            // even at the cap, without letting the warning drive endless eviction.
            assert_eq!(restarted.maintain_derived(&[]).unwrap().candidate_rows, 0);
            assert!(!restarted.has_pending_scopes().unwrap());
            claim(&mut restarted, &root.path);
            let refresh = restarted
                .begin_scope_refresh(&root, &root.path, None)
                .unwrap();
            save(&mut restarted, vec![newer.clone()]);
            restarted
                .finish_scope_refresh(
                    &refresh,
                    &ScanStats {
                        complete: true,
                        ..Default::default()
                    },
                    false,
                )
                .unwrap();
            assert_eq!(restarted.maintain_derived(&[]).unwrap().candidate_rows, 1);
            drop(restarted);
            let restarted = Store::open(&dir.path().join("db"))
                .unwrap()
                .with_derived_limits(limits);
            assert_eq!(restarted.candidate("new").unwrap(), newer);
            assert!(restarted.candidate("old").is_err());
            assert!(!restarted.incomplete("root").unwrap());
            let mut stats = ScanStats {
                complete: true,
                ..Default::default()
            };
            restarted.apply_coverage(&mut stats).unwrap();
            assert!(stats.complete);
            assert!(!stats.message.contains("storage-limited"));
        }
    }

    #[test]
    fn admission_pressure_preserves_protected_rows_and_is_consumed_once() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("db")).unwrap();
        discovery_root(&mut store);
        save(
            &mut store,
            vec![
                item("one", "/root/one", "cargo", 100_000_000, true),
                item("two", "/root/two", "cargo", 100_000_000, true),
            ],
        );
        store.derived_limits.max_candidate_rows = 2;
        store.keep("/root/two", true).unwrap();
        save(
            &mut store,
            vec![item("new", "/root/new", "cargo", 100_000_000, true)],
        );
        let wallet = store.wallet().unwrap();
        let pending = store.has_pending_scopes().unwrap();
        store
            .conn
            .execute("INSERT INTO active_scopes VALUES('root','/root/one')", [])
            .unwrap();
        assert_eq!(store.maintain_derived(&[]).unwrap().candidate_rows, 2);
        store.conn.execute("DELETE FROM active_scopes", []).unwrap();
        assert_eq!(
            store
                .maintain_derived(&["one".into()])
                .unwrap()
                .candidate_rows,
            2
        );
        assert_eq!(store.maintain_derived(&[]).unwrap().candidate_rows, 1);
        assert!(store.candidate("two").is_ok());
        store.keep("/root/two", false).unwrap();
        // Persisted incomplete/limited coverage cannot by itself evict again.
        assert_eq!(store.maintain_derived(&[]).unwrap().candidate_rows, 1);
        assert_eq!(
            store.wallet().unwrap().credited_bytes,
            wallet.credited_bytes
        );
        assert_eq!(store.has_pending_scopes().unwrap(), pending);
        let mut oversized = item("large", "/root/large", "cargo", 100_000_000, true);
        oversized.title = "x".repeat(DEFAULT_MAX_CANDIDATE_JSON_BYTES);
        save(&mut store, vec![oversized]);
        assert_eq!(store.maintain_derived(&[]).unwrap().candidate_rows, 1);
    }

    #[test]
    fn derived_retention_preserves_durable_truth_and_never_requeues_by_itself() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("db")).unwrap();
        discovery_root(&mut store);
        save(
            &mut store,
            vec![
                item("one", "/root/one", "cargo", 100_000_000, true),
                item("two", "/root/two", "cargo", 100_000_000, true),
            ],
        );
        store.derived_limits.max_candidate_rows = 1;
        store.keep("/root/two", true).unwrap();
        let before = store.wallet().unwrap();
        let pending_before = store.has_pending_scopes().unwrap();
        let usage = store.maintain_derived(&[]).unwrap();
        assert!(usage.candidate_rows >= 1);
        assert!(
            store.candidate("two").is_ok(),
            "Keep-protected rows survive"
        );
        assert_eq!(
            store.wallet().unwrap().credited_bytes,
            before.credited_bytes
        );
        assert_eq!(store.has_pending_scopes().unwrap(), pending_before);
        assert!(store.incomplete("root").unwrap());
    }

    #[test]
    fn root_coverage_overlay_does_not_leak_sibling_incomplete_state() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("db")).unwrap();
        discovery_root(&mut store);
        store
            .add_root(&Root {
                id: "other".into(),
                path: "/other".into(),
                kind: "projects".into(),
                identity: item("other", "/other", "cargo", 1, false).identity,
            })
            .unwrap();
        store
            .conn
            .execute("DELETE FROM incomplete_roots WHERE root_id='other'", [])
            .unwrap();
        let mut stats = ScanStats {
            complete: true,
            ..Default::default()
        };
        store.apply_root_coverage("other", &mut stats).unwrap();
        assert!(stats.complete);
        store.apply_root_coverage("root", &mut stats).unwrap();
        assert!(
            stats.complete,
            "A finished scope can coexist with an incomplete root"
        );
        store
            .conn
            .execute(
                "INSERT INTO derived_limited_roots VALUES('root',1,1024,'disposable limit')",
                [],
            )
            .unwrap();
        store.apply_root_coverage("other", &mut stats).unwrap();
        assert!(stats.complete);
        store.apply_root_coverage("root", &mut stats).unwrap();
        assert!(!stats.complete);
    }

    #[test]
    fn event_loss_requeues_every_root_before_setting_exact_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("db")).unwrap();
        discovery_root(&mut store);
        store
            .conn
            .execute_batch(
                "CREATE TABLE event_cursor(id INTEGER PRIMARY KEY CHECK(id=1),cursor INTEGER NOT NULL);
                 INSERT INTO event_cursor VALUES(1,99);",
            )
            .unwrap();
        store.reconcile_events(7).unwrap();
        let cursor: u64 = store
            .conn
            .query_row("SELECT cursor FROM event_cursor WHERE id=1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(cursor, 7);
        assert!(store.has_pending_scopes().unwrap());
        assert!(store.incomplete("root").unwrap());

        store
            .conn
            .execute("DELETE FROM pending_scopes", [])
            .unwrap();
        store.conn.execute("DELETE FROM event_cursor", []).unwrap();
        assert!(store.reconcile_events(8).is_err());
        assert!(!store.has_pending_scopes().unwrap());
    }
}
