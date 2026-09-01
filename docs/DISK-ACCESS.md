# Scan access on macOS

“Scan my Mac” opens scan setup in the existing notebook interface before scanning the home folder: three pages you click through, each with one drawing, one headline, one sentence and one button. Choosing individual folders remains available on every page.

1. **Permission.** Why Full Disk Access is needed and what stays untouched. **Open System Settings** opens Privacy & Security → Full Disk Access, saves the waiting intent and moves to the next page.
2. **Drag it in.** A looping sketch shows the app card being carried into the Full Disk Access list and dropped; the real card under it is the drag source. **Reveal in Finder** supports adding the exact running copy with **+** instead. If setup identifies an earlier app build, the page says to remove that stale chippytea entry first.
3. **Switch it on.** A sketch flips the switch beside chippytea and answers the **Quit & Reopen** prompt. Setup resumes on this page after reopening. **It’s switched on — scan my Mac** checks the intended folders and starts the Home scan. If access is still denied, the page stays open with the affected folder names; no scan begins and completion is not saved.

The four UI phases are `intro`, `openingSettings`, `waiting` and `starting`. The page is separate presentation state (`diskAccessStep`); a saved waiting intent always resumes on the last page. Opening Settings saves the waiting intent atomically before handing focus to macOS. Returning focus only releases the hold that keeps chippytea visible during a system dialog; it does not start a scan. The explicit confirmation button calls `confirmDiskAccessAndScan`; saved waiting intent always resumes in `waiting`.

Only the user can grant Full Disk Access in macOS. Apple documents the system grant in [Privacy & Security settings](https://support.apple.com/guide/mac-help/change-privacy-security-settings-on-mac-mchl211c911f/mac).

## Permission handling

Apple does not expose a supported API for reading the Full Disk Access toggle. Apple DTS advises handling errors from the intended operation and warns that the TCC database is not an API. See [Reliable test for Full Disk Access?](https://developer.apple.com/forums/thread/114452).

Opening or enumerating a protected folder to test access can itself trigger a consent prompt. Instead, after the user's confirmation, chippytea uses the public nonprompting `access` API on Desktop, Documents and Downloads, off the main thread. Apple describes these per-path authorization checks in [WWDC19, Advances in macOS Security](https://developer.apple.com/videos/play/wwdc2019/701/). Missing optional folders are allowed; denied folders keep the app in setup. A remembered Home setup is checked again before startup resumes its scan. Failed confirmation invalidates previous completion, so Back cannot restore a denied setup.

These checks establish access to the intended paths, not the state of the global Full Disk Access toggle. chippytea never queries or modifies TCC, opens unrelated protected files as sentinels, or polls permission state while idle. The saved completion record remains the user's confirmation, tied to this app identity. The Rust scan reports actual coverage and errors.

## Stable identity across builds

The reported repeated prompts were traced to macOS rejecting the old ad-hoc code requirement for `SystemPolicyAllFiles`, then requesting individual access such as Photos. Rebuilding an ad-hoc app changes its code hash; an enabled-looking Settings entry can therefore belong to an older build.

The build supports `CHIPPYTEA_SIGNING_IDENTITY` or an ignored `.swiftpm/chippytea/signing-identity` file. A configured certificate must succeed; signing never silently falls back. Keep the same bundle identifier, signing identity and distribution channel between updates. Apple explains how remembered privileges follow the designated requirement in [TN3127](https://developer.apple.com/documentation/technotes/tn3127-inside-code-signing-requirements). Switching from the old ad-hoc build requires removing its stale entry and approving the new signed app once. That is a migration repair, not an extra permission stage after each scan.

## Scope and persistence

The confirmation authorizes the current user's Home with an explicit `home` policy for supported developer artifacts and reviewable Downloads. Home's immediate Music, Pictures and Movies folders are excluded; a normal project such as `Projects/Music` remains discoverable. Photos and Music library bundles are protected anywhere. Suggestion traversal filters these directory names before metadata reads outside Downloads. Incremental events, replayed scopes and old cleanup candidates use the same Home boundary. Downloads and artifact measurements retain full metadata, with protected bundles excluded from descent and protected artifact descendants invalidating cleanup.

Every cleanup still requires review. The [recommendation policy](SUGGESTIONS.md) also excludes Library, system locations, cloud stores, ambiguous data, active projects and unsafe links. Scanning stays within authorized locations and on the authorized filesystem. A legacy `folder` Home grant is narrowed to `home` on successful confirmation or startup with a matching completed setup. The transaction replaces only that same physical grant, invalidates derived candidates and queues a replacement scan while preserving history, Keep and rewards. Queuing in the same transaction makes this upgrade resumable after interruption.

Switching from selected folders to Home replaces contained grants on the same filesystem in one SQLite transaction. Independent mounted-volume grants remain separate because scanning stays on one filesystem. Keep choices, Trash history and rewards survive the change. Restore requires a current same-volume authorization and revalidates both the current grant and the original recorded root identity.

The native bookmark for a broader grant is saved before that grant reaches the engine. `disk-access.json` records setup intent separately from authorization. Completing setup saves that confirmation before starting the ordinary Rust scan. A failed Home authorization keeps its older grant gated, including when bookmark persistence fails after confirmation. Back dismisses setup and cancels its pending transition.

Legacy home grants without completed setup also pass through this gate at startup and on Refresh. Watchers stay stopped during access restoration and while an unconfirmed Home grant exists, including watchers for other roots: an event can resume the engine's entire durable queue. Dismissing setup does not bypass the gate. Choosing a specific folder instead can revoke an unconfirmed legacy home grant and replace its scan scope with the selected folder. This changes authorization and the discovery index only: user files, Keep choices, Trash history and rewards remain intact. Access and scanning earn no coins.

## Verification

```sh
./scripts/build.sh
build/chippytea.app/Contents/MacOS/chippytea --access-flow-test
build/chippytea.app/Contents/MacOS/chippytea --self-test
```

The access-flow test injects a new disposable home and application-data directory. It covers saved waiting intent, dismissal, returning focus without automatic scanning, denied folder access before and after prior completion, successful confirmation, the Home policy, automatic legacy-grant narrowing and scan resumption, failed bookmark persistence, startup/Refresh/filesystem-event gates, and the selected-folder fallback. It checks that fixture contents remain unchanged. The test simulates reaching the waiting phase and denies a disposable directory with ordinary permissions; it does not operate System Settings or prove an actual TCC grant.

The native integration test (`--self-test`) exercises native Trash and receipt restoration after broadening and narrowing authorization, including restart and conflict refusal. Both test modes use disposable files and never grant Full Disk Access or scan the developer's real home.

The screenshot harness supports `disk-access`, `disk-access-add`, `disk-access-waiting` and `disk-access-starting` through `CHIPPYTEA_SCREENSHOT_STATE`, one per page; `CHIPPYTEA_SCREENSHOT_SCENE_TIME` freezes that page's sketch at a moment of its loop, in seconds, and the `-reduceMotion 1` argument captures the still versions. Use a fresh `CHIPPYTEA_DATA_DIR` and `--screenshot`; these static states neither grant access nor probe folders or start scans.

Actual consent in System Settings and its Quit & Reopen prompt remain manual verification on the intended signed app. The configured local build uses a stable certificate; unconfigured contributor builds remain ad-hoc. Distribution still requires its own hardened-runtime, timestamp and notarization verification.

The saved setup also records the bundle path and designated code requirement. A different installation or changed ad-hoc signature returns to the upfront setup before any Home scan; it does not reuse a stale attestation and trigger a series of protected-folder prompts. This is an identity check, not a query of the Full Disk Access toggle. A stable production signature keeps its designated requirement across compatible updates.
