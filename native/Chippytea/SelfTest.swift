import Foundation
import Combine
import CoreServices
import SQLite3

enum NativeSelfTest {
    @MainActor static func runAccessFlow() async {
        do {
            if CommandLine.arguments.contains("--forget-summary-regression") {
                try await forgottenRootInvalidatesScanPresentation()
                print("PASS native forgotten-root presentation regression")
                exit(0)
            }
            if CommandLine.arguments.contains("--watcher-regression") {
                try watcherEventDecoding()
                try watcherCursorCoalescing()
                try await watcherExcludesState()
                print("PASS native watcher decoding, cleanup filtering and state exclusion")
                exit(0)
            }
            if CommandLine.arguments.contains("--background-cleanup-regression") {
                try await storageStatusSampling()
                try await cleanupInteraction(includeDiscoveryRegressions: false)
                print("PASS native background cleanup: immediate collection, recovery, cancellation, quiet handoff and storage sampling")
                exit(0)
            }
            if CommandLine.arguments.contains("--cleanup-queue-regression") {
                try await cleanupQueueInteraction()
                print("PASS native cleanup queue: bounded FIFO, cancellation, frozen reviews and confirmed rewards")
                exit(0)
            }
            try await accessFlow()
            try await foregroundSummarySurvivesRestart()
            try await forgottenRootInvalidatesScanPresentation()
            try await warningPresentation()
            try await acceptedScanSurvivesSnapshotFailures()
            try watcherEventDecoding()
            try watcherCursorCoalescing()
            try await watcherExcludesState()
            try await ignoredEventsDoNotPublish()
            try await backgroundPollKeepsNewDemand()
            try await storageStatusSampling()
            try await cleanupInteraction()
            try await cleanupQueueInteraction()
            print("PASS native access and interaction: setup, restart, stable discovery, quiet ignored events, state event exclusion, immediate cleanup feedback, live progress and preserved files")
            exit(0)
        }
        catch { fputs("FAIL native access flow: \(error)\n", stderr); exit(1) }
    }

    @MainActor private static func accessFlow() async throws {
        try snapshotResponseDecoding()
        try discoveryPresentation()
        try snapshotPublication()
        let fm = FileManager.default
        guard let physical = realpath(fm.temporaryDirectory.path, nil) else { throw EngineError.message("Cannot resolve disposable access-flow directory") }
        let temporary = URL(fileURLWithPath: String(cString: physical)); free(physical)
        let base = temporary.appendingPathComponent("chippytea-access-flow-\(UUID().uuidString)")
        let home = base.appendingPathComponent("Home")
        let data = base.appendingPathComponent("State")
        print("Disposable access-flow evidence: \(base.path)")
        try fm.createDirectory(at: data, withIntermediateDirectories: true)
        for name in ["Desktop", "Documents", "Downloads"] {
            try fm.createDirectory(at: home.appendingPathComponent(name), withIntermediateDirectories: true)
        }
        let source = home.appendingPathComponent("Documents/preserve.txt")
        let content = Data("Disposable access-flow fixture; preserve me.".utf8)
        try content.write(to: source)
        let record = data.appendingPathComponent("disk-access.json")
        try JSONEncoder().encode("waiting").write(to: record)
        func stored() -> String? {
            guard let bytes = try? Data(contentsOf: record) else { return nil }
            return try? JSONDecoder().decode(String.self, from: bytes)
        }
        func until(_ message: String, _ condition: () -> Bool) async throws {
            let deadline = Date().addingTimeInterval(10)
            while !condition() {
                try require(Date() < deadline, message)
                try await Task.sleep(for: .milliseconds(20))
            }
        }
        func libraryAvailable() -> Bool {
            let lock = data.appendingPathComponent("library.lock")
            let fd = open(lock.path, O_RDWR | O_CLOEXEC)
            guard fd >= 0 else { return false }
            defer { close(fd) }
            guard flock(fd, LOCK_EX | LOCK_NB) == 0 else { return false }
            _ = flock(fd, LOCK_UN)
            return true
        }
        var model: AppModel? = AppModel(directory: data, scanHome: home)
        await model!.start()
        try require(model!.showDiskAccess && model!.diskAccessPhase == .waiting, "Restart must resume the saved instructions")
        try require(model!.snapshot.roots.isEmpty && !model!.snapshot.scanning, "Setup must not authorize or scan before the explicit start action")
        model!.diskAccessReturned()
        try require(model!.diskAccessPhase == .waiting && model!.snapshot.roots.isEmpty, "Returning from Settings must not probe folders or start scanning")
        model!.dismissDiskAccess()
        try await until("Dismissal was not saved") { stored() == "dismissed" }
        try require(!model!.showDiskAccess && model!.diskAccessPhase == .intro, "Dismissal must close setup")
        model!.beginDiskAccessSetup()
        model!.confirmDiskAccessAndScan()
        try require(model!.snapshot.roots.isEmpty && model!.diskAccessPhase == .intro, "Intro must not bypass the Settings step")
        model!.diskAccessPhase = .waiting // Simulate the saved return from System Settings.
        let deniedFolder = home.appendingPathComponent("Documents")
        try fm.setAttributes([.posixPermissions: 0], ofItemAtPath: deniedFolder.path)
        defer { try? fm.setAttributes([.posixPermissions: 0o700], ofItemAtPath: deniedFolder.path) }
        model!.confirmDiskAccessAndScan()
        try await until("Denied folder access did not stay in setup") {
            model!.diskAccessPhase == .waiting && model!.diskAccessMessage?.contains("Documents") == true
        }
        try require(model!.snapshot.roots.isEmpty && !model!.snapshot.scanning && stored() != "completed", "A blocked permission check must not authorize, scan or save completion")
        try fm.setAttributes([.posixPermissions: 0o700], ofItemAtPath: deniedFolder.path)
        model!.confirmDiskAccessAndScan()
        try await until("Explicit scan did not complete") {
            model!.snapshot.roots.count == 1 && model!.snapshot.stats.complete && !model!.snapshot.scanning && !model!.busy
        }
        try require(model!.snapshot.roots[0].path == home.path, "Only the injected disposable home is authorized")
        try require(model!.snapshot.roots[0].kind == "home", "Home authorization must carry its restricted media policy")
        try require(try Data(contentsOf: source) == content, "Scan must preserve files")
        let checkedBeforeNoise = model!.snapshot.stats.entries
        let history = home.appendingPathComponent(".zsh_history")
        for _ in 0..<10 { try Data("Disposable shell history".utf8).write(to: history) }
        try await Task.sleep(for: .milliseconds(900))
        await model!.reload()
        try require(!model!.snapshot.scanning && model!.snapshot.stats.entries == checkedBeforeNoise, "Ordinary Home file events must not trigger another scan")
        try fm.removeItem(at: history)
        try await Task.sleep(for: .milliseconds(700))
        await model!.reload()
        try require(!model!.snapshot.scanning && model!.snapshot.stats.entries == checkedBeforeNoise, "Removed temporary files must not promote to Home")
        model!.beginDiskAccessSetup()
        try require(model!.showDiskAccess && model!.diskAccessPhase == .intro, "Completed setup must reopen with working controls")
        model!.dismissDiskAccess()
        try await until("Completed setup was not saved") { stored() == "completed" }
        let released = { [weak model] in model == nil }
        model = nil
        try await until("Previous engine did not close") { released() && libraryAvailable() }
        // Reproduce an installation saved by the older app: the same Home
        // bookmark and matching code identity, but the unrestricted root kind.
        var legacyClient: EngineClient? = try EngineClient(database: data.appendingPathComponent("library.sqlite"))
        let savedSnapshot = try EngineClient.decode(EngineSnapshot.self, await legacyClient!.request(["action": "snapshot"]))
        _ = try await legacyClient!.request(["action": "forget", "id": savedSnapshot.roots[0].id])
        _ = try await legacyClient!.request(["action": "authorize", "path": home.path, "kind": "folder"])
        legacyClient = nil
        try await until("Legacy fixture engine did not close") { libraryAvailable() }
        let completedRecord = try Data(contentsOf: record)
        try JSONEncoder().encode("waiting").write(to: record)
        model = AppModel(directory: data, scanHome: home)
        await model!.start()
        do {
            // A failed bookmark write must not leave the legacy Home watcher
            // enabled. All paths here belong to this disposable fixture.
            let bookmarkFile = data.appendingPathComponent("bookmarks.json")
            let savedBookmark = data.appendingPathComponent("bookmarks.saved.json")
            try fm.moveItem(at: bookmarkFile, to: savedBookmark)
            defer {
                try? fm.removeItem(at: bookmarkFile)
                try? fm.moveItem(at: savedBookmark, to: bookmarkFile)
            }
            try fm.createDirectory(at: bookmarkFile, withIntermediateDirectories: false)
            model!.confirmDiskAccessAndScan()
            try await until("Home authorization failure did not return") {
                !model!.busy && model!.errorMessage != nil
            }
            try require(model!.snapshot.roots.count == 1 && model!.snapshot.roots[0].kind == "folder",
                        "Failed authorization must preserve the existing grant")
            model!.refresh()
            try require(model!.showDiskAccess && !model!.snapshot.scanning,
                        "Failed Home authorization must gate the old folder policy")
            model!.dismissDiskAccess()
            try await until("Failed authorization restored completion on Back") { stored() == "dismissed" }
        }
        let failedAuthorizationReleased = { [weak model] in model == nil }
        model = nil
        try await until("Failed authorization engine did not close") { failedAuthorizationReleased() && libraryAvailable() }
        try completedRecord.write(to: record)
        model = AppModel(directory: data, scanHome: home)
        await model!.start()
        try require(!model!.showDiskAccess && model!.snapshot.roots.count == 1, "Restart must retain the grant without restarting onboarding")
        try require(model!.snapshot.roots[0].kind == "home" && model!.snapshot.roots[0].id == savedSnapshot.roots[0].id,
                    "Startup must narrow the existing Home grant before resuming its scan")
        try require(model!.snapshot.wallet.pendingCoins == 0, "Access and scans never earn coins")
        try await until("The migrated Home scan did not complete") {
            !model!.discoveryPresentation.isRequestPending && !model!.snapshot.scanning && model!.snapshot.stats.complete && model!.snapshot.stats.entries > 0
        }
        model!.beginDiskAccessSetup()
        model!.diskAccessPhase = .waiting
        try fm.setAttributes([.posixPermissions: 0], ofItemAtPath: deniedFolder.path)
        model!.confirmDiskAccessAndScan()
        try await until("A changed permission did not invalidate completed setup") {
            model!.diskAccessPhase == .waiting && model!.diskAccessMessage?.contains("Documents") == true && stored() == "waiting"
        }
        model!.dismissDiskAccess()
        try await until("Back restored completion after a denied check") { stored() == "dismissed" }
        model!.refresh()
        try require(model!.showDiskAccess && !model!.snapshot.scanning,
                    "Refresh must remain gated after a previously configured permission fails")
        try fm.setAttributes([.posixPermissions: 0o700], ofItemAtPath: deniedFolder.path)
        model!.diskAccessPhase = .waiting
        model!.confirmDiskAccessAndScan()
        try await until("Restored permission did not complete setup") {
            !model!.showDiskAccess && !model!.busy && !model!.snapshot.scanning && stored() == "completed"
        }
        let releasedAgain = { [weak model] in model == nil }
        model = nil
        try await until("Restarted engine did not close") { releasedAgain() && libraryAvailable() }
        let identityFile = data.appendingPathComponent("disk-access-app.json")
        let identityData = try Data(contentsOf: identityFile)
        let oldIdentity = try JSONDecoder().decode(DiskAccessAppIdentity.self, from: identityData)
        let otherInstall = DiskAccessAppIdentity(path: oldIdentity.path + ".replaced", requirement: oldIdentity.requirement)
        try JSONEncoder().encode(otherInstall).write(to: identityFile)
        model = AppModel(directory: data, scanHome: home)
        await model!.start()
        try require(model!.showDiskAccess && !model!.snapshot.scanning, "A changed app installation must return to setup before scanning Home: \(model!.errorMessage ?? "no startup error")")
        let replacedReleased = { [weak model] in model == nil }
        model = nil
        try await until("Replaced app engine did not close") { replacedReleased() && libraryAvailable() }
        try identityData.write(to: identityFile)
        try JSONEncoder().encode("waiting").write(to: record)
        model = AppModel(directory: data, scanHome: home)
        let startup = Task { await model!.start() }
        let resumeDeadline = Date().addingTimeInterval(10)
        while !model!.showDiskAccess {
            try require(Date() < resumeDeadline, "Waiting setup did not resume")
            await Task.yield()
        }
        model!.dismissDiskAccess()
        await startup.value
        try require(!model!.showDiskAccess && !model!.snapshot.scanning, "Startup must respect Back while restoring a waiting setup")
        model!.refresh()
        try require(model!.showDiskAccess && model!.diskAccessPhase == .intro, "A legacy home grant must enter setup before scanning")
        model!.dismissDiskAccess()
        model!.unkeep(path: home.appendingPathComponent("Documents").path)
        try require(model!.showDiskAccess && !model!.snapshot.scanning, "Include again must not bypass the unconfirmed home gate")
        model!.dismissDiskAccess()
        try Data("New disposable file".utf8).write(to: home.appendingPathComponent("Documents/later.txt"))
        try await Task.sleep(for: .milliseconds(500))
        try require(!model!.snapshot.scanning, "A filesystem event must not bypass the unconfirmed home gate")
        model!.refresh()
        try require(model!.showDiskAccess && !model!.snapshot.scanning, "Refresh must also respect the home permission gate")
        model!.dismissDiskAccess()
        let documents = home.appendingPathComponent("Documents")
        model!.authorizeAndScan(path: documents.path)
        try await until("Selected-folder fallback did not complete") {
            model!.snapshot.roots.count == 1 && model!.snapshot.roots[0].path == documents.path && model!.snapshot.stats.complete && !model!.busy
        }
        try require(try Data(contentsOf: source) == content, "Narrowing authorization must preserve existing files")
    }

    private static func discoveryPresentation() throws {
        var idle = EngineSnapshot()
        idle.stats.complete = true
        idle.stats.entries = 400
        idle.foregroundScan = ForegroundScan(active: false, stats: idle.stats)
        var presentation = DiscoveryPresentation()
        try require(presentation.statusLine(rootCount: 0) == "No folders authorised",
                    "An empty library must not imply a previous or active scan")
        presentation.update(idle)
        let settledLine = presentation.statusLine(rootCount: 1)
        try require(settledLine == "Last scan complete · 400 entries" && presentation.scanControlTitle == "Pause"
                    && presentation.scanControlAccessibilityLabel == "Pause background updates",
                    "Settled counts and the background control must be clearly distinguished from an active scan")
        for _ in 0..<20 {
            var background = idle
            background.scanning = true
            background.stats.complete = false
            background.stats.entries = 401
            presentation.update(background)
            try require(!presentation.isForeground && presentation.stats.entries == 400,
                        "Background refresh must preserve settled presentation")
            try require(presentation.statusLine(rootCount: 1) == settledLine && presentation.scanControlTitle == "Pause",
                        "Background work must retain the last-scan label and offer Pause rather than Cancel")
            presentation.update(idle)
            try require(!presentation.isForeground, "Settled content must not jump after maintenance")
        }
        presentation.beginRequestedScan()
        presentation.update(idle)
        try require(presentation.isForeground, "A pending explicit scan must survive an older idle snapshot")
        try require(presentation.statusLine(rootCount: 1) == "Starting scan…" && presentation.scanControlTitle == "Cancel",
                    "A pending scan must not present the preceding count as current progress")
        var running = idle
        running.scanning = true
        running.stats.complete = false
        running.foregroundScan = ForegroundScan(active: true, stats: running.stats)
        presentation.finishRequestedScan(running)
        try require(presentation.isForeground, "An explicit scan must show foreground progress")
        try require(presentation.statusLine(rootCount: 1).hasPrefix("400 entries checked")
                    && presentation.scanControlAccessibilityLabel == "Cancel scan",
                    "Active progress must use current counts and a scan-specific cancellation label")
        var cancelled = idle
        cancelled.stats.complete = false
        cancelled.stats.cancelled = true
        cancelled.foregroundScan = ForegroundScan(active: false, stats: cancelled.stats)
        presentation.update(cancelled)
        try require(!presentation.isForeground && presentation.stats.cancelled, "Cancellation must settle the presentation")
        try require(presentation.statusLine(rootCount: 1) == "Last scan cancelled · 400 entries"
                    && presentation.scanControlTitle == "Pause" && !presentation.stats.complete,
                    "A cancelled scan remains historical partial coverage, not completed work")
        presentation.beginRequestedScan()
        presentation.finishRequestedScan(idle)
        try require(!presentation.isForeground, "A fast completed scan must not leave a loading state")
        var initial = DiscoveryPresentation()
        initial.update(running)
        try require(initial.isForeground, "The initial scan must show foreground progress")

        // A full scan has a finite completion boundary. Subsequent event work
        // can keep the worker alive without extending the user's scan.
        var current = running
        current.foregroundScan = ForegroundScan(active: true, stats: running.stats)
        initial.update(current)
        try require(initial.isForeground, "An active requested scan must show progress")
        current.foregroundScan = ForegroundScan(active: false, stats: idle.stats)
        current.stats.entries = 401
        initial.update(current)
        try require(!initial.isForeground && initial.stats == idle.stats && current.scanning,
                    "A requested scan must finish even while background work is still running")
        try require(initial.statusLine(rootCount: 1) == settledLine && initial.scanControlTitle == "Pause",
                    "The foreground falling edge must immediately label settled counts and background pausing")
        for index in 0..<20 {
            current.scanning = index % 2 == 0
            current.stats.entries += 1
            initial.update(current)
            try require(!initial.isForeground && initial.stats == idle.stats,
                        "Background jobs must not overwrite the completed scan's statistics")
        }
        initial.beginRequestedScan()
        initial.update(current)
        try require(initial.isForeground && initial.isRequestPending && initial.stats == idle.stats,
                    "An older completed request must not clear the pending Scan action")
        current.scanning = true
        current.foregroundScan = ForegroundScan(active: true, stats: ScanStats(entries: 12))
        initial.finishRequestedScan(current)
        try require(initial.isForeground && !initial.isRequestPending && initial.stats.entries == 12,
                    "The accepted request must replace the prior scan's progress")
        current.foregroundScan = ForegroundScan(active: false, stats: cancelled.stats)
        initial.update(current)
        try require(!initial.isForeground && initial.stats.cancelled && !initial.stats.complete,
                    "Cancelled foreground scans must settle even while other work remains")
        var failed = idle.stats
        failed.complete = false
        failed.errors = 1
        current.foregroundScan = ForegroundScan(active: false, stats: failed)
        initial.update(current)
        try require(!initial.isForeground && initial.stats.errors == 1 && !initial.stats.complete,
                    "Failed foreground scans must retain their incomplete coverage")
        try require(initial.statusLine(rootCount: 1).hasPrefix("Last scan partial · 400 entries")
                    && initial.statusLine(rootCount: 1).hasSuffix("1 inaccessible"),
                    "Last-scan labeling must retain the failed scan's error count")
        initial.beginRequestedScan()
        current.foregroundScan = ForegroundScan(active: false, stats: idle.stats)
        initial.finishRequestedScan(current)
        try require(!initial.isForeground && initial.stats.complete,
                    "A foreground scan may finish before its first progress snapshot")
        try require(initial.statusLine(rootCount: 0) == "No folders authorised",
                    "Removing every grant must not display stale scan counts")
        initial.beginRequestedScan()
        current.foregroundScan = nil
        current.stats.entries = 50_000
        initial.update(current)
        try require(initial.isForeground && initial.isRequestPending
                    && initial.statusLine(rootCount: 1) == "Starting scan…",
                    "An older no-summary snapshot must not settle a pending explicit scan")
        initial.finishRequestedScan(current)
        try require(!initial.isForeground && !initial.isRequestPending && initial.stats.entries == 0
                    && initial.statusLine(rootCount: 1) == "No saved scan · 1 folder",
                    "An authoritative missing summary must clear counts instead of adopting raw diagnostics")
    }

    @MainActor private static func snapshotPublication() throws {
        let base = URL(fileURLWithPath: "/disposable/snapshot-publication", isDirectory: true)
        // No engine or filesystem work is started by this presentation-only model.
        let model = AppModel(directory: base, scanHome: base.appendingPathComponent("UnusedHome"))
        var uiNotifications = 0
        var rawValues: [EngineSnapshot] = []
        var valuesBeforeAssignment: [EngineSnapshot] = []
        let ui = model.objectWillChange.sink { uiNotifications += 1 }
        let raw = model.snapshots.sink {
            rawValues.append($0)
            valuesBeforeAssignment.append(model.snapshot)
        }
        defer { ui.cancel(); raw.cancel() }
        try require(rawValues == [model.snapshot], "Raw observers must immediately receive the current snapshot")

        var completed = model.snapshot
        completed.stats.entries = 400
        completed.stats.complete = true
        completed.foregroundScan = ForegroundScan(active: false, stats: completed.stats)
        model.snapshot = completed
        uiNotifications = 0
        rawValues.removeAll()
        valuesBeforeAssignment.removeAll()
        var previous = completed
        for index in 1...20 {
            var current = previous
            current.stats.entries += 1
            current.stats.elapsedMs = UInt64(index * 10)
            current.stats.complete = index % 2 == 0
            current.stats.message = "Background diagnostic \(index)"
            model.snapshot = current
            try require(rawValues.last == current && valuesBeforeAssignment.last == previous,
                        "Raw delivery must preserve the new value before the model assignment")
            try require(model.snapshot == current, "The stored snapshot must remain authoritative")
            previous = current
        }
        try require(uiNotifications == 0 && rawValues.count == 20,
                    "Twenty invisible background-stat updates must produce all raw values and no UI notifications")
        var lateValue: EngineSnapshot?
        let late = model.snapshots.sink { lateValue = $0 }
        late.cancel()
        try require(lateValue == previous, "Later observers must receive current raw diagnostics, not a presentation projection")

        func publication(_ label: String, _ change: (inout EngineSnapshot) -> Void) throws {
            let beforeUI = uiNotifications
            let beforeRaw = rawValues.count
            let before = model.snapshot
            var current = before
            change(&current)
            model.snapshot = current
            try require(uiNotifications == beforeUI + 1 && rawValues.count == beforeRaw + 1,
                        "\(label) must publish to the UI and raw observers")
            try require(rawValues.last == current && valuesBeforeAssignment.last == before && model.snapshot == current,
                        "\(label) must preserve raw notification timing and the latest value")
        }
        let identity = EngineIdentity(device: 1, inode: 2, mode: 0o040755, size: 0, modifiedNs: 0, changedNs: 0)
        let root = ScanRoot(id: "root", path: "/disposable/project", kind: "projects", identity: identity)
        let candidate = Candidate(id: "candidate", rootId: root.id, path: root.path + "/node_modules",
                                  title: "Disposable dependencies", kind: "node", logicalBytes: 100_000_000,
                                  allocatedBytes: 100_000_000, fileCount: 1, modifiedNs: 0,
                                  explanation: "Synthetic review", consequence: "Reinstall dependencies",
                                  eligiblePermanent: true, blockedReason: nil, identity: identity,
                                  fingerprint: "contents", evidence: "ownership", suggestionEligible: true)
        let receipt = Receipt(id: "receipt", path: candidate.path, title: candidate.title, operation: "trash",
                              outcome: "trashed", detail: "Synthetic history", createdAt: 0,
                              reportedBytes: 100_000_000, observedBytes: 0, creditedBytes: 0, coins: 0,
                              trashPath: nil, canRestore: false)
        try publication("Authorized roots") { $0.roots = [root] }
        try publication("Changed root identity") { $0.roots[0].identity.inode += 1 }
        try publication("Candidates") { $0.candidates = [candidate] }
        try publication("Changed review fingerprint") { $0.candidates[0].fingerprint = "changed" }
        try publication("Changed candidate identity") { $0.candidates[0].identity.inode += 1 }
        try publication("Wallet") { $0.wallet.pendingCoins = 1 }
        try publication("History") { $0.history = [receipt] }
        try publication("Error") { $0.error = "Synthetic error" }
        try publication("Keep") { $0.keptPaths = [candidate.path] }
        try publication("Scanning start") { $0.scanning = true }
        try publication("Scanning end") { $0.scanning = false }
        try publication("Cleanup start") { $0.cleaning = true }
        try publication("Cleanup end") { $0.cleaning = false }
        try publication("Foreground progress") { $0.foregroundScan = ForegroundScan(active: true, stats: ScanStats(entries: 12)) }
        try publication("Foreground completion") { $0.foregroundScan = ForegroundScan(active: false, stats: completed.stats) }
        try publication("Invalidated foreground summary") { $0.foregroundScan = nil }
        let withoutSummaryUI = uiNotifications
        let withoutSummaryRaw = rawValues.count
        for index in 1...20 {
            let previous = model.snapshot
            var current = previous
            current.stats.entries += 1
            current.stats.elapsedMs = UInt64(index)
            current.stats.message = "No-summary background diagnostic \(index)"
            model.snapshot = current
            try require(rawValues.last == current && valuesBeforeAssignment.last == previous && model.snapshot == current,
                        "No-summary raw observations must retain exact values and pre-assignment timing")
        }
        try require(uiNotifications == withoutSummaryUI && rawValues.count == withoutSummaryRaw + 20,
                    "Raw background counters without a saved result must not invalidate the UI")

        let beforeUI = uiNotifications
        let beforeRaw = rawValues.count
        let unchanged = model.snapshot
        model.snapshot = unchanged
        try require(uiNotifications == beforeUI && rawValues.count == beforeRaw + 1,
                    "Explicit identical assignments must preserve raw delivery without invalidating the UI")
        model.busy = true
        try require(uiNotifications == beforeUI + 1,
                    "Other @Published properties must retain the same synthesized objectWillChange publisher")
        print("Native snapshot publication: background_ui=0 background_raw=20 control_edges=true raw_timing=preserved")
    }

    @MainActor private static func foregroundSummarySurvivesRestart() async throws {
        let fm = FileManager.default
        guard let physical = realpath(fm.temporaryDirectory.path, nil) else {
            throw EngineError.message("Cannot resolve disposable foreground-restart directory")
        }
        let temporary = URL(fileURLWithPath: String(cString: physical))
        free(physical)
        let base = temporary.appendingPathComponent("chippytea-foreground-restart-\(UUID().uuidString)")
        try fm.createDirectory(at: base, withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700])
        try Data("Disposable chippytea foreground-restart fixture".utf8).write(to: base.appendingPathComponent(".chippytea-fixture"))
        print("Disposable foreground-restart evidence: \(base.path)")
        let projects = base.appendingPathComponent("Projects")
        let child = projects.appendingPathComponent("Changed")
        let unrelated = projects.appendingPathComponent("Unrelated")
        let data = base.appendingPathComponent("State")
        try fm.createDirectory(at: data, withIntermediateDirectories: false)
        var preserved: [(url: URL, contents: Data)] = []
        for (directory, count) in [(child, 1), (unrelated, 32)] {
            try fm.createDirectory(at: directory, withIntermediateDirectories: true)
            for index in 0..<count {
                let url = directory.appendingPathComponent("source-\(index).txt")
                let contents = Data("Preserve \(directory.lastPathComponent) source \(index).".utf8)
                try contents.write(to: url)
                preserved.append((url, contents))
            }
        }
        let database = data.appendingPathComponent("library.sqlite")
        func settled(_ client: EngineClient) async throws -> EngineSnapshot {
            let deadline = Date().addingTimeInterval(10)
            while true {
                let snapshot = try await client.snapshot()
                if !snapshot.scanning && !snapshot.cleaning { return snapshot }
                try require(Date() < deadline, "Disposable foreground discovery did not settle")
                try await Task.sleep(for: .milliseconds(20))
            }
        }
        func libraryAvailable() -> Bool {
            let fd = open(data.appendingPathComponent("library.lock").path, O_RDWR | O_CLOEXEC)
            guard fd >= 0 else { return false }
            defer { close(fd) }
            guard flock(fd, LOCK_EX | LOCK_NB) == 0 else { return false }
            _ = flock(fd, LOCK_UN)
            return true
        }
        func unchangedLedger(_ snapshot: EngineSnapshot) throws {
            try require(snapshot.wallet == Wallet() && snapshot.history.isEmpty && snapshot.keptPaths.isEmpty,
                        "Foreground discovery and restart must not change the disposable ledger")
            try require(snapshot.error == nil && snapshot.stats.complete && !snapshot.stats.cancelled && snapshot.stats.errors == 0,
                        "Foreground discovery and restart must retain complete coverage")
        }

        var client: EngineClient? = try EngineClient(database: database)
        defer { client = nil }
        let root = try EngineClient.decode(ScanRoot.self, await client!.request([
            "action": "authorize", "path": projects.path, "kind": "projects"
        ]))
        _ = try await client!.request(["action": "scan"])
        let full = try await settled(client!)
        try unchangedLedger(full)
        guard let foreground = full.foregroundScan else {
            throw EngineError.message("The full disposable scan did not establish a foreground summary")
        }
        let fullEntries = UInt64(preserved.count + 3)
        try require(!foreground.active && foreground.stats.complete && !foreground.stats.cancelled
                    && foreground.stats.errors == 0 && foreground.stats.entries == fullEntries,
                    "The initial foreground summary must cover the whole disposable root")
        try require(full.stats.entries == fullEntries && full.roots == [root] && full.candidates.isEmpty,
                    "The full scan must cover exactly the authorized disposable files")

        _ = try await client!.request([
            "action": "dirty", "root_id": root.id,
            "events": [["path": child.path, "kind": "directory", "recursive": true]]
        ])
        let maintained = try await settled(client!)
        try unchangedLedger(maintained)
        try require(maintained.foregroundScan == foreground && maintained.stats.entries == fullEntries + 2,
                    "The tiny child refresh must preserve the full foreground summary")
        client = nil
        let closeDeadline = Date().addingTimeInterval(10)
        while !libraryAvailable() {
            try require(Date() < closeDeadline, "The disposable engine did not release its library before restart")
            try await Task.sleep(for: .milliseconds(20))
        }
        client = try EngineClient(database: database)
        let restored = try await client!.snapshot()
        try unchangedLedger(restored)
        try require(!restored.scanning && !restored.cleaning && restored.roots == full.roots
                    && restored.candidates == full.candidates && restored.foregroundScan == foreground,
                    "A fresh engine must restore the exact completed foreground summary without scanning")
        try require(restored.stats.entries == 2 && restored.stats.entries < foreground.stats.entries,
                    "Restart must keep the tiny stored scope statistics separate from full-scan presentation")

        // This exercises presentation with an actual reopened FFI snapshot. It
        // does not start app setup, restore bookmarks or create a filesystem watcher.
        let model = AppModel(directory: data, scanHome: base.appendingPathComponent("UnusedHome"))
        model.snapshot = restored
        var presentation = DiscoveryPresentation()
        presentation.update(restored)
        let completedPresentation = presentation
        try require(!presentation.isForeground && !presentation.isRequestPending && presentation.stats == foreground.stats,
                    "Fresh presentation must select the restored full-scan count")
        try require(presentation.statusLine(rootCount: restored.roots.count).hasPrefix("Last scan complete · \(fullEntries.formatted()) entries")
                    && presentation.scanControlTitle == "Pause",
                    "A restored native snapshot must label its saved full-scan count and background pause control")
        var uiNotifications = 0
        var rawValues: [EngineSnapshot] = []
        let ui = model.objectWillChange.sink { uiNotifications += 1 }
        let raw = model.snapshots.sink { rawValues.append($0) }
        defer { ui.cancel(); raw.cancel() }
        try require(rawValues == [restored], "A new raw observer must receive the actual restored snapshot")
        rawValues.removeAll()
        for index in 1...20 {
            var current = restored
            current.stats.entries += UInt64(index)
            current.stats.elapsedMs = UInt64(index)
            current.stats.message = "Post-restart background diagnostic \(index)"
            model.snapshot = current
            presentation.update(current)
            try require(presentation == completedPresentation && model.snapshot == current && rawValues.last == current,
                        "Post-restart diagnostic delivery must preserve full-scan presentation and current raw state")
        }
        try require(uiNotifications == 0 && rawValues.count == 20,
                    "Post-restart raw statistics must deliver every update without invalidating stable presentation")
        try unchangedLedger(try await client!.snapshot())
        for item in preserved {
            try require(try Data(contentsOf: item.url) == item.contents, "Foreground restart must preserve every disposable file")
        }
        print("Native foreground restart: full_entries=\(fullEntries) restored_raw_entries=\(restored.stats.entries) background_ui=0 background_raw=20 files_preserved=true ledger_unchanged=true")
    }

    @MainActor private static func forgottenRootInvalidatesScanPresentation() async throws {
        let fm = FileManager.default
        guard let physical = realpath(fm.temporaryDirectory.path, nil) else {
            throw EngineError.message("Cannot resolve disposable forgotten-root directory")
        }
        let temporary = URL(fileURLWithPath: String(cString: physical))
        free(physical)
        let base = temporary.appendingPathComponent("chippytea-forget-summary-\(UUID().uuidString)")
        try fm.createDirectory(at: base, withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700])
        try Data("Disposable chippytea forgotten-root fixture".utf8).write(to: base.appendingPathComponent(".chippytea-fixture"))
        let first = base.appendingPathComponent("First")
        let remaining = base.appendingPathComponent("Remaining")
        let child = remaining.appendingPathComponent("Changed")
        let data = base.appendingPathComponent("State")
        try fm.createDirectory(at: data, withIntermediateDirectories: false)
        var preserved: [(url: URL, contents: Data)] = []
        for (directory, count) in [(first, 16), (remaining, 1), (child, 1)] {
            try fm.createDirectory(at: directory, withIntermediateDirectories: true)
            for index in 0..<count {
                let url = directory.appendingPathComponent("source-\(index).txt")
                let contents = Data("Preserve \(directory.lastPathComponent) source \(index).".utf8)
                try contents.write(to: url)
                preserved.append((url, contents))
            }
        }
        print("Disposable forgotten-root evidence: \(base.path)")

        let database = data.appendingPathComponent("library.sqlite")
        var model: AppModel? = AppModel(directory: data, scanHome: base.appendingPathComponent("UnusedHome"))
        var client: EngineClient?
        defer { client?.cancel(); model?.client?.cancel(); client = nil; model = nil }
        func waitForModel(_ message: String, _ condition: () -> Bool) async throws {
            let deadline = Date().addingTimeInterval(10)
            while !condition() {
                try require(Date() < deadline && model?.errorMessage == nil, model?.errorMessage ?? message)
                try await Task.sleep(for: .milliseconds(20))
            }
        }
        func settled(_ client: EngineClient) async throws -> EngineSnapshot {
            let deadline = Date().addingTimeInterval(10)
            while true {
                let snapshot = try await client.snapshot()
                if !snapshot.scanning && !snapshot.cleaning { return snapshot }
                try require(Date() < deadline, "Forgotten-root discovery did not settle")
                try await Task.sleep(for: .milliseconds(20))
            }
        }
        func preservedState(_ snapshot: EngineSnapshot) throws {
            try require(snapshot.wallet == Wallet() && snapshot.history.isEmpty && snapshot.keptPaths.isEmpty
                        && snapshot.candidates.isEmpty && snapshot.error == nil
                        && snapshot.stats.complete && !snapshot.stats.cancelled && snapshot.stats.errors == 0,
                        "Grant changes and discovery must preserve the disposable files and zero ledger")
        }
        func neutral(_ presentation: DiscoveryPresentation) throws {
            try require(!presentation.isForeground && !presentation.isRequestPending
                        && presentation.stats.entries == 0 && !presentation.stats.complete
                        && !presentation.settledCoverageNeedsAttention
                        && presentation.statusLine(rootCount: 1) == "No saved scan · 1 folder"
                        && presentation.scanControlTitle == "Pause",
                        "A forgotten root must clear historical counts instead of relabeling raw maintenance as Last scan")
        }

        await model!.start()
        client = model!.client
        try require(client != nil, "The forgotten-root engine did not open")
        model!.authorizeAndScan(path: first.path)
        try await waitForModel("The first disposable root did not finish") {
            model!.snapshot.roots.count == 1 && !model!.busy && !model!.snapshot.scanning
                && !model!.discoveryPresentation.isRequestPending && model!.snapshot.foregroundScan?.stats.complete == true
        }
        model!.authorizeAndScan(path: remaining.path)
        try await waitForModel("The two-root foreground scan did not finish") {
            model!.snapshot.roots.count == 2 && !model!.busy && !model!.snapshot.scanning
                && !model!.discoveryPresentation.isRequestPending && model!.snapshot.foregroundScan?.stats.complete == true
        }
        let full = model!.snapshot
        try preservedState(full)
        guard let firstRoot = full.roots.first(where: { $0.path == first.path }),
              let remainingRoot = full.roots.first(where: { $0.path == remaining.path }),
              let foreground = full.foregroundScan else {
            throw EngineError.message("The two disposable roots or their foreground result are missing")
        }
        try require(!foreground.active && foreground.stats.entries == 21 && full.stats.entries == 21
                    && model!.discoveryPresentation.stats == foreground.stats,
                    "The two-root fixture must establish exactly 21 foreground entries")
        let dirty: [String: Any] = ["action": "dirty", "root_id": remainingRoot.id,
                                   "events": [["path": child.path, "kind": "directory", "recursive": true]]]
        _ = try await client!.request(dirty)
        let maintained = try await settled(client!)
        await model!.reload()
        try preservedState(maintained)
        try require(maintained.stats.entries == 23 && maintained.foregroundScan == foreground
                    && model!.discoveryPresentation.stats == foreground.stats,
                    "The two-entry child pass must remain separate from the full foreground result")

        model!.forgetRoot(firstRoot) // Exercise the real native action and its reload.
        try await waitForModel("The forgotten root remained in the native snapshot") {
            model!.snapshot.roots == [remainingRoot] && !model!.snapshot.scanning
        }
        let forgotten = model!.snapshot
        try preservedState(forgotten)
        try require(forgotten.foregroundScan == nil && forgotten.stats.entries == 23,
                    "Successful forgetting must invalidate the summary while retaining raw engine diagnostics")
        try neutral(model!.discoveryPresentation)
        _ = try await client!.request(dirty)
        let after = try await settled(client!)
        await model!.reload()
        try preservedState(after)
        try require(after.roots == [remainingRoot] && after.foregroundScan == nil && after.stats.entries == 25,
                    "Later maintenance must process the remaining root without inventing another full scan")
        try neutral(model!.discoveryPresentation)

        client = nil
        model = nil
        let closeDeadline = Date().addingTimeInterval(10)
        while true {
            let fd = open(data.appendingPathComponent("library.lock").path, O_RDWR | O_CLOEXEC)
            try require(fd >= 0, "Cannot inspect the disposable engine lock")
            let available = flock(fd, LOCK_EX | LOCK_NB) == 0
            if available { _ = flock(fd, LOCK_UN) }
            close(fd)
            if available { break }
            try require(Date() < closeDeadline, "The forgotten-root engine did not release its library")
            try await Task.sleep(for: .milliseconds(20))
        }
        client = try EngineClient(database: database)
        let restored = try await client!.snapshot()
        try preservedState(restored)
        try require(!restored.scanning && restored.roots == [remainingRoot]
                    && restored.foregroundScan == nil && restored.stats.entries == 2,
                    "Reopening must expose the remaining stored scope without recreating a foreground result")
        // Do not start a new AppModel: startup resume/Scan would repair the
        // evidence. Present the actual reopened snapshot without scheduling work.
        var presentation = DiscoveryPresentation()
        presentation.update(restored)
        try neutral(presentation)
        let observerModel = AppModel(directory: data, scanHome: base.appendingPathComponent("UnusedHome"))
        observerModel.snapshot = restored
        var uiNotifications = 0
        var rawValues: [EngineSnapshot] = []
        let ui = observerModel.objectWillChange.sink { uiNotifications += 1 }
        let raw = observerModel.snapshots.dropFirst().sink { rawValues.append($0) }
        for index in 1...20 {
            var current = restored
            current.stats.entries += UInt64(index)
            current.stats.elapsedMs = UInt64(index)
            observerModel.snapshot = current
            presentation.update(current)
            try neutral(presentation)
            try require(observerModel.snapshot == current && rawValues.last == current,
                        "No-summary diagnostics must remain available to raw observers")
        }
        ui.cancel(); raw.cancel()
        try require(uiNotifications == 0 && rawValues.count == 20,
                    "No-summary raw counters must not invalidate the neutral native presentation")
        _ = try await client!.request(["action": "scan"])
        let rescanned = try await settled(client!)
        try preservedState(rescanned)
        presentation.beginRequestedScan()
        presentation.finishRequestedScan(rescanned)
        try require(rescanned.roots == [remainingRoot] && rescanned.foregroundScan?.active == false
                    && rescanned.foregroundScan?.stats.complete == true && rescanned.foregroundScan?.stats.entries == 4
                    && presentation.stats.entries == 4 && presentation.statusLine(rootCount: 1).hasPrefix("Last scan complete · 4 entries"),
                    "Only a real new scan may establish the remaining root's four-entry result")
        for item in preserved {
            try require(try Data(contentsOf: item.url) == item.contents, "Forgetting a grant must preserve every disposable file")
        }
        print("Native forgotten-root summary: full=21 maintenance=23,25 reopened_raw=2 new_foreground=4 files_preserved=18 ledger_unchanged=true")
    }

    @MainActor private final class SnapshotReadFixture {
        var snapshot: EngineSnapshot?
        var failure: String?
        var holdCount = 0
        var calls = 0
        var held: [Int: CheckedContinuation<EngineSnapshot, Error>] = [:]

        func read(_ client: EngineClient) async throws -> EngineSnapshot {
            calls += 1
            let call = calls
            if holdCount > 0 {
                holdCount -= 1
                return try await withCheckedThrowingContinuation { held[call] = $0 }
            }
            if let failure { throw EngineError.message(failure) }
            if let snapshot { return snapshot }
            return try await client.snapshot()
        }

        func finish(_ call: Int, with result: Result<EngineSnapshot, Error>) throws {
            guard let continuation = held.removeValue(forKey: call) else {
                throw EngineError.message("No matching disposable snapshot read")
            }
            continuation.resume(with: result)
        }

        func release() {
            let outstanding = held
            held.removeAll()
            for continuation in outstanding.values {
                continuation.resume(throwing: EngineError.message("Disposable snapshot fixture finished"))
            }
        }
    }

    @MainActor private static func warningPresentation() async throws {
        let fm = FileManager.default
        guard let physical = realpath(fm.temporaryDirectory.path, nil) else {
            throw EngineError.message("Cannot resolve disposable warning directory")
        }
        let temporary = URL(fileURLWithPath: String(cString: physical))
        free(physical)
        let base = temporary.appendingPathComponent("chippytea-warning-presentation-\(UUID().uuidString)")
        try fm.createDirectory(at: base, withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700])
        try Data("Disposable chippytea warning presentation fixture".utf8).write(to: base.appendingPathComponent(".chippytea-fixture"))
        print("Disposable warning-presentation evidence: \(base.path)")
        let projects = base.appendingPathComponent("Projects")
        try fm.createDirectory(at: projects, withIntermediateDirectories: false)
        let source = projects.appendingPathComponent("preserve.txt")
        let content = Data("Preserve this source through warning dismissal and recovery.".utf8)
        try content.write(to: source)
        let reads = SnapshotReadFixture()
        defer { reads.release() }
        let model = AppModel(directory: base.appendingPathComponent("State"), scanHome: base.appendingPathComponent("UnusedHome")) {
            try await reads.read($0)
        }
        await model.start()
        try require(model.snapshot.roots.isEmpty && !model.discoveryPresentation.settledCoverageNeedsAttention,
                    "A new library without authorized roots must not show an incomplete-coverage warning")
        guard let client = model.client else { throw EngineError.message("Warning fixture engine did not start") }
        func until(_ message: String, _ condition: () -> Bool) async throws {
            let deadline = Date().addingTimeInterval(8)
            while !condition() {
                try require(Date() < deadline, message)
                try await Task.sleep(for: .milliseconds(20))
            }
        }
        func settled() async throws -> EngineSnapshot {
            let deadline = Date().addingTimeInterval(8)
            while true {
                let snapshot = try await client.snapshot()
                if !snapshot.scanning && !snapshot.cleaning { return snapshot }
                try require(Date() < deadline, "Warning fixture discovery did not settle")
                try await Task.sleep(for: .milliseconds(20))
            }
        }
        _ = try await client.request(["action": "authorize", "path": projects.path, "kind": "projects"])
        await model.reload()
        try require(!model.snapshot.roots.isEmpty && !model.discoveryPresentation.settledCoverageNeedsAttention,
                    "An authorized location without a completed scan must not show an incomplete-coverage warning")
        _ = try await client.request(["action": "scan"])
        let healthy = try await settled()
        try require(healthy.stats.complete && healthy.error == nil && healthy.foregroundScan?.stats.complete == true,
                    "Warning tests require a real completed foreground snapshot")
        reads.snapshot = healthy
        await model.reload()
        let fullStats = model.discoveryPresentation.stats
        var coverageNotifications = 0
        let coverageObserver = model.objectWillChange.sink { coverageNotifications += 1 }
        var unreportedPartial = healthy
        unreportedPartial.stats.complete = false
        unreportedPartial.stats.errors = 1
        reads.snapshot = unreportedPartial
        await model.reload()
        try require(model.discoveryPresentation.settledCoverageNeedsAttention
                    && model.discoveryPresentation.stats == fullStats && coverageNotifications == 1,
                    "Settled coverage alone must publish its warning even when raw statistics are hidden")
        reads.snapshot = healthy
        await model.reload()
        try require(!model.discoveryPresentation.settledCoverageNeedsAttention && coverageNotifications == 2,
                    "Settled coverage recovery must publish without changing the full-scan count")
        coverageObserver.cancel()

        var noSummary = healthy
        noSummary.foregroundScan = nil
        reads.snapshot = noSummary
        await model.reload()
        try require(model.discoveryPresentation.stats.entries == 0
                    && model.discoveryPresentation.statusLine(rootCount: 1) == "No saved scan · 1 folder"
                    && !model.discoveryPresentation.settledCoverageNeedsAttention,
                    "Missing foreground results must remain neutral even with completed raw scope statistics")
        var noSummaryNotifications = 0
        var noSummaryRawUpdates = 0
        let noSummaryUI = model.objectWillChange.sink { noSummaryNotifications += 1 }
        let noSummaryRaw = model.snapshots.dropFirst().sink { _ in noSummaryRawUpdates += 1 }
        for _ in 1...20 {
            noSummary.stats.elapsedMs += 1
            reads.snapshot = noSummary
            await model.reload()
        }
        try require(noSummaryNotifications == 0 && noSummaryRawUpdates == 20,
                    "Applied no-summary raw diagnostics must stay observable without repainting the neutral result")
        var noSummaryPartial = noSummary
        noSummaryPartial.stats.complete = false
        noSummaryPartial.stats.errors = 1
        reads.snapshot = noSummaryPartial
        await model.reload()
        try require(model.discoveryPresentation.settledCoverageNeedsAttention
                    && model.discoveryPresentation.stats.entries == 0 && noSummaryNotifications == 1,
                    "Incomplete no-summary coverage must publish its own warning without adopting raw counts")
        reads.snapshot = noSummary
        await model.reload()
        try require(!model.discoveryPresentation.settledCoverageNeedsAttention
                    && model.discoveryPresentation.stats.entries == 0 && noSummaryNotifications == 2
                    && noSummaryRawUpdates == 22,
                    "No-summary coverage recovery must publish independently of raw snapshot invalidation")
        for message in ["Disposable no-summary access failure", "Disposable no-summary identity failure"] {
            noSummaryPartial.error = message
            reads.snapshot = noSummaryPartial
            await model.reload()
            try require(model.errorMessage == message && model.discoveryPresentation.stats.entries == 0
                        && model.discoveryPresentation.settledCoverageNeedsAttention,
                        "Changed engine errors must still surface without a saved foreground result")
        }
        try require(noSummaryRawUpdates == 24, "No-summary failures must reach raw observers too")
        noSummaryUI.cancel(); noSummaryRaw.cancel()
        reads.snapshot = healthy
        await model.reload()
        model.errorMessage = nil

        var removedGrantPresentation = DiscoveryPresentation()
        removedGrantPresentation.update(unreportedPartial)
        var withoutRoots = unreportedPartial
        withoutRoots.roots = []
        removedGrantPresentation.update(withoutRoots)
        try require(!removedGrantPresentation.settledCoverageNeedsAttention,
                    "Removing the authorized locations must clear their settled coverage warning")
        var retryPresentation = DiscoveryPresentation()
        retryPresentation.update(unreportedPartial)
        retryPresentation.beginRequestedScan()
        try require(!retryPresentation.settledCoverageNeedsAttention,
                    "A pending foreground retry must not suggest starting another scan")
        retryPresentation.failRequestedScan()
        try require(retryPresentation.settledCoverageNeedsAttention,
                    "A failed request handoff must retain the previous settled coverage warning")
        retryPresentation.beginRequestedScan()
        var foregroundActive = healthy
        foregroundActive.scanning = true
        foregroundActive.foregroundScan?.active = true
        foregroundActive.foregroundScan?.stats.entries = 1
        foregroundActive.foregroundScan?.stats.complete = false
        retryPresentation.finishRequestedScan(foregroundActive)
        try require(retryPresentation.isForeground && retryPresentation.stats.entries == 1
                    && !retryPresentation.settledCoverageNeedsAttention,
                    "Live foreground counts must not carry historical-coverage help")
        retryPresentation.update(healthy)
        try require(!retryPresentation.settledCoverageNeedsAttention && retryPresentation.stats == fullStats,
                    "A successful foreground retry must settle coverage and its full count")
        var active = healthy
        active.scanning = true
        active.stats.complete = false
        reads.snapshot = active
        await model.reload()
        try require(!model.discoveryPresentation.settledCoverageNeedsAttention && model.discoveryPresentation.stats == fullStats,
                    "Active background work must not create an incomplete-coverage warning")

        let engineFailure = "Disposable folder could not be checked"
        let readFailure = "Disposable snapshot read failed"
        var partial = healthy
        partial.stats.entries = 1
        partial.stats.complete = false
        partial.stats.errors = 1
        partial.stats.elapsedMs = 0
        partial.error = engineFailure
        reads.snapshot = partial
        await model.reload()
        try require(model.errorMessage == engineFailure && model.discoveryPresentation.settledCoverageNeedsAttention
                    && model.discoveryPresentation.stats == fullStats,
                    "A settled failure must warn without replacing the full-scan count")
        model.errorMessage = nil
        var uiNotifications = 0
        var rawUpdates = 0
        let ui = model.objectWillChange.sink { uiNotifications += 1 }
        let raw = model.snapshots.dropFirst().sink { _ in rawUpdates += 1 }
        defer { ui.cancel(); raw.cancel() }
        for index in 1...20 {
            partial.stats.elapsedMs = UInt64(index)
            reads.snapshot = partial
            await model.reload()
            try require(model.errorMessage == nil && model.discoveryPresentation.settledCoverageNeedsAttention
                        && model.discoveryPresentation.stats == fullStats,
                        "Unchanged warnings must remain dismissed while coverage stays visibly partial")
        }
        try require(uiNotifications == 0 && rawUpdates == 20,
                    "Twenty raw updates must not resurrect a dismissed warning or invalidate stable presentation")
        let cleanupFailure = "Disposable cleanup was refused"
        model.errorMessage = cleanupFailure
        await model.reload()
        try require(model.errorMessage == cleanupFailure, "An old engine warning must not overwrite a cleanup error")
        model.errorMessage = nil

        reads.holdCount = 1
        let staleSuccess = Task { @MainActor in await model.reload() }
        try await until("Stale success read was not held") { reads.held.count == 1 }
        let staleSuccessID = reads.held.keys.first!
        await model.reload()
        try reads.finish(staleSuccessID, with: .success(healthy))
        await staleSuccess.value
        await model.reload()
        try require(model.snapshot == partial && model.errorMessage == nil && model.discoveryPresentation.settledCoverageNeedsAttention,
                    "A superseded successful read must not clear coverage or re-arm an old engine warning")

        reads.holdCount = 1
        let staleFailure = Task { @MainActor in await model.reload() }
        try await until("Stale failure read was not held") { reads.held.count == 1 }
        let staleFailureID = reads.held.keys.first!
        await model.reload()
        try reads.finish(staleFailureID, with: .failure(EngineError.message("Superseded disposable read")))
        await staleFailure.value
        try require(model.errorMessage == nil, "A superseded failed read must not publish a warning")

        reads.failure = readFailure
        await model.reload()
        try require(model.errorMessage == readFailure, "A new snapshot-read failure must be reported")
        model.errorMessage = nil
        for _ in 0..<3 { await model.reload() }
        try require(model.errorMessage == nil, "Repeated failed reads must respect dismissal")
        reads.holdCount = 1
        let olderRecovery = Task { @MainActor in await model.reload() }
        try await until("Older recovery read was not held") { reads.held.count == 1 }
        let olderRecoveryID = reads.held.keys.first!
        await model.reload()
        try reads.finish(olderRecoveryID, with: .success(healthy))
        await olderRecovery.value
        await model.reload()
        try require(model.errorMessage == nil, "An older successful read must not re-arm a newer read failure")
        reads.failure = nil
        reads.snapshot = healthy
        await model.reload()
        try require(!model.discoveryPresentation.settledCoverageNeedsAttention,
                    "Authoritative settled recovery must clear the coverage warning")
        reads.failure = readFailure
        await model.reload()
        try require(model.errorMessage == readFailure, "The same read failure after successful recovery must be reported again")
        model.errorMessage = nil

        reads.failure = nil
        reads.snapshot = partial
        await model.reload()
        try require(model.errorMessage == engineFailure, "A recovered engine error must re-arm after an authoritative nil")
        var different = partial
        different.error = "Another disposable folder failed"
        reads.snapshot = different
        await model.reload()
        try require(model.errorMessage == different.error, "A different engine error must be reported")
        model.errorMessage = nil
        reads.snapshot = active
        await model.reload()
        try require(model.discoveryPresentation.settledCoverageNeedsAttention,
                    "Active background work must not clear an existing coverage warning")
        reads.snapshot = healthy
        await model.reload()
        try require(!model.discoveryPresentation.settledCoverageNeedsAttention && model.discoveryPresentation.stats == fullStats,
                    "Settled recovery must preserve the historical count")

        reads.snapshot = partial
        await model.reload()
        try require(model.errorMessage == engineFailure, "Explicit retry must begin from an observed failure")
        model.errorMessage = nil
        reads.holdCount = 1
        let beforeRetryRecovery = Task { @MainActor in await model.reload() }
        try await until("Pre-retry recovery read was not held") { reads.held.count == 1 }
        let beforeRetryRecoveryID = reads.held.keys.first!
        reads.holdCount = 1
        let beforeRetryError = Task { @MainActor in await model.reload() }
        try await until("Pre-retry error read was not held") { reads.held.count == 2 }
        let beforeRetryErrorID = reads.held.keys.first { $0 != beforeRetryRecoveryID }!
        reads.holdCount = 1
        model.refresh()
        try await until("Post-acceptance read was not held") { reads.held.count == 3 }
        let afterRetryID = reads.held.keys.first { $0 != beforeRetryRecoveryID && $0 != beforeRetryErrorID }!
        try reads.finish(beforeRetryRecoveryID, with: .success(healthy))
        await beforeRetryRecovery.value
        try require(model.discoveryPresentation.isRequestPending && model.errorMessage == nil,
                    "A pre-acceptance nil error must not re-arm the dismissed warning")
        try reads.finish(beforeRetryErrorID, with: .success(partial))
        await beforeRetryError.value
        try require(model.discoveryPresentation.isRequestPending && model.errorMessage == nil,
                    "Pre-acceptance recovery then error must not resurrect the old warning")
        reads.holdCount = 1
        try reads.finish(afterRetryID, with: .failure(EngineError.message(readFailure)))
        try await until("Pending retry did not continue polling after a failed read") { reads.held.count == 1 }
        try require(model.discoveryPresentation.isRequestPending && model.errorMessage == readFailure,
                    "A failed post-acceptance read must preserve the retry boundary")
        model.errorMessage = nil
        try reads.finish(reads.held.keys.first!, with: .success(partial))
        try await until("Authoritative retry result did not re-arm the same engine warning") {
            !model.discoveryPresentation.isRequestPending && model.errorMessage == engineFailure
        }
        reads.snapshot = nil
        let final = try await settled()
        try require(final.wallet == Wallet() && final.history.isEmpty && final.error == nil,
                    "Injected warning presentation must not change the real disposable ledger")
        try require(try Data(contentsOf: source) == content, "Warning presentation must preserve the disposable source")
        print("Native warning presentation: dismissed_polls=20 background_ui=0 raw_updates=20 stale_reads_ignored=true recovery_and_retry=true coverage_stable=true ledger_unchanged=true")
    }

    @MainActor private static func acceptedScanSurvivesSnapshotFailures() async throws {
        let fm = FileManager.default
        guard let physical = realpath(fm.temporaryDirectory.path, nil) else {
            throw EngineError.message("Cannot resolve disposable snapshot-failure directory")
        }
        let temporary = URL(fileURLWithPath: String(cString: physical))
        free(physical)
        let base = temporary.appendingPathComponent("chippytea-snapshot-failure-\(UUID().uuidString)")
        let projects = base.appendingPathComponent("Projects")
        try fm.createDirectory(at: projects, withIntermediateDirectories: true)
        try Data("Disposable chippytea snapshot-failure test".utf8).write(to: base.appendingPathComponent(".chippytea-fixture"))
        let source = projects.appendingPathComponent("source.txt")
        let content = Data("Preserve this source through failed snapshot reads".utf8)
        try content.write(to: source)
        var failReads = false
        var failedReads = 0
        let model = AppModel(directory: base.appendingPathComponent("State"), scanHome: base.appendingPathComponent("UnusedHome")) { client in
            if failReads {
                failedReads += 1
                throw EngineError.message("Injected disposable snapshot read failure")
            }
            return try await client.snapshot()
        }
        await model.start()
        guard let client = model.client else { throw EngineError.message("Snapshot fixture engine did not start") }
        _ = try await client.request(["action": "authorize", "path": projects.path, "kind": "projects"])
        await model.reload()
        func until(_ message: String, _ condition: () -> Bool) async throws {
            let deadline = Date().addingTimeInterval(8)
            while !condition() {
                try require(Date() < deadline, message)
                try await Task.sleep(for: .milliseconds(20))
            }
        }
        model.refresh()
        try await until("Initial disposable scan did not finish") {
            !model.discoveryPresentation.isRequestPending && model.snapshot.foregroundScan?.active == false
        }
        let previous = model.snapshot
        try require(previous.foregroundScan?.stats.complete == true, "Initial scan must supply a completed snapshot")
        failReads = true
        model.refresh()
        try await until("Failed post-acceptance reads must continue polling") { failedReads >= 3 }
        try require(model.discoveryPresentation.isForeground && model.discoveryPresentation.isRequestPending
                    && model.snapshot == previous,
                    "Failed reads must not present the old completed snapshot as the newly accepted scan")
        failReads = false
        try await until("A subsequent authoritative snapshot must settle the accepted request") {
            !model.discoveryPresentation.isRequestPending && !model.discoveryPresentation.isForeground
                && model.snapshot.foregroundScan?.active == false
        }
        try require(model.snapshot.foregroundScan?.stats.complete == true && model.snapshot.wallet == Wallet(),
                    "Recovered snapshot reads must retain real scan completion without manufacturing rewards")
        try require(try Data(contentsOf: source) == content, "Snapshot failures must preserve source files")
        print("Native scan handoff: failed_reads=\(failedReads) stale_completion=false recovered=true")
    }

    private static func watcherCursorCoalescing() throws {
        var timers: [() -> Void] = []
        var delivered: [FolderEventBatch] = []
        let delivery = FolderEventDelivery(schedule: { timers.append($0) }) { events, last, lost in
            delivered.append(FolderEventBatch(events: events, last: last, historyLost: lost))
        }
        defer { withExtendedLifetime(delivery) {} }
        let event = FolderEvent(path: "/disposable/source.rs", kind: "file", recursive: false)
        func fireTimer() throws {
            try require(timers.count == 1, "Cursor coalescing must keep exactly one scheduled timer")
            let action = timers.removeFirst()
            action()
        }

        for id in (1...2048).reversed() { delivery.receive([], last: UInt64(id), historyLost: false) }
        delivery.receive([], last: 0, historyLost: false)
        try require(delivered.isEmpty && timers.count == 1, "An empty burst must wait for one bounded flush")
        try fireTimer()
        try require(delivered.count == 1 && delivered[0].last == 2048 && delivered[0].events.isEmpty
                    && !delivered[0].historyLost && timers.isEmpty, "The burst must deliver only its maximum cursor")

        delivered.removeAll()
        delivery.receive([], last: 10, historyLost: false)
        delivery.receive([event], last: 20, historyLost: false)
        try require(delivered.count == 1 && delivered[0].events.first?.path == event.path
                    && delivered[0].last == 20 && timers.count == 1,
                    "A meaningful event must pass immediately without replacing the pending timer")
        delivery.receive([], last: 30, historyLost: false)
        try fireTimer()
        try require(delivered.count == 2 && delivered[1].events.isEmpty && delivered[1].last == 30,
                    "Later empty events must use the original nearer deadline")

        delivered.removeAll()
        delivery.receive([], last: 50, historyLost: false)
        delivery.receive([event], last: 40, historyLost: false)
        try require(delivered.count == 1 && delivered[0].last == 50,
                    "A meaningful event must carry an earlier pending cursor maximum")
        try fireTimer()
        try require(delivered.count == 1 && timers.isEmpty, "A consumed cursor must not flush twice or restart an idle timer")
        delivery.receive([event], last: 60, historyLost: false)
        try require(delivered.count == 2 && delivered[1].last == 60 && timers.isEmpty,
                    "Meaningful events alone must not schedule timers")

        delivered.removeAll()
        delivery.receive([], last: .max, historyLost: false)
        delivery.receive([], last: 0, historyLost: true)
        try require(delivered.count == 1 && delivered[0].last == 0 && delivered[0].historyLost
                    && timers.count == 1, "A loss barrier must immediately discard the pre-wrap maximum")
        delivery.receive([], last: 2, historyLost: false)
        try fireTimer()
        try require(delivered.count == 2 && delivered[1].last == 2 && !delivered[1].historyLost,
                    "A post-wrap cursor must not inherit the discarded maximum or lose the existing deadline")
        delivery.receive([], last: 100, historyLost: false)
        delivery.receive([event], last: 3, historyLost: true)
        try require(delivered.count == 3 && delivered[2].last == 3 && delivered[2].historyLost
                    && delivered[2].events.first?.path == event.path, "A loss barrier must retain its original batch")
        try fireTimer()
        try require(delivered.count == 3 && timers.isEmpty, "The timer must not revive a discarded cursor after loss")

        do {
            var reentrantTimers: [() -> Void] = []
            var cursors: [UInt64] = []
            weak var receiver: FolderEventDelivery?
            let reentrant = FolderEventDelivery(schedule: { reentrantTimers.append($0) }) { _, last, _ in
                cursors.append(last)
                if last == 7 { receiver?.receive([], last: 8, historyLost: false) }
            }
            defer { withExtendedLifetime(reentrant) {} }
            receiver = reentrant
            reentrant.receive([], last: 7, historyLost: false)
            try require(reentrantTimers.count == 1, "The initial reentrant case needs one timer")
            reentrantTimers.removeFirst()()
            try require(cursors == [7] && reentrantTimers.count == 1,
                        "A downstream callback must be able to schedule a new flush after the old state is cleared")
            reentrantTimers.removeFirst()()
            try require(cursors == [7, 8] && reentrantTimers.isEmpty, "Reentrant cursor delivery must survive exactly once")
        }

        var abandonedTimers: [() -> Void] = []
        var abandonedDeliveries = 0
        weak var abandoned: FolderEventDelivery?
        do {
            let temporary = FolderEventDelivery(schedule: { abandonedTimers.append($0) }) { _, _, _ in
                abandonedDeliveries += 1
            }
            abandoned = temporary
            temporary.receive([], last: 99, historyLost: false)
        }
        try require(abandoned == nil && abandonedTimers.count == 1, "A pending timer must not retain a removed watcher's delivery")
        abandonedTimers.removeFirst()()
        try require(abandonedDeliveries == 0 && abandonedTimers.isEmpty, "A removed watcher must not deliver its pending cursor later")
        print("Native cursor coalescing: burst=2048 flushes=1 original_deadline=true loss_barrier=true reentrant=true weak_timer=true")
    }

    private static func watcherEventDecoding() throws {
        typealias Flags = FSEventStreamEventFlags
        let root = "/private/tmp/chippytea-event-decoder"
        let state = root + "/State"
        let staged = root + "/Project/.chippytea-123-456-789"
        let recovery = root + "/Project/.chippytea-recovery-123-456-789"
        let ordinary = root + "/Project/source.rs"
        let file = Flags(kFSEventStreamEventFlagItemIsFile)
        let directory = Flags(kFSEventStreamEventFlagItemIsDir)
        let created = Flags(kFSEventStreamEventFlagItemCreated)
        let recursive = Flags(kFSEventStreamEventFlagMustScanSubDirs)
        let rootChanged = Flags(kFSEventStreamEventFlagRootChanged)
        let historyDone = Flags(kFSEventStreamEventFlagHistoryDone)

        func decode(_ paths: [Any], flags: [Flags]? = nil, ids: [UInt64]? = nil,
                    roots: [String]? = nil) -> FolderEventBatch? {
            let flags = flags ?? Array(repeating: file, count: paths.count)
            let ids = ids ?? paths.indices.map { UInt64($0 + 1) }
            let decoder = FolderEventDecoder(paths: roots ?? [root], excluding: [state])
            return flags.withUnsafeBufferPointer { flagBuffer in
                ids.withUnsafeBufferPointer { idBuffer in
                    decoder.decode(paths: paths as NSArray, flags: flagBuffer, ids: idBuffer)
                }
            }
        }

        for path in [staged, staged + "/leaf", staged + "/target/leaf", recovery + "/captured",
                     root + "/targeted/.chippytea-staged/leaf"] {
            let batch = decode([path], flags: [directory | recursive])
            try require(batch?.events.isEmpty == true && batch?.last == 1 && batch?.historyLost == false,
                        "An internal-only batch must acknowledge its cursor without retaining paths: \(path)")
        }
        for path in [ordinary, root, root + "/State-External/file", root + "/Project/target/.chippytea-log/leaf",
                     root + "/Project/node_modules/.chippytea-log/leaf", root + "/Project/.git/.chippytea-log",
                     root + "/Project/.cargo/.chippytea-log", root + "/.chippytea", root + "/prefix.chippytea-log",
                     root + "/.CHIPPYTEA-staged/leaf", root + "-neighbor/.chippytea-staged/leaf",
                     staged + "/../source.rs", root + "/Project/../.chippytea-staged/leaf",
                     staged + "/./leaf", staged + "//leaf", staged + "/", staged + "/nul\0leaf"] {
            let batch = decode([path])
            try require(batch?.events.count == 1 && batch?.events.first?.path == path && batch?.historyLost == false,
                        "A meaningful or ambiguous event must reach Rust unchanged: \(path)")
        }
        let explicit = root + "/.chippytea-authorized"
        for roots in [[explicit], [root, explicit], [explicit, root]] {
            for path in [explicit, explicit + "/source.rs", explicit + "/target/leaf"] {
                try require(decode([path], roots: roots)?.events.first?.path == path,
                            "An explicit root or an overlapping grant must preserve its events")
            }
        }
        let changedRoot = decode([explicit], flags: [rootChanged], ids: [0], roots: [explicit])
        try require(changedRoot?.events.first?.recursive == true && changedRoot?.last == 0,
                    "An explicit root change must survive its reserved-looking name and zero event ID")
        let nested = explicit + "/Project/.chippytea-staged/leaf"
        try require(decode([nested], roots: [explicit])?.events.isEmpty == true,
                    "Internal children of an explicitly authorized root still use relative policy")
        let unicodeRoot = root + "/caf\u{00e9}"
        let differentlySpelled = root + "/cafe\u{0301}/.chippytea-staged/leaf"
        try require(decode([differentlySpelled], roots: [unicodeRoot])?.events.count == 1,
                    "The early filter must not normalize a different root spelling")

        try require(decode([state + "/library.sqlite-wal"]) == nil,
                    "State-only batches must not feed another cursor write")
        try require(decode([123], flags: [historyDone], ids: [90]) == nil,
                    "HistoryDone has no path to decode or acknowledge on its own")
        let mixed = decode([staged + "/leaf", ordinary, state + "/cursor"], ids: [30, 11, 35])
        try require(mixed?.events.map(\.path) == [ordinary] && mixed?.last == 35 && mixed?.historyLost == false,
                    "Mixed batches must retain ordinary events and the maximum cursor")
        for loss in [kFSEventStreamEventFlagUserDropped, kFSEventStreamEventFlagKernelDropped, kFSEventStreamEventFlagEventIdsWrapped] {
            for path in [staged + "/leaf", state + "/cursor"] {
                let batch = decode([path], flags: [Flags(loss) | file], ids: [70])
                try require(batch?.events.isEmpty == true && batch?.last == 70 && batch?.historyLost == true,
                            "Loss flags must survive every path exclusion")
            }
            try require(decode([123], flags: [Flags(loss) | historyDone])?.historyLost == true,
                        "HistoryDone must not hide a loss flag")
            let mixedLoss = decode([ordinary, staged], flags: [file, Flags(loss) | file], ids: [80, 70])
            try require(mixedLoss?.historyLost == true && mixedLoss?.last == 80 && mixedLoss?.events.isEmpty == true,
                        "A later loss flag must replace a mixed batch with full reconciliation")
        }
        try require(decode([ordinary], flags: [], ids: [90])?.historyLost == true,
                    "Mismatched callback metadata requires conservative reconciliation")
        try require(decode([123])?.historyLost == true, "An unreadable callback path must not be silently lost")
        try require(decode([]) == nil, "An empty callback has no work")

        let typed = decode([ordinary, ordinary, root, root, root, ordinary],
                           flags: [file | created, file | recursive, directory | created, directory, rootChanged,
                                   Flags(kFSEventStreamEventFlagItemIsSymlink)], ids: [1, 2, 3, 4, 0, 5])
        try require(typed?.events.map(\.kind) == ["file", "file", "directory", "directory", "unknown", "file"]
                    && typed?.events.map(\.recursive) == [false, true, true, false, true, false],
                    "File, directory, recursive and root-change semantics must remain intact")
        let burst = (0..<2048).map { staged + "/leaf-\($0)" } + [ordinary]
        let burstBatch = decode(burst)
        try require(burstBatch?.events.map(\.path) == [ordinary] && burstBatch?.last == UInt64(burst.count),
                    "A cleanup burst must not retain its per-file payload or hide a real change")
        print("Native watcher decoding: root_relative=true cursor_preserved=true loss_preserved=true cleanup_paths_filtered=2048")
    }

    @MainActor private static func watcherExcludesState() async throws {
        let fm = FileManager.default
        guard let physical = realpath(fm.temporaryDirectory.path, nil) else {
            throw EngineError.message("Cannot resolve disposable watcher directory")
        }
        let temporary = URL(fileURLWithPath: String(cString: physical))
        free(physical)
        let base = temporary.appendingPathComponent("chippytea-watcher-\(UUID().uuidString)")
        try require(mkdir(base.path, 0o700) == 0, "Cannot exclusively create the disposable watcher directory")
        let state = base.appendingPathComponent("State")
        let sibling = base.appendingPathComponent("State-External")
        let project = base.appendingPathComponent("Project")
        let staged = project.appendingPathComponent(".chippytea-fixture-staged")
        let recovery = project.appendingPathComponent(".chippytea-recovery-fixture")
        // State is ninth to exercise the callback fallback beyond the native
        // API's eight-path limit; the first path exercises native exclusion.
        let excluded = (0..<8).map { base.appendingPathComponent("Ignored-\($0)") } + [state]
        for directory in excluded + [sibling, project, staged, recovery] {
            try fm.createDirectory(at: directory, withIntermediateDirectories: true)
        }
        let sentinelData = Data("Preserve this watcher fixture source and sibling".utf8)
        let sentinels = [project.appendingPathComponent("source.rs"), sibling.appendingPathComponent("preserve.txt")]
        let sentinelIdentities = try sentinels.map { path -> stat in
            try sentinelData.write(to: path, options: .withoutOverwriting)
            var info = stat()
            try require(lstat(path.path, &info) == 0, "Cannot inspect a watcher sentinel")
            return info
        }
        try Data("Disposable chippytea watcher test".utf8).write(to: base.appendingPathComponent(".chippytea-fixture"))
        let cursor = state.appendingPathComponent("cursor")
        try Data("0".utf8).write(to: cursor)
        print("Disposable watcher evidence: \(base.path)")
        var batches = 0
        var cursorOnlyBatches = 0
        var lastEvent: UInt64 = 0
        var observed = Set<String>()
        var failure: String?
        let watcher = FolderWatcher(paths: [base.path], since: 0, excluding: excluded.map(\.path)) { events, last, historyLost in
            Task { @MainActor in
                batches += 1
                lastEvent = max(lastEvent, last)
                if events.isEmpty && !historyLost { cursorOnlyBatches += 1 }
                observed.formUnion(events.map(\.path))
                if historyLost { failure = "Disposable watcher lost event history" }
                // Reproduce the app's durable cursor write after each batch.
                do { try Data(String(lastEvent).utf8).write(to: cursor) }
                catch { failure = error.localizedDescription }
            }
        }
        defer { withExtendedLifetime(watcher) {} }
        try require(watcher.isRunning, "Disposable watcher did not start")
        func quiet() async throws {
            let deadline = Date().addingTimeInterval(6)
            var stableSince = Date()
            var previous = batches
            while Date().timeIntervalSince(stableSince) < 1.2 {
                try require(Date() < deadline && failure == nil, failure ?? "State writes created a filesystem-event loop")
                try await Task.sleep(for: .milliseconds(50))
                if batches != previous { previous = batches; stableSince = Date() }
            }
        }
        func received(_ path: String) async throws {
            let deadline = Date().addingTimeInterval(8)
            while !observed.contains(path) {
                try require(Date() < deadline && failure == nil, failure ?? "External changes did not reach the watcher")
                try await Task.sleep(for: .milliseconds(50))
            }
        }
        func advanced(after value: UInt64) async throws {
            let deadline = Date().addingTimeInterval(8)
            while lastEvent <= value {
                try require(Date() < deadline && failure == nil, failure ?? "Internal cleanup events did not advance the cursor")
                try await Task.sleep(for: .milliseconds(50))
            }
        }
        try await quiet()
        let beforeStateWrites = batches
        for index in 0..<10 {
            try Data(String(index).utf8).write(to: cursor)
            try Data(String(index).utf8).write(to: excluded[0].appendingPathComponent("index"))
        }
        try await quiet()
        try require(batches == beforeStateWrites, "Excluded state writes must not deliver another cursor-persisting batch")

        let beforeInternal = lastEvent
        let beforeCursorOnly = cursorOnlyBatches
        observed.removeAll()
        let leaves = (0..<8).map { staged.appendingPathComponent("leaf-\($0)") }
        for leaf in leaves { try Data("Disposable compiled output".utf8).write(to: leaf, options: .withoutOverwriting) }
        try await advanced(after: beforeInternal)
        try await quiet()
        let beforeRemoval = lastEvent
        for leaf in leaves {
            let captured = recovery.appendingPathComponent(leaf.lastPathComponent)
            try fm.moveItem(at: leaf, to: captured)
            try fm.removeItem(at: captured)
        }
        try await advanced(after: beforeRemoval)
        try await quiet()
        try require(cursorOnlyBatches > beforeCursorOnly, "Filtered cleanup events must produce a cursor-only acknowledgment")
        try require(!observed.contains(where: { $0 == staged.path || $0.hasPrefix(staged.path + "/")
            || $0 == recovery.path || $0.hasPrefix(recovery.path + "/") }),
                    "Staging and recovery churn must not retain per-file events")
        try require(try fm.contentsOfDirectory(atPath: staged.path).isEmpty && fm.contentsOfDirectory(atPath: recovery.path).isEmpty,
                    "Only the generated cleanup leaves should have been removed")
        try require(try String(contentsOf: cursor, encoding: .utf8) == String(lastEvent),
                    "Internal-only acknowledgment must persist the received cursor")

        let external = sibling.appendingPathComponent("reviewable.txt")
        observed.removeAll()
        try Data("Disposable external change".utf8).write(to: external)
        try await received(external.path)
        try await quiet()
        try require(!observed.contains(where: { $0 == state.path || $0.hasPrefix(state.path + "/") }), "Callback fallback exposed the state directory")
        observed.removeAll()
        try fm.removeItem(at: external)
        try await received(external.path)
        try await quiet()
        try require(fm.fileExists(atPath: cursor.path), "Watcher exclusions must preserve state files")
        for (path, expected) in zip(sentinels, sentinelIdentities) {
            var current = stat()
            try require(lstat(path.path, &current) == 0 && current.st_dev == expected.st_dev
                        && current.st_ino == expected.st_ino && current.st_mode == expected.st_mode
                        && current.st_size == expected.st_size && current.st_uid == expected.st_uid
                        && current.st_nlink == expected.st_nlink
                        && current.st_mtimespec.tv_sec == expected.st_mtimespec.tv_sec
                        && current.st_mtimespec.tv_nsec == expected.st_mtimespec.tv_nsec
                        && current.st_ctimespec.tv_sec == expected.st_ctimespec.tv_sec
                        && current.st_ctimespec.tv_nsec == expected.st_ctimespec.tv_nsec,
                        "Watcher exercise changed a source or sibling identity")
            try require(try Data(contentsOf: path) == sentinelData, "Watcher exercise changed source or sibling contents")
        }
        print("Native watcher fixture: cursor_only_batches=\(cursorOnlyBatches - beforeCursorOnly) internal_paths_retained=0 external_changes_delivered=true sentinels_preserved=true")
    }

    @MainActor private static func ignoredEventsDoNotPublish() async throws {
        let fm = FileManager.default
        guard let physical = realpath(fm.temporaryDirectory.path, nil) else {
            throw EngineError.message("Cannot resolve disposable event-publication directory")
        }
        let temporary = URL(fileURLWithPath: String(cString: physical))
        free(physical)
        let base = temporary.appendingPathComponent("chippytea-event-publication-\(UUID().uuidString)")
        let projects = base.appendingPathComponent("Projects")
        let project = projects.appendingPathComponent("Disposable")
        try fm.createDirectory(at: project, withIntermediateDirectories: true)
        try Data("Disposable chippytea event-publication test".utf8).write(to: base.appendingPathComponent(".chippytea-fixture"))
        let manifest = project.appendingPathComponent("package.json")
        let source = project.appendingPathComponent("source.txt")
        try Data("{\"name\":\"disposable\"}".utf8).write(to: manifest)
        try Data("Preserve this source".utf8).write(to: source)
        print("Disposable event-publication evidence: \(base.path)")
        let model = AppModel(directory: base.appendingPathComponent("State"), scanHome: base.appendingPathComponent("UnusedHome"))
        await model.start()
        model.authorizeAndScan(path: projects.path, kind: "projects")
        guard let client = model.client else { throw EngineError.message("Event fixture engine did not start") }
        func currentCursor() async throws -> UInt64 {
            let result = try EngineClient.decode([String: UInt64].self, await client.request(["action": "cursor"]))
            guard let value = result["cursor"] else { throw EngineError.message("Missing durable event cursor") }
            return value
        }
        func received(after cursor: UInt64) async throws {
            let deadline = Date().addingTimeInterval(8)
            while try await currentCursor() <= cursor {
                try require(Date() < deadline && model.errorMessage == nil, model.errorMessage ?? "The watcher did not durably receive the disposable event")
                try await Task.sleep(for: .milliseconds(50))
            }
        }
        func settled() async throws {
            let deadline = Date().addingTimeInterval(10)
            var stableSince = Date()
            var previous = model.snapshotRequestID
            while Date().timeIntervalSince(stableSince) < 1.2 {
                try require(Date() < deadline && model.errorMessage == nil, model.errorMessage ?? "Event publication did not settle")
                try await Task.sleep(for: .milliseconds(50))
                if model.busy || model.snapshot.scanning || !model.snapshot.stats.complete || model.snapshotRequestID != previous {
                    previous = model.snapshotRequestID
                    stableSince = Date()
                }
            }
        }
        model.windowOpened()
        defer { model.windowClosed() }
        try await settled()
        let typedSnapshot = try await client.snapshot()
        let legacySnapshot = try EngineClient.decode(EngineSnapshot.self, await client.request(["action": "snapshot"]))
        try require(typedSnapshot == legacySnapshot, "Queued typed snapshots must preserve the complete disposable library state")
        var walletNotifications = 0
        model.onWalletChange = { _ in walletNotifications += 1 }
        await model.reload()
        try require(walletNotifications == 1, "A new badge observer must receive the current wallet")
        var publications = 0
        let observation = model.objectWillChange.sink { publications += 1 }
        defer { observation.cancel() }
        for _ in 0..<3 { await model.reload() }
        try require(publications == 0 && walletNotifications == 1, "Identical authoritative snapshots must not republish model or badge values")

        let beforeNoise = model.snapshot
        try require(beforeNoise.foregroundScan?.active == false && beforeNoise.foregroundScan?.stats.complete == true,
                    "The native client must observe explicit foreground completion")
        let completedPresentation = model.discoveryPresentation
        let ignoredRequests = model.snapshotRequestID
        let ignoredCursor = try await currentCursor()
        let content = Data("Ordinary source changes remain preserved".utf8)
        for _ in 0..<10 { try content.write(to: source) }
        try await received(after: ignoredCursor)
        try await settled()
        try require(model.snapshotRequestID == ignoredRequests, "Ignored file events must not request another application snapshot")
        try require(publications == 0 && walletNotifications == 1, "Ignored events must not redraw the visible idle model or badge")
        try require(model.snapshot == beforeNoise, "Ignored file events must preserve settled discovery state")

        let acceptedCursor = try await currentCursor()
        try Data("{\"name\":\"disposable-updated\"}".utf8).write(to: manifest)
        try await received(after: acceptedCursor)
        try await settled()
        try require(model.snapshotRequestID > ignoredRequests && publications > 0, "Accepted ownership events must refresh and publish authoritative scan state")
        try require(model.snapshot != beforeNoise && model.snapshot.stats.complete && !model.snapshot.scanning, "Accepted ownership refresh must finish with current results")
        try require(model.snapshot.foregroundScan == beforeNoise.foregroundScan && model.discoveryPresentation == completedPresentation,
                    "A real accepted filesystem event must preserve the completed foreground scan and its presentation")
        try require(model.snapshot.wallet == Wallet() && walletNotifications == 1, "Filesystem refresh must not manufacture rewards or repeat the unchanged badge")
        try require(try Data(contentsOf: source) == content, "Filesystem observation must preserve disposable source contents")
        print("Native event publication: ignored_reloads=0 ignored_publications=0 accepted_refresh=true durable_cursor_advanced=true")
    }

    @MainActor private static func backgroundPollKeepsNewDemand() async throws {
        let fm = FileManager.default
        guard let physical = realpath(fm.temporaryDirectory.path, nil) else {
            throw EngineError.message("Cannot resolve disposable poll-demand directory")
        }
        let temporary = URL(fileURLWithPath: String(cString: physical))
        free(physical)
        let base = temporary.appendingPathComponent("chippytea-poll-demand-\(UUID().uuidString)")
        try fm.createDirectory(at: base, withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700])
        let projects = base.appendingPathComponent("Projects")
        let changed = projects.appendingPathComponent("Changed")
        let artifact = changed.appendingPathComponent("target")
        let marker = artifact.appendingPathComponent("CACHEDIR.TAG")
        let manifest = changed.appendingPathComponent("Cargo.toml")
        let source = projects.appendingPathComponent("preserve.txt")
        let preserved = Data("Preserve this poll-demand source".utf8)
        let markerContents = Data("Signature: 8a477f597d28d172789f06886806bc55\n".utf8)
        try fm.createDirectory(at: artifact, withIntermediateDirectories: true)
        try Data("Disposable chippytea poll-demand fixture".utf8).write(to: base.appendingPathComponent(".chippytea-fixture"))
        try Data("[package]\nname=\"changed\"\nversion=\"0.0.0\"\n".utf8).write(to: manifest)
        try markerContents.write(to: marker)
        try preserved.write(to: source)
        let old = Date().addingTimeInterval(-9 * 86_400)
        for path in [marker, artifact] { try fm.setAttributes([.modificationDate: old], ofItemAtPath: path.path) }
        print("Disposable poll-demand evidence: \(base.path)")

        weak var observedModel: AppModel?
        var captureIdle = false
        var minimumDemand: UInt64 = 0
        var heldSnapshot: EngineSnapshot?
        var heldReadID: UInt64 = 0
        var heldRead: CheckedContinuation<EngineSnapshot, Error>?
        let model = AppModel(directory: base.appendingPathComponent("State"), scanHome: base.appendingPathComponent("UnusedHome")) { client in
            let demand = observedModel?.pollDemandID ?? 0
            let readID = observedModel?.snapshotRequestID ?? 0
            let current = try await client.snapshot()
            if captureIdle && demand > minimumDemand && !current.scanning && !current.cleaning {
                captureIdle = false
                // Capture an actual engine result; delay only its delivery to
                // reload while a later watcher acknowledgment requests polling.
                return try await withCheckedThrowingContinuation { continuation in
                    heldSnapshot = current
                    heldReadID = readID
                    heldRead = continuation
                }
            }
            return current
        }
        observedModel = model
        defer {
            captureIdle = false
            heldRead?.resume(throwing: EngineError.message("Disposable poll-demand fixture finished"))
        }
        await model.start()
        guard let client = model.client else { throw EngineError.message("Poll-demand engine did not start") }
        func until(_ message: String, _ condition: () async throws -> Bool) async throws {
            let deadline = Date().addingTimeInterval(15)
            while !(try await condition()) {
                try require(Date() < deadline && model.errorMessage == nil, model.errorMessage ?? message)
                try await Task.sleep(for: .milliseconds(20))
            }
        }
        func cursor() async throws -> UInt64 {
            let result = try EngineClient.decode([String: UInt64].self, await client.request(["action": "cursor"]))
            guard let value = result["cursor"] else { throw EngineError.message("Missing poll-demand cursor") }
            return value
        }
        func manifestMetadata() throws -> stat {
            var value = stat()
            try require(lstat(manifest.path, &value) == 0 && value.st_uid == geteuid()
                        && value.st_mode & mode_t(S_IFMT) == mode_t(S_IFREG) && value.st_nlink == 1,
                        "The poll-demand manifest must remain an owned regular file")
            return value
        }
        func modifiedNs(_ value: stat) -> Int64 {
            Int64(value.st_mtimespec.tv_sec) * 1_000_000_000 + Int64(value.st_mtimespec.tv_nsec)
        }
        model.authorizeAndScan(path: projects.path, kind: "projects")
        try await until("Initial poll-demand scan did not finish") {
            !model.busy && !model.snapshot.scanning && !model.discoveryPresentation.isRequestPending
                && model.snapshot.foregroundScan?.stats.complete == true
        }
        let before = model.snapshot
        let manifestBefore = try manifestMetadata()
        let firstCursor = try await cursor()
        minimumDemand = model.pollDemandID
        captureIdle = true
        func writeVersion(_ version: Int) throws -> (Data, Int64) {
            let bytes = Data("[package]\nname=\"changed\"\nversion=\"0.0.\(version)\"\n".utf8)
            try require(Int64(bytes.count) == manifestBefore.st_size, "Poll-demand writes must retain their byte length")
            // Close each in-place update before waiting for FSEvents. Keeping
            // the write handle open can defer delivery until the test ends.
            let file = try FileHandle(forWritingTo: manifest)
            do {
                var opened = stat()
                try require(fstat(file.fileDescriptor, &opened) == 0
                            && opened.st_dev == manifestBefore.st_dev && opened.st_ino == manifestBefore.st_ino
                            && opened.st_mode == manifestBefore.st_mode && opened.st_size == manifestBefore.st_size,
                            "Every poll-demand write must open the same existing manifest inode")
                try file.seek(toOffset: 0)
                try file.write(contentsOf: bytes)
                try file.synchronize()
                try file.close()
            } catch {
                try? file.close()
                throw error
            }
            let written = try manifestMetadata()
            try require(written.st_dev == manifestBefore.st_dev && written.st_ino == manifestBefore.st_ino,
                        "Poll-demand writes must not replace the manifest")
            return (bytes, modifiedNs(written))
        }
        let (_, firstModifiedNs) = try writeVersion(1)
        try await until("The event poll did not reach its idle-delivery boundary") {
            guard heldRead != nil && model.pollDemandID > minimumDemand else { return false }
            return try await cursor() > firstCursor
        }
        guard let stale = heldSnapshot else { throw EngineError.message("Missing held idle snapshot") }
        let demandBeforeFinalWrite = model.pollDemandID
        let cursorBeforeFinalWrite = try await cursor()
        let (finalContents, finalModifiedNs) = try writeVersion(2)
        try require(finalModifiedNs != firstModifiedNs && finalModifiedNs != modifiedNs(manifestBefore),
                    "The final poll-demand version needs distinct indexed evidence")
        try await until("The final watcher event did not request polling during the held read") {
            guard model.pollDemandID > demandBeforeFinalWrite else { return false }
            return try await cursor() > cursorBeforeFinalWrite
        }
        try require(model.snapshotRequestID == heldReadID && heldRead != nil,
                    "The later demand must arrive while the original poll read is still held")

        var connection: OpaquePointer?
        let database = base.appendingPathComponent("State/library.sqlite")
        guard sqlite3_open_v2(database.path, &connection, SQLITE_OPEN_READONLY | SQLITE_OPEN_NOFOLLOW, nil) == SQLITE_OK else {
            if let connection { sqlite3_close(connection) }
            throw EngineError.message("Cannot read the disposable poll-demand journal")
        }
        defer { sqlite3_close(connection) }
        var statement: OpaquePointer?
        let sql = """
            SELECT (SELECT count(*) FROM pending_scopes)+(SELECT count(*) FROM active_scopes)
                     +(SELECT count(*) FROM refreshes)+(SELECT count(*) FROM refresh_seen)+(SELECT count(*) FROM incomplete_roots),
                   (SELECT json_extract(json,'$.entries') FROM scans LIMIT 1),
                   (SELECT count(*) FROM candidates WHERE path=?1 AND json_extract(json,'$.modified_ns')=?2
                     AND json_extract(json,'$.suggestion_eligible')=0 AND json_extract(json,'$.eligible_permanent')=0
                     AND json_extract(json,'$.provisional')=0 AND json_extract(json,'$.blocked_reason') IS NOT NULL)
            """
        try require(sqlite3_prepare_v2(connection, sql, -1, &statement, nil) == SQLITE_OK,
                    "Cannot prepare the final poll-demand evidence check")
        defer { sqlite3_finalize(statement) }
        let copiedText = unsafeBitCast(-1, to: sqlite3_destructor_type.self)
        try require(artifact.path.withCString({ sqlite3_bind_text(statement, 1, $0, -1, copiedText) }) == SQLITE_OK
                    && sqlite3_bind_int64(statement, 2, finalModifiedNs) == SQLITE_OK,
                    "Cannot bind the final poll-demand evidence")
        func finalEvidenceIndexed() throws -> Bool {
            try require(sqlite3_step(statement) == SQLITE_ROW, "Cannot read the final poll-demand evidence")
            let current = sqlite3_column_int64(statement, 0) == 0 && sqlite3_column_int64(statement, 1) == 3
                && sqlite3_column_int64(statement, 2) == 1
            try require(sqlite3_reset(statement) == SQLITE_OK, "Cannot reset the final poll-demand evidence check")
            return current
        }
        heldRead?.resume(returning: stale)
        heldRead = nil
        // No manual model reload or poll may repair the missed-demand case.
        // Direct client reads below only validate what the model observes itself.
        try await until("The stale idle delivery lost the newer accepted event") {
            guard model.snapshotRequestID > heldReadID, try finalEvidenceIndexed() else { return false }
            let current = try await client.snapshot()
            return !current.scanning && !current.cleaning && current.stats.complete
                && current.error == nil && current.stats.errors == 0 && !current.stats.cancelled
                && current.stats.entries > stale.stats.entries && model.snapshot == current
        }
        try require(model.snapshot.foregroundScan == before.foregroundScan && model.snapshot.roots == before.roots
                    && model.snapshot.candidates == before.candidates && model.snapshot.wallet == Wallet()
                    && model.snapshot.history.isEmpty && model.snapshot.keptPaths.isEmpty,
                    "Poll-demand recovery must preserve the foreground, recommendations and zero ledger")
        let manifestAfter = try manifestMetadata()
        let actualManifest = try Data(contentsOf: manifest)
        try require(manifestAfter.st_dev == manifestBefore.st_dev && manifestAfter.st_ino == manifestBefore.st_ino
                    && manifestAfter.st_mode == manifestBefore.st_mode && manifestAfter.st_size == manifestBefore.st_size
                    && modifiedNs(manifestAfter) == finalModifiedNs && actualManifest == finalContents,
                    "Polling must preserve the final manifest identity and contents")
        let actualSource = try Data(contentsOf: source)
        let actualMarker = try Data(contentsOf: marker)
        try require(actualSource == preserved && actualMarker == markerContents,
                    "Polling must preserve the disposable source and artifact marker")
        print("Native poll demand: genuine_idle_read_held=true later_watcher_demand=true final_write_indexed=true native_matches_engine=true foreground_unchanged=true files_preserved=true ledger_unchanged=true")
    }

    /// A watcher integration check, not a timing or CPU benchmark. The existing
    /// eligible interaction artifact stays untouched while another project changes.
    @MainActor private static func ownershipEventBurst(model: AppModel, base: URL, candidate: Candidate) async throws {
        guard let client = model.client else { throw EngineError.message("Ownership event engine did not start") }
        let manifest = base.appendingPathComponent("Projects/Changed/Cargo.toml")
        let changedArtifact = manifest.deletingLastPathComponent().appendingPathComponent("target")
        let changedMarker = changedArtifact.appendingPathComponent("CACHEDIR.TAG")
        func cursor() async throws -> UInt64 {
            let result = try EngineClient.decode([String: UInt64].self, await client.request(["action": "cursor"]))
            guard let value = result["cursor"] else { throw EngineError.message("Missing ownership event cursor") }
            return value
        }
        func settled() async throws {
            let deadline = Date().addingTimeInterval(15)
            var stableSince = ProcessInfo.processInfo.systemUptime
            var requests = model.snapshotRequestID
            while ProcessInfo.processInfo.systemUptime - stableSince < 1.2 {
                try require(Date() < deadline && model.errorMessage == nil,
                            model.errorMessage ?? "Ownership event work did not settle")
                try await Task.sleep(for: .milliseconds(50))
                if model.busy || model.snapshot.scanning || model.isCleaning || model.discoveryPresentation.isRequestPending
                    || !model.snapshot.stats.complete || model.snapshotRequestID != requests {
                    requests = model.snapshotRequestID
                    stableSince = ProcessInfo.processInfo.systemUptime
                }
            }
        }
        func metadata(_ path: String, type: mode_t = mode_t(S_IFREG)) throws -> stat {
            var value = stat()
            try require(lstat(path, &value) == 0 && value.st_uid == geteuid()
                        && value.st_mode & mode_t(S_IFMT) == type && value.st_size >= 0,
                        "Ownership fixture must remain an owned physical item")
            return value
        }
        func identity(_ value: stat) -> EngineIdentity {
            EngineIdentity(device: UInt64(value.st_dev), inode: UInt64(value.st_ino), mode: UInt32(value.st_mode),
                           size: UInt64(value.st_size),
                           modifiedNs: Int64(value.st_mtimespec.tv_sec) * 1_000_000_000 + Int64(value.st_mtimespec.tv_nsec),
                           changedNs: Int64(value.st_ctimespec.tv_sec) * 1_000_000_000 + Int64(value.st_ctimespec.tv_nsec))
        }
        try await settled()
        let before = try await client.snapshot()
        let presentation = model.discoveryPresentation
        let beforeCursor = try await cursor()
        let manifestBefore = try metadata(manifest.path)
        let changedArtifactBefore = try metadata(changedArtifact.path, type: mode_t(S_IFDIR))
        let changedMarkerBefore = try metadata(changedMarker.path)
        let source = base.appendingPathComponent("Projects/Disposable/preserve.txt")
        let sourceBefore = try metadata(source.path)
        let payload = base.appendingPathComponent("Projects/Disposable/target/debug/payload")
        let payloadBefore = try metadata(payload.path)
        let artifactBefore = try metadata(candidate.path, type: mode_t(S_IFDIR))
        try require(before.foregroundScan?.active == false && before.foregroundScan?.stats.complete == true
                    && before.candidates == [candidate] && candidate.eligiblePermanent && candidate.recommended
                    && !candidate.fingerprint.isEmpty && !candidate.evidence.isEmpty && identity(artifactBefore) == candidate.identity,
                    "Ownership updates require a completed scan and a separate preserved eligible artifact")
        let writes = 6
        func writeBurst() async throws -> Data {
            // Keep one inode and handle throughout: atomic replacements would
            // exercise structural-directory events instead of file ownership events.
            let file = try FileHandle(forWritingTo: manifest)
            defer { try? file.close() }
            var final = Data()
            var modificationTimes: Set<Int64> = [identity(manifestBefore).modifiedNs]
            for index in 0..<writes {
                if index > 0 { try await Task.sleep(for: .milliseconds(400)) }
                final = Data("[package]\nname=\"changed\"\nversion=\"0.0.\(index)\"\n".utf8)
                try require(Int64(final.count) == manifestBefore.st_size, "Manifest writes must retain their byte length")
                try file.seek(toOffset: 0)
                try file.write(contentsOf: final)
                try file.synchronize()
                try require(modificationTimes.insert(identity(try metadata(manifest.path)).modifiedNs).inserted,
                            "Each ownership write must have a distinct timestamp for the final-event proof")
            }
            return final
        }
        let expectedManifest = try await writeBurst()
        let expectedModifiedNs = identity(try metadata(manifest.path)).modifiedNs

        // The tiny artifact stays ineligible because its manifest is fresh. Its
        // diagnostic timestamp proves that the final write reached discovery;
        // an earlier cursor advance or a quiet UI alone cannot prove delivery.
        // Opening read-only cannot create or repair another library.
        var connection: OpaquePointer?
        let database = base.appendingPathComponent("State/library.sqlite")
        guard sqlite3_open_v2(database.path, &connection, SQLITE_OPEN_READONLY | SQLITE_OPEN_NOFOLLOW, nil) == SQLITE_OK else {
            if let connection { sqlite3_close(connection) }
            throw EngineError.message("Cannot read the disposable ownership journal")
        }
        defer { sqlite3_close(connection) }
        var statement: OpaquePointer?
        let sql = """
            SELECT (SELECT count(*) FROM pending_scopes)+(SELECT count(*) FROM active_scopes)
                     +(SELECT count(*) FROM refreshes)+(SELECT count(*) FROM incomplete_roots),
                   (SELECT json_extract(json,'$.entries') FROM scans LIMIT 1),
                   (SELECT count(*) FROM candidates WHERE path=?1 AND json_extract(json,'$.modified_ns')=?2
                     AND json_extract(json,'$.suggestion_eligible')=0 AND json_extract(json,'$.eligible_permanent')=0
                     AND json_extract(json,'$.provisional')=0 AND json_extract(json,'$.blocked_reason') IS NOT NULL)
            """
        try require(sqlite3_prepare_v2(connection, sql, -1, &statement, nil) == SQLITE_OK,
                    "Cannot prepare the disposable ownership journal check")
        defer { sqlite3_finalize(statement) }
        let copiedText = unsafeBitCast(-1, to: sqlite3_destructor_type.self)
        try require(changedArtifact.path.withCString({ sqlite3_bind_text(statement, 1, $0, -1, copiedText) }) == SQLITE_OK
                    && sqlite3_bind_int64(statement, 2, expectedModifiedNs) == SQLITE_OK,
                    "Cannot bind the final ownership evidence check")
        func finalEvidenceIndexed() throws -> Bool {
            try require(sqlite3_step(statement) == SQLITE_ROW, "Cannot read the disposable ownership journal")
            let verified = sqlite3_column_int64(statement, 0) == 0 && sqlite3_column_int64(statement, 1) == 3
                && sqlite3_column_int64(statement, 2) == 1
            try require(sqlite3_reset(statement) == SQLITE_OK, "Cannot reset the disposable ownership journal check")
            return verified
        }
        let receiveDeadline = Date().addingTimeInterval(15)
        while try !finalEvidenceIndexed() {
            try require(Date() < receiveDeadline && model.errorMessage == nil,
                        model.errorMessage ?? "The final ownership write was not indexed with a drained journal")
            try await Task.sleep(for: .milliseconds(50))
        }
        try await settled()
        let after = try await client.snapshot()
        let afterCursor = try await cursor()
        try require(try finalEvidenceIndexed(), "The final ownership evidence and drained journal must remain current")
        try require(!after.scanning && !after.cleaning && after.error == nil && after.stats.complete
                    && !after.stats.cancelled && after.stats.errors == 0 && after.stats.entries > before.stats.entries,
                    "Ownership events must finish real scope work without losing complete coverage")
        try require(after.foregroundScan == before.foregroundScan && model.discoveryPresentation == presentation
                    && after.roots == before.roots && after.candidates == before.candidates,
                    "Ownership bursts must preserve the full-scan presentation and unrelated candidate proof")
        try require(after.wallet == before.wallet && after.history == before.history && after.keptPaths == before.keptPaths
                    && after.wallet == Wallet() && afterCursor > beforeCursor,
                    "Ownership refresh must advance its durable cursor without changing the ledger")

        let manifestAfter = try metadata(manifest.path)
        let sourceAfter = try metadata(source.path)
        let payloadAfter = try metadata(payload.path)
        let artifactAfter = try metadata(candidate.path, type: mode_t(S_IFDIR))
        let manifestContents = try Data(contentsOf: manifest)
        try require(manifestAfter.st_dev == manifestBefore.st_dev && manifestAfter.st_ino == manifestBefore.st_ino
                    && manifestAfter.st_mode == manifestBefore.st_mode && manifestAfter.st_size == manifestBefore.st_size
                    && identity(manifestAfter).modifiedNs == expectedModifiedNs && manifestContents == expectedManifest,
                    "Manifest events must preserve the owned file identity and exact final contents")
        let changedArtifactAfter = try metadata(changedArtifact.path, type: mode_t(S_IFDIR))
        let changedMarkerAfter = try metadata(changedMarker.path)
        for (before, after) in [(sourceBefore, sourceAfter), (payloadBefore, payloadAfter), (artifactBefore, artifactAfter),
                                (changedArtifactBefore, changedArtifactAfter), (changedMarkerBefore, changedMarkerAfter)] {
            try require(identity(before) == identity(after) && before.st_flags == after.st_flags && before.st_nlink == after.st_nlink,
                        "Ownership refresh must not change the separate artifact payload or source identity")
        }
        try require(try Data(contentsOf: source) == Data("Preserve this sibling".utf8),
                    "Ownership refresh must preserve the unrelated source contents")
        try require(try Data(contentsOf: changedMarker) == Data("Signature: 8a477f597d28d172789f06886806bc55\n".utf8),
                    "Ownership refresh must preserve the diagnostic artifact marker")
        try await Task.detached(priority: .utility) {
            let file = try FileHandle(forReadingFrom: payload)
            defer { try? file.close() }
            var total = 0
            while let chunk = try file.read(upToCount: 1_048_576), !chunk.isEmpty {
                try require(chunk == Data(repeating: 0x39, count: chunk.count), "Ownership updates changed the preserved payload")
                total += chunk.count
                try require(total <= 100 * 1_048_576, "Ownership updates enlarged the preserved payload")
            }
            try require(total == 100 * 1_048_576, "Ownership updates changed the preserved payload length")
            let debug = payload.deletingLastPathComponent()
            let expected = Data(repeating: 0x40, count: 4096)
            for index in 0..<4096 {
                try require(try Data(contentsOf: debug.appendingPathComponent("file-\(index)")) == expected,
                            "Ownership updates changed a preserved artifact entry")
            }
        }.value
        let entries = after.stats.entries - before.stats.entries
        let inferredPasses = entries % 3 == 0 ? String(entries / 3) : "unavailable"
        print("Native ownership events: writes=\(writes) spacing_ms=400 refreshed_entries=\(entries) inferred_three_entry_scope_passes=\(inferredPasses) final_write_indexed=true cursor_advanced=true foreground_and_candidate_unchanged=true journal_drained=true files_preserved=true ledger_unchanged=true (integration only; FSEvents may coalesce)")
    }

    @MainActor private static func collectionScreenshotPreservesLibrary(model: AppModel, base: URL, candidate: Candidate) async throws {
        guard let client = model.client else { throw EngineError.message("Collection capture needs the disposable interaction engine") }
        let before = try await client.snapshot()
        try require(!model.busy && !model.isCleaning && !before.scanning && before.error == nil
                    && before.roots.count == 1 && before.roots[0].path == base.appendingPathComponent("Projects").path
                    && before.candidates == [candidate] && candidate.eligiblePermanent && candidate.recommended,
                    "Capture regression needs an idle, eligible candidate outside the supplied screenshot fixture")
        let captureRoot = base.appendingPathComponent("Capture/baseline")
        let marker = captureRoot.deletingLastPathComponent().appendingPathComponent(".chippytea-benchmark-fixture.json")
        let markerData = try JSONSerialization.data(withJSONObject: ["magic": "chippytea-disposable-benchmark-v1",
            "status": "complete", "baseline_relative_path": "baseline"], options: [.sortedKeys])
        let preservedCapture = captureRoot.appendingPathComponent("preserve.txt")
        let captureData = Data("Preserve the supplied capture fixture too".utf8)
        try FileManager.default.createDirectory(at: captureRoot, withIntermediateDirectories: true)
        try markerData.write(to: marker)
        try captureData.write(to: preservedCapture)
        let payload = base.appendingPathComponent("Projects/Disposable/target/debug/payload")
        let sibling = base.appendingPathComponent("Projects/Disposable/preserve.txt")
        let artifact = URL(fileURLWithPath: candidate.path)
        func auditFiles() async throws -> [EngineIdentity] {
            try await Task.detached(priority: .utility) {
                var identities: [EngineIdentity] = []
                for (path, type) in [(artifact, S_IFDIR), (payload, S_IFREG), (sibling, S_IFREG),
                                     (captureRoot, S_IFDIR), (marker, S_IFREG), (preservedCapture, S_IFREG)] {
                    var value = stat()
                    try require(lstat(path.path, &value) == 0 && value.st_uid == geteuid()
                                && value.st_mode & mode_t(S_IFMT) == mode_t(type) && value.st_size >= 0,
                                "Collection capture changed a disposable physical file")
                    identities.append(EngineIdentity(device: UInt64(value.st_dev), inode: UInt64(value.st_ino),
                        mode: UInt32(value.st_mode), size: UInt64(value.st_size),
                        modifiedNs: Int64(value.st_mtimespec.tv_sec) * 1_000_000_000 + Int64(value.st_mtimespec.tv_nsec),
                        changedNs: Int64(value.st_ctimespec.tv_sec) * 1_000_000_000 + Int64(value.st_ctimespec.tv_nsec)))
                }
                let file = try FileHandle(forReadingFrom: payload)
                defer { try? file.close() }
                let expected = Data(repeating: 0x39, count: 1_048_576)
                var count = 0
                while let bytes = try file.read(upToCount: expected.count), !bytes.isEmpty {
                    try require(bytes == expected.prefix(bytes.count), "Collection capture changed the eligible payload")
                    count += bytes.count
                }
                try require(count == 100 * 1_048_576, "Collection capture changed the eligible payload length")
                try require(try Data(contentsOf: sibling) == Data("Preserve this sibling".utf8)
                            && Data(contentsOf: marker) == markerData && Data(contentsOf: preservedCapture) == captureData,
                            "Collection capture changed a sibling or the supplied fixture")
                return identities
            }.value
        }
        let filesBefore = try await auditFiles()
        var connection: OpaquePointer?
        let database = base.appendingPathComponent("State/library.sqlite")
        guard sqlite3_open_v2(database.path, &connection, SQLITE_OPEN_READONLY | SQLITE_OPEN_NOFOLLOW, nil) == SQLITE_OK else {
            if let connection { sqlite3_close(connection) }
            throw EngineError.message("Cannot inspect the disposable capture ledger")
        }
        defer { sqlite3_close(connection) }
        var statement: OpaquePointer?
        let sql = """
            SELECT (SELECT count(*) FROM operations), (SELECT count(*) FROM earnings),
                   (SELECT count(*) FROM windows), (SELECT count(*) FROM allocations),
                   (SELECT count(*) FROM wallet), (SELECT collected FROM wallet WHERE id=1),
                   (SELECT remainder FROM wallet WHERE id=1), (SELECT credited FROM wallet WHERE id=1)
            """
        try require(sqlite3_prepare_v2(connection, sql, -1, &statement, nil) == SQLITE_OK, "Cannot query the disposable capture ledger")
        defer { sqlite3_finalize(statement) }
        func ledger() throws -> [Int64] {
            defer { sqlite3_reset(statement) }
            try require(sqlite3_step(statement) == SQLITE_ROW, "Cannot read the disposable capture ledger")
            let columns = 0..<sqlite3_column_count(statement)
            try require(columns.allSatisfy { sqlite3_column_type(statement, $0) == SQLITE_INTEGER }, "Capture ledger fields must remain present")
            return columns.map { sqlite3_column_int64(statement, $0) }
        }
        let ledgerBefore = try ledger()
        try require(ledgerBefore == [0, 0, 0, 0, 1, 0, 0, 0], "Capture regression must start before any cleanup or credit")
        let delegate = AppDelegate(model: model)
        let presentedBefore = model.snapshot
        let selectionBefore = model.selection
        let reviewBefore = model.reviewItems
        for environment in [
            ["CHIPPYTEA_SCREENSHOT_STATE": "collect", "CHIPPYTEA_SCREENSHOT_ROOT": captureRoot.path],
            ["CHIPPYTEA_SCREENSHOT_STATE": "collect"]
        ] {
            await delegate.stageScreenshot(environment: environment)
            guard let burst = model.collection else { throw EngineError.message("Collection capture must stage an animation without cleanup") }
            try require(burst.amount > 0 && burst.to >= burst.from && burst.to - burst.from == burst.amount,
                        "Collection capture must have a valid synthetic counter range")
            try require(model.destination == .coins && !model.showDiskAccess && !model.showReview
                        && !model.busy && !model.isCleaning && !model.snapshot.scanning
                        && !model.discoveryPresentation.isRequestPending && model.cleanupCandidateIDs.isEmpty
                        && model.snapshot == presentedBefore && model.selection == selectionBefore && model.reviewItems == reviewBefore,
                        "Collection capture must not submit review, authorize, scan, clean or change the wallet")
            model.finishCollection()
        }
        let after = try await client.snapshot()
        let ledgerAfter = try ledger()
        let filesAfter = try await auditFiles()
        try require(after == before && ledgerAfter == ledgerBefore, "Collection capture changed the real engine state in the disposable library")
        try require(filesAfter == filesBefore, "Collection capture changed disposable file identities")

        let empty = AppModel(directory: base.appendingPathComponent("EmptyCaptureState"), scanHome: base.appendingPathComponent("UnusedHome"))
        await empty.start()
        guard let emptyClient = empty.client else { throw EngineError.message("Cannot open the empty disposable capture library") }
        let emptyBefore = try await emptyClient.snapshot()
        try require(emptyBefore.roots.isEmpty && !empty.busy && !emptyBefore.scanning, "Empty capture setup must have no grants")
        await AppDelegate(model: empty).stageScreenshot(environment: ["CHIPPYTEA_SCREENSHOT_STATE": "collect", "CHIPPYTEA_SCREENSHOT_ROOT": captureRoot.path])
        let emptyAfter = try await emptyClient.snapshot()
        try require(empty.collection != nil && !empty.busy && !empty.isCleaning && emptyAfter == emptyBefore,
                    "Collection capture must return before authorizing or scanning its supplied root")
        empty.finishCollection()

        // An empty or maximum-balance preview needs neither a grant nor an engine.
        let unopened = AppModel(directory: base.appendingPathComponent("UnusedCaptureState"), scanHome: base.appendingPathComponent("UnusedHome"))
        let preview = AppDelegate(model: unopened)
        for balance in [UInt64(0), UInt64.max] {
            unopened.snapshot.wallet.collectedCoins = balance
            let snapshot = unopened.snapshot
            await preview.stageScreenshot(environment: ["CHIPPYTEA_SCREENSHOT_STATE": "collect", "CHIPPYTEA_SCREENSHOT_ROOT": captureRoot.path])
            guard let burst = unopened.collection else { throw EngineError.message("Collection preview requires no authorized roots") }
            try require(burst.to == max(balance, 8) && burst.amount == 8 && burst.from == burst.to - 8
                        && unopened.snapshot == snapshot && unopened.client == nil && !unopened.busy && !unopened.isCleaning,
                        "Collection preview must handle UInt64 bounds without opening or changing a library")
            unopened.finishCollection()
        }
        print("PASS native collection capture: presentation only; eligible files, authorization and ledger preserved")
    }

    /// Gates injected capacity reads without ever querying a real volume.
    private final class StorageQueryProbe: @unchecked Sendable {
        struct Observation {
            var started = 0
            var completed = 0
            var active = 0
            var maximumActive = 0
            var ranOnMain = false
            var timedOut = false
        }
        private let lock = NSLock()
        private var observation = Observation()
        private let values: [StorageStatus?]
        private let gates: [DispatchSemaphore]

        init(values: [StorageStatus?]) {
            self.values = values
            gates = values.map { _ in DispatchSemaphore(value: 0) }
        }
        private func update<T>(_ body: (inout Observation) -> T) -> T {
            lock.lock()
            defer { lock.unlock() }
            return body(&observation)
        }
        func snapshot() -> Observation { update { $0 } }
        func release(_ index: Int) { gates[index].signal() }
        func releaseAll() { gates.forEach { $0.signal() } }
        func read() -> StorageStatus? {
            let index = update { value in
                let index = value.started
                value.started += 1
                value.active += 1
                value.maximumActive = max(value.maximumActive, value.active)
                value.ranOnMain = value.ranOnMain || Thread.isMainThread
                return index
            }
            let timedOut = index >= gates.count || gates[index].wait(timeout: .now() + 2) != .success
            update {
                $0.active -= 1
                $0.completed += 1
                $0.timedOut = $0.timedOut || timedOut
            }
            return index < values.count ? values[index] : nil
        }
    }

    @MainActor private static func storageStatusSampling() async throws {
        guard let available = StorageStatus(blockSize: 4096, totalBlocks: 4096, availableBlocks: 2048),
              let stale = StorageStatus(blockSize: 4096, totalBlocks: 4096, availableBlocks: 1024),
              let full = StorageStatus(blockSize: 4096, totalBlocks: 4096, availableBlocks: 0) else {
            throw EngineError.message("Valid capacity counters were rejected")
        }
        try require(available.totalBytes == 16_777_216 && available.availableBytes == 8_388_608
                    && available.unavailableBytes == 8_388_608 && available.menuTitle == "<1 GB free"
                    && full.availableBytes == 0 && full.unavailableBytes == full.totalBytes,
                    "Capacity must preserve zero availability and checked block arithmetic")
        for (blockSize, total, free) in [(UInt64(0), UInt64(1), UInt64(0)),
                                         (UInt64(UInt32.max), 1, 0), (4096, 0, 0),
                                         (4096, UInt64.max, 0), (4096, 1, UInt64.max),
                                         (4096, 1, 2), (4096, UInt64.max / 4096 + 1, 0)] {
            try require(StorageStatus(blockSize: blockSize, totalBlocks: total, availableBlocks: free) == nil,
                        "Unavailable, inconsistent or overflowing capacity counters must not become a disk size")
        }

        let probe = StorageQueryProbe(values: [nil, available, available, stale, full])
        var delivered: [StorageStatus?] = []
        var callbacksOnMain = true
        let monitor = StorageStatusMonitor(query: { probe.read() }) { value in
            callbacksOnMain = callbacksOnMain && Thread.isMainThread
            delivered.append(value)
        }
        defer { monitor.stop(); probe.releaseAll() }
        func until(_ message: String, _ condition: () -> Bool) async throws {
            let deadline = ProcessInfo.processInfo.systemUptime + 2
            while !condition() {
                try require(ProcessInfo.processInfo.systemUptime < deadline, message)
                try await Task.sleep(for: .milliseconds(10))
            }
        }
        monitor.start()
        monitor.start()
        try await until("The initial capacity query did not start") { probe.snapshot().started == 1 }
        for _ in 0..<20 { monitor.refresh() }
        try await Task.sleep(for: .milliseconds(20))
        try require(probe.snapshot().started == 1 && delivered.isEmpty,
                    "Repeated starts and refreshes must not overlap the first capacity read")
        probe.release(0)
        try await until("A burst of capacity refreshes did not coalesce") {
            delivered.count == 1 && probe.snapshot().started == 2
        }
        try require(delivered[0] == nil, "Unavailable capacity must be reported distinctly from zero free space")
        probe.release(1)
        try await until("The coalesced capacity result did not arrive") { delivered.count == 2 }
        try require(delivered[1] == available && probe.snapshot().started == 2,
                    "A refresh burst must request exactly one follow-up")

        monitor.refresh()
        try await until("The unchanged capacity query did not start") { probe.snapshot().started == 3 }
        probe.release(2)
        try await until("The unchanged capacity query did not finish") { probe.snapshot().completed == 3 }
        // Starting the next read also proves the unchanged result has passed
        // through the monitor's main-queue completion, not just the probe.
        monitor.refresh()
        try await until("The pre-stop capacity query did not start") { probe.snapshot().started == 4 }
        try require(delivered.count == 2, "Unchanged capacity must not publish another menu update")
        monitor.stop()
        monitor.refresh()
        monitor.start()
        monitor.start()
        for _ in 0..<20 { monitor.refresh() }
        try await Task.sleep(for: .milliseconds(20))
        try require(probe.snapshot().started == 4, "Restart must wait for an outstanding capacity query")
        probe.release(3)
        try await until("Restart did not schedule one fresh capacity query") { probe.snapshot().started == 5 }
        try require(delivered.count == 2, "A result from the stopped generation must not update the menu")
        probe.release(4)
        try await until("Restarted capacity sampling did not publish its result") { delivered.count == 3 }
        monitor.stop()
        monitor.refresh()
        try await Task.sleep(for: .milliseconds(20))
        let observation = probe.snapshot()
        try require(delivered[2] == full && callbacksOnMain && !observation.ranOnMain
                    && observation.started == 5 && observation.completed == 5
                    && observation.active == 0 && observation.maximumActive == 1 && !observation.timedOut,
                    "Capacity reads must stay off-main, serialize, discard stale generations and stop cleanly")
        print("PASS native storage sampling: checked counters, off-main reads, coalesced refreshes and stale-stop rejection")
    }

    @MainActor private static func cleanupPresentationEstimates(candidate: Candidate, base: URL) throws {
        var item = candidate
        item.allocatedBytes = 40_000_000
        let wallet = Wallet(collectedCoins: 7, pendingCoins: 3, fractionalBytes: 60_000_000, creditedBytes: 1_060_000_000)
        let fractional = CleanupPreview(items: [item], wallet: wallet, permanently: true)
        try require(fractional.estimatedCoins == 1 && fractional.estimatedBytes == 40_000_000
                    && fractional.itemCount == 1 && fractional.isPermanent && fractional.from == 10
                    && fractional.estimatedTo == 11 && fractional.targetCoins == 11
                    && fractional.resolvedTo == nil && !fractional.presentationFinished,
                    "A cleanup estimate must carry only the fractional bytes, not existing earned coins")
        let collectedFloor = CleanupPreview(items: [item], wallet: wallet, permanently: true, collectedCoinsFloor: 30)
        try require(collectedFloor.from == 30 && collectedFloor.targetCoins == 31,
                    "A confirmed collection response must take precedence over an older wallet snapshot")
        var zeroRecovery = fractional
        zeroRecovery.resolvedTo = fractional.from
        try require(zeroRecovery.id == fractional.id && zeroRecovery.targetCoins == 10 && zeroRecovery.estimatedTo == 11,
                    "Zero verified recovery must correct the target without replacing the presentation identity")
        item.allocatedBytes -= 1
        try require(CleanupPreview(items: [item], wallet: wallet, permanently: true).estimatedCoins == 0,
                    "A cleanup estimate must not round up below a whole coin")
        item.allocatedBytes = 100_000_000
        let trash = CleanupPreview(items: [item], wallet: wallet, permanently: false)
        try require(trash.estimatedCoins == 0 && trash.estimatedBytes == item.allocatedBytes && !trash.isPermanent
                    && trash.targetCoins == trash.from,
                    "Trash may estimate bytes but never coins")
        item.eligiblePermanent = false
        try require(CleanupPreview(items: [candidate, item], wallet: wallet, permanently: true).estimatedCoins == 0,
                    "A mixed ineligible review must not promise permanent-cleanup coins")
        item.eligiblePermanent = true
        item.blockedReason = "Disposable blocked estimate"
        try require(CleanupPreview(items: [item], wallet: wallet, permanently: true).estimatedCoins == 0,
                    "Blocked items must not promise cleanup coins")
        item.blockedReason = nil
        item.allocatedBytes = 1
        let invalidRemainder = Wallet(fractionalBytes: .max)
        try require(CleanupPreview(items: [item], wallet: invalidRemainder, permanently: true).estimatedCoins == 0,
                    "An invalid remainder must not overflow or manufacture an estimate")
        item.allocatedBytes = .max
        let maximum = CleanupPreview(items: [item, item], wallet: Wallet(fractionalBytes: 99_999_999), permanently: true)
        try require(maximum.estimatedBytes == .max && maximum.estimatedCoins == UInt64.max / 100_000_000 + 1,
                    "Large review totals must saturate bytes and calculate whole coins without overflowing")
        let fullWallet = CleanupPreview(items: [item], wallet: Wallet(collectedCoins: .max, pendingCoins: 1), permanently: true)
        let nearlyFull = CleanupPreview(items: [item], wallet: Wallet(collectedCoins: .max - 1), permanently: true)
        try require(fullWallet.from == UInt64.max && fullWallet.targetCoins == UInt64.max
                    && nearlyFull.from == UInt64.max - 1 && nearlyFull.targetCoins == UInt64.max,
                    "Both the starting balance and estimated target must saturate without wrapping")
        var partialRecovery = maximum
        partialRecovery.resolvedTo = 1
        try require(partialRecovery.targetCoins == 1 && partialRecovery.estimatedTo == maximum.estimatedTo,
                    "A partial recovery must show its verified target instead of retaining the estimate")
        let empty = CleanupPreview(items: [], wallet: wallet, permanently: true)
        try require(empty.estimatedCoins == 0 && empty.estimatedBytes == 0 && empty.itemCount == 0 && empty.targetCoins == 10,
                    "An empty cleanup must not promise a coin from the existing wallet")
        try require(fractional.id != CleanupPreview(items: [candidate], wallet: wallet, permanently: true).id,
                    "Every cleanup attempt needs its own presentation identity")

        // These are presentation-only values: this model never opens a library.
        let unopened = AppModel(directory: base.appendingPathComponent("UnusedPresentationState"))
        let old = CollectionBurst(from: 0, to: 1, amount: 1)
        let current = CollectionBurst(from: 1, to: 2, amount: 1)
        unopened.collection = current
        unopened.finishCollection(id: old.id)
        try require(unopened.collection?.id == current.id, "An old animation must not finish its replacement")
        unopened.finishCollection(id: current.id)
        try require(unopened.collection == nil && unopened.client == nil && unopened.snapshot.wallet == Wallet(),
                    "Finishing an animation must not open a library or change earned rewards")
    }

    @MainActor private static func cleanupInteraction(includeDiscoveryRegressions: Bool = true) async throws {
        let base = try await Task.detached(priority: .utility) {
            let fm = FileManager.default
            guard let physical = realpath(fm.temporaryDirectory.path, nil) else {
                throw EngineError.message("Cannot resolve disposable interaction directory")
            }
            let temporary = URL(fileURLWithPath: String(cString: physical))
            free(physical)
            let available = try temporary.resourceValues(forKeys: [.volumeAvailableCapacityKey]).volumeAvailableCapacity ?? 0
            try require(available > 3 * 1024 * 1024 * 1024 + 200_000_000, "The interaction fixture requires a 3 GiB reserve")
            let base = temporary.appendingPathComponent("chippytea-interaction-\(UUID().uuidString)")
            let project = base.appendingPathComponent("Projects/Disposable")
            let target = project.appendingPathComponent("target")
            try fm.createDirectory(at: target.appendingPathComponent("debug"), withIntermediateDirectories: true)
            let changed = base.appendingPathComponent("Projects/Changed")
            let changedTarget = changed.appendingPathComponent("target")
            try fm.createDirectory(at: changedTarget, withIntermediateDirectories: true)
            try Data("[package]\nname=\"changed\"\nversion=\"0.0.0\"\n".utf8).write(to: changed.appendingPathComponent("Cargo.toml"))
            try Data("Signature: 8a477f597d28d172789f06886806bc55\n".utf8).write(to: changedTarget.appendingPathComponent("CACHEDIR.TAG"))
            try Data("Disposable chippytea interaction test".utf8).write(to: base.appendingPathComponent(".chippytea-fixture"))
            try Data("[package]\nname=\"disposable\"\nversion=\"0.1.0\"\n".utf8).write(to: project.appendingPathComponent("Cargo.toml"))
            try Data("Signature: 8a477f597d28d172789f06886806bc55\n".utf8).write(to: target.appendingPathComponent("CACHEDIR.TAG"))
            try Data("Preserve this sibling".utf8).write(to: project.appendingPathComponent("preserve.txt"))
            let payload = target.appendingPathComponent("debug/payload")
            fm.createFile(atPath: payload.path, contents: nil)
            let file = try FileHandle(forWritingTo: payload)
            let bytes = Data(repeating: 0x39, count: 1_048_576)
            for _ in 0..<100 { try file.write(contentsOf: bytes) }
            try file.synchronize(); try file.close()
            let old = Date().addingTimeInterval(-9 * 86_400)
            for path in [changedTarget.appendingPathComponent("CACHEDIR.TAG"), changedTarget] {
                try fm.setAttributes([.modificationDate: old], ofItemAtPath: path.path)
            }
            for index in 0..<4096 {
                let path = target.appendingPathComponent("debug/file-\(index)")
                try Data(repeating: 0x40, count: 4096).write(to: path)
                try fm.setAttributes([.modificationDate: old], ofItemAtPath: path.path)
            }
            for path in [payload, target.appendingPathComponent("CACHEDIR.TAG"), project.appendingPathComponent("Cargo.toml"), target.appendingPathComponent("debug"), target, project] {
                try fm.setAttributes([.modificationDate: old], ofItemAtPath: path.path)
            }
            return base
        }.value
        print("Disposable interaction evidence: \(base.path)")
        var failingReads = 0
        var failedReads = 0
        var holdCollectRead = false
        var heldSnapshot: EngineSnapshot?
        var heldRead: CheckedContinuation<EngineSnapshot, Error>?
        var heldReadReturned = false
        var completedReads = 0
        let confirmationPreference = UserDefaults.standard.object(forKey: "confirmBeforeDeleting")
        UserDefaults.standard.removeObject(forKey: "confirmBeforeDeleting")
        let model = AppModel(directory: base.appendingPathComponent("State"), scanHome: base.appendingPathComponent("UnusedHome")) { client in
            if failingReads > 0 {
                failingReads -= 1
                failedReads += 1
                throw EngineError.message("Disposable cleanup snapshot failure")
            }
            let current = try await client.snapshot()
            if holdCollectRead {
                holdCollectRead = false
                heldSnapshot = current
                // Hold delivery, not the engine queue. The older collect has
                // really run and its real zero-pending result is now stale.
                let delivered: EngineSnapshot = try await withCheckedThrowingContinuation { heldRead = $0 }
                heldReadReturned = true
                completedReads += 1
                return delivered
            }
            completedReads += 1
            return current
        }
        let soundPreference = UserDefaults.standard.object(forKey: "soundEnabled")
        let soundEnabled = model.soundEnabled
        model.soundEnabled = false
        defer {
            holdCollectRead = false
            heldRead?.resume(throwing: EngineError.message("Disposable cleanup fixture finished"))
            model.windowClosed()
            model.soundEnabled = soundEnabled
            if let soundPreference { UserDefaults.standard.set(soundPreference, forKey: "soundEnabled") }
            else { UserDefaults.standard.removeObject(forKey: "soundEnabled") }
            if let confirmationPreference { UserDefaults.standard.set(confirmationPreference, forKey: "confirmBeforeDeleting") }
            else { UserDefaults.standard.removeObject(forKey: "confirmBeforeDeleting") }
        }
        try require(model.confirmBeforeDeleting, "A missing confirmation preference must default to review")
        model.confirmBeforeDeleting = true
        await model.start()
        guard let client = model.client else { throw EngineError.message("Interaction engine did not start") }
        func until(_ message: String, _ condition: () -> Bool) async throws {
            let deadline = Date().addingTimeInterval(15)
            while !condition() {
                try require(Date() < deadline, message)
                try await Task.sleep(for: .milliseconds(20))
            }
        }
        func settleReads() async throws {
            let deadline = Date().addingTimeInterval(10)
            var previous = model.snapshotRequestID
            var stableSince = ProcessInfo.processInfo.systemUptime
            while ProcessInfo.processInfo.systemUptime - stableSince < 0.3 {
                try require(Date() < deadline, "Interaction snapshot work did not settle")
                try await Task.sleep(for: .milliseconds(20))
                if previous != model.snapshotRequestID || model.busy || model.snapshot.scanning {
                    previous = model.snapshotRequestID
                    stableSince = ProcessInfo.processInfo.systemUptime
                }
            }
        }
        model.authorizeAndScan(path: base.appendingPathComponent("Projects").path)
        let discoveryDeadline = Date().addingTimeInterval(30)
        while model.busy || model.snapshot.scanning || model.snapshot.candidates.isEmpty {
            try require(Date() < discoveryDeadline && model.errorMessage == nil, "Interaction fixture did not become reviewable: \(model.errorMessage ?? "timeout")")
            try await Task.sleep(for: .milliseconds(20))
        }
        guard let candidate = model.snapshot.candidates.first else { throw EngineError.message("No interaction candidate") }
        if includeDiscoveryRegressions {
            try await ownershipEventBurst(model: model, base: base, candidate: candidate)
            try await collectionScreenshotPreservesLibrary(model: model, base: base, candidate: candidate)
        }
        try cleanupPresentationEstimates(candidate: candidate, base: base)
        try await settleReads()
        let beforeCleanup = try await client.snapshot()
        try require(beforeCleanup.candidates == [candidate] && beforeCleanup.wallet == Wallet()
                    && beforeCleanup.history.isEmpty && candidate.eligiblePermanent,
                    "Background cleanup requires a real eligible candidate and an untouched ledger")
        let preserved = base.appendingPathComponent("Projects/Disposable/preserve.txt")
        let artifactPaths = [URL(fileURLWithPath: candidate.path),
            base.appendingPathComponent("Projects/Disposable/target/debug"),
            base.appendingPathComponent("Projects/Disposable/target/debug/payload"),
            base.appendingPathComponent("Projects/Disposable/target/debug/file-0"),
            base.appendingPathComponent("Projects/Disposable/target/debug/file-4095"),
            base.appendingPathComponent("Projects/Disposable/target/CACHEDIR.TAG"),
            base.appendingPathComponent("Projects/Disposable/Cargo.toml"), preserved]
        func identities() throws -> [EngineIdentity] {
            try artifactPaths.map { path in
                var value = stat()
                try require(lstat(path.path, &value) == 0 && value.st_uid == geteuid() && value.st_size >= 0,
                            "Cleanup preparation changed a disposable file")
                return EngineIdentity(device: UInt64(value.st_dev), inode: UInt64(value.st_ino), mode: UInt32(value.st_mode),
                    size: UInt64(value.st_size),
                    modifiedNs: Int64(value.st_mtimespec.tv_sec) * 1_000_000_000 + Int64(value.st_mtimespec.tv_nsec),
                    changedNs: Int64(value.st_ctimespec.tv_sec) * 1_000_000_000 + Int64(value.st_ctimespec.tv_nsec))
            }
        }
        let beforeIdentities = try identities()

        // A rejected prepare still needs authoritative reconciliation. Two
        // failed reads must retain its reservation without retrying forever.
        var outdated = candidate
        outdated.fingerprint += "-outdated-review"
        model.requestCleanupOne(candidate)
        try require(model.showReview && !model.isCleaning && model.cleanupPreview == nil,
                    "Enabled confirmation must open review before starting cleanup")
        model.reviewItems = [outdated]
        model.destination = .discover
        failingReads = 2
        model.clean(permanently: true)
        guard let rejectedPreview = model.cleanupPreview else { throw EngineError.message("Missing rejected cleanup preview") }
        try require(model.destination == .coins && model.busy && model.isCleaning && !model.showReview
                    && model.displayedCandidates.isEmpty && model.snapshot.wallet == beforeCleanup.wallet && model.collection == nil,
                    "Confirmation must immediately return to the stash without claiming recovery")
        try require(!model.claimCleanupPreview(rejectedPreview.id), "A hidden window must not consume its preview")
        model.visible = true
        try require(!model.claimCleanupPreview(rejectedPreview.id), "A hidden panel must not consume its preview")
        model.panelVisible = true
        try require(model.claimCleanupPreview(rejectedPreview.id) && !model.claimCleanupPreview(rejectedPreview.id),
                    "A visible cleanup preview must be claimable exactly once")
        try await until("The rejected prepare did not reach both failed final reads") { failedReads == 2 && !model.isCleaning }
        let failedRequestID = model.snapshotRequestID
        try await Task.sleep(for: .milliseconds(120))
        try require(model.busy && model.cleanupPreview?.id == rejectedPreview.id && model.displayedCandidates.isEmpty
                    && model.snapshotRequestID == failedRequestID && model.snapshot.wallet == beforeCleanup.wallet,
                    "Failed final reads must retain the pending state without an idle retry loop")
        model.clean(permanently: true)
        try require(model.cleanupPreview?.id == rejectedPreview.id, "Unreconciled cleanup must reject another mutation")
        model.windowClosed()
        model.destination = .coins
        model.windowOpened()
        try require(!model.claimCleanupPreview(rejectedPreview.id), "Reopening must not replay a pending preview")
        model.destination = .activity
        try await until("Reopening did not reconcile the rejected cleanup") { !model.busy && model.cleanupPreview == nil }
        let rejected = try await client.snapshot()
        try require(rejected.wallet == beforeCleanup.wallet && rejected.history == beforeCleanup.history
                    && rejected.candidates == beforeCleanup.candidates && model.destination == .activity
                    && model.cleanupCandidateIDs.isEmpty && model.errorMessage != nil,
                    "Prepare rejection must restore recommendations without rewards or a page change")
        model.errorMessage = nil

        // Opting out only bypasses review for eligible permanent cleanup. This
        // attempt is cancelled synchronously before its token can be executed.
        model.confirmBeforeDeleting = false
        let reopenedPreferences = AppModel(directory: base.appendingPathComponent("UnusedPreferenceState"))
        try require(UserDefaults.standard.object(forKey: "confirmBeforeDeleting") as? Bool == false
                    && !reopenedPreferences.confirmBeforeDeleting && reopenedPreferences.client == nil,
                    "A new model must read the persisted confirmation choice without opening a library")
        model.reviewOne(candidate)
        try require(model.showReview && !model.busy && !model.isCleaning && model.cleanupPreview == nil
                    && model.snapshot.wallet == beforeCleanup.wallet && model.snapshot.history == beforeCleanup.history,
                    "Presentation-only review must not start deletion when confirmation is disabled")
        model.showReview = false
        var reviewOnly = candidate
        reviewOnly.eligiblePermanent = false
        let beforeReview = model.snapshot
        model.snapshot.candidates = [reviewOnly]
        model.requestCleanupOne(reviewOnly)
        try require(model.showReview && !model.busy && !model.isCleaning && model.cleanupPreview == nil
                    && model.reviewItems == [reviewOnly],
                    "Ineligible permanent cleanup must still require review when confirmation is disabled")
        model.showReview = false
        model.snapshot = beforeReview
        model.requestCleanupOne(candidate)
        guard let cancelledPreview = model.cleanupPreview else { throw EngineError.message("Missing cancelled cleanup preview") }
        try require(cancelledPreview.id != rejectedPreview.id && model.busy && model.isCleaning
                    && !model.showReview && cancelledPreview.isPermanent,
                    "An eligible opt-out action must immediately start a new permanent cleanup")
        model.finishCleanupPreview(rejectedPreview.id)
        try require(model.cleanupPreview?.id == cancelledPreview.id && model.cleanupPreview?.presentationFinished == false,
                    "An old presentation completion must not finish a new cleanup")
        model.windowClosed()
        model.windowOpened()
        try require(!model.claimCleanupPreview(cancelledPreview.id), "An unshown preview must not appear late after reopening")
        model.cancel()
        model.destination = .activity
        try await until("Pre-execute cancellation did not reconcile") { !model.busy && !model.isCleaning && model.cleanupPreview == nil }
        let cancelled = try await client.snapshot()
        try require(cancelled.wallet == beforeCleanup.wallet && cancelled.history == beforeCleanup.history
                    && cancelled.candidates == beforeCleanup.candidates && model.destination == .activity
                    && model.cleanupCandidateIDs.isEmpty && model.errorMessage != nil,
                    "Cancellation before execute must preserve files, recommendations and earned rewards")
        try require(try identities() == beforeIdentities && String(contentsOf: preserved, encoding: .utf8) == "Preserve this sibling",
                    "Rejected and cancelled cleanup must preserve the artifact, payload and sibling identities")
        model.errorMessage = nil
        try await settleReads()
        model.confirmBeforeDeleting = true
        model.requestCleanupOne(candidate)
        try require(model.showReview && !model.busy && !model.isCleaning && model.cleanupPreview == nil
                    && UserDefaults.standard.object(forKey: "confirmBeforeDeleting") as? Bool == true
                    && AppModel(directory: base.appendingPathComponent("UnusedPreferenceState")).confirmBeforeDeleting,
                    "Restoring confirmation must persist and restore the review step immediately")
        model.showReview = false

        var presentations: [CollectionBurst] = []
        let collectionObserver = model.$collection.sink { if let burst = $0 { presentations.append(burst) } }
        defer { collectionObserver.cancel() }
        model.destination = .coins
        holdCollectRead = true
        model.collect()
        try await until("The older collect did not reach its final snapshot") { heldRead != nil }
        guard let stale = heldSnapshot else { throw EngineError.message("Missing real held collection snapshot") }
        try require(stale.wallet == Wallet() && stale.history.isEmpty && stale.candidates == [candidate],
                    "The collection race must begin from a real empty ledger, not synthetic rewards")
        let deadline = Date().addingTimeInterval(30)
        model.reviewOne(candidate)
        var usageBefore = rusage()
        let measuredCPU = getrusage(RUSAGE_SELF, &usageBefore) == 0
        let started = ProcessInfo.processInfo.systemUptime
        model.clean(permanently: true)
        let acknowledgment = (ProcessInfo.processInfo.systemUptime - started) * 1000
        try require(model.isCleaning && model.displayedCandidates.isEmpty && !model.showReview, "Confirmation must immediately show in-flight cleanup")
        guard let activePreview = model.cleanupPreview else { throw EngineError.message("Missing active cleanup preview") }
        try require(model.destination == .coins && model.snapshot.wallet == Wallet() && model.collection == nil
                    && activePreview.estimatedBytes == candidate.allocatedBytes && activePreview.estimatedCoins > 0
                    && activePreview.from == 0 && activePreview.resolvedTo == nil
                    && model.displayedCoinBalance == activePreview.estimatedTo,
                    "Pending feedback must show its estimate without claiming recovery or rewards")
        model.finishCleanupPreview(cancelledPreview.id)
        try require(!model.claimCleanupPreview(cancelledPreview.id) && model.cleanupPreview?.presentationFinished == false,
                    "An earlier cleanup must not claim or finish the current presentation")
        // Deliberately let execution finish before the first presentation claim.
        // Fast cleanup must not erase an animation SwiftUI has yet to start.
        // A request arriving while both cleanup and an older collect are active
        // must be retained once without replacing that initial collection.
        model.collect()
        model.collect()
        var phases = Set<String>()
        var heartbeats = 0
        var maximumGap = 0.0
        var previous = ProcessInfo.processInfo.systemUptime
        while model.busy || model.isCleaning {
            try require(Date() < deadline, "Native cleanup did not finish")
            try await Task.sleep(for: .milliseconds(20))
            let now = ProcessInfo.processInfo.systemUptime
            maximumGap = max(maximumGap, (now - previous) * 1000)
            previous = now
            heartbeats += 1
            if let progress = model.cleanupProgress { phases.insert(progress.phase) }
        }
        let cleanupSeconds = ProcessInfo.processInfo.systemUptime - started
        var usageAfter = rusage()
        var cpuSeconds: Double?
        if getrusage(RUSAGE_SELF, &usageAfter) == 0 && measuredCPU {
            cpuSeconds = Double(usageAfter.ru_utime.tv_sec - usageBefore.ru_utime.tv_sec)
                + Double(usageAfter.ru_utime.tv_usec - usageBefore.ru_utime.tv_usec) / 1_000_000
                + Double(usageAfter.ru_stime.tv_sec - usageBefore.ru_stime.tv_sec)
                + Double(usageAfter.ru_stime.tv_usec - usageBefore.ru_stime.tv_usec) / 1_000_000
        }
        try require(heartbeats > 1 && !phases.isEmpty, "Cleanup must deliver live progress while the main actor stays responsive")
        try require(model.snapshot.history.first?.outcome == "removed", "Native interaction cleanup failed: \(model.errorMessage ?? "missing receipt")")
        try require(!FileManager.default.fileExists(atPath: candidate.path), "The completed cleanup must actually remove its artifact")
        try require(try String(contentsOf: preserved, encoding: .utf8) == "Preserve this sibling", "Interaction cleanup touched a sibling")
        print("Native cleanup acknowledgment_ms=\(acknowledgment) cleanup_elapsed_seconds=\(cleanupSeconds) self_cpu_seconds=\(cpuSeconds.map { String($0) } ?? "unavailable") main_actor_max_gap_ms=\(maximumGap) heartbeat_samples=\(heartbeats) phases=\(phases.sorted())")

        let committed = try await client.snapshot()
        guard let receipt = committed.history.first else { throw EngineError.message("Missing committed cleanup receipt") }
        try require(committed.history.count == 1 && receipt.operation == "permanent" && receipt.outcome == "removed"
                    && committed.wallet.collectedCoins == 0 && committed.wallet.pendingCoins == receipt.coins
                    && committed.wallet.creditedBytes == receipt.creditedBytes
                    && committed.wallet.fractionalBytes == receipt.creditedBytes % 100_000_000
                    && receipt.coins == receipt.creditedBytes / 100_000_000,
                    "Only the real committed recovery may establish pending rewards and their remainder")
        let resolvedTarget = committed.wallet.collectedCoins + committed.wallet.pendingCoins
        try require(model.cleanupPreview?.id == activePreview.id && model.cleanupPreview?.resolvedTo == resolvedTarget
                    && model.cleanupPreview?.targetCoins == resolvedTarget && model.cleanupPreview?.presentationFinished == false
                    && model.displayedCoinBalance == resolvedTarget
                    && model.cleanupProgress == nil && model.cleanupCandidateIDs.isEmpty
                    && model.collection == nil && presentations.isEmpty && model.snapshot.wallet == committed.wallet,
                    "Completion must resolve the existing preview without erasing it or claiming collected rewards")
        try require(model.claimCleanupPreview(activePreview.id) && !model.claimCleanupPreview(activePreview.id),
                    "The initial collection must remain claimable once even after fast cleanup completes")
        var balanceChanged = false
        let balanceObserver = model.$cleanupPreview.combineLatest(model.$confirmedCollectedCoins, model.snapshots).sink { value in
            let (preview, confirmed, snapshot) = value
            let balance = preview?.targetCoins ?? max(confirmed, snapshot.wallet.collectedCoins)
            if balance != resolvedTarget { balanceChanged = true }
        }
        defer { balanceObserver.cancel() }
        model.finishCleanupPreview(cancelledPreview.id)
        try require(model.cleanupPreview?.presentationFinished == false,
                    "A stale presentation acknowledgment must not finish the retained preview")
        model.finishCleanupPreview(activePreview.id)
        if receipt.coins > 0 {
            try require(model.cleanupPreview?.id == activePreview.id && model.cleanupPreview?.presentationFinished == true
                        && model.confirmedCollectedCoins == 0 && model.displayedCoinBalance == resolvedTarget,
                        "A finished preview must bridge pending rewards until a real collect confirms their balance")
        } else {
            // APFS sharing, snapshots or concurrent disk activity can correctly
            // produce zero credit. Never manufacture coins to satisfy a test.
            try require(model.cleanupPreview == nil && model.displayedCoinBalance == 0,
                        "Unverified recovery must retire the finished estimate without earning coins")
        }
        var stalePublished = false
        let snapshotObserver = model.snapshots.sink { value in
            if value.wallet.creditedBytes < committed.wallet.creditedBytes || value.history != committed.history
                || value.candidates.contains(where: { $0.id == candidate.id }) { stalePublished = true }
        }
        defer { snapshotObserver.cancel() }
        // Releasing the stale response on another tab must neither roll back
        // the completed cleanup nor silently consume its earned collection.
        model.destination = .activity
        let continuation = heldRead
        heldRead = nil
        continuation?.resume(returning: stale)
        try await until("The held collection snapshot did not resume") { heldReadReturned }
        try await settleReads()
        let offTab = try await client.snapshot()
        try require(model.destination == .activity && model.collection == nil && presentations.isEmpty
                    && offTab.wallet == committed.wallet && !stalePublished && !balanceChanged
                    && model.displayedCoinBalance == resolvedTarget,
                    "A stale collect must not restore removed rows, lower rewards or drain them off-tab")

        let readsBeforeHandoff = completedReads
        model.destination = .coins
        // An ordinary refresh may drain the retained demand. Do not issue a
        // new collect here: it would conceal a lost completion handoff.
        await model.reload()
        try await until("The retained cleanup collection demand was lost") {
            completedReads >= readsBeforeHandoff + 2 && model.snapshot.wallet.pendingCoins == 0
                && model.snapshot.wallet.collectedCoins == receipt.coins
        }
        try require(!stalePublished && !balanceChanged && model.snapshot.wallet.creditedBytes == receipt.creditedBytes
                    && model.snapshot.wallet.fractionalBytes == committed.wallet.fractionalBytes
                    && model.confirmedCollectedCoins == receipt.coins && model.displayedCoinBalance == resolvedTarget
                    && model.cleanupPreview == nil && model.collection == nil && presentations.isEmpty,
                    "The real collection must quietly replace the preview without a counter bounce or a second celebration")
        model.finishCleanupPreview(activePreview.id)
        try require(!model.claimCleanupPreview(activePreview.id) && model.displayedCoinBalance == resolvedTarget,
                    "A retired preview must not replay or change the confirmed counter")
        let readsBeforeReopen = completedReads
        model.windowClosed()
        model.windowOpened()
        try await until("Reopening did not complete its empty collect") { completedReads > readsBeforeReopen }
        let repeated = try EngineClient.decode([String: UInt64].self, await client.request(["action": "collect"]))
        try require(repeated["amount"] == 0 && repeated["to"] == receipt.coins && model.collection == nil
                    && presentations.isEmpty && !stalePublished && !balanceChanged
                    && model.displayedCoinBalance == resolvedTarget,
                    "Reopening and repeated collection must not credit or animate the same recovery twice")
        print("Native background cleanup: prepare_rejected=true final_read_failures=\(failedReads) reopen_reconciled=true pre_execute_cancelled=true confirmation_preference=true retained_preview=true quiet_handoff=true stable_counter=true stale_snapshot_rejected=true off_tab_rewards_preserved=true credited_bytes=\(receipt.creditedBytes) earned_coins=\(receipt.coins) earned_presentations=\(presentations.count) positive_reward_handoff_exercised=\(receipt.coins > 0)")
    }

    private static func cleanupQueueEstimates(_ candidates: [Candidate]) throws {
        var items = Array(candidates.prefix(3))
        for (index, bytes) in [UInt64(60_000_000), 40_000_000, 40_000_000].enumerated() {
            items[index].allocatedBytes = bytes
        }
        let requests = items.map { CleanupRequest(items: [$0], permanently: true) }
        let initial = CleanupEstimate(requests: requests, wallet: Wallet())
        let afterFirst = CleanupEstimate(requests: Array(requests.dropFirst()),
            wallet: Wallet(fractionalBytes: 60_000_000, creditedBytes: 60_000_000))
        let afterSecond = CleanupEstimate(requests: [requests[2]],
            wallet: Wallet(pendingCoins: 1, creditedBytes: 100_000_000))
        let finished = CleanupEstimate(requests: [],
            wallet: Wallet(pendingCoins: 1, fractionalBytes: 40_000_000, creditedBytes: 140_000_000))
        try require(initial.allocatedBytes == 140_000_000 && initial.itemCount == 3 && initial.pendingCoins == 1
                    && afterFirst.pendingCoins == 1 && afterSecond.pendingCoins == 0 && finished.pendingCoins == 0
                    && [initial, afterFirst, afterSecond, finished].allSatisfy { $0.targetCoins == 1 },
                    "Moving 60 + 40 + 40 MB through the ledger must apply one remainder and retain one target coin")
        let trash = CleanupEstimate(requests: [CleanupRequest(items: items, permanently: false)],
                                    wallet: Wallet(fractionalBytes: 60_000_000))
        try require(trash.allocatedBytes == 140_000_000 && trash.pendingCoins == 0 && !trash.hasPermanentCleanup,
                    "Queued Trash work must never turn an existing remainder into a coin")
    }

    @MainActor private static func cleanupQueueInteraction() async throws {
        let names = ["A", "B", "C", "D"]
        let base = try await Task.detached(priority: .utility) {
            let fm = FileManager.default
            guard let physical = realpath(fm.temporaryDirectory.path, nil) else {
                throw EngineError.message("Cannot resolve the disposable queue directory")
            }
            let temporary = URL(fileURLWithPath: String(cString: physical))
            free(physical)
            let capacity = try temporary.resourceValues(forKeys: [.volumeAvailableCapacityKey]).volumeAvailableCapacity ?? 0
            try require(capacity > 3 * 1024 * 1024 * 1024 + 500_000_000,
                        "The queue fixture requires its payloads plus a 3 GiB reserve")
            let base = temporary.appendingPathComponent("chippytea-queue-\(UUID().uuidString)")
            try fm.createDirectory(at: base, withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700])
            try Data("Disposable chippytea cleanup queue fixture; preserve all remaining files.\n".utf8)
                .write(to: base.appendingPathComponent(".chippytea-fixture"))
            let old = Date().addingTimeInterval(-9 * 86_400)
            for (index, name) in names.enumerated() {
                let project = base.appendingPathComponent("Projects/\(name)")
                let target = project.appendingPathComponent("target")
                try fm.createDirectory(at: target.appendingPathComponent("debug"), withIntermediateDirectories: true)
                try Data("[package]\nname=\"queue-\(name.lowercased())\"\nversion=\"0.0.0\"\n".utf8)
                    .write(to: project.appendingPathComponent("Cargo.toml"))
                try Data("Signature: 8a477f597d28d172789f06886806bc55\n".utf8)
                    .write(to: target.appendingPathComponent("CACHEDIR.TAG"))
                try Data("Preserve queue sibling \(name)".utf8).write(to: project.appendingPathComponent("preserve.txt"))
                let payload = target.appendingPathComponent("debug/payload")
                try Data().write(to: payload, options: .withoutOverwriting)
                let file = try FileHandle(forWritingTo: payload)
                let chunk = Data(repeating: UInt8(0x61 + index), count: 1_000_000)
                for _ in 0..<100 { try file.write(contentsOf: chunk) }
                try file.synchronize()
                try file.close()
                for path in [payload, target.appendingPathComponent("CACHEDIR.TAG"), target.appendingPathComponent("debug"),
                             target, project.appendingPathComponent("Cargo.toml"), project] {
                    try fm.setAttributes([.modificationDate: old], ofItemAtPath: path.path)
                }
            }
            return base
        }.value
        print("Disposable cleanup queue evidence: \(base.path)")
        enum ReadGate: Equatable { case none, first, stale }
        let firstPath = base.appendingPathComponent("Projects/A/target").path
        var rejectFirstFinal = false
        var failedReads = 0
        var gate = ReadGate.none
        var gateClaimed = false
        var startedReads = 0
        var gateStartsAfter = 0
        var capturedGate = ReadGate.none
        var heldSnapshot: EngineSnapshot?
        var heldRead: CheckedContinuation<EngineSnapshot, Error>?
        var completedReads = 0
        let model = AppModel(directory: base.appendingPathComponent("State"), scanHome: base.appendingPathComponent("UnusedHome")) { client in
            startedReads += 1
            let readNumber = startedReads
            let current = try await client.snapshot()
            if !current.cleaning && current.history.contains(where: { $0.path == firstPath && $0.outcome == "removed" }) {
                if rejectFirstFinal {
                    failedReads += 1
                    throw EngineError.message("Disposable queue final read failed")
                }
                if gate != .none && readNumber > gateStartsAfter {
                    guard !gateClaimed else { throw EngineError.message("Disposable queue completion is held") }
                    gateClaimed = true
                    capturedGate = gate
                    heldSnapshot = current
                    let delivered: EngineSnapshot = try await withCheckedThrowingContinuation { heldRead = $0 }
                    completedReads += 1
                    return delivered
                }
            }
            completedReads += 1
            return current
        }
        let soundPreference = UserDefaults.standard.object(forKey: "soundEnabled")
        let confirmationPreference = UserDefaults.standard.object(forKey: "confirmBeforeDeleting")
        model.soundEnabled = false
        model.confirmBeforeDeleting = true
        defer {
            rejectFirstFinal = false
            gate = .none
            model.cancel()
            heldRead?.resume(throwing: EngineError.message("Disposable queue fixture finished"))
            model.windowClosed()
            if let soundPreference { UserDefaults.standard.set(soundPreference, forKey: "soundEnabled") }
            else { UserDefaults.standard.removeObject(forKey: "soundEnabled") }
            if let confirmationPreference { UserDefaults.standard.set(confirmationPreference, forKey: "confirmBeforeDeleting") }
            else { UserDefaults.standard.removeObject(forKey: "confirmBeforeDeleting") }
        }
        func until(_ message: String, _ condition: () -> Bool) async throws {
            let deadline = ProcessInfo.processInfo.systemUptime + 15
            while !condition() {
                try require(ProcessInfo.processInfo.systemUptime < deadline, message)
                try await Task.sleep(for: .milliseconds(20))
            }
        }
        func identity(_ path: URL) throws -> EngineIdentity {
            var value = stat()
            try require(lstat(path.path, &value) == 0 && value.st_uid == geteuid() && value.st_size >= 0,
                        "A disposable queue file disappeared or changed ownership")
            return EngineIdentity(device: UInt64(value.st_dev), inode: UInt64(value.st_ino), mode: UInt32(value.st_mode),
                size: UInt64(value.st_size),
                modifiedNs: Int64(value.st_mtimespec.tv_sec) * 1_000_000_000 + Int64(value.st_mtimespec.tv_nsec),
                changedNs: Int64(value.st_ctimespec.tv_sec) * 1_000_000_000 + Int64(value.st_ctimespec.tv_nsec))
        }
        func paths(_ name: String) -> [URL] {
            ["Cargo.toml", "preserve.txt", "target", "target/CACHEDIR.TAG", "target/debug", "target/debug/payload"]
                .map { base.appendingPathComponent("Projects/\(name)/\($0)") }
        }
        func releaseCheckpoint() throws {
            guard let continuation = heldRead, let current = heldSnapshot else {
                throw EngineError.message("Missing queue completion checkpoint")
            }
            heldRead = nil
            heldSnapshot = nil
            gate = .none
            gateClaimed = false
            capturedGate = .none
            continuation.resume(returning: current)
        }
        await model.start()
        guard let client = model.client else { throw EngineError.message("Queue engine did not start") }
        model.authorizeAndScan(path: base.appendingPathComponent("Projects").path)
        try await until("Four disposable queue candidates did not become reviewable") {
            !model.busy && !model.snapshot.scanning && model.snapshot.candidates.count == 4
        }
        let candidates = try names.map { name -> Candidate in
            guard let candidate = model.snapshot.candidates.first(where: { $0.path == base.appendingPathComponent("Projects/\(name)/target").path }),
                  candidate.eligiblePermanent && candidate.blockedReason == nil else {
                throw EngineError.message("Missing eligible queue fixture \(name)")
            }
            return candidate
        }
        guard let root = model.snapshot.roots.first else { throw EngineError.message("Queue fixture has no authorized root") }
        try cleanupQueueEstimates(candidates)
        let originalIdentities = try names.map { try paths($0).map(identity) }
        try require(model.snapshot.wallet == Wallet() && model.snapshot.history.isEmpty,
                    "Queue tests must begin with an untouched real ledger")

        // No suspension occurs between admission and Stop. Even the first job
        // can only prepare a token; none of these requests may reach execute.
        model.reviewOne(candidates[0])
        model.clean(permanently: true)
        guard let firstPreview = model.cleanupPreview else { throw EngineError.message("Missing queue admission preview") }
        try require(model.hasCleanupWork && model.isCleaning && model.queuedCleanupCount == 0 && model.canEnqueueCleanup,
                    "The first job must become active synchronously while further review stays available")
        func synthetic(_ index: Int) -> Candidate {
            var item = candidates[0]
            item.id = "disposable-queued-\(index)"
            item.path = base.appendingPathComponent("Projects/Unused-\(index)/target").path
            item.identity.inode = UInt64.max - UInt64(index)
            return item
        }
        var duplicateID = synthetic(1)
        duplicateID.id = candidates[0].id
        var samePath = synthetic(2)
        samePath.path = candidates[0].path
        var childPath = synthetic(3)
        childPath.path = candidates[0].path + "/debug"
        var parentPath = synthetic(4)
        parentPath.path = URL(fileURLWithPath: candidates[0].path).deletingLastPathComponent().path
        var sameIdentity = synthetic(5)
        sameIdentity.identity = candidates[0].identity
        for duplicate in [duplicateID, samePath, childPath, parentPath, sameIdentity] {
            model.reviewItems = [duplicate]
            model.clean(permanently: true)
            try require(model.queuedCleanupCount == 0 && model.cleanupPreview?.id == firstPreview.id,
                        "A reserved ID, overlapping path or physical identity must not be queued twice")
        }
        model.reviewItems = (1000..<1101).map(synthetic)
        model.clean(permanently: true)
        try require(model.queuedCleanupCount == 0 && model.cleanupPreview?.id == firstPreview.id,
                    "A queue job must not admit more than 100 items")
        for candidate in candidates[1...2] {
            model.reviewOne(candidate)
            model.clean(permanently: true)
        }
        let waitingPreview = model.cleanupPreview?.id
        model.reviewItems = [candidates[1]]
        model.clean(permanently: true)
        try require(model.queuedCleanupCount == 2 && model.cleanupPreview?.id == waitingPreview,
                    "Waiting jobs must hold the same duplicate reservation as the active job")
        for index in 10..<108 {
            model.reviewItems = [synthetic(index)]
            model.clean(permanently: true)
        }
        let cappedPreview = model.cleanupPreview?.id
        model.reviewItems = [synthetic(108)]
        model.clean(permanently: true)
        try require(model.queuedCleanupCount == 100 && !model.canEnqueueCleanup && model.cleanupPreview?.id == cappedPreview,
                    "The waiting queue must stop at 100 jobs without replacing the last accepted preview")
        model.cancel()
        try require(model.queuedCleanupCount == 0 && model.cleanupCandidateIDs == [candidates[0].id]
                    && Set(model.displayedCandidates.map(\.id)) == Set(candidates.dropFirst().map(\.id)),
                    "Stop must immediately release every waiting row while retaining the active reservation")
        if let id = model.cleanupPreview?.id { model.finishCleanupPreview(id) }
        try await until("Stopped cleanup queue did not reconcile") { !model.hasCleanupWork && !model.busy }
        let stopped = try await client.snapshot()
        try require(stopped.wallet == Wallet() && stopped.history.isEmpty && stopped.candidates == model.snapshot.candidates
                    && model.queuedCleanupCount == 0 && model.cleanupPreview == nil && model.cleanupCandidateIDs.isEmpty,
                    "Stopping before execute must leave no receipts, rewards or queued jobs")
        try require(try names.map { try paths($0).map(identity) } == originalIdentities,
                    "Stop changed a disposable payload or its surrounding files")
        model.errorMessage = nil
        model.reviewItems = []

        var presentations: [CollectionBurst] = []
        let collectionObserver = model.$collection.sink { if let burst = $0 { presentations.append(burst) } }
        defer { collectionObserver.cancel() }
        model.visible = true
        model.panelVisible = true
        rejectFirstFinal = true
        var previewIDs: [UUID] = []
        for candidate in candidates.prefix(3) {
            model.reviewOne(candidate)
            try require(model.showReview && model.reviewItems == [candidate], "Waiting cleanup must leave the next review usable")
            model.clean(permanently: true)
            guard let preview = model.cleanupPreview else { throw EngineError.message("Missing combined queue preview") }
            previewIDs.append(preview.id)
        }
        guard let combinedPreview = model.cleanupPreview else { throw EngineError.message("Missing accepted queue preview") }
        let requests = candidates.prefix(3).map { CleanupRequest(items: [$0], permanently: true) }
        let initialEstimate = CleanupEstimate(requests: requests, wallet: Wallet())
        try require(Set(previewIDs).count == 3 && model.queuedCleanupCount == 2
                    && model.pendingCleanupCoinEstimate == initialEstimate.pendingCoins
                    && model.displayedCoinBalance == initialEstimate.targetCoins && model.confirmedEarnedCoins == 0
                    && model.displayedCandidates == [candidates[3]],
                    "Each confirmation must combine outstanding estimates, reserve its rows and create one new preview")
        try require(model.claimCleanupPreview(combinedPreview.id), "The combined queue must present its immediate collection")
        model.finishCleanupPreview(combinedPreview.id)
        model.collect()
        model.collect()
        model.reviewOne(candidates[3])
        model.destination = .activity
        var previewReplaced = false
        let previewObserver = model.$cleanupPreview.sink {
            if let preview = $0, preview.id != combinedPreview.id { previewReplaced = true }
            if $0 == nil && model.hasCleanupWork { previewReplaced = true }
        }
        defer { previewObserver.cancel() }
        try await until("The first cleanup did not reach the failed final reads") { failedReads >= 2 && !model.isCleaning }
        try await Task.sleep(for: .milliseconds(120))
        let first = try await client.snapshot()
        try require(model.hasCleanupWork && model.busy && model.queuedCleanupCount == 2
                    && first.history.count == 1 && first.history[0].path == candidates[0].path
                    && first.history[0].outcome == "removed" && first.wallet.collectedCoins == 0
                    && model.snapshot.wallet == Wallet() && !previewReplaced,
                    "Failed final reads must retain the active job and hold the FIFO without collecting")
        try require(try names.dropFirst().map { try paths($0).map(identity) } == Array(originalIdentities.dropFirst()),
                    "A waiting or unsubmitted artifact changed before the first cleanup reconciled")

        // Give the store a newer, still eligible B while its frozen request is
        // waiting. Queue execution must reject the old review, never adopt it.
        let stalePayload = base.appendingPathComponent("Projects/B/target/debug/payload")
        try await Task.detached(priority: .utility) {
            let file = try FileHandle(forWritingTo: stalePayload)
            try file.write(contentsOf: Data([0x7f]))
            try file.synchronize()
            try file.close()
            try FileManager.default.setAttributes([.modificationDate: Date().addingTimeInterval(-9 * 86_400)],
                                                  ofItemAtPath: stalePayload.path)
        }.value
        let changedIdentity = try identity(stalePayload)
        _ = try await client.request(["action": "scan", "root_id": root.id])
        let refreshDeadline = ProcessInfo.processInfo.systemUptime + 15
        var refreshed = try await client.snapshot()
        while refreshed.scanning {
            try require(ProcessInfo.processInfo.systemUptime < refreshDeadline, "Disposable stale-review scan did not finish")
            try await Task.sleep(for: .milliseconds(20))
            refreshed = try await client.snapshot()
        }
        guard let updatedB = refreshed.candidates.first(where: { $0.id == candidates[1].id }) else {
            throw EngineError.message("The changed disposable queued item did not remain reviewable")
        }
        try require(updatedB.fingerprint != candidates[1].fingerprint && updatedB.eligiblePermanent && updatedB.blockedReason == nil,
                    "The queue race requires a real refreshed candidate distinct from the frozen review")
        rejectFirstFinal = false
        gate = .first
        gateStartsAfter = startedReads
        let recovery = Task { await model.reload() }
        defer { recovery.cancel() }
        try await until("First cleanup recovery did not reach its checkpoint") { capturedGate == .first && heldRead != nil }
        try require(model.queuedCleanupCount == 2 && model.showReview && model.reviewItems == [candidates[3]]
                    && model.selection == [candidates[3].id] && model.destination == .activity,
                    "An unresolved active job must preserve a newer open review and unsubmitted selection")
        var staleReviewRejected = false
        let errorObserver = model.$errorMessage.sink { message in
            if message?.contains("changed while its review was open") == true {
                staleReviewRejected = true
                gate = .stale
                gateClaimed = false
                gateStartsAfter = startedReads
            }
        }
        defer { errorObserver.cancel() }
        try releaseCheckpoint()
        try await until("The frozen queued review was not rejected at the head") { staleReviewRejected && capturedGate == .stale && heldRead != nil }
        guard let afterStale = heldSnapshot else { throw EngineError.message("Missing stale-review checkpoint") }
        let remainingEstimate = CleanupEstimate(requests: Array(requests.dropFirst()), wallet: afterStale.wallet)
        try require(model.queuedCleanupCount == 1 && model.hasCleanupWork && afterStale.history == first.history
                    && model.snapshot.wallet == afterStale.wallet && afterStale.wallet.collectedCoins == 0
                    && model.pendingCleanupCoinEstimate == remainingEstimate.pendingCoins
                    && model.displayedCoinBalance == remainingEstimate.targetCoins
                    && model.confirmedEarnedCoins == afterStale.wallet.pendingCoins
                    && model.showReview && model.reviewItems == [candidates[3]] && model.selection == [candidates[3].id]
                    && model.cleanupPreview?.id == combinedPreview.id && !previewReplaced,
                    "FIFO completion must preserve the next review and combine confirmed recovery with all remaining work")
        try require(try identity(stalePayload) == changedIdentity
                    && paths("C").map(identity) == originalIdentities[2],
                    "A stale head must remain intact and hold the following job until its own final snapshot")
        try releaseCheckpoint()
        try await until("The remaining cleanup queue did not drain") { !model.hasCleanupWork && !model.busy }
        let committed = try await client.snapshot()
        let credited = committed.history.reduce(UInt64(0)) { $0 + $1.creditedBytes }
        let coins = credited / 100_000_000
        try require(committed.history.count == 2 && Set(committed.history.map(\.path)) == Set([candidates[0].path, candidates[2].path])
                    && committed.history.allSatisfy {
                        $0.operation == "permanent" && $0.outcome == "removed"
                            && $0.creditedBytes <= $0.observedBytes && $0.creditedBytes <= $0.reportedBytes
                    }
                    && committed.wallet.collectedCoins == 0 && committed.wallet.pendingCoins == coins
                    && committed.wallet.creditedBytes == credited && committed.wallet.fractionalBytes == credited % 100_000_000
                    && committed.history.reduce(UInt64(0), { $0 + $1.coins }) == coins
                    && model.queuedCleanupCount == 0 && model.cleanupCandidateIDs.isEmpty
                    && model.pendingCleanupCoinEstimate == 0 && model.confirmedEarnedCoins == coins
                    && model.displayedCoinBalance == coins && !previewReplaced && presentations.isEmpty,
                    "Only the two real removals may establish queue rewards, with one carried remainder and no duplicate credit")
        try require(model.showReview && model.reviewItems == [candidates[3]] && model.selection == [candidates[3].id]
                    && model.destination == .activity,
                    "Draining the queue must leave an unsubmitted review and its selection untouched")
        try require(try identity(stalePayload) == changedIdentity
                    && paths("D").map(identity) == originalIdentities[3],
                    "The stale queued file and unsubmitted project must retain their identities")
        for name in names {
            try require(try String(contentsOf: base.appendingPathComponent("Projects/\(name)/preserve.txt"), encoding: .utf8) == "Preserve queue sibling \(name)",
                        "Queue cleanup changed an unrelated sibling")
        }
        try require(!FileManager.default.fileExists(atPath: candidates[0].path)
                    && !FileManager.default.fileExists(atPath: candidates[2].path), "Committed queue receipts must correspond to physical removal")
        model.showReview = false
        model.destination = .coins
        let beforeHandoff = completedReads
        await model.reload()
        try await until("The drained queue lost its held collection demand") {
            completedReads >= beforeHandoff + 2 && model.snapshot.wallet.pendingCoins == 0 && model.snapshot.wallet.collectedCoins == coins
        }
        try require(model.cleanupPreview == nil && model.collection == nil && presentations.isEmpty
                    && model.displayedCoinBalance == coins && model.snapshot.wallet.creditedBytes == credited,
                    "Queue rewards must collect once without replacing the immediate presentation or lowering the counter")
        let beforeReopen = completedReads
        model.windowClosed()
        model.windowOpened()
        try await until("The queue reopen collection did not finish") { completedReads > beforeReopen }
        let repeated = try EngineClient.decode([String: UInt64].self, await client.request(["action": "collect"]))
        try require(repeated["amount"] == 0 && repeated["to"] == coins && presentations.isEmpty
                    && model.displayedCoinBalance == coins && model.snapshot.wallet.creditedBytes == credited,
                    "Reopening must not credit or animate queued cleanup a second time")
        print("Native cleanup queue: fifo=true waiting_limit=100 item_limit=100 overlap_rejected=true stop_before_execute=true failed_final_reads=\(failedReads) stale_review_rejected=true open_review_preserved=true aggregate_remainder=true credited_bytes=\(credited) earned_coins=\(coins) earned_presentations=\(presentations.count) positive_reward_handoff_exercised=\(coins > 0)")
    }

    static func run() {
        do { try test(); print("PASS native integration: Rust discovery → native Trash → durable receipt → conflict-safe restore → permanent cleanup → restart"); exit(0) }
        catch { fputs("FAIL native integration: \(error)\n", stderr); exit(1) }
    }
    private static func require(_ value: @autoclosure () throws -> Bool, _ message: String) throws {
        if try !value() { throw EngineError.message(message) }
    }
    private static func snapshotResponseDecoding() throws {
        let aboveDoublePrecision: UInt64 = 9_007_199_254_740_993
        let identity = EngineIdentity(device: .max, inode: aboveDoublePrecision, mode: .max,
                                      size: .max, modifiedNs: .min, changedNs: .max)
        var expected = EngineSnapshot()
        expected.roots = [ScanRoot(id: "disposable-root", path: "/disposable/precision-only", kind: "folder", identity: identity)]
        expected.wallet = Wallet(collectedCoins: .max, pendingCoins: aboveDoublePrecision,
                                 fractionalBytes: 99_999_999, creditedBytes: .max)
        expected.stats.entries = .max
        expected.stats.firstFindingMs = aboveDoublePrecision
        expected.stats.message = "Disposable Unicode: café · 🪙"
        let encoder = JSONEncoder()
        encoder.keyEncodingStrategy = .convertToSnakeCase
        // Assemble the wire bytes directly so no NSNumber conversion happens
        // before the decoder whose integer precision is being checked.
        let payload = try encoder.encode(expected)
        var response = Data("{\"ok\":true,\"data\":".utf8)
        response.append(payload)
        response.append(Data("}".utf8))
        try require(try EngineClient.decodeSnapshotResponse(response) == expected,
                    "Snapshot decoding must preserve UInt64, signed timestamps, Unicode and optional fields exactly")
        var nullPayload = Data(payload.dropLast())
        nullPayload.append(Data(",\"foreground_scan\":null}".utf8))
        for data in [payload, nullPayload] {
            var noSummaryResponse = Data("{\"ok\":true,\"data\":".utf8)
            noSummaryResponse.append(data)
            noSummaryResponse.append(Data("}".utf8))
            let decoded = try EngineClient.decodeSnapshotResponse(noSummaryResponse)
            var presentation = DiscoveryPresentation()
            presentation.update(decoded)
            try require(decoded == expected && decoded.foregroundScan == nil && presentation.stats.entries == 0
                        && presentation.statusLine(rootCount: 1) == "No saved scan · 1 folder",
                        "Missing and null foreground fields must decode without presenting raw counters as a scan")
        }
        expected.foregroundScan = ForegroundScan(active: false, stats: expected.stats)
        var foregroundResponse = Data("{\"ok\":true,\"data\":".utf8)
        foregroundResponse.append(try encoder.encode(expected))
        foregroundResponse.append(Data("}".utf8))
        try require(try EngineClient.decodeSnapshotResponse(foregroundResponse) == expected,
                    "Foreground completion and its exact statistics must cross the native boundary")
        let invalid = [
            "", "{", "[]", "null", "{}", "{\"ok\":true}",
            "{\"ok\":true,\"data\":null}", "{\"ok\":true,\"data\":{}}",
            "{\"ok\":true,\"data\":[]}", "{\"ok\":true,\"data\":{\"ok\":true}}",
            "{\"ok\":\"true\",\"data\":{}}", "{\"ok\":1,\"data\":{}}",
        ]
        for text in invalid {
            var rejected = false
            do { _ = try EngineClient.decodeSnapshotResponse(Data(text.utf8)) } catch { rejected = true }
            try require(rejected, "Malformed or foreign snapshot responses must be rejected: \(text)")
        }
        for prefix in ["{\"ok\":1,\"data\":", "{\"ok\":\"true\",\"data\":", "{\"data\":"] {
            var invalidEnvelope = Data(prefix.utf8)
            invalidEnvelope.append(payload)
            invalidEnvelope.append(Data("}".utf8))
            var rejected = false
            do { _ = try EngineClient.decodeSnapshotResponse(invalidEnvelope) } catch { rejected = true }
            try require(rejected, "A complete snapshot must still require an explicit Boolean success envelope")
        }
        var rejectedBarePayload = false
        do { _ = try EngineClient.decodeSnapshotResponse(payload) } catch { rejectedBarePayload = true }
        try require(rejectedBarePayload, "An unwrapped snapshot must not bypass its success envelope")
        for (text, message) in [
            ("{\"ok\":false,\"error\":\"Explicit engine failure\",\"data\":[]}", "Explicit engine failure"),
            ("{\"ok\":false}", "The operation could not be completed."),
        ] {
            var actual: String?
            do { _ = try EngineClient.decodeSnapshotResponse(Data(text.utf8)) }
            catch let error as EngineError { actual = error.errorDescription }
            catch { actual = String(describing: error) }
            try require(actual == message, "Error envelopes must preserve the engine message before decoding data")
        }
    }
    private static func snapshotResponseReuse() throws {
        var first = EngineSnapshot()
        first.stats.entries = 9_007_199_254_740_993
        first.stats.message = "Disposable response: café · 🪙"
        first.wallet.pendingCoins = 11
        first.roots = [ScanRoot(id: "fixture", path: "/disposable/snapshot-only", kind: "folder",
                                identity: EngineIdentity(device: 1, inode: 2, mode: 0o40700,
                                                         size: 0, modifiedNs: .min, changedNs: .max))]
        var second = first
        second.wallet.pendingCoins = 12
        func response(_ snapshot: EngineSnapshot) throws -> Data {
            let encoder = JSONEncoder()
            encoder.keyEncodingStrategy = .convertToSnakeCase
            var data = Data("{\"ok\":true,\"data\":".utf8)
            data.append(try encoder.encode(snapshot))
            data.append(Data("}".utf8))
            return data
        }
        let original = try response(first)
        let changed = try response(second)
        try require(original.count == changed.count, "The freshness control must change bytes without changing length")
        var decoder = SnapshotResponseDecoder()
        for data in [original, original, changed, changed, original] {
            let expected = data == changed ? second : first
            try require(try decoder.decode(data) == expected, "Only identical current response bytes may reuse a snapshot")
        }
        var returned = try decoder.decode(original)
        returned.roots.removeAll()
        returned.stats.message = "Changed only in this local value"
        returned.wallet.pendingCoins = 0
        try require(try decoder.decode(original) == first, "A caller's changes must not mutate the retained snapshot")
        for text in ["", "{", "{\"ok\":true,\"data\":null}", "{\"ok\":false,\"error\":\"Fixture read failed\"}"] {
            var rejected = false
            do { _ = try decoder.decode(Data(text.utf8)) }
            catch {
                rejected = true
                if text.contains("Fixture read failed") {
                    try require(error.localizedDescription == "Fixture read failed", "The current engine error must survive a prior cache hit")
                }
            }
            try require(rejected, "A failed current response must never return an earlier successful snapshot")
            try require(try decoder.decode(original) == first, "A valid response must recover after failure")
        }
        var atLimit = changed
        atLimit.append(Data(repeating: 0x20, count: SnapshotResponseDecoder.maximumResponseBytes - atLimit.count))
        try require(try decoder.decode(atLimit) == second, "An exactly capped valid response must decode completely")
        var oversized = original
        oversized.append(Data(repeating: 0x20, count: SnapshotResponseDecoder.maximumResponseBytes + 1 - oversized.count))
        for _ in 0..<2 {
            try require(try decoder.decode(oversized) == first, "Oversized valid responses must still be decoded without truncation")
        }
        try require(try decoder.decode(changed) == second, "A smaller changed response must replace an oversized response")
        decoder.clear()
        try require(try decoder.decode(original) == first, "Clearing the decoder must allow fresh reads")
    }
    private static func test() throws {
        try snapshotResponseDecoding()
        try snapshotResponseReuse()
        let fm = FileManager.default
        guard let physicalTemp = realpath(fm.temporaryDirectory.path, nil) else { throw EngineError.message("Cannot resolve the disposable test directory") }
        let temporaryRoot = URL(fileURLWithPath: String(cString: physicalTemp)); free(physicalTemp)
        let directory = temporaryRoot.appendingPathComponent("chippytea-native-test-\(UUID().uuidString)")
        let projects = directory.appendingPathComponent("Projects")
        let project = projects.appendingPathComponent("Disposable")
        let target = project.appendingPathComponent("target")
        try fm.createDirectory(at: target.appendingPathComponent("debug"), withIntermediateDirectories: true)
        try Data("chippytea disposable native integration fixture. No user files.\n".utf8).write(to: directory.appendingPathComponent(".chippytea-fixture"))
        try Data("[package]\nname=\"disposable\"\nversion=\"0.1.0\"\n".utf8).write(to: project.appendingPathComponent("Cargo.toml"))
        try Data("Signature: 8a477f597d28d172789f06886806bc55\n".utf8).write(to: target.appendingPathComponent("CACHEDIR.TAG"))
        let capacity = try temporaryRoot.resourceValues(forKeys: [.volumeAvailableCapacityKey]).volumeAvailableCapacity ?? 0
        try require(capacity >= 3 * 1024 * 1024 * 1024 + 300_000_000, "The disposable native test requires a 3 GiB reserve")
        let payload = target.appendingPathComponent("debug/disposable.bin")
        fm.createFile(atPath: payload.path, contents: nil)
        let file = try FileHandle(forWritingTo: payload)
        let chunk = Data(repeating: 0x31, count: 1_048_576)
        for _ in 0..<256 { try file.write(contentsOf: chunk) }
        try file.synchronize(); try file.close()
        let alias = target.appendingPathComponent("debug/disposable-alias.bin")
        try fm.linkItem(at: payload, to: alias)
        func verifyInternalLinks() throws {
            let first = try fm.attributesOfItem(atPath: payload.path)
            let second = try fm.attributesOfItem(atPath: alias.path)
            try require(first[.systemFileNumber] as? NSNumber == second[.systemFileNumber] as? NSNumber
                        && (first[.referenceCount] as? NSNumber)?.intValue == 2
                        && (second[.referenceCount] as? NSNumber)?.intValue == 2,
                        "The disposable payload must retain two internal names for one inode")
        }
        try verifyInternalLinks()
        // Simulate a genuinely old opportunity without changing any user files.
        let old = Date().addingTimeInterval(-9 * 86_400)
        // Exercise metadata-heavy deletion too: a single large file hid the
        // reward failure seen with thousands of dependency/build files.
        let metadata = target.appendingPathComponent("debug/metadata")
        try fm.createDirectory(at: metadata, withIntermediateDirectories: true)
        for index in 0..<2048 {
            let item = metadata.appendingPathComponent("file-\(index)")
            try Data(repeating: 0x32, count: 4096).write(to: item)
            try fm.setAttributes([.modificationDate: old], ofItemAtPath: item.path)
        }
        try fm.setAttributes([.modificationDate: old], ofItemAtPath: metadata.path)
        for item in [payload, target.appendingPathComponent("CACHEDIR.TAG"), project.appendingPathComponent("Cargo.toml"), target.appendingPathComponent("debug"), target, project] {
            try fm.setAttributes([.modificationDate: old], ofItemAtPath: item.path)
        }
        let database = directory.appendingPathComponent("ledger.sqlite")
        var client: EngineClient? = try EngineClient(database: database)
        let rootData = try client!.requestSync(["action": "authorize", "path": projects.path, "kind": "projects"])
        var root = try EngineClient.decode(ScanRoot.self, rootData)
        func snapshot() throws -> EngineSnapshot { try EngineClient.decode(EngineSnapshot.self, client!.requestSync(["action": "snapshot"])) }
        func scan() throws -> Candidate {
            _ = try client!.requestSync(["action": "scan", "root_id": root.id])
            let deadline = Date().addingTimeInterval(20)
            while Date() < deadline {
                let state = try snapshot()
                if !state.scanning {
                    guard let item = state.candidates.first(where: { $0.kind == "cargo" && $0.blockedReason == nil }) else {
                        throw EngineError.message("Expected eligible Cargo fixture: \(state.candidates.map { $0.blockedReason ?? $0.title }) / \(state.error ?? state.stats.message)")
                    }
                    return item
                }
                Thread.sleep(forTimeInterval: 0.03)
            }
            throw EngineError.message("Fixture scan timed out")
        }
        func clean(_ candidate: Candidate, operation: String) throws -> Receipt {
            let encoder = JSONEncoder(); encoder.keyEncodingStrategy = .convertToSnakeCase
            let items = try JSONSerialization.jsonObject(with: encoder.encode([candidate]))
            let data = try client!.requestSync(["action": "prepare", "operation": operation, "items": items])
            let review = try JSONSerialization.jsonObject(with: data) as! [String: String]
            let result = try client!.requestSync(["action": "execute", "token": review["token"]!, "confirmed": true])
            let deadline = Date().addingTimeInterval(20)
            while try snapshot().scanning {
                try require(Date() < deadline, "Discovery did not finish after cleanup")
                Thread.sleep(forTimeInterval: 0.01)
            }
            return try EngineClient.decode([Receipt].self, result)[0]
        }
        let first = try scan()
        try require(first.allocatedBytes < 300_000_000, "Both payload names must count as one allocation")
        let receipt = try clean(first, operation: "trash")
        try require(receipt.outcome == "trashed", "Native Trash failed: \(receipt.detail)")
        try require(receipt.coins == 0 && receipt.creditedBytes == 0, "Trash must never earn coins")
        try require(!fm.fileExists(atPath: target.path), "Original should have moved to native Trash")
        try require(fm.fileExists(atPath: receipt.trashPath ?? ""), "Returned native Trash destination must exist")
        // The access flow can broaden a selected folder without losing Trash history.
        let broader = try client!.requestSync(["action": "authorize", "path": directory.path, "kind": "folder", "replace_contained": true])
        root = try EngineClient.decode(ScanRoot.self, broader)
        try require(try snapshot().roots.count == 1, "A broader grant must replace contained scan roots atomically")
        client = nil
        client = try EngineClient(database: database)
        try require(try snapshot().wallet.pendingCoins == 0, "Restart must not credit a Trash operation")
        try fm.createDirectory(at: target, withIntermediateDirectories: false)
        var refusedConflict = false
        do { _ = try client!.requestSync(["action": "restore", "id": receipt.id]) } catch { refusedConflict = true }
        try require(refusedConflict, "Restore must refuse an existing destination")
        try fm.removeItem(at: target) // only the empty conflict created by this test
        _ = try client!.requestSync(["action": "forget", "id": root.id])
        let narrower = try client!.requestSync(["action": "authorize", "path": project.path, "kind": "projects"])
        root = try EngineClient.decode(ScanRoot.self, narrower)
        _ = try client!.requestSync(["action": "restore", "id": receipt.id])
        try require(fm.fileExists(atPath: target.appendingPathComponent("debug/disposable.bin").path), "Restore should recover the exact disposable file")
        try verifyInternalLinks()
        let next = try scan()
        let removed = try clean(next, operation: "permanent")
        try require(removed.outcome == "removed", "Permanent cleanup failed: \(removed.detail)")
        try require(!fm.fileExists(atPath: target.path), "Reviewed target should be removed")
        try require(fm.fileExists(atPath: project.appendingPathComponent("Cargo.toml").path), "Project source must survive")
        try require(removed.creditedBytes <= removed.observedBytes && removed.creditedBytes <= removed.reportedBytes, "Credit must respect both bounds")
        try require(removed.creditedBytes >= 100_000_000 && removed.coins >= 1, "The disposable unshared cleanup must actually award a coin: \(removed.detail)")
        _ = try client!.requestSync(["action": "collect"])
        let collected = try snapshot().wallet.collectedCoins
        _ = try client!.requestSync(["action": "collect"])
        try require(try snapshot().wallet.collectedCoins == collected, "Double collect must be idempotent")
        client = nil
        client = try EngineClient(database: database)
        try require(try snapshot().wallet.collectedCoins == collected, "Collection must survive restart")
        client = nil
        print("Disposable native evidence: \(directory.path)")
        print("Native receipt observed=\(removed.observedBytes) credited=\(removed.creditedBytes) coins=\(removed.coins)")
    }
}
