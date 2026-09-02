//! Explicit, bounded duplicate-file verification.
//!
//! Indexed metadata only selects a small set of candidates. Duplicate evidence
//! is produced here, on demand, after the scanner policy is revalidated and
//! after byte-for-byte comparison of independently owned local files.

use crate::model::{Candidate, Result, Root};
use crate::{safety, scanner};
use std::collections::{BTreeMap, HashSet};
use std::ffi::{CString, OsStr, OsString};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

pub(crate) const MAX_FILES: usize = 500;
pub(crate) const MAX_READ_BYTES: u64 = 64 * 1024 * 1024 * 1024;

const CHUNK_BYTES: usize = 64 * 1024;
// Every opened input pins its file and parent. One reference plus fifteen
// peers therefore keeps the exact-comparison working set at thirty-two FDs.
const PEERS_PER_BATCH: usize = 15;
const MAX_ELAPSED: Duration = Duration::from_secs(60);
const UPDATE_INTERVAL: Duration = Duration::from_millis(100);
const PERSONAL_KINDS: [&str; 4] = ["download", "installer", "archive", "largefile"];

#[derive(Clone)]
pub(crate) struct Input {
    pub root: Root,
    pub candidate: Candidate,
    pub keeper_only: bool,
}

#[derive(Clone)]
pub(crate) struct Group {
    pub items: Vec<Input>,
}

#[derive(Clone, Default, serde::Serialize)]
pub(crate) struct Progress {
    pub phase: String,
    pub files_considered: usize,
    pub files_compared: usize,
    pub skipped_files: usize,
    pub bytes_read: u64,
    pub groups_found: usize,
    pub limited: bool,
    pub cancelled: bool,
    pub complete: bool,
}

pub(crate) struct Analysis {
    pub groups: Vec<Group>,
    pub progress: Progress,
}

#[derive(Clone)]
struct Item {
    ordinal: usize,
    input: Input,
}

#[derive(Debug)]
enum WorkFailure {
    Unsafe(String),
    Cancelled,
    Limited,
}

type WorkResult<T> = std::result::Result<T, WorkFailure>;

impl WorkFailure {
    fn message(self) -> String {
        match self {
            Self::Unsafe(reason) => reason,
            Self::Cancelled => "Duplicate verification was cancelled".into(),
            Self::Limited => "Duplicate verification reached its read or time limit".into(),
        }
    }
}

impl From<String> for WorkFailure {
    fn from(reason: String) -> Self {
        Self::Unsafe(reason)
    }
}

struct WorkState<'a> {
    cancel: &'a AtomicBool,
    started: Instant,
    bytes_read: u64,
    read_limit: u64,
    time_limit: Duration,
}

impl<'a> WorkState<'a> {
    fn new(cancel: &'a AtomicBool) -> Self {
        Self {
            cancel,
            started: Instant::now(),
            bytes_read: 0,
            read_limit: MAX_READ_BYTES,
            time_limit: MAX_ELAPSED,
        }
    }

    #[cfg(test)]
    fn limited(cancel: &'a AtomicBool, read_limit: u64, time_limit: Duration) -> Self {
        Self {
            cancel,
            started: Instant::now(),
            bytes_read: 0,
            read_limit,
            time_limit,
        }
    }

    fn checkpoint(&self) -> WorkResult<()> {
        if self.cancel.load(std::sync::atomic::Ordering::Relaxed) {
            Err(WorkFailure::Cancelled)
        } else if self.started.elapsed() >= self.time_limit {
            Err(WorkFailure::Limited)
        } else {
            Ok(())
        }
    }

    /// Reserve before reading. This deliberately charges a failed short read
    /// in full, so an inaccessible or changing file can never bypass the cap.
    fn reserve(&mut self, bytes: u64) -> WorkResult<u64> {
        self.checkpoint()?;
        let total = self
            .bytes_read
            .checked_add(bytes)
            .filter(|total| *total <= self.read_limit)
            .ok_or(WorkFailure::Limited)?;
        self.bytes_read = total;
        Ok(total)
    }
}

struct Publisher<'a, F: FnMut(&Progress)> {
    callback: &'a mut F,
    last: Instant,
}

impl<'a, F: FnMut(&Progress)> Publisher<'a, F> {
    fn new(callback: &'a mut F) -> Self {
        Self {
            callback,
            last: Instant::now(),
        }
    }

    fn emit(&mut self, progress: &Progress, force: bool) {
        if force || self.last.elapsed() >= UPDATE_INTERVAL {
            (self.callback)(progress);
            self.last = Instant::now();
        }
    }
}

fn valid_input(input: &Input) -> Result<()> {
    let candidate = &input.candidate;
    let root = &input.root;
    if candidate.root_id != root.id
        || candidate.path == root.path
        || !candidate.path.starts_with(&root.path)
        || !candidate.path.is_absolute()
    {
        return Err("A duplicate input is outside its authorized root".into());
    }
    if !PERSONAL_KINDS.contains(&candidate.kind.as_str()) {
        return Err("Duplicate verification is restricted to indexed personal files".into());
    }
    if candidate.provisional
        || !candidate.suggestion_eligible
        || candidate.blocked_reason.is_some()
        || candidate.eligible_permanent
        || candidate.fingerprint.is_empty()
        || candidate.evidence.is_empty()
    {
        return Err("A duplicate input is not a completed eligible snapshot".into());
    }
    if candidate.file_count != 1
        || candidate.logical_bytes != candidate.identity.size
        || candidate.identity.device != root.identity.device
        || candidate.identity.mode & libc::S_IFMT as u32 != libc::S_IFREG as u32
    {
        return Err("A duplicate input is not one regular file on its authorized volume".into());
    }
    if candidate.path.parent().is_none() || candidate.path.file_name().is_none() {
        return Err("A duplicate input has no file parent or name".into());
    }
    Ok(())
}

fn validate_inputs(inputs: &[Input]) -> Result<()> {
    if inputs.len() > MAX_FILES {
        return Err(format!(
            "Duplicate verification is limited to {MAX_FILES} files per action"
        ));
    }
    let mut identities = HashSet::with_capacity(inputs.len());
    let mut paths = HashSet::with_capacity(inputs.len());
    for input in inputs {
        valid_input(input)?;
        let identity = (
            input.candidate.identity.device,
            input.candidate.identity.inode,
        );
        if !identities.insert(identity) {
            return Err(
                "Hard-linked or repeated file identities cannot be duplicate inputs".into(),
            );
        }
        if !paths.insert(input.candidate.path.clone()) {
            return Err("The same path cannot appear twice in duplicate inputs".into());
        }
    }
    Ok(())
}

pub(crate) fn analyze(
    inputs: Vec<Input>,
    cancel: &AtomicBool,
    mut callback: impl FnMut(&Progress),
) -> Result<Analysis> {
    validate_inputs(&inputs)?;
    let _local_io = safety::LocalOnlyIo::new()?;
    let mut state = WorkState::new(cancel);
    let mut progress = Progress {
        phase: "validating".into(),
        files_considered: inputs.len(),
        ..Progress::default()
    };
    let mut publisher = Publisher::new(&mut callback);
    publisher.emit(&progress, true);
    let mut skipped = vec![false; inputs.len()];
    let mut verified = Vec::with_capacity(inputs.len());

    // Scanner revalidation fingerprints metadata, not regular-file payloads.
    // The content-read budget begins with the fixed samples below.
    for (ordinal, input) in inputs.into_iter().enumerate() {
        if let Err(stop) = state.checkpoint() {
            return Ok(finish_stopped(
                Vec::new(),
                stop,
                &mut progress,
                &state,
                &mut publisher,
            ));
        }
        let result = scanner::revalidate(&input.root, &input.candidate, cancel);
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Ok(finish_stopped(
                Vec::new(),
                WorkFailure::Cancelled,
                &mut progress,
                &state,
                &mut publisher,
            ));
        }
        if state.started.elapsed() >= state.time_limit {
            return Ok(finish_stopped(
                Vec::new(),
                WorkFailure::Limited,
                &mut progress,
                &state,
                &mut publisher,
            ));
        }
        match result {
            Ok(()) => verified.push(Item { ordinal, input }),
            Err(_) => mark_skipped(ordinal, &mut skipped, &mut progress),
        }
    }

    // Store selection is already priority-ordered. Keep each size bucket at
    // its first input position instead of reordering opportunities by key.
    let mut buckets: Vec<((u64, u64), Vec<Item>)> = Vec::new();
    for item in verified {
        let key = (
            item.input.candidate.identity.device,
            item.input.candidate.logical_bytes,
        );
        if let Some((_, bucket)) = buckets.iter_mut().find(|(existing, _)| *existing == key) {
            bucket.push(item);
        } else {
            buckets.push((key, vec![item]));
        }
    }

    let mut groups = Vec::new();
    for (_, bucket) in buckets {
        if bucket.len() < 2 || bucket.iter().all(|item| item.input.keeper_only) {
            continue;
        }
        set_phase("sampling", &mut progress, &mut publisher);
        let mut samples: BTreeMap<[u8; 32], Vec<Item>> = BTreeMap::new();
        for item in bucket {
            let mut tick = |bytes| {
                progress.bytes_read = bytes;
                publisher.emit(&progress, false);
            };
            match sample_digest(&item.input, &mut state, &mut tick) {
                Ok(digest) => samples.entry(digest).or_default().push(item),
                Err(WorkFailure::Unsafe(_)) => {
                    mark_skipped(item.ordinal, &mut skipped, &mut progress)
                }
                Err(stop) => {
                    return Ok(finish_stopped(
                        groups,
                        stop,
                        &mut progress,
                        &state,
                        &mut publisher,
                    ));
                }
            }
        }

        for sample_group in stable_groups(samples) {
            if sample_group.len() < 2 || sample_group.iter().all(|item| item.input.keeper_only) {
                continue;
            }
            set_phase("hashing", &mut progress, &mut publisher);
            let mut hashes: BTreeMap<[u8; 32], Vec<Item>> = BTreeMap::new();
            for item in sample_group {
                let mut tick = |bytes| {
                    progress.bytes_read = bytes;
                    publisher.emit(&progress, false);
                };
                match full_digest(&item.input, &mut state, &mut tick) {
                    Ok(digest) => {
                        progress.files_compared = progress.files_compared.saturating_add(1);
                        hashes.entry(digest).or_default().push(item);
                    }
                    Err(WorkFailure::Unsafe(_)) => {
                        mark_skipped(item.ordinal, &mut skipped, &mut progress)
                    }
                    Err(stop) => {
                        return Ok(finish_stopped(
                            groups,
                            stop,
                            &mut progress,
                            &state,
                            &mut publisher,
                        ));
                    }
                }
            }

            for hash_group in stable_groups(hashes) {
                if hash_group.len() < 2 || hash_group.iter().all(|item| item.input.keeper_only) {
                    continue;
                }
                set_phase("comparing", &mut progress, &mut publisher);
                let mut tick = |bytes| {
                    progress.bytes_read = bytes;
                    publisher.emit(&progress, false);
                };
                let classes = match exact_classes(&hash_group, &mut state, &mut tick) {
                    Ok(classes) => classes,
                    Err(WorkFailure::Unsafe(_)) => {
                        for item in hash_group {
                            mark_skipped(item.ordinal, &mut skipped, &mut progress);
                        }
                        continue;
                    }
                    Err(stop) => {
                        return Ok(finish_stopped(
                            groups,
                            stop,
                            &mut progress,
                            &state,
                            &mut publisher,
                        ));
                    }
                };

                // Reopen every member after exact partitioning. One unsafe
                // member invalidates the whole hash group, so a changed file
                // can never leave a partially actionable collision class.
                set_phase("finalizing", &mut progress, &mut publisher);
                match finalize_hash_group(&hash_group, &state) {
                    Ok(()) => {}
                    Err(WorkFailure::Unsafe(_)) => {
                        for item in hash_group {
                            mark_skipped(item.ordinal, &mut skipped, &mut progress);
                        }
                        continue;
                    }
                    Err(stop) => {
                        return Ok(finish_stopped(
                            groups,
                            stop,
                            &mut progress,
                            &state,
                            &mut publisher,
                        ));
                    }
                }

                for class in classes {
                    if class.len() < 2 || class.iter().all(|item| item.input.keeper_only) {
                        continue;
                    }
                    groups.push(Group {
                        items: class.into_iter().map(|item| item.input).collect(),
                    });
                    progress.groups_found = groups.len();
                    publisher.emit(&progress, true);
                }
            }
        }
    }

    progress.phase = "complete".into();
    progress.bytes_read = state.bytes_read;
    progress.complete = true;
    publisher.emit(&progress, true);
    Ok(Analysis { groups, progress })
}

fn finish_stopped<F: FnMut(&Progress)>(
    groups: Vec<Group>,
    stop: WorkFailure,
    progress: &mut Progress,
    state: &WorkState<'_>,
    publisher: &mut Publisher<'_, F>,
) -> Analysis {
    progress.bytes_read = state.bytes_read;
    progress.complete = false;
    match stop {
        WorkFailure::Cancelled => {
            progress.phase = "cancelled".into();
            progress.cancelled = true;
        }
        WorkFailure::Limited | WorkFailure::Unsafe(_) => {
            progress.phase = "limited".into();
            progress.limited = true;
        }
    }
    progress.groups_found = groups.len();
    publisher.emit(progress, true);
    Analysis {
        groups,
        progress: progress.clone(),
    }
}

fn set_phase<F: FnMut(&Progress)>(
    phase: &str,
    progress: &mut Progress,
    publisher: &mut Publisher<'_, F>,
) {
    progress.phase = phase.into();
    publisher.emit(progress, true);
}

fn mark_skipped(ordinal: usize, skipped: &mut [bool], progress: &mut Progress) {
    if !skipped[ordinal] {
        skipped[ordinal] = true;
        progress.skipped_files = progress.skipped_files.saturating_add(1);
    }
}

fn stable_groups(map: BTreeMap<[u8; 32], Vec<Item>>) -> Vec<Vec<Item>> {
    let mut groups: Vec<_> = map.into_values().collect();
    groups.sort_by_key(|group| group.first().map_or(usize::MAX, |item| item.ordinal));
    groups
}

fn sample_digest(
    input: &Input,
    state: &mut WorkState<'_>,
    tick: &mut dyn FnMut(u64),
) -> WorkResult<[u8; 32]> {
    let mut opened = open_input(input, state)?;
    let size = opened.meta.identity.size;
    let length = size.min(CHUNK_BYTES as u64);
    let offsets = [
        0,
        size.saturating_sub(length) / 2,
        size.saturating_sub(length),
    ];
    let mut buffer = [0u8; CHUNK_BYTES];
    let mut digest = blake3::Hasher::new();
    digest.update(b"chippytea-duplicate-sample-v1\0");
    for offset in offsets {
        state.checkpoint()?;
        opened.file.seek(SeekFrom::Start(offset)).map_err(|error| {
            WorkFailure::Unsafe(format!("Cannot seek duplicate input: {error}"))
        })?;
        state.checkpoint()?;
        let bytes = &mut buffer[..length as usize];
        read_filled(&mut opened.file, bytes, state, tick)?;
        state.checkpoint()?;
        digest.update(&offset.to_le_bytes());
        digest.update(&(bytes.len() as u64).to_le_bytes());
        digest.update(bytes);
    }
    opened.validate_as_input(input, state.cancel)?;
    Ok(*digest.finalize().as_bytes())
}

fn full_digest(
    input: &Input,
    state: &mut WorkState<'_>,
    tick: &mut dyn FnMut(u64),
) -> WorkResult<[u8; 32]> {
    let mut opened = open_input(input, state)?;
    opened
        .file
        .seek(SeekFrom::Start(0))
        .map_err(|error| WorkFailure::Unsafe(format!("Cannot seek duplicate input: {error}")))?;
    let mut remaining = opened.meta.identity.size;
    let mut buffer = [0u8; CHUNK_BYTES];
    let mut digest = blake3::Hasher::new();
    while remaining > 0 {
        state.checkpoint()?;
        let length = remaining.min(CHUNK_BYTES as u64) as usize;
        read_filled(&mut opened.file, &mut buffer[..length], state, tick)?;
        state.checkpoint()?;
        digest.update(&buffer[..length]);
        remaining -= length as u64;
    }
    opened.validate_as_input(input, state.cancel)?;
    Ok(*digest.finalize().as_bytes())
}

struct BatchPeer {
    item: Item,
    opened: OpenedFile,
    equal: bool,
}

fn exact_classes(
    hash_group: &[Item],
    state: &mut WorkState<'_>,
    tick: &mut dyn FnMut(u64),
) -> WorkResult<Vec<Vec<Item>>> {
    exact_classes_with_hook(hash_group, state, tick, &mut || {})
}

fn exact_classes_with_hook(
    hash_group: &[Item],
    state: &mut WorkState<'_>,
    tick: &mut dyn FnMut(u64),
    after_batch: &mut dyn FnMut(),
) -> WorkResult<Vec<Vec<Item>>> {
    let mut pending = hash_group.to_vec();
    let mut classes = Vec::new();
    let mut reference_bytes = [0u8; CHUNK_BYTES];
    let mut peer_bytes = [0u8; CHUNK_BYTES];

    while !pending.is_empty() {
        state.checkpoint()?;
        let reference_item = pending.remove(0);
        if pending.is_empty() {
            classes.push(vec![reference_item]);
            break;
        }
        let mut reference = open_input(&reference_item.input, state)?;
        let peers = std::mem::take(&mut pending);
        let mut class = vec![reference_item.clone()];
        let mut mismatches = Vec::new();

        for batch in peers.chunks(PEERS_PER_BATCH) {
            let results = exact_batch(
                &reference_item,
                &mut reference,
                batch,
                &mut reference_bytes,
                &mut peer_bytes,
                state,
                tick,
            )?;
            for (item, equal) in results {
                if equal {
                    class.push(item);
                } else {
                    mismatches.push(item);
                }
            }
            after_batch();
            state.checkpoint()?;
        }
        reference.validate_as_input(&reference_item.input, state.cancel)?;
        state.checkpoint()?;
        classes.push(class);
        pending = mismatches;
    }
    state.checkpoint()?;
    Ok(classes)
}

fn exact_batch(
    reference_item: &Item,
    reference: &mut OpenedFile,
    items: &[Item],
    reference_bytes: &mut [u8; CHUNK_BYTES],
    peer_bytes: &mut [u8; CHUNK_BYTES],
    state: &mut WorkState<'_>,
    tick: &mut dyn FnMut(u64),
) -> WorkResult<Vec<(Item, bool)>> {
    state.checkpoint()?;
    reference.validate_as_input(&reference_item.input, state.cancel)?;
    reference
        .file
        .seek(SeekFrom::Start(0))
        .map_err(|error| WorkFailure::Unsafe(format!("Cannot seek duplicate input: {error}")))?;
    state.checkpoint()?;

    let mut peers = Vec::with_capacity(items.len());
    for item in items {
        if reference_item.input.candidate.identity.device == item.input.candidate.identity.device
            && reference_item.input.candidate.identity.inode == item.input.candidate.identity.inode
        {
            return Err(WorkFailure::Unsafe(
                "A hard-linked identity reached exact duplicate comparison".into(),
            ));
        }
        let mut opened = open_input(&item.input, state)?;
        if opened.meta.identity.size != reference.meta.identity.size {
            return Err(WorkFailure::Unsafe(
                "An exact-comparison hash group changed size".into(),
            ));
        }
        opened.file.seek(SeekFrom::Start(0)).map_err(|error| {
            WorkFailure::Unsafe(format!("Cannot seek duplicate input: {error}"))
        })?;
        state.checkpoint()?;
        peers.push(BatchPeer {
            item: item.clone(),
            opened,
            equal: true,
        });
    }

    let mut remaining = reference.meta.identity.size;
    while remaining > 0 && peers.iter().any(|peer| peer.equal) {
        state.checkpoint()?;
        let length = remaining.min(CHUNK_BYTES as u64) as usize;
        read_filled(
            &mut reference.file,
            &mut reference_bytes[..length],
            state,
            tick,
        )?;
        for peer in peers.iter_mut().filter(|peer| peer.equal) {
            read_filled(
                &mut peer.opened.file,
                &mut peer_bytes[..length],
                state,
                tick,
            )?;
            state.checkpoint()?;
            if reference_bytes[..length] != peer_bytes[..length] {
                peer.equal = false;
            }
        }
        remaining -= length as u64;
    }
    reference.validate_as_input(&reference_item.input, state.cancel)?;
    for peer in &peers {
        peer.opened
            .validate_as_input(&peer.item.input, state.cancel)?;
    }
    Ok(peers
        .into_iter()
        .map(|peer| (peer.item, peer.equal))
        .collect())
}

fn finalize_hash_group(hash_group: &[Item], state: &WorkState<'_>) -> WorkResult<()> {
    for item in hash_group {
        state.checkpoint()?;
        let opened = open_input(&item.input, state)?;
        opened.validate_as_input(&item.input, state.cancel)?;
    }
    state.checkpoint()
}

fn read_filled(
    file: &mut File,
    mut bytes: &mut [u8],
    state: &mut WorkState<'_>,
    tick: &mut dyn FnMut(u64),
) -> WorkResult<()> {
    let total = state.reserve(bytes.len() as u64)?;
    while !bytes.is_empty() {
        state.checkpoint()?;
        match file.read(bytes) {
            Ok(0) => {
                return Err(WorkFailure::Unsafe(
                    "A duplicate input became shorter while reading".into(),
                ));
            }
            Ok(count) => {
                let (_, rest) = bytes.split_at_mut(count);
                bytes = rest;
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => {
                return Err(WorkFailure::Unsafe(format!(
                    "Cannot read duplicate input: {error}"
                )));
            }
        }
    }
    tick(total);
    Ok(())
}

struct OpenedFile {
    file: File,
    parent: OwnedFd,
    parent_path: PathBuf,
    parent_meta: safety::EntryMeta,
    path: PathBuf,
    name: OsString,
    meta: safety::EntryMeta,
}

impl OpenedFile {
    fn validate_current_path(&self, cancel: &AtomicBool) -> WorkResult<()> {
        safety::cancelled(cancel).map_err(|_| WorkFailure::Cancelled)?;
        if safety::stat_fd(self.file.as_raw_fd())? != self.meta
            || safety::stat_fd(self.parent.as_raw_fd())? != self.parent_meta
            || safety::metadata(&self.parent_path)? != self.parent_meta
            || stat_child(self.parent.as_raw_fd(), &self.name)? != Some(self.meta.clone())
        {
            return Err(WorkFailure::Unsafe(
                "A duplicate file or its parent changed during verification".into(),
            ));
        }
        Ok(())
    }

    fn validate_as_input(&self, input: &Input, cancel: &AtomicBool) -> WorkResult<()> {
        safety::cancelled(cancel).map_err(|_| WorkFailure::Cancelled)?;
        safety::validate_root(&input.root)?;
        safety::check_scope_policy(&input.root, &input.candidate.path)?;
        safety::validate_ancestors(&self.parent_path)?;
        self.validate_current_path(cancel)?;
        if self.path != input.candidate.path || self.meta.identity != input.candidate.identity {
            return Err(WorkFailure::Unsafe(
                "A duplicate input no longer matches its indexed identity".into(),
            ));
        }
        Ok(())
    }
}

fn open_input(input: &Input, state: &WorkState<'_>) -> WorkResult<OpenedFile> {
    state.checkpoint()?;
    valid_input(input)?;
    safety::validate_root(&input.root)?;
    safety::check_scope_policy(&input.root, &input.candidate.path)?;
    let parent_path = input
        .candidate
        .path
        .parent()
        .ok_or_else(|| WorkFailure::Unsafe("A duplicate input has no parent".into()))?;
    safety::validate_ancestors(parent_path)?;
    let opened = open_named_file(parent_path, &input.candidate.path)?;
    if opened.meta.identity != input.candidate.identity
        || opened.meta.identity.device != input.root.identity.device
        || !safe_regular(&opened.meta)
    {
        return Err(WorkFailure::Unsafe(
            "A duplicate input changed or is not an independent local file".into(),
        ));
    }
    state.checkpoint()?;
    Ok(opened)
}

fn safe_regular(meta: &safety::EntryMeta) -> bool {
    meta.is_file()
        && !meta.is_dataless()
        && meta.links == 1
        && meta.uid == unsafe { libc::geteuid() }
}

fn open_named_file(parent_path: &Path, path: &Path) -> WorkResult<OpenedFile> {
    let name = path
        .file_name()
        .ok_or_else(|| WorkFailure::Unsafe("A duplicate file has no name".into()))?
        .to_os_string();
    let parent = safety::open_directory(parent_path)?;
    safety::check_local(parent.as_raw_fd())?;
    let parent_meta = safety::stat_fd(parent.as_raw_fd())?;
    let meta = stat_child(parent.as_raw_fd(), &name)?.ok_or_else(|| {
        WorkFailure::Unsafe("A duplicate input disappeared before opening".into())
    })?;
    if !safe_regular(&meta) {
        return Err(WorkFailure::Unsafe(
            "Duplicate inputs must be owned, independent, physical regular files".into(),
        ));
    }
    let encoded = c_name(&name)?;
    let raw = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            encoded.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if raw < 0 {
        return Err(WorkFailure::Unsafe(format!(
            "Cannot open duplicate input: {}",
            std::io::Error::last_os_error()
        )));
    }
    let file = unsafe { File::from_raw_fd(raw) };
    if safety::stat_fd(file.as_raw_fd())? != meta
        || safety::stat_fd(parent.as_raw_fd())? != parent_meta
    {
        return Err(WorkFailure::Unsafe(
            "A duplicate input changed while opening".into(),
        ));
    }
    Ok(OpenedFile {
        file,
        parent,
        parent_path: parent_path.to_path_buf(),
        parent_meta,
        path: path.to_path_buf(),
        name,
        meta,
    })
}

fn c_name(name: &OsStr) -> WorkResult<CString> {
    CString::new(name.as_bytes())
        .map_err(|_| WorkFailure::Unsafe("A duplicate path contains a NUL byte".into()))
}

fn stat_child(parent: i32, name: &OsStr) -> WorkResult<Option<safety::EntryMeta>> {
    let name = c_name(name)?;
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe {
        libc::fstatat(
            parent,
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        let error = std::io::Error::last_os_error();
        return if error.raw_os_error() == Some(libc::ENOENT) {
            Ok(None)
        } else {
            Err(WorkFailure::Unsafe(format!(
                "Cannot inspect duplicate input: {error}"
            )))
        };
    }
    let stat = unsafe { stat.assume_init() };
    Ok(Some(safety::EntryMeta::from_stat(&stat)))
}

pub(crate) fn verify_staged(
    copy_root: &Root,
    copy: &Candidate,
    stage: &Path,
    keeper: &Input,
    cancel: &AtomicBool,
    mut callback: impl FnMut(u64),
) -> Result<RetainedFile> {
    let copy_input = Input {
        root: copy_root.clone(),
        candidate: copy.clone(),
        keeper_only: false,
    };
    valid_input(&copy_input)?;
    valid_input(keeper)?;
    if (copy.identity.device, copy.identity.inode)
        == (
            keeper.candidate.identity.device,
            keeper.candidate.identity.inode,
        )
    {
        return Err("The retained file must have an independent identity".into());
    }
    let _local_io = safety::LocalOnlyIo::new()?;
    let mut state = WorkState::new(cancel);
    callback(0);

    state.checkpoint().map_err(WorkFailure::message)?;
    scanner::revalidate(&keeper.root, &keeper.candidate, cancel)?;
    state.checkpoint().map_err(WorkFailure::message)?;

    let mut staged = open_stage(copy_root, copy, stage, &state).map_err(WorkFailure::message)?;
    let mut retained = open_input(keeper, &state).map_err(WorkFailure::message)?;
    if staged.meta.identity.size != retained.meta.identity.size {
        return Err("The staged copy and retained file no longer have the same size".into());
    }
    staged.file.seek(SeekFrom::Start(0)).map_err(|error| {
        format!("Cannot seek the staged duplicate during final verification: {error}")
    })?;
    retained.file.seek(SeekFrom::Start(0)).map_err(|error| {
        format!("Cannot seek the retained duplicate during final verification: {error}")
    })?;
    let mut remaining = staged.meta.identity.size;
    let mut stage_bytes = [0u8; CHUNK_BYTES];
    let mut keeper_bytes = [0u8; CHUNK_BYTES];
    let mut last_update = Instant::now();
    while remaining > 0 {
        state.checkpoint().map_err(WorkFailure::message)?;
        let length = remaining.min(CHUNK_BYTES as u64) as usize;
        read_filled(
            &mut staged.file,
            &mut stage_bytes[..length],
            &mut state,
            &mut |_| {},
        )
        .map_err(WorkFailure::message)?;
        read_filled(
            &mut retained.file,
            &mut keeper_bytes[..length],
            &mut state,
            &mut |_| {},
        )
        .map_err(WorkFailure::message)?;
        if stage_bytes[..length] != keeper_bytes[..length] {
            return Err("The staged copy is not exactly equal to the retained file".into());
        }
        remaining -= length as u64;
        if last_update.elapsed() >= UPDATE_INTERVAL {
            callback(state.bytes_read);
            last_update = Instant::now();
        }
    }
    validate_stage(copy_root, copy, stage, &staged, cancel).map_err(WorkFailure::message)?;
    retained
        .validate_as_input(keeper, cancel)
        .map_err(WorkFailure::message)?;
    state.checkpoint().map_err(WorkFailure::message)?;
    callback(state.bytes_read);
    Ok(RetainedFile {
        input: keeper.clone(),
        opened: retained,
    })
}

pub(crate) struct RetainedFile {
    input: Input,
    opened: OpenedFile,
}

impl RetainedFile {
    pub(crate) fn validate(&self, cancel: &AtomicBool) -> Result<()> {
        let _local_io = safety::LocalOnlyIo::new()?;
        self.opened
            .validate_as_input(&self.input, cancel)
            .map_err(WorkFailure::message)
    }
}

fn stage_name(path: &Path) -> Result<()> {
    let bytes = path
        .file_name()
        .ok_or("The staged duplicate has no name")?
        .as_bytes();
    let Some(suffix) = bytes.strip_prefix(b".chippytea-") else {
        return Err("The staged duplicate does not have an application-generated name".into());
    };
    if suffix.is_empty()
        || suffix
            .iter()
            .any(|byte| !byte.is_ascii_hexdigit() && *byte != b'-')
    {
        return Err("The staged duplicate has an invalid generated name".into());
    }
    Ok(())
}

fn open_stage(
    copy_root: &Root,
    copy: &Candidate,
    stage: &Path,
    state: &WorkState<'_>,
) -> WorkResult<OpenedFile> {
    state.checkpoint()?;
    stage_name(stage)?;
    safety::validate_root(copy_root)?;
    safety::check_scope_policy(copy_root, &copy.path)?;
    let parent = copy
        .path
        .parent()
        .ok_or_else(|| WorkFailure::Unsafe("The original copy has no parent".into()))?;
    if stage.parent() != Some(parent) || stage == copy.path {
        return Err(WorkFailure::Unsafe(
            "The staged duplicate is not beside its reviewed original path".into(),
        ));
    }
    safety::validate_ancestors(parent)?;
    let opened = open_named_file(parent, stage)?;
    if !same_after_rename(&copy.identity, &opened.meta)
        || opened.meta.identity.device != copy_root.identity.device
        || !safe_regular(&opened.meta)
        || stat_child(opened.parent.as_raw_fd(), copy.path.file_name().unwrap())?.is_some()
    {
        return Err(WorkFailure::Unsafe(
            "The staged duplicate does not exactly represent the reviewed copy".into(),
        ));
    }
    state.checkpoint()?;
    Ok(opened)
}

fn validate_stage(
    copy_root: &Root,
    copy: &Candidate,
    stage: &Path,
    opened: &OpenedFile,
    cancel: &AtomicBool,
) -> WorkResult<()> {
    safety::cancelled(cancel).map_err(|_| WorkFailure::Cancelled)?;
    stage_name(stage)?;
    safety::validate_root(copy_root)?;
    safety::check_scope_policy(copy_root, &copy.path)?;
    safety::validate_ancestors(&opened.parent_path)?;
    opened.validate_current_path(cancel)?;
    if stage != opened.path
        || !same_after_rename(&copy.identity, &opened.meta)
        || stat_child(opened.parent.as_raw_fd(), copy.path.file_name().unwrap())?.is_some()
    {
        return Err(WorkFailure::Unsafe(
            "The staged duplicate or original pathname changed during comparison".into(),
        ));
    }
    Ok(())
}

fn same_after_rename(expected: &crate::model::Identity, current: &safety::EntryMeta) -> bool {
    expected.device == current.identity.device
        && expected.inode == current.identity.inode
        && expected.mode == current.identity.mode
        && expected.size == current.identity.size
        && expected.modified_ns == current.identity.modified_ns
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct Fixture {
        _temp: tempfile::TempDir,
        root: Root,
    }

    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            // macOS spells temporary directories through /var, a symlink to
            // /private/var. Production correctly rejects redirected ancestry;
            // tests authorize the physical spelling of their private fixture.
            let path = temp.path().canonicalize().unwrap().join("scope");
            std::fs::create_dir(&path).unwrap();
            let root = safety::authorize(&path, "downloads").unwrap();
            Self { _temp: temp, root }
        }

        fn input(&self, name: &str, bytes: &[u8]) -> Input {
            let path = self.root.path.join(name);
            std::fs::write(&path, bytes).unwrap();
            self.input_for(path)
        }

        fn input_for(&self, path: PathBuf) -> Input {
            let meta = safety::metadata(&path).unwrap();
            Input {
                root: self.root.clone(),
                candidate: Candidate {
                    id: path.to_string_lossy().into(),
                    root_id: self.root.id.clone(),
                    path: path.clone(),
                    title: path.file_name().unwrap().to_string_lossy().into(),
                    kind: "download".into(),
                    logical_bytes: meta.identity.size,
                    allocated_bytes: meta.allocated,
                    file_count: 1,
                    modified_ns: meta.identity.modified_ns,
                    explanation: String::new(),
                    consequence: String::new(),
                    eligible_permanent: false,
                    blocked_reason: None,
                    identity: meta.identity,
                    fingerprint: "test-fingerprint".into(),
                    evidence: "test-evidence".into(),
                    suggestion_eligible: true,
                    provisional: false,
                },
                keeper_only: false,
            }
        }
    }

    fn items(inputs: Vec<Input>) -> Vec<Item> {
        inputs
            .into_iter()
            .enumerate()
            .map(|(ordinal, input)| Item { ordinal, input })
            .collect()
    }

    fn class_names(classes: &[Vec<Item>]) -> Vec<Vec<String>> {
        classes
            .iter()
            .map(|class| {
                class
                    .iter()
                    .map(|item| item.input.candidate.title.clone())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn samples_filter_but_full_hash_and_exact_bytes_decide() {
        let fixture = Fixture::new();
        let mut original = vec![7u8; CHUNK_BYTES * 8];
        let equal = fixture.input("equal-a.bin", &original);
        let equal_copy = fixture.input("equal-b.bin", &original);
        original[CHUNK_BYTES * 2] ^= 1;
        let mismatch = fixture.input("mismatch.bin", &original);
        let cancel = AtomicBool::new(false);
        let mut state = WorkState::new(&cancel);
        let sample_a = sample_digest(&equal, &mut state, &mut |_| {}).unwrap();
        let sample_mismatch = sample_digest(&mismatch, &mut state, &mut |_| {}).unwrap();
        assert_eq!(sample_a, sample_mismatch, "the changed byte is unsampled");
        assert_ne!(
            full_digest(&equal, &mut state, &mut |_| {}).unwrap(),
            full_digest(&mismatch, &mut state, &mut |_| {}).unwrap()
        );
        let classes = exact_classes(
            &items(vec![equal, equal_copy, mismatch]),
            &mut state,
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(
            class_names(&classes),
            vec![
                vec!["equal-a.bin".to_string(), "equal-b.bin".to_string()],
                vec!["mismatch.bin".to_string()],
            ]
        );
    }

    #[test]
    fn exact_comparison_reads_one_reference_per_peer_batch() {
        let fixture = Fixture::new();
        let bytes = vec![0x5au8; CHUNK_BYTES + 19];
        let group = items(
            (0..32)
                .map(|index| fixture.input(&format!("equal-{index:02}.bin"), &bytes))
                .collect(),
        );
        let cancel = AtomicBool::new(false);
        let mut state = WorkState::new(&cancel);
        let classes = exact_classes(&group, &mut state, &mut |_| {}).unwrap();
        let batches = (group.len() - 1).div_ceil(PEERS_PER_BATCH);
        let expected_reads = (group.len() - 1 + batches) as u64 * bytes.len() as u64;

        assert_eq!(classes.len(), 1);
        assert_eq!(classes[0].len(), group.len());
        assert_eq!(state.bytes_read, expected_reads);
        assert!(state.bytes_read < (2 * (group.len() - 1)) as u64 * bytes.len() as u64);
    }

    #[test]
    fn exact_partition_keeps_mismatches_for_later_collision_anchors() {
        let fixture = Fixture::new();
        let size = CHUNK_BYTES + 7;
        let a = vec![0x11; size];
        let mut b = a.clone();
        b[0] = 0x22;
        let mut c = a.clone();
        c[size - 1] = 0x33;
        let mut inputs = Vec::new();
        // Interleave collision classes across the fifteen-peer boundary.
        for index in 0..7 {
            inputs.push(fixture.input(&format!("a-{index}.bin"), &a));
            inputs.push(fixture.input(&format!("b-{index}.bin"), &b));
            inputs.push(fixture.input(&format!("c-{index}.bin"), &c));
        }
        let group = items(inputs);
        let cancel = AtomicBool::new(false);
        let classes = exact_classes(&group, &mut WorkState::new(&cancel), &mut |_| {}).unwrap();

        assert_eq!(
            class_names(&classes),
            ['a', 'b', 'c']
                .into_iter()
                .map(|prefix| (0..7)
                    .map(|index| format!("{prefix}-{index}.bin"))
                    .collect())
                .collect::<Vec<Vec<String>>>()
        );
    }

    #[test]
    fn changed_reference_peer_or_final_member_invalidates_the_hash_group() {
        for changed in [0, 1, PEERS_PER_BATCH + 1] {
            let fixture = Fixture::new();
            let bytes = vec![0x44; CHUNK_BYTES + 1];
            let group = items(
                (0..=PEERS_PER_BATCH + 1)
                    .map(|index| fixture.input(&format!("race-{index:02}.bin"), &bytes))
                    .collect(),
            );
            let path = group[changed].input.candidate.path.clone();
            let cancel = AtomicBool::new(false);
            let mut state = WorkState::new(&cancel);
            let mut batches = 0;
            let result = exact_classes_with_hook(&group, &mut state, &mut |_| {}, &mut || {
                batches += 1;
                if batches == 1 {
                    let mut file = std::fs::OpenOptions::new()
                        .append(true)
                        .open(&path)
                        .unwrap();
                    file.write_all(&[0x99]).unwrap();
                    file.sync_all().unwrap();
                }
            });
            if changed == 1 {
                // This peer was already read and closed in the first batch.
                // Finalization must still reject the entire hash group.
                assert_eq!(result.unwrap().len(), 1);
                assert!(matches!(
                    finalize_hash_group(&group, &state),
                    Err(WorkFailure::Unsafe(_))
                ));
            } else {
                assert!(matches!(result, Err(WorkFailure::Unsafe(_))));
            }
        }

        let fixture = Fixture::new();
        let bytes = vec![0x55; 4096];
        let group = items(vec![
            fixture.input("final-a.bin", &bytes),
            fixture.input("final-b.bin", &bytes),
        ]);
        let cancel = AtomicBool::new(false);
        let mut state = WorkState::new(&cancel);
        assert_eq!(
            exact_classes(&group, &mut state, &mut |_| {})
                .unwrap()
                .len(),
            1
        );
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&group[1].input.candidate.path)
            .unwrap();
        file.write_all(&[0x77]).unwrap();
        file.sync_all().unwrap();
        assert!(matches!(
            finalize_hash_group(&group, &state),
            Err(WorkFailure::Unsafe(_))
        ));
    }

    #[test]
    fn cancellation_and_budget_between_batches_return_no_partial_classes() {
        let fixture = Fixture::new();
        let bytes = vec![0x66; 4096];
        let group = items(
            (0..=PEERS_PER_BATCH + 1)
                .map(|index| fixture.input(&format!("bounded-{index:02}.bin"), &bytes))
                .collect(),
        );
        let first_batch_reads = (PEERS_PER_BATCH + 1) as u64 * bytes.len() as u64;

        let cancelled = AtomicBool::new(false);
        let mut cancelled_state = WorkState::new(&cancelled);
        let mut batches = 0;
        let result =
            exact_classes_with_hook(&group, &mut cancelled_state, &mut |_| {}, &mut || {
                batches += 1;
                if batches == 1 {
                    cancelled.store(true, Ordering::Relaxed);
                }
            });
        assert!(matches!(result, Err(WorkFailure::Cancelled)));
        assert_eq!(cancelled_state.bytes_read, first_batch_reads);

        let active = AtomicBool::new(false);
        let mut limited_state = WorkState::limited(&active, first_batch_reads, MAX_ELAPSED);
        let result = exact_classes(&group, &mut limited_state, &mut |_| {});
        assert!(matches!(result, Err(WorkFailure::Limited)));
        assert_eq!(limited_state.bytes_read, first_batch_reads);
    }

    #[test]
    fn unsafe_identity_links_redirection_and_protected_paths_are_rejected() {
        let fixture = Fixture::new();
        let linked = fixture.input("linked.bin", b"linked");
        std::fs::hard_link(
            &linked.candidate.path,
            fixture.root.path.join("linked-again.bin"),
        )
        .unwrap();
        let cancel = AtomicBool::new(false);
        assert!(open_input(&linked, &WorkState::new(&cancel)).is_err());

        let changed = fixture.input("changed.bin", b"before");
        std::fs::write(&changed.candidate.path, b"after!").unwrap();
        assert!(open_input(&changed, &WorkState::new(&cancel)).is_err());

        let redirected = fixture.input("redirected.bin", b"original");
        let target = fixture.root.path.join("target.bin");
        std::fs::write(&target, b"original").unwrap();
        std::fs::remove_file(&redirected.candidate.path).unwrap();
        symlink(&target, &redirected.candidate.path).unwrap();
        assert!(open_input(&redirected, &WorkState::new(&cancel)).is_err());

        let protected_dir = fixture.root.path.join(".git");
        std::fs::create_dir(&protected_dir).unwrap();
        let protected = fixture.input_for({
            let path = protected_dir.join("secret.bin");
            std::fs::write(&path, b"secret").unwrap();
            path
        });
        assert!(open_input(&protected, &WorkState::new(&cancel)).is_err());
    }

    #[test]
    fn cancellation_and_read_budget_stop_before_unbounded_work() {
        let fixture = Fixture::new();
        let input = fixture.input("bounded.bin", &[5u8; CHUNK_BYTES]);
        let cancelled = AtomicBool::new(true);
        assert!(matches!(
            sample_digest(&input, &mut WorkState::new(&cancelled), &mut |_| {}),
            Err(WorkFailure::Cancelled)
        ));
        let active = AtomicBool::new(false);
        assert!(matches!(
            sample_digest(
                &input,
                &mut WorkState::limited(&active, CHUNK_BYTES as u64 - 1, MAX_ELAPSED),
                &mut |_| {}
            ),
            Err(WorkFailure::Limited)
        ));
    }

    #[test]
    fn staged_and_retained_path_replacements_fail_final_validation() {
        let fixture = Fixture::new();
        let copy = fixture.input("copy.bin", b"same bytes");
        let keeper = fixture.input("keeper.bin", b"same bytes");
        let stage = fixture.root.path.join(".chippytea-a-b-c");
        std::fs::rename(&copy.candidate.path, &stage).unwrap();
        let cancel = AtomicBool::new(false);
        let state = WorkState::new(&cancel);
        let staged = open_stage(&fixture.root, &copy.candidate, &stage, &state).unwrap();
        let staged_old = fixture.root.path.join("staged-old.bin");
        std::fs::rename(&stage, &staged_old).unwrap();
        std::fs::write(&stage, b"same bytes").unwrap();
        assert!(validate_stage(&fixture.root, &copy.candidate, &stage, &staged, &cancel).is_err());

        let retained = RetainedFile {
            input: keeper.clone(),
            opened: open_input(&keeper, &state).unwrap(),
        };
        let keeper_old = fixture.root.path.join("keeper-old.bin");
        std::fs::rename(&keeper.candidate.path, &keeper_old).unwrap();
        std::fs::write(&keeper.candidate.path, b"same bytes").unwrap();
        assert!(retained.validate(&cancel).is_err());
    }
}
