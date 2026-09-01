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
        guard let destination else { throw NSError(domain: "Chippytea", code: 1, userInfo: [NSLocalizedDescriptionKey: "Trash did not return a destination."]) }
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
}

struct FolderEventBatch {
    let events: [FolderEvent]
    let last: UInt64
    let historyLost: Bool
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
                ids: UnsafeBufferPointer<FSEventStreamEventId>) -> FolderEventBatch? {
        let last = ids.max() ?? 0
        let dropped = FSEventStreamEventFlags(kFSEventStreamEventFlagUserDropped | kFSEventStreamEventFlagKernelDropped | kFSEventStreamEventFlagEventIdsWrapped)
        // Loss flags apply even when every path is excluded. The bridge responds
        // by refreshing all roots, so there is no need to retain individual paths.
        if flags.count != ids.count || flags.contains(where: { $0 & dropped != 0 }) {
            return FolderEventBatch(events: [], last: last, historyLost: true)
        }
        var changed: [FolderEvent] = []
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
            changed.append(FolderEvent(path: path, kind: kind, recursive: recursive))
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
                        if component.elementsEqual("target".utf8) || component.elementsEqual("node_modules".utf8)
                            || component.elementsEqual(".git".utf8) || component.elementsEqual(".cargo".utf8) { return false }
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
