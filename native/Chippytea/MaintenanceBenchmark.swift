import AppKit
import Combine
import Darwin
import Foundation

/// Opt-in measurement of the ordinary watcher, bridge, polling and visible UI.
/// The runner writes only its disposable manifest, in a different process.
@MainActor final class NativeMaintenanceBenchmark {
    private enum Phase { case setup, ready, measuring, verifying, finished }
    private struct Usage {
        let nanoseconds: UInt64
        let user: Double
        let system: Double
        let lifetimePeakRSS: Int64

        static func read() -> Usage? {
            var value = rusage()
            var clock = timespec()
            guard getrusage(RUSAGE_SELF, &value) == 0,
                  clock_gettime(CLOCK_MONOTONIC_RAW, &clock) == 0 else { return nil }
            return Usage(nanoseconds: UInt64(clock.tv_sec) * 1_000_000_000 + UInt64(clock.tv_nsec),
                         user: Double(value.ru_utime.tv_sec) + Double(value.ru_utime.tv_usec) / 1_000_000,
                         system: Double(value.ru_stime.tv_sec) + Double(value.ru_stime.tv_usec) / 1_000_000,
                         lifetimePeakRSS: Int64(value.ru_maxrss))
        }
    }

    private let model: AppModel
    private let rootPath: String
    private let output: URL
    private let windowState: () -> (visible: Bool, occluded: Bool)
    private let reduceMotion: Bool
    private let systemReduceMotion: Bool
    private var phase = Phase.setup
    private var observations: [AnyCancellable] = []
    private var startSignal: DispatchSourceSignal?
    private var work: Task<Void, Never>?
    private var timeout: Task<Void, Never>?
    private var baseline: EngineSnapshot?
    private var baselinePresentation: DiscoveryPresentation?
    private var startRequests: UInt64 = 0
    private var rawPublications = 0
    private var presentationPublications = 0
    private var uiInvalidations = 0
    private var lastMeasuredScanning = false
    private var scanningRisingEdges = 0
    private var scanningFallingEdges = 0

    init(model: AppModel, rootPath: String, output: URL,
         windowState: @escaping () -> (visible: Bool, occluded: Bool)) {
        self.model = model
        self.rootPath = rootPath
        self.output = output
        self.windowState = windowState
        reduceMotion = model.reduceMotion
        systemReduceMotion = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
    }

    func prepare() {
        guard validPresentation(), !model.busy, !model.snapshot.scanning, !model.snapshot.cleaning,
              !model.discoveryPresentation.isRequestPending, model.snapshot.roots.count == 1,
              model.snapshot.roots[0].path == rootPath, model.snapshot.roots[0].kind == "folder",
              model.snapshot.wallet.pendingCoins == 0, model.snapshot.history.isEmpty else {
            invalidate("invalid_setup_state"); return
        }
        observations = [
            model.snapshots.dropFirst().sink { [weak self] in self?.receive($0) },
            model.$discoveryPresentation.dropFirst().sink { [weak self] value in
                guard let self, self.phase != .setup, self.phase != .finished else { return }
                if self.phase == .measuring { self.presentationPublications += 1 }
                if value != self.baselinePresentation { self.invalidate("foreground_presentation_changed") }
            },
            model.objectWillChange.sink { [weak self] in
                guard let self, self.phase == .measuring else { return }
                self.uiInvalidations += 1
            },
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
        timeout = Task { [weak self] in
            do { try await Task.sleep(for: .seconds(60)) } catch { return }
            self?.invalidate("deadline_exceeded")
        }
        model.refresh() // Untimed full pass; no Scan action occurs in the measured interval.
        work = Task { [weak self] in
            guard let self else { return }
            var stableSince = ProcessInfo.processInfo.systemUptime
            var requests = self.model.snapshotRequestID
            while self.phase == .setup {
                do { try await Task.sleep(for: .milliseconds(50)) } catch { return }
                if !self.settled(self.model.snapshot) || self.model.discoveryPresentation.isRequestPending
                    || self.model.snapshotRequestID != requests {
                    requests = self.model.snapshotRequestID
                    stableSince = ProcessInfo.processInfo.systemUptime
                } else if ProcessInfo.processInfo.systemUptime - stableSince >= 1.2 {
                    self.publishReady()
                    return
                }
            }
        }
    }

    private func validPresentation() -> Bool {
        let window = windowState()
        return window.visible && !window.occluded && model.panelVisible && model.destination == .discover
            && !model.showReview && !model.showDiskAccess && model.systemDialogDepth == 0
            && model.collection == nil && model.errorMessage == nil
    }

    private func settled(_ snapshot: EngineSnapshot) -> Bool {
        !snapshot.scanning && !snapshot.cleaning && snapshot.error == nil && snapshot.stats.complete
            && !snapshot.stats.cancelled && snapshot.stats.errors == 0
            && snapshot.foregroundScan?.active == false && snapshot.foregroundScan?.stats.complete == true
            && snapshot.foregroundScan?.stats.cancelled == false && snapshot.foregroundScan?.stats.errors == 0
    }

    private func preservesProof(_ snapshot: EngineSnapshot) -> Bool {
        guard let baseline else { return false }
        return snapshot.roots == baseline.roots && snapshot.candidates == baseline.candidates
            && snapshot.wallet == baseline.wallet && snapshot.history == baseline.history
            && snapshot.keptPaths == baseline.keptPaths && snapshot.foregroundScan == baseline.foregroundScan
            && !snapshot.cleaning && snapshot.error == nil && !snapshot.stats.cancelled && snapshot.stats.errors == 0
    }

    private func receive(_ snapshot: EngineSnapshot) {
        guard phase != .finished else { return }
        guard validPresentation(), !snapshot.cleaning, snapshot.error == nil,
              !snapshot.stats.cancelled, snapshot.stats.errors == 0 else {
            invalidate("invalid_snapshot"); return
        }
        guard phase != .setup else { return }
        guard preservesProof(snapshot) else { invalidate("candidate_ledger_or_foreground_changed"); return }
        if phase == .ready && snapshot != baseline { invalidate("work_before_start") }
        if phase == .measuring {
            rawPublications += 1
            // Follow observed publications, independently of the model's
            // pre-assignment timing. These are not worker or scope counts.
            if snapshot.scanning != lastMeasuredScanning {
                if snapshot.scanning { scanningRisingEdges += 1 }
                else { scanningFallingEdges += 1 }
                lastMeasuredScanning = snapshot.scanning
            }
        }
        // Any ordinary publication after the fixed endpoint is late work. A
        // fresh comparison below reads the engine without applying its result.
        if phase == .verifying { invalidate("snapshot_changed_after_endpoint") }
    }

    private func publishReady() {
        guard phase == .setup, validPresentation(), settled(model.snapshot), !model.busy,
              !model.discoveryPresentation.isRequestPending, model.snapshot.candidates.count == 1,
              let candidate = model.snapshot.candidates.first, candidate.recommended, candidate.eligiblePermanent,
              candidate.path == rootPath + "/Positive/target", !candidate.fingerprint.isEmpty, !candidate.evidence.isEmpty else {
            invalidate("setup_did_not_preserve_positive_fixture"); return
        }
        baseline = model.snapshot
        baselinePresentation = model.discoveryPresentation
        phase = .ready
        signal(SIGUSR1, SIG_IGN)
        let source = DispatchSource.makeSignalSource(signal: SIGUSR1, queue: .main)
        source.setEventHandler { [weak self] in self?.startMeasurement() }
        startSignal = source
        source.resume()
        var record = state()
        record["phase"] = "ready"
        record["snapshot"] = encoded(model.snapshot)
        record["window_seconds"] = 8
        record["first_write_offset_ms"] = 250
        record["write_interval_ms"] = 400
        record["writes"] = 16
        if !write(record, name: "ready.json") { invalidate("ready_output_failed") }
    }

    private func startMeasurement() {
        guard phase == .ready, validPresentation(), !model.busy, model.snapshot == baseline,
              model.discoveryPresentation == baselinePresentation, let start = Usage.read() else {
            invalidate("invalid_start_state"); return
        }
        startRequests = model.snapshotRequestID
        lastMeasuredScanning = model.snapshot.scanning
        scanningRisingEdges = 0
        scanningFallingEdges = 0
        phase = .measuring
        var record = state()
        record["phase"] = "measuring"
        record["start_monotonic_raw_ns"] = start.nanoseconds
        guard write(record, name: "started.json") else { invalidate("start_output_failed"); return }
        work = Task { [weak self] in
            guard let self else { return }
            var now = timespec()
            guard clock_gettime(CLOCK_MONOTONIC_RAW, &now) == 0 else { self.invalidate("clock_failed"); return }
            let current = UInt64(now.tv_sec) * 1_000_000_000 + UInt64(now.tv_nsec)
            let deadline = start.nanoseconds + 8_000_000_000
            if current < deadline {
                // Request no timer tolerance; the actual endpoint still has to
                // satisfy the raw-clock budget below after actor scheduling.
                do {
                    try await Task.sleep(for: .nanoseconds(Int64(deadline - current)), tolerance: .zero)
                } catch { return }
            }
            await self.finish(start)
        }
    }

    private func finish(_ start: Usage) async {
        // Freeze CPU and the applied model before any proof reads or output.
        guard phase == .measuring, let end = Usage.read() else { invalidate("cpu_sample_failed"); return }
        let frozen = model.snapshot
        let requests = model.snapshotRequestID
        let presentation = model.discoveryPresentation
        let wall = Double(end.nanoseconds - start.nanoseconds) / 1_000_000_000
        phase = .verifying
        guard (8...8.05).contains(wall), validPresentation(), settled(frozen), preservesProof(frozen),
              presentation == baselinePresentation, !model.busy, let client = model.client else {
            let window = windowState()
            let diagnostics: [String: Any] = [
                "start_monotonic_raw_ns": start.nanoseconds,
                "end_monotonic_raw_ns": end.nanoseconds,
                "wall_seconds": wall,
                "user_cpu_seconds": end.user - start.user,
                "system_cpu_seconds": end.system - start.system,
                "cpu_seconds": end.user + end.system - start.user - start.system,
                "snapshot": encoded(frozen),
                "start_snapshot_request_id": startRequests,
                "end_snapshot_request_id": requests,
                "snapshot_read_attempts": requests - startRequests,
                "raw_snapshot_publications": rawPublications,
                "scanning_rising_edges": scanningRisingEdges,
                "scanning_falling_edges": scanningFallingEdges,
                "presentation_stats": encoded(presentation.stats),
                "request_pending": presentation.isRequestPending,
                "foreground_active": presentation.isForeground,
                "visible": window.visible,
                "occluded": window.occluded,
                "panel_visible": model.panelVisible,
                "guards": [
                    "window_within_budget": (8...8.05).contains(wall),
                    "valid_presentation": validPresentation(),
                    "settled_snapshot": settled(frozen),
                    "preserved_proof": preservesProof(frozen),
                    "same_presentation": presentation == baselinePresentation,
                    "not_busy": !model.busy,
                    "client_available": model.client != nil,
                ],
            ]
            invalidate("unsettled_or_late_endpoint", endpointDiagnostics: diagnostics); return
        }
        var record = state()
        record["phase"] = "endpoint"
        record["snapshot"] = encoded(frozen)
        record["start_monotonic_raw_ns"] = start.nanoseconds
        record["end_monotonic_raw_ns"] = end.nanoseconds
        record["wall_seconds"] = wall
        record["user_cpu_seconds"] = end.user - start.user
        record["system_cpu_seconds"] = end.system - start.system
        record["cpu_seconds"] = end.user + end.system - start.user - start.system
        record["process_lifetime_peak_rss_bytes"] = end.lifetimePeakRSS
        record["snapshot_read_attempts"] = requests - startRequests
        record["raw_snapshot_publications"] = rawPublications
        record["presentation_publications"] = presentationPublications
        record["model_ui_invalidations"] = uiInvalidations
        record["scanning_rising_edges"] = scanningRisingEdges
        record["scanning_falling_edges"] = scanningFallingEdges
        guard write(record, name: "endpoint.json") else { invalidate("endpoint_output_failed"); return }
        do {
            guard try await client.snapshot() == frozen else { invalidate("native_snapshot_stale_at_endpoint"); return }
            try await Task.sleep(for: .milliseconds(1200))
            guard phase == .verifying, validPresentation(), model.snapshot == frozen,
                  model.snapshotRequestID == requests, model.discoveryPresentation == presentation,
                  try await client.snapshot() == frozen else {
                invalidate("late_work_or_stale_snapshot_in_quiet_tail"); return
            }
            // Check again after the comparison's suspension, without a reload.
            guard phase == .verifying, model.snapshot == frozen, model.snapshotRequestID == requests,
                  model.discoveryPresentation == presentation else { invalidate("late_native_poll"); return }
        } catch { invalidate("endpoint_comparison_failed"); return }
        record["phase"] = "complete"
        record["valid"] = true
        record["fresh_snapshot_matches_frozen"] = true
        record["quiet_tail_seconds"] = 1.2
        record["quiet_tail_unchanged"] = true
        record["method"] = "Eight-second visible Find space window. Process getrusage includes ordinary FolderWatcher delivery, engine work, bridge polling, SwiftUI updates and constant benchmark observer/start-handshake overhead. Python writes the same manifest inode at +250 ms then every 400 ms, opening and closing each pulse outside app CPU. Endpoint CPU precedes output, two unapplied fresh-engine comparisons and the 1.2-second quiet tail. No reload, Scan or queue repair at the endpoint. RSS is the process lifetime peak, including setup."
        record["limits"] = "Model invalidations and snapshot deliveries are not compositor frames, main-thread latency or exact FSEvents callback counts. Scanning edges are observed snapshot transitions, not worker or scope counts. This measures finite background maintenance, not a full scan or idle CPU."
        dispose()
        if !write(record, name: "final.json") { fputs("Maintenance benchmark could not write its result.\n", stderr) }
    }

    func invalidate(_ reason: String, endpointDiagnostics: [String: Any]? = nil) {
        guard phase != .finished else { return }
        let failedPhase = String(describing: phase)
        dispose()
        model.client?.cancel() // The process can only open the guarded disposable library.
        var record = state()
        record["phase"] = "failed"
        record["failed_phase"] = failedPhase
        record["valid"] = false
        record["reason"] = reason
        if let endpointDiagnostics { record["endpoint_diagnostics"] = endpointDiagnostics }
        _ = write(record, name: "final.json")
    }

    private func dispose() {
        phase = .finished
        observations.removeAll()
        timeout?.cancel()
        timeout = nil
        work?.cancel()
        work = nil
        startSignal?.cancel()
        startSignal = nil
    }

    private func state() -> [String: Any] {
        let window = windowState()
        return ["maintenance_protocol": 2, "pid": getpid(), "destination": model.destination.rawValue,
                "visible": window.visible, "occluded": window.occluded, "panel_visible": model.panelVisible,
                "root_path": rootPath, "reduce_motion": model.reduceMotion,
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
