import Darwin
import Foundation

/// A live volume-capacity sample, independent of scanning and the reward ledger.
struct StorageStatus: Equatable, Sendable {
    let totalBytes: UInt64
    let availableBytes: UInt64

    /// Includes capacity used by other APFS volumes or reserved by the system.
    var unavailableBytes: UInt64 { totalBytes - availableBytes }

    init?(blockSize: UInt64, totalBlocks: UInt64, availableBlocks: UInt64) {
        // statfs uses all-one bits for fields the filesystem cannot supply.
        guard blockSize > 0, blockSize < UInt64(UInt32.max),
              totalBlocks > 0, totalBlocks != .max, availableBlocks != .max,
              availableBlocks <= totalBlocks else { return nil }
        let total = totalBlocks.multipliedReportingOverflow(by: blockSize)
        let available = availableBlocks.multipliedReportingOverflow(by: blockSize)
        guard !total.overflow, !available.overflow else { return nil }
        totalBytes = total.partialValue
        availableBytes = available.partialValue
    }

    var menuTitle: String {
        let gigabytes = availableBytes / 1_000_000_000
        if gigabytes == 0 && availableBytes > 0 { return "<1 GB free" }
        return "\(gigabytes.formatted()) GB free"
    }

    var detail: String {
        "Startup disk: \(space(availableBytes)) free\n"
            + "\(space(unavailableBytes)) used or reserved of \(space(totalBytes))"
    }

    static func readStartupDisk() -> StorageStatus? {
        var value = statfs()
        // Query the startup Data volume on supported macOS versions. This
        // reads filesystem counters, never user folders or directory contents.
        guard statfs("/System/Volumes/Data", &value) == 0 else { return nil }
        // f_bavail excludes blocks reserved from ordinary users. Apple's disk
        // management guidance recommends this over the larger f_bfree value.
        // https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/getattrlist.2.html
        return StorageStatus(blockSize: UInt64(value.f_bsize), totalBlocks: value.f_blocks,
                             availableBlocks: value.f_bavail)
    }
}

/// At most one utility-queue read and one coalesced follow-up. The timer has
/// substantial leeway; neither opening the window nor cleanup waits for a read.
@MainActor final class StorageStatusMonitor {
    private let queue = DispatchQueue(label: "app.chippytea.storage-status", qos: .utility)
    private let query: @Sendable () -> StorageStatus?
    private let onChange: (StorageStatus?) -> Void
    private var timer: DispatchSourceTimer?
    private var active = false
    private var reading = false
    private var refreshPending = false
    private var generation: UInt64 = 0
    private var hasResult = false
    private var result: StorageStatus?

    init(query: @escaping @Sendable () -> StorageStatus? = { StorageStatus.readStartupDisk() },
         onChange: @escaping (StorageStatus?) -> Void) {
        self.query = query
        self.onChange = onChange
    }

    func start() {
        guard !active else { return }
        active = true
        generation &+= 1
        let source = DispatchSource.makeTimerSource(queue: .main)
        source.schedule(deadline: .now() + 60, repeating: .seconds(60), leeway: .seconds(15))
        source.setEventHandler { [weak self] in self?.refresh() }
        timer = source
        source.resume()
        refresh()
    }

    func refresh() {
        guard active else { return }
        guard !reading else { refreshPending = true; return }
        reading = true
        let generation = generation
        queue.async { [query, weak self] in
            let value = query()
            DispatchQueue.main.async { [weak self] in
                guard let self else { return }
                self.reading = false
                let refreshAgain = self.active && self.refreshPending
                self.refreshPending = false
                if self.active && self.generation == generation && (!self.hasResult || self.result != value) {
                    self.hasResult = true
                    self.result = value
                    self.onChange(value)
                }
                if refreshAgain { self.refresh() }
            }
        }
    }

    func stop() {
        active = false
        generation &+= 1
        refreshPending = false
        timer?.cancel()
        timer = nil
    }

    deinit { timer?.cancel() }
}
