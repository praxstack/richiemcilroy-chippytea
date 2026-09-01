import Combine
import Foundation
import Sparkle

/// The model state that must be stable before an updater may interrupt the app.
/// This value type is deliberately independent of Sparkle so it can be tested
/// without starting an updater or touching user defaults.
struct UpdateModelState: Equatable, Sendable {
    let hasCleanupWork: Bool
    let busy: Bool
    let systemDialogDepth: Int
}

enum UpdateSafetyPolicy {
    static func denialReason(for state: UpdateModelState) -> String? {
        if state.hasCleanupWork { return "Finish the current cleanup before checking for updates." }
        if state.busy { return "Finish the current operation before checking for updates." }
        if state.systemDialogDepth > 0 { return "Close the open macOS dialog before checking for updates." }
        return nil
    }

    static func isSafe(_ state: UpdateModelState) -> Bool {
        denialReason(for: state) == nil
    }
}

enum UpdateActionState: Equatable, Sendable {
    case idle
    case checking
    case available(String)
    case installing

    static func resolve(isInstalling: Bool, isSessionActive: Bool,
                        sessionRequested: Bool, availableVersion: String?) -> UpdateActionState {
        if isInstalling { return .installing }
        if let availableVersion { return .available(availableVersion) }
        if isSessionActive || sessionRequested { return .checking }
        return .idle
    }
}

// Sparkle calls its UI delegate on the main thread, but this protocol predates
// its actor annotations. Keep the UI boundary checked by Swift at runtime.
@MainActor final class UpdateController: NSObject, ObservableObject, SPUUpdaterDelegate, @preconcurrency SPUStandardUserDriverDelegate {
    static let bundleIdentifier = "app.chippytea.mac"
    private static let suppressedArguments = [
        "--self-test", "--access-flow-test", "--energy-benchmark", "--scan-benchmark",
        "--maintenance-benchmark", "--screenshot", "--ui-benchmark", "--update-self-test",
    ]

    let model: AppModel
    private var updaterController: SPUStandardUpdaterController?
    private var postponedInstall: (() -> Void)?
    private var updaterObservations: [NSKeyValueObservation] = []
    private var sessionRequested = false
    private var started = false
    private var syncingPublishedSettings = false

    @Published private(set) var canCheckForUpdates = false
    @Published private(set) var availableVersion: String?
    @Published private(set) var lastCheckDate: Date?
    @Published private(set) var statusMessage: String?
    @Published private(set) var isInstalling = false
    @Published private(set) var isSessionActive = false
    @Published private(set) var isEnabled = false

    /// Sparkle owns persistence for these settings. These mirrors are only
    /// published for SwiftUI bindings; they are never written to defaults here.
    @Published var automaticallyChecksForUpdates = false {
        didSet {
            guard automaticallyChecksForUpdates != oldValue, started,
                  !syncingPublishedSettings,
                  let updater = updaterController?.updater else { return }
            updater.automaticallyChecksForUpdates = automaticallyChecksForUpdates
            refreshPublishedState()
        }
    }
    @Published var automaticallyDownloadsUpdates = false {
        didSet {
            guard automaticallyDownloadsUpdates != oldValue, started,
                  !syncingPublishedSettings,
                  let updater = updaterController?.updater else { return }
            updater.automaticallyDownloadsUpdates = automaticallyDownloadsUpdates
            refreshPublishedState()
        }
    }

    var hasAvailableUpdate: Bool { availableVersion != nil }
    var canStartUpdate: Bool { isEnabled && canCheckForUpdates && !isInstalling }
    var actionState: UpdateActionState {
        UpdateActionState.resolve(isInstalling: isInstalling, isSessionActive: isSessionActive,
                                  sessionRequested: sessionRequested, availableVersion: availableVersion)
    }

    init(model: AppModel, bundle: Bundle = .main, enabled: Bool = true) {
        self.model = model
        super.init()
        isEnabled = enabled && Self.defaultEnabled(in: bundle)
    }

    /// Keep raw executables, benchmark harnesses, and self-tests away from
    /// Sparkle. A normal app bundle may still be a signed development build.
    static func defaultEnabled(in bundle: Bundle = .main, arguments: [String] = CommandLine.arguments) -> Bool {
        guard !arguments.contains(where: { suppressedArguments.contains($0) }),
              bundle.bundleIdentifier == bundleIdentifier,
              bundle.bundleURL.pathExtension == "app" else { return false }
        return structuralInvariantFailures(in: bundle).isEmpty
    }

    static func updatesSuppressed(arguments: [String]) -> Bool {
        arguments.contains(where: { suppressedArguments.contains($0) })
    }

    /// These checks are intentionally static: they catch a bundle that could
    /// otherwise make Sparkle present a misleading configuration error.
    static func structuralInvariantFailures(in bundle: Bundle) -> [String] {
        var failures: [String] = []
        guard bundle.bundleIdentifier == bundleIdentifier else { return failures }
        guard let feed = bundle.object(forInfoDictionaryKey: "SUFeedURL") as? String,
              let url = URL(string: feed), url.scheme?.lowercased() == "https" else {
            failures.append("SUFeedURL must be an HTTPS URL")
            return failures
        }
        if let key = bundle.object(forInfoDictionaryKey: "SUPublicEDKey") as? String,
           !key.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
           Data(base64Encoded: key)?.count == 32 {
            // Valid Ed25519 public key.
        } else {
            failures.append("SUPublicEDKey must be a 32-byte base64 key")
        }
        if (bundle.object(forInfoDictionaryKey: "SUVerifyUpdateBeforeExtraction") as? Bool) != true {
            failures.append("SUVerifyUpdateBeforeExtraction must be enabled")
        }
        if (bundle.object(forInfoDictionaryKey: "SURequireSignedFeed") as? Bool) != true {
            failures.append("SURequireSignedFeed must be enabled")
        }
        if let version = bundle.object(forInfoDictionaryKey: "CFBundleVersion") as? String, !version.isEmpty {
            // A non-empty build number is required by Sparkle's version comparison.
        } else {
            failures.append("CFBundleVersion is missing")
        }
        return failures
    }

    func start() {
        guard isEnabled, !started, !Self.updatesSuppressed(arguments: CommandLine.arguments) else { return }
        started = true
        let controller = SPUStandardUpdaterController(
            startingUpdater: false, updaterDelegate: self, userDriverDelegate: self)
        updaterController = controller
        let observedSettings: [KeyPath<SPUUpdater, Bool>] = [
            \.canCheckForUpdates, \.automaticallyChecksForUpdates, \.automaticallyDownloadsUpdates
        ]
        updaterObservations = observedSettings.map { keyPath in
            controller.updater.observe(keyPath, options: [.initial, .new]) { [weak self] _, _ in
                Task { @MainActor [weak self] in self?.refreshPublishedState() }
            }
        }
        controller.startUpdater()
        refreshPublishedState()
    }

    func checkForUpdates() {
        guard canStartUpdate, let controller = updaterController else {
            if let reason = UpdateSafetyPolicy.denialReason(for: currentModelState) { statusMessage = reason }
            return
        }
        sessionRequested = true
        isSessionActive = true
        statusMessage = "Checking for updates…"
        refreshPublishedState()
        controller.checkForUpdates(nil)
    }

    /// Call this from model Combine observations and after cleanup transitions.
    /// It is also the release point for an updater relaunch postponed by cleanup.
    func modelStateDidChange() {
        if let handler = postponedInstall, UpdateSafetyPolicy.isSafe(currentModelState) {
            postponedInstall = nil
            isInstalling = true
            statusMessage = "Installing update…"
            handler()
        }
        refreshPublishedState()
    }

    func shutdown() {
        postponedInstall = nil
        sessionRequested = false
        isSessionActive = false
        started = false
        isInstalling = false
        isEnabled = false
        updaterObservations.removeAll()
        updaterController = nil
        refreshPublishedState()
    }

    private var currentModelState: UpdateModelState {
        UpdateModelState(hasCleanupWork: model.hasCleanupWork, busy: model.busy,
                          systemDialogDepth: model.systemDialogDepth)
    }

    private func refreshPublishedState() {
        guard let updater = updaterController?.updater else {
            if canCheckForUpdates { canCheckForUpdates = false }
            if lastCheckDate != nil { lastCheckDate = nil }
            return
        }
        let checkDate = updater.lastUpdateCheckDate
        if lastCheckDate != checkDate { lastCheckDate = checkDate }
        // Sparkle's own update dialog can change these preferences too.
        // Reflect them without writing them back or triggering a KVO loop.
        syncingPublishedSettings = true
        if automaticallyChecksForUpdates != updater.automaticallyChecksForUpdates {
            automaticallyChecksForUpdates = updater.automaticallyChecksForUpdates
        }
        if automaticallyDownloadsUpdates != updater.automaticallyDownloadsUpdates {
            automaticallyDownloadsUpdates = updater.automaticallyDownloadsUpdates
        }
        syncingPublishedSettings = false
        let canCheck = isEnabled && started && !isInstalling
            && updater.canCheckForUpdates && UpdateSafetyPolicy.isSafe(currentModelState)
        if canCheckForUpdates != canCheck { canCheckForUpdates = canCheck }
    }

    private func unsafeError(for state: UpdateModelState) -> NSError {
        NSError(domain: "app.chippytea.updater", code: 1,
                userInfo: [NSLocalizedDescriptionKey: UpdateSafetyPolicy.denialReason(for: state)
                    ?? "chippytea is busy."])
    }

    // MARK: - SPUUpdaterDelegate

    func updater(_ updater: SPUUpdater, mayPerform updateCheck: SPUUpdateCheck) throws {
        let state = currentModelState
        guard UpdateSafetyPolicy.isSafe(state), !isInstalling else {
            throw unsafeError(for: state)
        }
        sessionRequested = true
        isSessionActive = true
        refreshPublishedState()
    }

    func updater(_ updater: SPUUpdater, shouldProceedWithUpdate updateItem: SUAppcastItem,
                 updateCheck: SPUUpdateCheck) throws {
        let state = currentModelState
        guard UpdateSafetyPolicy.isSafe(state), !isInstalling else {
            throw unsafeError(for: state)
        }
    }

    func updater(_ updater: SPUUpdater, userDidMake choice: SPUUserUpdateChoice,
                 forUpdate updateItem: SUAppcastItem, state: SPUUserUpdateState) {
        if choice == .install {
            isInstalling = true
            isSessionActive = true
            statusMessage = "Installing update…"
        }
        else if state.stage != .installing { isInstalling = false }
        refreshPublishedState()
    }

    func updater(_ updater: SPUUpdater, shouldPostponeRelaunchForUpdate item: SUAppcastItem,
                 untilInvokingBlock installHandler: @escaping () -> Void) -> Bool {
        guard !UpdateSafetyPolicy.isSafe(currentModelState) else { return false }
        postponedInstall = installHandler
        isInstalling = true
        isSessionActive = true
        statusMessage = UpdateSafetyPolicy.denialReason(for: currentModelState)
        refreshPublishedState()
        return true
    }

    func updaterWillRelaunchApplication(_ updater: SPUUpdater) {
        isInstalling = true
        isSessionActive = true
        statusMessage = "Restarting with the update…"
    }

    func updater(_ updater: SPUUpdater, didAbortWithError error: Error) {
        finishSession(error: error)
    }

    func updater(_ updater: SPUUpdater, didFinishUpdateCycleFor updateCheck: SPUUpdateCheck,
                 error: Error?) {
        finishSession(error: error)
    }

    // MARK: - Gentle scheduled reminders for a menu-bar app

    var supportsGentleScheduledUpdateReminders: Bool { true }

    func standardUserDriverShouldHandleShowingScheduledUpdate(_ update: SUAppcastItem,
                                                               andInImmediateFocus immediateFocus: Bool) -> Bool {
        // A recently active app may show Sparkle's normal alert. A background
        // menu-bar app must leave the banner/action to chippytea instead.
        immediateFocus
    }

    func standardUserDriverWillHandleShowingUpdate(_ handleShowingUpdate: Bool,
                                                   forUpdate update: SUAppcastItem,
                                                   state: SPUUserUpdateState) {
        availableVersion = update.displayVersionString
        statusMessage = "Update \(update.displayVersionString) is ready."
        refreshPublishedState()
    }

    func standardUserDriverDidReceiveUserAttention(forUpdate update: SUAppcastItem) {
        availableVersion = nil
        statusMessage = nil
    }

    func standardUserDriverWillFinishUpdateSession() {
        finishSession()
    }

    private func finishSession(error: Error? = nil) {
        // A cancelled download, failed signature, or dismissed dialog must not
        // leave the whole tray disabled, nor a stale relaunch callback armed.
        postponedInstall = nil
        sessionRequested = false
        isSessionActive = false
        isInstalling = false
        availableVersion = nil
        if let error { statusMessage = error.localizedDescription }
        else if statusMessage?.hasPrefix("Checking") == true || statusMessage?.hasPrefix("Installing") == true {
            statusMessage = nil
        }
        refreshPublishedState()
    }
}
