//! Pure npm and Bun text-lock ownership facts for one discovery scan. Separate
//! typed caches preserve each format's grammar. Callers must still supply freshly
//! and safely read, bounded bytes.
//! File identities, timestamps, workspace declarations and mutation eligibility
//! are deliberately outside the cache. Inputs below 32 KiB use the uncached
//! check, avoiding hashing and retention for small, usually unique project locks.

use crate::{model::Result, safety};
use serde_json::Value;
use std::{mem::size_of, sync::atomic::AtomicBool};

const MAX_ENTRIES: usize = 8;
const MAX_RETAINED_BYTES: usize = 8 * 1024 * 1024;
const MAX_BUN_RETAINED_BYTES: usize = 1024 * 1024;
const MIN_CACHE_BYTES: usize = 32 * 1024;

fn parsed_npm(bytes: &[u8], cancel: &AtomicBool) -> Result<Value> {
    safety::cancelled(cancel)?;
    // Keep serde_json::Value's complete validation and duplicate-key last-wins
    // semantics. In particular, any JSON number is an accepted version here.
    let parsed = serde_json::from_slice::<Value>(bytes);
    safety::cancelled(cancel)?;
    let parsed = parsed.map_err(|_| "The npm lockfile is invalid")?;
    if !parsed.get("lockfileVersion").is_some_and(Value::is_number)
        || !(parsed.get("packages").is_some_and(Value::is_object)
            || parsed.get("dependencies").is_some_and(Value::is_object))
    {
        return Err("The npm lockfile lacks recognized dependency evidence".into());
    }
    Ok(parsed)
}

fn owns_value(parsed: &Value, relative: &str) -> bool {
    relative.is_empty()
        || parsed
            .get("packages")
            .and_then(|packages| packages.get(relative))
            .is_some_and(Value::is_object)
}

/// Uncached ownership check for callers that must not reuse discovery facts.
pub(crate) fn npm_owns(bytes: &[u8], relative: &str, cancel: &AtomicBool) -> Result<bool> {
    let parsed = parsed_npm(bytes, cancel)?;
    let owns = owns_value(&parsed, relative);
    safety::cancelled(cancel)?;
    Ok(owns)
}

#[derive(Clone, Copy)]
struct KeyRange {
    offset: u32,
    length: u32,
}

struct NpmFacts {
    bytes: Vec<u8>,
    keys: Vec<KeyRange>,
}

impl NpmFacts {
    fn capture(parsed: &Value, heap_limit: usize, cancel: &AtomicBool) -> Result<Option<Self>> {
        safety::cancelled(cancel)?;
        let mut keys = Vec::new();
        let mut byte_count = 0usize;
        let mut required = 0usize;
        if let Some(packages) = parsed.get("packages").and_then(Value::as_object) {
            for (key, value) in packages {
                safety::cancelled(cancel)?;
                if !value.is_object() {
                    continue;
                }
                let Some(next) = required
                    .checked_add(key.len())
                    .and_then(|size| size.checked_add(size_of::<KeyRange>()))
                else {
                    return Ok(None);
                };
                if next > heap_limit {
                    return Ok(None);
                }
                required = next;
                byte_count += key.len();
                keys.push(key.as_bytes());
            }
        }
        // Explicit sorting also preserves lookup semantics if serde_json ever
        // uses its optional insertion-order map representation.
        keys.sort_unstable();
        safety::cancelled(cancel)?;
        let mut facts = Self {
            bytes: Vec::with_capacity(byte_count),
            keys: Vec::with_capacity(keys.len()),
        };
        // Charge actual capacities, not requested lengths. Parsing and this
        // temporary list of borrowed keys are released before the next caller.
        if facts.heap_bytes() > heap_limit {
            return Ok(None);
        }
        for key in keys {
            safety::cancelled(cancel)?;
            let (Ok(offset), Ok(length)) =
                (u32::try_from(facts.bytes.len()), u32::try_from(key.len()))
            else {
                return Ok(None);
            };
            facts.keys.push(KeyRange { offset, length });
            facts.bytes.extend_from_slice(key);
        }
        safety::cancelled(cancel)?;
        Ok(Some(facts))
    }

    fn owns(&self, relative: &str) -> bool {
        relative.is_empty()
            || self
                .keys
                .binary_search_by(|key| {
                    let start = key.offset as usize;
                    self.bytes[start..start + key.length as usize].cmp(relative.as_bytes())
                })
                .is_ok()
    }

    fn heap_bytes(&self) -> usize {
        self.bytes.capacity() + self.keys.capacity() * size_of::<KeyRange>()
    }
}

struct CachedNpm {
    digest: blake3::Hash,
    facts: NpmFacts,
}

/// Small LRU of validated content, scoped to one scan. The byte budget covers
/// retained heap capacities, including unused entry slots and both packed-key
/// buffers. It excludes allocator bookkeeping, this fixed-size inline object,
/// and transient input/serde allocations. The filesystem reader bounds input.
pub(crate) struct NpmLockCache {
    entries: Vec<CachedNpm>,
    entry_limit: usize,
    byte_limit: usize,
    retained_bytes: usize,
}

impl Default for NpmLockCache {
    fn default() -> Self {
        Self::with_limits(MAX_ENTRIES, MAX_RETAINED_BYTES)
    }
}

impl NpmLockCache {
    fn with_limits(entry_limit: usize, byte_limit: usize) -> Self {
        let mut entry_limit = entry_limit
            .min(MAX_ENTRIES)
            .min(byte_limit / size_of::<CachedNpm>());
        let mut entries = Vec::with_capacity(entry_limit);
        let mut retained_bytes = entries.capacity() * size_of::<CachedNpm>();
        if retained_bytes > byte_limit {
            entries = Vec::new();
            entry_limit = 0;
            retained_bytes = 0;
        }
        Self {
            entries,
            entry_limit,
            byte_limit,
            retained_bytes,
        }
    }

    #[cfg(test)]
    pub(crate) fn owns(
        &mut self,
        bytes: &[u8],
        relative: &str,
        cancel: &AtomicBool,
    ) -> Result<bool> {
        self.owns_captured(None, bytes, relative, cancel)
    }

    /// A caller that already holds the blake3 digest of exactly these bytes may
    /// pass it to avoid re-hashing. The digest and bytes must come from one
    /// captured evidence source; a mismatched pair would corrupt fact lookups.
    pub(crate) fn owns_captured(
        &mut self,
        digest: Option<blake3::Hash>,
        bytes: &[u8],
        relative: &str,
        cancel: &AtomicBool,
    ) -> Result<bool> {
        safety::cancelled(cancel)?;
        if self.entry_limit == 0 || bytes.len() < MIN_CACHE_BYTES {
            return npm_owns(bytes, relative, cancel);
        }
        let digest = digest.unwrap_or_else(|| blake3::hash(bytes));
        safety::cancelled(cancel)?;
        if let Some(index) = self.entries.iter().position(|entry| entry.digest == digest) {
            let owns = self.entries[index].facts.owns(relative);
            safety::cancelled(cancel)?;
            self.entries[index..].rotate_left(1);
            return Ok(owns);
        }

        let parsed = parsed_npm(bytes, cancel)?;
        let owns = owns_value(&parsed, relative);
        let container_bytes = self.entries.capacity() * size_of::<CachedNpm>();
        let Some(facts) = NpmFacts::capture(&parsed, self.byte_limit - container_bytes, cancel)?
        else {
            // A valid, oversized lock still answers normally. It neither evicts
            // useful small entries nor weakens the fresh ownership check.
            return Ok(owns);
        };
        let heap_bytes = facts.heap_bytes();
        safety::cancelled(cancel)?;
        while self.entries.len() == self.entry_limit
            || heap_bytes > self.byte_limit - self.retained_bytes
        {
            let evicted = self.entries.remove(0);
            self.retained_bytes -= evicted.facts.heap_bytes();
        }
        self.retained_bytes += heap_bytes;
        self.entries.push(CachedNpm { digest, facts });
        Ok(owns)
    }
}

/// Bun's text lock is JSON with comments and trailing commas. Normalize only
/// those two extensions, preserving strings verbatim, then use serde's parser.
fn bun_json(bytes: &[u8], cancel: &AtomicBool) -> Result<Value> {
    safety::cancelled(cancel)?;
    let mut normalized = bytes.to_vec();
    let mut index = 0;
    let mut string = false;
    let mut escaped = false;
    while index < normalized.len() {
        if index & 4095 == 0 {
            safety::cancelled(cancel)?;
        }
        let byte = normalized[index];
        if string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                string = false;
            }
        } else if byte == b'"' {
            string = true;
        } else if byte == b'/' && normalized.get(index + 1) == Some(&b'/') {
            while index < normalized.len() && normalized[index] != b'\n' {
                if index & 4095 == 0 {
                    safety::cancelled(cancel)?;
                }
                normalized[index] = b' ';
                index += 1;
            }
            continue;
        } else if byte == b'/' && normalized.get(index + 1) == Some(&b'*') {
            normalized[index] = b' ';
            normalized[index + 1] = b' ';
            index += 2;
            loop {
                if index & 4095 == 0 {
                    safety::cancelled(cancel)?;
                }
                if index + 1 >= normalized.len() {
                    return Err("The Bun text lock contains an unfinished comment".into());
                }
                if normalized[index] == b'*' && normalized[index + 1] == b'/' {
                    normalized[index] = b' ';
                    normalized[index + 1] = b' ';
                    index += 2;
                    break;
                }
                normalized[index] = b' ';
                index += 1;
            }
            continue;
        }
        index += 1;
    }
    string = false;
    escaped = false;
    for index in 0..normalized.len() {
        if index & 4095 == 0 {
            safety::cancelled(cancel)?;
        }
        let byte = normalized[index];
        if string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                string = false;
            }
        } else if byte == b'"' {
            string = true;
        } else if byte == b',' {
            let next = normalized[index + 1..]
                .iter()
                .find(|byte| !byte.is_ascii_whitespace());
            if matches!(next, Some(b'}' | b']')) {
                normalized[index] = b' ';
            }
        }
    }
    safety::cancelled(cancel)?;
    let parsed = serde_json::from_slice(&normalized);
    safety::cancelled(cancel)?;
    parsed.map_err(|_| "The Bun text lock is invalid JSON".into())
}

fn parsed_bun(bytes: &[u8], cancel: &AtomicBool) -> Result<Value> {
    let parsed = bun_json(bytes, cancel)?;
    // Match the existing parser exactly: 1.0 is not an integer version, and
    // duplicate keys retain serde_json::Value's last-wins semantics.
    if !matches!(
        parsed.get("lockfileVersion").and_then(Value::as_u64),
        Some(1 | 2)
    ) || !parsed.get("packages").is_some_and(Value::is_object)
    {
        return Err("The Bun text lock lacks a recognized version and package table".into());
    }
    Ok(parsed)
}

fn bun_owns_value(parsed: &Value, relative: &str, manifest_name: Option<&str>) -> bool {
    let Some(workspace) = parsed
        .get("workspaces")
        .and_then(|workspaces| workspaces.get(relative))
        .filter(|workspace| workspace.is_object())
    else {
        return false;
    };
    match (workspace.get("name").and_then(Value::as_str), manifest_name) {
        (Some(locked), Some(actual)) => locked == actual,
        _ => true,
    }
}

/// Uncached Bun ownership check, including the caller's current manifest name.
/// Even the empty/root workspace needs an object in the lock's workspace table.
pub(crate) fn bun_owns(
    bytes: &[u8],
    relative: &str,
    manifest_name: Option<&str>,
    cancel: &AtomicBool,
) -> Result<bool> {
    let parsed = parsed_bun(bytes, cancel)?;
    let owns = bun_owns_value(&parsed, relative, manifest_name);
    safety::cancelled(cancel)?;
    Ok(owns)
}

struct BunWorkspace {
    key: KeyRange,
    name: Option<KeyRange>,
}

struct BunFacts {
    bytes: Vec<u8>,
    workspaces: Vec<BunWorkspace>,
}

impl BunFacts {
    fn capture(parsed: &Value, heap_limit: usize, cancel: &AtomicBool) -> Result<Option<Self>> {
        safety::cancelled(cancel)?;
        let mut workspaces = Vec::new();
        let mut byte_count = 0usize;
        let mut required = 0usize;
        if let Some(members) = parsed.get("workspaces").and_then(Value::as_object) {
            for (key, value) in members {
                safety::cancelled(cancel)?;
                if !value.is_object() {
                    continue;
                }
                let name = value.get("name").and_then(Value::as_str);
                let Some(bytes) = key.len().checked_add(name.map_or(0, str::len)) else {
                    return Ok(None);
                };
                let Some(next) = required
                    .checked_add(bytes)
                    .and_then(|size| size.checked_add(size_of::<BunWorkspace>()))
                else {
                    return Ok(None);
                };
                if next > heap_limit {
                    return Ok(None);
                }
                required = next;
                byte_count += bytes;
                workspaces.push((key.as_str(), name));
            }
        }
        // Do not depend on serde_json's optional insertion-order feature.
        workspaces.sort_unstable_by(|left, right| left.0.cmp(right.0));
        safety::cancelled(cancel)?;
        let mut facts = Self {
            bytes: Vec::with_capacity(byte_count),
            workspaces: Vec::with_capacity(workspaces.len()),
        };
        // Retain only workspace keys and optional string names, never the
        // parsed dependency tree. Charge capacities before filling the buffers.
        if facts.heap_bytes() > heap_limit {
            return Ok(None);
        }
        for (key, name) in workspaces {
            safety::cancelled(cancel)?;
            let Some(key) = facts.append(key) else {
                return Ok(None);
            };
            let name = match name {
                Some(name) => match facts.append(name) {
                    Some(range) => Some(range),
                    None => return Ok(None),
                },
                None => None,
            };
            facts.workspaces.push(BunWorkspace { key, name });
        }
        safety::cancelled(cancel)?;
        Ok(Some(facts))
    }

    fn append(&mut self, value: &str) -> Option<KeyRange> {
        let offset = u32::try_from(self.bytes.len()).ok()?;
        let length = u32::try_from(value.len()).ok()?;
        self.bytes.extend_from_slice(value.as_bytes());
        Some(KeyRange { offset, length })
    }

    fn slice(&self, range: KeyRange) -> &[u8] {
        let start = range.offset as usize;
        &self.bytes[start..start + range.length as usize]
    }

    fn owns(&self, relative: &str, manifest_name: Option<&str>) -> bool {
        let Ok(index) = self
            .workspaces
            .binary_search_by(|workspace| self.slice(workspace.key).cmp(relative.as_bytes()))
        else {
            return false;
        };
        match (self.workspaces[index].name, manifest_name) {
            (Some(locked), Some(actual)) => self.slice(locked) == actual.as_bytes(),
            _ => true,
        }
    }

    fn heap_bytes(&self) -> usize {
        self.bytes.capacity() + self.workspaces.capacity() * size_of::<BunWorkspace>()
    }
}

struct CachedBun {
    digest: blake3::Hash,
    facts: BunFacts,
}

/// Bun's own scan-local LRU. The 1 MiB cap covers actual retained capacities:
/// unused entry slots, packed workspace keys/names and their range records.
/// Input/normalization/serde allocations are transient and are not retained;
/// filesystem input limits and the existing uncached grammar remain unchanged.
pub(crate) struct BunLockCache {
    entries: Vec<CachedBun>,
    entry_limit: usize,
    byte_limit: usize,
    retained_bytes: usize,
}

impl Default for BunLockCache {
    fn default() -> Self {
        Self::with_limits(MAX_ENTRIES, MAX_BUN_RETAINED_BYTES)
    }
}

impl BunLockCache {
    fn with_limits(entry_limit: usize, byte_limit: usize) -> Self {
        let mut entry_limit = entry_limit
            .min(MAX_ENTRIES)
            .min(byte_limit / size_of::<CachedBun>());
        let mut entries = Vec::with_capacity(entry_limit);
        let mut retained_bytes = entries.capacity() * size_of::<CachedBun>();
        if retained_bytes > byte_limit {
            entries = Vec::new();
            entry_limit = 0;
            retained_bytes = 0;
        }
        Self {
            entries,
            entry_limit,
            byte_limit,
            retained_bytes,
        }
    }

    /// `digest`, when supplied, must describe exactly the bytes in the same
    /// identity-verified EvidenceSource. The manifest name is checked anew on
    /// every lookup; it is not part of the cached lockfile's ownership facts.
    pub(crate) fn owns_captured(
        &mut self,
        digest: Option<blake3::Hash>,
        bytes: &[u8],
        relative: &str,
        manifest_name: Option<&str>,
        cancel: &AtomicBool,
    ) -> Result<bool> {
        safety::cancelled(cancel)?;
        if self.entry_limit == 0 || bytes.len() < MIN_CACHE_BYTES {
            return bun_owns(bytes, relative, manifest_name, cancel);
        }
        let digest = digest.unwrap_or_else(|| blake3::hash(bytes));
        safety::cancelled(cancel)?;
        if let Some(index) = self.entries.iter().position(|entry| entry.digest == digest) {
            let owns = self.entries[index].facts.owns(relative, manifest_name);
            safety::cancelled(cancel)?;
            self.entries[index..].rotate_left(1);
            return Ok(owns);
        }

        let parsed = parsed_bun(bytes, cancel)?;
        let owns = bun_owns_value(&parsed, relative, manifest_name);
        let container_bytes = self.entries.capacity() * size_of::<CachedBun>();
        let Some(facts) = BunFacts::capture(&parsed, self.byte_limit - container_bytes, cancel)?
        else {
            // Oversized valid facts still answer normally without displacing
            // retained locks. Neither invalid nor cancelled parses are cached.
            return Ok(owns);
        };
        let heap_bytes = facts.heap_bytes();
        safety::cancelled(cancel)?;
        while self.entries.len() == self.entry_limit
            || heap_bytes > self.byte_limit - self.retained_bytes
        {
            let evicted = self.entries.remove(0);
            self.retained_bytes -= evicted.facts.heap_bytes();
        }
        self.retained_bytes += heap_bytes;
        self.entries.push(CachedBun { digest, facts });
        Ok(owns)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    fn cacheable(source: &[u8]) -> Vec<u8> {
        let mut bytes = source.to_vec();
        bytes.resize(bytes.len().max(MIN_CACHE_BYTES), b' ');
        bytes
    }

    fn document(member: &str) -> Vec<u8> {
        cacheable(
            &serde_json::to_vec(&serde_json::json!({
                "lockfileVersion": 3,
                "packages": {(member): {}},
            }))
            .unwrap(),
        )
    }

    fn assert_budget(cache: &NpmLockCache) {
        let actual = cache.entries.capacity() * size_of::<CachedNpm>()
            + cache
                .entries
                .iter()
                .map(|entry| entry.facts.heap_bytes())
                .sum::<usize>();
        assert_eq!(cache.retained_bytes, actual);
        assert!(actual <= cache.byte_limit);
        assert!(cache.entries.len() <= cache.entry_limit);
    }

    #[test]
    fn valid_npm_shapes_preserve_exact_member_and_duplicate_key_semantics() {
        let cancel = AtomicBool::new(false);
        let mut cache = NpmLockCache::default();
        let cases: &[(&str, &[&str])] = &[
            (r#"{"lockfileVersion":3,"packages":{}}"#, &[]),
            (
                r#"{"lockfileVersion":-0.5,"packages":{"apps/a":{}}}"#,
                &["apps/a"],
            ),
            (r#"{"lockfileVersion":3,"dependencies":{}}"#, &[]),
            (
                r#"{"lockfileVersion":3,"packages":false,"dependencies":{}}"#,
                &[],
            ),
            (
                r#"{"lockfileVersion":3,"packages":[{}],"dependencies":{}}"#,
                &[],
            ),
            (
                r#"{"lockfileVersion":3,"packages":{"apps/a":{},"apps/b":[],"apps/c":null}}"#,
                &["apps/a"],
            ),
            (
                r#"{"lockfileVersion":"bad","lockfileVersion":3.0,"packages":{"apps/a":{}}}"#,
                &["apps/a"],
            ),
            (
                r#"{"lockfileVersion":3,"packages":{"apps/a":{}},"packages":{},"dependencies":{}}"#,
                &[],
            ),
            (
                r#"{"lockfileVersion":3,"packages":false,"packages":{"apps/b":{}}}"#,
                &["apps/b"],
            ),
            (
                r#"{"lockfileVersion":3,"packages":{"apps/a":{},"apps/a":false}}"#,
                &[],
            ),
            (
                r#"{"lockfileVersion":3,"packages":{"apps/a":false,"apps/a":{}}}"#,
                &["apps/a"],
            ),
            (
                r#"{"lockfileVersion":3,"packages":{"apps/a":{},"\u0061pps/a":false,"apps/b":{}}}"#,
                &["apps/b"],
            ),
        ];
        for (source, members) in cases {
            let bytes = cacheable(source.as_bytes());
            // Repeated queries cover both fresh parsing and packed cache hits.
            for _ in 0..2 {
                for relative in ["", "apps/a", "apps/b", "apps/c", "apps", "apps/a/child"] {
                    let expected = relative.is_empty() || members.contains(&relative);
                    assert_eq!(npm_owns(source.as_bytes(), relative, &cancel), Ok(expected));
                    assert_eq!(cache.owns(&bytes, relative, &cancel), Ok(expected));
                }
            }
            assert_budget(&cache);
        }
    }

    #[test]
    fn invalid_documents_keep_existing_errors_and_do_not_enter_the_cache() {
        let cancel = AtomicBool::new(false);
        let mut cache = NpmLockCache::default();
        let cases: &[(&[u8], &str)] = &[
            (b"", "The npm lockfile is invalid"),
            (b"\xff", "The npm lockfile is invalid"),
            (
                br#"{"lockfileVersion":3,"packages":{}} trailing"#,
                "The npm lockfile is invalid",
            ),
            (
                br#"{"lockfileVersion":3,"packages":{"apps/a":{,}}}"#,
                "The npm lockfile is invalid",
            ),
            (
                br#"{"lockfileVersion":1e400,"packages":{}}"#,
                "The npm lockfile is invalid",
            ),
            (
                b"null",
                "The npm lockfile lacks recognized dependency evidence",
            ),
            (
                b"[]",
                "The npm lockfile lacks recognized dependency evidence",
            ),
            (
                br#"{"lockfileVersion":"3","packages":{}}"#,
                "The npm lockfile lacks recognized dependency evidence",
            ),
            (
                br#"{"lockfileVersion":3,"packages":[]}"#,
                "The npm lockfile lacks recognized dependency evidence",
            ),
            (
                br#"{"lockfileVersion":3,"lockfileVersion":false,"packages":{}}"#,
                "The npm lockfile lacks recognized dependency evidence",
            ),
            (
                br#"{"lockfileVersion":3,"packages":{},"packages":false}"#,
                "The npm lockfile lacks recognized dependency evidence",
            ),
        ];
        for (source, message) in cases {
            let bytes = cacheable(source);
            for relative in ["", "apps/a"] {
                assert_eq!(npm_owns(source, relative, &cancel), Err((*message).into()));
                assert_eq!(
                    cache.owns(&bytes, relative, &cancel),
                    Err((*message).into())
                );
            }
            assert!(cache.entries.is_empty());
            assert_budget(&cache);
        }
    }

    #[test]
    fn packed_keys_preserve_empty_prefix_unicode_and_embedded_nul_names() {
        let cancel = AtomicBool::new(false);
        let source = r#"{"lockfileVersion":3,"packages":{"":false,"a":{},"ab":{},"a/b":{},"é":{},"\u0000":{},"a/b/c":null}}"#;
        let bytes = cacheable(source.as_bytes());
        let mut cache = NpmLockCache::default();
        for relative in ["", "a", "ab", "a/b", "é", "\0"] {
            assert!(cache.owns(&bytes, relative, &cancel).unwrap());
        }
        for relative in ["a/", "a/b/c", "e\u{301}", "\0a"] {
            assert!(!cache.owns(&bytes, relative, &cancel).unwrap());
        }
        let facts = &cache.entries[0].facts;
        assert_eq!(facts.keys.len(), 5);
        assert_eq!(facts.bytes.len(), 9);
        assert_budget(&cache);
    }

    #[test]
    fn changed_bytes_with_the_same_length_produce_fresh_ownership_facts() {
        let cancel = AtomicBool::new(false);
        let mut cache = NpmLockCache::default();
        let before = document("apps/a");
        let after = document("apps/b");
        assert_eq!(before.len(), after.len());
        assert!(cache.owns(&before, "apps/a", &cancel).unwrap());
        assert!(!cache.owns(&after, "apps/a", &cancel).unwrap());
        assert!(cache.owns(&after, "apps/b", &cancel).unwrap());
        assert!(cache.owns(&before, "apps/a", &cancel).unwrap());
        assert_eq!(cache.entries.len(), 2);
        assert_budget(&cache);
    }

    #[test]
    fn entry_eviction_keeps_the_most_recently_used_facts() {
        let cancel = AtomicBool::new(false);
        let mut cache = NpmLockCache::with_limits(2, 4096);
        let first = document("a");
        let second = document("b");
        let third = document("c");
        for source in [&first, &second, &first, &third] {
            assert!(cache.owns(source, "", &cancel).unwrap());
        }
        assert_eq!(cache.entries.len(), 2);
        assert_eq!(cache.entries[0].digest, blake3::hash(&first));
        assert_eq!(cache.entries[1].digest, blake3::hash(&third));
        assert!(cache.owns(&second, "b", &cancel).unwrap());
        assert!(!cache.owns(&second, "a", &cancel).unwrap());
        assert_budget(&cache);
    }

    #[test]
    fn byte_eviction_and_oversized_bypass_count_all_retained_capacities() {
        let cancel = AtomicBool::new(false);
        let one_key_bytes = 1 + size_of::<KeyRange>();
        let budget = 2 * size_of::<CachedNpm>() + one_key_bytes;
        let mut cache = NpmLockCache::with_limits(2, budget);
        assert!(cache.owns(&document("a"), "a", &cancel).unwrap());
        assert!(cache.owns(&document("b"), "b", &cancel).unwrap());
        assert_eq!(
            cache.entries.len(),
            1,
            "The byte cap evicts before the entry cap"
        );
        assert_eq!(cache.entries[0].digest, blake3::hash(&document("b")));
        assert_budget(&cache);

        let large_key = "x".repeat(4096);
        let large = document(&large_key);
        assert!(cache.owns(&large, &large_key, &cancel).unwrap());
        assert!(!cache.owns(&large, "b", &cancel).unwrap());
        assert_eq!(cache.entries.len(), 1);
        assert_eq!(cache.entries[0].digest, blake3::hash(&document("b")));
        assert_budget(&cache);
    }

    #[test]
    fn zero_or_tiny_budgets_use_uncached_semantics() {
        let cancel = AtomicBool::new(false);
        for (entries, bytes) in [(0, 4096), (8, 0), (8, size_of::<CachedNpm>() - 1)] {
            let mut cache = NpmLockCache::with_limits(entries, bytes);
            assert!(cache.owns(&document("a"), "a", &cancel).unwrap());
            assert!(!cache.owns(&document("a"), "b", &cancel).unwrap());
            assert!(cache.owns(b"broken", "", &cancel).is_err());
            assert!(cache.entries.is_empty());
            assert_budget(&cache);
        }
    }

    #[test]
    fn small_locks_bypass_retention_below_the_exact_threshold() {
        let cancel = AtomicBool::new(false);
        let mut cache = NpmLockCache::default();
        let source = br#"{"lockfileVersion":3,"packages":{"a":{}}}"#;
        let mut below = source.to_vec();
        below.resize(MIN_CACHE_BYTES - 1, b' ');
        for bytes in [source.as_slice(), below.as_slice()] {
            assert!(cache.owns(bytes, "a", &cancel).unwrap());
            assert!(!cache.owns(bytes, "b", &cancel).unwrap());
            assert!(cache.entries.is_empty());
        }
        below.push(b' ');
        assert!(cache.owns(&below, "a", &cancel).unwrap());
        assert_eq!(cache.entries.len(), 1);
        assert_budget(&cache);
    }

    #[test]
    fn cancellation_applies_to_uncached_misses_hits_and_compaction() {
        let cancel = AtomicBool::new(false);
        let source = document("a");
        let mut cache = NpmLockCache::default();
        assert!(cache.owns(&source, "a", &cancel).unwrap());
        let retained = cache.retained_bytes;
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(cache.owns(&source, "a", &cancel), Err("Cancelled".into()));
        assert_eq!(
            cache.owns(&document("b"), "b", &cancel),
            Err("Cancelled".into())
        );
        assert_eq!(cache.owns(b"broken", "", &cancel), Err("Cancelled".into()));
        assert_eq!(npm_owns(&source, "a", &cancel), Err("Cancelled".into()));
        let parsed: Value = serde_json::from_slice(&source).unwrap();
        assert!(
            matches!(NpmFacts::capture(&parsed, 4096, &cancel), Err(message) if message == "Cancelled")
        );
        assert_eq!(cache.entries.len(), 1);
        assert_eq!(cache.retained_bytes, retained);
        assert_budget(&cache);
    }

    fn bun_document(member: &str, name: Option<&str>) -> Vec<u8> {
        let workspace = match name {
            Some(name) => serde_json::json!({"name": name}),
            None => serde_json::json!({}),
        };
        cacheable(
            &serde_json::to_vec(&serde_json::json!({
                "lockfileVersion": 1,
                "packages": {},
                "workspaces": {(member): workspace},
            }))
            .unwrap(),
        )
    }

    fn assert_bun_budget(cache: &BunLockCache) {
        let actual = cache.entries.capacity() * size_of::<CachedBun>()
            + cache
                .entries
                .iter()
                .map(|entry| entry.facts.heap_bytes())
                .sum::<usize>();
        assert_eq!(cache.retained_bytes, actual);
        assert!(actual <= cache.byte_limit);
        assert!(cache.entries.len() <= cache.entry_limit);
    }

    #[test]
    fn bun_cache_preserves_member_name_type_and_duplicate_key_semantics() {
        type ExpectedWorkspace = (&'static str, Option<&'static str>);
        let cases: &[(&str, &[ExpectedWorkspace])] = &[
            (
                r#"{"lockfileVersion":1,"packages":{},"workspaces":{"":{"name":"root"},"apps/a":{"name":"alpha"},"apps/b":{},"apps/c":{"name":7}}}"#,
                &[
                    ("", Some("root")),
                    ("apps/a", Some("alpha")),
                    ("apps/b", None),
                    ("apps/c", None),
                ],
            ),
            (r#"{"lockfileVersion":2,"packages":{}}"#, &[]),
            (
                r#"{"lockfileVersion":2,"packages":{},"workspaces":[]}"#,
                &[],
            ),
            (
                r#"{"lockfileVersion":1,"packages":{},"workspaces":{"apps/a":false,"apps/b":[],"apps/c":null}}"#,
                &[],
            ),
            (
                r#"{"lockfileVersion":"bad","lockfileVersion":2,"packages":{},"workspaces":{"apps/a":{"name":"alpha"}}}"#,
                &[("apps/a", Some("alpha"))],
            ),
            (
                r#"{"lockfileVersion":1,"packages":false,"packages":{},"workspaces":{"apps/a":{"name":"alpha","name":false}}}"#,
                &[("apps/a", None)],
            ),
            (
                r#"{"lockfileVersion":1,"packages":{},"workspaces":{"apps/a":{"name":false,"name":"omega"}}}"#,
                &[("apps/a", Some("omega"))],
            ),
            (
                r#"{"lockfileVersion":1,"packages":{},"workspaces":{"apps/a":{},"\u0061pps/a":false}}"#,
                &[],
            ),
            (
                r#"{"lockfileVersion":1,"packages":{},"workspaces":{"apps/a":false,"apps/a":{"name":"alpha"}}}"#,
                &[("apps/a", Some("alpha"))],
            ),
            (
                r#"{"lockfileVersion":1,"packages":{},"workspaces":{"apps/a":{}},"workspaces":{}}"#,
                &[],
            ),
            (
                "// leading comment\n{\"lockfileVersion\":1, /* before table */ \"packages\":{},\"workspaces\":{\"apps/a\":{\"name\":\"alpha\",},},} // end",
                &[("apps/a", Some("alpha"))],
            ),
        ];
        let cancel = AtomicBool::new(false);
        let mut cache = BunLockCache::default();
        for (source, members) in cases {
            let bytes = cacheable(source.as_bytes());
            let digest = blake3::hash(&bytes);
            for _ in 0..2 {
                for relative in ["", "apps/a", "apps/b", "apps/c", "apps", "apps/a/child"] {
                    for name in [None, Some("root"), Some("alpha"), Some("omega"), Some("")] {
                        let expected = members.iter().any(|(path, locked)| {
                            *path == relative
                                && match (locked, name) {
                                    (Some(locked), Some(actual)) => *locked == actual,
                                    _ => true,
                                }
                        });
                        assert_eq!(
                            bun_owns(source.as_bytes(), relative, name, &cancel),
                            Ok(expected)
                        );
                        assert_eq!(
                            cache.owns_captured(Some(digest), &bytes, relative, name, &cancel),
                            Ok(expected),
                        );
                    }
                }
            }
            assert_bun_budget(&cache);
        }
    }

    #[test]
    fn bun_invalid_documents_keep_errors_and_never_enter_cache() {
        let parse_error = "The Bun text lock is invalid JSON";
        let shape_error = "The Bun text lock lacks a recognized version and package table";
        let cases: &[(&[u8], &str)] = &[
            (b"", parse_error),
            (b"\xff", parse_error),
            (
                br#"{"lockfileVersion":1,"packages":{}} trailing"#,
                parse_error,
            ),
            (br#"{"lockfileVersion":1,"packages":{,,}}"#, parse_error),
            (br#"{"lockfileVersion":1e400,"packages":{}}"#, parse_error),
            (
                b"/* unfinished",
                "The Bun text lock contains an unfinished comment",
            ),
            (b"null", shape_error),
            (b"[]", shape_error),
            (br#"{"lockfileVersion":1.0,"packages":{}}"#, shape_error),
            (br#"{"lockfileVersion":0,"packages":{}}"#, shape_error),
            (br#"{"lockfileVersion":3,"packages":{}}"#, shape_error),
            (br#"{"lockfileVersion":-1,"packages":{}}"#, shape_error),
            (br#"{"lockfileVersion":"1","packages":{}}"#, shape_error),
            (
                br#"{"lockfileVersion":1,"lockfileVersion":false,"packages":{}}"#,
                shape_error,
            ),
            (br#"{"lockfileVersion":1,"packages":[]}"#, shape_error),
            (
                br#"{"lockfileVersion":1,"packages":{},"packages":false}"#,
                shape_error,
            ),
        ];
        let cancel = AtomicBool::new(false);
        let mut cache = BunLockCache::default();
        for (source, message) in cases {
            let bytes = cacheable(source);
            for relative in ["", "apps/a"] {
                assert_eq!(
                    bun_owns(source, relative, Some("a"), &cancel),
                    Err((*message).into())
                );
                assert_eq!(
                    cache.owns_captured(None, &bytes, relative, Some("a"), &cancel),
                    Err((*message).into()),
                );
            }
            assert!(cache.entries.is_empty());
            assert_bun_budget(&cache);
        }
    }

    #[test]
    fn bun_normalization_and_packed_names_preserve_string_bytes() {
        let cancel = AtomicBool::new(false);
        let name = "https://example.invalid/a//b/*c*/,}\\\"";
        let mut workspaces = serde_json::Map::new();
        for key in ["", "a", "ab", "a/b", "é", "\0"] {
            workspaces.insert(key.into(), serde_json::json!({"name": name}));
        }
        let source = cacheable(
            &serde_json::to_vec(&serde_json::json!({
                "lockfileVersion": 2,
                "packages": {},
                "workspaces": workspaces,
            }))
            .unwrap(),
        );
        let mut cache = BunLockCache::default();
        for relative in ["", "a", "ab", "a/b", "é", "\0"] {
            assert!(bun_owns(&source, relative, Some(name), &cancel).unwrap());
            assert!(
                cache
                    .owns_captured(None, &source, relative, Some(name), &cancel)
                    .unwrap()
            );
            assert!(
                !cache
                    .owns_captured(None, &source, relative, Some("other"), &cancel)
                    .unwrap()
            );
        }
        for relative in ["a/", "a/b/c", "e\u{301}", "\0a"] {
            assert!(
                !cache
                    .owns_captured(None, &source, relative, Some(name), &cancel)
                    .unwrap()
            );
        }
        assert_eq!(cache.entries[0].facts.workspaces.len(), 6);
        assert_bun_budget(&cache);
    }

    #[test]
    fn bun_changed_digest_and_current_manifest_name_are_both_observed() {
        let cancel = AtomicBool::new(false);
        let mut cache = BunLockCache::default();
        let before = bun_document("apps/a", Some("alpha"));
        let after = bun_document("apps/a", Some("omega"));
        assert_eq!(before.len(), after.len());
        for (source, current, old) in [(&before, "alpha", "omega"), (&after, "omega", "alpha")] {
            let digest = Some(blake3::hash(source));
            assert!(
                cache
                    .owns_captured(digest, source, "apps/a", Some(current), &cancel)
                    .unwrap()
            );
            assert!(
                !cache
                    .owns_captured(digest, source, "apps/a", Some(old), &cancel)
                    .unwrap()
            );
            assert!(
                cache
                    .owns_captured(digest, source, "apps/a", None, &cancel)
                    .unwrap()
            );
            assert!(
                !cache
                    .owns_captured(digest, source, "apps/b", None, &cancel)
                    .unwrap()
            );
        }
        assert_eq!(cache.entries.len(), 2);
        assert_bun_budget(&cache);
    }

    #[test]
    fn bun_entry_eviction_keeps_recent_facts() {
        let cancel = AtomicBool::new(false);
        let mut cache = BunLockCache::with_limits(2, 4096);
        let first = bun_document("a", None);
        let second = bun_document("b", None);
        let third = bun_document("c", None);
        for (source, member) in [(&first, "a"), (&second, "b"), (&first, "a"), (&third, "c")] {
            assert!(
                cache
                    .owns_captured(None, source, member, None, &cancel)
                    .unwrap()
            );
        }
        assert_eq!(cache.entries.len(), 2);
        assert_eq!(cache.entries[0].digest, blake3::hash(&first));
        assert_eq!(cache.entries[1].digest, blake3::hash(&third));
        assert_bun_budget(&cache);
    }

    #[test]
    fn bun_byte_eviction_charges_names_and_oversized_facts_do_not_displace_cache() {
        let cancel = AtomicBool::new(false);
        let budget = 2 * size_of::<CachedBun>() + size_of::<BunWorkspace>() + 2;
        let mut cache = BunLockCache::with_limits(2, budget);
        for key in ["a", "b"] {
            assert!(
                cache
                    .owns_captured(None, &bun_document(key, Some("n")), key, Some("n"), &cancel)
                    .unwrap()
            );
        }
        assert_eq!(cache.entries.len(), 1);
        let retained_digest = cache.entries[0].digest;
        assert_eq!(retained_digest, blake3::hash(&bun_document("b", Some("n"))));
        for (key, name) in [
            ("x".repeat(4096), "n".into()),
            ("a".into(), "x".repeat(4096)),
        ] {
            let source = bun_document(&key, Some(&name));
            assert!(
                cache
                    .owns_captured(None, &source, &key, Some(&name), &cancel)
                    .unwrap()
            );
            assert!(
                !cache
                    .owns_captured(None, &source, &key, Some("other"), &cancel)
                    .unwrap()
            );
            assert_eq!(cache.entries.len(), 1);
            assert_eq!(cache.entries[0].digest, retained_digest);
            assert_bun_budget(&cache);
        }
    }

    #[test]
    fn bun_small_sources_and_tiny_budgets_keep_uncached_semantics() {
        let cancel = AtomicBool::new(false);
        let source = br#"{"lockfileVersion":1,"packages":{},"workspaces":{"a":{}}}"#;
        for (entries, bytes) in [(0, 4096), (8, 0), (8, size_of::<CachedBun>() - 1)] {
            let mut cache = BunLockCache::with_limits(entries, bytes);
            let large = cacheable(source);
            assert!(
                cache
                    .owns_captured(None, &large, "a", None, &cancel)
                    .unwrap()
            );
            assert!(
                !cache
                    .owns_captured(None, &large, "", None, &cancel)
                    .unwrap()
            );
            assert!(
                cache
                    .owns_captured(None, b"broken", "a", None, &cancel)
                    .is_err()
            );
            assert!(cache.entries.is_empty());
            assert_bun_budget(&cache);
        }
        let mut cache = BunLockCache::default();
        let mut below = source.to_vec();
        below.resize(MIN_CACHE_BYTES - 1, b' ');
        for bytes in [source.as_slice(), below.as_slice()] {
            assert!(
                cache
                    .owns_captured(None, bytes, "a", None, &cancel)
                    .unwrap()
            );
            assert!(cache.entries.is_empty());
        }
        below.push(b' ');
        assert!(
            cache
                .owns_captured(None, &below, "a", None, &cancel)
                .unwrap()
        );
        assert_eq!(cache.entries.len(), 1);
        assert_bun_budget(&cache);
    }

    #[test]
    fn bun_cancellation_covers_uncached_misses_hits_and_compaction() {
        let cancel = AtomicBool::new(false);
        let source = bun_document("a", Some("name"));
        let mut cache = BunLockCache::default();
        assert!(
            cache
                .owns_captured(None, &source, "a", Some("name"), &cancel)
                .unwrap()
        );
        let retained = cache.retained_bytes;
        cancel.store(true, Ordering::Relaxed);
        let other = bun_document("b", None);
        for bytes in [source.as_slice(), other.as_slice(), b"broken"] {
            assert_eq!(
                cache.owns_captured(None, bytes, "a", None, &cancel),
                Err("Cancelled".into())
            );
        }
        assert_eq!(
            bun_owns(&source, "a", None, &cancel),
            Err("Cancelled".into())
        );
        let parsed: Value = serde_json::from_slice(&source).unwrap();
        assert!(
            matches!(BunFacts::capture(&parsed, 4096, &cancel), Err(message) if message == "Cancelled")
        );
        assert_eq!(cache.entries.len(), 1);
        assert_eq!(cache.retained_bytes, retained);
        assert_bun_budget(&cache);
    }
}
