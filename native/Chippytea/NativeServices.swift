import AppKit
import AVFoundation
import CoreServices
import QuickLookUI

@_cdecl("chippytea_native_trash")
func nativeTrash(_ path: UnsafePointer<CChar>?, _ result: UnsafeMutablePointer<CChar>?, _ capacity: Int) -> Int32 {
    guard let path, let result, capacity > 0 else { return 1 }
    var status: Int32 = 0
    let message: String
    do {
        var destination: NSURL?
        try FileManager.default.trashItem(at: URL(fileURLWithPath: String(cString: path)), resultingItemURL: &destination)
        guard let destination else { throw NSError(domain: "chippytea", code: 1, userInfo: [NSLocalizedDescriptionKey: "Trash did not return a destination."]) }
        message = destination.path ?? ""
    } catch { status = 1; message = error.localizedDescription }
    let bytes = Array(message.utf8)
    guard bytes.count < capacity else { result[0] = 0; return 2 }
    for (index, byte) in bytes.enumerated() { result[index] = CChar(bitPattern: byte) }
    result[bytes.count] = 0
    return status
}

struct FolderEvent {
    let path: String
    let kind: String
    let recursive: Bool
    var request: [String: Any] { ["path": path, "kind": kind, "recursive": recursive] }

    /// A conservative bound for the retained Swift payload and its eventual
    /// JSON representation. The fixed allowance keeps object/dictionary
    /// overhead bounded without serializing the event twice.
    var ingressBytes: Int { path.utf8.count + 64 }
}

struct FolderEventBatch {
    let events: [FolderEvent]
    let last: UInt64
    let historyLost: Bool
    let ingressBytes: Int

    init(events: [FolderEvent], last: UInt64, historyLost: Bool) {
        self.events = events
        self.last = last
        self.historyLost = historyLost
        ingressBytes = events.reduce(0) { $0 + $1.ingressBytes }
    }
}

struct FolderEventIngressLimits: Equatable {
    let maximumBatchEvents: Int
    let maximumBatchBytes: Int
    let maximumQueuedBatches: Int
    let maximumQueuedBytes: Int

    static let production = FolderEventIngressLimits(
        maximumBatchEvents: 512,
        maximumBatchBytes: 256 * 1024,
        // Includes the currently processing batch, not just pending work.
        maximumQueuedBatches: 16,
        maximumQueuedBytes: 4 * 1024 * 1024)

    init(maximumBatchEvents: Int, maximumBatchBytes: Int,
         maximumQueuedBatches: Int, maximumQueuedBytes: Int) {
        self.maximumBatchEvents = max(1, maximumBatchEvents)
        self.maximumBatchBytes = max(1, maximumBatchBytes)
        // Even the smallest test limit must retain an active item and a loss
        // barrier: silently dropping the barrier could acknowledge lost work.
        self.maximumQueuedBatches = max(2, maximumQueuedBatches)
        self.maximumQueuedBytes = max(1, maximumQueuedBytes)
    }
}

struct FolderEventWork {
    let batch: FolderEventBatch
    let revision: Int
}

enum FolderEventEnqueueResult: Equatable {
    case accepted
    case coalesced
    case lossBarrier
    case overflow
}

/// A deterministic bounded FIFO. The active item remains accounted for while
/// the engine awaits its durable dirty writes, so the total retained payload is
/// bounded rather than merely bounding the not-yet-started queue.
struct BoundedFolderEventIngress {
    private let limits: FolderEventIngressLimits
    private var pending: [FolderEventWork] = []
    private var active: FolderEventWork?
    private(set) var queuedEventCount = 0
    private(set) var queuedEventBytes = 0

    init(limits: FolderEventIngressLimits = .production) { self.limits = limits }

    var count: Int { pending.count + (active == nil ? 0 : 1) }
    var isEmpty: Bool { pending.isEmpty && active == nil }

    func containsChanges(overlapping paths: [String]) -> Bool {
        func overlaps(_ work: FolderEventWork) -> Bool {
            work.batch.historyLost || work.batch.events.contains { event in
                paths.contains { path in
                    event.path == path || event.path.hasPrefix(path + "/") || path.hasPrefix(event.path + "/")
                }
            }
        }
        return active.map(overlaps) == true || pending.contains(where: overlaps)
    }

    mutating func enqueue(_ work: FolderEventWork) -> FolderEventEnqueueResult {
        let batch = work.batch
        if batch.historyLost {
            replacePending(with: FolderEventWork(
                batch: FolderEventBatch(events: [], last: batch.last, historyLost: true),
                revision: work.revision))
            return .lossBarrier
        }
        guard batch.events.count <= limits.maximumBatchEvents,
              batch.ingressBytes <= limits.maximumBatchBytes else {
            return overflow(with: work)
        }

        if let index = pending.indices.last,
           pending[index].revision == work.revision,
           !pending[index].batch.historyLost {
            let existing = pending[index].batch
            let combinedCount = existing.events.count + batch.events.count
            let combinedBytes = existing.ingressBytes + batch.ingressBytes
            if combinedCount <= limits.maximumBatchEvents && combinedBytes <= limits.maximumBatchBytes
                && queuedEventBytes + batch.ingressBytes <= limits.maximumQueuedBytes {
                let merged = FolderEventBatch(events: existing.events + batch.events,
                                              last: max(existing.last, batch.last), historyLost: false)
                queuedEventCount += batch.events.count
                queuedEventBytes += batch.ingressBytes
                pending[index] = FolderEventWork(batch: merged, revision: work.revision)
                return .coalesced
            }
        }
        guard count < limits.maximumQueuedBatches,
              queuedEventBytes + batch.ingressBytes <= limits.maximumQueuedBytes else {
            return overflow(with: work)
        }
        pending.append(work)
        queuedEventCount += batch.events.count
        queuedEventBytes += batch.ingressBytes
        return .accepted
    }

    mutating func startNext() -> FolderEventWork? {
        guard active == nil, !pending.isEmpty else { return nil }
        active = pending.removeFirst()
        return active
    }

    mutating func finishActive() {
        guard let active else { return }
        queuedEventCount -= active.batch.events.count
        queuedEventBytes -= active.batch.ingressBytes
        self.active = nil
    }

    private mutating func overflow(with work: FolderEventWork) -> FolderEventEnqueueResult {
        replacePending(with: FolderEventWork(
            batch: FolderEventBatch(events: [], last: work.batch.last, historyLost: true),
            revision: work.revision))
        return .overflow
    }

    private mutating func replacePending(with work: FolderEventWork) {
        queuedEventCount -= pending.reduce(0) { $0 + $1.batch.events.count }
        queuedEventBytes -= pending.reduce(0) { $0 + $1.batch.ingressBytes }
        pending.removeAll(keepingCapacity: true)
        pending.append(work)
        queuedEventCount += work.batch.events.count
        queuedEventBytes += work.batch.ingressBytes
    }
}

/// The bound lives on the producer side of the actor hop. A filesystem storm
/// can schedule only one drain task, even while the main actor is busy. Every
/// retained payload (including active work) lives in the bounded FIFO above.
final class FolderEventMailbox: @unchecked Sendable {
    private let lock = NSLock()
    private var ingress: BoundedFolderEventIngress
    private var scheduled = false
    private var invalidation: UInt64 = 0
    private var revision = 0

    init(limits: FolderEventIngressLimits = .production) {
        ingress = BoundedFolderEventIngress(limits: limits)
    }

    func activate(revision: Int) {
        lock.lock()
        defer { lock.unlock() }
        self.revision = revision
    }

    /// True only when the caller must schedule the single main-actor drain.
    func enqueue(_ work: FolderEventWork) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        // Late callbacks from an invalidated stream must not replace pending
        // current-generation paths with a barrier the consumer would ignore.
        guard work.revision == revision else { return false }
        if work.batch.historyLost || !work.batch.events.isEmpty { invalidation &+= 1 }
        _ = ingress.enqueue(work)
        guard !scheduled else { return false }
        scheduled = true
        return true
    }

    func startNext() -> FolderEventWork? {
        lock.lock()
        defer { lock.unlock() }
        let work = ingress.startNext()
        if work == nil && ingress.isEmpty { scheduled = false }
        return work
    }

    func finishActive() {
        lock.lock()
        defer { lock.unlock() }
        ingress.finishActive()
    }

    var invalidationRevision: UInt64 {
        lock.lock()
        defer { lock.unlock() }
        return invalidation
    }

    func containsChanges(overlapping paths: [String]) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return ingress.containsChanges(overlapping: paths)
    }

    var retainedWork: (batches: Int, events: Int, bytes: Int) {
        lock.lock()
        defer { lock.unlock() }
        return (ingress.count, ingress.queuedEventCount, ingress.queuedEventBytes)
    }
}

/// Decodes borrowed callback values before retaining paths for the engine queue.
struct FolderEventDecoder {
    private struct WatchedPath {
        let path: String
        let prefix: String
        let pathByteCount: Int
        let prefixBytes: [UInt8]

        init(_ value: String) {
            // Grant matching must preserve the engine's exact bytes. Building a
            // file URL can change Unicode normalization even without resolving it.
            path = value
            prefix = path.hasSuffix("/") ? path : path + "/"
            var bytes = Array(value.utf8)
            pathByteCount = bytes.count
            if bytes.last != 47 { bytes.append(47) }
            prefixBytes = bytes
        }
    }

    private let roots: [WatchedPath]
    private let excluded: [WatchedPath]
    var excludedPaths: [String] { excluded.map(\.path) }

    init(paths: [String], excluding: [String] = []) {
        roots = paths.map(WatchedPath.init)
        // Preserve the existing state-directory exclusion spelling.
        excluded = excluding.map {
            WatchedPath(URL(fileURLWithPath: $0, isDirectory: true).path)
        }
    }

    func decode(paths: NSArray, flags: UnsafeBufferPointer<FSEventStreamEventFlags>,
                ids: UnsafeBufferPointer<FSEventStreamEventId>,
                limits: FolderEventIngressLimits = .production) -> FolderEventBatch? {
        let last = ids.max() ?? 0
        let dropped = FSEventStreamEventFlags(kFSEventStreamEventFlagUserDropped | kFSEventStreamEventFlagKernelDropped | kFSEventStreamEventFlagEventIdsWrapped)
        // Loss flags apply even when every path is excluded. The bridge responds
        // by refreshing all roots, so there is no need to retain individual paths.
        if flags.count != ids.count || flags.contains(where: { $0 & dropped != 0 }) {
            // After an ID wrap the numerical maximum may belong to the old
            // epoch. The final delivered event is the reconciliation boundary.
            let wrapped = flags.contains { $0 & FSEventStreamEventFlags(kFSEventStreamEventFlagEventIdsWrapped) != 0 }
            return FolderEventBatch(events: [], last: wrapped ? ids.last ?? 0 : last, historyLost: true)
        }
        var changed: [FolderEvent] = []
        var retainedBytes = 0
        var filteredInternal = false
        for index in ids.indices {
            let flag = flags[index]
            guard flag & FSEventStreamEventFlags(kFSEventStreamEventFlagHistoryDone) == 0 else { continue }
            // Bridge one element at a time. Cleanup bursts must not retain a
            // second full array of Swift strings before their paths are filtered.
            guard index < paths.count, let path = paths[index] as? String else {
                return FolderEventBatch(events: [], last: last, historyLost: true)
            }
            guard !excluded.contains(where: { path == $0.path || path.hasPrefix($0.prefix) }) else { continue }
            if ignoresInternalPath(path) {
                filteredInternal = true
                continue
            }
            let kind = flag & FSEventStreamEventFlags(kFSEventStreamEventFlagItemIsDir) != 0 ? "directory"
                : flag & FSEventStreamEventFlags(kFSEventStreamEventFlagItemIsFile | kFSEventStreamEventFlagItemIsSymlink) != 0 ? "file" : "unknown"
            let subtree = FSEventStreamEventFlags(kFSEventStreamEventFlagMustScanSubDirs | kFSEventStreamEventFlagRootChanged)
            let structural = FSEventStreamEventFlags(kFSEventStreamEventFlagItemCreated | kFSEventStreamEventFlagItemRemoved | kFSEventStreamEventFlagItemRenamed)
            let recursive = flag & subtree != 0 || (kind != "file" && flag & structural != 0)
            let event = FolderEvent(path: path, kind: kind, recursive: recursive)
            guard changed.count < limits.maximumBatchEvents,
                  retainedBytes + event.ingressBytes <= limits.maximumBatchBytes else {
                return FolderEventBatch(events: [], last: last, historyLost: true)
            }
            retainedBytes += event.ingressBytes
            changed.append(event)
        }
        // Internal-only batches still acknowledge their cursor. State-only
        // batches must stay silent or the cursor write would feed back forever.
        guard !changed.isEmpty || filteredInternal else { return nil }
        return FolderEventBatch(events: changed, last: last, historyLost: false)
    }

    private func ignoresInternalPath(_ path: String) -> Bool {
        guard path.contains("/.chippytea-") else { return false }
        // FSEvents supplies NSString-backed values. Borrow native UTF-8 or copy
        // once, rather than repeatedly traversing a foreign String.UTF8View.
        var contiguousPath = path
        return contiguousPath.withUTF8 { bytes in
            var matchedRoot = false
            for root in roots {
                if bytes.count == root.pathByteCount
                    && bytes.elementsEqual(root.prefixBytes.prefix(root.pathByteCount)) { return false }
                guard bytes.starts(with: root.prefixBytes) else { continue }
                matchedRoot = true
                var offset = root.prefixBytes.count
                var internalComponent = false
                while offset < bytes.count {
                    let start = offset
                    while offset < bytes.count && bytes[offset] != 47 {
                        if bytes[offset] == 0 { return false }
                        offset += 1
                    }
                    let component = bytes[start..<offset]
                    // Fail open on ambiguous spellings, including components
                    // after the internal prefix. Rust validates the whole path.
                    if component.isEmpty || component.elementsEqual(".".utf8)
                        || component.elementsEqual("..".utf8) { return false }
                    if !internalComponent {
                        // These names resolve a scope before Rust reaches a later
                        // .chippytea- component. Preserve those invalidations.
                        if Self.scopeBoundaryNames.contains(where: { component.elementsEqual($0) }) { return false }
                        internalComponent = component.starts(with: ".chippytea-".utf8)
                    }
                    if offset == bytes.count { break }
                    offset += 1
                    if offset == bytes.count { return false }
                }
                // An overlapping, explicitly authorized root can make this same
                // path meaningful even when another root would ignore it.
                if !internalComponent { return false }
            }
            return matchedRoot
        }
    }

    // Mirror Rust's lexical artifact boundaries, not its ownership decisions.
    // A change beneath any possible artifact must reach Rust for validation.
    private static let scopeBoundaryNames: [[UInt8]] = [
        "target", "node_modules", ".git", ".cargo", ".venv", "venv", ".next", ".nuxt",
        ".turbo", ".parcel-cache", ".build", ".dart_tool", ".zig-cache", "build", ".gradle", "bin", "obj"
    ].map { Array($0.utf8) }
}

/// Receives and flushes on the watcher's serial queue. Only a cursor is retained;
/// meaningful events and history-loss barriers reach the handler immediately.
final class FolderEventDelivery {
    private let schedule: (@escaping () -> Void) -> Void
    private let handler: ([FolderEvent], UInt64, Bool) -> Void
    private var pendingCursor: UInt64?
    private var timerScheduled = false

    init(schedule: @escaping (@escaping () -> Void) -> Void,
         handler: @escaping ([FolderEvent], UInt64, Bool) -> Void) {
        self.schedule = schedule
        self.handler = handler
    }

    func receive(_ events: [FolderEvent], last: UInt64, historyLost: Bool) {
        if historyLost {
            // A wrapped ID cannot be combined with a pending pre-wrap maximum.
            pendingCursor = nil
            handler(events, last, true)
            return
        }
        if !events.isEmpty {
            let cursor = max(last, pendingCursor ?? last)
            pendingCursor = nil
            handler(events, cursor, false)
            return
        }
        pendingCursor = max(last, pendingCursor ?? last)
        // Reuse the original timer even if an intervening batch consumed its
        // cursor. New empty batches must not keep pushing the flush later.
        guard !timerScheduled else { return }
        timerScheduled = true
        schedule { [weak self] in self?.flush() }
    }

    private func flush() {
        let cursor = pendingCursor
        pendingCursor = nil
        timerScheduled = false
        if let cursor { handler([], cursor, false) }
    }
}

final class FolderWatcher {
    private var stream: FSEventStreamRef?
    private let handler: ([FolderEvent], UInt64, Bool) -> Void
    private let decoder: FolderEventDecoder
    init(paths: [String], since: UInt64, excluding: [String] = [], handler: @escaping ([FolderEvent], UInt64, Bool) -> Void) {
        let queue = DispatchQueue(label: "app.chippytea.events", qos: .utility)
        let delivery = FolderEventDelivery(schedule: { action in
            queue.asyncAfter(deadline: .now() + .milliseconds(100), execute: DispatchWorkItem(block: action))
        }, handler: handler)
        self.handler = { events, last, historyLost in delivery.receive(events, last: last, historyLost: historyLost) }
        decoder = FolderEventDecoder(paths: paths, excluding: excluding)
        guard !paths.isEmpty else { return }
        var context = FSEventStreamContext(version: 0, info: Unmanaged.passUnretained(self).toOpaque(), retain: nil, release: nil, copyDescription: nil)
        let callback: FSEventStreamCallback = { _, info, count, paths, flags, ids in
            guard let info else { return }
            let watcher = Unmanaged<FolderWatcher>.fromOpaque(info).takeUnretainedValue()
            if let batch = watcher.decoder.decode(paths: unsafeBitCast(paths, to: NSArray.self),
                                                  flags: UnsafeBufferPointer(start: flags, count: count),
                                                  ids: UnsafeBufferPointer(start: ids, count: count)) {
                watcher.handler(batch.events, batch.last, batch.historyLost)
            }
        }
        stream = FSEventStreamCreate(nil, callback, &context, paths as CFArray,
            since == 0 ? FSEventStreamEventId(kFSEventStreamEventIdSinceNow) : since,
            0.4, FSEventStreamCreateFlags(kFSEventStreamCreateFlagUseCFTypes | kFSEventStreamCreateFlagFileEvents | kFSEventStreamCreateFlagWatchRoot))
        if let stream {
            let excludedPaths = decoder.excludedPaths
            if !excludedPaths.isEmpty {
                // The system API accepts at most eight paths. The callback
                // covers the complete list, including a failed native setup.
                _ = FSEventStreamSetExclusionPaths(stream, Array(excludedPaths.prefix(8)) as CFArray)
            }
            FSEventStreamSetDispatchQueue(stream, queue)
            if !FSEventStreamStart(stream) { FSEventStreamInvalidate(stream); FSEventStreamRelease(stream); self.stream = nil }
        }
    }
    var isRunning: Bool { stream != nil }
    deinit { if let stream { FSEventStreamStop(stream); FSEventStreamInvalidate(stream); FSEventStreamRelease(stream) } }
}

@MainActor final class CoinAudio {
    private var player: AVAudioPlayer?
    private var generation: UInt64 = 0
    func play(enabled: Bool) {
        stop()
        guard enabled else { return }
        let requestedGeneration = generation
        // A short synthesized five-note arcade arpeggio. No downloaded audio or idle audio engine.
        DispatchQueue.global(qos: .userInitiated).async {
            let sampleRate = 22_050.0
            let duration = 0.67
            let count = Int(duration * sampleRate)
            let notes = [783.99, 987.77, 1174.66, 1567.98, 1975.53]
            var samples = [Int16](repeating: 0, count: count)
            for i in 0..<count {
                let t = Double(i) / sampleRate
                var value = 0.0
                for (index, hz) in notes.enumerated() {
                    let noteT = t - Double(index) * 0.09
                    if noteT >= 0 && noteT < 0.28 {
                        let attack = min(1, noteT / 0.004)
                        let envelope = attack * exp(-noteT * 18)
                        value += (sin(noteT * hz * 2 * .pi) + sin(noteT * hz * 4 * .pi) * 0.22) * envelope * 0.18
                    }
                }
                samples[i] = Int16(max(-1, min(1, value)) * 25_000)
            }
            var data = Data()
            func text(_ s: String) { data.append(contentsOf: s.utf8) }
            func u32(_ v: UInt32) { withUnsafeBytes(of: v.littleEndian) { data.append(contentsOf: $0) } }
            func u16(_ v: UInt16) { withUnsafeBytes(of: v.littleEndian) { data.append(contentsOf: $0) } }
            text("RIFF"); u32(UInt32(36 + count * 2)); text("WAVEfmt "); u32(16); u16(1); u16(1)
            u32(UInt32(sampleRate)); u32(UInt32(sampleRate) * 2); u16(2); u16(16); text("data"); u32(UInt32(count * 2))
            samples.withUnsafeBytes { data.append(contentsOf: $0) }
            Task { @MainActor in
                guard self.generation == requestedGeneration else { return }
                self.player = try? AVAudioPlayer(data: data)
                self.player?.volume = 0.5
                self.player?.play()
            }
        }
    }
    func stop() { generation &+= 1; player?.stop(); player = nil }
}

@MainActor final class QuickLookController: NSObject, @preconcurrency QLPreviewPanelDataSource {
    private var item: NSURL?
    func show(path: String) {
        item = URL(fileURLWithPath: path) as NSURL
        guard let panel = QLPreviewPanel.shared() else { return }
        panel.dataSource = self
        panel.reloadData()
        panel.makeKeyAndOrderFront(nil)
    }
    func numberOfPreviewItems(in panel: QLPreviewPanel!) -> Int { item == nil ? 0 : 1 }
    func previewPanel(_ panel: QLPreviewPanel!, previewItemAt index: Int) -> QLPreviewItem! { item }
}
