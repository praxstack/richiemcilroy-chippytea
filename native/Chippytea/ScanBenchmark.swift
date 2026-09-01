import AppKit
import Combine
import Darwin
import Foundation

/// An opt-in, disposable-fixture coordinator. It uses the real Scan action and
/// its existing snapshot publications; it adds no recurring timer or UI poll.
@MainActor final class NativeScanBenchmark {
    private enum Phase { case setup, ready, measuring, finished }
    private struct Usage {
        let nanoseconds: UInt64
        let user: Double
        let system: Double
        let lifetimePeakRSS: Int64

        static func read() -> Usage? {
            var value = rusage()
            guard getrusage(RUSAGE_SELF, &value) == 0 else { return nil }
            return Usage(nanoseconds: DispatchTime.now().uptimeNanoseconds,
                         user: Double(value.ru_utime.tv_sec) + Double(value.ru_utime.tv_usec) / 1_000_000,
                         system: Double(value.ru_stime.tv_sec) + Double(value.ru_stime.tv_usec) / 1_000_000,
                         lifetimePeakRSS: Int64(value.ru_maxrss))
        }
    }

    private let model: AppModel
    private let rootPath: String
    private let output: URL
    private let windowState: () -> (visible: Bool, occluded: Bool)
    private let originalWallet: Wallet
    private let originalHistory: [Receipt]
    private let originalRoots: [ScanRoot]
    private let reduceMotion: Bool
    private let systemReduceMotion: Bool
    private var phase = Phase.setup
    private var observations: [AnyCancellable] = []
    private var startSignal: DispatchSourceSignal?
    private var timeout: Task<Void, Never>?
    private var sawActive = false
    private var setupSnapshot: EngineSnapshot?
    private var cachedPaths: [String] = []
    private var started: Usage?
    private var firstActiveMS: Double?
    private var firstFreshFindingObservedMS: Double?
    private var firstNewPathObservedMS: Double?
    private var snapshotCallbacks = 0
    private var lastSnapshotNS: UInt64?
    private var largestSnapshotGapMS = 0.0

    init(model: AppModel, rootPath: String, output: URL,
         windowState: @escaping () -> (visible: Bool, occluded: Bool)) {
        self.model = model
        self.rootPath = rootPath
        self.output = output
        self.windowState = windowState
        originalWallet = model.snapshot.wallet
        originalHistory = model.snapshot.history
        originalRoots = model.snapshot.roots
        reduceMotion = model.reduceMotion
        systemReduceMotion = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
    }

    func prepare() {
        guard validPresentation(), !model.busy, !model.discoveryPresentation.isRequestPending,
              !model.snapshot.scanning, !model.snapshot.cleaning,
              originalRoots.count == 1, originalRoots[0].path == rootPath,
              originalRoots[0].kind == "folder", originalWallet.pendingCoins == 0,
              originalHistory.isEmpty else { invalidate("invalid_setup_state"); return }
        observations = [
            model.snapshots.dropFirst().sink { [weak self] in self?.receive($0) },
            model.$discoveryPresentation.dropFirst().sink { [weak self] in self?.requestChanged($0) },
            model.$panelVisible.dropFirst().sink { [weak self] in if !$0 { self?.invalidate("panel_hidden") } },
            model.$destination.dropFirst().sink { [weak self] in if $0 != .discover { self?.invalidate("page_changed") } },
            model.$busy.dropFirst().sink { [weak self] in if $0 { self?.invalidate("unexpected_operation") } },
            model.$collection.dropFirst().sink { [weak self] in if $0 != nil { self?.invalidate("collection_started") } },
            model.$showReview.dropFirst().sink { [weak self] in if $0 { self?.invalidate("review_opened") } },
            model.$showDiskAccess.dropFirst().sink { [weak self] in if $0 { self?.invalidate("access_setup_opened") } },
            model.$errorMessage.dropFirst().sink { [weak self] in if $0 != nil { self?.invalidate("error_presented") } },
            model.$reduceMotion.dropFirst().sink { [weak self] value in
                guard let self else { return }
                if value != self.reduceMotion { self.invalidate("reduce_motion_changed") }
            },
            NSWorkspace.shared.notificationCenter.publisher(for: NSWorkspace.accessibilityDisplayOptionsDidChangeNotification)
                .receive(on: DispatchQueue.main).sink { [weak self] _ in
                    guard let self else { return }
                    if NSWorkspace.shared.accessibilityDisplayShouldReduceMotion != self.systemReduceMotion {
                        self.invalidate("system_reduce_motion_changed")
                    }
                },
        ]
        // One deadline bounds setup, the external start handshake and scanning.
        timeout = Task { [weak self] in
            do { try await Task.sleep(for: .seconds(60)) } catch { return }
            self?.invalidate("deadline_exceeded")
        }
        model.refresh() // Completed, untimed setup pass in this same process.
    }

    private func validPresentation() -> Bool {
        let window = windowState()
        return window.visible && !window.occluded && model.panelVisible
            && model.destination == .discover && !model.showReview && !model.showDiskAccess
            && model.systemDialogDepth == 0 && model.collection == nil && model.errorMessage == nil
    }

    private func requestChanged(_ presentation: DiscoveryPresentation) {
        guard phase == .setup || phase == .measuring else { return }
        if !presentation.isRequestPending && !sawActive && !model.snapshot.scanning {
            invalidate("scan_finished_before_native_active_edge_use_a_larger_fixture")
        }
    }

    private func eligiblePaths(_ snapshot: EngineSnapshot) -> [String] {
        let prefix = rootPath + "/"
        return snapshot.candidates.filter(\.recommended).map {
            $0.path.hasPrefix(prefix) ? String($0.path.dropFirst(prefix.count)) : $0.path
        }.sorted()
    }

    private func receive(_ snapshot: EngineSnapshot) {
        guard phase != .finished else { return }
        guard validPresentation(), snapshot.wallet == originalWallet,
              snapshot.history == originalHistory, snapshot.roots == originalRoots,
              !snapshot.cleaning, !snapshot.stats.cancelled,
              snapshot.stats.errors == 0, snapshot.error == nil else {
            invalidate("scan_state_changed_or_failed"); return
        }
        if phase == .ready {
            if snapshot != setupSnapshot { invalidate("unexpected_scan_or_snapshot_before_start") }
            return
        }
        let now = DispatchTime.now().uptimeNanoseconds
        if phase == .measuring, let started {
            snapshotCallbacks += 1
            if let lastSnapshotNS { largestSnapshotGapMS = max(largestSnapshotGapMS, Double(now - lastSnapshotNS) / 1_000_000) }
            lastSnapshotNS = now
            if snapshot.scanning && firstActiveMS == nil { firstActiveMS = Double(now - started.nanoseconds) / 1_000_000 }
        }
        if snapshot.scanning { sawActive = true }
        guard sawActive else { return }
        if phase == .measuring, let started {
            let elapsed = Double(now - started.nanoseconds) / 1_000_000
            // The explicit Scan action resets engine stats. Cached candidate
            // rows do not establish that this scan has found anything yet.
            if snapshot.stats.firstFindingMs != nil && firstFreshFindingObservedMS == nil {
                firstFreshFindingObservedMS = elapsed
            }
            if firstNewPathObservedMS == nil && eligiblePaths(snapshot).contains(where: { !cachedPaths.contains($0) }) {
                firstNewPathObservedMS = elapsed
            }
        }
        guard !snapshot.scanning else { return }
        guard snapshot.stats.complete else { invalidate("incomplete_scan"); return }
        if phase == .setup {
            setupSnapshot = snapshot
            cachedPaths = eligiblePaths(snapshot)
            phase = .ready
            // Published emits before assignment; let the ordinary reload apply
            // its final snapshot and presentation before advertising readiness.
            DispatchQueue.main.async { [weak self] in self?.publishReady() }
        } else {
            finish(snapshot)
        }
    }

    private func publishReady() {
        guard phase == .ready, validPresentation(), !model.busy,
              !model.discoveryPresentation.isRequestPending,
              model.snapshot == setupSnapshot else { invalidate("setup_did_not_settle"); return }
        signal(SIGUSR1, SIG_IGN)
        let source = DispatchSource.makeSignalSource(signal: SIGUSR1, queue: .main)
        source.setEventHandler { [weak self] in self?.startMeasurement() }
        startSignal = source
        source.resume()
        var record = state(model.snapshot)
        record["phase"] = "ready"
        record["cached_eligible_paths"] = cachedPaths
        record["setup_stats"] = encoded(model.snapshot.stats)
        record["deadline_seconds_including_setup"] = 60
        if !write(record, name: "ready.json") { invalidate("ready_output_failed") }
    }

    private func startMeasurement() {
        guard phase == .ready, validPresentation(), !model.busy,
              !model.discoveryPresentation.isRequestPending,
              !model.snapshot.scanning, model.snapshot == setupSnapshot else {
            invalidate("invalid_start_state"); return
        }
        sawActive = false
        guard let usage = Usage.read() else { invalidate("cpu_sample_failed"); return }
        started = usage
        phase = .measuring
        model.refresh()
    }

    private func finish(_ snapshot: EngineSnapshot) {
        // Capture before JSON, filesystem output or subscription disposal.
        guard let end = Usage.read(), let start = started else { invalidate("cpu_sample_failed"); return }
        let wall = Double(end.nanoseconds - start.nanoseconds) / 1_000_000_000
        let user = end.user - start.user
        let system = end.system - start.system
        var record = state(snapshot)
        record["phase"] = "complete"
        record["valid"] = true
        record["observed_active_edge"] = sawActive
        record["wall_seconds"] = wall
        record["user_cpu_seconds"] = user
        record["system_cpu_seconds"] = system
        record["cpu_seconds"] = user + system
        record["cpu_percent_of_one_core"] = (user + system) / wall * 100
        record["process_lifetime_peak_rss_bytes"] = end.lifetimePeakRSS
        record["cached_eligible_paths"] = cachedPaths
        let finalPaths = eligiblePaths(snapshot)
        record["final_eligible_paths"] = finalPaths
        record["new_eligible_paths"] = finalPaths.filter { !cachedPaths.contains($0) }
        record["first_active_snapshot_ms"] = firstActiveMS
        record["first_revalidated_finding_engine_ms"] = snapshot.stats.firstFindingMs
        record["first_revalidated_finding_observed_ms"] = firstFreshFindingObservedMS
        record["first_new_path_observed_ms"] = firstNewPathObservedMS
        record["snapshot_callbacks"] = snapshotCallbacks
        record["maximum_snapshot_delivery_gap_ms"] = largestSnapshotGapMS
        record["stats"] = encoded(snapshot.stats)
        record["setup_stats"] = setupSnapshot.map { encoded($0.stats) }
        record["method"] = "Warm native Find space rescan via ordinary AppModel.refresh. Process CPU from getrusage; wall time from the monotonic clock. Starts before request scheduling and ends at final MainActor snapshot delivery, before the final reload presentation assignment and rendering. Includes ordinary progressive UI work and bridge delivery. RSS is the process lifetime peak, including untimed setup."
        record["observer_timing_limits"] = "The nominal 150 ms poll interval is not an upper bound on callback latency: main-actor scheduling, bridge queuing and decoding add delay. Observed first-finding time includes that delay. Engine first-finding time starts inside Rust traversal. Snapshot delivery gaps do not measure main-thread responsiveness or compositor presentation."
        dispose()
        if !write(record, name: "final.json") { fputs("Native scan benchmark could not write its result.\n", stderr) }
    }

    func invalidate(_ reason: String) {
        guard phase != .finished else { return }
        let failedPhase = String(describing: phase)
        dispose()
        model.client?.cancel() // Only the prevalidated disposable library exists.
        var record = state(model.snapshot)
        record["phase"] = "failed"
        record["failed_phase"] = failedPhase
        record["valid"] = false
        record["reason"] = reason
        _ = write(record, name: "final.json")
    }

    private func dispose() {
        phase = .finished
        observations.removeAll()
        timeout?.cancel()
        timeout = nil
        startSignal?.cancel()
        startSignal = nil
    }

    private func state(_ snapshot: EngineSnapshot) -> [String: Any] {
        let window = windowState()
        return ["scan_protocol": 1, "pid": getpid(), "destination": model.destination.rawValue,
                "visible": window.visible, "occluded": window.occluded, "panel_visible": model.panelVisible,
                "scanning": snapshot.scanning, "cleaning": snapshot.cleaning,
                "root_path": rootPath, "collected_coins": snapshot.wallet.collectedCoins,
                "pending_coins": snapshot.wallet.pendingCoins,
                "reduce_motion": model.reduceMotion,
                "effective_reduce_motion": model.reduceMotion || NSWorkspace.shared.accessibilityDisplayShouldReduceMotion]
    }

    private func encoded<T: Encodable>(_ value: T) -> Any {
        guard let data = try? JSONEncoder().encode(value),
              let object = try? JSONSerialization.jsonObject(with: data) else { return NSNull() }
        return object
    }

    private func write(_ record: [String: Any], name: String) -> Bool {
        guard let data = try? JSONSerialization.data(withJSONObject: record, options: [.prettyPrinted]), data.count <= 65_536 else { return false }
        return (try? data.write(to: output.deletingLastPathComponent().appendingPathComponent(name), options: .atomic)) != nil
    }
}
