use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const COIN_BYTES: u64 = 100_000_000;
// Rebuild derived findings for category-specific everyday Mac recommendations.
pub const RULE_VERSION: u32 = 10;
pub type Result<T> = std::result::Result<T, String>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Identity {
    pub device: u64,
    pub inode: u64,
    pub mode: u32,
    pub size: u64,
    pub modified_ns: i64,
    pub changed_ns: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Root {
    pub id: String,
    pub path: PathBuf,
    pub kind: String,
    pub identity: Identity,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Candidate {
    pub id: String,
    pub root_id: String,
    pub path: PathBuf,
    pub title: String,
    pub kind: String,
    pub logical_bytes: u64,
    pub allocated_bytes: u64,
    pub file_count: u64,
    pub modified_ns: i64,
    pub explanation: String,
    pub consequence: String,
    pub eligible_permanent: bool,
    pub blocked_reason: Option<String>,
    pub identity: Identity,
    pub fingerprint: String,
    pub evidence: String,
    /// Server-owned recommendation policy. Missing in legacy indexes and Swift
    /// review payloads; it never grants mutation permission on its own.
    #[serde(default)]
    pub suggestion_eligible: bool,
    /// In-progress measurement; never replaces a completed cached finding.
    #[serde(default)]
    pub provisional: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScanStats {
    pub entries: u64,
    pub files: u64,
    pub directories: u64,
    pub logical_bytes: u64,
    pub allocated_bytes: u64,
    pub skipped: u64,
    /// Whole artifact boundaries excluded before their contents were enumerated.
    #[serde(default)]
    pub excluded_artifacts: u64,
    /// Non-candidate names examined without fetching full file metadata.
    #[serde(default)]
    pub metadata_skipped: u64,
    pub errors: u64,
    pub candidates: u64,
    pub elapsed_ms: u64,
    pub first_finding_ms: Option<u64>,
    pub cancelled: bool,
    pub complete: bool,
    pub message: String,
}

/// One finite, explicit Scan request. Background index refreshes have their own
/// worker lifetime and do not change this result after the request finishes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForegroundScan {
    pub active: bool,
    pub stats: ScanStats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanBatch {
    pub candidates: Vec<Candidate>,
    pub stats: ScanStats,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Wallet {
    pub collected_coins: u64,
    pub pending_coins: u64,
    pub fractional_bytes: u64,
    pub credited_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Receipt {
    pub id: String,
    pub path: String,
    pub title: String,
    pub operation: String,
    pub outcome: String,
    pub detail: String,
    pub created_at: i64,
    pub reported_bytes: u64,
    pub observed_bytes: u64,
    pub credited_bytes: u64,
    pub coins: u64,
    pub trash_path: Option<String>,
    pub can_restore: bool,
    /// Ledger position assigned only when a paged history query serves this
    /// receipt. Snapshot receipts and stored receipt JSON omit it entirely.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub roots: Vec<Root>,
    pub candidates: Vec<Candidate>,
    pub history: Vec<Receipt>,
    pub wallet: Wallet,
    pub scanning: bool,
    pub cleaning: bool,
    pub stats: ScanStats,
    #[serde(default)]
    pub foreground_scan: Option<ForegroundScan>,
    pub error: Option<String>,
    pub kept_paths: Vec<String>,
}

/// The small, frequently changing part of a native snapshot. Indexed rows and
/// reward/recovery history are deliberately absent from progress-only replies.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct SnapshotProgress {
    pub scanning: bool,
    pub cleaning: bool,
    pub stats: ScanStats,
    pub foreground_scan: Option<ForegroundScan>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct SnapshotUpdate {
    pub revision: String,
    pub content_revision: String,
    pub changed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<Snapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<SnapshotProgress>,
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

pub fn unique_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "{time:x}-{:x}-{:x}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}
