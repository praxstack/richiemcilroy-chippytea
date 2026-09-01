# Performance

Reproducible performance comparisons below use generated, disposable fixtures. Personal library paths, inventory totals, storage estimates and wallet balances are omitted. Raw local diagnostics remain private and are not publication artifacts.

Measured 31 August and 1 September 2026 on an Apple M4 Max MacBook Pro (`Mac16,6`), 16 CPU cores, 128 GiB memory, arm64, macOS 27.0 build `26A5421a`, local APFS. The workstation was also in normal use; these are reproducible local measurements, not a hardware-wide promise.

## Shared Bun workspaces and safer discovery

The 1 September scanner update reuses compact Bun workspace ownership facts instead of repeatedly normalizing and parsing the same shared lockfile. The cache holds at most eight entries and 1 MiB of retained capacity. It contains workspace keys and optional names, not dependency trees or permission to delete. Current manifest names, captured file identities, workspace declarations and configuration are still checked; cleanup uses uncached revalidation. The first parse still allocates the normalized input and JSON tree, so the retention cap is not a peak-memory limit.

Two generated workspaces each contain one independently allocated, eligible 100 MiB artifact; all fixture modification times are nine days old. Each comparison has one separate first pair and five alternating warm pairs. All **24 scans** passed independent before/after fixture audits and returned identical eligible paths, identities, fingerprints, sizes and permanent-cleanup eligibility.

| Bun fixture | Warm elapsed median, before → after | Warm CPU median, before → after | Maximum process RSS, before → after |
| --- | ---: | ---: | ---: |
| 128 members, 1 MiB lock | 430.987 → 34.844 ms | 420 → 20 ms | 9.078 → 8.094 MiB |
| 512 members, 4 MiB lock | 6,564.901 → 103.203 ms | 6,540 → 90 ms | 21.688 → 21.531 MiB |

The larger case is **63.6× faster** and uses **98.6% less median CPU**. Its first eligible result moved from 2,423.337 to 57.083 ms at warm p95. Large-case peak memory is essentially unchanged; the gain comes from avoiding repeated work, not eliminating the first parse. Warm elapsed p95 was 435.262 → 35.617 ms for 128 members and 6,579.233 → 111.449 ms for 512. With five samples, nearest-rank p95 is the maximum, not a reliable population-tail estimate.

Timings cover the whole CLI process; CPU is user plus system time reported by `/usr/bin/time -l`, with its coarse resolution. RSS is the largest process-lifetime high-water mark across the six invocations. Fixture creation and audits warm filesystem caches. First invocations are not cold-cache measurements: the 128-member first pair was 449.010 → 281.058 ms, and the 512-member pair was 6,628.601 → 110.969 ms. Concurrent release/site compilation was paused for these timed batches.

Unrelated workload controls also required exact recommendation equivalence:

| Control | Warm elapsed median, before → after | Interpretation |
| --- | ---: | --- |
| One million ordinary source files plus an eligible artifact | 334.608 → 338.328 ms | Essentially unchanged; names-first discovery, not exhaustive metadata inventory |
| Directory-heavy tree with 200,000 ordinary source files | 934.341 → 926.912 ms | Essentially unchanged |
| 512-member npm workspace, first five warm pairs | 99.671 → 109.586 ms | Slower in this series |
| Same npm workspace, additional twelve warm pairs | 109.826 → 107.575 ms | Similar medians; 100 ms median CPU for both |

The npm repeat had elapsed p95 of 116.199 → 127.907 ms. These variable results do not establish an npm speedup or a general regression-free guarantee. The million-file control used 3.234 → 3.219 MiB maximum CLI RSS. No whole-Home or cross-tool speed claim follows from these fixtures.

Discovery also fixes two correctness gaps. Recent, unmarked folders named `.venv`, `venv`, `.next`, `.nuxt`, `.turbo` or `.parcel-cache` no longer hide nested projects. A kept descendant excludes its enclosing recognized cleanup unit before measurement and again during presentation, review and execution, without hiding unrelated siblings. Rules version 8 rebuilds the derived index once while preserving Keep, history and rewards.

```sh
python3 scripts/make-workspace-fixture.py /private/tmp/chippytea-bun-512 \
  --lock-format bun --members 512 --lock-kib 4096 --age-days 9
python3 scripts/benchmark-suggestions.py /private/tmp/chippytea-bun-512 \
  --baseline-cli /path/to/before/chippytea-cli \
  --candidate-cli target/release/chippytea-cli --warm-runs 5 \
  --output benchmarks/local/bun-512-comparison
```

The fixture and output paths must be new; existing directories are refused. For the smaller case use 128 members and 1,024 KiB. Omit `--lock-format bun` to create the npm control. The OSS review supported retaining bounded, descriptor-relative traversal; see the pinned references in [REFERENCE-STUDY.md](REFERENCE-STUDY.md) and the [ncdu 2.9.2 source](https://dev.yorhel.nl/download/ncdu-2.9.2.tar.gz). These measurements do not compare Chippytea with those tools.

### Typed Rust snapshot responses

Native snapshot requests now serialize the typed snapshot directly into the JSON envelope instead of constructing an intermediate generic JSON value tree. The public Rust request API and JSON field semantics are unchanged. A new synthetic FFI harness measures the real `ct_request` and `ct_free_string` calls, including byte-for-byte validation of every timed response.

| Synthetic snapshot, each with 100 receipts | Median elapsed per call, before → after | Median CPU per call, before → after | Maximum lifetime process RSS, before → after |
| --- | ---: | ---: | ---: |
| 0 candidates | 0.202 → 0.114 ms | 0.202 → 0.114 ms | 6.063 → 5.875 MiB |
| 500 candidates | 1.295 → 0.457 ms | 1.294 → 0.457 ms | 15.344 → 14.391 MiB |

The populated case used **64.7% less CPU**. Each shape has six alternating measured pairs of 200 calls per process, plus an omitted warm-up pair. All 5,600 loop responses, including warm-ups, matched a validated successful reference. Parsed before/after responses matched across builds, including large integers, escaped text, nulls and omitted fields; every private database remained logically unchanged. Seeding, opening, parsed JSON comparisons and database audits are outside the timers. Timing includes C-string access, response comparison and validation counting. RSS includes setup, initial validation and one retained raw reference; it is not live allocation or native-app memory. This measures neither Swift decoding nor UI latency.

```sh
mkdir -p benchmarks/local
python3 scripts/benchmark-snapshot-ffi.py build \
  --rlib /path/to/before/libchippytea_core.rlib \
  --dependencies /path/to/before/deps --output benchmarks/local/snapshot-before
python3 scripts/benchmark-snapshot-ffi.py build \
  --rlib target/release/libchippytea_core.rlib \
  --dependencies target/release/deps --output benchmarks/local/snapshot-after
python3 scripts/benchmark-snapshot-ffi.py compare \
  --before benchmarks/local/snapshot-before/driver \
  --after benchmarks/local/snapshot-after/driver \
  --iterations 200 --rounds 6 --output benchmarks/local/snapshot-comparison
```

### Integrated verification and limits

The frozen core passed **330 Rust tests**, formatting and Clippy with warnings denied. Both disposable native suites passed: Trash/restore/permanent cleanup/restart and the complete access/interaction suite. An isolated app using the existing native source and this final core completed one visible Bun512 rescan in **253.654 ms**, using **182.839 ms process CPU** and **122.781 MiB lifetime peak RSS**. It verified all 1,541 entries, the expected eligible artifact and an unchanged ledger. The endpoint is main-actor snapshot delivery before final rendering, not frame latency; this is one warm integration observation, not a native before/after or p95 result. It does not validate the separate updater/release build.

Forty final-CLI cancellation samples passed: twenty on Bun512 and twenty on the million-source-file fixture, requesting cancellation after 40 ms. Whole-process wall time minus that requested delay was at most **9.335 ms** and **8.845 ms**, respectively. Those are upper bounds including startup, timer scheduling and shutdown, not timestamps of the cancellation flag or native-button latency.

A packed metadata-fingerprint update was also tested and **not retained**: on the exhaustive million-entry artifact fixture, median wall time was 2,058.446 → 2,064.543 ms and median CPU was 2.05 s for both. Exact fingerprints matched, but there was no demonstrated gain.

Private raw timings, fixture audits, source manifests and native logs are under `benchmarks/local/scan-performance-20260901/`. The Bun reports are `bun-comparison-128` and `bun-comparison-512`; controls are `control-*`, and FFI evidence is `ffi-snapshot-probe/comparison-v2`. The rejected FFI protocol-1 probe was never used for results; protocol 2 validates every timed response, not only endpoints.

- Before CLI SHA-256: `3c0c52273fce70c9ec0fa7035d35bef23ec1fa384c943730fd1650503c29ca83`.
- After CLI SHA-256: `5d448ac17198417423aeafb3c67df0e37e60d75f51a835601872ec782848f7e0`.
- Before/after Rust libraries and all harness hashes are recorded in the private build and comparison manifests.

## Immediate background cleanup

Confirmation closes review and returns to the stash while cleanup continues behind a small progress ring. A positive permanent-cleanup estimate starts the full chip flight, rising counter and ascending sound. The estimate is marked pending and never enters the wallet or history. After this preview is shown, verified credit settles quietly in the same window session without a second flight, sound or counter restart. A lower verified result corrects the estimate. Mute and Reduce Motion remain supported.

Permanent cleanup has one confirmation explaining the selected items and consequences. Its optional “Don't ask again” choice applies to future deliberate cleanup actions for eligible artifacts; Settings can restore confirmation. Inspection and screenshot staging remain presentation-only, even with that preference disabled. Every deletion still passes the engine's preparation and identity checks. Trash earns no chips.

The menu bar independently shows current free capacity from one off-main `statfs` query on the startup Data volume. It refreshes at startup, later window openings and cleanup completion, with a 60-second periodic refresh and 15 seconds of timer leeway. One query can be in flight, with one coalesced follow-up; changed results update the menu item. This reads volume counters without scanning files or establishing recovery credit. See [architecture](ARCHITECTURE.md) for capacity semantics.

One disposable native integration run on 1 September 2026 used a generated Rust target containing 4,096 small leaves and an independently written 100 MiB payload:

| Measurement | Observed result |
| --- | ---: |
| Synchronous model acknowledgment | 0.0381 ms |
| Confirmation through cleanup and final snapshot settling | 0.9504 s |
| Native process CPU during that interval | 0.5892 s |
| Maximum main-actor heartbeat gap | 25.101 ms |
| Confirmed storage credit | 121,638,912 bytes, earning 1 chip |

Acknowledgment times the `clean(permanently:)` call returning with pending presentation state; rendered-frame latency was not measured. The heartbeat requested 20 ms sleeps and recorded 40 samples during cleanup. CPU is `RUSAGE_SELF`, excluding child processes. These are single-run integration measurements, not p95 results or evidence of instant filesystem deletion.

The complete access-flow suite passed on this binary. The cleanup regression checked rejected preparation, cancellation before execution, recovery after two failed final snapshot reads, stale-snapshot rejection, retained feedback after fast completion and preserved pending rewards on another tab. A held older collection could not lose the newer collection demand or lower the displayed balance. The real receipt earned one chip and settled with zero additional earned presentations, exercising the quiet handoff. Reopening and repeated collection did not duplicate credit. Confirmation and sound preferences were absent before and after the suite.

Injected storage tests also passed checked counter conversion, off-main reads, coalesced refreshes and rejection of callbacks from a stopped monitor. They do not measure real capacity-query cost or idle CPU.

A separate native GUI check used an isolated library and generated artifacts. It verified the single confirmation, remembered opt-out, Settings toggle, immediate chip flight, progress ring, reduced-motion presentation and Stop control. One cleanup earned one chip; cancelling the second during removal restored the displayed balance without awarding credit. All five source and outside control files retained their original hashes. Captured screen states establish appearance and interaction, not frame latency or audio quality. Private evidence is in `benchmarks/local/background-cleanup/instant-ui/`.

Run the focused disposable regression after building the app:

```sh
build/Chippytea.app/Contents/MacOS/Chippytea \
  --access-flow-test --background-cleanup-regression
```

Private evidence: `benchmarks/local/background-cleanup/instant-access-flow-v2.log`, `benchmarks/local/background-cleanup/instant-access-flow-v2-result.json` and `benchmarks/local/background-cleanup/instant-final-v2/build-result.json`. The measured native executable SHA-256 is `9d4f7e6b8f73328f72eb75d379ed5e2cbfb3227858b58aeb15f131e820c412cc`.

## Permanent cleanup throughput

Permanent cleanup now reads its committed identity manifest within one scoped SQLite read transaction and decodes each identity directly from borrowed text. For captured, single-link regular files, one descriptor-relative metadata query obtains private allocation without an additional open, descriptor stat and close. Full identity checks before and after that query remain mandatory. Unsupported metadata uses the previous descriptor path; a proven mismatch preserves the captured file. Internally linked files retain their descriptor and link-transition checks. See [architecture](ARCHITECTURE.md) for these safety boundaries.

The comparison times the real `cleanup::execute_with_progress` operation, including revalidation, staging, removal and durable accounting. Every invocation generates a new private fixture containing small leaves and an independently written 100 MiB payload. It accepts no existing cleanup target. One separate first pair precedes alternating warm pairs: three pairs for each 16,384-leaf case and two for the 131,072-leaf case. All **22 invocations** passed.

| Fixture | Entries in each validation and removal pass | Warm elapsed median, before → after | Warm CPU median, before → after |
| --- | ---: | ---: | ---: |
| 16,384 regular leaves | 16,452 | 2.496 → 2.118 s | 2.147 → 1.794 s |
| 131,072 regular leaves | 131,588 | 16.652 → 15.073 s | 16.193 → 14.560 s |
| 16,384 leaves in 8,192 internal link groups | 16,452 | 4.107 → 4.148 s | 3.770 → 3.815 s |

The ordinary-file cases used **15.1% and 9.5% less elapsed time**, with **16.5% and 10.1% less CPU**. The linked control was **1.0% slower** and used **1.2% more CPU**; this change does not demonstrate a benefit for that workload. Large cleanups still take seconds: the larger updated fixture spent 14.013 seconds of its median operation in removal. Renaming each captured leaf, rechecking its identity and unlinking it remain real filesystem work.

CPU is `RUSAGE_SELF`, excluding the `diskutil` child used for storage-domain discovery. RSS in the raw reports is lifetime process memory, including fixture generation and audits. Setup warms the filesystem cache, so no cold-cache or p95 acceptance claim follows from these small samples. The `staged_checking` phase includes recovery-directory setup and the initial capacity-sampling window, not just verification. These timings exclude Swift, FFI, filesystem-event handling and UI work.

Independent audits require identical per-case file counts and inode-deduplicated allocation, complete removal of the intended artifact, preserved source/sibling/outside sentinels, a cleared removal manifest and conservative receipts. Retrying the completed cleanup, reopening the store and collecting again must not duplicate credit. A separate cancellation regression interrupts after an unlink and checks that partial recovery evidence survives restart with zero coins.

```sh
python3 scripts/benchmark-cleanup.py \
  --before-rlib /path/to/before/libchippytea_core.rlib \
  --after-rlib target/release/libchippytea_core.rlib \
  --cases regular hardlinks --leaves 16384 --warm-runs 3 \
  --output benchmarks/local/cleanup-comparison
python3 scripts/benchmark-cleanup.py \
  --before-rlib /path/to/before/libchippytea_core.rlib \
  --after-rlib target/release/libchippytea_core.rlib \
  --cases regular --leaves 131072 --warm-runs 2 \
  --output benchmarks/local/cleanup-large-comparison
```

Keep the matching dependency artifacts for both release libraries in the supplied `--deps-dir` (default `target/release/deps`). The harness retains generated fixtures and all evidence; it never accepts a user directory for deletion.

- Before rlib: `e47a7a946bd3a97ef504da1dd62c0c4d394b5639f7f766b55d9aecbe70e6c414`.
- After rlib: `86d4c8d7d2830d1cb5bb33b7b6186b824f2aefaf3af86ec11b48bd0247c77748`.
- Harness: Python `9e8cff793e329c8118c8d8d8834be761526ab8b89ed4eafcf481606a0d4c81b8`, Rust `62a411f1326d7f2201745ccda35cc6e0111c827abe113f98006d66334aad5556`.
- Private frozen sources, raw phase timings, audits and validation logs: `benchmarks/local/cleanup-throughput/`; the table uses `comparison-16k` and `comparison-131k`.

### Native cleanup notifications

Cleanup creates internal staging events as it safely removes each file. The native watcher now discards those path payloads before enqueueing engine work and combines their cursor acknowledgments using one flush scheduled after 100 ms. Ordinary changes and history-loss signals retain immediate delivery. The delay depends on availability of the existing serial event queue; this is not a hard deadline or an absolute downstream queue bound.

An isolated native observer compared the old and current watcher while the same Rust cleanup driver removed fresh 4,096-leaf fixtures. It retained delivered arrays through cleanup and a quiet tail, and required ordinary file controls before and after cleanup. The first completed pair recorded:

| Observer measurement | Before | After |
| --- | ---: | ---: |
| Delivered batches | 278 | 8 |
| Retained paths | 8,220 | 3 |
| Lifetime peak observer RSS | 16.875 MiB | 14.734 MiB |
| Observer CPU | 9.967 ms | 36.942 ms |

These are descriptive observations from one pair, not an accepted aggregate benchmark. The series stopped at its sixth invocation when the watcher reported lost event history; five invocations had passed. All six underlying cleanup operations passed their fixture, sentinel and accounting audits. RSS is the lifetime high-water mark through the retention endpoint, including observer setup and excluding its Rust child. The observer excludes the app model, engine event processing and UI. Filtering reduced retained data and downstream submissions, but increased CPU inside this isolated observer. No whole-app CPU or cleanup-throughput improvement is claimed from this experiment.

```sh
python3 scripts/benchmark-cleanup-events.py \
  --before-services /path/to/before/NativeServices.swift \
  --after-services native/Chippytea/NativeServices.swift \
  --leaves 4096 --warm-runs 3 \
  --output benchmarks/local/cleanup-events-comparison
```

Build the current release Rust library before running this comparison. The harness uses it for both variants, generates its own disposable fixtures and preserves failed runs. Private evidence is in `benchmarks/local/cleanup-throughput/watcher-comparison-4k`. Earlier rejected 16,384-leaf comparisons are also retained. A separate instrumented diagnostic captured `MustScanSubDirs | UserDropped` on the baseline, establishing client-side event-history loss for that invocation; it does not establish the cause of every rejected run.

- Current watcher: `624661a882a219580a14aa4ee2b92c66ee9fb2073afefa560cf76ee2a7d1a162`.
- Observer harness: Python `813e7d3f7ff2c0595b65e4722439a921c5554e6cce8750ff4fa72d0e22a4bbae`, Swift `3f027eb5f79ad3a0001c300f81b52e9c86a45d77ff0a5a997de083b7522b8144`.

The Rust suite passed 303 tests, followed by focused parser checks after the final unsupported-record fallback adjustment. Clippy passed with warnings denied. The final signed native build passed the complete access and interaction suite, including cursor coalescing, loss barriers, real filesystem events, restart, immediate cleanup feedback and preservation of unrelated files. That single integration run measured a 0.061 ms cleanup acknowledgment and a 25.107 ms maximum main-actor heartbeat gap. These are integration observations, not p95 latency results. The disposable native cleanup suite also passed Trash, restore, permanent removal and reward replay with the same final Rust archive.

## Remembering where an artifact proved too recent

An old artifact directory can contain one recently edited file deep inside it. Repeated full scans previously walked down to that file again, even after discovery had established why the artifact was ineligible. Suggestions scans now retain a bounded pool of file paths learned during successful measurement or received through file events. Each use repeats the current identity, ancestry, authorization, local-file and age checks. A stale path falls back to normal traversal. No exclusion decision, allocation total or cleanup proof is cached.

Each scan works on a local copy of at most 64 hints and 64 KiB of path data. It can return that copy only after complete, error-free, uncancelled finalization and only if the runtime revision is unchanged. Events, new Scan requests and permission or Keep changes prevent older work from replacing newer state. This deliberately loses learning when events arrive during a long scope; frequent Home activity can therefore reduce the gain. Exhaustive metadata scans and cleanup do not use the shortcut.

The comparison uses the public Rust Engine API and an aged fixture with 32,768 generated leaves, a 100 MiB payload and an independent eligible 100 MiB control. Each invocation starts with a fresh library, writes one byte in the selected existing leaf, and performs an untimed full suggestion scan to learn its location. The next full suggestion scan is timed. No filesystem event is submitted. The control ages the learned file before timing without sending an event, requiring full fallback traversal and an eligible result.

All **56 invocations** passed: one separate first pair and six alternating warm pairs for each case. The table reports warm medians and nearest-rank p95, which is the maximum of these six samples. RSS is maximum lifetime Rust process memory across all seven invocations per build, including initial and learning scans. Inputs and audits warm caches; none of these runs is described as cold-cache.

| Recent-file position | Repeated-scan traversal entries, before → after | CPU median, before → after | Elapsed median, before → after | Elapsed p95, before → after | Peak process RSS, before → after |
| --- | ---: | ---: | ---: | ---: | ---: |
| First | 14 → 10 | 3.958 → 3.536 ms | 6.505 → 6.532 ms | 6.603 → 6.563 ms | 5.328 → 5.328 MiB |
| Middle | 16,460 → 10 | 28.874 → 3.986 ms | 31.894 → 6.523 ms | 38.131 → 6.549 ms | 5.359 → 5.375 MiB |
| Last | 32,908 → 10 | 53.330 → 4.109 ms | 56.266 → 6.049 ms | 63.819 → 6.567 ms | 5.563 → 5.406 MiB |
| Learned file aged before timing | 32,909 → 32,909 | 54.305 → 53.087 ms | 56.714 → 56.599 ms | 63.831 → 57.016 ms | 5.391 → 5.563 MiB |

Middle and last cases used **86.2% and 92.3% less median CPU**, improving in all six paired samples. Their median elapsed times fell by 79.5% and 89.2%. First-file elapsed time was effectively flat. The aged control also remained effectively flat; its first pair was slower after the change, at 54.764 → 57.002 ms. This is not evidence of faster full traversal.

Both builds still visited 14, 16,460 and 32,908 entries during the respective untimed learning scans; the aged control learned at the middle position. The gain applies to the subsequent scan. Ten traversal entries do not mean ten filesystem operations: the fresh leaf and ancestor probes are included in CPU and elapsed time but not that traversal counter. Timings also include the Scan request, durable journal and foreground-summary writes, scheduling and 5 ms snapshot polling. They exclude input preparation, learning, aging, Python audits and subsequent exhaustive verification. This does not measure native UI, idle CPU, window latency or whole-Home speed.

Every initial and final exhaustive scan checked **32,909 entries, 32,774 files and 135 directories**. All 14 aged timed endpoints matched those counts and the full current candidate proofs. Across all 224 captured stages, the eligible control remained identical, authorization and saved foreground state were valid, journals drained and the accounting ledger remained zero. Independent physical audits verified the selected file's identity, exact one-byte change, timestamps, allocation and all other files. Fixtures and libraries are retained.

```sh
python3 scripts/benchmark-recent-files.py \
  --before-rlib /path/to/before/libchippytea_core.rlib \
  --after-rlib target/release/libchippytea_core.rlib \
  --workload rescan --leaves 32768 --warm-runs 6 \
  --output benchmarks/local/repeated-scan-comparison
```

- Before rlib: `a787b9ef0ee8ccf3fbe0bb6f6835ec987b20e3d5e3d143b8f58f83e4df5806d7`.
- After rlib: `e47a7a946bd3a97ef504da1dd62c0c4d394b5639f7f766b55d9aecbe70e6c414`.
- Harness: Python `288288ac527c9fea814e537dcebeb4af141adf4627b3b27928a969dd7473e71b`, Rust `bbfbdcd43cfcdd456eded4997c4717fdf76a4cb2f2fd0294358a1a588842c32e`.
- Private frozen inputs, raw records, physical audits and validation logs: `benchmarks/local/repeated-scan-work/`.

A separate 16-invocation comparison of the default event protocol on a 256-leaf fixture passed. The 16-invocation unchanged-engine rescan smoke test also passed; neither small-fixture series supplies the performance claims above. All 299 Rust tests passed. The partial-finalization test was then strengthened to reject a swallowed observer panic, and its focused rerun passed. Final-source formatting, Clippy with warnings denied, the stable signed native build and both disposable native integration suites passed. Native permanent cleanup credited 276,828,160 bytes and two coins with replay/restart checks; Trash earned zero. The access suite observed 0.052 ms cleanup acknowledgment and a 25.099 ms maximum main-actor heartbeat gap in one run. The SwiftUI views and artwork are unchanged.

## Reusing identical native snapshot responses

The native bridge previously decoded every response, including unchanged replies received while filesystem events were settling. It now retains one successful decoded response and reuses it only when a fresh engine reply is byte-for-byte identical. Changed replies decode normally; errors and responses above 1 MiB clear the cache. The cap bounds retained serialized bytes, not total decoded object memory. The Rust engine, polling schedule, request ordering and interface are unchanged.

The direct comparison compiles the actual before and after bridge code against the same models and Rust archive. Each invocation processes 100 synthetic responses, starting with an empty cache and allocating a fresh byte buffer every time. Full snapshot equality, exact integer precision and matching checksums are required. The changing controls alternate two different, equal-length replies; the oversized control exercises the uncached path. All **70 invocations** passed: one separate first pair and six alternating warm pairs per case. These are not cold-cache measurements.

The table reports the complete 100-response loop, including copying, decoding and validation. It excludes input generation, encoding, process startup, FFI, filesystem traversal and UI work. RSS is maximum whole-process memory across all seven invocations per build, including setup. With six warm samples, nearest-rank p95 is the observed maximum.

| Responses | Wire bytes per reply | Warm CPU median, before → after | Warm elapsed median, before → after | Warm elapsed p95, before → after | Peak process RSS, before → after |
| --- | ---: | ---: | ---: | ---: | ---: |
| Nine candidates, identical | 9,289 | 15.186 → 0.365 ms | 15.208 → 0.362 ms | 15.576 → 0.422 ms | 15.563 → 15.406 MiB |
| 500 candidates, identical | 440,606 | 609.914 → 12.336 ms | 610.668 → 12.340 ms | 620.257 → 12.548 ms | 21.719 → 21.547 MiB |
| Nine candidates, changing | 9,289 | 15.078 → 15.201 ms | 15.105 → 15.219 ms | 15.309 → 15.470 ms | 15.641 → 15.703 MiB |
| 500 candidates, changing | 440,606 | 610.508 → 608.355 ms | 611.468 → 609.045 ms | 615.873 → 611.929 ms | 21.828 → 22.672 MiB |
| Oversized, identical | 1,057,841 | 64.263 → 64.323 ms | 64.305 → 64.371 ms | 65.610 → 65.526 ms | 23.813 → 21.828 MiB |

Identical responses used **97.6% and 98.0% less median loop CPU**. The changing and oversized controls were within 0.9% in median CPU. These results isolate repeated decoding; they do not establish a comparable whole-app saving. The 500-candidate changing control retained up to 0.844 MiB more whole-process RSS in this sample.

The existing native maintenance protocol supplies a separate full-app check: 16 same-inode manifest writes at 400 ms intervals during a fixed eight-second native process CPU window. One separate first pair and four alternating warm pairs all passed, using fresh disposable fixtures and the same linked Rust archive. Every invocation indexed the final write, completed eight three-entry scope passes, preserved the eligible control and foreground result, drained its journal and agreed with a fresh engine snapshot after a quiet tail. No cleanup was requested.

Warm median native process CPU was **134.073 → 123.472 ms**, a **7.9% reduction**; three of four paired samples improved. The warm samples were `[116.366, 129.833, 138.312, 140.487]` ms before and `[114.823, 138.214, 100.335, 132.120]` ms after. Maximum warm lifetime process RSS was 102.375 → 102.500 MiB. Event scheduling and snapshot-read counts varied, so this small sample supports a modest benefit in this workload, not a general scanner, idle-CPU or window-opening claim.

Reproduce the direct comparison with a saved pre-change bridge and the current release archive:

```sh
python3 scripts/benchmark-snapshot-decoding.py \
  --baseline-source /path/to/before/EngineBridge.swift \
  --candidate-source native/Chippytea/EngineBridge.swift \
  --core-archive target/release/libchippytea_core.a \
  --iterations 100 --warm-runs 6 \
  --output benchmarks/local/snapshot-decoding-comparison
```

For the native comparison, run `scripts/benchmark-native-maintenance.py --app /path/to/Chippytea.app --core-archive /path/to/linked/libchippytea_core.a --output benchmarks/local/native-comparison-run` once per invocation, with a new output directory each time. Use one first pair followed by four alternating warm pairs of the before and after signed bundles. The harness preserves its fixture, input audit, journal proof, screenshots and raw timings.

- Before bridge: `00c3c7801efd630e60365507b81aa68ff115cd2e85f796cf568f3ef6e9e15ed4`.
- After bridge: `624f06aa6d36d3e186b7c9d26abeb714afa7347b35a0628266980d63f5c4d33f`.
- Identical linked Rust archive: `2a3d9b66cc715e678b8d1fa7329833311c7837ea101c04bcba61c29b43bf9f15`.
- Direct harness: Python `cbf5346361cfeea96c0697d18c341c29963b0a10e34877674db4367f38133335`, Swift `6aefbdc82ab38203ed253f05998642927dddad2c8ad8de0ecf1238824dbeee17`.
- Private frozen inputs, compiler commands, raw results and native verification logs: `benchmarks/local/postscan-work/`. The table uses `direct-decoding-final`; an earlier valid 70-invocation run is retained in `direct-decoding`. The only harness change between them raised the configurable minimum iteration count from one to two; both used 100.

The signed native build and both disposable native integration suites passed. New snapshot checks cover changed values, errors after a hit, recovery, returned-value mutation, exact large integers and the size boundary. Existing tests cover polling demand during an outstanding idle response, stable presentation, permissions setup, real Trash, restore, permanent cleanup and reward replay after restart. Permanent cleanup credited 276,828,160 bytes and two coins; Trash earned zero. The access suite measured 0.049 ms cleanup acknowledgment and a 30.109 ms maximum main-actor heartbeat gap in one integration run. Rust source and the archive are unchanged from the preceding 293-test validation.

## Complete internal hard-link discovery

Cargo build output can contain multiple names for the same file within one `target` directory. Previously the first such file excluded the whole artifact. Discovery now completes a bounded per-inode containment proof. It admits a linked developer artifact only when every alias belongs to that artifact and the full ownership, age, activity and metadata checks pass. Outside aliases remain excluded. Cleanup independently rebuilds the proof and verifies each unlink; only the final name can contribute private allocation. See [architecture](ARCHITECTURE.md) for the safety boundaries.

The reproducible comparison uses three independently written, aged Cargo fixtures: ordinary files, 4,096 pairs linked only within one target, and the same pair layout with one extra alias outside the authorized root. Each fixture also contains its own 100 MiB payload. Every complete traversal examines 8,201 paths. Independent audits verify all identities, link counts, timestamps, contents and inode-deduplicated allocation before and after the run. All **84 invocations** passed. The 70 exhaustive invocations retained identical full metadata fingerprints for each fixture across builds and modes.

Each series contains one separate first invocation and six alternating warm pairs. Creation and content audits warm the filesystem cache, so none is described as cold-cache. With six warm samples, nearest-rank p95 is the observed maximum. Values below are before → after; RSS is maximum whole-process memory, including CLI startup and JSON output.

| Fixture and mode | Examined entries | Warm elapsed median | Warm elapsed p95 | Peak process RSS | Eligible, before → after |
| --- | ---: | ---: | ---: | ---: | --- |
| Ordinary, suggestions | 8,201 → 8,201 | 24.300 → 24.454 ms | 24.900 → 28.149 ms | 3.250 → 3.203 MiB | Yes → Yes |
| Ordinary, metadata | 8,201 → 8,201 | 21.043 → 21.199 ms | 22.445 → 21.667 ms | 3.219 → 3.219 MiB | Yes → Yes |
| Internal links, suggestions | 8 → 8,201 | 10.911 → 24.209 ms | 10.952 → 24.527 ms | 3.172 → 4.953 MiB | No → Yes |
| Internal links, metadata | 8,201 → 8,201 | 22.117 → 22.587 ms | 22.516 → 23.049 ms | 3.703 → 4.969 MiB | No → Yes |
| Outside alias, suggestions | 8 → 8,201 | 10.306 → 22.200 ms | 11.011 → 23.636 ms | 3.156 → 4.891 MiB | No → No |
| Outside alias, metadata | 8,201 → 8,201 | 24.129 → 24.702 ms | 25.140 → 25.474 ms | 3.922 → 4.875 MiB | No → No |

The ordinary-file control retains identical recommendation evidence and coverage, with a 0.154 ms difference in median suggestion time; its observed p95 increased from 24.900 to 28.149 ms. Internal-link discovery now completes the traversal that suggestion mode previously skipped. Its extra work explains the longer duration. Metadata mode supplies equal path coverage but intentionally changes internal-link eligibility. The new containment check also requires a full walk before rejecting an outside alias. Reported CPU time has 10 ms precision on these short processes, too coarse to quantify small CPU differences.

This comparison measures public Rust CLI discovery. It does not measure native window latency, idle CPU, persistent event refreshes, cleanup duration or million-file memory. Estimated allocation is not observed physical recovery. The benchmark never requests deletion, and retains its marked disposable fixtures.

```sh
python3 scripts/benchmark-hardlinks.py \
  --baseline-cli /path/to/before/chippytea-cli \
  --candidate-cli target/release/chippytea-cli \
  --output benchmarks/local/hardlink-comparison
```

- Before CLI: `c3969c8e79bada29b4a6d2b6a78a7e00d974c75aaaac9e45a64a171c68d1d620`.
- After CLI: `de2a30d413cb7fea75f24f00eadd79b5b470c7cf0c99cfada9534691ddc41714`.
- Harness: `13a07fd7c00af59cb7f4973dbb4ac05044cbb366c2bf2f5c41b5978d61921683`.
- Private raw samples, full audits, frozen sources and validation logs: `benchmarks/local/internal-hardlinks/`.

All **293 Rust tests**, formatting and Clippy with warnings denied passed. The signed native integration test passed real Trash, conflict-safe restore and permanent removal of a disposable internally linked payload. It credited 276,828,160 bytes and two coins, with repeat collection and restart checks; Trash earned zero. The native access and interaction suite also passed, including stable scan status, permissions setup and cleanup progress. Its single integration run measured 0.051 ms cleanup acknowledgment and a 30.084 ms maximum main-actor heartbeat gap.

## Native scan completion and empty results

A scan-tail investigation distinguished foreground traversal from subsequent background maintenance. Profiling supplied diagnostic attribution, not a controlled before/after benchmark.

Subsequent structural changes to a short-lived lock directory kept background maintenance active. Those exact scopes reconcile separately; they do not extend or increment the completed foreground scan. The visible status now explicitly says whether the last scan completed, was cancelled, or remained partial. Detailed exclusion and error counts remain in its help text. The existing layout, current-scan counters and background controls are preserved.

An empty recommendation list does not imply an empty index. Artifacts below the 100 MB recommendation threshold, downloads inside the 14-day quiet period, and candidates failing ownership, activity, protected-data or link checks remain withheld. Measured allocation is not proof of reclaimable space. At that revision, supporting links entirely within an artifact still required a complete inode-membership proof and coordinated cleanup revalidation and accounting. The later internal-link change above adds those checks. No eligibility or deletion rule changed in the presentation correction itself.

The stable signed build and native access/interaction suite passed, including completed, cancelled and partial presentation across background updates and restarts. The Rust archive was unchanged. Private snapshots, the profile and native verification logs are retained in ignored `benchmarks/local/scan-tail-investigation`.

## Recent-file proofs during artifact refresh

An in-place file edit can leave its enclosing artifact directory looking old. Suggestion discovery previously had to reach the edited file during traversal before rejecting the artifact as too recent. The original event-hint change permitted a direct, descriptor-bound proof of recent content. Its cache retained at most 64 hints and 64 KiB of root IDs and paths. Missing, old or unsafe hints fell back to ordinary traversal. In the revision measured in this section, full scans did not use the shortcut; the later repeated-scan change extends that behavior. Cleanup validation never uses these hints. See [architecture](ARCHITECTURE.md) for the current identity and journal boundaries.

The local fixture contains 32,768 generated leaves below one aged artifact, a 100 MiB payload, and an independent eligible 100 MiB control. An untimed production traversal selected fixed first, middle and last leaf ordinals before any timing. Each invocation builds a fresh library, checks initial eligibility, rewrites one byte in the selected existing inode and then submits one typed file event. An unchanged-file control submits the event without writing. Full filesystem audits surround every invocation and mtime restoration, allowing only the intentional leaf content/timestamp change. Every endpoint must preserve the control recommendation and foreground result, drain the durable journal, maintain a zero ledger, and agree with a subsequent exhaustive metadata scan.

All **88 primary invocations** passed. Each case has one separate first invocation per build and ten alternating warm pairs; no result is labeled cold-cache. The table reports warm medians, elapsed p95 and maximum lifetime process RSS.

| File position | Traversed entries, before → after | Process CPU median, before → after | Elapsed median, before → after | Elapsed p95, before → after | Peak RSS, before → after |
| --- | ---: | ---: | ---: | ---: | ---: |
| First | 5 → 1 | 28.679 → 27.056 ms | 758.520 → 753.676 ms | 767.974 → 764.159 ms | 5.359 → 5.281 MiB |
| Middle | 16,451 → 1 | 61.773 → 22.163 ms | 792.936 → 759.966 ms | 797.358 → 764.255 ms | 5.375 → 5.344 MiB |
| Last | 32,899 → 1 | 77.276 → 26.727 ms | 814.150 → 753.570 ms | 828.411 → 767.239 ms | 5.422 → 5.312 MiB |
| Unchanged file | 32,900 → 32,900 | 83.130 → 89.843 ms | 785.530 → 816.741 ms | 823.015 → 843.607 ms | 5.375 → 5.453 MiB |

Middle and last positions used **64.1% and 65.4% less median process CPU**; all ten pairs improved in each case. The single traversed entry is the artifact boundary, not a claim of one filesystem operation: the verified leaf and ancestor probes are included in CPU and elapsed time. The first position showed a small difference, with only six of ten pairs lower. The unchanged-file control was **8.1% higher** in the primary sample and should not be presented as an improvement.

Two bounded follow-up diagnostics separated that control result from the mechanism. With the same candidate binary and unchanged fixture, eight alternating typed-versus-unknown event pairs measured 64.175 versus 63.947 ms median CPU, a 0.228 ms hint-miss cost; both traversed all 32,900 artifact entries. A further eight before/after pairs with unknown events and no hint measured 66.204 versus 63.888 ms. The shared-path regression did not reproduce in that diagnostic. These separate runs retain complete endpoint checks and bracketing immutable-tree audits; they do not replace the original higher control result or establish a general speedup.

The existing 512-event FFI control also passed all 12 invocations (one first plus five warm per build). Both builds traversed the same 514 entries once. Median total CPU was effectively flat at 14.272 → 13.694 ms, while dirty acknowledgment CPU increased from 0.805 to 1.393 ms. That 0.588 ms bookkeeping cost remains an optimization opportunity.

The primary comparison calls the public Rust Engine API. Timed intervals include the unchanged 600 ms background debounce, worker scheduling, durable cursor handling and 5 ms snapshot polling. Initial indexing, full verification and Python filesystem audits are outside the timed interval. RSS includes initial discovery. This does not measure Swift/FFI overhead, native event delivery, idle CPU, window latency or full Home scan performance. Partial ineligible diagnostics intentionally have different measured totals; exclusions, eligible control proof and exhaustive coverage must agree.

Reproduce with two matching release rlibs and their dependency directory. Omit `--fixture-manifest` to create a fresh marked fixture and freeze its ordinal selection:

```sh
python3 scripts/benchmark-recent-files.py \
  --before-rlib /path/to/before/libchippytea_core.rlib \
  --after-rlib target/release/libchippytea_core.rlib \
  --leaves 32768 --warm-runs 10 \
  --output benchmarks/local/recent-file-comparison
```

- Before rlib: `bc68ab42203b515070cb990c8d2555c4c036bb066b8b0c82b9038cbf03c732c8`.
- After rlib: `d40674a17673b69c1cc0cbffbc307490fd1e853815fae722fb087f3e679b12c8`.
- Measured harness: Rust `6409a47163a7d03ee7f4cf334197d03d9b65b50c44503e651d7e4b92a6338231`, Python `4a2a475105647c32eb9fcf0fd7586c11ba4c46ec33df8acdad6f344a71d37140`.
- Private protocol, frozen sources, two preliminary compatibility invocations, raw snapshots, audits and diagnostic comparisons: `benchmarks/local/recent-file-witness/`.

The signed native build, **278 Rust tests**, formatting and Clippy with warnings denied passed. Both native disposable integration suites passed, including permanent cleanup and restart with 276,527,125 credited bytes and two coins. The access/interaction suite observed 0.045 ms cleanup acknowledgment and a 30.105 ms maximum main-actor heartbeat gap in one run. Native interface source and artwork are unchanged.

## Refreshing Cargo lockfile dependencies

An exact `Cargo.lock` file event previously refreshed the whole containing project. It now refreshes the original item and its immediate `target` sibling. Other manifests and configuration retain their broader dependency scopes. The worker verifies exact directory-entry spelling and file identities before publishing the target, and replays the original durable scope after interruption. See [architecture](ARCHITECTURE.md) for the coverage and race checks.

The new `cargo-lock-refresh` workload replays one typed, nonrecursive file event through the real FFI. Its aged Cargo project contains a small measured target, plus either 8,192 unrelated source files in 512 directories or 16 source files in one directory. An independent eligible 100 MiB Node artifact remains unchanged. Every invocation uses a fresh library. Before accepting a timing, read-only checks require the same complete two-row index, candidate identities, evidence, fingerprints and allocation, a drained journal, unchanged foreground result and zero accounting ledger. An untimed explicit Scan must then produce the same index. Independent audits verify every fixture file, metadata and allocated-block count.

| Workload | Warm runs per build | Refresh entries, before → after | CPU median, before → after | Elapsed median, before → after | Elapsed p95, before → after | Peak RSS, before → after |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Cargo, 512 source directories | 20 | 8,710 → 4 | 30.671 → 14.632 ms | 724.043 → 656.518 ms | 778.372 → 721.681 ms | 9.406 → 9.563 MiB |
| Cargo, one source directory | 20 | 23 → 4 | 14.431 → 14.018 ms | 706.172 → 637.097 ms | 758.771 → 752.976 ms | 9.375 → 9.531 MiB |
| Mixed directory control, 512 events | 5 | 512 → 512 | 242.127 → 241.599 ms | 1,024.991 → 980.121 ms | 1,059.586 → 1,030.237 ms | 9.813 → 9.938 MiB |

The large Cargo workload used **52.3% less median CPU time**; all 20 paired warm samples were lower, and mean CPU fell from 31.734 to 15.945 ms. The tiny workload and mixed control were effectively flat in CPU: their means changed from 14.311 to 14.333 ms and 242.231 to 243.471 ms. Elapsed timings include the unchanged 600 ms background debounce, scheduling, journal work, cursor receipt and 5 ms snapshot polling. The four refreshed entries exclude the initial and final shallow parent-name checks; their actual work is included in time and CPU. RSS is lifetime process peak through the measured endpoint, including initial discovery. These measurements do not establish a native UI, idle CPU or full-scan speedup.

All **96 accepted invocations** passed. Each series also retains one separate first invocation per build; none is labeled cold-cache. A first baseline attempt rejected by an incorrect harness assumption about idle Scan counter resets remains in the private evidence, with the correction recorded before the accepted comparison. Source and raw evidence are identified by:

- Before Rust archive: `e9e390735c86eaa721108233f3cc72e7357298a85d9c2f790ca2309ee15a2a54`.
- After Rust archive: `68c3a07bec05d3c61353eb1346ed2dddec006dbb346d7b8a4eaddfb6ada783cc`.
- Harness: Python `4b15ce1a7b8c052258cc0a038e753f082eedfcdcf1ff1e22022444e36b1e1372`, C `39d12374bcb28e57d913cf01ecb3be8aa608e8e3fc44a700285ba37559c589e7`.
- Private protocol, logs, snapshots and audits: `benchmarks/local/cargo-lock-refresh/`.

Reproduce with two release archives, then repeat with `--scopes 1` and with `--workload mixed --scopes 512 --warm-runs 5`, each using a new output directory:

```sh
python3 scripts/benchmark-events.py \
  --baseline-archive /path/to/before/libchippytea_core.a \
  --candidate-archive target/release/libchippytea_core.a \
  --workload cargo-lock-refresh --scopes 512 --warm-runs 20 \
  --output benchmarks/local/cargo-lock-comparison
```

The signed native build, **261 Rust tests**, formatting and Clippy with warnings denied passed. Both native disposable integration modes passed, including permanent cleanup and restart with 276,828,160 credited bytes and two coins. Cleanup acknowledgment was 0.047 ms and the maximum main-actor heartbeat gap was 30.098 ms in that single integration run. Native source and artwork are unchanged.

## Coalescing event batches before journal lookups

Many file events resolve to the same artifact. Previously every resolved path repeated grant and pending-ancestor queries, even when an earlier path in the batch already covered it. Multi-scope batches now validate all paths against one grant read inside the transaction, sort by path components, remove duplicates and covered descendants, then restore the surviving paths' first-occurrence order. Existing queue order, active claims, late events and atomic failure behavior are preserved. Single-scope requests retain their direct path. No filesystem traversal or eligibility rule changed.

The new `artifact-event-batch` workload submits 512 distinct existing file paths in one typed FFI request. The unchanged fixture contains those files and an aged 100 MiB payload in one eligible artifact. Both builds must traverse the same 514 artifact entries exactly once, retain the same candidate identity, fingerprint, evidence, allocation and file count, preserve full coverage and the saved foreground result, and drain the durable queue. Independent filesystem and SQLite audits verify every file, allocated-block total, cursor and empty accounting ledger. The mixed control submits 512 unrelated directory scopes, half present and half absent.

The first five warm samples showed higher total CPU despite cheaper acknowledgment. A fixed follow-up of twenty warm samples per build was recorded before running either workload again; the initial results remain below. Each series also retains one separate first invocation per build. All **108 invocations** passed their coverage and preservation gates.

| Workload; warm runs per build | Request CPU median, before → after | Total CPU median, before → after | Total elapsed median, before → after | Total elapsed p95, before → after |
| --- | ---: | ---: | ---: | ---: |
| Artifact batch; 5 | 2.152 → 0.894 ms | 32.230 → 35.303 ms | 676.936 → 674.311 ms | 724.603 → 706.578 ms |
| Artifact batch; 20 | 2.047 → 1.007 ms | 35.911 → 32.328 ms | 643.719 → 634.783 ms | 764.089 → 724.073 ms |
| Mixed control; 5 | Not measured | 222.725 → 231.075 ms | 1,009.172 → 951.986 ms | 1,036.502 → 1,019.714 ms |
| Mixed control; 20 | Not measured | 241.700 → 242.266 ms | 950.598 → 955.977 ms | 1,044.939 → 1,029.636 ms |

The larger artifact series used **50.8% less median CPU to acknowledge the event request**, improving in all twenty paired rounds. Acknowledgment elapsed p95 fell from 2.236 to 1.130 ms. Total CPU fell 10.0% by median and 10.5% by mean, with fourteen of twenty pairs faster. The larger mixed control was effectively unchanged: median CPU rose 0.23% and elapsed time 0.57%. Maximum process lifetime RSS in those follow-ups was 9.672 → 9.828 MiB for the artifact batch and 9.750 → 9.875 MiB for the control. These measurements support cheaper duplicate event ingestion; they do not establish a native app, idle-CPU or full-scan speedup.

Acknowledgment timing includes event parsing/classification, durable enqueueing, worker launch and response disposal. It ends before cursor receipt or snapshot polling. Total timing additionally includes the unchanged 600 ms background wait, traversal, cursor persistence and full snapshot polling every 5 ms. Those unchanged operations account for much of the total CPU and its variation. Setup, input-path verification and initial full discovery are excluded; RSS includes setup. Order alternates, p95 uses nearest rank, and first invocations are not controlled cold-cache measurements. No app, test, build or profiler ran concurrently with accepted timings; the harness links its drivers before timing.

```sh
python3 scripts/benchmark-events.py --baseline-archive /path/to/preserved/libchippytea_core.a \
  --candidate-archive target/release/libchippytea_core.a \
  --workload artifact-event-batch --scopes 512 --warm-runs 20 \
  --output benchmarks/local/new-artifact-comparison
python3 scripts/benchmark-events.py --baseline-archive /path/to/preserved/libchippytea_core.a \
  --candidate-archive target/release/libchippytea_core.a \
  --workload mixed --scopes 512 --warm-runs 20 \
  --output benchmarks/local/new-mixed-control
```

The preserved archive is `894ccaacb89b4f0278c9ec40c57098ead201a6febef798baf1e5e7f42f3f44ef`; the candidate is `e9e390735c86eaa721108233f3cc72e7357298a85d9c2f790ca2309ee15a2a54`. All 242 Rust tests, formatting and Clippy with warnings denied passed. New regressions compare batch results and FIFO order with sequential enqueueing, exercise normalized and literal-prefix paths, retain events during active work, and verify invalid-input and injected-SQL-failure rollback. Exact source/build hashes, the fixed follow-up protocol, every run and private fixture audits remain under ignored `benchmarks/local/event-batch-coalescing`.

Both native disposable gates passed on executable `b5b63a1c5d3076ca5055cafaa175ba0f8700ce76aabf61807539e22a84ee46e7`, with the measured archive stamp and unchanged signing requirement. They exercised access, watcher delivery, scan/restart presentation, native Trash, conflict-safe restore and permanent cleanup, crediting 276,828,160 disposable bytes and two coins. Native source and artwork are unchanged.

## Active scan CPU profile and invalidated summaries

Diagnostic profiling attributed most scanner work to directory access and enumeration. These exploratory captures were not controlled benchmark comparisons. Source review found one retained no-follow descriptor per ordinary directory, with no repeated open to remove. The earlier minimal-bulk reader trials were slower on equivalent fixtures. Neither finding justifies weakening validation or adding another traversal path. Rust source is unchanged in this pass.

For attribution on a disposable test library, set `CHIPPYTEA_APP_PID` to the running test app process ID and request Refresh during recording. Use new output paths beneath ignored `benchmarks/local`:

```sh
xcrun xctrace record --template 'Time Profiler' --attach "$CHIPPYTEA_APP_PID" \
  --time-limit 30s --output benchmarks/local/new-profile.trace
xcrun xctrace export --input benchmarks/local/new-profile.trace \
  --xpath '/trace-toc/run[@number="1"]/data/table[@schema="time-profile"]' \
  --output benchmarks/local/new-profile.xml
```

Raw traces can contain private paths and process environment. They remain in ignored `benchmarks/local/scanner-hot-paths` alongside the analysis script, aggregates, source hashes and preservation evidence. Profiling and validation are separate; no performance timings are inferred from test runs.

The native fix addresses another way maintenance could appear to be a slowly continuing scan. Forgetting a grant invalidates the full-scan summary, but Swift previously substituted cumulative counters, including work from the removed root. After restart it could instead label one stored scope as the full scan. A missing summary now displays “No saved scan,” and raw diagnostics no longer trigger UI invalidation. Pending requests, active controls, errors and independently derived coverage warnings still publish. Layout and artwork are unchanged.

The regression is available through `Chippytea --access-flow-test --forget-summary-regression`; the full access-flow gate includes it too. It uses disposable files and real native grant actions, scoped Rust maintenance and engine reopening. It must preserve the distinction between a 21-entry full scan, cumulative diagnostic counts of 23 then 25, two restored scope entries, and a later explicit four-entry scan. No full-root repair is scheduled to manufacture the restart result.

With the same new test, the old production code fails at the post-forget presentation assertion. The candidate passes both native disposable gates, including all 18 files and the empty ledger in the new regression. Twenty no-summary raw updates reach observers with zero UI notifications; incomplete coverage, recovery and changed errors are verified separately through the real snapshot application path. These are publication checks, not frame-time or CPU benchmarks. The cleanup integration also passes native Trash, restore, permanent cleanup and reward persistence across restart, crediting 276,828,160 disposable bytes and two coins.

The verified native executable is `5f6d65c88fe54f0cc4e06c3a16d93fe44d75a03a1c716e36ad6d00bdd3de097e`, linking the unchanged `894ccaacb89b4f0278c9ec40c57098ead201a6febef798baf1e5e7f42f3f44ef` archive with the same signing requirement. No scanner, idle-CPU or general native speedup is claimed from this pass; the unchanged engine retains its preceding 239-test validation.

## Navigation updates and filtered findings

The bottom navigation now uses an equatable boundary over its action model's identity, selected destination, pending-coin presence and finding count. Progress-only updates no longer reevaluate its four tabs. Discovery also filters and sorts once per body evaluation, sharing that result with the list and toolbar instead of evaluating it four times. Layout, artwork, row observation and cleanup controls are unchanged.

Separate private body-count profiles observed zero bottom-bar and tab evaluations after the change while those inputs stayed fixed. Filter/sort calls per Discovery evaluation fell from four to one. The profiles observed eight versus one scanning episode despite both completing eight scopes, so their other absolute body-count differences cannot isolate this optimization. These are body/getter evaluations, not compositor frames; instrumented process CPU is excluded from performance samples.

The ordinary-build comparison uses the same Rust archive and protocol-2 native maintenance harness on both sides. Sixteen same-inode manifest writes drive the actual watcher, engine, bridge and visible Find space window over eight seconds. Protocol 2 additionally counts observed scanning transitions in the existing raw-snapshot subscriber. It adds no polling or engine reads. Every run must satisfy the endpoint, final-write freshness, drained-journal, candidate, filesystem, accounting and quiet-tail checks described below.

| Visible native maintenance, 5 warm runs per build | Before | After |
| --- | ---: | ---: |
| Median CPU over eight seconds | 134.153 ms | 140.172 ms |
| Median CPU as a fraction of one core | 1.68% | 1.75% |
| p95 CPU | 138.033 ms | 159.171 ms |
| Maximum process lifetime RSS | 103.125 MiB | 102.453 MiB |
| Completed three-entry scopes, every run | 8 | 8 |
| Observed scanning rises and falls, warm range | 2–3 each | 3–6 each |
| Model UI invalidations, warm range | 4–6 | 6–12 |
| Foreground presentation publications | 0 | 0 |

All twelve invocations passed, including the two separately recorded first runs. Median CPU increased **4.5%**, about 6 ms over eight seconds. This series does **not** demonstrate a native CPU reduction. Different observed transitions mean equal scope coverage did not produce equal UI update counts; the measurements cannot isolate the navigation boundary's CPU effect. The supported improvement is less repeated view work. No full-scan, idle-CPU or general app speedup is claimed.

Order alternates, p95 is nearest rank and therefore the maximum of five warm samples, first invocations are not controlled cold-cache measurements, and RSS includes setup. No other Chippytea process, build, test or profiler ran during timing. Each report's app, archive, runner and shared-helper hashes were checked. Reproduce each invocation with `scripts/benchmark-native-maintenance.py --app /path/to/signed/Chippytea.app --core-archive /path/to/exact-linked/libchippytea_core.a --output benchmarks/local/new-comparison`, using identical benchmark sources in both builds. The baseline executable is `a82d7968fcfedb7fe976afbb330004ffc9392290b7d9c3d87627661ec18d9ac6`; the candidate is `a994577be2410869c1abda79bb840fbdd23343bfa8750ce2f8a0ba86744c1ea4`. Both link archive `894ccaacb89b4f0278c9ec40c57098ead201a6febef798baf1e5e7f42f3f44ef`.

Both native disposable modes passed on those exact candidate bytes. Permanent cleanup credited 276,828,160 bytes and earned two coins through restart. Cleanup acknowledgment was 0.045 ms and the maximum main-actor heartbeat gap was 30.174 ms; these are individual integration observations. The unchanged Rust engine retains the preceding pass's 239-test, formatting and Clippy validation. A separate normal-app GUI check exercised all four tabs, category and text filters, clearing search, Keep and Include again. The list count and global badge changed correctly in both directions, and the restored finding retained the notebook layout. Every disposable file identity/content and all five accounting tables were unchanged. No cleanup ran through this manual GUI check. Exact provenance, profiles, all samples, native logs and screenshots remain under ignored `benchmarks/local/native-ui-updates`.

## Index statement reuse and native update handoff

Scoped refreshes now prepare the two ancestor-reconciliation statements once and reuse them throughout the ancestor walk. SQL, scope boundaries, transactions and filesystem validation are unchanged. The existing mixed-event benchmark submits 256 scopes, half existing and half absent, with an unchanged eligible 100 MiB artifact. Both builds use the same 600 ms background wait.

| Warm mixed-event series | Median elapsed, before → after | p95 elapsed, before → after | Median CPU, before → after | Maximum lifetime RSS, before → after |
| --- | ---: | ---: | ---: | ---: |
| Initial, 5 runs per build | 847.174 → 861.416 ms | 923.482 → 923.108 ms | 165.743 → 159.469 ms | 9.344 → 9.344 MiB |
| Confirmation, 15 runs per build | 866.655 → 863.461 ms | 939.006 → 940.836 ms | 164.975 → 159.677 ms | 9.422 → 9.375 MiB |

The larger series used **3.2% less median CPU**, with essentially unchanged elapsed time. The initial elapsed regression is retained. All 44 invocations, including separately recorded first runs, passed candidate, coverage, journal, cursor, filesystem and ledger checks. Order alternates; p95 uses nearest rank. First runs are not controlled cold-cache measurements, RSS includes setup, and timing includes the wait plus 5 ms snapshot polling. These are Rust FFI measurements, not full-scan or native app speedups. No app, build, test or profiler ran concurrently. Reproduce with `scripts/benchmark-events.py --workload mixed --warm-runs 15` and the archive/output arguments shown below.

A separate native race could discard a request to continue polling when it arrived during an older idle snapshot read. The poller now remembers demand across that read and rereads if the idle response would otherwise stop it. Active work retains the 150 ms cadence, cleanup priority and existing response ordering; no idle timer is added. A disposable regression holds a genuine idle response, delivers a later ownership-file change through FolderWatcher, and requires the final model to match the engine without manual refresh. The old idle-stop decision fails that assertion; the corrected app passes. The comparison variant retains only the demand counter as test instrumentation and is not an exact old binary.

The new native maintenance runner opens Find space on a guarded disposable library. Another process writes the same manifest inode sixteen times, starting at +250 ms and then every 400 ms, with each write synchronized and closed within 50 ms of its absolute deadline. Native process CPU includes FolderWatcher, Rust, bridge polling and SwiftUI over eight seconds. The endpoint must be settled and match two fresh engine snapshots without applying them or manually refreshing; a 1.2-second quiet tail must remain unchanged. The final derived timestamp, drained journal, candidate identity/evidence, every file and all five accounting tables are checked. FSEvents may coalesce writes; completed scope counts come from exact entry deltas, not callback counts.

| Visible native maintenance, 5 warm runs per build | Before | After |
| --- | ---: | ---: |
| Median CPU over eight seconds | 119.820 ms | 150.699 ms |
| Median CPU as a fraction of one core | 1.50% | 1.88% |
| p95 CPU | 136.148 ms | 165.267 ms |
| Maximum process lifetime RSS | 102.625 MiB | 102.516 MiB |
| Completed three-entry scopes, every run | 8 | 8 |
| Model UI invalidations, warm range | 2–8 | 6–16 |
| Foreground presentation publications | 0 | 0 |

All twelve invocations passed, including the two separately recorded first runs. **Native CPU increased 25.8% in this small series** despite the separate Rust statement saving. The absolute difference is about 31 ms over eight seconds. More model UI invalidations occurred in the candidate series. Source inspection points to observed scanning-state edges under these fixed inputs; the next profile should count those transitions directly. Equal scope coverage does not guarantee equal sampling of gaps between workers. This is a reason to profile view updates, not evidence that every invalidation renders a frame or that suppressing truthful control updates is safe. The poll fix is retained for correctness. No full-scan, idle-CPU or general native speedup is claimed.

The native inputs contain one unchanged positive artifact and one changing ownership file, with ten examined entries in the initial Suggestions pass. Each invocation gets fresh identities and preserves its own initial proof. Both sides use the same benchmark code; the production differences are the Rust statement reuse and native polling fix. Order alternates, p95 is nearest rank, setup warms caches, and RSS includes setup. The measured window excludes Python writer CPU, post-endpoint audits and the quiet tail. Counts are model notifications, not compositor frames or event callbacks. No other Chippytea process, build, test or profiler ran during the comparison.

```sh
python3 scripts/benchmark-native-maintenance.py --app /path/to/signed/Chippytea.app \
  --core-archive /path/to/exact-linked/libchippytea_core.a \
  --output benchmarks/local/new-maintenance-comparison
```

Use the same native benchmark source in both builds. The initial baseline and candidate invocations were rejected at the endpoint; a diagnostic run measured 8.383 seconds with every non-time guard passing. The benchmark sleep now requests zero timer tolerance, while still rejecting an actual endpoint outside 8 to 8.05 seconds. Normal app timers are unchanged. All three rejected invocations and setup errors remain recorded; none contributes a performance sample. [Swift documents the timer tolerance parameter](https://developer.apple.com/documentation/swift/task/sleep(for:tolerance:clock:)); an explicit request does not remove the need to validate actual scheduling.

All 239 Rust tests, formatting, Clippy with warnings denied and both native disposable flows passed. On the final build, cleanup acknowledged in 0.048 ms with a 30.226 ms maximum main-actor heartbeat gap; permanent cleanup credited 276,828,160 bytes and earned two coins through restart. These latency values are individual integration observations. The candidate Rust archive is `894ccaacb89b4f0278c9ec40c57098ead201a6febef798baf1e5e7f42f3f44ef`, against `2fc042d2392e27d3f3f4fef0a8ee0d33e9caf19e449050a042614543ff02dc21`. Private source provenance, all samples, regression failures and logs are retained under `benchmarks/local/native-maintenance`.

The final native executable is `a37958ebb2c38e35c6a413fb5cd6aa15f3299345e0b45ad1a99441ea18d8b35e`; its linked archive stamp and unchanged signing requirement were verified. Both disposable modes passed again on these exact bytes.

## Background batching and immediate Scan

The 150 ms background wait often finished before the next native event delivery, allowing repeated changes to traverse the same scope separately. Scan and Resume also could not interrupt a worker already waiting. Background workers now use a fixed 600 ms deadline captured before thread creation. Explicit requests and captured full-root recovery wake an existing wait immediately; later events never extend the deadline. Cancellation, cleanup parking and durable event acknowledgment retain their existing boundaries.

The new `periodic-background` workload submits sixteen identical typed directory events at absolute 400 ms intervals, each followed by durable cursor receipt. Both builds observe the same eight-second window without intermediate snapshot polling. The unchanged scope contains one directory and one file; exact entry, file and directory deltas count completed traversals, not threads or database transactions. An unrelated aged 100 MiB artifact must retain the same eligible recommendation and complete proof. A separate probe submits Scan while an exact child scope remains queued with no active traversal.

| Measurement, five warm runs per build | Before | After |
| --- | ---: | ---: |
| Completed scope traversals for sixteen events | 16 | 8 |
| Median process CPU over the eight-second window | 52.396 ms | 30.873 ms |
| Median Scan-to-completed-foreground delivery | 228.143 ms | 16.019 ms |
| p95 Scan-to-completed-foreground delivery | 255.166 ms | 16.457 ms |
| Maximum process lifetime RSS at the window endpoint | 8.984 MiB | 9.031 MiB |

This repeated-scope workload used **41.1% less median CPU time**. All twelve invocations, including the two separately recorded first runs, passed exact candidate, coverage, journal, cursor, filesystem and zero-ledger checks. Archives alternate; p95 uses nearest rank, and first runs are not controlled cold-cache measurements. Scan completion includes a small full-root traversal and 5 ms snapshot polling, so it is an upper bound on wake latency rather than a pure scheduling measurement. Acknowledgment alone was essentially unchanged: 0.663 to 0.729 ms median.

The fixed window is not a throughput measurement. A lone background event can now wait an additional 450 ms, and native progress polling remains active while the Rust worker sleeps. These synthetic FFI results do not establish a native app CPU reduction or faster full scans. No app, build, test or profiler ran concurrently with the comparison. RSS includes initial setup and ends before the separate Scan probe. Every input deadline, proof and sample is retained under ignored `benchmarks/local/background-batching/periodic-comparison`.

```sh
python3 scripts/benchmark-events.py --baseline-archive /path/to/preserved/libchippytea_core.a \
  --candidate-archive target/release/libchippytea_core.a \
  --workload periodic-background --warm-runs 5 --output benchmarks/local/new-comparison
```

All 239 Rust tests, formatting and Clippy with warnings denied passed. The public Scan regression failed against the preserved implementation before the wake fix. Handshake tests cover requests before and during the wait, subsequent workers, full-root promotion, unchanged deadlines, cancellation and mutation parking. The measured archive is `2fc042d2392e27d3f3f4fef0a8ee0d33e9caf19e449050a042614543ff02dc21`, against `fe7be727401c58ea8e2d4d2dd7a7e4191d89de947953983d1c64f4b92464d6e0`.

Both native disposable modes passed on the signed app. Six same-inode ownership-file writes, spaced 400 ms apart, reached the real FolderWatcher and AppModel. A tiny ineligible diagnostic had to match the final write's distinct timestamp in the same SQLite read that proved the journal had drained. The separate eligible candidate, completed foreground presentation, source contents and ledger stayed unchanged. This is an integration check; FSEvents can combine writes before Rust receives them. Cleanup acknowledged in 0.047 ms, with a 30.131 ms maximum main-actor heartbeat gap. Permanent cleanup credited 276,828,160 bytes and earned two coins through the disposable restart flow.

The native executable is `ccdc9967739c361f85d5c34ded762cea061eda30871a06eb714920e3c549a85e`; its embedded archive stamp matches the measured engine and its signing requirement is unchanged. Native production source files and notebook artwork were unchanged; only the native self-test changed. Logs, exact hashes and screenshots remain in `benchmarks/local/background-batching`.

## Artifact refresh boundaries and scan status

Raw persisted or unkeep paths could start an index refresh for one child while the scanner traversed the entire enclosing artifact. The next queued sibling then repeated that traversal, and stale sibling rows were outside the reconciliation boundary. The journal and scanner now use the same outermost artifact strictly below the grant. Protected inputs retain their exact scope, original claims remain the cancellation/acknowledgment key, and events arriving after traversal begins remain queued.

A new `unindexed-artifact-replay` workload stages 32 raw descendant paths around one aged, eligible artifact containing a 100 MiB payload and 4,096 small files. The harness verifies an exact persisted queue and empty index through read-only SQLite before timing Resume after reopening. Each build returns the same candidate identity, fingerprint, evidence, allocation and file count. An untimed full scan independently confirms whole-root coverage. Scoped replay itself correctly leaves global coverage incomplete. Scanned files and rewards remain unchanged.

| Workload and measurement | Before | After |
| --- | ---: | ---: |
| Artifact entries examined per replay | 8,324 | 4,162 |
| Replay warm median elapsed, 5 samples | 22.963 ms | 15.288 ms |
| Replay warm p95 elapsed | 25.527 ms | 15.327 ms |
| Replay warm median process CPU | 21.159 ms | 11.045 ms |
| Replay maximum process lifetime RSS | 8.984 MiB | 8.906 MiB |
| Mixed event control warm median elapsed, 15 samples | 399.336 ms | 395.293 ms |
| Mixed event control warm p95 elapsed | 484.922 ms | 462.540 ms |
| Mixed event control warm median process CPU | 132.380 ms | 131.521 ms |

The targeted replay performs one artifact traversal instead of two, with 47.8% less median CPU time in this series. This is not a full-scan speedup. An earlier five-sample mixed control was slower: elapsed median 356.576 to 409.877 ms and CPU 136.871 to 141.609 ms. The longer confirmation was effectively unchanged, so no general event-processing gain is claimed. All 56 accepted invocations passed exact coverage, recommendation, cursor, journal, wallet and filesystem audits. One separate replay attempt started before native test completion was confirmed and was invalidated in full before interpreting its timings; its output is retained under `replay-comparison/INVALIDATED.json`.

Binaries alternate; first invocations are retained separately and are not controlled cold-cache measurements. p95 uses nearest rank. Replay timing includes Resume, worker startup, reconciliation and 5 ms snapshot polling; staging, reopening and full-scan validation are excluded. RSS is the process lifetime peak through the measured endpoint. Both builds in this historical mixed control used a 150 ms debounce. No app, build, test or profiler ran concurrently with the accepted comparisons. Commands, hashes, audits and every sample are under ignored `benchmarks/local/artifact-scopes`:

```sh
python3 scripts/benchmark-events.py --baseline-archive /path/to/preserved/libchippytea_core.a \
  --candidate-archive target/release/libchippytea_core.a \
  --workload unindexed-artifact-replay --warm-runs 5 --output benchmarks/local/new-comparison
python3 scripts/benchmark-events.py --baseline-archive /path/to/preserved/libchippytea_core.a \
  --candidate-archive target/release/libchippytea_core.a \
  --workload mixed --warm-runs 15 --output benchmarks/local/another-new-comparison
```

The native row now visibly labels settled totals “Last scan” and uses “Pause” for background updates; foreground work retains “Cancel.” Pending requests do not display stale counts or completion help. Artwork and layout are unchanged. All 234 Rust tests, formatting and Clippy with warnings denied passed. Native disposable integration verifies Trash, restore, permanent cleanup and durable restart; access/interaction tests verify the new presentation states. The final native build evidence is recorded alongside the comparison.

## Transient refresh safety

Incremental startup previously checked existence and then looked up initial metadata separately. A short-lived directory disappearing between those operations could become a global warning and leave coverage incomplete. Startup now returns one verified metadata or absence result directly to the scanner. Both outcomes validate the pinned ancestry and current grant; one observed disappearance before traversal can enter a fresh absence proof. Later traversal failures stay partial. This fixes a race without expanding refreshes to parents or clearing unrelated coverage failures.

The existing event benchmark exercised 256 typed scopes per invocation, half existing and half absent, against fresh libraries and an unchanged fixture containing one eligible 100 MiB artifact. All **12 invocations** passed exact coverage, candidate identity/fingerprint/allocation, journal, cursor, wallet and independent filesystem audits. Binaries alternate; each has five warm runs and one separately recorded first invocation.

| Background refresh | Before | After |
| --- | ---: | ---: |
| Warm median wall time | 372.535 ms | 345.197 ms |
| Warm p95 wall time | 453.429 ms | 441.071 ms |
| Warm median process CPU | 123.408 ms | 129.198 ms |
| Maximum process lifetime RSS | 9.391 MiB | 9.344 MiB |

Elapsed time improved in this small series, but median CPU rose **4.7%** with the stronger ancestry proof. This is a correctness improvement, not evidence of lower CPU usage or faster full scans. Timings include the existing 150 ms debounce, SQLite work and 5 ms snapshot polling; setup is excluded. RSS includes setup. p95 is nearest rank, the maximum with five warm samples; the first invocation is not a controlled cold-cache result. No profiler or concurrent build/test ran during timing.

Reproduce with `scripts/benchmark-events.py --baseline-archive /path/to/preserved/libchippytea_core.a --candidate-archive target/release/libchippytea_core.a --scopes 256 --warm-runs 5 --output benchmarks/local/new-event-comparison`. Exact binaries, commands, all samples and unchanged fixture audits are retained in ignored `benchmarks/local/transient-scopes/event-comparison`.

All **228 Rust tests**, formatting, Clippy with warnings denied, the stable signed build and both native disposable modes passed. The new races cover disappearing and reappearing leaves, detached/replaced grants and parents, symlinks, permissions, cancellation, late events and preservation of prior incomplete coverage. Native tests verify warning dismissal through twenty raw updates with zero UI notifications, stale-read ordering, failed-read recovery, explicit retries and truthful coverage help during foreground work. Native cleanup acknowledged in **0.045 ms**; the maximum main-actor heartbeat gap was **30.101 ms** in that disposable run. Permanent cleanup credited 276,828,160 bytes and earned two coins.

The signed app identity and linked archive stamp were verified. Native screenshots, validation logs and exact build hashes are retained in ignored `benchmarks/local/transient-scopes`.

## Less per-directory path work

Diagnostic profiling pointed to per-directory traversal overhead. Wall-stack observations are not CPU percentages and do not justify removing filesystem validation. Profiling was separate from all timings below.

Two small changes remove computation around those operations: ordinary directory names now fail adapter dispatch before parent/root-prefix checks, and directory readers reserve child-path capacity before copying the parent. Supported artifacts retain the same authorization checks; raw path bytes, cancellation, cloud checks, first/final identity barriers and directory opens are unchanged. Binary inspection confirmed the names reader remains 1,300 bytes / 325 instructions, with the path helper already out of line. No compiler hints were added.

Each series alternates binaries on the same audited fixture, with one separately recorded first invocation per version. All **66 invocations** retained exact coverage and the same positive 100 MiB recommendation, including its identity, fingerprint, allocation and permanent-cleanup eligibility. Before/after audits passed. The directory fixture contains 240,106 entries, including 40,004 directories; the source control contains 1,001,106 entries, including 1,000,102 files.

| Suggestions workload | Warm runs per version | Median wall, before → after | p95 wall, before → after | Median CPU, before → after | Peak RSS, before → after |
| --- | ---: | ---: | ---: | ---: | ---: |
| Directory-heavy | 5 | 942.917 → 925.450 ms | 949.902 → 939.253 ms | 0.930 → 0.910 s | 3.172 → 3.094 MiB |
| Million-file control | 5 | 333.406 → 341.487 ms | 358.379 → 344.712 ms | 0.320 → 0.330 s | 3.125 → 3.109 MiB |
| Million-file follow-up | 20 | 362.411 → 359.466 ms | 401.104 → 398.161 ms | 0.350 → 0.350 s | 3.172 → 3.141 MiB |

The directory median improved about 1.9%, and all five paired rounds favored the candidate. The initial source-control regression remains visible. Its larger follow-up had essentially identical mean elapsed time, **367.992/367.996 ms**, and identical mean recorded CPU, **0.357 seconds**. This supports keeping a small directory improvement without claiming a general traversal speedup. CPU readings have 10 ms granularity. p95 is nearest rank, the maximum with five warm runs; first invocations are not controlled cold-cache measurements. First eligible output stayed below 9.1 ms at warm p95 in each candidate series.

Reproduce with `scripts/benchmark-suggestions.py`, preserving both binaries, using the directory/million fixtures documented below and the warm-run counts above. The first harness attempt rejected a private baseline copy lacking its execute bit before any scanner ran. Its bytes were verified, that copy's mode was corrected and the rejected preflight log was retained. No fixture repairs or slower-run exclusions were made.

All **210 Rust tests**, formatting, Clippy with warnings denied and both native disposable modes passed. The new path-equivalence regression compares bytes across Unicode, non-UTF-8, trailing separators and long components. The native flows retain restart summaries, review/restore safety, immediate cleanup acknowledgment and exactly-once rewards. Their permanent cleanup credited 276,828,160 bytes and earned two coins from disposable files. The measured CLI is `3e8049aeefa1beb736f9d052ff1538878a152e20699270ca54b71b66b993dcda`; baseline is `3c1fded145fa9847cca1c472606b7188a41e3ad5d4c2da27dcca33ed94635ea2`. The signed native executable is `252c1732feec3aa7eae5b1da3312fdae6feeb4e5a8b53eb2bf55d3a7f47ddefb`. Reports, profiles, reviews and hashes remain under ignored `benchmarks/local/active-cost`.

## Scan summaries after restart

The engine now loads the last terminal foreground summary once at startup, separately from the latest incremental scope statistics. This lets the existing native publication filter retain stable scan totals after a process restart. It does not require another full scan, add per-snapshot queries or write the summary during background jobs. A revision invalidates the saved result atomically when grants or rules change. Saved JSON is limited to 64 KiB; malformed optional cache data cannot prevent access to the library. Coverage and cleanup eligibility still come from their authoritative state and live revalidation.

A native disposable test completed a **36-entry** scan, processed a **two-entry** child refresh, closed the engine, waited for its process lock to release and reopened it. The reopened engine retained the exact 36-entry foreground result while raw stored scope statistics contained two entries. A fresh presentation model then received **20 raw counter updates with zero UI invalidations** and an unchanged displayed count. All 33 source files and the empty ledger were preserved. This is a real FFI restart plus a model-publication check; it is not a measured frame-time or idle-CPU improvement.

All **209 Rust tests**, formatting and Clippy with warnings denied passed. Both native disposable modes passed. New regressions cover resumed incremental work without a full rescan, actual full-root recovery, cancelled/failed summaries, grant/rule invalidation and rollback, bounded malformed-cache handling, and failed summary writes leaving completed work and the ledger intact. A write-count trigger observed one terminal summary update across the initial scan, child event, reopen and another child event. The trigger runs only in a regression test, never in the benchmark.

The same 256-scope event benchmark used five warm runs per version plus a separately recorded first invocation, alternating archives. All **12 invocations** passed exact coverage, positive-candidate identity/fingerprint, unchanged fixture and empty-ledger checks. Setup and the initial terminal save are outside this event-only timing.

| Warm event processing | Before | After |
| --- | ---: | ---: |
| Median wall | 390.508 ms | 385.378 ms |
| p95 wall | 394.076 ms | 430.577 ms |
| Median CPU | 150.954 ms | 153.580 ms |
| Lifetime peak RSS | 9.344 MiB | 9.484 MiB |

This small control shows no clear event-processing speedup: median CPU was 1.7% higher, and the slower p95 is retained. The change fixes restart behavior and avoids background-only view invalidation after reopening; it does not claim faster traversal. Reproduce with `scripts/benchmark-events.py` as below, using `--warm-runs 5` for this table, and `scripts/test.sh` for the native restart and safety checks. First invocations are not controlled cold-cache measurements.

The verified archive is `0543c0486e965344c5ea562a7b4c06b099d8fcb2b8250bb29d7a7ebbd1538c8e`, against baseline `4ddb8cb035c2a3f71e3a6605ea53b2653f0009319352e9ce4f4c1a5b527d7fc3`. The native executable is `3e73bcecb515f85763ef72167418942d9f5f77367a20fa52ae90fed7c52533a9`; its Rust stamp and stable signing requirement match. Private reports, source hashes and native logs remain under ignored `benchmarks/local/restart-summary`. The first Rust run caught an over-specific new test assertion that confused a missing grant with a missing child entry; it is retained alongside the corrected passing run.

A separate manual restart check retained the saved foreground summary without another full-root pass. This is behavioral validation, not equivalent-workload performance evidence.

## Background event processing

Small event jobs now use one transaction to start a claimed refresh and one to finish it. The initial durable claim remains separate for crash recovery. Coverage, candidate pruning, cancellation requeue and acknowledgment commit together; failed commits retain recoverable work. This removes redundant database commits without weakening SQLite durability or delaying event receipt.

`scripts/benchmark-events.py` compares preserved and current Rust archives through the real FFI. Each invocation starts a fresh library, completes an untimed full scan, then processes 256 typed recursive directory events: half refer to unchanged one-file directories and half to absent paths. Timings include the 150 ms debounce, cursor receipt, journal work and 5 ms snapshot polling. No profiler or transaction hooks are attached. Binary order alternates; the first invocation is recorded separately and is not a controlled cold-cache measurement. p95 uses nearest rank.

| Series | Warm runs per version | Median wall, before → after | p95 wall, before → after | Median CPU, before → after | Peak RSS, before → after |
| --- | ---: | ---: | ---: | ---: | ---: |
| Initial comparison | 5 | 339.591 → 385.813 ms | 413.221 → 425.974 ms | 134.170 → 118.564 ms | 9.391 → 9.328 MiB |
| Follow-up | 20 | 414.708 → 389.545 ms | 490.902 → 457.273 ms | 144.844 → 128.180 ms | 9.406 → 9.281 MiB |

The larger sample used **11.5% less CPU time**, with a **6.1% lower median elapsed time**. The initial elapsed-time regression remains in the report. Latency varies on this live workstation; these results establish a modest event-processing CPU reduction, not a full-scan speedup or resting CPU target. RSS is the lifetime process peak, including setup.

All **54 invocations** passed exact entry coverage, complete foreground state, unchanged authorization and an unchanged positive 100 MiB recommendation, including its identity and fingerprint. The foreground summary remained frozen through background events. Independent before/after audits verified fixture identities, metadata and contents; each synthetic wallet and history remained empty. SQLite integrity, durable cursor receipt and empty pending, active and refresh journals were checked. Driver rejection checks also preserved existing database and rollback-journal sentinels.

Swift keeps raw snapshots current while suppressing view invalidation when only background statistics change and a foreground summary exists. A native regression delivered all **20 raw updates with zero UI publications**. Candidate, wallet, error and control changes still publish; legacy snapshots without a foreground summary retain their previous behavior. This is a publication-count result, not measured rendering time or idle CPU. In that build, a foreground summary lasted only for the process lifetime; restarting with only incremental work did not restore the previous full-scan summary. The subsequent restart work addresses that gap.

All **198 Rust tests**, formatting and Clippy with warnings denied passed. Seven new regressions cover atomic refresh failure and recovery. Both native disposable test modes passed, including actual Trash, restore, permanent cleanup and restart. Permanent cleanup credited 276,828,160 bytes and earned two coins. A final visible native rescan of the 512-member fixture examined all 1,541 entries and delivered its exact expected recommendation in **969.725 ms**, completing in **2.218 seconds** with **2.234 CPU seconds** and **133.672 MiB lifetime peak RSS**. This is one warm observation, not p95 or a relative UI-speed claim.

```sh
python3 scripts/benchmark-events.py \
  --baseline-archive /path/to/preserved/libchippytea_core.a \
  --candidate-archive target/release/libchippytea_core.a \
  --scopes 256 --warm-runs 20 \
  --output benchmarks/local/event-comparison
```

Use a new output directory. The harness creates a guarded disposable fixture and never removes existing files. The measured Rust archive is `4ddb8cb035c2a3f71e3a6605ea53b2653f0009319352e9ce4f4c1a5b527d7fc3`; baseline is `7071eb49bcf33ec61f6cbd84794aad84f1b796ca6b8333d8d19d87f403df9a45`. The signed native executable is `93365ca2699434eb25fee4c552e5d32efd58239f91517c7e5f66074c9bf08e5d`. Reports, source hashes, reviews and native logs are retained under ignored `benchmarks/local/maintenance-cost`.

## Scan completion and workspace matching

A requested scan now finishes when its selected roots finish, even if filesystem events keep the worker busy. The same finite boundary applies to full-root work recovered at startup. Background jobs retain durable coverage work without extending the foreground counter. Native request handoff also survives failed progress reads: an older completed snapshot cannot finish a newly accepted Scan. The existing Refresh button stays available during background maintenance.

The workspace matcher now accepts component stars and root-only declarations without accepting hidden components implicitly. Valid syntax does not bypass cleanup safeguards or guarantee another recommendation.

Two controlled fixtures retained identical eligible paths, identities, fingerprints and sizes across **24 invocations**, alternating binaries with five warm runs each. On 128 members sharing a 1 MiB npm lock, warm median changed from 154.097 to 154.566 ms with unchanged 0.14-second median CPU time. On the million-source-file fixture, median changed from 326.626 to 328.331 ms with unchanged 0.32-second median CPU time and **3.078 MiB peak RSS**. Candidate p95 was 329.768 ms. This pass corrects completion and recognition; it does not claim a faster full traversal. Reproduce with `scripts/benchmark-suggestions.py` and the fixture generators below.

All **191 Rust tests**, formatting and Clippy with warnings denied passed. Nine engine regressions exercise finite multi-root completion, late events, cancellation/restart races, partial failures, startup recovery and worker panic. Both native disposable modes passed, including three injected progress-read failures followed by recovery. Actual native cleanup observed 277,327,872 bytes, credited 276,828,160 bytes and earned two coins. Cleanup acknowledgment was 0.086 ms on the main actor; the maximum heartbeat gap was 30.096 ms. These are model/actor measurements, not compositor latency.

Two visible native runs on the 512-member fixture completed in **2.227 and 2.377 seconds**, using 2.216 and 2.246 CPU seconds. Both examined all 1,541 entries and delivered the exact expected recommendation within 972 ms of requesting the fresh scan. Lifetime peak RSS stayed below 136 MiB. Both observations are retained; two samples do not establish p95. The second run verifies a corrected pre-scan audit label in the harness. The first report's generic label said the audit happened afterward, although its call site already performed it before setup and timing. Both synthetic ledgers and fixture audits passed.

A separate local FFI probe measures only synchronous snapshot requests, including result comparison and freeing. With 500 synthetic rows, median cost fell from 1.704 to 1.097 ms per request after moving the owned response into its envelope. Five warm batches of 300 requests were measured per build; all 24 first/warm batches across empty and 500-row datasets preserved their synthetic contents and zero wallets. This is serialization evidence, not a scanner, native UI or idle-CPU speedup. The event debounce also uses one interruptible deadline wait instead of periodic polling; cancellation and mutation-pause handshakes are tested.

The verified native executable is `b9ea441e0806325f8d727203b5f4af40584f0cd0b9061d747d07fe23eb9be0a6`; CLI is `ac68dc59c59849406ffb6b37707d7239ae672e3a0fe089e233181f37c5662cb9`. The linked Rust archive stamp and stable designated signing requirement match. Synthetic benchmark evidence and the local FFI probe are retained under ignored `benchmarks/local/repeated-work`. Private paths and library contents are not publication artifacts.

## Shared workspace evidence

The scanner now reuses compact npm ownership facts within one scan. Every use still reads and validates the current file, configuration and workspace declarations. Only parsing results are cached by content, with an eight-entry, 8 MiB retained-capacity limit; small locks bypass caching. Cleanup revalidation remains uncached. A separate baseline stack sample identified repeated JSON parsing as a substantial cost on the shared-lock fixture; profiling was not attached to timed runs.

Five warm runs per version, plus one first invocation, alternate binary order on each fixture. Every invocation requires identical eligible paths, identities, fingerprints, sizes and permanent-cleanup eligibility. Independent fixture audits and all **36 timed invocations** passed. Each fixture has exactly one aged 100 MiB opportunity. The workspace fixtures additionally declare its exact expected path, preventing two implementations that both miss it from passing.

| Workload | Warm median, before → after | Warm p95, before → after | CPU p95, before → after | Peak RSS, before → after |
| --- | ---: | ---: | ---: | ---: |
| 128 members, shared 1 MiB npm lock | 357.582 → 148.446 ms | 359.655 → 152.738 ms | 0.350 → 0.140 s | 11.656 → 12.188 MiB |
| 512 members, shared 4 MiB npm lock | 5,615.982 → 2,066.452 ms | 5,683.269 → 2,087.233 ms | 5.650 → 2.070 s | 32.578 → 34.203 MiB |
| 3,001 deep projects, separate small locks | 513.904 → 513.357 ms | 521.215 → 521.853 ms | 0.510 → 0.510 s | 3.641 → 3.703 MiB |

At warm p95, the 512-member fixture finished about **2.7 times faster** with **63% less CPU time**. First eligible output improved from 2,072.149 to 777.261 ms p95. The 128-member fixture improved from 68.403 to 33.349 ms for first eligible output. The small-lock control remained essentially unchanged. Peak process memory increased by about 1.6 MiB on the largest lock fixture, including transient parsing allocations. These are workload-specific process measurements, not a Home-folder or native UI speedup claim. CPU seconds measure total work; an active worker can still show a high instantaneous CPU percentage.

The first small-lock attempt failed its pre-timing audit because old disposable empty directories were missing. No timings were accepted, no marker was relaxed and no files were repaired. A fresh fixture was generated for the control above. All reports, including this rejected attempt, remain under ignored `benchmarks/local/shared-lock`. The two shared-workspace first-invocation pairs were 575.501/308.908 ms and 5,558.817/2,000.765 ms; creation and auditing warm caches, so these are not controlled cold-cache measurements. p95 is nearest rank, the largest of five warm samples here.

The measured cache-only CLI SHA-256 is `56a01c6fc1adc98de7cc0588eea680916e2f5551f7f880943867e7525de15c96`; baseline is `a0acfc2d437fd573635acede7f4a0fa8c41b8d992f71b7e34ffcbb81cb03faf3`. All **165 Rust tests**, formatting and Clippy with warnings denied passed, including live changed-content, workspace/configuration and symlink-substitution checks. Exact source/binary/harness hashes and per-run evidence are retained with the reports.

```sh
python3 scripts/make-workspace-fixture.py /private/tmp/chippytea-workspace-example \
  --members 512 --lock-kib 4096 --age-days 9
python3 scripts/benchmark-suggestions.py /private/tmp/chippytea-workspace-example \
  --baseline-cli /path/to/preserved/chippytea-cli \
  --candidate-cli target/release/chippytea-cli --warm-runs 5 \
  --output benchmarks/local/workspace-example-results
```

Use a new destination. The generator writes local disposable data, reserves free space and never downloads dependencies or removes existing files.

## Previous build verification

The retained local implementation keeps the bounded lockfile cache and typed native snapshots, with the experimental directory path removed. Final Rust CLI SHA-256: `092d3ad616f22e4382c71ef2dac33c7e5666648282907cba49313339b86fa06b`. Final native executable: `f103a7754379a8039fc36ca7730aa561196ca56f6f871ac05ca94cd04826b951`. The bundle's recorded Rust archive matched its linked build, and the existing designated signing requirement passed strict verification.

All **172 Rust tests**, formatting and Clippy with warnings denied passed. Both native disposable test modes passed, including typed-response precision/errors, real Trash, restore conflicts, permanent cleanup, reward collection/restart, access setup and event suppression. Permanent cleanup observed 277,889,024 bytes, conservatively credited 276,828,160 bytes and earned two coins. The earlier native restore test caught directory ctime changing about 53 ms after its Trash identity was captured. The fix permits only historical directory-root ctime drift, retains full contents validation, and adds an exact identity check immediately before restore. Seven disposable regressions verify this boundary; regular files remain strict.

A final labelled native scan on the 512-member fixture completed in **2.218 seconds**, using **2.231 CPU seconds**. It examined all 1,541 entries and returned the exact one expected recommendation with zero errors. The fresh finding occurred at 810 ms inside the engine and was observed at 966.312 ms in Swift. Process lifetime peak RSS was **134.109 MiB**, including untimed setup and the native interface. This is one warm observation, not p95 or a comparison against the earlier native build. The fixture and synthetic reward ledger remained unchanged.

All twenty final cancellation samples cancelled safely and preserved partial coverage. The whole-process upper bound was **11.277 ms p95** and **238.497 ms maximum** after subtracting the requested 40 ms delay. The maximum was the first invocation and includes startup, timer scheduling and shutdown; it was retained. The experiment therefore does **not** establish that every invocation meets the 200 ms target or directly measure the cancellation flag's response time. These results are separate from native button latency.

Exact final hashes, native logs and cancellation reports are under ignored `benchmarks/local/shared-lock`; the native scan is `benchmarks/local/native-scan/final-labelled-workspace`. Interface artwork and the design document were preserved; only benchmark launches show the test label and pause pointer interaction.

## Directory experiment not adopted

A public `fgetattrlist` prototype combined initial directory identity and cloud flags, retaining first/final `fstat` barriers and falling back on the same descriptor only for unsupported attributes. Seventeen helper/integration tests passed, and 24 alternating Suggestions invocations passed identical recommendation proofs and independent audits. Review also found and corrected a macOS 14 invalid-attribute packing issue before execution.

On 40,004 directories, median wall time changed from **952.613 to 937.236 ms**, CPU median from **0.940 to 0.920 seconds**, and warm p95 from **989.814 to 939.935 ms**. Its first invocation was slower: 1,189.894 versus 967.906 ms. On the million-source fixture, medians were **343.939 and 342.873 ms** with identical **0.330-second CPU medians**; warm p95 worsened from **351.081 to 385.886 ms**. Each version had five warm runs plus one first invocation. RSS stayed below 3.2 MiB.

The roughly 1.6% directory median improvement and unchanged source median did not justify another native parser and traversal branch. **This experiment was not adopted.** Its complete source, binary hashes, safety results, slower observations and decision remain in ignored `benchmarks/local/directory-facts`; production retains the previously tested descriptor metadata path.

Separate reader experiments also stayed out of production. Sixty audited timings compared libc, a private raw directory-entry symbol at three buffer sizes, and public names-only bulk attributes at two sizes. The private path's modest gain required an unsupported API, while public bulk attributes were slower on both the directory and source fixtures. Exact coverage checks included forced unknown entry types. A combined full-metadata prototype also returned a directory link count different from `fstat` and was rejected before timing. These experiments are diagnostic evidence, not claims about the retained scanner.

## Native scan and snapshot delivery

`scripts/benchmark-native-scan.py` measures an ordinary, visible Find space rescan in a disposable library. An untimed setup pass establishes cached rows; a one-shot start signal begins process CPU and monotonic wall measurement immediately before the normal Scan action. The result must observe a new active scan and completed scan, exact fixture coverage, the declared eligible paths, a fresh engine finding and an unchanged reward ledger. Cached rows alone cannot satisfy first-finding validation. Hidden or occluded windows, page changes, dialogs, cleanup, errors and incomplete scans invalidate the run.

The typed snapshot bridge removes intermediate JSON deserialization/re-encoding and moves snapshot decoding off the main actor. Three alternating native pairs on the 512-member, 4 MiB shared-lock fixture used the **same Rust archive**. Baseline median wall/CPU was **2.220/2.192 seconds**; typed decoding was **2.236/2.224 seconds**. All six runs passed exact coverage, findings and unchanged-ledger checks. This one-result workload does **not** establish an overall scan speedup from the bridge change; the small CPU regression is retained. Tests also cover exact 64-bit integer precision, malformed/error/foreign envelopes and parity with the previous response path.

Timing ends at final main-actor snapshot delivery, before final presentation assignment and rendering. It includes ordinary progressive UI work and bridge delivery, but not compositor latency. Engine first-finding time and observed snapshot time are reported separately. Snapshot callback gaps do not measure main-thread responsiveness, and the recorded RSS is the process lifetime peak, including setup. Exact binaries, common Rust archive hash, per-run commands and fixture audits are in ignored `benchmarks/local/native-scan`.

```sh
python3 scripts/benchmark-native-scan.py /private/tmp/chippytea-workspace-example \
  --app build/Chippytea.app --output benchmarks/local/native-workspace-example
```

Use a fresh fixture or its existing synthetic library with the same synthetic balance. Expected positive paths come from the workspace fixture marker, or repeated `--expected-eligible` arguments. A negative control requires `--expect-no-findings` explicitly. Profiling must run separately from timing.

Resting-window measurements are a separate protocol. Two attempted before/after comparisons were rejected because the visible test window changed state; the second included cleanup and collection in its synthetic library. Neither produced an accepted resting-CPU comparison, and the changed fixture was not reset or reused. A separate hide-then-reopen exercise proved that the validator rejects an interval even when its final window is visible again. Benchmark windows now carry a visible disposable-files label and ignore pointer interaction with their content; normal application windows keep their interface and controls. The earlier unguarded resting result remains diagnostic only.

## Earlier ancestor and recent-artifact follow-up

The scanner now uses one no-follow absolute directory lookup for evidence reads on macOS, retains one fewer path copy for each discovered directory, and rejects recent supported artifact boundaries before reading their ownership files. These changes preserve recommendation eligibility and cleanup validation. They do not throttle the worker or add parallelism.

Suggestions comparisons alternate binary order and require identical eligible paths, identities, fingerprints, sizes and cleanup eligibility, with independent fixture audits before and after. All **90 invocations** passed. Each comparison includes one first invocation in addition to the warm runs below. p95 uses nearest rank; with five warm runs it is the largest warm observation. File creation and auditing warm filesystem caches, so none is a controlled cold-cache measurement.

| Workload | Warm runs per version | Median, before → after | p95, before → after | CPU p95, before → after | Peak RSS, before → after |
| --- | ---: | ---: | ---: | ---: | ---: |
| 3,001 aged projects at a deep path | 5 | 1,058.472 → 522.077 ms | 1,068.759 → 530.726 ms | 1.050 → 0.510 s | 3.781 → 3.734 MiB |
| 3,000 recent artifacts plus one eligible artifact | 5 | 675.349 → 125.406 ms | 684.255 → 131.584 ms | 0.670 → 0.110 s | 3.641 → 3.078 MiB |
| 40,004 directories, initial check | 5 | 942.169 → 943.332 ms | 979.381 → 1,324.199 ms | 0.960 → 1.290 s | 3.141 → 3.125 MiB |
| 40,004 directories, tail follow-up | 20 | 944.523 → 939.594 ms | 1,034.956 → 1,052.751 ms | 1.020 → 1.030 s | 3.141 → 3.109 MiB |
| Million ordinary source files | 5 | 330.176 → 340.036 ms | 343.830 → 353.926 ms | 0.330 → 0.340 s | 3.109 → 3.078 MiB |
| Full metadata discovery, 1,016,001 entries | 3 | 3,344.805 → 2,417.253 ms | 3,372.640 → 2,478.366 ms | 3.350 → 2.460 s | 3.813 → 3.797 MiB |

The two project-evidence fixtures used about **51% and 84% less CPU time per scan** at warm p95. The source and directory fixtures do not establish an improvement; the initial directory outlier prompted the larger follow-up, and every slower observation remains in the report. The follow-up's directory median was nearly unchanged and p95 was about 2% slower. The benefit is workload-specific, and lower CPU seconds per scan does not promise a lower instantaneous Activity Monitor percentage.

The full metadata comparison used a separate alternating sequence. All eight runs matched the independently audited 1,016,001 entries and 1,008,000 files, with zero errors and complete coverage. Its first invocations were 4,603.272 and 2,504.970 ms. The Suggestions first invocations, per-run measurements and first-finding timings are retained in the local reports. Deep-project first eligible findings were 81.596 ms p95 after the change; the recent-artifact fixture was 28.705 ms, and the source-million fixture was 8.708 ms. These fixtures each contain a deliberately eligible 100 MiB artifact.

Twenty cancellation runs all passed: 9.047 ms p95 and 12.954 ms maximum for whole-process wall time minus the requested 40 ms cancellation delay. This includes startup, timer scheduling and shutdown; it does not bound a blocked filesystem operation or native button delivery.

The measured Rust CLI is `193ecb0d7d0baa29f417341a1037f15e0e9f6b2adfad66019b746d10df753b84`, compared with `5691b08f42d95268ed907217f2a432131f502ab9e5554be54f98191f8f73e3a3`. Results, exact commands, binary/source hashes, audits and raw outputs are under the ignored `benchmarks/local/ancestor-open` directory. The measured scanner passed 152 Rust tests, formatting and Clippy with warnings denied. The subsequent exact commit-message event exclusion passed all 155 Rust tests, including cursor/restart and mixed-event regressions; it is separate from these initial-traversal timings.

Reproduce the new recent-boundary fixture in a **new** directory:

```sh
python3 scripts/make-fixture.py /private/tmp/chippytea-recent-example \
  --files 100 --files-per-project 100 --bytes-per-file 1048576 \
  --recent-empty-projects 3000 --age-days 9 --no-cases
python3 scripts/benchmark-suggestions.py /private/tmp/chippytea-recent-example \
  --baseline-cli /path/to/preserved/chippytea-cli \
  --candidate-cli target/release/chippytea-cli --warm-runs 5 \
  --output benchmarks/local/recent-example-results
```

The recorded deep-path fixtures place that generated directory eight levels below their temporary parent. Use `--empty-projects 3000` instead for the aged evidence workload. The generator never reuses an existing directory or removes files.

## Historical directory traversal follow-up

Known directory names now go directly to a no-follow open and authoritative descriptor metadata. The same descriptor survives classification and queued traversal. Readers initialize only when needed, with an identity recheck before the first read. Full metadata enumeration no longer constructs an unused libc names reader. The [reference study](REFERENCE-STUDY.md#names-and-types-before-full-metadata) explains the syscall costs and safety boundaries. No worker threads, cache or recommendation rules were added.

The new directory-heavy fixture contains **240,106 entries and 40,004 directories**, including 200,000 ordinary files distributed five per directory and one aged 100 MiB developer artifact. The existing source-heavy fixture contains 1,001,106 entries. These comparisons alternate binary order, audit unchanged fixtures and require identical eligible paths, identities, fingerprints, sizes and cleanup eligibility. All 66 invocations passed, with identical examined entry counts and zero errors.

| Suggestions workload | Warm runs per version | Median, before → after | p95, before → after | CPU p95, before → after | Peak RSS, before → after |
| --- | ---: | ---: | ---: | ---: | ---: |
| 40,004 directories | 5 | 984.050 → 968.732 ms | 1,000.916 → 993.280 ms | 0.980 → 0.970 s | 3.188 → 3.141 MiB |
| Million source files, initial check | 5 | 335.319 → 333.880 ms | 342.141 → 435.134 ms | 0.330 → 0.390 s | 3.063 → 3.094 MiB |
| Million source files, follow-up | 20 | 355.227 → 359.479 ms | 390.688 → 431.778 ms | 0.380 → 0.410 s | 3.141 → 3.125 MiB |

The directory-only gain is modest. The removed pathname lookup is partly offset by the delayed first-read identity check, and traversed directories still require their initial names read. The million-file follow-up investigated a slower tail in the initial check; it retained a roughly 1% median and 11% p95 regression. These results do not establish a general Suggestions speedup. No slower observations were discarded. First invocations were 1,116.339/1,165.580 ms for the directory fixture, 348.709/353.784 ms for the initial million-file check and 441.018/430.191 ms for its follow-up. They are not controlled cold-cache measurements. The 20-run candidate's first eligible result was 9.593 ms p95.

A separate read-only C prototype compared libc names with minimal `getattrlistbulk` attributes and an 8 KiB buffer, sharing directory validation and exact audited coverage. Five alternating pairs on each fixture retained every invocation, including the first. Median wall time was 1.43/1.57 seconds for the directory-heavy fixture and 0.44/1.93 seconds for the million-file fixture, names/minimal-bulk respectively. That configuration was slower on both topologies and was not adopted. This is a bounded reader experiment, not production scanner timing, proof of permission behavior or a conclusion about every possible bulk configuration. Source, commands and raw observations remain in `benchmarks/local/directory-traversal/minimal-probe`.

Full metadata scanning benefits more from removing unused reader setup. The independent fixture audit matched **1,016,001 entries, 1,008,000 files, 8,001 directories and 32,768,000 allocated bytes**. Every Chippytea run reported exact entry/file counts, complete coverage and zero errors. The following are sequential tool groups, each with one first invocation and three warm invocations; the preserved baseline was run afterward on the same unchanged fixture. p95 is nearest rank, and RSS covers all four invocations.

| Full metadata workload | Workers | First invocation | Warm p95 | CPU p95 | Peak RSS |
| --- | ---: | ---: | ---: | ---: | ---: |
| Chippytea raw traversal, before | 1 | 2,970.952 ms | 2,014.386 ms | 1.990 s | 2.766 MiB |
| Chippytea raw traversal, current | 1 | 7,814.148 ms | 1,781.483 ms | 1.770 s | 2.656 MiB |
| Chippytea metadata discovery, before | 1 | 5,217.529 ms | 4,465.480 ms | 4.420 s | 3.984 MiB |
| Chippytea metadata discovery, current | 1 | 4,603.918 ms | 3,330.238 ms | 3.300 s | 3.859 MiB |
| Apple du | 1 | 5,526.909 ms | 5,715.358 ms | 5.700 s | 2.703 MiB |
| dua 2.44.0 | 4 | 1,323.164 ms | 1,410.551 ms | 5.530 s | 11.703 MiB |
| dust 1.2.5 | 4 | 3,839.829 ms | 3,948.427 ms | 15.090 s | 397.906 MiB |

The current metadata-discovery observations used about **25% less CPU time** and completed about **25% sooner** at warm p95. Raw traversal used about 11% less CPU time. dua remains faster than Chippytea's single worker on this workload, at higher aggregate CPU cost. Raw traversal is the equivalent inventory comparison; discovery also checks project evidence and recommendation policy. All tools received the same audited root. dua's statistics matched 1,016,000 entries excluding the root, and its total allocation matched 32,768,000 bytes. du and dust also reported that allocation but expose no comparable entry count in these commands. This does not prove every tool visited identical paths. The harness now parses dua's aggregate total instead of its first child row; original local timing reports remain unchanged, with corrected extraction recorded separately.

Twenty cancellation runs on the directory-heavy fixture all reported partial cancellation with zero errors. Whole-process wall time minus the requested 40 ms delay was **9.064 ms p95** and **13.010 ms maximum**, including launch, timer scheduling and shutdown. This is an upper bound, not native button latency or a bound on a blocked filesystem operation.

All **144 Rust tests**, formatting, Clippy with warnings denied and both native test modes passed. Native disposable cleanup credited 276,828,160 bytes and two coins. Confirmation acknowledgment took 0.041 ms; 46 main-actor heartbeat samples had a maximum 25.134 ms interval. Forty warm native window openings measured **11.469 ms p95** through synchronous layout/display, excluding click delivery and compositor presentation. A subsequent 30.002-second closed-window interval recorded no CPU-time increase at `ps`'s 10 ms resolution, approximately 0.034% of one core. The app database was inside its watched disposable root; scan state and pending work remained unchanged. These native observations do not establish idle behavior for every Home workload.

The main interface files matched their preserved baselines. The current bundle also satisfies the previous certificate-signed bundle's designated requirement, preserving the stable app identity needed by the [Full Disk Access flow](DISK-ACCESS.md). Actual System Settings consent remains a separate manual check.

Current CLI SHA-256: `5691b08f42d95268ed907217f2a432131f502ab9e5554be54f98191f8f73e3a3`. Baseline: `1f0bab5f105a4462b7561e2fbc03dab87c08c9a360f5f17bcd6a2da1d38d4ce2`. Native executable: `0ed609d0b21006de89b484a2e15b5931f3cc6332da48244681d995b3c8f083a7`. Local evidence is under `benchmarks/local/directory-traversal`: `directory-comparison`, `million-comparison`, `million-followup-20`, `full-metadata-million`, `before-full-metadata-million`, `comparison-tools`, `cancellation`, `native` and `validation-final.log`. Profiling, fixture creation, builds and native tests ran separately from CLI timings. These are local measurements on a workstation in normal use, not a best-in-class claim.

## Earlier evidence-reader and access follow-up

Project evidence now returns file contents and their validated identity together. A successful manifest read and fingerprint previously walked its absolute ancestors three times; it now walks them twice, pinning the parent during the read and verifying the original path afterward. The change retains ownership, size, cloud, link and replacement checks, adds cancellation between ancestor opens and bounded reads, and preserves the fingerprint format. It adds no cache or worker threads.

The new fixture has **3,001 Node project boundaries, 6,002 evidence files and 12,105 entries**, beneath eight additional ancestor directories. One aged artifact contains 100 MiB of written payload; the other 3,000 artifacts are empty and intentionally ineligible. The existing million-source-file fixture exercises the ordinary-file fast path separately. All comparisons alternate binary order, audit their fixtures before and after, and require identical eligible paths, identities, fingerprints, sizes and cleanup eligibility on every run. Both versions also examined identical entry counts with zero errors.

| Workload | Warm runs per version | Warm p95, before → after | CPU p95, before → after | Peak RSS, before → after |
| --- | ---: | ---: | ---: | ---: |
| 3,001 project boundaries | 5 | 1,749.294 → 1,406.788 ms | 1.720 → 1.390 s | 3.891 → 3.812 MiB |
| Million source files, initial check | 5 | 381.046 → 427.726 ms | 0.370 → 0.420 s | 3.172 → 3.156 MiB |
| Million source files, follow-up | 20 | 710.791 → 566.218 ms | 0.510 → 0.500 s | 3.219 → 3.141 MiB |

The project-heavy fixture used **19% less CPU time** and completed about **20% sooner** at warm p95. Its first eligible finding improved from 260.565 to 203.178 ms p95. First invocations were 2,431.899 and 1,605.836 ms; they are not controlled cold-cache measurements.

The million-file results are mixed. The initial candidate's first warm observation was its slowest, despite a slightly better median. A 20-warm-run follow-up was added to investigate that regression; both implementations then experienced higher and more variable timings. Its medians were 460.395 and 449.913 ms, with CPU medians of 0.445 and 0.435 seconds. These observations do not establish a consistent speedup or regression in ordinary-file enumeration. All 66 invocations across the three comparisons passed their recommendation proofs and fixture audits; none of the slower observations were discarded. p95 uses nearest rank, and RSS covers first and warm invocations.

The [access correction](DISK-ACCESS.md) also keeps a stable certificate identity between builds, checks intended folder access without prompting, narrows legacy Home grants before resuming, and skips Home media folders. Actual consent in System Settings remains a manual check, separate from the disposable permission and startup regressions.

All **136 Rust tests**, formatting and Clippy with warnings denied passed. Both native test modes passed on the final bundle, including failed permission and bookmark writes, interrupted setup, grant migration, Trash, restore conflicts, permanent removal and reward persistence. The disposable permanent cleanup received 276,828,160 credited bytes and two coins. The interaction test acknowledged confirmation in 0.047 ms and recorded 55 main-actor heartbeats with a maximum 25.099 ms interval; these are model measurements, not compositor latency. The main interface files matched their preserved baselines.

Final native executable SHA-256: `40bc3a13b7e325bd940496939584422f4e8d67add991ef11a341c81ab35459a9`. The rebuilt bundle satisfies the same designated signing requirement as the earlier certificate-signed build. Validation logs: `evidence-reader/validation.log` and `evidence-reader/native-final-validation.log` under `benchmarks/local`.

Measured CLI SHA-256: `1f0bab5f105a4462b7561e2fbc03dab87c08c9a360f5f17bcd6a2da1d38d4ce2`. Baseline: `a11ffd17e6255869371c9b3f1858094cb4682455d1bb3273100794f87751ad51`. Evidence is local under `benchmarks/local/evidence-reader`: `deep-comparison`, `million-regression` and `million-followup-20`. The qualitative `before-profile.sample.txt` confirmed repeated manifest ancestor opens on the preserved baseline; its sampled fractions are not whole-run timings. Reproduction commands below include `--empty-projects` for the new workload.

## Earlier names-first comparison

The app now enumerates names and file types before fetching full metadata. Ordinary files outside Downloads and recognized artifacts cannot become recommendations, so discovery counts them without reading sizes, timestamps or allocation, or allocating a full path for each file. Directories and unknown types still receive authoritative no-follow metadata. Eligible artifacts retain complete measurements and cleanup revalidation. The [reference study](REFERENCE-STUDY.md#names-and-types-before-full-metadata) records the Apple, Kondo, dua and dust research behind this choice.

The fixed comparison fixture contains **1,001,106 entries**: one million empty ordinary source files in 1,000 directories, one aged Node artifact with 100 MiB of written payload, its manifest/lockfile and containing directories. Both binaries run `Suggestions` mode. Every timed run must complete without errors and return identical eligible paths, identities, fingerprints, sizes and cleanup eligibility. Independent audits before and after timing matched the marker; all twelve runs passed. The optimization does not change the recommendation policy.

| Million-file Suggestions scan | First invocation | Warm p95, 5 runs | CPU p95, 5 warm runs | First eligible p95 | Peak RSS, all 6 runs |
| --- | ---: | ---: | ---: | ---: | ---: |
| Previous scanner | 2,221.677 ms | 2,183.370 ms | 2.170 s | 9.603 ms | 3.344 MiB |
| Names-first scanner | 459.452 ms | 382.299 ms | 0.370 s | 9.132 ms | 3.078 MiB |

On this fixture, warm p95 is **5.7 times faster**, with **83% less CPU time** and slightly lower memory. Both versions examined the same entry count and returned the same 104,857,600-byte opportunity. The new version omitted full metadata for 1,000,002 ordinary files. Its byte totals describe measured entries; they are not a whole-folder disk-usage total.

The harness alternates baseline/candidate order each round and records exact binary and harness hashes. These are whole-process readings from `/usr/bin/time -l`. Creation and the initial audit warm caches; neither first invocation is a controlled cold-cache result. p95 uses nearest rank, which is the largest of five warm observations here. These measurements establish a local improvement over the previous Chippytea discovery algorithm, not superiority over exhaustive inventory tools or a guarantee for every directory topology.

A separate 100,206-entry fixture passed the same twelve-run comparison: warm p95 improved from 177.008 to 51.170 ms and CPU p95 from 0.160 to 0.040 s. The candidate's first invocation was slower, 232.055 versus 159.558 ms, although its in-process scan took 38 ms. That launch overhead is included rather than discarded.

Twenty cancellation runs on the million-file fixture all reported cancellation, partial completion and zero scan errors. With a cancellation request after 40 ms, whole-process time minus that delay had an **11.507 ms p95 upper bound**, with a **14.538 ms maximum**. This includes process startup, timer scheduling and shutdown; it is not a timestamp of the cancellation flag or a native button-latency measurement.

Final CLI SHA-256: `2ce359ed14d3d716a61445ae4e5b8a251bee1e3544ba25fd9118028fb01b79b3`. Baseline: `46b4178cf05f3079ebc01f32057785f7a9979e92accb21520001eb69cd0fd83a`. Local evidence: `benchmarks/local/suggestions-million-names-first`, `suggestions-names-first`, `suggestions-million-cancellation` and `cleanup-latency`.

## Cleanup and presentation, earlier measurements

Initial or explicitly requested scans show foreground progress. Background event refreshes retain the settled screen and reserve the status-control geometry. Native state tests exercise repeated background pulses, a pending explicit request, fast completion and cancellation. Cleanup confirmation immediately closes review and suppresses the selected rows until authoritative results arrive; a failed refresh does not make deleted rows reappear. Progress reads use an independent mutex and utility queue, so a database transaction cannot block feedback.

Cleanup now streams its durable manifest during full revalidation and reuses prepared statements. This reduces full pre-deletion walks from four to two while retaining staged fingerprint verification and every per-leaf identity check. A 32,768-payload-file disposable experiment took 5.195 seconds before and 4.817 seconds after; the latter overlapped another scan, so this is not a controlled deletion speedup claim. Physical removal still depends on file count and filesystem latency.

The final native interaction test acknowledged confirmation in **0.044 ms** on the main actor, delivered checking/removing/accounting progress and recorded 51 heartbeats at a requested 20 ms interval; the largest interval was **25.159 ms**. This measures model acknowledgment and main-actor availability, not compositor presentation. The test actually removed its 4,096-small-file artifact plus a 100 MiB payload and preserved its sibling. No recovery or rewards were fabricated while the operation was pending.

All **124 Rust tests**, formatting, Clippy with warnings denied, and both native test modes passed. The separate native Trash/restore/permanent/restart test earned **275,736,150 credited bytes and two coins** from disposable files. The final native executable SHA-256 is `4cd09af75ee5d584eed21a0f93af9c3bc04965a985280b90c4c849b93756566c`. Evidence: `cleanup-latency/final-validation-v3.log` and `cleanup-latency/final-native-validation-v3.log`.

The idle audit also identified a Home-watch feedback path: persisting an event cursor changes SQLite/WAL files inside the watched Home directory. Native and callback-level exclusions now omit only the app's private state directory, suppressing state-only callbacks while retaining external changes and lost-history signals. The native regression writes a synthetic cursor after every callback and checks that it settles, including the fallback beyond the system's eight-exclusion limit. A similarly prefixed external directory still delivers creation and deletion events. The test caught Foundation rewriting physical `/private/var` paths to the `/var` alias; exclusions now retain the stream's physical path spelling.

The final native bundle was then run with its actual SQLite database inside the authorized disposable watch root. Forty warm window openings measured **5.844 ms p95** through synchronous layout/display, with one visible app window. After five seconds settling, a **30.012-second closed-window interval recorded 0.00 CPU seconds** at `ps`'s 10 ms resolution, a detection bound of approximately 0.033% of one core. The durable event cursor was unchanged. This excludes click delivery and compositor presentation, and does not establish long-duration idle behavior on an actively changing Home folder. Evidence: `cleanup-latency/native-state-exclusion/window.json`, `idle.json` and `discover.png`.

Earlier native probes remain recorded under `cleanup-latency/native-final`: the first 20-second post-window interval used 0.04 CPU seconds (0.20%); later 30-second settled background and closed-window intervals each recorded 0.00. Three-second stack samples showed parked threads and no recurring hidden animation. Those shorter observations were not silently discarded or treated as proof of the separate Home-watch feedback cause.

## Historical reward and CPU regression follow-up

Earlier investigation found three additional causes of poor behavior:

- Reward accounting required exactly stable free-capacity samples, so ordinary writes could reject otherwise recoverable space. It now uses conservative before/after bounds and reserves measured ambient growth. Older receipts lack the observations needed to assign new credit safely, so historical accounting is not retroactively changed.
- Ordinary file events were promoted to their parent directory; missing temporary files climbed to an existing ancestor. A shell-history write could therefore trigger a Home scan. Typed events now filter unrelated writes and directory metadata noise, while verified deleted scopes reconcile exactly.
- Discovery fully traversed artifacts already excluded by ownership, activity or age. The app now prunes these in `Suggestions` mode and stops at the first disqualifying descendant. Counts describe examined entries and excluded artifact contents. Explicit `MetadataCoverage` retains exhaustive traversal within policy boundaries.

The final explicit million-file coverage check examined **1,016,001 entries**, reported zero errors and used **4.14 MiB peak RSS**. Its 17.640-second wall time overlapped native window testing; this run verifies coverage and memory, not comparative speed.

All **112 Rust tests** and both native integration/access tests passed. A disposable cleanup containing 2,048 small files plus a 256 MiB payload received **276,828,160 credited bytes and two coins**, with durable restart behavior. Forty warm native openings measured **12.970 ms p95** through synchronous layout/display, excluding click delivery and compositor presentation. A closed-window, event-free 20.004-second interval used 0.01 CPU seconds at `ps`'s 10 ms resolution, approximately **0.05% of one core**. Ten ordinary file writes and one removal left scan state unchanged. These native checks used a disposable-folder index; they do not establish long-duration Home idle acceptance.

Evidence is local under `benchmarks/local/reward-cpu`: `million-coverage.summary.json`, `final-validation-v3.log` and `native-probe.json`. Final CLI SHA-256: `46b4178cf05f3079ebc01f32057785f7a9979e92accb21520001eb69cd0fd83a`; initial pruning CLI: `7b24508feacd8f83a07e48dda1675bdc908952e57f98a6d16b89475a34b3e679`; native executable: `1fb689c9f7180c0bd4338c85239857e3ed8ad7f1ba5a7f1b704326da4e18c98a`.

## Historical scoped-refresh corrections

Exploratory diagnostics identified three full-refresh amplifiers: the 65th pending sibling event, a batch-wide interpretation of per-path FSEvents flags, and any new event after partial coverage. These paths are covered by scoped-queue and partial-coverage regression tests; partial coverage no longer schedules a full pass by itself. Personal live-folder inventories and timings are excluded from this document, because changing inputs and uncontrolled cache state do not support reproducible comparisons.

That scanner revision added Bun and supported workspace evidence and permitted generated bundles and leaf symlinks within verified developer output. Hardlinks, protected/cloud data, active projects, recent content and ambiguous ownership remained withheld. At that time, fresh or blocked artifacts still received a deferred metadata pass; this is now exclusive to `MetadataCoverage` mode. The cached index avoids repeating initial discovery on ordinary reopen.

## Historical native window and idle checks

The updated signed development bundle was exercised against a separate disposable-folder index. Forty warm panel openings measured **10.324 ms p95** from `makeKeyAndOrderFront` through synchronous layout/display work, with one visible app window. This excludes external click delivery and compositor presentation, so it does not claim click-to-photon latency.

After the benchmark called the ordinary window-close model path and hid its panel, a 20.017-second interval with no filesystem events recorded **0.00 seconds of process CPU increase** at `ps`'s 10 ms resolution (approximately a 0.05% one-core detection bound). This is a short, event-free fixture observation, not a long-duration Home-folder acceptance run. The visible animated panel was not used for the idle result.

The native disposable integration test passed real Trash, durable history, restart, conflicting restore, narrower/broader authorization and confirmed permanent cleanup. The access-flow test passed explicit setup, app-identity changes, restart and legacy Home gating without probing protected folders. Rust's 84 tests passed, including permanent cleanup while the discovery worker is parked, external symlink target survival, cursor persistence, partial coverage, 1,000 sibling scopes and stale-batch suppression. Clippy with warnings denied and the release build passed.

Local native evidence: `window-final.json`, `idle-final.json`, `native-safety-final.log`, `native-access-final.log`, and `review.png` under `benchmarks/local/live-tuning`.

## Historical million-file comparison

The table and references to the final run in this section describe the earlier `release-million` build, not the latest executable identified above.

Fixture: 1,000,000 empty regular payload files across 4,000 Node projects, plus 8,000 manifest/lock files and 8,001 directories: **1,016,001 entries**. Empty payloads isolate metadata cost and memory; they do not simulate the allocation or age of a useful cleanup opportunity. An independent post-run traversal matched the fixture marker.

Whole-process wall time, one first invocation followed by three warm invocations. p95 uses the nearest-rank rule; with three warm samples it is the largest warm sample. RSS is the maximum across all four invocations. Chippytea uses one traversal worker; dua and dust use four.

| Workload | Baseline warm p95 | Final first run | Final warm p95 | Final peak RSS |
| --- | ---: | ---: | ---: | ---: |
| Chippytea metadata traversal | 6.606 s | 2.769 s | 2.680 s | 2.66 MiB |
| Chippytea discovery and classification | 8.965 s | 4.213 s | 3.104 s | 3.52 MiB |
| Chippytea engine with a new SQLite index | — | 3.571 s | 4.046 s | 12.12 MiB |
| macOS `du` | — | 7.242 s | 12.751 s | 2.69 MiB |
| dua 2.44.0 | — | 3.643 s | 3.585 s | 11.89 MiB |
| dust 1.2.5 | — | 8.141 s | 4.578 s | 397.75 MiB |

The final discovery scan measured **2.9× faster** than the original baseline on this fixture (8,965.245 ms → 3,104.008 ms warm p95); its underlying metadata traversal measured **2.5× faster** (6,605.593 ms → 2,679.709 ms). These are observed ratios across runs on a workstation in normal use. The final indexed-engine run takes 4,045.605 ms warm p95 with 12,713,984 bytes peak RSS. It creates a fresh disposable SQLite database, authorizes the fixture, scans, persists candidate batches and reads snapshots every 150 ms. It is not a cached-index reopen benchmark. All Chippytea modes remain below the 256 MB scanner-memory target on this fixture.

All 24 final million-file timed invocations succeeded without a timeout. Chippytea's three modes each reported complete coverage of 1,016,001 entries with zero errors; an independent audit matched the marker. Discovery adds classification and recommendation checks; the indexed mode also includes persistence and engine snapshots. Fresh, tiny artifacts intentionally avoid full fingerprint and Git/process work, so this metadata fixture does not represent a million files that all qualify for cleanup.

The sequential tool runs show substantial variance. In the preceding `benchmarks/local/final-million` report, warm p95 was 5,772.048 ms for `du`, 970.423 ms for dua and 3,552.460 ms for dust, compared with 12,751.018 ms, 3,584.891 ms and 4,578.314 ms above. That earlier, apparently less-contended pass also measured Chippytea metadata at 2,077.517 ms, discovery at 3,202.901 ms and indexed discovery at 3,590.743 ms. Contention was not independently measured. **This sequential run does not establish superiority over another tool**; a controlled, interleaved comparison is needed for that claim.

An earlier optimized run remains in `benchmarks/local/after-million`: metadata warm p95 1,871.430 ms and discovery warm p95 3,077.050 ms. The table uses the subsequent `release-million` observations rather than selecting the fastest earlier run; all of these timings are historical.

All tools receive the same root, include hidden entries, do not follow symlinks and stay on one filesystem. Allocated totals include directory allocation in the raw traversal comparison. Discovery's opportunity totals count candidate file allocation separately. Kondo recognizes supported projects and intentionally prunes other work, so it is studied as a design reference rather than represented as equivalent full coverage.

**These are first-run and warm-cache measurements. No controlled cold-cache result is claimed.** Fixture creation and earlier tool runs warm caches. The harness does not purge system caches, force memory pressure, unmount user volumes or require administrator access.

## Historical useful first result

A separate fixture contains one Node artifact with 100 MB of written payload, 102 regular files total, and synthetic modification times set nine days earlier. Independent metadata coverage measured 105 entries and 100,360,192 allocated bytes including the evidence files. Over 20 warm scans with the final executable, the p95 time from process launch to a **completed eligible suggestion** was **9.620 ms**; whole-discovery p95 was **10.195 ms**. A provisional measuring row does not count as a finding.

With a fresh SQLite index and engine snapshot polling, the completed eligible suggestion appeared at **167.665 ms p95**, and the whole process finished at **171.222 ms p95**. This includes the CLI harness's 150 ms snapshot polling interval. Both modes produced a completed eligible finding in all 20 warm samples and passed fixture coverage checks. These are CLI observations, not native window or compositor timings.

The preceding `final-quality` report measured 9.010 ms first-eligible p95 and 9.554 ms whole-discovery p95; the older `quality-latency` report measured 9.982 ms and 10.687 ms. Both remain historical references. The final numbers above come from `benchmarks/local/release-quality`.

The million-file metadata fixture correctly produces no suggestions: its payloads are empty and newly created. The benchmark must not advertise that diagnostic output as actionable cleanup.

## Historical cancellation

Twenty sequential scans of the million-file fixture requested in-process cancellation after 40 ms. All 20 exited successfully with `cancelled=true`, `complete=false`, `errors=0`, and no timeout or validation failure. The binary hash and fixture marker remained unchanged.

The measured bound is **whole-process wall time minus the requested 40 ms delay**. It includes startup, timer scheduling, cancellation handling, output and shutdown. It does not timestamp the instant the Rust cancellation flag changes and does not measure the native Cancel button. On this fixture, the bound was **9.369 ms p95**, **8.6215 ms median**, and **9.425 ms maximum**; all 20 bounds were below 200 ms. These measurements do not establish the same latency for blocked filesystem operations or other storage devices.

`scripts/benchmark-cancellation.py` records the exact command, hardware, executable hash, selected final scan statistics and validation failures in a new local JSON report. It refuses unmarked or incomplete fixtures, never changes the fixture, and does not persist raw child stdout/stderr. Results: `benchmarks/local/release-cancellation/summary.json`. The preceding `final-cancellation` report measured 9.150 ms p95 and 9.325 ms maximum on its earlier executable.

## Implementation

- A 64 KiB `getattrlistbulk` buffer batches macOS metadata syscalls. Packed fields, record bounds and returned-attribute masks are checked. Directories, links, special entries and missing fields use anchored no-follow `fstatat` semantics.
- Unsupported initial bulk calls restart on a fresh descriptor. Failures after progress report partial coverage rather than restarting and double-counting.
- A bounded shallow directory frontier prioritizes nearby project boundaries, with depth-first overflow and a depth limit. The scanner streams metadata, retains only multi-link identities in a capped set, and does not retain a million-node tree.
- `Suggestions` mode prunes ineligible artifact contents and stops at a disqualifying descendant. Explicit `MetadataCoverage` completes their metadata traversal; benchmark and cancellation harnesses request it explicitly.
- Names-first discovery avoids metadata and path allocation for ordinary files outside measurement boundaries. Its buffered steps cap work between worker checkpoints at 256 names, with an atomic cancellation check between entries. The cancellation harness also supports `--mode suggestions` to exercise this path directly.
- Candidate batches contain at most 64 rows and progress is published around every 100 ms. Progress-only batches do not write candidate transactions.
- The persistent index has a partial recommendation-rank index and a `(root_id, path)` index. Snapshot reads reuse a bounded cache until a candidate or Keep change. Scoped invalidation does not deserialize every row in the root.
- Typed filesystem events enter a disk-backed queue in bounded batches and resolve paths on the worker. A newly acquired background worker waits at most 600 ms to combine queued events; explicit Scan, Resume and captured full-root recovery bypass that wait. Later events never extend its deadline. Unrelated file and directory-metadata noise is filtered before queuing; missing paths reconcile exactly and arbitrary event counts do not widen scopes. Completed candidates remain visible during remeasurement. Cancellation pauses durable work until an explicit scan or resume; idle discovery has no polling loop.

## Reproduce

```sh
./scripts/build.sh
python3 scripts/make-fixture.py benchmarks/local/million --million --no-cases
python3 scripts/benchmark.py benchmarks/local/million --warm-runs 3 --include-index
python3 scripts/benchmark-cancellation.py benchmarks/local/million \
  --samples 20 --cancel-after-ms 40

# A new directory outside a containing Git checkout keeps fixture ownership unambiguous.
python3 scripts/make-fixture.py /private/tmp/chippytea-quality-example \
  --files 100 --bytes-per-file 1000000 --age-days 9 --no-cases
python3 scripts/benchmark.py /private/tmp/chippytea-quality-example \
  --warm-runs 20 --include-index

# Save a previous release CLI before rebuilding the candidate. Use a new fixture path.
python3 scripts/make-fixture.py /private/tmp/chippytea-suggestions-example \
  --files 100 --bytes-per-file 1048576 --files-per-project 100 \
  --source-files 1000000 --age-days 8 --no-cases
python3 scripts/benchmark-suggestions.py /private/tmp/chippytea-suggestions-example \
  --baseline-cli /path/to/previous/chippytea-cli \
  --candidate-cli target/release/chippytea-cli --warm-runs 5
python3 scripts/benchmark-cancellation.py /private/tmp/chippytea-suggestions-example \
  --mode suggestions --samples 20 --cancel-after-ms 40

# Many project boundaries exercise manifest checks and repeated ancestor lookup.
# The added empty artifacts are deliberately ineligible; the original payload
# still supplies a completed recommendation for strict proof comparison.
python3 scripts/make-fixture.py /private/tmp/chippytea-evidence-example/one/two/three/four/five/six/seven/eight \
  --files 100 --bytes-per-file 1048576 --files-per-project 100 \
  --empty-projects 3000 --age-days 8 --no-cases
python3 scripts/benchmark-suggestions.py /private/tmp/chippytea-evidence-example/one/two/three/four/five/six/seven/eight \
  --baseline-cli /path/to/previous/chippytea-cli \
  --candidate-cli target/release/chippytea-cli --warm-runs 5

# Many small directories exercise per-directory opening and reader setup.
python3 scripts/make-fixture.py /private/tmp/chippytea-directories-example \
  --files 100 --bytes-per-file 1048576 --files-per-project 100 \
  --source-files 200000 --source-files-per-directory 5 --age-days 8 --no-cases
python3 scripts/benchmark-suggestions.py /private/tmp/chippytea-directories-example \
  --baseline-cli /path/to/previous/chippytea-cli \
  --candidate-cli target/release/chippytea-cli --warm-runs 5
python3 scripts/benchmark-cancellation.py /private/tmp/chippytea-directories-example \
  --mode suggestions --samples 20 --cancel-after-ms 40
```

Fixture generation refuses existing paths and enforces a 3 GiB free-space reserve. It never removes existing files. Optional `dua` and `dust` binaries must be on `PATH`; the harness records missing tools instead of installing them. Local reports include hardware, tool versions, commands, binary SHA-256, raw process output, CPU time, RSS, fixture counts and coverage checks. Those local artifacts contain absolute paths and are ignored by Git.

Earlier fixture measurements are in `benchmarks/local/release-million`, `benchmarks/local/release-quality`, and `benchmarks/local/release-cancellation`. The `release-*` names identify local benchmark artifacts; they do not indicate a notarized or distribution-ready app. All three reports identify the same earlier CLI SHA-256:

`817448414526fc3a2867636286a453e64f47f327d5320d7c03b9686ef98e7ae4`

The preceding `final-million`, `final-quality` and `final-cancellation` reports identify CLI SHA-256 `588f1eb85336e86d9001431fe8bd88027b027f83d23dcb06677360690f1f3c92`.

The earlier reports remain in `benchmarks/local/before-million`, `benchmarks/local/after-million`, and `benchmarks/local/quality-latency`. Their executable SHA-256 values are, respectively, `926985ae746515fb19a15d7d5176ce79520e6ba3efb81a08db8aa1c50abf95a0`, `199208773a9da23fc27a771e9c2542cc1795c792d11534acf84e849ba387d581`, and `72bbef464b50c2c462b9c2cf904eae68139b52c35045a049a1372719443a1869`. The repository had no resolvable Git HEAD in these reports; executable hashes identify the measured builds.

Native window presentation, compositor latency, hidden-app memory, and settled CPU are separate measurements. They are not inferred from CLI timings. This performance pass preserves the existing visual interface.
