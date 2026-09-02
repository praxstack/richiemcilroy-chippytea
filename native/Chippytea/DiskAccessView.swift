import AppKit
import SwiftUI

/// Scan setup: three pages you click through. macOS grants Full Disk Access;
/// the confirmation checks intended folder access without prompting and never
/// reads or infers the system's global privacy toggle.
struct DiskAccessView: View {
    @ObservedObject var model: AppModel
    @Environment(\.accessibilityReduceMotion) private var systemReduceMotion

    private var reduced: Bool { model.reduceMotion || systemReduceMotion }
    private var step: DiskAccessStep { model.diskAccessStep }
    private var starting: Bool { model.diskAccessPhase == .starting }
    private var opening: Bool { model.diskAccessPhase == .openingSettings }
    private var working: Bool { starting || opening }
    /// The sketches move only while their page is on screen and motion is welcome.
    private var animating: Bool { model.panelVisible && !reduced && !starting }
    private var message: String? {
        guard let message = model.diskAccessMessage, !message.isEmpty else { return nil }
        return message
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            StepMarkers(current: step, seed: 751)
                .padding(.horizontal, TeaTheme.panelPadding + 10)
                .padding(.top, 8).padding(.bottom, 4)
            ScrollView {
                page
                    .id(step)
                    .transition(.opacity)
                    .padding(.horizontal, TeaTheme.panelPadding)
                    .padding(.top, 8).padding(.bottom, 14)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .scrollIndicators(.hidden)
            .animation(reduced ? nil : .easeInOut(duration: 0.18), value: step)
            footer
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .font(TeaFont.body)
        .foregroundStyle(TeaTheme.ink)
        .background {
            TeaTheme.paper
            DotGrid()
        }
        .transaction { transaction in
            if reduced { transaction.animation = nil }
        }
    }

    private var header: some View {
        HStack(spacing: 8) {
            Button { back() } label: {
                HStack(spacing: 4) {
                    Image(systemName: "chevron.left").font(TeaFont.caption)
                    Text("Back")
                }
            }
            .buttonStyle(InkButtonStyle(kind: .quiet, compact: true, seed: 701))
            .disabled(starting)
            .accessibilityLabel(step == .permission ? "Back to chippytea" : "Back to step \(step.rawValue - 1)")
            Spacer(minLength: 4)
            Text("Scan setup · \(step.rawValue) of \(DiskAccessStep.allCases.count)")
                .font(TeaFont.caption).monospacedDigit().foregroundStyle(TeaTheme.inkSoft)
        }
        .padding(.horizontal, TeaTheme.panelPadding)
        .padding(.top, 10)
        .padding(.bottom, 2)
    }

    @ViewBuilder private var page: some View {
        switch step {
        case .permission: permissionPage
        case .add: addPage
        case .enable: enablePage
        }
    }

    private var permissionPage: some View {
        VStack(alignment: .leading, spacing: 12) {
            LookRoundSketch()
                .frame(height: 150).frame(maxWidth: .infinity)
            title("Let chippytea have a look round.", seed: 703)
            copy("To look through your home folder, macOS asks you to switch on Full Disk Access once. It’s quick: open Settings, drag chippytea in, switch it on.")
            VStack(alignment: .leading, spacing: 6) {
                dashed("Finds old caches, logs, build files, downloads and large personal files.", seed: 771)
                dashed("Leaves photo and music libraries alone.", seed: 773)
                dashed("Removes nothing without your review.", seed: 775)
            }
            if let message { SetupSlip(text: message, working: false) }
        }
    }

    private var addPage: some View {
        VStack(alignment: .leading, spacing: 12) {
            DragSketch(animating: animating, frozenAt: model.diskAccessSceneTime)
                .frame(height: 150).frame(maxWidth: .infinity)
            title("Drag chippytea into the list.", seed: 705)
            copy(model.diskAccessNeedsReplacement
                 ? "An older copy of chippytea is already in the list. Remove that one first, then drag this card in."
                 : "System Settings should be open at Privacy & Security → Full Disk Access. Drag this card straight into the list.")
            DraggableAppCard(enabled: !working)
            HStack(spacing: 8) {
                Button("Open Settings again") { model.openFullDiskAccessSettings() }
                    .buttonStyle(InkButtonStyle(kind: .quiet, compact: true, seed: 709))
                    .disabled(working)
                Button { model.revealCurrentApp() } label: {
                    HStack(spacing: 5) {
                        Image(systemName: "folder").font(TeaFont.caption)
                        Text("Reveal in Finder")
                    }
                }
                .buttonStyle(InkButtonStyle(kind: .quiet, compact: true, seed: 717))
                .disabled(working)
                .help("Show this running copy of chippytea in Finder, ready to add with + in System Settings")
                .accessibilityHint("Shows the exact running app in Finder so you can add it to Full Disk Access with the plus button.")
                Spacer(minLength: 0)
            }
            if let message { SetupSlip(text: message, working: false) }
        }
    }

    private var enablePage: some View {
        VStack(alignment: .leading, spacing: 12) {
            ToggleSketch(animating: animating, frozenAt: model.diskAccessSceneTime)
                .frame(height: 150).frame(maxWidth: .infinity)
            title(starting ? "Getting your scan ready…" : "Switch it on.", seed: 707)
            copy(starting
                 ? "Checking the folders chippytea will look through. A scan never removes anything."
                 : "Turn on the switch beside chippytea. If macOS asks, choose Quit & Reopen — your place here is saved.")
            if let message { SetupSlip(text: message, working: working) }
        }
    }

    private func title(_ text: String, seed: Int) -> some View {
        ScreenTitle(text: text, seed: seed, font: TeaFont.headline)
            .fixedSize(horizontal: false, vertical: true)
            .accessibilityAddTraits(.isHeader)
    }

    private func copy(_ text: String) -> some View {
        Text(text).font(TeaFont.body).foregroundStyle(TeaTheme.inkSoft)
            .fixedSize(horizontal: false, vertical: true).lineSpacing(2)
    }

    /// One promise per line, each behind a short gold dash like the chips slip.
    private func dashed(_ text: String, seed: Int) -> some View {
        HStack(alignment: .top, spacing: 8) {
            WobblyLine(amplitude: 0.5, seed: seed)
                .stroke(TeaTheme.goldDeep, style: StrokeStyle(lineWidth: 2, lineCap: .round))
                .frame(width: 9, height: 2)
                .padding(.top, 7)
            Text(text).font(TeaFont.body).foregroundStyle(TeaTheme.ink)
                .fixedSize(horizontal: false, vertical: true).lineSpacing(2)
        }
    }

    private var footer: some View {
        VStack(spacing: 7) {
            primaryAction
            Button("Choose a folder instead") { model.chooseFolderFromDiskAccess() }
                .buttonStyle(InkButtonStyle(kind: .quiet, fullWidth: true, compact: true, seed: 743))
                .disabled(starting)
                .accessibilityHint("Continue with a folder you choose. Full Disk Access is optional.")
        }
        .padding(.horizontal, TeaTheme.panelPadding)
        .padding(.top, 10).padding(.bottom, 12)
        .background(TeaTheme.paperDeep)
        .overlay(alignment: .top) {
            WobblyLine(amplitude: 0.8, seed: 745)
                .stroke(TeaTheme.ink.opacity(0.3), style: StrokeStyle(lineWidth: 1.2, lineCap: .round))
                .frame(height: 3)
        }
    }

    @ViewBuilder private var primaryAction: some View {
        switch step {
        case .permission:
            Button { model.openFullDiskAccessSettings() } label: {
                Label(opening ? "Opening System Settings…" : "Open System Settings", systemImage: "arrow.up.forward.app")
            }
            .buttonStyle(InkButtonStyle(kind: .primary, fullWidth: true, seed: 733))
            .disabled(working)
            .accessibilityHint("Opens Privacy & Security → Full Disk Access in System Settings and saves your place here.")
        case .add:
            Button(opening ? "Opening System Settings…" : "It’s in the list — next") { go(to: .enable) }
                .buttonStyle(InkButtonStyle(kind: .primary, fullWidth: true, seed: 733))
                .disabled(working)
        case .enable:
            Button(starting ? "Starting your scan…" : "It’s switched on — scan my Mac") { model.confirmDiskAccessAndScan() }
                .buttonStyle(InkButtonStyle(kind: .primary, fullWidth: true, seed: 733))
                .disabled(working || model.diskAccessPhase != .waiting)
                .accessibilityHint("Confirms that you enabled Full Disk Access for this app in macOS and completed Quit & Reopen if asked, then starts your home-folder scan.")
        }
    }

    private func back() {
        switch step {
        case .permission: model.dismissDiskAccess()
        case .add: go(to: .permission)
        case .enable: go(to: .add)
        }
    }

    private func go(to target: DiskAccessStep) { model.diskAccessStep = target }
}

// MARK: - Steps

/// Three numbered rings joined by a ruled line: the current one scribbled in
/// biro, finished ones ticked, the rest waiting in faint ink.
private struct StepMarkers: View {
    let current: DiskAccessStep
    var seed: Int

    var body: some View {
        HStack(alignment: .top, spacing: 0) {
            ForEach(Array(DiskAccessStep.allCases.enumerated()), id: \.element) { index, step in
                marker(step, seed: seed &+ index &* 7)
                if step != DiskAccessStep.allCases.last {
                    WobblyLine(amplitude: 0.5, seed: seed &+ index &* 3)
                        .stroke(step.rawValue < current.rawValue ? TeaTheme.biro.opacity(0.55) : TeaTheme.ink.opacity(0.18),
                                style: StrokeStyle(lineWidth: 1.2, lineCap: .round))
                        .frame(height: 2)
                        .frame(maxWidth: .infinity)
                        .padding(.horizontal, 6)
                        .padding(.top, 12)
                }
            }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Step \(current.rawValue) of \(DiskAccessStep.allCases.count): \(current.title)")
    }

    private func marker(_ step: DiskAccessStep, seed: Int) -> some View {
        let done = step.rawValue < current.rawValue
        let active = step == current
        return VStack(spacing: 3) {
            ZStack {
                ScribbleRing(seed: seed, amplitude: active ? 1.0 : 0.7)
                    .stroke(active ? TeaTheme.biro : (done ? TeaTheme.biro.opacity(0.55) : TeaTheme.ink.opacity(0.25)),
                            style: StrokeStyle(lineWidth: active ? 1.5 : 1.2, lineCap: .round))
                if done {
                    TickStroke(seed: seed &+ 5)
                        .stroke(TeaTheme.biro, style: StrokeStyle(lineWidth: 2, lineCap: .round, lineJoin: .round))
                        .frame(width: 12, height: 12)
                } else {
                    Text(step.rawValue.formatted()).font(TeaFont.bodyNumber)
                        .foregroundStyle(active ? TeaTheme.biro : TeaTheme.inkSoft)
                }
            }
            .frame(width: 26, height: 26)
            Text(step.title).font(active ? TeaFont.captionSemibold : TeaFont.caption)
                .foregroundStyle(active ? TeaTheme.biro : TeaTheme.inkSoft)
                .lineLimit(1).fixedSize()
        }
    }
}

private struct TickStroke: Shape {
    var seed: Int = 95
    func path(in rect: CGRect) -> Path {
        handPath([CGPoint(x: rect.minX + rect.width * 0.15, y: rect.minY + rect.height * 0.55),
                  CGPoint(x: rect.minX + rect.width * 0.42, y: rect.minY + rect.height * 0.82),
                  CGPoint(x: rect.minX + rect.width * 0.88, y: rect.minY + rect.height * 0.2)],
                 amplitude: 0.5, seed: seed)
    }
}

/// The real drag source: the same card the sketch shows on the move, drawn with a
/// dashed outline like the ghost chip, because it has not been placed yet.
private struct DraggableAppCard: View {
    let enabled: Bool
    @State private var hovering = false
    @State private var cursorPushed = false

    var body: some View {
        let shape = WobblyRect(radius: 10, amplitude: 0.9, seed: 723, step: 10)
        HStack(spacing: 10) {
            BatteredFishLogo(height: 26)
            VStack(alignment: .leading, spacing: 3) {
                HandWordmark(height: 15)
                Text("Drag me into the list").font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft)
            }
            Spacer(minLength: 0)
            VStack(spacing: 3) {
                ForEach(0..<3, id: \.self) { index in
                    WobblyLine(amplitude: 0.4, seed: 727 + index)
                        .stroke(TeaTheme.ink.opacity(0.35), style: StrokeStyle(lineWidth: 1.4, lineCap: .round))
                        .frame(width: 14, height: 2)
                }
            }
            .accessibilityHidden(true)
        }
        .padding(.horizontal, 12).padding(.vertical, 9)
        .background {
            shape.fill(TeaTheme.card)
                .shadow(color: TeaTheme.ink.opacity(hovering ? 0.18 : 0.10), radius: hovering ? 3 : 1.5, y: hovering ? 3 : 1.5)
        }
        .overlay(shape.stroke(TeaTheme.ink.opacity(enabled ? 0.9 : 0.4),
                              style: StrokeStyle(lineWidth: 1.4, lineCap: .round, lineJoin: .round, dash: [5, 4])))
        .scaleEffect(hovering ? 1.02 : 1)
        .rotationEffect(.degrees(hovering ? -1 : 0))
        .animation(.easeOut(duration: 0.15), value: hovering)
        .contentShape(Rectangle())
        .onDrag { NSItemProvider(object: Bundle.main.bundleURL as NSURL) }
        .onHover { over in
            let active = over && enabled
            hovering = active
            if active, !cursorPushed { NSCursor.openHand.push(); cursorPushed = true }
            if !active, cursorPushed { NSCursor.pop(); cursorPushed = false }
        }
        .onDisappear {
            if cursorPushed { NSCursor.pop(); cursorPushed = false }
        }
        .allowsHitTesting(enabled)
        .opacity(enabled ? 1 : 0.6)
        .help("Drag this copy of chippytea into the Full Disk Access list in System Settings")
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("chippytea app, drag into the Full Disk Access list")
        .accessibilityHint("Or use Reveal in Finder, then add the app with the plus button in System Settings.")
    }
}

/// One slip for progress and problems: a spinner while checking, rust when macOS said no.
private struct SetupSlip: View {
    let text: String
    let working: Bool

    var body: some View {
        InkCard(padding: 9, seed: 731, fill: TeaTheme.card, stroke: working ? TeaTheme.ink.opacity(0.45) : TeaTheme.rust) {
            HStack(alignment: .top, spacing: 8) {
                if working {
                    ProgressView().controlSize(.small).scaleEffect(0.7).frame(width: 16, height: 16)
                        .accessibilityHidden(true)
                } else {
                    Image(systemName: "exclamationmark.circle").font(TeaFont.body)
                        .foregroundStyle(TeaTheme.rust).frame(width: 16, height: 16)
                        .accessibilityHidden(true)
                }
                Text(text).font(TeaFont.caption).foregroundStyle(working ? TeaTheme.inkSoft : TeaTheme.rust)
                    .fixedSize(horizontal: false, vertical: true).lineSpacing(2)
                    .textSelection(.enabled)
                Spacer(minLength: 0)
            }
        }
    }
}

// MARK: - Sketches

/// Smoothstep between two moments of a loop.
private func phase(_ t: Double, _ from: Double, _ to: Double) -> Double {
    let u = max(0, min(1, (t - from) / (to - from)))
    return u * u * (3 - 2 * u)
}

private func lerp(_ a: CGPoint, _ b: CGPoint, _ t: Double) -> CGPoint {
    CGPoint(x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t)
}

private func bezier(_ a: CGPoint, _ c: CGPoint, _ b: CGPoint, _ t: Double) -> CGPoint {
    lerp(lerp(a, c, t), lerp(c, b, t), t)
}

/// Every sketch is 340 × 150 design units, centred in whatever it is given.
private let sketchSize = CGSize(width: 340, height: 150)

/// The parts the setup sketches are drawn from, in the same ink as the page.
private enum Sketch {
    /// A jittered polygon kept sharp-cornered, for the pointer and arrowheads.
    static func jagged(_ points: [CGPoint], amplitude: CGFloat, seed: Int, closed: Bool = true) -> Path {
        var path = Path()
        for (index, point) in points.enumerated() {
            let jittered = CGPoint(x: point.x + inkNoise(index &* 2, seed) * amplitude,
                                   y: point.y + inkNoise(index &* 2 &+ 1, seed) * amplitude)
            if index == 0 { path.move(to: jittered) } else { path.addLine(to: jittered) }
        }
        if closed { path.closeSubpath() }
        return path
    }

    static func settingsWindow(_ ctx: inout GraphicsContext, _ rect: CGRect, title: String, seed: Int) {
        let frame = handPath(roundedRectSamples(rect, radius: 9, step: 12), closed: true, amplitude: 0.9, seed: seed)
        ctx.fill(frame, with: .color(TeaTheme.card))
        ctx.stroke(frame, with: .color(TeaTheme.ink.opacity(0.85)), style: StrokeStyle(lineWidth: 1.4, lineCap: .round, lineJoin: .round))
        for index in 0..<3 {
            let light = handPath(circleSamples(center: CGPoint(x: rect.minX + 13 + CGFloat(index) * 9, y: rect.minY + 11), radius: 2.6, count: 8),
                                 closed: true, amplitude: 0.3, seed: seed &+ index)
            ctx.stroke(light, with: .color(TeaTheme.ink.opacity(0.45)), style: StrokeStyle(lineWidth: 1, lineCap: .round))
        }
        ctx.draw(Text(title).font(TeaFont.captionSemibold).foregroundStyle(TeaTheme.ink),
                 at: CGPoint(x: rect.minX + 44, y: rect.minY + 11), anchor: .leading)
        let rule = handPath(lineSamples(from: CGPoint(x: rect.minX + 6, y: rect.minY + 22), to: CGPoint(x: rect.maxX - 6, y: rect.minY + 22), step: 14),
                            amplitude: 0.6, seed: seed &+ 5)
        ctx.stroke(rule, with: .color(TeaTheme.ink.opacity(0.25)), style: StrokeStyle(lineWidth: 1.1, lineCap: .round))
    }

    /// Someone else's app name, scribbled rather than lettered.
    static func scribble(_ ctx: inout GraphicsContext, from: CGPoint, width: CGFloat, seed: Int, opacity: Double = 0.28) {
        let path = handPath(lineSamples(from: from, to: CGPoint(x: from.x + width, y: from.y), step: 6), amplitude: 1.6, seed: seed)
        ctx.stroke(path, with: .color(TeaTheme.ink.opacity(opacity)), style: StrokeStyle(lineWidth: 2.2, lineCap: .round))
    }

    static func iconBox(_ ctx: inout GraphicsContext, _ rect: CGRect, fill: Color, seed: Int) {
        let box = handPath(roundedRectSamples(rect, radius: 4, step: 5), closed: true, amplitude: 0.5, seed: seed)
        ctx.fill(box, with: .color(fill))
        ctx.stroke(box, with: .color(TeaTheme.ink.opacity(0.45)), style: StrokeStyle(lineWidth: 1, lineCap: .round))
    }

    /// A small square button with a sign in it: the + and − under the list.
    static func signButton(_ ctx: inout GraphicsContext, _ rect: CGRect, plus: Bool, seed: Int) {
        let box = handPath(roundedRectSamples(rect, radius: 3, step: 5), closed: true, amplitude: 0.4, seed: seed)
        ctx.fill(box, with: .color(TeaTheme.card))
        ctx.stroke(box, with: .color(TeaTheme.ink.opacity(0.5)), style: StrokeStyle(lineWidth: 1, lineCap: .round))
        let ink = TeaTheme.ink.opacity(0.7)
        ctx.stroke(handPath(lineSamples(from: CGPoint(x: rect.minX + 3.5, y: rect.midY), to: CGPoint(x: rect.maxX - 3.5, y: rect.midY), step: 4),
                            amplitude: 0.3, seed: seed &+ 1), with: .color(ink), style: StrokeStyle(lineWidth: 1.3, lineCap: .round))
        if plus {
            ctx.stroke(handPath(lineSamples(from: CGPoint(x: rect.midX, y: rect.minY + 3.5), to: CGPoint(x: rect.midX, y: rect.maxY - 3.5), step: 4),
                                amplitude: 0.3, seed: seed &+ 2), with: .color(ink), style: StrokeStyle(lineWidth: 1.3, lineCap: .round))
        }
    }

    /// A drawn switch. `on` blends 0…1 so the knob can slide across.
    static func toggle(_ ctx: inout GraphicsContext, _ rect: CGRect, on: Double, seed: Int, faint: Bool = false) {
        let track = handPath(roundedRectSamples(rect, radius: rect.height / 2, step: 6), closed: true, amplitude: 0.5, seed: seed)
        ctx.fill(track, with: .color(TeaTheme.card))
        if on > 0.01 { ctx.fill(track, with: .color(TeaTheme.gold.opacity(on))) }
        if on > 0.5 {
            var hatch = ctx
            hatch.clip(to: track)
            var lines = Path()
            var x = rect.minX - rect.height
            var index = 0
            while x < rect.maxX {
                let wobble = inkNoise(index, seed) * 0.6
                lines.move(to: CGPoint(x: x + wobble, y: rect.maxY))
                lines.addLine(to: CGPoint(x: x + rect.height - wobble, y: rect.minY))
                x += 4.5
                index += 1
            }
            hatch.stroke(lines, with: .color(TeaTheme.goldDeep.opacity(0.3 * (on - 0.5) * 2)), lineWidth: 0.9)
        }
        let line = TeaTheme.ink.opacity(faint ? 0.35 : 0.85)
        ctx.stroke(track, with: .color(line), style: StrokeStyle(lineWidth: 1.2, lineCap: .round, lineJoin: .round))
        let radius = rect.height / 2 - 2.5
        let knobX = rect.minX + rect.height / 2 + (rect.width - rect.height) * CGFloat(on)
        let knob = handPath(circleSamples(center: CGPoint(x: knobX, y: rect.midY), radius: radius, count: 12), closed: true, amplitude: 0.4, seed: seed &+ 9)
        ctx.fill(knob, with: .color(TeaTheme.card))
        ctx.stroke(knob, with: .color(line), style: StrokeStyle(lineWidth: 1.2, lineCap: .round))
    }

    /// The arrow pointer, drawn by hand. Pressing nudges it down a touch.
    static func cursor(_ ctx: inout GraphicsContext, at point: CGPoint, pressed: Bool, seed: Int, opacity: Double = 1) {
        guard opacity > 0.01 else { return }
        let dy: CGFloat = pressed ? 1.2 : 0
        let outline = [CGPoint(x: 0, y: 0), CGPoint(x: 0, y: 15), CGPoint(x: 4.2, y: 11.6), CGPoint(x: 7.2, y: 18),
                       CGPoint(x: 10, y: 16.8), CGPoint(x: 7, y: 10.6), CGPoint(x: 11.6, y: 10.6)]
            .map { CGPoint(x: point.x + $0.x, y: point.y + $0.y + dy) }
        let path = jagged(outline, amplitude: 0.35, seed: seed)
        var layer = ctx
        layer.opacity = opacity
        layer.fill(path, with: .color(TeaTheme.card))
        layer.stroke(path, with: .color(TeaTheme.ink), style: StrokeStyle(lineWidth: 1.3, lineCap: .round, lineJoin: .round))
    }

    /// The fish and the wordmark side by side, at a chosen fish height.
    static func appName(_ ctx: inout GraphicsContext, at origin: CGPoint, fishHeight: CGFloat, seed: Int, opacity: Double = 1) {
        guard opacity > 0.01 else { return }
        var fish = ctx
        fish.opacity = opacity
        fish.translateBy(x: origin.x, y: origin.y)
        let s = fishHeight / 44
        fish.scaleBy(x: s, y: s)
        paintBatteredFish(&fish, seed: seed, steam: false)
        var word = ctx
        word.opacity = opacity
        _ = paintHandWord("chippytea", in: &word,
                          origin: CGPoint(x: origin.x + fishHeight * 64 / 44 + 6, y: origin.y + fishHeight * 0.14),
                          scale: fishHeight * 0.66 / 92, color: TeaTheme.ink, seed: seed &+ 101)
    }

    /// The drag card as the sketch shows it on the move: lifted cards cast more shadow.
    static func card(_ ctx: inout GraphicsContext, _ rect: CGRect, lift: Double, seed: Int, opacity: Double = 1) {
        guard opacity > 0.01 else { return }
        var layer = ctx
        layer.opacity = opacity
        let shape = handPath(roundedRectSamples(rect, radius: 8, step: 9), closed: true, amplitude: 0.8, seed: seed)
        let shadow = shape.applying(CGAffineTransform(translationX: 0, y: 1.5 + 3 * lift))
        layer.fill(shadow, with: .color(TeaTheme.ink.opacity(0.10 + 0.08 * lift)))
        layer.fill(shape, with: .color(TeaTheme.card))
        layer.stroke(shape, with: .color(TeaTheme.ink), style: StrokeStyle(lineWidth: 1.4, lineCap: .round, lineJoin: .round))
        appName(&layer, at: CGPoint(x: rect.minX + 9, y: rect.minY + 7), fishHeight: rect.height - 14, seed: seed &+ 3)
    }

    /// The dashed outline of a row not yet filled, like the ghost chip.
    static func ghost(_ ctx: inout GraphicsContext, _ rect: CGRect, seed: Int, opacity: Double) {
        guard opacity > 0.01 else { return }
        let shape = handPath(roundedRectSamples(rect, radius: 6, step: 9), closed: true, amplitude: 0.7, seed: seed)
        ctx.stroke(shape, with: .color(TeaTheme.ink.opacity(0.45 * opacity)),
                   style: StrokeStyle(lineWidth: 1.2, lineCap: .round, dash: [4, 3.5]))
    }

    /// A curved arrow with a hand-cut head, for the still versions.
    static func arrow(_ ctx: inout GraphicsContext, from: CGPoint, via: CGPoint, to: CGPoint, seed: Int) {
        let samples = (0...14).map { bezier(from, via, to, Double($0) / 14) }
        ctx.stroke(handPath(samples, amplitude: 0.8, seed: seed), with: .color(TeaTheme.biro),
                   style: StrokeStyle(lineWidth: 1.6, lineCap: .round, dash: [6, 4]))
        let tail = bezier(from, via, to, 0.92)
        let angle = atan2(to.y - tail.y, to.x - tail.x)
        let head = [to,
                    CGPoint(x: to.x - cos(angle - 0.5) * 9, y: to.y - sin(angle - 0.5) * 9),
                    CGPoint(x: to.x - cos(angle + 0.5) * 9, y: to.y - sin(angle + 0.5) * 9)]
        ctx.fill(jagged(head, amplitude: 0.4, seed: seed &+ 3), with: .color(TeaTheme.biro))
    }
}

/// Step 1: the home folder, and the shop's magnifying glass having a look.
private struct LookRoundSketch: View {
    var body: some View {
        ZStack {
            Canvas { context, size in
                var ctx = context
                ctx.translateBy(x: (size.width - sketchSize.width) / 2, y: (size.height - sketchSize.height) / 2)
                let body = CGRect(x: 104, y: 36, width: 138, height: 92)
                let tab = [CGPoint(x: body.minX + 6, y: body.minY + 1), CGPoint(x: body.minX + 8, y: body.minY - 11),
                           CGPoint(x: body.minX + 54, y: body.minY - 11), CGPoint(x: body.minX + 60, y: body.minY + 1)]
                let folder = handPath(roundedRectSamples(body, radius: 9, step: 12), closed: true, amplitude: 1.0, seed: 741)
                let stroke = StrokeStyle(lineWidth: 1.6, lineCap: .round, lineJoin: .round)
                let tabPath = Sketch.jagged(tab, amplitude: 0.6, seed: 742)
                ctx.fill(tabPath, with: .color(TeaTheme.biro.opacity(0.10)))
                ctx.stroke(tabPath, with: .color(TeaTheme.ink.opacity(0.8)), style: stroke)
                ctx.fill(folder, with: .color(TeaTheme.card))
                ctx.stroke(folder, with: .color(TeaTheme.ink.opacity(0.85)), style: stroke)
                // The little house of a home folder.
                let house = [CGPoint(x: 126, y: 84), CGPoint(x: 141, y: 68), CGPoint(x: 156, y: 84), CGPoint(x: 156, y: 102),
                             CGPoint(x: 126, y: 102)]
                ctx.stroke(Sketch.jagged(house, amplitude: 0.7, seed: 744), with: .color(TeaTheme.ink.opacity(0.6)),
                           style: StrokeStyle(lineWidth: 1.3, lineCap: .round, lineJoin: .round))
                ctx.stroke(Sketch.jagged([CGPoint(x: 137, y: 102), CGPoint(x: 137, y: 91), CGPoint(x: 145, y: 91), CGPoint(x: 145, y: 102)],
                                         amplitude: 0.4, seed: 745, closed: false), with: .color(TeaTheme.ink.opacity(0.6)),
                           style: StrokeStyle(lineWidth: 1.1, lineCap: .round, lineJoin: .round))
                // A few papers in it.
                for index in 0..<3 {
                    Sketch.scribble(&ctx, from: CGPoint(x: 172, y: 78 + CGFloat(index) * 12), width: 48 - CGFloat(index) * 9,
                                    seed: 747 + index, opacity: 0.22)
                }
            }
            MagnifierDoodle(size: 66).offset(x: 62, y: 30)
        }
        .accessibilityHidden(true)
    }
}

/// Step 2: the card slides across the page and drops into the Full Disk Access list.
private struct DragSketch: View {
    let animating: Bool
    var frozenAt: Double?
    static let period: Double = 4.8
    @State private var start = Date()

    var body: some View {
        Group {
            if let frozenAt {
                Canvas { ctx, size in Self.draw(&ctx, size: size, time: frozenAt) }
            } else if animating {
                TimelineView(.animation(minimumInterval: 1.0 / 30.0)) { timeline in
                    Canvas { ctx, size in
                        let elapsed = max(0, timeline.date.timeIntervalSince(start))
                        Self.draw(&ctx, size: size, time: elapsed.truncatingRemainder(dividingBy: Self.period))
                    }
                }
                .onAppear { start = Date() }
            } else {
                Canvas { ctx, size in Self.drawStill(&ctx, size: size) }
            }
        }
        .accessibilityHidden(true)
    }

    private static let window = CGRect(x: 122, y: 4, width: 214, height: 142)
    private static let cardSize = CGSize(width: 106, height: 36)
    private static let startCenter = CGPoint(x: 58, y: 112)

    private static func rowRect(_ index: Int) -> CGRect {
        CGRect(x: window.minX + 12, y: window.minY + 30 + CGFloat(index) * 31, width: window.width - 24, height: 28)
    }

    /// Everything that does not move: the window, the other apps, the buttons.
    private static func drawScene(_ ctx: inout GraphicsContext, size: CGSize) {
        ctx.translateBy(x: (size.width - sketchSize.width) / 2, y: (size.height - sketchSize.height) / 2)
        Sketch.settingsWindow(&ctx, window, title: "Full Disk Access", seed: 761)
        for (index, on) in [(0, 1.0), (2, 0.0)] {
            let row = rowRect(index)
            Sketch.iconBox(&ctx, CGRect(x: row.minX + 2, y: row.minY + 5, width: 18, height: 18),
                           fill: index == 0 ? TeaTheme.biro.opacity(0.10) : TeaTheme.gold.opacity(0.18), seed: 771 + index)
            Sketch.scribble(&ctx, from: CGPoint(x: row.minX + 28, y: row.midY), width: index == 0 ? 58 : 44, seed: 775 + index)
            Sketch.toggle(&ctx, CGRect(x: row.maxX - 34, y: row.minY + 6, width: 32, height: 17), on: on, seed: 781 + index, faint: true)
        }
        Sketch.signButton(&ctx, CGRect(x: window.minX + 12, y: window.maxY - 18, width: 15, height: 13), plus: true, seed: 791)
        Sketch.signButton(&ctx, CGRect(x: window.minX + 29, y: window.maxY - 18, width: 15, height: 13), plus: false, seed: 793)
    }

    private static func drawRow(_ ctx: inout GraphicsContext, opacity: Double) {
        guard opacity > 0.01 else { return }
        let slot = rowRect(1)
        Sketch.appName(&ctx, at: CGPoint(x: slot.minX + 2, y: slot.minY + 3), fishHeight: 22, seed: 797, opacity: opacity)
        var layer = ctx
        layer.opacity = opacity
        Sketch.toggle(&layer, CGRect(x: slot.maxX - 34, y: slot.minY + 6, width: 32, height: 17), on: 0, seed: 799)
    }

    static func draw(_ ctx: inout GraphicsContext, size: CGSize, time t: Double) {
        drawScene(&ctx, size: size)
        let slot = rowRect(1)
        let reset = 1 - phase(t, 4.2, 4.8)
        let grab = phase(t, 0.7, 1.0)
        let travel = phase(t, 1.0, 2.35)
        let drop = phase(t, 2.35, 2.6)
        let filled = phase(t, 2.5, 2.85) * reset
        Sketch.ghost(&ctx, slot, seed: 787, opacity: 1 - filled)
        drawRow(&ctx, opacity: filled)

        // The card: lifted, carried along an arc, dropped, then back for another go.
        let scale = 1 + 0.06 * grab - 0.26 * travel
        let squash = 1 - 0.12 * sin(drop * .pi)
        let endCenter = CGPoint(x: slot.minX + 2 + cardSize.width * 0.8 / 2, y: slot.midY)
        let via = CGPoint(x: (startCenter.x + endCenter.x) / 2, y: min(startCenter.y, endCenter.y) - 46)
        let center = reset < 1 ? startCenter : bezier(startCenter, via, endCenter, travel)
        let cardOpacity = (1 - drop) * reset + (1 - reset)
        let size = CGSize(width: cardSize.width * scale, height: cardSize.height * scale * squash)
        let rect = CGRect(x: center.x - size.width / 2, y: center.y - size.height / 2, width: size.width, height: size.height)
        let boil = Int(t * 6) % 3
        Sketch.card(&ctx, rect, lift: grab * (1 - drop), seed: 811 + (travel > 0 && travel < 1 ? boil * 7 : 0), opacity: cardOpacity)

        // The pointer: comes in, grabs, carries, lets go, drifts off.
        let approach = phase(t, 0.15, 0.7)
        let grabPoint = CGPoint(x: center.x + 6, y: center.y + 2)
        let approachFrom = CGPoint(x: startCenter.x + 74, y: startCenter.y + 34)
        var pointer = reset < 1 ? approachFrom : lerp(approachFrom, grabPoint, approach)
        let drift = phase(t, 2.8, 3.6)
        pointer.x += 26 * drift
        pointer.y += 30 * drift
        let pointerOpacity = min(phase(t, 0, 0.15) + (1 - reset), 1 - phase(t, 3.2, 3.8)) * (reset < 1 ? 0 : 1) + (reset < 1 ? reset : 0)
        Sketch.cursor(&ctx, at: pointer, pressed: grab > 0.5 && drop < 0.5, seed: 821, opacity: pointerOpacity)
    }

    /// Reduce Motion gets the whole instruction as one drawing: the card, the
    /// empty row, and an arrow between them.
    static func drawStill(_ ctx: inout GraphicsContext, size: CGSize) {
        drawScene(&ctx, size: size)
        let slot = rowRect(1)
        Sketch.ghost(&ctx, slot, seed: 787, opacity: 1)
        let rect = CGRect(x: startCenter.x - cardSize.width / 2, y: startCenter.y - cardSize.height / 2,
                          width: cardSize.width, height: cardSize.height)
        Sketch.card(&ctx, rect, lift: 0, seed: 811)
        let from = CGPoint(x: rect.maxX - 6, y: rect.minY - 4)
        let to = CGPoint(x: slot.minX + 4, y: slot.midY + 2)
        Sketch.arrow(&ctx, from: from, via: CGPoint(x: (from.x + to.x) / 2 - 8, y: to.y - 34), to: to, seed: 831)
    }
}

/// Step 3: the switch beside chippytea flips on, and macOS asks to Quit & Reopen.
private struct ToggleSketch: View {
    let animating: Bool
    var frozenAt: Double?
    static let period: Double = 4.6
    @State private var start = Date()

    var body: some View {
        Group {
            if let frozenAt {
                Canvas { ctx, size in Self.draw(&ctx, size: size, time: frozenAt) }
            } else if animating {
                TimelineView(.animation(minimumInterval: 1.0 / 30.0)) { timeline in
                    Canvas { ctx, size in
                        let elapsed = max(0, timeline.date.timeIntervalSince(start))
                        Self.draw(&ctx, size: size, time: elapsed.truncatingRemainder(dividingBy: Self.period))
                    }
                }
                .onAppear { start = Date() }
            } else {
                Canvas { ctx, size in Self.draw(&ctx, size: size, time: 3.3) }
            }
        }
        .accessibilityHidden(true)
    }

    private static let row = CGRect(x: 22, y: 16, width: 296, height: 54)
    private static let toggleRect = CGRect(x: row.maxX - 66, y: row.minY + 15, width: 48, height: 24)
    private static let dialog = CGRect(x: 118, y: 84, width: 200, height: 60)
    private static let quitButton = CGRect(x: dialog.maxX - 98, y: dialog.maxY - 26, width: 86, height: 18)
    private static let laterButton = CGRect(x: quitButton.minX - 50, y: quitButton.minY, width: 42, height: 18)

    static func draw(_ ctx: inout GraphicsContext, size: CGSize, time t: Double) {
        ctx.translateBy(x: (size.width - sketchSize.width) / 2, y: (size.height - sketchSize.height) / 2)
        let reset = 1 - phase(t, 4.0, 4.6)
        let on = phase(t, 0.85, 1.25) * reset
        let dialogIn = phase(t, 1.5, 1.9) * reset
        let press = phase(t, 2.7, 2.8) * (1 - phase(t, 2.95, 3.1))

        let rowShape = handPath(roundedRectSamples(row, radius: 10, step: 12), closed: true, amplitude: 0.9, seed: 801)
        ctx.fill(rowShape, with: .color(TeaTheme.card))
        ctx.stroke(rowShape, with: .color(TeaTheme.ink.opacity(0.85)), style: StrokeStyle(lineWidth: 1.4, lineCap: .round, lineJoin: .round))
        Sketch.appName(&ctx, at: CGPoint(x: row.minX + 14, y: row.minY + 12), fishHeight: 30, seed: 805)
        Sketch.toggle(&ctx, toggleRect, on: on, seed: 811 + (on > 0.01 && on < 0.99 ? Int(t * 6) % 3 * 7 : 0))

        if dialogIn > 0.01 {
            var layer = ctx
            layer.opacity = dialogIn
            layer.translateBy(x: 0, y: 10 * (1 - dialogIn))
            let box = handPath(roundedRectSamples(dialog, radius: 9, step: 12), closed: true, amplitude: 0.9, seed: 841)
            layer.fill(box.applying(CGAffineTransform(translationX: 0, y: 2.5)), with: .color(TeaTheme.ink.opacity(0.12)))
            layer.fill(box, with: .color(TeaTheme.card))
            layer.stroke(box, with: .color(TeaTheme.ink.opacity(0.85)), style: StrokeStyle(lineWidth: 1.4, lineCap: .round, lineJoin: .round))
            layer.draw(Text("“chippytea” needs to quit and reopen").font(TeaFont.captionSemibold).foregroundStyle(TeaTheme.ink),
                       at: CGPoint(x: dialog.minX + 12, y: dialog.minY + 14), anchor: .leading)
            layer.draw(Text("before it can have full disk access.").font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft),
                       at: CGPoint(x: dialog.minX + 12, y: dialog.minY + 27), anchor: .leading)
            let later = handPath(roundedRectSamples(laterButton, radius: 5, step: 7), closed: true, amplitude: 0.6, seed: 845)
            layer.stroke(later, with: .color(TeaTheme.ink.opacity(0.6)), style: StrokeStyle(lineWidth: 1.1, lineCap: .round))
            layer.draw(Text("Later").font(TeaFont.captionMedium).foregroundStyle(TeaTheme.inkSoft),
                       at: CGPoint(x: laterButton.midX, y: laterButton.midY))
            var button = layer
            let pressScale = 1 - 0.05 * press
            button.translateBy(x: quitButton.midX, y: quitButton.midY)
            button.scaleBy(x: pressScale, y: pressScale)
            button.translateBy(x: -quitButton.midX, y: -quitButton.midY)
            let quit = handPath(roundedRectSamples(quitButton, radius: 5, step: 7), closed: true, amplitude: 0.6, seed: 847 + Int(press * 2))
            button.fill(quit, with: .color(TeaTheme.gold))
            var hatch = button
            hatch.clip(to: quit)
            var lines = Path()
            var x = quitButton.minX - quitButton.height
            var index = 0
            while x < quitButton.maxX {
                let wobble = inkNoise(index, 849) * 0.6
                lines.move(to: CGPoint(x: x + wobble, y: quitButton.maxY))
                lines.addLine(to: CGPoint(x: x + quitButton.height - wobble, y: quitButton.minY))
                x += 4.5
                index += 1
            }
            hatch.stroke(lines, with: .color(TeaTheme.goldDeep.opacity(0.3)), lineWidth: 0.9)
            button.stroke(quit, with: .color(TeaTheme.ink), style: StrokeStyle(lineWidth: 1.2, lineCap: .round, lineJoin: .round))
            button.draw(Text("Quit & Reopen").font(TeaFont.captionSemibold).foregroundStyle(TeaTheme.ink),
                        at: CGPoint(x: quitButton.midX, y: quitButton.midY))
        }

        // The pointer: to the switch, click, over to Quit & Reopen, click, drift off.
        let togglePoint = CGPoint(x: toggleRect.midX - 2, y: toggleRect.midY - 3)
        let quitPoint = CGPoint(x: quitButton.midX - 8, y: quitButton.midY - 4 + 10 * (1 - dialogIn))
        let approachFrom = CGPoint(x: togglePoint.x + 36, y: togglePoint.y + 56)
        let approach = phase(t, 0.1, 0.65)
        let move = phase(t, 1.9, 2.6)
        var pointer = move > 0 ? lerp(togglePoint, quitPoint, move) : lerp(approachFrom, togglePoint, approach)
        if reset < 1 { pointer = approachFrom }
        let drift = phase(t, 3.3, 3.9)
        pointer.x += 24 * drift
        pointer.y += 22 * drift
        let pressed = (t > 0.65 && t < 0.85) || (t > 2.7 && t < 2.85)
        let pointerOpacity = reset < 1 ? reset : min(phase(t, 0, 0.15), 1 - phase(t, 3.4, 3.9))
        Sketch.cursor(&ctx, at: pointer, pressed: pressed, seed: 851, opacity: pointerOpacity)
    }
}
