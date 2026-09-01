# Discovery, accounting, and benchmark reference study

Reviewed on 2026-08-31 against the commits below. This document records design
references, not benchmark results or a claim that every technique is implemented.
No upstream source was copied into chippytea during this study.

## Repositories and licenses

| Project | Reviewed commit | License |
| --- | --- | --- |
| [Kondo](https://github.com/tbillington/kondo/tree/1d351ca80b3d3adfad9bbe7db872c27359190210) | `1d351ca80b3d3adfad9bbe7db872c27359190210` | [MIT](https://github.com/tbillington/kondo/blob/1d351ca80b3d3adfad9bbe7db872c27359190210/LICENSE), copyright Trent Billington |
| [dua](https://github.com/Byron/dua-cli/tree/ebf4cffd611953725ac7819a8f02d0bf7afd1e75) | `ebf4cffd611953725ac7819a8f02d0bf7afd1e75` | [MIT](https://github.com/Byron/dua-cli/blob/ebf4cffd611953725ac7819a8f02d0bf7afd1e75/LICENSE), copyright Sebastian Thiel |
| [dua, follow-up review](https://github.com/Byron/dua-cli/tree/48109fe7af6c855dd80435473fdd841717bd16b3) | `48109fe7af6c855dd80435473fdd841717bd16b3` | [MIT](https://github.com/Byron/dua-cli/blob/48109fe7af6c855dd80435473fdd841717bd16b3/LICENSE), copyright Sebastian Thiel |
| [dust](https://github.com/bootandy/dust/tree/8a846f6689f2db6be6ef595239a21ec784d62b57) | `8a846f6689f2db6be6ef595239a21ec784d62b57` | [Apache-2.0](https://github.com/bootandy/dust/blob/8a846f6689f2db6be6ef595239a21ec784d62b57/LICENSE) |

If future work copies or adapts upstream code, preserve the applicable copyright
and license notices. Apache adaptations also need the applicable notices and
modification markings. A design reference is not an exemption from those
obligations. Benchmark binaries retain their own licenses and are not bundled by
the benchmark scripts.

## Kondo: recognize useful work early

Kondo recognizes project manifests and associates them with named artifact
directories. Its iterator stops descending after recognizing a project, returning
the project before all projects have been discovered. The CLI separates discovery
from interaction using a bounded channel with capacity five.

chippytea can use manifest evidence and early publication without adopting
Kondo's full pruning policy. Skipping an entire recognized project would omit
nested projects. Skipping every hidden directory would also omit projects in
authorized hidden worktree folders. Recognize an artifact early, publish its explanation,
and finish measuring its subtree before describing its estimate as complete.
Track excluded, failed, and cancelled coverage separately.

Kondo's sizing sums logical file lengths. That is an opportunity estimate, not
proof of recoverable APFS allocation. chippytea's cleanup requires explicit review
and live revalidation; its optional native Trash route remains recoverable.

Sources: [classification and pruning](https://github.com/tbillington/kondo/blob/1d351ca80b3d3adfad9bbe7db872c27359190210/kondo-lib/src/lib.rs#L370),
[size calculation](https://github.com/tbillington/kondo/blob/1d351ca80b3d3adfad9bbe7db872c27359190210/kondo-lib/src/lib.rs#L505),
[bounded channel and separate workers](https://github.com/tbillington/kondo/blob/1d351ca80b3d3adfad9bbe7db872c27359190210/kondo/src/main.rs#L352).

## dua: metadata batching and compact state

The reviewed dua walker uses a work-stealing pool. Idle workers park rather than
poll. macOS workers consume metadata obtained during native directory enumeration;
other platforms may use separate metadata jobs. Small result chunks expose
parallel work without requiring a whole directory to accumulate before delivery.
The iterator's shutdown stops and joins the workers.

Its macOS implementation uses a 64 KiB aligned `getattrlistbulk` buffer and a
fallback when bulk enumeration is unsupported. It validates returned attributes,
handles entry-specific errors, and falls back to path metadata for cases where
bulk metadata does not satisfy the expected contract. Sharing the parent path
avoids storing a separate complete path for every entry.

These techniques are candidates for measured optimization. A simpler streaming
walker with bounded concurrency is preferable to an unverified native parser.
Any bulk implementation needs tests for packed records, missing attributes,
permission failures, firmlinks, mounts, and unsupported filesystems.

Sources: [worker design and result chunks](https://github.com/Byron/dua-cli/blob/ebf4cffd611953725ac7819a8f02d0bf7afd1e75/crates/dua-lib/src/lib.rs),
[macOS enumeration](https://github.com/Byron/dua-cli/blob/ebf4cffd611953725ac7819a8f02d0bf7afd1e75/crates/dua-lib/src/macos/mod.rs),
[attribute parsing](https://github.com/Byron/dua-cli/blob/ebf4cffd611953725ac7819a8f02d0bf7afd1e75/crates/dua-lib/src/macos/attributes.rs).

dua's retained tree uses 64-byte nodes with 32-bit links and a shared name arena.
That avoids an owned full path and separate edge allocation for every entry.
chippytea should retain candidate summaries and index records, rather than
materializing a million full paths in its interface. Its interactive traversal
also uses a bounded event channel and throttled updates.

For hard links, dua tracks `(device, inode)` and avoids retaining ordinary
single-link files in the same bookkeeping map. Normalize overlapping roots before
using this optimization: a single-link file can otherwise be visited again.
chippytea's reward ledger must deduplicate cleanup operations independently of
scan deduplication.

Sources: [arena representation](https://github.com/Byron/dua-cli/blob/ebf4cffd611953725ac7819a8f02d0bf7afd1e75/src/traverse.rs#L128),
[bounded event channel](https://github.com/Byron/dua-cli/blob/ebf4cffd611953725ac7819a8f02d0bf7afd1e75/src/traverse.rs#L820),
[inode accounting](https://github.com/Byron/dua-cli/blob/ebf4cffd611953725ac7819a8f02d0bf7afd1e75/src/inodefilter.rs).

The reviewed version optionally deduplicates fully shared APFS data streams using
clone metadata. Its documented limitation is partial sharing: files sharing only
some extents are not fully accounted for by that deduplication. Resource forks
also need separate consideration. Clone identifiers are useful for estimates;
they are insufficient proof of space that deletion will return.
Source: [APFS option and limitation](https://github.com/Byron/dua-cli/blob/ebf4cffd611953725ac7819a8f02d0bf7afd1e75/README.md).

## dust: parallel directory iteration and cheap fast paths

dust bridges directory iteration into Rayon, handles ordinary files without an
extra recursive call, and avoids filter work when the relevant filter is empty.
It updates progress through atomics. These are useful examples of keeping the
common metadata path small.

Its implementation retains a complete node tree, then removes duplicate
inode/device entries and recomputes totals. chippytea's million-file memory goal
favors streaming aggregation and bounded index writes instead. Allocated bytes
and apparent bytes are distinct modes; comparisons must select the same one.

Sources: [directory walker and inode pass](https://github.com/bootandy/dust/blob/8a846f6689f2db6be6ef595239a21ec784d62b57/src/dir_walker.rs),
[platform metadata](https://github.com/bootandy/dust/blob/8a846f6689f2db6be6ef595239a21ec784d62b57/src/platform.rs),
[CLI options](https://github.com/bootandy/dust/blob/8a846f6689f2db6be6ef595239a21ec784d62b57/src/cli.rs).

## Names and types before full metadata

### Absolute directory opens

The macOS 14 XNU source supports `O_NOFOLLOW_ANY`, which rejects a symlink at any
component of an absolute lookup. chippytea uses it for directory and evidence-parent
opens after lexical validation, replacing repeated per-component opens on this
platform. It must not be combined with `O_NOFOLLOW` or `O_SYMLINK`; XNU rejects
those combinations. Search-only and readable-directory modes remain distinct.
Overlong paths retain the component-relative implementation. Legacy `/.vol`
spellings also retain that implementation because XNU can translate an absolute
volume-relative path before lookup. Other failures do not fall back to weaker
semantics. Disposable tests cover identity, descriptor ownership, permissions,
long paths, symlinks and cancellation.

Sources: [macOS 14 open implementation](https://github.com/apple-oss-distributions/xnu/blob/xnu-10002.1.13/bsd/vfs/vfs_vnops.c#L391),
[flag contract](https://github.com/apple-oss-distributions/xnu/blob/xnu-10002.1.13/bsd/man/man2/open.2),
[volume-path translation](https://github.com/apple-oss-distributions/xnu/blob/xnu-10002.1.13/bsd/vfs/vfs_syscalls.c#L4274).

### Directory enumeration

The follow-up review on the same date selected a separate enumeration path for
`Suggestions`. Outside Downloads and recognized artifacts, ordinary regular
files cannot become recommendations, so their sizes, timestamps and allocation
need not be fetched. Apple libc already buffers `readdir` through
`__getdirentries64`, enlarging its buffer to amortize calls. Using that reader
avoids introducing another raw-record parser. A known regular file can be
counted without allocating its full path.
Sources: [Apple libc reader](https://github.com/apple-oss-distributions/Libc/blob/main/gen/FreeBSD/readdir.c),
[Darwin directory-entry contract](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/man/man2/getdirentries.2).

Each chippytea discovery step consumes at most 256 entries before returning to
the worker's progress/cancellation checkpoint. A known `DT_DIR` entry is a hint,
not authoritative metadata. Lexical scope and Keep exclusions run first, then
`openat` with `O_DIRECTORY | O_NOFOLLOW` opens the single child name relative to
its parent descriptor. `fstat` supplies the opened directory's metadata. Device,
dataless and cloud-root checks remain, and the same descriptor is retained through
classification and queued traversal. This replaces a separate pathname metadata
lookup followed by opening the same directory. `DT_UNKNOWN` retains
descriptor-relative, no-follow metadata and the subsequent open-and-compare
identity check; it is never assumed to mean a file. The depth limit remains after
classification and before descent, preserving artifact recognition at that
boundary. Symlinks outside recognized artifacts are skipped. Final
directory-identity checks and queued events detect changing coverage; errors
remain partial. Nested and hidden projects remain within discovery's scope.
Source: [Apple descriptor-relative open and no-follow flags](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/man/man2/open.2).

Apple libc's `fdopendir` immediately reads the first directory batch. chippytea
therefore retains an owned descriptor without constructing `DIR` until names
enumeration is requested. It rechecks the captured identity before that first
read and transfers descriptor ownership only after successful `fdopendir`.
Directories discarded during classification need no unused prefetch; directories
actually traversed still need their first read. Full bulk enumeration uses its
own untouched descriptor, without initializing a libc reader. Unsupported bulk
enumeration opens `.` through the retained descriptor to obtain a fresh file
description, checks its identity, then initializes the fallback reader. This
avoids sharing an advanced offset between `readdir` and `getattrlistbulk`. These
changes remove redundant work by construction; measured gains are reported
separately.
Sources: [Apple libc initialization](https://github.com/apple-oss-distributions/Libc/blob/main/gen/FreeBSD/opendir.c#L70),
[initial batch read](https://github.com/apple-oss-distributions/Libc/blob/main/gen/FreeBSD/opendir.c#L370),
[Apple bulk enumeration contract](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/man/man2/getattrlistbulk.2#L196).

Downloads and artifact measurements retain the full metadata path, including
the validated 64 KiB bulk reader and its no-follow fallbacks. Only completed
eligible measurements authorize review; cleanup repeats the full safety checks.
`MetadataCoverage` remains explicit and exhaustive within its policy boundaries.
Names enumerated without metadata contribute no invented byte totals. Reporting
must distinguish examined names, metadata omitted, excluded artifact contents
and incomplete coverage.

A reduced `getattrlistbulk` mask remains an alternative to compare:
`NAME | RETURNED_ATTRS | OBJTYPE | FILEID | ERROR`, without file, directory or
fork attributes. XNU identifies these as directory-listing attributes; requesting
other metadata additionally requires search access. Missing types still need a
no-follow fallback. Bulk attributes can describe an underlying mountpoint or
firmlink, so they cannot replace authoritative directory-boundary checks.
`readdir` and `getattrlistbulk` must not share an advanced enumeration offset;
fallback uses a fresh open file description.
Sources: [XNU listing subset](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/vfs/vfs_attrlist.c#L3962),
[Apple bulk API](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/man/man2/getattrlistbulk.2),
[dua's minimal and full masks](https://github.com/Byron/dua-cli/blob/48109fe7af6c855dd80435473fdd841717bd16b3/crates/dua-lib/src/macos/attributes.rs#L26).

An isolated 8 KiB minimal-mask prototype was slower than libc names on both
audited fixture topologies; its measurements are in [PERFORMANCE.md](PERFORMANCE.md#historical-directory-traversal-follow-up).
It is not part of the engine. Fewer public API calls do not imply fewer kernel
operations: libc buffers names and can receive an EOF flag with its initial read,
whereas bulk enumeration requires a later zero-result call. XNU can also emulate
bulk through per-child pathname and attribute operations when the filesystem
does not implement it directly. A future minimal reader would need separate
performance and protected-path access evidence, not just a smaller attribute mask.
Source: [XNU bulk fallback](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/vfs/vfs_attrlist.c).

Apple TN3150 warns that enumeration can materialize dataless directories and
that path-based metadata calls can materialize intermediate folders. chippytea
therefore also uses a scoped thread policy:
`setiopolicy_np(IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES, IOPOL_SCOPE_THREAD,
IOPOL_MATERIALIZE_DATALESS_FILES_OFF)`. The guard restores the previous setting
and cannot move between threads. Failure to establish the policy stops the
operation; `EDEADLK` is handled as unavailable data, never retried by allowing a
download. This supplements the existing cloud and descriptor checks.
Source: [Apple TN3150](https://developer.apple.com/documentation/technotes/tn3150-getting-ready-for-data-less-files).

The chosen first change keeps one discovery worker. Current dua distributes
directory jobs across parked workers, while native metadata remains on the
enumerating worker. Its completion mode streams four-entry batches; its
parent-first mode collects a directory before publishing, which does not meet
chippytea's bounded-memory approach for wide directories. Kondo's project
recognition and dust's regular-file fast path support reducing unnecessary work;
neither establishes the best thread count for this app on APFS.
Sources: [current dua scheduling](https://github.com/Byron/dua-cli/blob/48109fe7af6c855dd80435473fdd841717bd16b3/crates/dua-lib/src/lib.rs#L881),
[Kondo name/type recognition](https://github.com/tbillington/kondo/blob/1d351ca80b3d3adfad9bbe7db872c27359190210/kondo-lib/src/lib.rs#L352),
[dust's file fast path](https://github.com/bootandy/dust/blob/8a846f6689f2db6be6ef595239a21ec784d62b57/src/dir_walker.rs#L186).

Any later concurrency change needs equal-coverage comparisons of one, two and
four workers, measuring first findings, total CPU, memory and UI responsiveness.
It also needs bounded job/descriptor counts, overflow that continues local work,
and a cleanup barrier that parks every worker. Adding threads to the current
single-worker pause flag is unsafe. No speedup or optimal thread count is claimed
by this source study; controlled measurements remain separate.

## APFS: estimates, observations, and reward bounds

Logical size, allocated size, file-private size, and observed available-space
change answer different questions. Neither `st_size` nor `st_blocks * 512`
proves that an unlink will return that many bytes. Hard links, APFS clones,
snapshots, and still-open file descriptors can retain storage.

Apple documents `ATTR_CMNEXT_PRIVATESIZE` as an `off_t` describing storage outside
clone/snapshot retention that deletion would release. Query it immediately before
an eligible permanent cleanup using an already verified descriptor. Attribute
absence or failure must not silently become an allocated-size fallback for
rewards. The current SDK defines this extended attribute as `0x8` and
`FSOPT_ATTR_CMN_EXTENDED` as `0x20`.

A minimal `fgetattrlist` request sets only `ATTR_CMN_RETURNED_ATTRS` in
`commonattr` and `ATTR_CMNEXT_PRIVATESIZE` in `forkattr`. The 24-byte `attrlist`
contains two 16-bit fields followed by five 32-bit masks. Its response has a
32-bit length, five 32-bit returned masks, and a signed 64-bit private size.
Parse native-endian bytes, validate the returned masks and length, and reject
negative sizes. Do not assume Rust's natural struct alignment matches the
four-byte packing of attribute results.

Sources: [Apple `getattrlist(2)`](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/man/man2/getattrlist.2#L1239),
[Apple attribute structures and constants](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/sys/attr.h).

A conservative reward bound is the minimum of the successfully deleted eligible
files' private bytes, their allocated-byte bound, and the positive observed
available-space change. Require final-link handling, close unlinked descriptors
before the observation, and consume each observation in exactly one persisted
cleanup transaction. Trash receives zero credit.

This is still sampled evidence: a concurrent process can create a clone or
snapshot between the private-size query and unlink, or change available space
independently. Do not describe a free-space delta as absolute causal proof. If
attribution is uncertain, retain the cleanup result and credit zero. Coins should
use integer bytes: one coin per 100,000,000 credited bytes, with the remainder
persisted transactionally.

An APFS volume identifier is not a storage-pool identifier. Multiple volumes can
share an APFS container. Obtain the mountpoint using `fstatfs`; unprivileged
`diskutil info -plist <mountpoint>` reports `APFSContainerReference`, and
`diskutil apfs list -plist` maps that reference to `APFSContainerUUID`. Serialize
accounting by that container, not by a transient `diskN` name. Query these outside
the scan hot path and outside the main thread. The installed `diskutil(8)` manual
documents plist output and the APFS hierarchy. These two read-only commands were
verified to work without elevated privileges on the development Mac.

Use ordinary available-space observations for recovery accounting. Capacity for
"important usage" is intended for storage admission decisions and should not be
treated as a measured cleanup result.
Source: [Apple capacity queries](https://developer.apple.com/documentation/foundation/checking-volume-storage-capacity).

The local SDK also exposes `SF_DATALESS`, `EF_IS_SYNC_ROOT`, and
`EF_IS_PURGEABLE`. Cloud/provider scope requires additional policy beyond a single
flag; metadata discovery must not materialize dataless content. Listing snapshots
is not a necessary condition for using private-size metadata. Apple's snapshot
manual documents privileged/entitled snapshot operations; chippytea should not
request elevated access merely to award rewards.
Sources: [Apple file flags](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/sys/stat.h),
[snapshot API](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/man/man2/fs_snapshot_create.2).

## Persistent index and native Trash

Use FSEvents as invalidation evidence, not a complete mutation journal. Start
watching before the initial scan and retain events received while that scan is
running. Refresh affected subtrees after the baseline finishes. Commit index
changes and the consumed event cursor together. Coalesce dirty subtrees and let
the app sleep when no work remains.

`MustScanSubDirs` requires recursive refresh. Dropped events require rescanning
the monitored roots. Root changes, event-ID wrap, or invalid volume/cursor identity
invalidate coverage. Never silently discard events on queue overflow. A bounded
queue can replace overflow with one durable "root dirty" record.
Source: [Apple FSEvents guide](https://developer.apple.com/library/archive/documentation/Darwin/Conceptual/FSEvents_ProgGuide/UsingtheFSEventsFramework/UsingtheFSEventsFramework.html).

For recoverable cleanup, call
`FileManager.trashItem(at:resultingItemURL:)`. Persist the returned URL because
macOS can rename the item in Trash. Record the original destination and fresh
identity information. Restoration must refuse an occupied destination or an
identity mismatch. Treat an interruption between the native operation and its
receipt as an unresolved operation to reconcile, not as permission to repeat it.
Source: [Apple Trash API](https://developer.apple.com/documentation/foundation/filemanager/trashitem(at:resultingitemurl:)).

## Reproducible measurements

The fixture generator only creates a new directory. Existing paths, including
empty directories and dangling symlinks, are refused. It leaves incomplete
fixtures marked as incomplete and never deletes them. It checks free space before
creation and during generation, retaining a reserve of at least 3 GiB. The
default uses 10,000 payload files; million mode uses zero-byte payloads so the
scanner's metadata cost is measured without creating a large data payload.

```sh
python3 scripts/make-fixture.py benchmarks/local/fixture-10k
cargo build --release --locked --bin chippytea-cli
python3 scripts/benchmark.py benchmarks/local/fixture-10k --warm-runs 20

# Optional larger experiment: creates 1,000,000 payload files plus project manifests.
python3 scripts/make-fixture.py benchmarks/local/fixture-million --million
python3 scripts/benchmark.py benchmarks/local/fixture-million --skip-discovery --warm-runs 5
```

The root marker records exact counts. Timed runs scan only `baseline/`, which
contains independent regular files and developer manifests. Hard-link, symlink,
and sparse-file examples are under `cases/` and excluded from baseline timing.
They require separate correctness measurements; they do not establish clone or
snapshot safety by themselves. The marker records files, directory counts with
and without the root, logical bytes, and the generator hash.

The harness uses `/usr/bin/time -l`, normalizes Darwin peak RSS as bytes, records
user/system CPU times, and stores raw stdout/stderr. Hardware collection is limited
to model, chip, core counts, memory, architecture, and OS version/build; it does
not collect hardware serials or UUIDs. The CLI binary hash and Git HEAD identify
the measured build; a Git HEAD alone does not identify uncommitted source.

`du` is mandatory. `dua` and `dust` are used only when already installed; missing
tools are reported. Exact versions and commands are saved. Upstream source pins
above document the study; an installed binary can be a different version and
must not be mislabeled as that pin. Unsupported flags fail visibly rather than
silently changing the workload.

Equivalent allocated-byte traversal commands are:

```sh
target/release/chippytea-cli traverse "$BASELINE"
/usr/bin/du -s -k -x "$BASELINE"
dua aggregate --threads 4 --stay-on-filesystem --format bytes --no-sort --stats "$BASELINE"
dust --threads 4 --config /dev/null --limit-filesystem --no-progress \
  --no-colors --no-percent-bars --depth 1 --output-format b "$BASELINE"
```

Use identical roots, hidden-entry policy, filesystem boundaries, symlink policy,
and allocated-byte semantics. Output depth affects dust's display, not complete
subtree traversal. A separate post-run streaming metadata audit verifies the
fixture against its marker. chippytea's raw traversal counts are checked against
those counts; baseline tools' lack of comparable entry counters is reported.

Candidate discovery is timed separately with `chippytea-cli scan`; first findings
are observed at the first nonempty JSON batch received by the harness. Kondo's
`kondo --dry-run --same-filesystem "$BASELINE"` is a useful optional discovery
comparison, but it has a different coverage contract and logical-size accounting,
so it is deliberately excluded from the equivalent-traversal table. Never use
Kondo's cleaning flags for a benchmark.

First-run and warm measurements are distinct. Fixture creation and earlier tools
populate filesystem caches, so **first run does not mean cold cache**. This
harness does not purge system caches, reboot, or claim cold-cache measurements.
A separate controlled cold-cache experiment needs its own recorded procedure.
The warm p95 uses nearest-rank calculation; use enough repetitions before making
a percentile claim, and report sample count and all failures.

The resulting `benchmarks/local/<timestamp>/summary.json` and `REPORT.md` contain
measurements only after execution. They are local artifacts, not fabricated
reference results. Native window latency, UI responsiveness, idle CPU, and
cancellation need their separate app/safety harnesses. CLI traversal timings do
not prove those targets. A missing result remains unmeasured.
