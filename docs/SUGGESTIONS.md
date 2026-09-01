# Recommendation policy

Chippytea indexes observations separately from the suggestions shown in the app. An indexed path is not a recommendation or permission to remove it. The public snapshot contains at most 500 completed, eligible suggestions; the coin screen uses the first three in the same order.

## What earns a place in the list

| Kind | Required local allocation | Quiet period | Evidence and consequence |
| --- | ---: | ---: | --- |
| Cargo `target` | 100 MB | 7 days | A valid Cargo package/workspace and a standard target marker. No custom or ambiguous output ownership. Removing it requires recompilation and removes compiled binaries too. |
| npm / Yarn classic / Bun / supported pnpm `node_modules` | 100 MB | 7 days | A real package manifest and recognized lockfile; workspace members require declared membership and lock evidence. Dependencies need reinstalling; network access and compatible options may be required. |
| Downloaded installer (`dmg`, `pkg`) | 100 MB | 14 days | A regular local file in an explicitly authorized Downloads scope. Review it yourself; its age does not establish that it is unwanted. |
| Other large download | 100 MB | 30 days | Same location and local-file rules. Inspection and native Trash only; no permanent cleanup or coins. |

MB means 1,000,000 bytes. The threshold uses allocated size, not a sparse file's apparent length. Developer quiet periods use the newest modification time across descendants and ownership/configuration evidence, not just the artifact directory's date. Future dates fail the quiet-period check. These dates are evidence for ranking, not proof of disuse.

Workspace declarations support literal path components, ordinary `*` within one component (such as `plugins/plugin-*`), whole-component `**`, explicit negations and root-only `.`. Wildcards do not include hidden components unless those components are explicitly named. Unsupported glob syntax remains a diagnostic. Declared membership still requires exact package-manager lock evidence and fresh ownership checks; it never overrides shared-store, activity or size restrictions. The pnpm interpretation follows its [package discovery](https://github.com/pnpm/pnpm/blob/v10.15.0/fs/find-packages/src/index.ts#L62) and [workspace discovery](https://github.com/pnpm/pnpm/blob/v10.15.0/workspace/find-packages/src/index.ts#L48) behavior.

When a home folder is explicitly authorized, only its direct `Downloads` subtree becomes a personal-file scope. A directory called `Downloads` inside an unrelated project does not qualify. Files outside Downloads are not turned into personal cleanup suggestions. Unfinished downloads and download bundles are excluded before descent.

Home discovery skips its immediate Music, Pictures and Movies folders before reading their metadata. A project such as `Projects/Music` is still supported. Photos and Music library bundles are protected wherever they appear; finding one inside an artifact disqualifies that artifact from cleanup. This keeps personal media outside the default cleanup scope and avoids unnecessary permission requests.

## What stays out

- Incomplete measurements, tiny artifacts, recently modified contents and unverified ownership.
- Observed running projects, applications executing from a project's build output, tracked descendants, failed Git checks, and unknown process-activity results.
- Cloud-managed or protected locations, shared stores, external or unverified hard links, nonstandard Cargo output, unverified package layouts and protected content. Within a validated developer artifact, generated `.app`, `.framework` and `.bundle` directories are supported. Symlinks are fingerprinted and removed as links; targets are never traversed, counted as recovered, or deleted. Strict traversal outside those artifacts retains its protected-bundle rules.
- Anything marked Keep. The scanner skips kept subtrees, and Include again requests a fresh scoped scan.

Regular hard links within a developer artifact qualify only after a complete measurement proves that every alias stays inside that artifact, with consistent identity, metadata and link counts. Bytes count once per inode. A link outside the artifact, a changing group or an exhausted proof limit excludes the entire artifact. Cleanup rechecks containment and verifies each link-count change during removal; only the final alias can contribute private allocation to the existing conservative recovery accounting. Hard-linked Downloads and hard-linked symlinks remain excluded.

Activity detection observes process working directories and executable paths. It cannot prove that no process has an arbitrary file descriptor or command-line reference to a project. Mutation therefore also requires fresh identity, ownership and full metadata-fingerprint checks.

Standard linked Git worktrees qualify when their project and repository metadata are inside the same authorized root and volume. The `.git` pointer, common directory, backpointer, index and configuration must be consistent, owned and local. Evidence reads are bounded, with index/configuration files limited to 4 MiB. Metadata symlinks, configuration includes, split/sparse or compressed indexes and unfamiliar layouts remain excluded. Tracked artifact contents are always excluded, including in supported worktrees.

## Ordering and freshness

Completed Cargo artifacts come first, then dependency installations, then downloads. Within each group, larger allocated opportunities come first; the path breaks ties deterministically. The database maintains this order in a partial index. Unsafe or provisional diagnostics cannot displace a useful suggestion.

Typed filesystem events schedule refreshes for affected artifacts, ownership/configuration evidence and Downloads. Ordinary file writes elsewhere and unrelated directory metadata changes do not start scans. Structural directory changes retain their exact subtree. A short debounce and persistent ancestor coalescing combine work without widening after an arbitrary event count. Verified deleted scopes remove their stale index entries without scanning a parent. `MustScanSubDirs` and root-change events apply to their own paths; genuinely dropped history requires a full authorized-root reconciliation. An unrelated event does not expand just because another part of Home was inaccessible.

The policy runs again immediately before cleanup. A change while a review is open requires a new review. No item is preselected. Eligible developer cleanup leads with permanent deletion, explicit confirmation and separate conservative space accounting. Native Trash remains an optional recovery route and earns zero coins. Downloads remain Trash-only. Completed findings stay reviewable during scanning; mutation parks the walker and resumes its saved frontier afterward.

A diagnostic can become old enough without generating a filesystem event. **Scan again** reevaluates age thresholds; there is no periodic background crawl just to promote old items. The event cursor records durable receipt, not successful coverage. Pending and active scopes survive interruptions; inaccessible areas remain visibly partial without replaying the whole event history. Cancel pauses automatic work until an explicit Scan or Resume.

## Coverage and tests

The app's `Suggestions` mode skips artifact contents already excluded by ownership, age or observed activity. Otherwise, measurement stops at the first unsafe or recently modified descendant. These paths remain ineligible; unvisited contents are not included in examined-entry or size counts. Progress reports examined entries and excluded artifact contents, and completion means suggestion discovery within those policy boundaries.

Explicit `MetadataCoverage` mode (`chippytea-cli scan <folder> --metadata-coverage`) retains exhaustive metadata traversal within the same authorization and protected-path boundaries. It defers ineligible artifacts behind useful discovery and drains that bounded queue before completion. Metadata-only diagnostic measurements never authorize cleanup. Equivalent traversal benchmarks use this mode or the raw traversal command; suggestion-mode counts must not be presented as exhaustive coverage.

Tests cover allocation versus sparse size, exact age cutoffs, recent descendants/configuration, Downloads boundaries, unfinished downloads, active processes, tracked files, Keep, path replacements, and a real allocated 100 MB fixture whose timestamps are deliberately aged. All mutations in the test suite are restricted to disposable fixtures.
