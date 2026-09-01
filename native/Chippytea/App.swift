import AppKit
import Combine
import Darwin
import SQLite3
import SwiftUI

@main struct ChippyteaMain {
    @MainActor static func main() {
        let energyMode = CommandLine.arguments.contains("--energy-benchmark")
        let scanMode = CommandLine.arguments.contains("--scan-benchmark")
        let maintenanceMode = CommandLine.arguments.contains("--maintenance-benchmark")
        let invalidationExercise = CommandLine.arguments.contains("--energy-invalidation-exercise")
        let energyStaging = CommandLine.arguments.contains("--screenshot")
            && ProcessInfo.processInfo.environment["CHIPPYTEA_ENERGY_OUTPUT"] != nil
        if invalidationExercise && !energyMode {
            fputs("The invalidation exercise requires an isolated energy benchmark.\n", stderr)
            exit(2)
        }
        if (energyMode || scanMode || maintenanceMode || energyStaging),
           [energyMode, scanMode, maintenanceMode, energyStaging].filter({ $0 }).count != 1
            || ((scanMode || maintenanceMode) && ProcessInfo.processInfo.environment["CHIPPYTEA_SCREENSHOT_STATE"] != "discover")
            || !energyBenchmarkEnvironmentIsValid(staging: energyStaging) {
            fputs("Energy measurement requires isolated state beside a marked disposable fixture.\n", stderr)
            exit(2)
        }
        if CommandLine.arguments.contains("--self-test") { NativeSelfTest.run(); return }
        if CommandLine.arguments.contains("--update-self-test") { UpdateSelfTest.run(); return }
        if CommandLine.arguments.contains("--access-flow-test") {
            let app = NSApplication.shared
            app.setActivationPolicy(.prohibited)
            Task { await NativeSelfTest.runAccessFlow() }
            app.run()
            return
        }
        let app = NSApplication.shared
        app.setActivationPolicy(.accessory)
        let delegate = AppDelegate(model: AppModel())
        app.delegate = delegate
        withExtendedLifetime(delegate) { app.run() }
    }
}

/// Benchmark mode may hold a window open, but must never load the real library.
private func energyBenchmarkEnvironmentIsValid(staging: Bool) -> Bool {
    let environment = ProcessInfo.processInfo.environment
    guard let statePath = environment["CHIPPYTEA_DATA_DIR"],
          let rootPath = environment["CHIPPYTEA_SCREENSHOT_ROOT"],
          let outputPath = environment["CHIPPYTEA_ENERGY_OUTPUT"],
          let page = environment["CHIPPYTEA_SCREENSHOT_STATE"],
          ["coins", "discover", "settings"].contains(page),
          statePath.hasPrefix("/"), rootPath.hasPrefix("/"), outputPath.hasPrefix("/") else { return false }
    // Preserve the physical spelling: standardization can turn /private/var
    // back into the /var symlink before the no-follow check.
    let state = URL(fileURLWithPath: statePath, isDirectory: true)
    let root = URL(fileURLWithPath: rootPath, isDirectory: true)
    let output = URL(fileURLWithPath: outputPath)
    let fixture = root.deletingLastPathComponent()
    guard root.lastPathComponent == "baseline", state.lastPathComponent == "state",
          state.deletingLastPathComponent() == fixture,
          energyPhysicalDirectory(root), energyPhysicalDirectory(state),
          energyPhysicalDirectory(output.deletingLastPathComponent()),
          output.lastPathComponent == "ready.json",
          environment["CHIPPYTEA_SCREENSHOT"] == output.deletingLastPathComponent().appendingPathComponent("window.png").path,
          let marker = energyReadJSON(fixture.appendingPathComponent(".chippytea-benchmark-fixture.json")),
          marker["magic"] as? String == "chippytea-disposable-benchmark-v1",
          marker["status"] as? String == "complete",
          marker["baseline_relative_path"] as? String == "baseline",
          let stateMarker = energyReadJSON(fixture.appendingPathComponent(".chippytea-energy-state.json")),
          stateMarker["magic"] as? String == "chippytea-synthetic-energy-state-v1",
          let coins = stateMarker["coins"] as? Int, (0...1_000_000).contains(coins),
          energyStateDirectoryIsValid(state, staging: staging) else { return false }
    let outputs = CommandLine.arguments.contains("--maintenance-benchmark")
        ? ["ready.json", "started.json", "endpoint.json", "final.json", "writes.json"] : ["ready.json", "final.json"]
    for name in outputs {
        var info = stat()
        if lstat(output.deletingLastPathComponent().appendingPathComponent(name).path, &info) == 0 || errno != ENOENT { return false }
    }
    if staging { return true }
    guard let bookmarks = energyReadJSON(state.appendingPathComponent("bookmarks.json")),
          Set(bookmarks.keys) == Set([root.path]) else { return false }
    return energyLibraryIsIsolated(state.appendingPathComponent("library.sqlite"), root: root, coins: coins)
}

private func energyStateDirectoryIsValid(_ state: URL, staging: Bool) -> Bool {
    let allowed: Set<String> = ["bookmarks.json", "library.sqlite", "library.sqlite-wal", "library.sqlite-shm", "library.lock"]
    var failed = false
    guard let entries = FileManager.default.enumerator(at: state, includingPropertiesForKeys: nil,
        options: [.skipsSubdirectoryDescendants], errorHandler: { _, _ in failed = true; return false }) else { return false }
    var count = 0
    while let entry = entries.nextObject() {
        count += 1
        guard !staging, count <= allowed.count, let file = entry as? URL,
              allowed.contains(file.lastPathComponent),
              energyPhysicalFile(file, maximumBytes: 64 * 1024 * 1024) else { return false }
    }
    return !failed
}

private func energyPhysicalDirectory(_ url: URL) -> Bool {
    let fd = open(url.path, O_SEARCH | O_NOFOLLOW_ANY | O_CLOEXEC)
    guard fd >= 0 else { return false }
    defer { close(fd) }
    var info = stat()
    return fstat(fd, &info) == 0 && info.st_uid == geteuid()
        && info.st_mode & mode_t(S_IFMT) == mode_t(S_IFDIR) && info.st_flags & 0x4000_0000 == 0
}

private func energyOpenFile(_ url: URL, maximumBytes: Int64) -> Int32? {
    let fd = open(url.path, O_RDONLY | O_NOFOLLOW_ANY | O_CLOEXEC | O_NONBLOCK)
    guard fd >= 0 else { return nil }
    var info = stat()
    guard fstat(fd, &info) == 0, info.st_mode & mode_t(S_IFMT) == mode_t(S_IFREG),
          info.st_uid == geteuid(), info.st_nlink == 1, info.st_size >= 0,
          info.st_size <= maximumBytes, info.st_flags & 0x4000_0000 == 0 else { close(fd); return nil }
    return fd
}

private func energyPhysicalFile(_ url: URL, maximumBytes: Int64) -> Bool {
    guard let fd = energyOpenFile(url, maximumBytes: maximumBytes) else { return false }
    close(fd)
    return true
}

private func energyReadJSON(_ url: URL) -> [String: Any]? {
    guard let fd = energyOpenFile(url, maximumBytes: 65_536) else { return nil }
    let file = FileHandle(fileDescriptor: fd, closeOnDealloc: true)
    defer { try? file.close() }
    guard let data = try? file.read(upToCount: 65_537), data.count <= 65_536 else { return nil }
    return (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
}

/// Inspect before Engine.open can reconcile operations or resume saved roots.
/// A reused measurement library must still contain only its synthetic fixture.
private func energyLibraryIsIsolated(_ database: URL, root: URL, coins: Int) -> Bool {
    guard energyPhysicalFile(database, maximumBytes: 64 * 1024 * 1024) else { return false }
    var connection: OpaquePointer?
    guard sqlite3_open_v2(database.path, &connection, SQLITE_OPEN_READONLY | SQLITE_OPEN_NOFOLLOW, nil) == SQLITE_OK else {
        if let connection { sqlite3_close(connection) }
        return false
    }
    defer { sqlite3_close(connection) }
    let sql = """
        SELECT (SELECT count(*) FROM roots)=1
          AND (SELECT count(*) FROM roots WHERE path=?1 AND json_extract(json,'$.path')=?1 AND json_extract(json,'$.kind')='folder')=1
          AND (SELECT count(*) FROM wallet)=1
          AND (SELECT count(*) FROM wallet WHERE id=1 AND collected=?2 AND remainder=50000000 AND credited=?3)=1
          AND (SELECT count(*) FROM earnings)=0 AND (SELECT count(*) FROM operations)=0
          AND (SELECT count(*) FROM pending_scopes)=0 AND (SELECT count(*) FROM active_scopes)=0
          AND (SELECT count(*) FROM refreshes)=0
        """
    var statement: OpaquePointer?
    guard sqlite3_prepare_v2(connection, sql, -1, &statement, nil) == SQLITE_OK else { return false }
    defer { sqlite3_finalize(statement) }
    let copiedText = unsafeBitCast(-1, to: sqlite3_destructor_type.self)
    guard root.path.withCString({ sqlite3_bind_text(statement, 1, $0, -1, copiedText) }) == SQLITE_OK,
          sqlite3_bind_int64(statement, 2, Int64(coins)) == SQLITE_OK,
          sqlite3_bind_int64(statement, 3, Int64(coins) * 100_000_000 + 50_000_000) == SQLITE_OK else { return false }
    return sqlite3_step(statement) == SQLITE_ROW && sqlite3_column_int(statement, 0) == 1
}

/// A borderless panel that hangs off the status item. It takes keyboard focus so search
/// fields and Esc work, but never activates the app on its own.
final class TrayPanel: NSPanel {
    override var canBecomeKey: Bool { true }
    var onCancel: (() -> Void)?
    override func cancelOperation(_ sender: Any?) { onCancel?() }
}

@MainActor final class AppDelegate: NSObject, NSApplicationDelegate, NSWindowDelegate, NSMenuItemValidation {
    private var statusItem: NSStatusItem!
    private var panel: TrayPanel!
    private var hosting: NSHostingView<RootView>!
    let model: AppModel
    private let updates: UpdateController
    private var updateObservations: [AnyCancellable] = []
    private var lastOpenMilliseconds = 0.0
    /// The benchmark drives show/hide itself; losing key focus must not race it.
    private var suppressAutoDismiss = false
    private var energyQuery: DispatchSourceSignal?
    private var energyObservations: [AnyCancellable] = []
    private var energyInvalidations = Set<String>()
    private var energyArmed = false
    private var scanBenchmark: NativeScanBenchmark?
    private var maintenanceBenchmark: NativeMaintenanceBenchmark?
    private var storageMonitor: StorageStatusMonitor?
    private var storageObservation: AnyCancellable?
    private var storageStatus: StorageStatus?
    private var storageSampleReceived = false
    private var pendingChips: UInt64 = 0
    /// Prints the first show's real geometry once, so placement can be checked without
    /// photographing the user's screen.
    private var reportPlacement = CommandLine.arguments.contains("--report-placement")

    init(model: AppModel) {
        self.model = model
        self.updates = UpdateController(model: model)
        super.init()
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        if let button = statusItem.button {
            button.image = fishTemplateImage(size: 18)
            button.image?.accessibilityDescription = "Chippytea"
            button.imagePosition = .imageLeading
            button.font = .monospacedDigitSystemFont(ofSize: 11, weight: .semibold)
            button.target = self; button.action = #selector(statusClicked(_:))
            button.sendAction(on: [.leftMouseUp, .rightMouseUp])
            button.toolTip = "Chippytea — your chips"
        }

        panel = TrayPanel(contentRect: NSRect(x: 0, y: 0, width: TeaTheme.panelWidth, height: TeaTheme.panelHeight),
                          styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = true
        panel.level = .statusBar
        panel.collectionBehavior = [.transient, .ignoresCycle]
        panel.isMovable = false
        panel.isMovableByWindowBackground = false
        panel.hidesOnDeactivate = false
        panel.isReleasedWhenClosed = false
        panel.animationBehavior = .none
        panel.delegate = self
        panel.onCancel = { [weak self] in self?.hidePanel() }

        let isPerformanceBenchmark = CommandLine.arguments.contains("--energy-benchmark")
            || CommandLine.arguments.contains("--scan-benchmark")
            || CommandLine.arguments.contains("--maintenance-benchmark")
        hosting = NSHostingView(rootView: RootView(model: model, updates: updates, isPerformanceBenchmark: isPerformanceBenchmark))
        hosting.wantsLayer = true
        hosting.layer?.backgroundColor = NSColor.clear.cgColor
        panel.contentView = hosting

        model.onWalletChange = { [weak self] pending in
            guard let self else { return }
            self.pendingChips = pending
            self.updateStatusItem()
        }
        model.onSystemDialogDepthChange = { [weak self] depth in
            self?.updates.modelStateDidChange()
            if depth > 0, self?.energyArmed == true { self?.energyInvalidations.insert("system_dialog_opened") }
            if depth > 0 { self?.scanBenchmark?.invalidate("system_dialog_opened") }
            if depth > 0 { self?.maintenanceBenchmark?.invalidate("system_dialog_opened") }
            guard depth == 0, let self, self.panel.isVisible else { return }
            self.panel.makeKeyAndOrderFront(nil)
        }

        let mainMenu = NSMenu()
        let appMenu = NSMenu()
        appMenu.addItem(withTitle: "Show Chippytea", action: #selector(showWindow), keyEquivalent: "0").target = self
        appMenu.addItem(withTitle: "Check for Updates…", action: #selector(checkForUpdates), keyEquivalent: "").target = self
        appMenu.addItem(NSMenuItem.separator())
        appMenu.addItem(withTitle: "Quit Chippytea", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        let item = NSMenuItem(); item.submenu = appMenu; mainMenu.addItem(item)
        let editMenu = NSMenu(title: "Edit")
        editMenu.addItem(withTitle: "Copy", action: #selector(NSText.copy(_:)), keyEquivalent: "c")
        editMenu.addItem(withTitle: "Paste", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
        editMenu.addItem(withTitle: "Select All", action: #selector(NSText.selectAll(_:)), keyEquivalent: "a")
        let edit = NSMenuItem(); edit.submenu = editMenu; mainMenu.addItem(edit)
        NSApp.mainMenu = mainMenu

        let screenshotOnly = CommandLine.arguments.contains("--screenshot")
        // Keep existing fixture capture and measurement modes unchanged.
        let allowsBackgroundServices = !isPerformanceBenchmark && !screenshotOnly
            && !CommandLine.arguments.contains("--ui-benchmark")
        if allowsBackgroundServices {
            startStorageReporting()
            updateObservations = [
                model.objectWillChange.receive(on: RunLoop.main).sink { [weak self] _ in
                    self?.updates.modelStateDidChange()
                },
                updates.$availableVersion.removeDuplicates().receive(on: RunLoop.main).sink { [weak self] _ in
                    self?.updateStatusItem()
                }
            ]
        }
        if !CommandLine.arguments.contains("--background") && !screenshotOnly {
            // The startup capacity query already covers this first presentation.
            presentPanel(deadline: Date().addingTimeInterval(2), refreshStorage: false)
        }
        Task {
            await model.start()
            if allowsBackgroundServices { updates.start() }
        }
        if CommandLine.arguments.contains("--energy-benchmark") { suppressAutoDismiss = true; Task { await stageEnergyBenchmark() } }
        else if CommandLine.arguments.contains("--scan-benchmark") { suppressAutoDismiss = true; Task { await stageScanBenchmark() } }
        else if CommandLine.arguments.contains("--maintenance-benchmark") { suppressAutoDismiss = true; Task { await stageMaintenanceBenchmark() } }
        else if CommandLine.arguments.contains("--ui-benchmark") { suppressAutoDismiss = true; Task { await runWindowBenchmark() } }
        else if screenshotOnly { suppressAutoDismiss = true; Task { await runScreenshot() } }
    }

    private func startStorageReporting() {
        storageMonitor = StorageStatusMonitor { [weak self] value in
            guard let self else { return }
            self.storageStatus = value
            self.storageSampleReceived = true
            self.model.updateStorageStatus(value)
            self.updateStatusItem()
        }
        storageObservation = model.snapshots.map(\.cleaning).removeDuplicates().dropFirst()
            .filter { !$0 }.sink { [weak self] _ in self?.storageMonitor?.refresh() }
        updateStatusItem()
        storageMonitor?.start()
    }

    private func updateStatusItem() {
        guard let button = statusItem?.button else { return }
        let chips = pendingChips > 0
            ? "\(chipsPhrase(pendingChips)) ready to collect"
            : "Your chips"
        var title: String
        var detail: String
        if storageMonitor != nil {
            title = " " + (storageStatus?.menuTitle ?? (storageSampleReceived ? "Storage unavailable" : "Storage…"))
            let capacity = storageStatus?.detail ?? (storageSampleReceived
                ? "Startup disk storage is unavailable." : "Checking startup disk storage…")
            detail = "Chippytea\n\(capacity)\n\(chips)"
        } else {
            title = pendingChips > 0 ? " \(pendingChips.formatted())" : ""
            detail = pendingChips > 0
                ? "\(chipsPhrase(pendingChips)) ready to collect"
                : "Chippytea — your chips"
        }
        if let version = updates.availableVersion {
            title += " ↑"
            detail += "\nChippytea \(version) is available. Open Chippytea to update."
        }
        if button.title != title { button.title = title }
        if button.toolTip != detail {
            button.toolTip = detail
            if storageMonitor != nil {
                button.setAccessibilityLabel(detail.replacingOccurrences(of: "\n", with: ". "))
            }
        }
    }

    @objc private func statusClicked(_ sender: Any?) {
        if NSApp.currentEvent?.type == .rightMouseUp {
            let menu = NSMenu()
            menu.addItem(withTitle: "Show your chips", action: #selector(showWindow), keyEquivalent: "").target = self
            let updateTitle = updates.availableVersion.map { "Update to Chippytea \($0)…" } ?? "Check for Updates…"
            menu.addItem(withTitle: updateTitle, action: #selector(checkForUpdates), keyEquivalent: "").target = self
            menu.addItem(NSMenuItem.separator())
            menu.addItem(withTitle: "Quit Chippytea", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
            statusItem.menu = menu; statusItem.button?.performClick(nil); statusItem.menu = nil
        } else if panel.isVisible {
            hidePanel()
        } else {
            showWindow()
        }
    }

    @objc func showWindow() { presentPanel(deadline: Date().addingTimeInterval(2)) }

    @objc private func checkForUpdates() { updates.checkForUpdates() }

    func validateMenuItem(_ menuItem: NSMenuItem) -> Bool {
        if menuItem.action == #selector(checkForUpdates) { return updates.canCheckForUpdates }
        return true
    }

    /// Shows the panel only once its final frame is known. During launch the status item's
    /// window is still reported at (0, 0, 34, 0); hanging the page off that anchor is what
    /// used to flash it in the top-left corner before it jumped to the tray. So the show is
    /// deferred — in real time, not runloop ticks — until the tray icon has been placed.
    private func presentPanel(deadline: Date, refreshStorage: Bool = true) {
        guard let placement = panelPlacement(allowFallback: Date() >= deadline) else {
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.02) { [weak self] in
                self?.presentPanel(deadline: deadline, refreshStorage: refreshStorage)
            }
            return
        }
        let start = CACurrentMediaTime()
        // Everything that can move, resize or re-lay-out the page happens off-screen.
        model.panelHeight = placement.height
        model.anchorX = placement.anchorX
        model.presentation &+= 1
        model.windowOpened()
        if refreshStorage { storageMonitor?.refresh() }
        panel.setFrame(placement.frame, display: false)
        hosting.frame = NSRect(origin: .zero, size: placement.frame.size)
        hosting.layoutSubtreeIfNeeded()
        // Only now does anything become visible, already in its final place.
        panel.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        panel.displayIfNeeded()
        panel.invalidateShadow()
        lastOpenMilliseconds = (CACurrentMediaTime() - start) * 1000
        if reportPlacement {
            reportPlacement = false
            let button = statusItem.button?.window?.frame ?? .zero
            fputs("CHIPPYTEA_PLACEMENT panel=\(placement.frame) tray=\(button) anchor_x=\(placement.anchorX)\n", stderr)
        }
    }

    private func hidePanel() {
        guard panel.isVisible else { return }
        model.windowClosed()
        panel.orderOut(nil)
    }

    /// Where the panel hangs from the status item: centred under the icon, clamped to the
    /// screen, with the tape anchor republished in panel coordinates. Nil while the status
    /// item has no real placed window to hang from — unless the caller has waited long
    /// enough that never opening would be worse than opening beside the tray area.
    private func panelPlacement(allowFallback: Bool) -> (frame: NSRect, anchorX: CGFloat, height: CGFloat)? {
        let buttonWindow = statusItem.button?.window
        guard let screen = buttonWindow?.screen ?? NSScreen.main else { return nil }
        let visible = screen.visibleFrame
        let buttonFrame = buttonWindow?.frame ?? .zero
        let placed = buttonFrame.width > 1 && buttonFrame.height > 1 && buttonFrame.maxY > visible.midY
        guard placed || allowFallback else { return nil }
        let width = TeaTheme.panelWidth
        let height = min(TeaTheme.panelHeight, visible.height - 24)
        let anchor = placed ? buttonFrame.midX : visible.maxX - 40
        let x = min(max(visible.minX + 12, anchor - width / 2), max(visible.minX + 12, visible.maxX - width - 12))
        let y = visible.maxY - height - 2
        let tapeMargin = TeaTheme.tapeWidth / 2 + 14
        return (NSRect(x: x, y: y, width: width, height: height),
                min(max(tapeMargin, anchor - x), width - tapeMargin),
                height)
    }

    func windowDidResignKey(_ notification: Notification) {
        guard !suppressAutoDismiss, (notification.object as AnyObject?) === panel else { return }
        guard model.systemDialogDepth == 0 else { return }
        hidePanel()
    }

    func windowDidBecomeKey(_ notification: Notification) {
        guard let window = notification.object as? NSWindow, window === panel else { return }
        model.diskAccessReturned()
    }

    func windowDidChangeOcclusionState(_ notification: Notification) {
        guard (notification.object as AnyObject?) === panel else { return }
        if !panel.isVisible || !panel.occlusionState.contains(.visible) {
            if energyArmed { energyInvalidations.insert("window_hidden_or_occluded") }
            scanBenchmark?.invalidate("window_hidden_or_occluded")
            maintenanceBenchmark?.invalidate("window_hidden_or_occluded")
        }
    }

    func windowWillClose(_ notification: Notification) {
        guard (notification.object as AnyObject?) === panel else { return }
        if energyArmed { energyInvalidations.insert("window_closed") }
        scanBenchmark?.invalidate("window_closed")
        maintenanceBenchmark?.invalidate("window_closed")
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }
    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        // Sparkle also asks AppKit to quit. Never interrupt a destructive job,
        // including queued cleanups accepted before an update was downloaded.
        guard !model.hasCleanupWork else {
            model.errorMessage = "Finish or cancel cleanup before quitting or installing an update."
            showWindow()
            return .terminateCancel
        }
        return .terminateNow
    }
    func applicationWillTerminate(_ notification: Notification) {
        updateObservations.removeAll()
        updates.shutdown()
        storageObservation?.cancel()
        storageMonitor?.stop()
        model.client?.cancel()
    }

    private func settle(_ seconds: Double, until: () -> Bool) async {
        let deadline = Date().addingTimeInterval(seconds)
        while Date() < deadline && !until() { try? await Task.sleep(for: .milliseconds(80)) }
    }

    private func writeScreenshot() {
        guard let path = ProcessInfo.processInfo.environment["CHIPPYTEA_SCREENSHOT"], !path.isEmpty,
              let bitmap = hosting.bitmapImageRepForCachingDisplay(in: hosting.bounds) else { return }
        hosting.cacheDisplay(in: hosting.bounds, to: bitmap)
        try? bitmap.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: path))
    }

    /// `--screenshot`: show the page once, in its final place, let the entrance settle,
    /// write the PNG and quit. No open/close cycling — that belongs to `--ui-benchmark`.
    private func runScreenshot() async {
        await settle(10) { model.client != nil }
        showWindow()
        await settle(5) { panel.isVisible }
        await stageScreenshot()
        try? await Task.sleep(for: .milliseconds(500))
        writeScreenshot()
        NSApp.terminate(nil)
    }

    /// Screenshot staging, also used for a validated resting energy measurement: puts the
    /// panel into a named state (and, with a disposable fixture folder, real engine state) so
    /// a capture can show more than the default screen. Collection is presentation-only.
    func stageScreenshot(environment: [String: String] = ProcessInfo.processInfo.environment) async {
        // Captures run without the live capacity monitor. A fixed synthetic
        // sample keeps the home gauge visible and the capture deterministic;
        // it can never reach a normal run, where the monitor owns the value.
        if storageMonitor == nil && environment["CHIPPYTEA_SCREENSHOT"] != nil {
            model.updateStorageStatus(StorageStatus(blockSize: 4096, totalBlocks: 242_500_000,
                                                    availableBlocks: 100_500_000))
        }
        if environment["CHIPPYTEA_SCREENSHOT_STATE"] == "collect" {
            // Replay a synthetic burst without authorizing folders, cleaning an
            // indexed candidate, or changing rewards. Subtraction also works
            // when the displayed balance is UInt64.max.
            let to = max(model.snapshot.wallet.collectedCoins, 8)
            model.showDiskAccess = false
            model.destination = .coins
            model.collection = CollectionBurst(from: to - 8, to: to, amount: 8)
            return
        }
        if let fixture = environment["CHIPPYTEA_SCREENSHOT_ROOT"], !fixture.isEmpty, model.snapshot.roots.isEmpty {
            model.authorizeAndScan(path: fixture)
            await settle(60) { !model.busy && !model.snapshot.scanning && !model.snapshot.candidates.isEmpty }
        }
        switch environment["CHIPPYTEA_SCREENSHOT_STATE"] ?? "" {
        case "coins":
            model.showDiskAccess = false
            model.destination = .coins
        case "discover":
            model.showDiskAccess = false
            model.destination = .discover
        case "activity":
            model.showDiskAccess = false
            model.destination = .activity
            // Fictional receipts for capture only, like the collect state's
            // synthetic burst: nothing here touches the engine or real files.
            // The opening collect's snapshot reload must land first, or its
            // authoritative empty history would replace the staged rows.
            if model.snapshot.history.isEmpty {
                await settle(3) { !model.busy && model.client != nil }
                try? await Task.sleep(for: .milliseconds(700))
                model.snapshot.history = Self.sampleReceipts
                model.expandedReceipt = Self.sampleReceipts.dropFirst().first?.id
            }
        case "settings":
            model.showDiskAccess = false
            model.destination = .settings
        case "disk-access", "disk-access-waiting", "disk-access-starting":
            // Render setup intent without reading user folders or changing TCC.
            model.showDiskAccess = true
            switch environment["CHIPPYTEA_SCREENSHOT_STATE"] {
            case "disk-access-waiting": model.diskAccessPhase = .waiting
            case "disk-access-starting": model.diskAccessPhase = .starting
            default: model.diskAccessPhase = .intro
            }
        case "expanded":
            model.destination = .coins
            model.expandedSuggestion = model.snapshot.candidates.first { $0.blockedReason == nil }?.id
        case "review":
            if let item = model.snapshot.candidates.first(where: { $0.blockedReason == nil }) { model.reviewOne(item) }
        default:
            break
        }
    }

    /// Fictional order-book rows for the activity capture. Sizes, dates and
    /// paths are invented; the staging never performs or records operations.
    private static var sampleReceipts: [Receipt] {
        let base = Int64(Date().timeIntervalSince1970)
        func receipt(_ index: Int, _ title: String, _ path: String, _ operation: String, _ outcome: String,
                     _ detail: String, bytes: UInt64, credited: UInt64, coins: UInt64,
                     trashPath: String? = nil, canRestore: Bool = false) -> Receipt {
            Receipt(id: "sample-\(index)", path: path, title: title, operation: operation, outcome: outcome,
                    detail: detail, createdAt: base - Int64(index) * 86_400 * 2 - 3600,
                    reportedBytes: bytes, observedBytes: operation == "trash" ? 0 : bytes,
                    creditedBytes: credited, coins: coins, trashPath: trashPath, canRestore: canRestore)
        }
        return [
            receipt(0, "cap-web dependencies", "/Users/sample/Projects/cap-web/node_modules",
                    "permanent", "removed",
                    "Permanently removed the reviewed developer artifacts. Reinstalling or rebuilding creates new artifacts; it does not restore these contents.",
                    bytes: 1_264_000_000, credited: 1_190_000_000, coins: 11),
            receipt(1, "render-farm build artifacts", "/Users/sample/Projects/render-farm/target",
                    "permanent", "removed",
                    "Permanently removed the reviewed developer artifacts. Credited from conservative storage observations; APFS sharing and other disk activity can reduce game credit.",
                    bytes: 9_812_000_000, credited: 9_400_000_000, coins: 94),
            receipt(2, "Xcode_15.4.dmg", "/Users/sample/Downloads/Xcode_15.4.dmg",
                    "trash", "trashed",
                    "Moved to native Trash. This did not earn chips or establish freed space. You can restore this item while its identity and original destination remain valid.",
                    bytes: 3_300_000_000, credited: 0, coins: 0,
                    trashPath: "/Users/sample/.Trash/Xcode_15.4.dmg", canRestore: true),
            receipt(3, "holiday-cut.mov", "/Users/sample/Downloads/holiday-cut.mov",
                    "trash", "restored",
                    "Restored from Trash to the original destination after identity checks.",
                    bytes: 2_100_000_000, credited: 0, coins: 0),
            receipt(4, "docs-site dependencies", "/Users/sample/Projects/docs-site/node_modules",
                    "permanent", "partial",
                    "Removal stopped part-way; remaining files were left in place. Space credit covers only verified recovery.",
                    bytes: 640_000_000, credited: 210_000_000, coins: 2),
            receipt(5, "analytics dependencies", "/Users/sample/Projects/analytics/node_modules",
                    "permanent", "removed",
                    "Permanently removed the reviewed developer artifacts. No space credited: stable storage observations could not support a reward.",
                    bytes: 410_000_000, credited: 0, coins: 0),
            receipt(6, "installer-archive.pkg", "/Users/sample/Downloads/installer-archive.pkg",
                    "trash", "failed",
                    "macOS refused the Trash operation. The file was not changed.",
                    bytes: 890_000_000, credited: 0, coins: 0),
            receipt(7, "old-blog dependencies", "/Users/sample/Projects/old-blog/node_modules",
                    "permanent", "removed",
                    "Permanently removed the reviewed developer artifacts.",
                    bytes: 350_000_000, credited: 320_000_000, coins: 3),
        ]
    }

    /// Leave a settled, visible page for external CPU measurements. The caller
    /// measures the process separately, without a timer or logger in the app.
    private func stageEnergyBenchmark() async {
        await settle(10) { model.client != nil && !model.busy }
        await stageScreenshot()
        await settle(60) { !model.busy && !model.discoveryPresentation.isRequestPending && !model.snapshot.scanning && !model.snapshot.cleaning }
        guard model.client != nil, !model.busy, model.errorMessage == nil,
              !model.discoveryPresentation.isRequestPending,
              !model.snapshot.scanning, !model.snapshot.cleaning,
              model.snapshot.wallet.pendingCoins == 0, model.collection == nil,
              let path = ProcessInfo.processInfo.environment["CHIPPYTEA_ENERGY_OUTPUT"] else {
            fputs("Energy measurement could not reach a settled fixture state.\n", stderr)
            NSApp.terminate(nil)
            return
        }
        showWindow()
        await settle(5) { panel.isVisible && panel.occlusionState.contains(.visible) }
        guard panel.isVisible && panel.occlusionState.contains(.visible) else { NSApp.terminate(nil); return }
        armEnergyMeasurement(output: URL(fileURLWithPath: path))
        guard writeEnergyState(to: URL(fileURLWithPath: path), final: false) else { NSApp.terminate(nil); return }
        writeScreenshot()
        if CommandLine.arguments.contains("--energy-invalidation-exercise") {
            // Exercise the ordinary actions once. No OS input is synthesized,
            // and the invalidated launch must never produce accepted timings.
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { [weak self] in
                guard let self, self.energyArmed else { return }
                self.hidePanel()
                self.showWindow()
            }
        }
    }

    private func stageScanBenchmark() async {
        await settle(60) { model.client != nil && !model.busy && !model.discoveryPresentation.isRequestPending && !model.snapshot.scanning }
        guard model.client != nil, !model.busy, !model.discoveryPresentation.isRequestPending,
              !model.snapshot.scanning, !model.snapshot.cleaning,
              let output = ProcessInfo.processInfo.environment["CHIPPYTEA_ENERGY_OUTPUT"],
              let root = ProcessInfo.processInfo.environment["CHIPPYTEA_SCREENSHOT_ROOT"] else {
            fputs("Scan measurement could not restore its isolated fixture.\n", stderr)
            NSApp.terminate(nil)
            return
        }
        model.destination = .discover
        model.showDiskAccess = false
        showWindow()
        await settle(5) { panel.isVisible && panel.occlusionState.contains(.visible) }
        let benchmark = NativeScanBenchmark(model: model, rootPath: root, output: URL(fileURLWithPath: output), windowState: { [weak self] in
            guard let self else { return (visible: false, occluded: true) }
            return (visible: self.panel.isVisible, occluded: !self.panel.occlusionState.contains(.visible))
        })
        scanBenchmark = benchmark
        benchmark.prepare()
    }

    private func stageMaintenanceBenchmark() async {
        await settle(60) { model.client != nil && !model.busy && !model.discoveryPresentation.isRequestPending && !model.snapshot.scanning }
        guard model.client != nil, !model.busy, !model.discoveryPresentation.isRequestPending,
              !model.snapshot.scanning, !model.snapshot.cleaning,
              let output = ProcessInfo.processInfo.environment["CHIPPYTEA_ENERGY_OUTPUT"],
              let root = ProcessInfo.processInfo.environment["CHIPPYTEA_SCREENSHOT_ROOT"] else {
            fputs("Maintenance measurement could not restore its isolated fixture.\n", stderr)
            NSApp.terminate(nil)
            return
        }
        model.destination = .discover
        model.showDiskAccess = false
        showWindow()
        await settle(5) { panel.isVisible && panel.occlusionState.contains(.visible) }
        let benchmark = NativeMaintenanceBenchmark(model: model, rootPath: root, output: URL(fileURLWithPath: output), windowState: { [weak self] in
            guard let self else { return (visible: false, occluded: true) }
            return (visible: self.panel.isVisible, occluded: !self.panel.occlusionState.contains(.visible))
        })
        maintenanceBenchmark = benchmark
        benchmark.prepare()
    }

    private func energyState(final: Bool) -> [String: Any] {
        [
            "energy_protocol": 2,
            "pid": getpid(),
            "final": final,
            "visible_windows": NSApp.windows.filter { $0.isVisible && $0 == panel }.count,
            "panel_visible": model.panelVisible,
            "occluded": !panel.occlusionState.contains(.visible),
            "destination": model.destination.rawValue,
            "collected_coins": model.snapshot.wallet.collectedCoins,
            "candidates": model.snapshot.candidates.count,
            "scanning": model.snapshot.scanning,
            "cleaning": model.snapshot.cleaning,
            "busy": model.busy,
            "review_open": model.showReview,
            "access_setup_open": model.showDiskAccess,
            "collecting": model.collection != nil,
            "system_dialog_open": model.systemDialogDepth > 0,
            "has_error": model.errorMessage != nil,
            "reduce_motion": model.reduceMotion,
            "effective_reduce_motion": model.reduceMotion || NSWorkspace.shared.accessibilityDisplayShouldReduceMotion,
            "invalidations": energyInvalidations.sorted(),
        ]
    }

    private func writeEnergyState(to output: URL, final: Bool) -> Bool {
        guard let data = try? JSONSerialization.data(withJSONObject: energyState(final: final), options: [.prettyPrinted]) else { return false }
        return (try? data.write(to: output, options: .atomic)) != nil
    }

    /// Changes invalidate the sample as they occur, including a hide followed
    /// by reopening. No recurring native timer or logger runs during sampling.
    private func armEnergyMeasurement(output: URL) {
        let snapshot = model.snapshot
        let destination = model.destination
        let reduceMotion = model.reduceMotion
        let systemReduceMotion = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
        energyInvalidations.removeAll()
        energyArmed = true
        energyObservations = [
            model.$panelVisible.dropFirst().sink { [weak self] in if !$0 { self?.energyInvalidations.insert("panel_hidden") } },
            model.$destination.dropFirst().sink { [weak self] in if $0 != destination { self?.energyInvalidations.insert("page_changed") } },
            model.snapshots.dropFirst().sink { [weak self] in if $0 != snapshot { self?.energyInvalidations.insert("snapshot_changed") } },
            model.$reduceMotion.dropFirst().sink { [weak self] in if $0 != reduceMotion { self?.energyInvalidations.insert("reduce_motion_changed") } },
            model.$collection.dropFirst().sink { [weak self] in if $0 != nil { self?.energyInvalidations.insert("collection_started") } },
            model.$busy.dropFirst().sink { [weak self] in if $0 { self?.energyInvalidations.insert("operation_started") } },
            model.$showReview.dropFirst().sink { [weak self] in if $0 { self?.energyInvalidations.insert("review_opened") } },
            model.$showDiskAccess.dropFirst().sink { [weak self] in if $0 { self?.energyInvalidations.insert("access_setup_opened") } },
            model.$errorMessage.dropFirst().sink { [weak self] in if $0 != nil { self?.energyInvalidations.insert("error_presented") } },
            NSWorkspace.shared.notificationCenter.publisher(for: NSWorkspace.accessibilityDisplayOptionsDidChangeNotification)
                .receive(on: DispatchQueue.main).sink { [weak self] _ in
                    if NSWorkspace.shared.accessibilityDisplayShouldReduceMotion != systemReduceMotion { self?.energyInvalidations.insert("system_reduce_motion_changed") }
                },
        ]
        // Only this gated benchmark installs the handler. The protocol is
        // advertised after it is ready, so older binaries cannot be signalled.
        signal(SIGUSR1, SIG_IGN)
        let query = DispatchSource.makeSignalSource(signal: SIGUSR1, queue: .main)
        query.setEventHandler { [weak self] in
            guard let self else { return }
            _ = self.writeEnergyState(to: output.deletingLastPathComponent().appendingPathComponent("final.json"), final: true)
            self.energyArmed = false
            self.energyObservations.removeAll()
            self.energyQuery?.cancel()
            self.energyQuery = nil
        }
        energyQuery = query
        query.resume()
    }

    private func runWindowBenchmark() async {
        try? await Task.sleep(for: .seconds(2))
        var samples: [Double] = []
        for _ in 0..<40 {
            model.windowClosed(); panel.orderOut(nil)
            try? await Task.sleep(for: .milliseconds(100))
            showWindow(); samples.append(lastOpenMilliseconds)
            try? await Task.sleep(for: .milliseconds(120))
        }
        let sorted = samples.sorted()
        let output: [String: Any] = ["samples_ms": samples, "p95_ms": sorted[Int(Double(sorted.count - 1) * 0.95)], "method": "Warm native tray panel makeKeyAndOrderFront through layout/displayIfNeeded; excludes external click delivery and compositor presentation", "visible_windows": NSApp.windows.filter { $0.isVisible && $0 == panel }.count]
        if let data = try? JSONSerialization.data(withJSONObject: output, options: [.prettyPrinted]), let path = ProcessInfo.processInfo.environment["CHIPPYTEA_BENCHMARK_OUTPUT"] { try? data.write(to: URL(fileURLWithPath: path)) }
        await stageScreenshot()
        // Let the entrance animation settle so the capture shows the resting panel.
        try? await Task.sleep(for: .milliseconds(400))
        writeScreenshot()
        print("CHIPPYTEA_WINDOW_BENCHMARK p95_ms=\(sorted[Int(Double(sorted.count - 1) * 0.95)])")
        // Leave the benchmark process in the real closed-window idle state so
        // external CPU measurements do not include visible doodle animations.
        model.windowClosed()
        panel.orderOut(nil)
    }
}
