import AppKit
import Foundation
import Sparkle

@MainActor enum UpdateSelfTest {
    private static func require(_ condition: @autoclosure () -> Bool, _ message: String) throws {
        guard condition() else { throw NSError(domain: "app.chippytea.update-self-test", code: 1,
                                               userInfo: [NSLocalizedDescriptionKey: message]) }
    }

    @MainActor static func run() {
        do {
            try policy()
            try bundleConfiguration()
            try lifecycle()
            print("PASS update self-test: cleanup gates, cancelled/failed installs, deferred relaunch, and bundle configuration")
            exit(0)
        } catch {
            fputs("FAIL update self-test: \(error.localizedDescription)\n", stderr)
            exit(1)
        }
    }

    private static func policy() throws {
        let idle = UpdateModelState(hasCleanupWork: false, busy: false, systemDialogDepth: 0)
        try require(UpdateSafetyPolicy.isSafe(idle), "Idle model state must allow update work")
        try require(UpdateSafetyPolicy.denialReason(for: idle) == nil, "Idle model state has a denial reason")

        let cleanup = UpdateModelState(hasCleanupWork: true, busy: false, systemDialogDepth: 0)
        try require(!UpdateSafetyPolicy.isSafe(cleanup), "Cleanup must block updater work")
        try require(UpdateSafetyPolicy.denialReason(for: cleanup) == "Finish the current cleanup before checking for updates.",
                    "Cleanup denial must explain why restart is unsafe")

        let busy = UpdateModelState(hasCleanupWork: false, busy: true, systemDialogDepth: 0)
        try require(UpdateSafetyPolicy.denialReason(for: busy) != nil, "Busy model must block updater work")
        let dialog = UpdateModelState(hasCleanupWork: false, busy: false, systemDialogDepth: 1)
        try require(UpdateSafetyPolicy.denialReason(for: dialog) != nil, "System dialog must block updater work")

        try require(UpdateActionState.resolve(isInstalling: false, isSessionActive: false,
                                              sessionRequested: false, availableVersion: nil) == .idle,
                    "Idle update action state must be idle")
        try require(UpdateActionState.resolve(isInstalling: false, isSessionActive: true,
                                              sessionRequested: true, availableVersion: nil) == .checking,
                    "An active check must expose checking state")
        try require(UpdateActionState.resolve(isInstalling: false, isSessionActive: true,
                                              sessionRequested: false, availableVersion: "1.2.3") == .available("1.2.3"),
                    "A queued update must remain an available action while its session is active")
        try require(UpdateActionState.resolve(isInstalling: true, isSessionActive: true,
                                              sessionRequested: false, availableVersion: "1.2.3") == .installing,
                    "Installing state must take priority over an available update")

        try require(UpdateController.updatesSuppressed(arguments: ["Chippytea", "--self-test"]),
                    "Self-test must suppress Sparkle")
        try require(UpdateController.updatesSuppressed(arguments: ["Chippytea", "--screenshot"]),
                    "Screenshot mode must suppress Sparkle")
        try require(!UpdateController.updatesSuppressed(arguments: ["Chippytea"]),
                    "Normal app launch must not be suppressed")
    }

    private static func bundleConfiguration() throws {
        // Raw self-test executables intentionally have no appcast configuration.
        // A genuine app bundle, once packaged, must satisfy the signed-feed gate.
        let bundle = Bundle.main
        if bundle.bundleIdentifier == UpdateController.bundleIdentifier {
            let failures = UpdateController.structuralInvariantFailures(in: bundle)
            try require(failures.isEmpty, failures.joined(separator: "; "))
        }
    }

    private static func lifecycle() throws {
        NSApplication.shared.setActivationPolicy(.prohibited)
        let isolated = FileManager.default.temporaryDirectory.appendingPathComponent("chippytea-update-test-\(UUID())")
        let model = AppModel(directory: isolated, scanHome: isolated)
        let controller = UpdateController(model: model, enabled: false)
        // Construct real Sparkle objects but never start an updater, open the
        // engine, touch files, or perform a network request.
        let driver = SPUStandardUpdaterController(startingUpdater: false, updaterDelegate: nil, userDriverDelegate: nil)
        let updater = driver.updater
        let item = SUAppcastItem.empty()
        try require(controller.responds(to: NSSelectorFromString("updater:mayPerformUpdateCheck:error:")),
                    "Sparkle check safety callback must be registered")
        try require(controller.responds(to: NSSelectorFromString("updater:shouldProceedWithUpdate:updateCheck:error:")),
                    "Sparkle pre-install safety callback must be registered")
        try require(controller.responds(to: NSSelectorFromString("updater:shouldPostponeRelaunchForUpdate:untilInvokingBlock:")),
                    "Sparkle deferred-relaunch callback must be registered")
        defer { controller.shutdown() }

        model.snapshot.cleaning = true
        var denied = false
        do { try controller.updater(updater, mayPerform: .updates) }
        catch { denied = true }
        try require(denied, "The registered Sparkle delegate must reject checks during cleanup")
        model.snapshot.cleaning = false
        model.beginSystemDialog()
        denied = false
        do { try controller.updater(updater, shouldProceedWithUpdate: item, updateCheck: .updates) }
        catch { denied = true }
        try require(denied, "The registered Sparkle delegate must reject installation during a system dialog")
        model.endSystemDialog()

        var installed = 0
        model.busy = true
        let postponed = controller.updater(updater, shouldPostponeRelaunchForUpdate: item,
                                         untilInvokingBlock: { installed += 1 })
        try require(postponed == true && controller.isInstalling, "Busy relaunch must be postponed by the registered delegate")
        model.busy = false
        controller.modelStateDidChange()
        controller.modelStateDidChange()
        try require(installed == 1, "Deferred installation must resume exactly once after work finishes")

        let downloadError = NSError(domain: "chippytea.test", code: 1,
                                    userInfo: [NSLocalizedDescriptionKey: "Synthetic failed download"])
        controller.updater(updater, didAbortWithError: downloadError)
        try require(!controller.isInstalling && !controller.isSessionActive,
                    "A failed download must re-enable app interaction")
        try require(controller.statusMessage == downloadError.localizedDescription, "Update errors must stay visible")

        // Cancellation has no error and does not emit userDidMake(.dismiss).
        // Reproduce its actual terminal delegate callback while an install is armed.
        model.busy = true
        _ = controller.updater(updater, shouldPostponeRelaunchForUpdate: item,
                              untilInvokingBlock: { installed += 1 })
        controller.updater(updater, didFinishUpdateCycleFor: .updates, error: nil)
        try require(!controller.isInstalling && !controller.isSessionActive,
                    "A cancelled update cycle must re-enable app interaction")
        model.busy = false
        controller.modelStateDidChange()
        try require(installed == 1, "A cancelled update must not relaunch when earlier work finishes")

        model.busy = true
        _ = controller.updater(updater, shouldPostponeRelaunchForUpdate: item,
                              untilInvokingBlock: { installed += 1 })
        controller.standardUserDriverWillFinishUpdateSession()
        model.busy = false
        controller.modelStateDidChange()
        try require(!controller.isInstalling && installed == 1,
                    "A dismissed update window must clear any pending installation")
    }
}
