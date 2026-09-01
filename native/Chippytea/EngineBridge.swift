import AppKit
import ChippyteaCore
import Combine
import CoreServices
import Darwin
import QuickLookUI
import SwiftUI

enum EngineError: LocalizedError {
    case message(String)
    var errorDescription: String? { if case let .message(message) = self { return message }; return nil }
}

/// Owned by EngineClient's serial queue. Reuse requires the complete response
/// from a fresh engine read; errors never become a cached successful result.
struct SnapshotResponseDecoder {
    static let maximumResponseBytes = 1024 * 1024
    private var cached: (bytes: Data, snapshot: EngineSnapshot)?

    mutating func decode(_ data: Data) throws -> EngineSnapshot {
        // This bounds retained serialized bytes, not decoded object memory.
        // Valid larger responses still decode normally without being retained.
        if data.count > Self.maximumResponseBytes {
            clear()
            return try EngineClient.decodeSnapshotResponse(data)
        }
        if let cached, cached.bytes == data { return cached.snapshot }
        do {
            let snapshot = try EngineClient.decodeSnapshotResponse(data)
            cached = (data, snapshot)
            return snapshot
        } catch {
            clear()
            throw error
        }
    }

    mutating func clear() { cached = nil }
}

final class EngineClient: @unchecked Sendable {
    private let handle: UnsafeMutableRawPointer
    private let queue = DispatchQueue(label: "app.chippytea.engine", qos: .utility)
    private let progressQueue = DispatchQueue(label: "app.chippytea.cleanup-progress", qos: .utility)
    private var snapshotDecoder = SnapshotResponseDecoder()
    init(database: URL) throws {
        guard let handle = database.path.withCString({ ct_open($0, nativeTrash) }) else { throw EngineError.message("The local library could not be opened. Check free space and whether another chippytea process has it open.") }
        self.handle = handle
    }
    deinit { ct_close(handle) }
    func cancel() { ct_cancel(handle) }
    func request(_ request: [String: Any]) async throws -> Data {
        try await withCheckedThrowingContinuation { continuation in
            queue.async { do { continuation.resume(returning: try self.requestSync(request)) } catch { continuation.resume(throwing: error) } }
        }
    }
    /// Scan progress uses this frequently. Decode changed responses on the
    /// engine queue before returning to the main actor.
    func snapshot() async throws -> EngineSnapshot {
        try await withCheckedThrowingContinuation { continuation in
            queue.async {
                do {
                    guard let result = "{\"action\":\"snapshot\"}".withCString({ ct_request(self.handle, $0) }) else {
                        throw EngineError.message("The engine did not return a result.")
                    }
                    defer { ct_free_string(result) }
                    let data = Data(bytes: result, count: strlen(result))
                    continuation.resume(returning: try self.snapshotDecoder.decode(data))
                } catch {
                    self.snapshotDecoder.clear()
                    continuation.resume(throwing: error)
                }
            }
        }
    }
    static func decodeSnapshotResponse(_ data: Data) throws -> EngineSnapshot {
        try decode(SnapshotResponse.self, data).snapshot
    }
    private struct SnapshotResponse: Decodable {
        let snapshot: EngineSnapshot
        private enum CodingKeys: String, CodingKey { case ok, data, error }

        init(from decoder: Decoder) throws {
            let values = try decoder.container(keyedBy: CodingKeys.self)
            guard (try? values.decode(Bool.self, forKey: .ok)) == true else {
                throw EngineError.message((try? values.decode(String.self, forKey: .error))
                    ?? "The operation could not be completed.")
            }
            // Missing, null or another endpoint's payload must never become an
            // authoritative empty snapshot that clears grants or cleanup masks.
            snapshot = try values.decode(EngineSnapshot.self, forKey: .data)
        }
    }
    /// This endpoint only reads the engine's independent progress mutex. It must
    /// not wait behind execute, which owns the ordinary request queue until done.
    func cleanupProgress() async throws -> CleanupProgress? {
        try await withCheckedThrowingContinuation { continuation in
            progressQueue.async {
                do {
                    let data = try self.requestSync(["action": "cleanup_progress"])
                    continuation.resume(returning: try Self.decode(CleanupProgress?.self, data))
                } catch { continuation.resume(throwing: error) }
            }
        }
    }
    func requestSync(_ request: [String: Any]) throws -> Data {
        let data = try JSONSerialization.data(withJSONObject: request)
        let text = String(decoding: data, as: UTF8.self)
        guard let result = text.withCString({ ct_request(handle, $0) }) else { throw EngineError.message("The engine did not return a result.") }
        defer { ct_free_string(result) }
        let decoded = Data(String(cString: result).utf8)
        guard let value = try JSONSerialization.jsonObject(with: decoded) as? [String: Any] else { throw EngineError.message("Invalid engine response") }
        guard value["ok"] as? Bool == true else { throw EngineError.message(value["error"] as? String ?? "The operation could not be completed.") }
        return try JSONSerialization.data(withJSONObject: value["data"] ?? [:], options: [.fragmentsAllowed])
    }
    static func decode<T: Decodable>(_ type: T.Type, _ data: Data) throws -> T {
        let decoder = JSONDecoder(); decoder.keyDecodingStrategy = .convertFromSnakeCase
        return try decoder.decode(type, from: data)
    }
}

private enum DiskAccessSetupRecord: String, Codable, Sendable {
    case waiting, completed, dismissed
}

private struct DirtyAcknowledgment: Decodable {
    let ignored: Bool?
}

/// Presentation follows the user's discovery session, not each short index
/// refresh. Background work keeps settled content and statistics on screen.
struct DiscoveryPresentation: Equatable {
    private(set) var isForeground = false
    private(set) var isRequestPending = false
    private(set) var stats = ScanStats()
    var settledCoverageNeedsAttention: Bool { settledCoverageIncomplete && !isForeground }
    private var settledCoverageIncomplete = false
    private var hasSettledScan = false

    func statusLine(rootCount: Int) -> String {
        guard rootCount > 0 else { return "No folders authorised" }
        // The pending handoff still holds the previous scan's statistics.
        guard !isRequestPending else { return "Starting scan…" }
        guard isForeground || hasSettledScan else {
            return "No saved scan · \(rootCount) \(rootCount == 1 ? "folder" : "folders")"
        }
        let entries = "\(stats.entries.formatted()) entries"
        if !isForeground {
            let outcome = stats.cancelled ? "cancelled" : stats.complete ? "complete" : "partial"
            let errors = stats.errors > 0 ? " · \(stats.errors.formatted()) inaccessible" : ""
            return "Last scan \(outcome) · \(entries)\(errors)"
        }
        return "\(entries) checked · \(rootCount) \(rootCount == 1 ? "folder" : "folders") · \(stats.excludedArtifacts.formatted()) artifacts skipped · \(stats.errors.formatted()) inaccessible"
    }

    var scanControlTitle: String { isForeground ? "Cancel" : "Pause" }
    var scanControlAccessibilityLabel: String { isForeground ? "Cancel scan" : "Pause background updates" }
    var scanControlHelp: String {
        if isForeground { return "Cancel this scan between filesystem operations. Completed findings stay available." }
        return hasSettledScan
            ? "Pause automatic updates between filesystem operations. Your last scan and findings stay available. Use Refresh to scan and resume updates."
            : "Pause automatic updates between filesystem operations. Findings stay available. Use Refresh to scan and resume updates."
    }

    mutating func beginRequestedScan() {
        isRequestPending = true
        isForeground = true
    }

    mutating func finishRequestedScan(_ snapshot: EngineSnapshot) {
        isRequestPending = false
        update(snapshot)
    }

    mutating func failRequestedScan() {
        isRequestPending = false
        isForeground = false
    }

    mutating func update(_ snapshot: EngineSnapshot) {
        // Reads queued before Scan was accepted may still describe the
        // previous request. The post-request reload settles this handoff.
        guard !isRequestPending else { return }
        if let foreground = snapshot.foregroundScan {
            stats = foreground.stats
            isForeground = foreground.active
            hasSettledScan = hasSettledScan || !foreground.active
        } else {
            // Grant changes invalidate the saved result. Raw statistics may
            // include removed roots or one resumed scope, never a full scan.
            stats = ScanStats()
            if !snapshot.roots.isEmpty { stats.message = "Use Refresh to scan your authorised folders." }
            isForeground = false
            hasSettledScan = false
        }
        if snapshot.roots.isEmpty {
            settledCoverageIncomplete = false
        } else if !snapshot.scanning && !snapshot.cleaning && !isForeground {
            let meaningful = snapshot.stats.entries > 0 || snapshot.stats.errors > 0
                || snapshot.stats.cancelled || snapshot.error != nil
                || stats.entries > 0 || stats.complete || stats.cancelled || stats.errors > 0
            settledCoverageIncomplete = meaningful && !snapshot.stats.complete
        }
    }
}

@MainActor final class AppModel: ObservableObject {
    private let snapshotSubject = CurrentValueSubject<EngineSnapshot, Never>(EngineSnapshot())
    /// Raw observers retain @Published's current value and pre-assignment timing.
    /// SwiftUI only receives changes to fields used by presentation or controls.
    var snapshots: AnyPublisher<EngineSnapshot, Never> { snapshotSubject.eraseToAnyPublisher() }
    var snapshot = EngineSnapshot() {
        willSet {
            if !snapshot.hasSamePresentation(as: newValue) { objectWillChange.send() }
            snapshotSubject.send(newValue)
        }
    }
    @Published private(set) var discoveryPresentation = DiscoveryPresentation()
    @Published private(set) var cleanupProgress: CleanupProgress?
    @Published private(set) var cleanupPreview: CleanupPreview?
    /// Only successful ledger collections raise this floor. It also covers a
    /// collection response arriving before its refreshed wallet snapshot.
    @Published private(set) var confirmedCollectedCoins: UInt64 = 0
    @Published private(set) var cleanupCandidateIDs = Set<String>()
    @Published private(set) var cleanupCancellationRequested = false
    @Published private(set) var queuedCleanupCount = 0
    @Published private(set) var pendingCleanupEstimate: CleanupEstimate?
    @Published var selection = Set<String>()
    @Published var destination = Destination.coins
    @Published var showReview = false
    @Published var showDiskAccess = false
    @Published var diskAccessPhase = DiskAccessPhase.intro
    @Published var diskAccessMessage: String?
    /// The setup page on screen. Phases record intent; this is presentation only.
    @Published var diskAccessStep = DiskAccessStep.permission
    /// Screenshot staging freezes the setup sketches at one moment of their loop.
    var diskAccessSceneTime: Double?
    @Published private(set) var diskAccessNeedsReplacement = false
    @Published var reviewItems: [Candidate] = []
    @Published var busy = false
    @Published var errorMessage: String?
    @Published var collection: CollectionBurst?
    /// The home suggestion expanded into its inline single-item review, if any.
    @Published var expandedSuggestion: String?
    /// The Activity receipt opened into its accordion details, if any.
    @Published var expandedReceipt: String?
    @Published var soundEnabled = UserDefaults.standard.object(forKey: "soundEnabled") as? Bool ?? true {
        didSet { UserDefaults.standard.set(soundEnabled, forKey: "soundEnabled"); if !soundEnabled { audio.stop() } }
    }
    @Published var reduceMotion = UserDefaults.standard.bool(forKey: "reduceMotion") {
        didSet { UserDefaults.standard.set(reduceMotion, forKey: "reduceMotion") }
    }
    @Published var confirmBeforeDeleting = UserDefaults.standard.object(forKey: "confirmBeforeDeleting") as? Bool ?? true {
        didSet { UserDefaults.standard.set(confirmBeforeDeleting, forKey: "confirmBeforeDeleting") }
    }
    /// A transient estimate may lead the visible counter, but never the wallet.
    var displayedCoinBalance: UInt64 {
        cleanupPreview?.targetCoins ?? max(confirmedCollectedCoins, snapshot.wallet.collectedCoins)
    }
    var confirmedEarnedCoins: UInt64 {
        let earned = snapshot.wallet.collectedCoins.addingReportingOverflow(snapshot.wallet.pendingCoins)
        return max(confirmedCollectedCoins, earned.overflow ? .max : earned.partialValue)
    }
    var pendingCleanupCoinEstimate: UInt64 { pendingCleanupEstimate?.pendingCoins ?? 0 }
    var pendingCleanupIncludesPermanent: Bool { pendingCleanupEstimate?.hasPermanentCleanup ?? false }
    /// The engine's `scanning` flag flips on every debounced background worker,
    /// which used to strobe the Pause control on Find space. This presentation
    /// signal waits out sub-second blips in both directions; a user-requested
    /// scan still shows its Cancel control immediately through the foreground
    /// presentation, never through this debounce.
    @Published private(set) var backgroundActivityVisible = false
    /// A live capacity sample for the home page gauge; fed by the app delegate's
    /// monitor and absent in benchmark modes, where the gauge simply hides.
    @Published private(set) var storageStatus: StorageStatus?
    /// Receipts older than the snapshot's bounded newest page, loaded on demand.
    @Published private(set) var olderReceipts: [Receipt] = []
    @Published private(set) var loadingOlderReceipts = false
    /// Total receipts in the ledger, when a history page has reported it.
    @Published private(set) var historyTotal: UInt64?
    private var historyNextBefore: Int64?
    private var historyPagingExhausted = false
    private var backgroundActivityTask: Task<Void, Never>?
    private var backgroundActivityObservation: AnyCancellable?
    /// Panel-local x of the status-item icon, so the washi tape stays under it when edge-clamped.
    @Published var anchorX: CGFloat = TeaTheme.panelWidth / 2
    /// Mirrors `visible` for the UI, so boiling ink animations never run behind a hidden panel.
    @Published var panelVisible = false
    /// The panel's live height. The SwiftUI root pins itself to this so a tall list can
    /// never grow the hosting view — and with it the window — beyond the tray panel.
    @Published var panelHeight: CGFloat = TeaTheme.panelHeight
    /// Bumped every time the panel is shown; drives the entrance animation only.
    @Published var presentation = 0
    /// Greater than zero while a system window (folder picker, Quick Look) owns the keyboard,
    /// so losing key focus to it must not dismiss the panel.
    private(set) var systemDialogDepth = 0
    var onSystemDialogDepthChange: ((Int) -> Void)?
    var onWalletChange: ((UInt64) -> Void)? {
        didSet { lastNotifiedPendingCoins = nil }
    }
    var visible = false
    private(set) var client: EngineClient?
    private let directory: URL
    private let scanHome: URL
    private let readSnapshot: (EngineClient) async throws -> EngineSnapshot
    private var bookmarks: [String: Data] = [:]
    private var accesses: [URL] = []
    private var watcher: FolderWatcher?
    private var pollTask: Task<Void, Never>?
    private(set) var pollDemandID: UInt64 = 0
    private(set) var snapshotRequestID: UInt64 = 0
    private var appliedSnapshotID: UInt64 = 0
    private var acceptedScanSnapshotBoundary: UInt64?
    private var lastNotifiedPendingCoins: UInt64?
    private var lastObservedEngineError: String?
    private var lastObservedSnapshotReadError: String?
    private var snapshotReadObservationID: UInt64 = 0
    private var collecting = false
    private var collectionRequested = false
    private var requestedCollectionCelebration = false
    private var suppressNextCollectionCelebration = false
    private var collectionPresentationGeneration: UInt64 = 0
    private var cleanupExecuting = false
    private var activeCleanup: CleanupRequest?
    // A bounded FIFO keeps queued consent in memory. Closing the panel continues
    // work; restarting the app never replays an unsent destructive request.
    private var queuedCleanups: [CleanupRequest] = []
    private let maximumQueuedCleanups = 100
    private var consumedCleanupPreviewID: UUID?
    private var presentedCleanupPreviewID: UUID?
    private var cleanupProgressTask: Task<Void, Never>?
    /// Armed while a cleanup is running, so the falling edge of `snapshot.cleaning` fires once.
    private var cleanupRunning = false
    private var eventTask: Task<Void, Never>?
    private var watcherRevision = 0
    private var eventReceiptFailed = false
    private var eventSubmissions = 0
    private var cursor: UInt64 = 0
    private var pendingCursor: UInt64 = 0
    private let audio = CoinAudio()
    private let preview = QuickLookController()
    private var quickLookWatch: Task<Void, Never>?
    private var diskAccessTask: Task<Void, Never>?
    private var diskAccessPersistence: Task<Void, Error>?
    private var restoringAccess = false
    private var diskAccessConfigured = false
    private var diskAccessAppIdentity: DiskAccessAppIdentity?
    private var diskAccessSystemDialog = false
    private var diskAccessRevision = 0

    init(directory: URL? = nil, scanHome: URL? = nil,
         readSnapshot: @escaping (EngineClient) async throws -> EngineSnapshot = { try await $0.snapshot() }) {
        let testDirectory = ProcessInfo.processInfo.environment["CHIPPYTEA_DATA_DIR"].map { URL(fileURLWithPath: $0, isDirectory: true) }
        self.directory = directory ?? testDirectory ?? FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0].appendingPathComponent("Chippytea", isDirectory: true)
        self.scanHome = scanHome ?? FileManager.default.homeDirectoryForCurrentUser
        self.readSnapshot = readSnapshot
        backgroundActivityObservation = snapshotSubject.map(\.scanning).removeDuplicates()
            .sink { [weak self] scanning in self?.scanningActivityChanged(scanning) }
    }

    /// Debounce the raw scanning flag into a calm presentation signal: appear
    /// only after activity persists, stay through short idle gaps between
    /// event-driven workers. State reads happen after the sleep so a train of
    /// flips settles on whatever the engine is actually doing.
    private func scanningActivityChanged(_ scanning: Bool) {
        backgroundActivityTask?.cancel()
        guard scanning != backgroundActivityVisible else { backgroundActivityTask = nil; return }
        backgroundActivityTask = Task { @MainActor [weak self] in
            try? await Task.sleep(for: .milliseconds(scanning ? 400 : 1200))
            guard !Task.isCancelled, let self, self.snapshot.scanning == scanning else { return }
            self.backgroundActivityVisible = scanning
        }
    }

    /// The delegate's monitor feeds capacity samples; nil hides the gauge.
    func updateStorageStatus(_ value: StorageStatus?) {
        if storageStatus != value { storageStatus = value }
    }

    /// What "scanning" means to a control's enabled state: the foreground scan,
    /// or settled background activity. Action methods keep their own checks on
    /// the raw engine flags; views use this so short background workers cannot
    /// grey-flash every control that defers to a scan.
    var scanActivityForUI: Bool { discoveryPresentation.isForeground || backgroundActivityVisible }

    var isCleaning: Bool { cleanupExecuting || snapshot.cleaning }
    var hasCleanupWork: Bool { activeCleanup != nil || !queuedCleanups.isEmpty || isCleaning }
    var canEnqueueCleanup: Bool {
        guard client != nil, queuedCleanups.count < maximumQueuedCleanups else { return false }
        if activeCleanup != nil { return !cleanupCancellationRequested }
        return !busy && !isCleaning
    }
    var displayedCandidates: [Candidate] {
        guard !cleanupCandidateIDs.isEmpty else { return snapshot.candidates }
        return snapshot.candidates.filter { !cleanupCandidateIDs.contains($0.id) }
    }

    func start() async {
        guard client == nil, !busy else { return }
        // Restore access before accepting grant changes or starting filesystem
        // work. Back remains available and invalidates this restoration.
        busy = true
        restoringAccess = true
        defer { busy = false; restoringAccess = false }
        let accessRevisionAtStart = diskAccessRevision
        do {
            let directory = directory
            client = try await Task.detached(priority: .utility) { try EngineClient(database: directory.appendingPathComponent("library.sqlite")) }.value
            let accessRecord = await Task.detached(priority: .utility) {
                guard let data = try? Data(contentsOf: directory.appendingPathComponent("disk-access.json")) else { return DiskAccessSetupRecord?.none }
                return try? JSONDecoder().decode(DiskAccessSetupRecord.self, from: data)
            }.value
            let identities = await Task.detached(priority: .utility) {
                let current = DiskAccessAppIdentity.current()
                let saved = (try? Data(contentsOf: directory.appendingPathComponent("disk-access-app.json")))
                    .flatMap { try? JSONDecoder().decode(DiskAccessAppIdentity.self, from: $0) }
                return (current, saved)
            }.value
            diskAccessAppIdentity = identities.0
            let completedAccessMatches = accessRecord == .completed && identities.0 != nil && identities.0 == identities.1
            if diskAccessRevision == accessRevisionAtStart {
                diskAccessNeedsReplacement = identities.1 != nil && identities.0 != identities.1
            }
            if accessRecord == .waiting && diskAccessRevision == accessRevisionAtStart {
                showDiskAccess = true
                diskAccessPhase = .waiting
                diskAccessStep = .enable
                diskAccessMessage = nil
            }
            if let data = try? Data(contentsOf: directory.appendingPathComponent("bookmarks.json")) { bookmarks = (try? JSONDecoder().decode([String: Data].self, from: data)) ?? [:] }
            await reload()
            for root in snapshot.roots {
                guard let data = bookmarks[root.path] else {
                    _ = try await client?.request(["action": "forget", "id": root.id]); continue
                }
                do {
                    var stale = false
                    var scoped = true
                    var url: URL
                    do {
                        url = try URL(resolvingBookmarkData: data, options: [.withSecurityScope, .withoutUI, .withoutMounting], relativeTo: nil, bookmarkDataIsStale: &stale)
                    } catch {
                        // A folder authorized without a picker may only have a plain bookmark.
                        scoped = false
                        url = try URL(resolvingBookmarkData: data, options: [.withoutUI, .withoutMounting], relativeTo: nil, bookmarkDataIsStale: &stale)
                    }
                    guard !stale, url.path == root.path else { throw EngineError.message("Choose \(root.name) again to renew folder access.") }
                    if scoped { _ = url.startAccessingSecurityScopedResource(); accesses.append(url) }
                } catch { _ = try await client?.request(["action": "forget", "id": root.id]); errorMessage = error.localizedDescription }
            }
            await reload()
            if completedAccessMatches && diskAccessRevision == accessRevisionAtStart {
                let home = scanHome
                let blocked = await Task.detached(priority: .utility) {
                    HomeFolderAccess.blockedLocations(in: home)
                }.value
                if diskAccessRevision == accessRevisionAtStart {
                    if blocked.isEmpty {
                        // Older builds stored Home as an unrestricted folder.
                        // Narrow the same physical grant atomically before any
                        // watcher or pending scan can use its old media policy.
                        if snapshot.roots.contains(where: { $0.path == homePath && $0.kind == "folder" }) {
                            _ = try await client?.request(["action": "authorize", "path": homePath, "kind": "home", "replace_contained": true])
                            await reload()
                            guard snapshot.roots.contains(where: { $0.path == homePath && $0.kind == "home" }) else {
                                throw EngineError.message("Could not restore Home’s scan policy. Open scan setup to try again.")
                            }
                        }
                        if diskAccessRevision == accessRevisionAtStart {
                            diskAccessConfigured = true
                        }
                    } else {
                        showDiskAccess = true
                        diskAccessPhase = .waiting
                        diskAccessStep = .enable
                        diskAccessMessage = HomeFolderAccess.message(for: blocked)
                    }
                }
            }
            if homeAuthorized && !diskAccessConfigured && diskAccessRevision == accessRevisionAtStart {
                showDiskAccess = true
                if diskAccessMessage == nil {
                    diskAccessPhase = accessRecord == .waiting ? .waiting : .intro
                    diskAccessStep = accessRecord == .waiting ? .enable : .permission
                }
            }
            if let data = try await client?.request(["action": "cursor"]), let value = try JSONSerialization.jsonObject(with: data) as? [String: UInt64] { cursor = value["cursor"] ?? 0 }
            restoringAccess = false
            var needsInitialScan = false
            if accessRecord != .waiting || !showDiskAccess {
                restartWatcher()
                if diskAccessRevision == accessRevisionAtStart && !snapshot.roots.isEmpty && !(homeAuthorized && !diskAccessConfigured) {
                    if cursor == 0 { needsInitialScan = true }
                    else { _ = try await client?.request(["action": "resume"]); poll() }
                }
            }
            if visible { collect() }
            // No suspension after releasing startup's reservation: another
            // operation must not acquire busy before our defer clears it.
            busy = false
            if needsInitialScan { refresh() }
        } catch { errorMessage = error.localizedDescription }
    }

    func reload() async {
        guard !cleanupExecuting, let client else { return }
        snapshotRequestID &+= 1
        let requestID = snapshotRequestID
        do {
            let current = try await readSnapshot(client)
            // Concurrent callers share the engine queue, but their MainActor
            // continuations may resume in a different order.
            guard !cleanupExecuting, requestID >= appliedSnapshotID else { return }
            appliedSnapshotID = requestID
            if requestID >= snapshotReadObservationID {
                snapshotReadObservationID = requestID
                lastObservedSnapshotReadError = nil
            }
            if current != snapshot { snapshot = current }
            var presentation = discoveryPresentation
            if let boundary = acceptedScanSnapshotBoundary, requestID > boundary {
                // Only an applied read started after Scan was accepted can
                // finish its handoff. Failed reads leave it pending for poll.
                // A new request may fail with the same text before a nil-error
                // snapshot is observed. Older reads must not re-arm its warning.
                lastObservedEngineError = nil
                presentation.finishRequestedScan(current)
                acceptedScanSnapshotBoundary = nil
            } else {
                presentation.update(current)
            }
            if presentation != discoveryPresentation { discoveryPresentation = presentation }
            let retainedSelection = selection.intersection(current.candidates.lazy.filter { $0.blockedReason == nil }.map(\.id))
            if retainedSelection != selection { selection = retainedSelection }
            if let expanded = expandedSuggestion, !current.candidates.contains(where: { $0.id == expanded }) { expandedSuggestion = nil }
            if !presentation.isRequestPending && current.error != lastObservedEngineError {
                lastObservedEngineError = current.error
                if let error = current.error, errorMessage != error { errorMessage = error }
            }
            if lastNotifiedPendingCoins != current.wallet.pendingCoins {
                lastNotifiedPendingCoins = current.wallet.pendingCoins
                onWalletChange?(current.wallet.pendingCoins)
            }
            // Even an identical snapshot can settle optimistic cleanup state.
            // Equality only suppresses publications, never these outcome edges.
            watchCleanupEdge()
            startCollectionIfPossible()
        } catch {
            if !cleanupExecuting && requestID >= appliedSnapshotID && requestID >= snapshotReadObservationID {
                snapshotReadObservationID = requestID
                let message = error.localizedDescription
                if message != lastObservedSnapshotReadError {
                    lastObservedSnapshotReadError = message
                    if errorMessage != message { errorMessage = message }
                }
            }
        }
    }

    /// Reconcile pending feedback only with an authoritative post-cleanup snapshot.
    private func watchCleanupEdge() {
        if snapshot.cleaning { cleanupRunning = true; return }
        guard cleanupRunning else {
            retireFinishedCleanupPreview()
            return
        }
        cleanupRunning = false
        let hasImmediateCollection = cleanupPreview.map {
            $0.isPermanent && $0.estimatedCoins > 0
                && ($0.id == presentedCleanupPreviewID || !$0.presentationFinished)
        } ?? false
        suppressNextCollectionCelebration = suppressNextCollectionCelebration || hasImmediateCollection
        // Only this authoritative read releases the completed job's reservation.
        // Other queued candidates and a newer open review remain untouched.
        activeCleanup = nil
        refreshCleanupReservations()
        reconcileCleanupEstimate()
        if cleanupProgress != nil { cleanupProgress = nil }
        if !queuedCleanups.isEmpty {
            startNextCleanupIfPossible()
            return
        }
        // A fast cleanup must not erase its flight before SwiftUI can render it.
        busy = false
        cleanupCancellationRequested = false
        retireFinishedCleanupPreview()
        guard visible else { return }  // Closed panel: the badge and collect-on-open already cover it.
        if snapshot.wallet.pendingCoins > 0 {
            // Background completion must not pull someone away from another tab.
            if destination == .coins {
                collect(celebrate: !hasImmediateCollection)
            }
        }
    }

    /// Keep the resolved counter in place while pending earned coins move into
    /// the collected wallet. This avoids briefly falling back to the old balance.
    private func retireFinishedCleanupPreview() {
        guard let preview = cleanupPreview, preview.presentationFinished, let target = preview.resolvedTo,
              !visible || max(confirmedCollectedCoins, snapshot.wallet.collectedCoins) >= target else { return }
        cleanupPreview = nil
        consumedCleanupPreviewID = nil
        presentedCleanupPreviewID = nil
    }

    private func poll() {
        guard !cleanupExecuting else { return }
        pollDemandID &+= 1
        guard pollTask == nil else { return }
        pollTask = Task { [weak self] in
            guard let self else { return }
            repeat {
                let demand = self.pollDemandID
                await self.reload()
                if self.cleanupExecuting { break }
                // An accepted event can arrive after this read captured idle
                // state but before its MainActor continuation resumes. Its
                // demand must survive the existing-task guard above.
                if !self.snapshot.scanning && !self.snapshot.cleaning
                    && !self.discoveryPresentation.isRequestPending {
                    if demand == self.pollDemandID { break }
                    continue
                }
                try? await Task.sleep(for: .milliseconds(150))
            } while !Task.isCancelled
            self.pollTask = nil

        }
    }

    private func restartWatcher() {
        watcher = nil
        watcherRevision &+= 1
        eventReceiptFailed = false
        let revision = watcherRevision
        // One event can resume the engine's entire durable queue, including a
        // Home scope left by an older build. Keep all watching gated with Scan.
        guard !restoringAccess, !(homeAuthorized && !diskAccessConfigured) else { return }
        if cursor == 0 { pendingCursor = max(pendingCursor, FSEventsGetCurrentEventId()) }
        let roots = snapshot.roots.filter { diskAccessConfigured || $0.path != homePath }
        guard !roots.isEmpty else { return }
        watcher = FolderWatcher(paths: roots.map(\.path), since: cursor == 0 ? pendingCursor : cursor, excluding: [directory.path]) { [weak self] events, last, historyLost in
            Task { @MainActor [weak self] in
                guard let self else { return }
                let previous = self.eventTask
                self.eventSubmissions += 1
                self.eventTask = Task { @MainActor [weak self] in
                    await previous?.value
                    guard let self else { return }
                    defer { self.eventSubmissions -= 1 }
                    guard self.watcherRevision == revision, !self.restoringAccess,
                          !(self.homeAuthorized && !self.diskAccessConfigured), let client = self.client else { return }
                    var needsReload = false
                    do {
                        for root in self.snapshot.roots where self.diskAccessConfigured || root.path != self.homePath {
                            let affected = events.filter { $0.path == root.path || $0.path.hasPrefix(root.path + "/") }
                            if historyLost {
                                _ = try await client.request(["action": "dirty", "root_id": root.id])
                                needsReload = true
                            } else {
                                for start in stride(from: 0, to: affected.count, by: 512) {
                                    let batch = affected[start..<min(start + 512, affected.count)].map(\.request)
                                    let response = try await client.request(["action": "dirty", "root_id": root.id, "events": batch])
                                    let acknowledgment = try EngineClient.decode(DirtyAcknowledgment.self, response)
                                    needsReload = needsReload || acknowledgment.ignored != true
                                }
                            }
                        }
                        // Requests are journaled in order. A failed submission keeps
                        // the earlier cursor so a relaunch can replay that history.
                        if !self.eventReceiptFailed, last > self.cursor {
                            _ = try await client.request(["action": "cursor", "value": last])
                            self.cursor = last
                        }
                    } catch {
                        self.eventReceiptFailed = true
                        needsReload = true
                        if self.errorMessage != error.localizedDescription { self.errorMessage = error.localizedDescription }
                    }
                    // Ignored events still advance the durable cursor, but do
                    // not read and republish an unchanged application snapshot.
                    if needsReload { self.poll() }
                }
            }
        }
        if watcher?.isRunning != true { errorMessage = "Filesystem observation could not start. Use Scan again to refresh this folder." }
    }

    /// Suspends panel auto-dismissal while a system window owns the keyboard.
    func beginSystemDialog() { systemDialogDepth += 1; onSystemDialogDepthChange?(systemDialogDepth) }
    func endSystemDialog() { systemDialogDepth = max(0, systemDialogDepth - 1); onSystemDialogDepthChange?(systemDialogDepth) }

    func chooseFolder(kind: String) {
        guard !busy, !snapshot.scanning else { return }
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true; panel.canChooseFiles = false; panel.allowsMultipleSelection = false
        panel.prompt = "Allow this folder"
        panel.message = "chippytea looks only inside folders you choose. Nothing is selected for cleanup automatically."
        if kind == "downloads" { panel.directoryURL = FileManager.default.urls(for: .downloadsDirectory, in: .userDomainMask).first }
        beginSystemDialog()
        panel.begin { [weak self] result in
            Task { @MainActor [weak self] in
                guard let self else { return }
                defer { self.endSystemDialog() }
                guard result == .OK, let url = panel.url, let client = self.client else { return }
                do {
                    if url.path == self.homePath && !self.diskAccessConfigured { self.beginDiskAccessSetup(); return }
                    let accessing = url.startAccessingSecurityScopedResource()
                    let data = try url.bookmarkData(options: [.withSecurityScope], includingResourceValuesForKeys: nil, relativeTo: nil)
                    try await self.removeUnconfirmedHomeGrant()
                    let rootData = try await client.request(["action": "authorize", "path": url.path, "kind": url.path == self.homePath ? "home" : kind])
                    let root = try EngineClient.decode(ScanRoot.self, rootData)
                    self.bookmarks[root.path] = data
                    do { try self.saveBookmarks() } catch { _ = try await client.request(["action": "forget", "id": root.id]); if accessing { url.stopAccessingSecurityScopedResource() }; throw error }
                    self.accesses.append(url)
                    await self.reload(); self.restartWatcher(); self.destination = .discover; self.refresh()
                } catch { self.errorMessage = error.localizedDescription }
            }
        }
    }
    private func saveBookmarks() throws { try JSONEncoder().encode(bookmarks).write(to: directory.appendingPathComponent("bookmarks.json"), options: .atomic) }

    /// Preserve the system-supplied path; Rust validates each physical ancestor.
    /// Foundation's path normalization can turn /private/var back into the /var link.
    var homePath: String { scanHome.path }
    var homeAuthorized: Bool { snapshot.roots.contains { $0.path == homePath } }

    func scanMyMac() {
        if diskAccessConfigured { authorizeAndScan(path: homePath, replaceContained: true) }
        else { beginDiskAccessSetup() }
    }

    func beginDiskAccessSetup() {
        guard client != nil, !busy, !snapshot.scanning, !snapshot.cleaning else { return }
        if diskAccessPhase != .waiting {
            diskAccessRevision &+= 1
            diskAccessTask?.cancel()
            diskAccessPhase = .intro
            diskAccessStep = .permission
            diskAccessMessage = nil
        }
        showReview = false
        showDiskAccess = true
    }

    /// Serialize the tiny preference writes off the main thread, including a Back action
    /// that arrives while System Settings is opening. The last user intent wins on disk.
    private func saveDiskAccessSetup(_ record: DiskAccessSetupRecord) async throws {
        let previous = diskAccessPersistence
        let file = directory.appendingPathComponent("disk-access.json")
        let data = try JSONEncoder().encode(record)
        let identityFile = directory.appendingPathComponent("disk-access-app.json")
        let identityData = record == .completed ? try diskAccessAppIdentity.map { try JSONEncoder().encode($0) } : nil
        let task = Task.detached(priority: .utility) {
            _ = try? await previous?.value
            if let identityData { try identityData.write(to: identityFile, options: .atomic) }
            try data.write(to: file, options: .atomic)
        }
        diskAccessPersistence = task
        try await task.value
    }

    func dismissDiskAccess() {
        diskAccessRevision &+= 1
        diskAccessTask?.cancel(); diskAccessTask = nil
        showDiskAccess = false
        diskAccessPhase = .intro
        diskAccessStep = .permission
        diskAccessMessage = nil
        finishDiskAccessSystemDialog()
        restartWatcher()
        Task {
            do { try await saveDiskAccessSetup(diskAccessConfigured ? .completed : .dismissed) }
            catch { errorMessage = "Could not save your access preference: \(error.localizedDescription)" }
        }
    }

    func chooseFolderFromDiskAccess() {
        dismissDiskAccess()
        chooseFolder(kind: "folder")
    }

    /// Selecting a specific folder replaces a legacy, unconfirmed whole-home grant.
    /// This revokes only scan authorization; user files, Keep and history are untouched.
    private func removeUnconfirmedHomeGrant() async throws {
        guard !diskAccessConfigured, let root = snapshot.roots.first(where: { $0.path == homePath }), let client else { return }
        _ = try await client.request(["action": "forget", "id": root.id])
        bookmarks.removeValue(forKey: root.path)
        accesses.filter { $0.path == root.path }.forEach { $0.stopAccessingSecurityScopedResource() }
        accesses.removeAll { $0.path == root.path }
    }

    private func beginDiskAccessSystemDialog() {
        guard !diskAccessSystemDialog else { return }
        diskAccessSystemDialog = true
        beginSystemDialog()
    }

    private func finishDiskAccessSystemDialog() {
        guard diskAccessSystemDialog else { return }
        diskAccessSystemDialog = false
        endSystemDialog()
    }

    func diskAccessReturned() {
        guard showDiskAccess else { return }
        finishDiskAccessSystemDialog()
    }

    func confirmDiskAccessAndScan() {
        guard showDiskAccess, diskAccessPhase == .waiting,
              client != nil, !busy, !snapshot.scanning, !snapshot.cleaning else { return }
        diskAccessRevision &+= 1
        let revision = diskAccessRevision
        diskAccessPhase = .starting
        diskAccessMessage = "Checking folder access…"
        // A previous successful setup is no longer evidence once the user is
        // checking changed permissions. Failed or cancelled checks stay gated.
        diskAccessConfigured = false
        restartWatcher()
        diskAccessTask = Task { [weak self] in
            guard let self else { return }
            do {
                let home = self.scanHome
                let blocked = await Task.detached(priority: .utility) {
                    HomeFolderAccess.blockedLocations(in: home)
                }.value
                guard !Task.isCancelled, self.showDiskAccess, self.diskAccessRevision == revision else { return }
                guard blocked.isEmpty else {
                    try await self.saveDiskAccessSetup(.waiting)
                    guard !Task.isCancelled, self.showDiskAccess, self.diskAccessRevision == revision else { return }
                    self.diskAccessPhase = .waiting
                    self.diskAccessStep = .enable
                    self.diskAccessMessage = HomeFolderAccess.message(for: blocked)
                    return
                }
                try await self.saveDiskAccessSetup(.completed)
                guard !Task.isCancelled, self.showDiskAccess, self.diskAccessRevision == revision else { return }
                self.diskAccessConfigured = true
                self.diskAccessNeedsReplacement = false
                self.showDiskAccess = false
                self.diskAccessPhase = .intro
                self.diskAccessMessage = nil
                self.finishDiskAccessSystemDialog()
                self.authorizeAndScan(path: self.homePath, replaceContained: true)
            } catch {
                guard self.diskAccessRevision == revision else { return }
                self.diskAccessPhase = .waiting
                self.diskAccessStep = .enable
                self.diskAccessMessage = "Could not save the scan setup: \(error.localizedDescription)"
            }
        }
    }

    /// The pickerless half of `chooseFolder`. Same engine action, same bookmark, same watcher.
    func authorizeAndScan(path: String, kind: String = "folder", replaceContained: Bool = false) {
        guard path.hasPrefix("/"), !busy, !snapshot.scanning, !snapshot.cleaning, let client else { return }
        let url = URL(fileURLWithPath: path, isDirectory: true)
        let scanKind = url.path == homePath ? "home" : kind
        guard !snapshot.roots.contains(where: { $0.path == url.path && $0.kind == scanKind }) else { restartWatcher(); destination = .discover; refresh(); return }
        busy = true
        Task {
            var authorized = false
            do {
                let data = try await Task.detached(priority: .utility) {
                    let scoped = try? url.bookmarkData(options: [.withSecurityScope], includingResourceValuesForKeys: nil, relativeTo: nil)
                    return try scoped ?? url.bookmarkData(includingResourceValuesForKeys: nil, relativeTo: nil)
                }.value
                let previousBookmarks = bookmarks
                bookmarks[url.path] = data
                // A restart after the engine commits a broader root must already have
                // its bookmark. Extra bookmarks from the old scopes are harmless.
                do { try saveBookmarks() }
                catch { bookmarks = previousBookmarks; throw error }
                let root: ScanRoot
                do {
                    if url.path != homePath { try await removeUnconfirmedHomeGrant() }
                    root = try EngineClient.decode(ScanRoot.self, await client.request(["action": "authorize", "path": url.path, "kind": scanKind, "replace_contained": replaceContained]))
                } catch {
                    bookmarks = previousBookmarks
                    try? saveBookmarks()
                    throw error
                }
                if replaceContained {
                    await reload()
                    let retained = Set(snapshot.roots.map(\.path))
                    func superseded(_ path: String) -> Bool { path.hasPrefix(root.path + "/") && !retained.contains(path) }
                    bookmarks = bookmarks.filter { !superseded($0.key) }
                    accesses.filter { superseded($0.path) }.forEach { $0.stopAccessingSecurityScopedResource() }
                    accesses.removeAll { superseded($0.path) }
                    // The broader bookmark is already durable if pruning fails.
                    do { try saveBookmarks() } catch { errorMessage = error.localizedDescription }
                }
                await reload(); restartWatcher(); destination = .discover; authorized = true
            } catch {
                // A failed Home upgrade can leave the older folder policy in
                // place. Do not let its watcher resume after confirmation.
                if url.path == homePath { diskAccessConfigured = false }
                errorMessage = error.localizedDescription
                await reload(); restartWatcher()
            }
            busy = false
            if authorized { refresh() }
        }
    }

    func openFullDiskAccessSettings() {
        guard showDiskAccess, diskAccessPhase == .intro || diskAccessPhase == .waiting else { return }
        diskAccessRevision &+= 1
        let revision = diskAccessRevision
        diskAccessTask?.cancel()
        diskAccessPhase = .openingSettings
        diskAccessStep = .add
        diskAccessTask = Task { [weak self] in
            guard let self else { return }
            do {
                // macOS can terminate the app after a permission change. Persist first.
                try await self.saveDiskAccessSetup(.waiting)
                guard !Task.isCancelled, self.showDiskAccess, self.diskAccessRevision == revision else { return }
                self.diskAccessPhase = .waiting
                self.diskAccessMessage = nil
                self.beginDiskAccessSystemDialog()
                let destination = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")!
                if !NSWorkspace.shared.open(destination) {
                    self.finishDiskAccessSystemDialog()
                    self.diskAccessMessage = "Open System Settings → Privacy & Security → Full Disk Access."
                }
            } catch {
                guard self.diskAccessRevision == revision else { return }
                self.diskAccessPhase = .intro
                self.diskAccessStep = .permission
                self.diskAccessMessage = "Could not save your place: \(error.localizedDescription)"
            }
        }
    }

    func revealCurrentApp() {
        guard showDiskAccess else { return }
        beginDiskAccessSystemDialog()
        NSWorkspace.shared.activateFileViewerSelecting([Bundle.main.bundleURL])
    }
    func refresh() {
        guard !busy, !discoveryPresentation.isRequestPending, let client else { return }
        if homeAuthorized && !diskAccessConfigured { beginDiskAccessSetup(); return }
        discoveryPresentation.beginRequestedScan()
        Task {
            var accepted = false
            do {
                _ = try await client.request(["action": "scan"])
                accepted = true
                acceptedScanSnapshotBoundary = snapshotRequestID
                if cursor == 0, pendingCursor > 0 {
                    _ = try await client.request(["action": "cursor", "value": pendingCursor])
                    cursor = pendingCursor
                }
            } catch { errorMessage = error.localizedDescription }
            if accepted {
                // Keep the requested state through the asynchronous handoff,
                // including scans which finish before their first snapshot.
                await reload()
            } else {
                acceptedScanSnapshotBoundary = nil
                discoveryPresentation.failRequestedScan()
            }
            poll()
        }
    }
    func cancel() {
        if activeCleanup != nil || !queuedCleanups.isEmpty {
            cleanupCancellationRequested = true
            queuedCleanups.removeAll(keepingCapacity: true)
            refreshCleanupReservations()
            reconcileCleanupEstimate()
        }
        client?.cancel()
    }
    /// Presentation-only entry points, also used by screenshot staging.
    /// A stored confirmation preference must never turn inspection into deletion.
    func reviewSelection() {
        guard canEnqueueCleanup else { return }
        reviewItems = displayedCandidates.filter { selection.contains($0.id) && $0.blockedReason == nil }
        showReview = !reviewItems.isEmpty
    }
    func reviewOne(_ candidate: Candidate) {
        guard canEnqueueCleanup, displayedCandidates.contains(candidate) else { return }
        selection = [candidate.id]
        reviewSelection()
    }
    /// Only a deliberate cleanup action may use the remembered opt-out.
    func requestCleanupSelection() {
        guard canEnqueueCleanup else { return }
        reviewItems = displayedCandidates.filter { selection.contains($0.id) && $0.blockedReason == nil }
        guard !reviewItems.isEmpty else { showReview = false; return }
        if !confirmBeforeDeleting && reviewItems.allSatisfy(\.eligiblePermanent) {
            clean(permanently: true)
        } else {
            showReview = true
        }
    }
    func requestCleanupOne(_ candidate: Candidate) {
        guard canEnqueueCleanup, displayedCandidates.contains(candidate) else { return }
        selection = [candidate.id]
        requestCleanupSelection()
    }
    func clean(permanently: Bool) {
        guard canEnqueueCleanup, !reviewItems.isEmpty, reviewItems.count <= 100 else { return }
        let reviewedItems = reviewItems
        guard !cleanupOverlapsReservation(reviewedItems) else { return }
        let from = max(displayedCoinBalance, confirmedEarnedCoins)
        if activeCleanup == nil && queuedCleanups.isEmpty { cleanupCancellationRequested = false }
        queuedCleanups.append(CleanupRequest(items: reviewedItems, permanently: permanently))
        refreshCleanupReservations()
        let estimate = makeCleanupEstimate()
        pendingCleanupEstimate = estimate
        collectionPresentationGeneration &+= 1
        finishCollection()
        audio.stop()
        cleanupPreview = CleanupPreview(estimate: estimate, from: from, permanently: permanently)
        consumedCleanupPreviewID = nil
        presentedCleanupPreviewID = nil
        selection.subtract(reviewedItems.map(\.id))
        reviewItems = []
        expandedSuggestion = nil
        showReview = false
        destination = .coins
        startNextCleanupIfPossible()
        // Adding work can also request one retry of an unresolved final read.
        // It never bypasses that read or creates an idle retry timer.
        if !cleanupExecuting { poll() }
    }

    private func cleanupOverlapsReservation(_ items: [Candidate]) -> Bool {
        func overlaps(_ lhs: Candidate, _ rhs: Candidate) -> Bool {
            lhs.id == rhs.id || lhs.path == rhs.path
                || lhs.path.hasPrefix(rhs.path + "/") || rhs.path.hasPrefix(lhs.path + "/")
                || (lhs.identity.device == rhs.identity.device && lhs.identity.inode == rhs.identity.inode)
        }
        for (index, item) in items.enumerated() {
            if cleanupCandidateIDs.contains(item.id)
                || items[..<index].contains(where: { overlaps(item, $0) })
                || activeCleanup?.items.contains(where: { overlaps(item, $0) }) == true
                || queuedCleanups.contains(where: { $0.items.contains(where: { overlaps(item, $0) }) }) {
                return true
            }
        }
        return false
    }

    private func refreshCleanupReservations() {
        var reserved = Set<String>()
        if let activeCleanup { reserved.formUnion(activeCleanup.items.map(\.id)) }
        for request in queuedCleanups { reserved.formUnion(request.items.map(\.id)) }
        if reserved != cleanupCandidateIDs { cleanupCandidateIDs = reserved }
        if queuedCleanupCount != queuedCleanups.count { queuedCleanupCount = queuedCleanups.count }
    }

    private func makeCleanupEstimate() -> CleanupEstimate {
        var requests = queuedCleanups
        if let activeCleanup { requests.append(activeCleanup) }
        return CleanupEstimate(requests: requests, wallet: snapshot.wallet, collectedCoinsFloor: confirmedCollectedCoins)
    }

    private func reconcileCleanupEstimate() {
        let estimate = makeCleanupEstimate()
        let finished = activeCleanup == nil && queuedCleanups.isEmpty
        pendingCleanupEstimate = finished ? nil : estimate
        if var preview = cleanupPreview, preview.resolvedTo == nil {
            let previousTarget = preview.targetCoins
            preview.adjustedTo = estimate.targetCoins
            if finished { preview.resolvedTo = estimate.targetCoins }
            cleanupPreview = preview
            if preview.targetCoins < previousTarget { audio.stop() }
        }
    }

    private func startNextCleanupIfPossible() {
        guard activeCleanup == nil, !queuedCleanups.isEmpty, !cleanupCancellationRequested, let client else { return }
        let request = queuedCleanups.removeFirst()
        activeCleanup = request
        refreshCleanupReservations()
        cleanupExecuting = true
        cleanupRunning = true
        cleanupProgress = nil
        // Invalidate reads already queued before confirmation. None may replace
        // the in-flight presentation or trigger the cleanup-complete edge.
        snapshotRequestID &+= 1
        appliedSnapshotID = snapshotRequestID
        busy = true; snapshot.cleaning = true
        startCleanupProgress(client)
        Task {
            do {
                let encoder = JSONEncoder(); encoder.keyEncodingStrategy = .convertToSnakeCase
                let items = try JSONSerialization.jsonObject(with: encoder.encode(request.items))
                let data = try await client.request(["action": "prepare", "operation": request.permanently ? "permanent" : "trash", "items": items])
                let result = try JSONSerialization.jsonObject(with: data) as? [String: String]
                guard let token = result?["token"] else { throw EngineError.message("Review could not be prepared.") }
                guard activeCleanup?.id == request.id, !cleanupCancellationRequested else {
                    throw EngineError.message("Cleanup cancelled before any file was changed.")
                }
                _ = try EngineClient.decode([Receipt].self, await client.request(["action": "execute", "token": token, "confirmed": true]))
            } catch {
                guard activeCleanup?.id == request.id else { return }
                errorMessage = error.localizedDescription
            }
            guard activeCleanup?.id == request.id else { return }
            // A partial batch can have committed receipts even if its response
            // failed. Reconcile those rewards instead of assuming nothing changed.
            cleanupRunning = true
            cleanupExecuting = false
            cleanupProgressTask?.cancel(); cleanupProgressTask = nil
            // Execute has returned; the final snapshot determines which files
            // remain available and which receipts/rewards actually committed.
            snapshot.cleaning = false
            await reload()
            // Only the authoritative reload releases this reservation. An old
            // continuation must not clear busy after another action acquired it.
            startCollectionIfPossible()
            poll()
        }
    }

    private func startCleanupProgress(_ client: EngineClient) {
        cleanupProgressTask?.cancel()
        cleanupProgressTask = Task { [weak self] in
            while !Task.isCancelled {
                do {
                    let progress = try await client.cleanupProgress()
                    guard !Task.isCancelled, let self, self.cleanupExecuting else { return }
                    // A nil sample can occur before execute starts or just after
                    // it ends. Keep the last known phase until the final reload.
                    if let progress, self.cleanupProgress != progress { self.cleanupProgress = progress }
                } catch {
                    guard !Task.isCancelled, let self, self.cleanupExecuting else { return }
                    self.cleanupProgress = nil
                }
                try? await Task.sleep(for: .milliseconds(150))
            }
        }
    }
    /// Consumed by the visible hero once; navigation and reopening cannot replay it.
    func claimCleanupPreview(_ id: UUID) -> Bool {
        guard visible, panelVisible, destination == .coins, !showReview, !showDiskAccess,
              let preview = cleanupPreview, preview.id == id, !preview.presentationFinished,
              consumedCleanupPreviewID != id else { return false }
        consumedCleanupPreviewID = id
        presentedCleanupPreviewID = id
        if preview.isPermanent && preview.targetCoins > preview.from {
            suppressNextCollectionCelebration = true
            audio.play(enabled: soundEnabled)
        }
        return true
    }

    func finishCleanupPreview(_ id: UUID) {
        guard let preview = cleanupPreview, preview.id == id else { return }
        if !preview.presentationFinished { cleanupPreview?.presentationFinished = true }
        retireFinishedCleanupPreview()
    }

    func collect(celebrate: Bool = true) {
        guard visible, client != nil else { return }
        collectionRequested = true
        requestedCollectionCelebration = requestedCollectionCelebration || celebrate
        if cleanupPreview != nil && cleanupPreview?.resolvedTo == nil && !cleanupExecuting {
            // Reopening or explicitly collecting retries reconciliation once.
            // Failed reads may settle idle; they never create an idle retry loop.
            poll()
        }
        startCollectionIfPossible()
    }

    private func startCollectionIfPossible() {
        guard visible, destination == .coins, collectionRequested, !collecting,
              !hasCleanupWork, cleanupPreview == nil || cleanupPreview?.resolvedTo != nil, let client else { return }
        let celebrate = requestedCollectionCelebration && !suppressNextCollectionCelebration
        let generation = collectionPresentationGeneration
        collectionRequested = false
        requestedCollectionCelebration = false
        collecting = true
        Task {
            defer {
                collecting = false
                // One retained demand handles completion racing an older collect.
                // A failed request does not manufacture another demand or retry loop.
                startCollectionIfPossible()
            }
            do {
                let data = try await client.request(["action": "collect"])
                let result = try EngineClient.decode([String: UInt64].self, data)
                guard let from = result["from"], let target = result["to"], let amount = result["amount"],
                      target >= from, target - from == amount else {
                    throw EngineError.message("The collected chip balance could not be verified.")
                }
                if target > confirmedCollectedCoins { confirmedCollectedCoins = target }
                // A finished preview retained across close/reopen may now be
                // fully backed by this response. Retire it before deciding
                // whether this opening still needs its earned collection.
                retireFinishedCleanupPreview()
                // An older request may finish after a new cleanup starts. Its
                // confirmed total is valid, but its celebration belongs to the
                // previous presentation and must not replace the new feedback.
                if amount > 0, celebrate, visible, destination == .coins,
                   generation == collectionPresentationGeneration, cleanupPreview == nil {
                    collection = CollectionBurst(from: from, to: target, amount: amount)
                    audio.play(enabled: soundEnabled)
                }
                if generation == collectionPresentationGeneration { suppressNextCollectionCelebration = false }
                await reload()
            } catch { errorMessage = error.localizedDescription }
        }
    }
    func finishCollection(id: UUID? = nil) {
        if let id, collection?.id != id { return }
        collection = nil
    }
    func windowClosed() {
        visible = false; panelVisible = false
        consumedCleanupPreviewID = cleanupPreview?.id
        if let id = cleanupPreview?.id { finishCleanupPreview(id) }
        presentedCleanupPreviewID = nil
        collectionRequested = false; requestedCollectionCelebration = false
        suppressNextCollectionCelebration = false
        finishCollection(); audio.stop()
    }
    func windowOpened() {
        visible = true; panelVisible = true
        if snapshot.wallet.pendingCoins > 0 { destination = .coins }
        collect()
    }
    func keep(_ candidate: Candidate) { perform(["action": "keep", "id": candidate.id]) }
    func unkeep(path: String) {
        if homeAuthorized && !diskAccessConfigured && (path == homePath || path.hasPrefix(homePath + "/")) {
            beginDiskAccessSetup()
            return
        }
        perform(["action": "unkeep", "path": path])
    }
    func reveal(path: String) { NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)]) }
    func quickLook(path: String) {
        preview.show(path: path)
        guard quickLookWatch == nil else { return }
        beginSystemDialog()
        quickLookWatch = Task { @MainActor [weak self] in
            try? await Task.sleep(for: .milliseconds(250))
            while QLPreviewPanel.sharedPreviewPanelExists(), QLPreviewPanel.shared()?.isVisible == true {
                if Task.isCancelled { break }
                try? await Task.sleep(for: .milliseconds(250))
            }
            self?.quickLookWatch = nil
            self?.endSystemDialog()
        }
    }
    func restore(_ receipt: Receipt) { perform(["action": "restore", "id": receipt.id]) }

    private struct HistoryPage: Decodable {
        let receipts: [Receipt]
        let nextBefore: Int64?
        let total: UInt64
    }

    /// Receipts for the Activity list: the snapshot's live newest page followed
    /// by any older pages loaded on demand, deduplicated by receipt identity.
    var displayedReceipts: [Receipt] {
        guard !olderReceipts.isEmpty else { return snapshot.history }
        var seen = Set(snapshot.history.map(\.id))
        return snapshot.history + olderReceipts.filter { seen.insert($0.id).inserted }
    }

    var hasOlderReceipts: Bool {
        guard !historyPagingExhausted else { return false }
        if let total = historyTotal { return UInt64(displayedReceipts.count) < total }
        // The snapshot page is bounded; a full page means older rows may exist.
        return snapshot.history.count >= 100
    }

    /// Pages the durable ledger past the snapshot's bounded newest rows. The
    /// first request re-reads that page to establish a cursor, so a bounded
    /// number of continuations may be needed before older rows appear.
    func loadOlderReceipts() {
        guard !loadingOlderReceipts, !historyPagingExhausted, let client else { return }
        loadingOlderReceipts = true
        Task {
            defer { loadingOlderReceipts = false }
            do {
                var appended = 0
                for _ in 0..<5 {
                    var request: [String: Any] = ["action": "history", "limit": 200]
                    if let before = historyNextBefore { request["before"] = before }
                    let page = try EngineClient.decode(HistoryPage.self, await client.request(request))
                    if historyTotal != page.total { historyTotal = page.total }
                    let known = Set(snapshot.history.map(\.id)).union(olderReceipts.map(\.id))
                    let fresh = page.receipts.filter { !known.contains($0.id) }
                    olderReceipts.append(contentsOf: fresh)
                    appended += fresh.count
                    guard let next = page.nextBefore else {
                        historyPagingExhausted = true
                        break
                    }
                    historyNextBefore = next
                    if appended > 0 { break }
                }
            } catch { errorMessage = error.localizedDescription }
        }
    }
    func forgetRoot(_ root: ScanRoot) {
        guard !busy, !snapshot.scanning else { return }
        Task {
            do {
                _ = try await client?.request(["action": "forget", "id": root.id])
                bookmarks.removeValue(forKey: root.path); try saveBookmarks()
                accesses.filter { $0.path == root.path }.forEach { $0.stopAccessingSecurityScopedResource() }
                accesses.removeAll { $0.path == root.path }
                await reload(); restartWatcher()
            } catch { errorMessage = error.localizedDescription }
        }
    }
    private func perform(_ request: [String: Any]) {
        guard !busy, let client else { return }
        busy = true
        Task { do { _ = try await client.request(request) } catch { errorMessage = error.localizedDescription }; busy = false; await reload(); poll() }
    }
}
