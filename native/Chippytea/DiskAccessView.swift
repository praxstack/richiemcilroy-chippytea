import SwiftUI

/// macOS grants Full Disk Access. The confirmation checks intended folder access
/// without prompting; it never reads or infers the system's global privacy toggle.
struct DiskAccessView: View {
    @ObservedObject var model: AppModel
    @Environment(\.accessibilityReduceMotion) private var systemReduceMotion

    private var reduced: Bool { model.reduceMotion || systemReduceMotion }
    private var working: Bool {
        switch model.diskAccessPhase {
        case .openingSettings, .starting: return true
        case .intro, .waiting: return false
        }
    }
    private var starting: Bool {
        if case .starting = model.diskAccessPhase { return true }
        return false
    }
    private var showSettingsShortcut: Bool {
        switch model.diskAccessPhase {
        case .waiting, .openingSettings: return true
        case .intro, .starting: return false
        }
    }
    private var statusTitle: String {
        switch model.diskAccessPhase {
        case .intro:
            return model.diskAccessMessage?.isEmpty == false ? "Before you start" : ""
        case .openingSettings: return "Opening System Settings…"
        case .waiting: return "Settings first. Then the scan."
        case .starting: return "Getting your scan ready…"
        }
    }
    private var statusDetail: String {
        if let message = model.diskAccessMessage, !message.isEmpty { return message }
        switch model.diskAccessPhase {
        case .intro: return ""
        case .openingSettings: return "Saving your place before opening macOS settings."
        case .waiting:
            return "Enable Chippytea and accept Quit & Reopen if macOS asks. Then confirm below to start."
        case .starting:
            return "We’ll look for cleanup opportunities. Every removal still needs your review."
        }
    }
    private var footerNote: String {
        switch model.diskAccessPhase {
        case .intro, .openingSettings: return "Every cleanup still needs your review."
        case .waiting: return "Enable this copy of Chippytea before starting."
        case .starting: return "A scan does not remove any files."
        }
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    introduction
                    if !starting { instructions }
                    if working || model.diskAccessMessage?.isEmpty == false { setupStatus }
                }
                .padding(.horizontal, TeaTheme.panelPadding)
                .padding(.top, 12)
                .padding(.bottom, 16)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .scrollIndicators(.hidden)
            footer
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .font(TeaFont.body)
        .foregroundStyle(TeaTheme.ink)
        .background {
            TeaTheme.paper
            DotGrid()
        }
        .animation(reduced ? nil : .easeInOut(duration: 0.15), value: statusTitle)
        .transaction { transaction in
            if reduced { transaction.animation = nil }
        }
    }

    private var header: some View {
        HStack(spacing: 8) {
            Button { model.dismissDiskAccess() } label: {
                HStack(spacing: 4) {
                    Image(systemName: "chevron.left").font(TeaFont.caption)
                    Text("Back")
                }
            }
            .buttonStyle(InkButtonStyle(kind: .quiet, compact: true, seed: 701))
            .accessibilityLabel("Back to Chippytea")
            Spacer(minLength: 4)
            Text("Scan setup").font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft)
        }
        .padding(.horizontal, TeaTheme.panelPadding)
        .padding(.top, 10)
        .padding(.bottom, 4)
    }

    private var introduction: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .top, spacing: 12) {
                ScreenTitle(
                    text: starting ? "Starting your scan…" : "A little permission.\nA better scan.",
                    seed: 703,
                    font: TeaFont.headline
                )
                .fixedSize(horizontal: false, vertical: true)
                .accessibilityAddTraits(.isHeader)
                Spacer(minLength: 0)
                MagnifierDoodle(size: 44).padding(.top, 2)
            }
            Text(starting
                 ? "We’ll look through your home folder for old build files, dependencies and large downloads."
                 : "Enable Full Disk Access once for developer artifacts and Downloads. Photos and music libraries are excluded.")
                .font(TeaFont.body).foregroundStyle(TeaTheme.inkSoft)
                .fixedSize(horizontal: false, vertical: true).lineSpacing(2)
        }
    }

    private var instructions: some View {
        VStack(alignment: .leading, spacing: 12) {
            DiskAccessStep(number: 1, title: "Open Full Disk Access", seed: 707) {
                Text("System Settings → Privacy & Security → Full Disk Access.")
                    .font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft)
                    .fixedSize(horizontal: false, vertical: true).lineSpacing(2)
                if showSettingsShortcut {
                    Button("Open settings again") { model.openFullDiskAccessSettings() }
                        .buttonStyle(InkButtonStyle(kind: .quiet, compact: true, seed: 709))
                        .disabled(working)
                }
            }
            DiskAccessStep(number: 2, title: "Add this Chippytea app", seed: 713) {
                Text(model.diskAccessNeedsReplacement
                     ? "Remove the old Chippytea entry first, then add this copy. The earlier build’s permission does not apply to this app."
                     : "Drag the app into the access list. Or click + and choose this copy in Finder.")
                    .font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft)
                    .fixedSize(horizontal: false, vertical: true).lineSpacing(2)
                appDragItem
                Button { model.revealCurrentApp() } label: {
                    HStack(spacing: 5) {
                        Image(systemName: "folder").font(TeaFont.caption)
                        Text("Reveal Chippytea")
                    }
                }
                .buttonStyle(InkButtonStyle(kind: .quiet, compact: true, seed: 717))
                .help("Show this running copy of Chippytea in Finder, ready to add with + in System Settings")
                .accessibilityHint("Shows the exact running app in Finder so you can add it to Full Disk Access using the plus button.")
                .disabled(working)
            }
            DiskAccessStep(number: 3, title: "Switch it on, then reopen", seed: 719) {
                Text("Turn Chippytea on. If macOS asks, choose Quit & Reopen. Your place here is saved.")
                    .font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft)
                    .fixedSize(horizontal: false, vertical: true).lineSpacing(2)
            }
        }
    }

    private var appDragItem: some View {
        InkCard(padding: 8, seed: 723, fill: TeaTheme.card, stroke: TeaTheme.ink.opacity(0.65)) {
            HStack(spacing: 9) {
                BatteredFishLogo(height: 24).accessibilityHidden(true)
                VStack(alignment: .leading, spacing: 3) {
                    Text("Chippytea").font(TeaFont.bodySemibold)
                    Text("Drag into the access list").font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft)
                }
                Spacer(minLength: 0)
                Image(systemName: "arrow.up.forward").font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft)
            }
        }
        .contentShape(Rectangle())
        .onDrag { NSItemProvider(object: Bundle.main.bundleURL as NSURL) }
        .allowsHitTesting(!working)
        .help("Drag this copy of Chippytea into the Full Disk Access list in System Settings")
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Chippytea app, drag into the Full Disk Access list")
        .accessibilityHint("Alternatively use Reveal Chippytea, then add the app with the plus button in System Settings.")
    }

    private var setupStatus: some View {
        InkCard(padding: 10, seed: 731, fill: TeaTheme.card, stroke: TeaTheme.ink.opacity(0.45)) {
            HStack(alignment: .top, spacing: 8) {
                statusIcon.frame(width: 16, height: 16)
                VStack(alignment: .leading, spacing: 5) {
                    Text(statusTitle).font(TeaFont.bodySemibold)
                    Text(statusDetail).font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft)
                        .fixedSize(horizontal: false, vertical: true).lineSpacing(2)
                        .textSelection(.enabled)
                }
                Spacer(minLength: 0)
            }
        }
    }

    @ViewBuilder private var statusIcon: some View {
        if working && !reduced {
            ProgressView().controlSize(.small).scaleEffect(0.7)
                .accessibilityLabel(statusTitle)
        } else {
            Image(systemName: working ? "hourglass" : "info.circle")
                .font(TeaFont.body).foregroundStyle(TeaTheme.biro).accessibilityHidden(true)
        }
    }

    private var footer: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(footerNote).font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft)
                .fixedSize(horizontal: false, vertical: true)
            primaryAction
            Button("Choose a folder instead") { model.chooseFolderFromDiskAccess() }
                .buttonStyle(InkButtonStyle(kind: .quiet, fullWidth: true, compact: true, seed: 743))
                .accessibilityHint("Continue with a folder you choose. Full Disk Access is optional.")
        }
        .padding(.horizontal, TeaTheme.panelPadding)
        .padding(.top, 12).padding(.bottom, 14)
        .background(TeaTheme.paperDeep)
        .overlay(alignment: .top) {
            WobblyLine(amplitude: 0.8, seed: 745)
                .stroke(TeaTheme.ink.opacity(0.3), style: StrokeStyle(lineWidth: 1.2, lineCap: .round))
                .frame(height: 3)
        }
    }

    @ViewBuilder private var primaryAction: some View {
        switch model.diskAccessPhase {
        case .intro:
            Button { model.openFullDiskAccessSettings() } label: {
                Label("Open System Settings", systemImage: "arrow.up.forward.app")
            }
            .buttonStyle(InkButtonStyle(kind: .primary, fullWidth: true, seed: 733))
        case .openingSettings:
            Button("Opening settings…") {}
                .buttonStyle(InkButtonStyle(kind: .primary, fullWidth: true, seed: 733))
                .disabled(true)
        case .waiting:
            Button("I’ve enabled access — scan") { model.confirmDiskAccessAndScan() }
                .buttonStyle(InkButtonStyle(kind: .primary, fullWidth: true, seed: 733))
                .accessibilityHint("Confirms that you enabled Full Disk Access for this app in macOS and completed Quit & Reopen if asked, then starts your home-folder scan.")
        case .starting:
            Button("Starting scan…") {}
                .buttonStyle(InkButtonStyle(kind: .primary, fullWidth: true, seed: 733))
                .disabled(true)
        }
    }
}

private struct DiskAccessStep<Content: View>: View {
    let number: Int
    let title: String
    let seed: Int
    @ViewBuilder var content: Content

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Text(number.formatted()).font(TeaFont.bodyNumber).foregroundStyle(TeaTheme.biro)
                .frame(width: 24, height: 24)
                .background(ScribbleRing(seed: seed).stroke(TeaTheme.biro.opacity(0.75), style: StrokeStyle(lineWidth: 1.3, lineCap: .round)))
                .accessibilityLabel("Step \(number)")
            VStack(alignment: .leading, spacing: 5) {
                Text(title).font(TeaFont.bodySemibold).accessibilityAddTraits(.isHeader)
                content
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.top, 3)
        }
    }
}
