# chippytea: original storage optimizer proposal

Product proposal · 31 August 2026

> Historical design, not current release documentation. The app has since been implemented and parts of this proposal have changed. Start with the [README](README.md), [current recommendation policy](docs/SUGGESTIONS.md), and [architecture](docs/ARCHITECTURE.md).

The accompanying concept used fictional files, space measurements and coin balances. This proposal made coin collection the main experience, before native filesystem behaviour, scanning and performance were tested.

## Product

An open-source, local macOS app where freeing storage earns coins for a personal collection. Finding useful cleanup opportunities, recovering space and collecting the resulting coins form the main product loop. The collection is the main screen, not a notification added to a conventional cleanup dashboard.

chippytea lives in the menu bar. Clicking its icon opens a compact panel attached directly beneath it; clicking outside or pressing Escape dismisses the panel and leaves the menu-bar app running. Quit is explicit. There is no permanent Dock icon by default.

The first audience is developers with several local projects, alongside ordinary Mac users looking through large downloads. The app should be understandable without knowing what a cache or worktree is.

In this first release, coin earning is supported only for recognized developer artifacts removed through eligible permanent cleanup. Personal files remain available for inspection and Move to Trash, but neither that operation nor later cleanup in Finder earns coins.

## Core loop: find space, earn coins, collect

1. A scan finds eligible opportunities and shows their estimated space alongside the consequence of removal. Reward estimates are operation-specific: the default **Move to Trash shows 0 coins**. Eligible permanent developer-artifact removal may show a potential coin amount, explicitly conditional on space accounting accepting the recovery. Personal files show no earning opportunity.
2. The user reviews the exact files and chooses the cleanup operation. Selecting a reward never authorizes deletion.
3. A completed cleanup goes through space accounting. Only eligible space recovery creates an earned coin entry. Trash, failed operations and unresolved recovery create no spendable reward.
4. Earned coins are collected automatically when the moment can be seen: a cleanup that finishes while the panel is open switches to the Coins screen, plays a short ascending arcade sound and counts the balance up as coins fall into the pile. A cleanup that earns nothing (such as Move to Trash) is acknowledged quietly, with no sound and no celebration.
5. If the panel is closed when credit lands, a small coin badge on the menu-bar icon indicates that a collection is ready and the collection plays on the next opening. All paths consume the same earning entries exactly once.

The proposed rate is **1 cleanup coin per 100 MB credited** (decimal bytes), with fractional progress carried forward across eligible cleanups. Coins have no monetary value. The UI shows estimated, earned/uncollected and collected values distinctly. Cosmetic milestones can be explored later; the MVP needs the stash, a good collection animation and clear receipts.

Use the phrase **space credited** for the amount the reward policy accepted, with its measurement details in the receipt. The coin balance is a game total, not a promise of exact lifetime physical disk recovery. A real fresh installation starts at zero. The concept's preloaded stash and ready-to-collect earnings are explicitly sample data.

## First release

### 1. Choose where to look

The primary path is one click: **Scan my Mac** authorizes the user's home folder and starts scanning immediately (the app is not sandboxed, so no picker is needed; macOS may still show its own one-time permission prompts for protected folders). Choosing specific folders with the system picker remains available as the secondary path, and Settings offers an optional, clearly explained shortcut to grant Full Disk Access for users who want zero permission prompts. Persist access records; never require Full Disk Access or an administrator helper for the core experience.

Only scan approved roots. Show which locations were scanned, when the scan happened, and any inaccessible or excluded portions. An incomplete scan must never look complete. Initial scope is local storage; skip network volumes and cloud placeholders without downloading their contents. Known cloud-managed roots and items are excluded from cleanup even when fully downloaded, since deletion can synchronize to other devices. Uncertain cloud ownership means inspection only.

### 2. Two ways to find space

**Suggestions:** a short list grouped by project or source folder, ordered by evidence and removal consequence, then size. Each row has a path, size, a plain explanation, and what the user should expect afterward.

Initial rules:

| Candidate | Required evidence | Consequence shown |
| --- | --- | --- |
| Node project dependencies | Real package manifest, recognized lockfile, physical dependency directory, unambiguous project ownership | Dependencies need reinstalling; network access and compatible installation options may be needed |
| Cargo build artifacts | Recognized Cargo project/workspace and verified default artifact location | Recompilation is required; compiled binaries in the selected directory will also be removed |
| Older installers in Downloads | Regular local installer/package/disk-image files in an approved folder | Individually review; age does not prove the installer is unwanted |

Exclude observed active installs/builds from developer cleanup. Treat shared dependency stores, linked directories, nonstandard Cargo output locations, tracked content, and unclear ownership as manual inspection rather than cleanup suggestions. Any Git-tracked descendant disqualifies the entire artifact directory, and this check runs again immediately before Trash or permanent deletion; unknown or failed Git checks are not evidence that a directory is untracked. Initial developer adapters support only cases they can positively recognize.

**Large files:** a simple sortable list of large regular local files, with Quick Look, path, type, modification date and Reveal in Finder. A starting threshold of 100 MB keeps the list useful; the threshold is a product default, not a safety boundary. Large files are discoveries, not automatically unwanted data.

Do not preselect personal files. The first release starts with nothing selected. Users can pin a project or file as Keep so that it stays out of future suggestions.

### 3. Review and clean

Cleaning a single suggested item confirms inline: the row expands to show the exact item, estimated size, consequence and any blocked state, and a second explicit tap performs the operation through the same validation path as the full review. Multi-item selection leads to one review screen showing the exact items, locations, reported size and removal consequences. In both cases the default operation is **Move to Trash**. It uses the native macOS Trash API and records the destination returned by the system.

For users who need space immediately, provide a separate **Delete permanently** choice in the review screen, with an explicit irreversible confirmation. In v1 this option is restricted to eligible developer artifacts; personal files go through Trash and Finder. Never silently switch from a failed Trash operation to deletion. chippytea does not empty unrelated Trash contents.

Immediately before either operation, revalidate root access, volume/file identity, object type, path containment, applicable manifests, recorded directory contents and activity signals. Changes or ambiguity invalidate that item and require a refreshed review. Do not follow symlinks during traversal or removal, cross into a different volume, or operate on overlapping parent/child selections twice. Races must be handled conservatively; these checks are not a claim of zero future risk.

The result shows successful, skipped and failed items individually. Cancellation stops before the next item; it does not promise to reverse an operation already completed.

### 4. History and recovery

Keep a small local ledger: operation, time, original identity/path, resulting Trash location where applicable, outcome, reported bytes and storage observations. Write an operation record before mutation, then its result; reconcile interrupted operations on next launch without replaying deletion.

Offer Restore only after revalidating the recorded Trash item's identity and type, destination authorization and containment, and the identities of destination parent directories without following changed symlinks. A conflict at the original path requires a choice; never overwrite an existing replacement. Permanently deleted items have no Restore action. Reinstalling dependencies is not restoration of their previous contents. If an item disappears from its Trash location, its final disposition is unknown unless independently established.

## The scanning and ranking algorithm

Use a deterministic, versioned rule engine. No model inference is required to scan, rank or clean.

1. **Discover cheaply.** Stream directory metadata and small, relevant project manifests inside approved roots. Prefetch filesystem resource keys. Do not read every file's contents or hash the whole drive.
2. **Classify before traversing deeply.** Find supported artifact directories and ordinary large files. Count an artifact once; do not also recommend its nested files. Avoid descending into app bundles, photo libraries, backups, Git object storage and unsupported package internals.
3. **Measure progressively.** Calculate totals on a background worker and publish useful partial results. Keep a disk-backed index and a bounded set of visible rows in memory. Overlapping selected roots and hard links must not be double-counted.
4. **Rank in understandable groups.** First exclude blocked/ambiguous cases; then group by consequence (reinstall, recompile, personal review); then use size and observed activity to order candidates. Activity and dates are signals, not proof that something is unused. Show the reason, not an invented safety percentage.
5. **Refresh what changed.** FSEvents marks indexed directories dirty; reconcile those paths in debounced batches. Start observing before a scan and reconcile events received during it. Dropped events, identity changes or incomplete coverage trigger a scoped rescan. No periodic full-disk crawl while idle.

The important optimization is doing less I/O and keeping work off the UI thread. More scanner threads are not automatically faster. Start with one utility-priority worker and measure bounded concurrency on representative disks.

The cleaner expansion adds an explicit, bounded duplicate check for already-indexed old/large personal-file suggestions. It selects whole same-device/size buckets independently of the visible results page, then uses samples, full digests and exact byte comparison with file-stability checks. Ordinary discovery remains metadata-only. This is not whole-drive duplicate detection: smaller, newer or otherwise unindexed files are outside its coverage. The user chooses one keeper and one copy for a Trash-only review; both are rechecked before the move. Matching content alone does not establish that either path is unnecessary, and APFS sharing complicates space estimates.

## Storage numbers and the reward

Keep these separate:

- **File size:** logical content length.
- **Size on disk:** allocated storage reported by the filesystem, when available.
- **Operation result:** bytes associated with items moved or removed.
- **Available space:** the operating system's capacity measurement, with its measurement basis kept consistent.

Allocated-size sums are estimates of opportunity, not guarantees of reclaimable space. Hard links, APFS clones, snapshots, compression, purgeable storage and concurrent writes affect the actual result. Do not add available space from volumes sharing the same APFS container.

Moving files to Trash generally frees no storage until they are removed from Trash. The result must say **Moved to Trash**, not **Space freed**. After permanent cleanup, show the observed change in available space separately; do not attribute every concurrent capacity change to chippytea or maintain a fictitious exact lifetime savings total.

The reward policy must be conservative: require a successful eligible removal and supporting storage observations; bound credited space by the successful items' allocated-size estimates and the observed capacity increase, and withhold credit when attribution is unclear. This bound alone does not prove causality on APFS. Snapshots, shared blocks and competing disk activity may leave a result checking or uncredited. A capacity increase alone never generates coins, and disappearance from a Trash path never proves permanent deletion.

Allocate credit through durable, exclusive observation windows for each APFS container, or each independent non-APFS volume. Only one accounting window may be open for that storage domain; multiple cleanups in the window share one before/after measurement and one allocation budget. Each cleanup belongs to at most one window, and each observed capacity increase can be allocated only once. Across all cleanups in a window, credited bytes must not exceed its positive observed capacity change or the combined successful items' allocated-size bounds. Each individual credit remains subject to its own item bound and attribution checks. Unknown storage-domain identity or overlapping observations mean credit is withheld.

Persist the window, its observations, participating cleanup identifiers and credit allocations. Commit an allocation and its earning entry atomically. A delayed checking result can use only the remaining unallocated budget of its original window; it cannot claim a later capacity increase. On restart, reconcile those records with operation outcomes before issuing rewards. Do not reconstruct missing observations from current free space or issue replacement credit for an already allocated increase; unresolved accounting remains uncredited.

Persist reward entries separately from file operations, using a unique cleanup identifier and an explicit uncollected/collected state. Transfer an earning entry to the bank atomically before playing its cosmetic animation. Reopening, restarting, double-clicking or interrupting an animation must not award the same entry again. Carry fractional bytes in the same transaction so small cleanups are not repeatedly rounded up or discarded.

Coin amounts come from space credited, never from item count, scanning, moving to Trash or merely attempting deletion. The previous idea of awarding a celebratory collection for moving files to Trash is removed. Keep unsafe or personal data out of suggested earning opportunities, and do not encourage repeatedly deleting/recreating useful caches. No streaks, countdowns or bonuses that pressure the user to delete more.

The collection uses a finite burst of representative coin particles, a smooth balance count-up, a small landing bounce and a short rising chime. Larger earnings can make the same brief animation fuller without generating one particle per coin or extending it indefinitely. Include sound-off and Reduce Motion support. Nothing animates while the window is closed; a completed credit remains durable if its animation is interrupted.

## Window and visual direction

One compact panel, 380 points wide and about 620 tall, hanging under the menu-bar icon like a notebook page held up by a strip of tape. The panel is borderless and fixed-size; the menu-bar icon toggles it, click-outside or Escape dismisses it, and system dialogs opened from the panel do not dismiss it. The full design system is recorded in `docs/DESIGN.md`.

Three destinations in a slim bottom bar: **Coins**, **Find space**, **Activity**, plus small Settings: selected folders, exclusions, sound and optional launch at login.

The visual language is hand-drawn: cream notebook paper, wobbly ink outlines that move like frame-by-frame animation, scribble-shaded gold coins, hand-lettered numerals for the balance, and one ballpoint-blue accent. No uppercase label typography. Coins is the home screen: a large hand-lettered coin balance, a panel-wide coin pile, credited space, and any collection waiting — collected coins tumble from under the tape into the pile. Under it, show a few upcoming cleanup opportunities with operation-specific reward labels: Move to Trash shows 0 coins; a potential reward for eligible permanent developer-artifact removal is clearly conditional and does not change the selected operation. Personal-file opportunities carry no coin estimate. Disk capacity and technical detail are secondary. Find space contains the reviewable file list; Activity contains cleanup and reward receipts. Review is a full-panel takeover; keep all deletion consequences visible in the review flow.

The interaction is: choose folders → find space → review → cleanup → account for recovered space → collect coins. A demo mockup accompanies this proposal and uses fictional files and sizes.

## Native architecture and performance

For this macOS-only MVP, start with SwiftUI, an AppKit menu/window coordinator, a background Foundation scanner and SQLite. The earlier Swift/Rust proposal concerned a portable multi-provider token engine. This scope benefits from direct access to native file metadata, permissions, Trash and Quick Look.

Keep scanning, classification and persistence behind narrow interfaces so a Rust engine can be introduced if profiling establishes a benefit. Language choice alone is not a performance result. Do not add a privileged daemon, web UI runtime or always-running model.

Proposed acceptance targets, not measured results:

| Behavior | Target |
| --- | --- |
| Warm window opens from cached state | Usable within 150 ms |
| Initial scan starts producing partial results | Within 1 second on the benchmark fixture |
| UI update rate during scan | Batched every 100–200 ms |
| Cancellation acknowledged on ordinary local storage | Within 200 ms between batches; no promise for an OS call already blocked |
| Idle CPU after settling | Below 0.1% in a defined measurement window |
| Hidden-app memory | Below 100 MB |
| Scan peak memory | Below 256 MB on a documented one-million-file fixture |

Measure cold and warm scans separately on Apple Silicon and an older supported Mac. Do not promise a fixed whole-drive scan time.

## Release scope and proof

Start on macOS 14+ and Apple Silicon, with a signed/notarized release before distributing a normal installer. An OSS license is a release decision; MIT is a reasonable default for original code, with dependencies reviewed individually.

The MVP does not include antivirus, RAM cleaning, generic system-cache purges, app uninstallation, browser history deletion, cloud cleanup, worktree deletion, whole-drive or automatic duplicate removal, or automatic background deletion. The cleaner expansion's reviewed duplicate check is limited to existing personal-file suggestions. Other capabilities remain separate decisions, not hidden first-release requirements.

Validate discovery and cleanup against fixtures covering linked/shared files, changed directories, inaccessible roots, active builds, cloud placeholders, partial failure, cancellation, crash recovery, Trash renames and restore conflicts. Prove that excluded paths are not mutated. Then verify the native picker → scan → review → cleanup → history flow using disposable real files. A convincing MVP must find useful candidates, remain responsive and describe every operation honestly; a larger junk total is not the success criterion.

## Sources

- [Apple: free storage and Trash behavior](https://support.apple.com/en-gb/102624)
- [Apple: native Trash operation](https://developer.apple.com/documentation/foundation/filemanager/trashitem(at:resultingitemurl:))
- [Apple: allocated size](https://developer.apple.com/documentation/foundation/urlresourcekey/totalfileallocatedsizekey)
- [Apple: APFS behavior](https://developer.apple.com/documentation/foundation/about-apple-file-system)
- [Apple: file access and bookmarks](https://developer.apple.com/documentation/security/accessing-files-from-the-macos-app-sandbox)
- [Apple: filesystem performance](https://developer.apple.com/documentation/foundation/improving-performance-and-stability-when-accessing-the-file-system)
- [Apple: FSEvents](https://developer.apple.com/library/archive/documentation/Darwin/Conceptual/FSEvents_ProgGuide/UsingtheFSEventsFramework/UsingtheFSEventsFramework.html)
- [Apple: accessory app activation](https://developer.apple.com/documentation/appkit/nsapplication/activationpolicy-swift.enum/accessory)
- [npm: clean installs and compatibility requirements](https://docs.npmjs.com/cli/v11/commands/npm-ci/)
- [Cargo: build cache and artifact locations](https://doc.rust-lang.org/cargo/reference/build-cache.html)
- [pnpm: shared and linked dependency layout](https://pnpm.io/symlinked-node-modules-structure)
- [Kondo: existing artifact-cleanup rules and library](https://github.com/tbillington/kondo)
