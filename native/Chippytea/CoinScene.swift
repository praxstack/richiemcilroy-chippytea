import AppKit
import SwiftUI

// MARK: - Tokens

/// The single source of colour, type, spacing and drawn chrome for the notebook panel.
/// Documented in `docs/DESIGN.md`; nothing in the UI may introduce a value outside this set.
enum TeaTheme {
    // Notebook stock
    static let paper = Color(red: 0.980, green: 0.961, blue: 0.918)      // #FAF5EA
    static let paperDeep = Color(red: 0.945, green: 0.918, blue: 0.859)  // #F1EADB
    static let card = Color(red: 1.000, green: 0.992, blue: 0.965)       // #FFFDF6

    // Pencil-charcoal ink
    static let ink = Color(red: 0.200, green: 0.188, blue: 0.169)        // #33302B
    static let inkSoft = Color(red: 0.478, green: 0.451, blue: 0.416)    // #7A736A
    static let inkFaint = Color(red: 0.200, green: 0.188, blue: 0.169).opacity(0.12)

    // Marigold
    static let gold = Color(red: 0.949, green: 0.714, blue: 0.235)       // #F2B63C
    static let goldDeep = Color(red: 0.753, green: 0.498, blue: 0.090)   // #C07F17

    // Ballpoint blue
    static let biro = Color(red: 0.247, green: 0.373, blue: 0.659)       // #3F5FA8

    // Caution
    static let rust = Color(red: 0.706, green: 0.333, blue: 0.239)       // #B4553D

    // 4 pt grid
    static let panelPadding: CGFloat = 20
    static let cardPadding: CGFloat = 10
    static let cardRadius: CGFloat = 12
    static let controlRadius: CGFloat = 9
    static let panelWidth: CGFloat = 380
    static let panelHeight: CGFloat = 620
    /// Height reserved above the page for the washi tape to stick out toward the tray.
    static let tapeOverhang: CGFloat = 12
    static let tapeWidth: CGFloat = 56
    static let tapeHeight: CGFloat = 24

    static let inkLine: CGFloat = 1.4
}

/// The complete type scale. System faces, rounded, ink-coloured; only the balance is drawn by hand.
/// There are no uppercase eyebrows: a label is either a heading, a caption, or part of its value.
enum TeaFont {
    static let headline = Font.system(size: 20, weight: .bold, design: .rounded)
    static let title = Font.system(size: 16, weight: .bold, design: .rounded)
    static let subtitle = Font.system(size: 13.5, weight: .semibold, design: .rounded)
    static let body = Font.system(size: 12, design: .rounded)
    static let bodyMedium = Font.system(size: 12, weight: .medium, design: .rounded)
    static let bodySemibold = Font.system(size: 12, weight: .semibold, design: .rounded)
    static let bodyNumber = Font.system(size: 12, weight: .bold, design: .rounded)
    static let caption = Font.system(size: 10, design: .rounded)
    static let captionMedium = Font.system(size: 10, weight: .medium, design: .rounded)
    static let captionSemibold = Font.system(size: 10, weight: .semibold, design: .rounded)
    static let mono = Font.system(size: 9, design: .monospaced)
    static let control = Font.system(size: 11, weight: .semibold, design: .rounded)
}

// MARK: - Ink primitives

/// Deterministic, seed-stable jitter in −1…1. Cycling the seed makes a stroke "boil".
@inline(__always) func inkNoise(_ index: Int, _ seed: Int) -> CGFloat {
    var h = UInt64(bitPattern: Int64(index &* 374_761_393 &+ seed &* 668_265_263 &+ 1_442_695_041))
    h ^= h >> 13
    h = h &* 1_274_126_177
    h ^= h >> 16
    return CGFloat(Int(h % 2001)) / 1000.0 - 1.0
}

/// Smooths a sampled outline through quadratic midpoints after perturbing every sample.
/// Every border, divider and outlined control in the app is drawn this way.
func handPath(_ points: [CGPoint], closed: Bool = false, amplitude: CGFloat = 1.1, seed: Int = 1) -> Path {
    var path = Path()
    guard points.count > 2 else {
        guard let first = points.first else { return path }
        path.move(to: first)
        for point in points.dropFirst() { path.addLine(to: point) }
        return path
    }
    var jittered: [CGPoint] = []
    jittered.reserveCapacity(points.count)
    for (index, point) in points.enumerated() {
        jittered.append(CGPoint(x: point.x + inkNoise(index &* 2, seed) * amplitude,
                                y: point.y + inkNoise(index &* 2 &+ 1, seed) * amplitude))
    }
    func mid(_ a: CGPoint, _ b: CGPoint) -> CGPoint { CGPoint(x: (a.x + b.x) / 2, y: (a.y + b.y) / 2) }
    if closed {
        let count = jittered.count
        path.move(to: mid(jittered[count - 1], jittered[0]))
        for index in 0..<count {
            path.addQuadCurve(to: mid(jittered[index], jittered[(index + 1) % count]), control: jittered[index])
        }
        path.closeSubpath()
    } else {
        path.move(to: jittered[0])
        for index in 1..<(jittered.count - 1) {
            path.addQuadCurve(to: mid(jittered[index], jittered[index + 1]), control: jittered[index])
        }
        path.addLine(to: jittered[jittered.count - 1])
    }
    return path
}

func lineSamples(from start: CGPoint, to end: CGPoint, step: CGFloat = 12) -> [CGPoint] {
    let distance = max(1, hypot(end.x - start.x, end.y - start.y))
    let count = max(2, Int(distance / step))
    return (0...count).map { index in
        let t = CGFloat(index) / CGFloat(count)
        return CGPoint(x: start.x + (end.x - start.x) * t, y: start.y + (end.y - start.y) * t)
    }
}

func circleSamples(center: CGPoint, radius: CGFloat, count: Int = 16, from: CGFloat = 0, sweep: CGFloat = .pi * 2) -> [CGPoint] {
    (0..<count).map { index in
        let angle = from + sweep * CGFloat(index) / CGFloat(count)
        return CGPoint(x: center.x + cos(angle) * radius, y: center.y + sin(angle) * radius)
    }
}

func arcSamples(center: CGPoint, radius: CGFloat, from: CGFloat, to: CGFloat, count: Int = 10) -> [CGPoint] {
    (0...count).map { index in
        let angle = from + (to - from) * CGFloat(index) / CGFloat(count)
        return CGPoint(x: center.x + cos(angle) * radius, y: center.y + sin(angle) * radius)
    }
}

func roundedRectSamples(_ rect: CGRect, radius: CGFloat, step: CGFloat = 10) -> [CGPoint] {
    let r = max(0, min(radius, min(rect.width, rect.height) / 2))
    var points: [CGPoint] = []
    func edge(_ a: CGPoint, _ b: CGPoint) {
        let distance = max(1, hypot(b.x - a.x, b.y - a.y))
        let count = max(1, Int(distance / step))
        for index in 0..<count {
            let t = CGFloat(index) / CGFloat(count)
            points.append(CGPoint(x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t))
        }
    }
    func corner(_ center: CGPoint, _ from: CGFloat, _ to: CGFloat) {
        let count = max(2, Int(r / 4) + 2)
        for index in 0..<count {
            let angle = from + (to - from) * CGFloat(index) / CGFloat(count)
            points.append(CGPoint(x: center.x + cos(angle) * r, y: center.y + sin(angle) * r))
        }
    }
    edge(CGPoint(x: rect.minX + r, y: rect.minY), CGPoint(x: rect.maxX - r, y: rect.minY))
    corner(CGPoint(x: rect.maxX - r, y: rect.minY + r), -.pi / 2, 0)
    edge(CGPoint(x: rect.maxX, y: rect.minY + r), CGPoint(x: rect.maxX, y: rect.maxY - r))
    corner(CGPoint(x: rect.maxX - r, y: rect.maxY - r), 0, .pi / 2)
    edge(CGPoint(x: rect.maxX - r, y: rect.maxY), CGPoint(x: rect.minX + r, y: rect.maxY))
    corner(CGPoint(x: rect.minX + r, y: rect.maxY - r), .pi / 2, .pi)
    edge(CGPoint(x: rect.minX, y: rect.maxY - r), CGPoint(x: rect.minX, y: rect.minY + r))
    corner(CGPoint(x: rect.minX + r, y: rect.minY + r), .pi, 1.5 * .pi)
    return points
}

/// A hand-drawn rounded box. Fill and stroke share the seed so they register exactly.
struct WobblyRect: Shape {
    var radius: CGFloat = TeaTheme.cardRadius
    var amplitude: CGFloat = 1.0
    var seed: Int = 3
    var step: CGFloat = 11
    var inset: CGFloat = 0.8

    func path(in rect: CGRect) -> Path {
        let r = rect.insetBy(dx: inset, dy: inset)
        guard r.width > 2, r.height > 2 else { return Path() }
        return handPath(roundedRectSamples(r, radius: radius, step: step), closed: true, amplitude: amplitude, seed: seed)
    }
}

/// A hand-drawn pill.
struct WobblyPill: Shape {
    var amplitude: CGFloat = 0.8
    var seed: Int = 5
    func path(in rect: CGRect) -> Path {
        let r = rect.insetBy(dx: 0.8, dy: 0.8)
        guard r.width > 2, r.height > 2 else { return Path() }
        return handPath(roundedRectSamples(r, radius: r.height / 2, step: 8), closed: true, amplitude: amplitude, seed: seed)
    }
}

/// A ruled line drawn by hand — used for every divider and hairline.
struct WobblyLine: Shape {
    var amplitude: CGFloat = 0.7
    var seed: Int = 9
    func path(in rect: CGRect) -> Path {
        handPath(lineSamples(from: CGPoint(x: rect.minX + 1, y: rect.midY), to: CGPoint(x: rect.maxX - 1, y: rect.midY), step: 16),
                 amplitude: amplitude, seed: seed)
    }
}

/// A circle scribbled around something, as if ringed in a notebook.
struct ScribbleRing: Shape {
    var seed: Int = 13
    var amplitude: CGFloat = 1.0
    func path(in rect: CGRect) -> Path {
        let center = CGPoint(x: rect.midX, y: rect.midY)
        let radius = min(rect.width, rect.height) / 2 - 1
        var samples = circleSamples(center: center, radius: radius, count: 15, from: -0.5, sweep: .pi * 2)
        samples.append(contentsOf: circleSamples(center: CGPoint(x: center.x + 0.6, y: center.y - 0.4), radius: radius * 0.94, count: 6, from: -0.5, sweep: .pi * 0.75))
        return handPath(samples, closed: false, amplitude: amplitude, seed: seed)
    }
}

struct InkDivider: View {
    var color: Color = TeaTheme.inkFaint
    var seed: Int = 9
    var body: some View {
        WobblyLine(seed: seed)
            .stroke(color, style: StrokeStyle(lineWidth: 1.1, lineCap: .round))
            .frame(height: 2)
    }
}

/// Diagonal pencil hatching, clipped to a shape. The "scribble fill".
struct ScribbleHatch<S: Shape>: View {
    let shape: S
    var color: Color = TeaTheme.goldDeep.opacity(0.32)
    var spacing: CGFloat = 5
    var lineWidth: CGFloat = 1
    var seed: Int = 17

    var body: some View {
        Canvas { context, size in
            var lines = Path()
            var x = -size.height
            var index = 0
            while x < size.width {
                let wobble = inkNoise(index, seed) * 0.9
                lines.move(to: CGPoint(x: x + wobble, y: size.height))
                lines.addQuadCurve(to: CGPoint(x: x + size.height - wobble, y: 0),
                                   control: CGPoint(x: x + size.height / 2 + wobble * 2, y: size.height / 2))
                x += spacing
                index += 1
            }
            var clipped = context
            clipped.clip(to: shape.path(in: CGRect(origin: .zero, size: size)))
            clipped.stroke(lines, with: .color(color), lineWidth: lineWidth)
        }
        .allowsHitTesting(false)
    }
}

/// A drawn card: cream slip, hand-wobbled border, whisper of a shadow.
struct InkCard<Content: View>: View {
    var padding: CGFloat = TeaTheme.cardPadding
    var seed: Int = 3
    var fill: Color = TeaTheme.card
    var stroke: Color = TeaTheme.ink
    var radius: CGFloat = TeaTheme.cardRadius
    var lineWidth: CGFloat = TeaTheme.inkLine
    @ViewBuilder var content: Content

    var body: some View {
        let shape = WobblyRect(radius: radius, seed: seed)
        content
            .padding(padding)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background { shape.fill(fill).shadow(color: TeaTheme.ink.opacity(0.10), radius: 1.5, y: 1.5) }
            .overlay(shape.stroke(stroke, style: StrokeStyle(lineWidth: lineWidth, lineCap: .round, lineJoin: .round)))
    }
}

/// A one-second ink flourish at ~6 fps, then the same drawing at rest.
/// Phase changes stay local; no timeline or sleeping task remains at idle.
struct Boiling<Content: View>: View {
    let active: Bool
    var replay = 0
    var event: UUID? = nil
    @ViewBuilder var content: (Int) -> Content
    @State private var phase = 0
    @State private var previousRequest: Request?

    private struct Request: Equatable {
        let active: Bool
        let replay: Int
        let event: UUID?
    }

    var body: some View {
        let request = Request(active: active, replay: replay, event: event)
        content(active ? phase : 0)
            .task(id: request) {
                guard !Task.isCancelled else { return }
                let previous = previousRequest
                previousRequest = request
                phase = 0
                guard request.active else { return }
                // Ending a collection settles its ink instead of replaying it.
                guard previous?.active != true || previous?.replay != request.replay || request.event != nil else { return }
                do {
                    for step in 1...6 {
                        try await Task.sleep(for: .milliseconds(167))
                        try Task.checkCancellation()
                        phase = step % 3
                    }
                } catch {
                    // The replacement task owns the reset. A cancelled task
                    // must not overwrite a newer flourish's phase.
                }
            }
    }
}

// MARK: - The page and its tape

/// The notebook page itself, inset below the tape overhang.
struct PageShape: Shape {
    var seed: Int = 11
    var radius: CGFloat = 13
    func path(in rect: CGRect) -> Path {
        let page = CGRect(x: rect.minX + 1, y: rect.minY + TeaTheme.tapeOverhang,
                          width: rect.width - 2, height: max(2, rect.height - TeaTheme.tapeOverhang - 1))
        return handPath(roundedRectSamples(page, radius: radius, step: 26), closed: true, amplitude: 1.3, seed: seed)
    }
}

/// The barely-there notebook dot grid.
struct DotGrid: View {
    var body: some View {
        Canvas { context, size in
            let dot = TeaTheme.ink.opacity(0.05)
            var y: CGFloat = 14
            while y < size.height {
                var x: CGFloat = 14
                while x < size.width {
                    context.fill(Path(ellipseIn: CGRect(x: x, y: y, width: 1.7, height: 1.7)), with: .color(dot))
                    x += 18
                }
                y += 18
            }
        }
        .allowsHitTesting(false)
    }
}

/// The tray connection: a strip of marigold washi tape straddling the top edge.
struct WashiTape: View {
    var boil: Int = 0

    var body: some View {
        Canvas { context, size in
            let seed = 41 + boil * 5
            let w = size.width, h = size.height
            var outline: [CGPoint] = []
            // Top edge, left to right.
            outline.append(contentsOf: lineSamples(from: CGPoint(x: 5, y: 1.5), to: CGPoint(x: w - 5, y: 1.5), step: 9))
            // Torn right end.
            outline.append(contentsOf: [CGPoint(x: w - 4, y: h * 0.18), CGPoint(x: w - 1, y: h * 0.34),
                                        CGPoint(x: w - 5, y: h * 0.52), CGPoint(x: w - 1.5, y: h * 0.70),
                                        CGPoint(x: w - 4.5, y: h * 0.88)])
            outline.append(contentsOf: lineSamples(from: CGPoint(x: w - 5, y: h - 1.5), to: CGPoint(x: 5, y: h - 1.5), step: 9))
            // Torn left end.
            outline.append(contentsOf: [CGPoint(x: 4, y: h * 0.84), CGPoint(x: 1, y: h * 0.66),
                                        CGPoint(x: 5, y: h * 0.50), CGPoint(x: 1.5, y: h * 0.32),
                                        CGPoint(x: 4, y: h * 0.16)])
            let shape = handPath(outline, closed: true, amplitude: 0.7, seed: seed)
            context.fill(shape, with: .color(TeaTheme.gold.opacity(0.62)))
            var inner = context
            inner.clip(to: shape)
            for index in 0..<5 {
                let x = CGFloat(index) * 13 + 4 + inkNoise(index, seed) * 1.5
                var streak = Path()
                streak.move(to: CGPoint(x: x, y: -2))
                streak.addLine(to: CGPoint(x: x + 7, y: h + 2))
                inner.stroke(streak, with: .color(TeaTheme.goldDeep.opacity(0.16)), lineWidth: 2.4)
            }
            context.stroke(shape, with: .color(TeaTheme.goldDeep.opacity(0.45)), lineWidth: 1.1)
        }
        .frame(width: TeaTheme.tapeWidth, height: TeaTheme.tapeHeight)
        .rotationEffect(.degrees(-3))
        .accessibilityHidden(true)
    }
}

// MARK: - The chip

/// One hand-cut chip: a wobbly golden stick with fried edges, an ink outline
/// and a catch of light. Drawn lying flat about `center`; rotate the context
/// to angle it. The pale version is the dashed ghost of a chip yet to come.
private func paintChip(_ context: inout GraphicsContext, center: CGPoint, length: CGFloat, seed: Int, pale: Bool = false) {
    let thickness = length * 0.30
    let rect = CGRect(x: center.x - length / 2, y: center.y - thickness / 2, width: length, height: thickness)
    let body = handPath(roundedRectSamples(rect, radius: thickness * 0.42, step: 5), closed: true, amplitude: length * 0.03, seed: seed)
    if pale {
        context.stroke(body, with: .color(TeaTheme.inkSoft.opacity(0.55)),
                       style: StrokeStyle(lineWidth: max(1, length * 0.05), lineCap: .round, dash: [length * 0.14, length * 0.11]))
        return
    }
    context.fill(body, with: .color(TeaTheme.gold))
    var fried = context
    fried.clip(to: body)
    // Two fried streaks running the length, and a crisper tip where the fryer caught it.
    for lane in 0..<2 {
        let y = center.y + (lane == 0 ? -1 : 1) * thickness * 0.28
        var streak = Path()
        streak.move(to: CGPoint(x: rect.minX + length * 0.10, y: y + inkNoise(lane, seed) * 0.8))
        streak.addQuadCurve(to: CGPoint(x: rect.maxX - length * 0.10, y: y + inkNoise(lane &+ 5, seed) * 0.8),
                            control: CGPoint(x: center.x, y: y + inkNoise(lane &+ 9, seed) * 1.8))
        fried.stroke(streak, with: .color(TeaTheme.goldDeep.opacity(lane == 0 ? 0.30 : 0.48)), lineWidth: max(0.9, thickness * 0.16))
    }
    let tip = handPath(arcSamples(center: CGPoint(x: rect.maxX - thickness * 0.45, y: center.y), radius: thickness * 0.32,
                                  from: -.pi / 2, to: .pi / 2, count: 5), amplitude: 0.5, seed: seed &+ 3)
    fried.stroke(tip, with: .color(TeaTheme.goldDeep.opacity(0.55)), lineWidth: max(0.9, thickness * 0.18))
    context.stroke(body, with: .color(TeaTheme.ink), style: StrokeStyle(lineWidth: max(1, length * 0.042), lineCap: .round, lineJoin: .round))
    var shine = Path()
    shine.move(to: CGPoint(x: rect.minX + length * 0.15, y: rect.minY + thickness * 0.30))
    shine.addQuadCurve(to: CGPoint(x: rect.minX + length * 0.42, y: rect.minY + thickness * 0.24),
                       control: CGPoint(x: rect.minX + length * 0.28, y: rect.minY + thickness * 0.12))
    context.stroke(shine, with: .color(.white.opacity(0.8)), style: StrokeStyle(lineWidth: max(0.9, thickness * 0.15), lineCap: .round))
}

private func strokeChipOutline(_ context: inout GraphicsContext, length: CGFloat, color: Color, seed: Int) {
    let thickness = length * 0.30
    let rect = CGRect(x: -length / 2, y: -thickness / 2, width: length, height: thickness)
    let body = handPath(roundedRectSamples(rect, radius: thickness * 0.42, step: 5), closed: true, amplitude: length * 0.03, seed: seed)
    context.stroke(body, with: .color(color), style: StrokeStyle(lineWidth: max(1, length * 0.075), lineCap: .round, lineJoin: .round))
}

/// The unit of the whole app: three chips standing upright, fresh from the fryer.
/// `filled` paints them golden; otherwise bare ink outline — the resting tab state.
struct ChipsDoodle: View {
    var size: CGFloat = 24
    var filled = true
    var color: Color = TeaTheme.inkSoft
    var seed: Int = 7

    private static let poses: [(dx: CGFloat, dy: CGFloat, tilt: Double, length: CGFloat)] = [
        (-0.28, 0.06, -11, 0.74), (0.29, 0.08, 12, 0.68), (0.00, 0.00, 2, 0.94)
    ]

    var body: some View {
        Canvas { context, dimensions in
            let extent = min(dimensions.width, dimensions.height)
            let center = CGPoint(x: dimensions.width / 2, y: dimensions.height / 2)
            for (index, pose) in Self.poses.enumerated() {
                var chip = context
                chip.translateBy(x: center.x + pose.dx * extent, y: center.y + pose.dy * extent)
                chip.rotate(by: .degrees(-90 + pose.tilt))
                let length = extent * pose.length
                if filled {
                    paintChip(&chip, center: .zero, length: length, seed: seed &+ index &* 7)
                } else {
                    strokeChipOutline(&chip, length: length, color: color, seed: seed &+ index &* 7)
                }
            }
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }
}

// MARK: - The shop sign

/// The fish: 64 × 44 design box, battered gold body, forked tail, one open eye.
/// Shared by the logo view and the monochrome menu-bar rendering.
private let fishBodyPoints: [CGPoint] = [
    CGPoint(x: 3, y: 26), CGPoint(x: 7, y: 20), CGPoint(x: 14, y: 15), CGPoint(x: 25, y: 12),
    CGPoint(x: 35, y: 13), CGPoint(x: 43, y: 17), CGPoint(x: 48, y: 20), CGPoint(x: 59, y: 12),
    CGPoint(x: 55, y: 25), CGPoint(x: 59, y: 36), CGPoint(x: 48, y: 28), CGPoint(x: 41, y: 33),
    CGPoint(x: 29, y: 36), CGPoint(x: 15, y: 34), CGPoint(x: 6, y: 31)
]

/// Paints the battered fish into a context already scaled to its 64 × 44 design
/// box. Shared by the shop sign, the hero counter and the fish in the paper.
func paintBatteredFish(_ ctx: inout GraphicsContext, seed: Int, steam: Bool = true) {
    if steam {
        // Steam off the fryer.
        for (index, wisp) in [[CGPoint(x: 30, y: 9), CGPoint(x: 33, y: 5), CGPoint(x: 29, y: 1)],
                              [CGPoint(x: 40, y: 8), CGPoint(x: 43, y: 4), CGPoint(x: 40, y: 1)]].enumerated() {
            ctx.stroke(handPath(wisp, amplitude: 0.5, seed: seed &+ index),
                       with: .color(TeaTheme.ink.opacity(0.38)), style: StrokeStyle(lineWidth: 1.3, lineCap: .round))
        }
    }

    let body = handPath(fishBodyPoints, closed: true, amplitude: 1.1, seed: seed)
    ctx.fill(body, with: .color(TeaTheme.gold))
    // Scribbled batter.
    var batter = ctx
    batter.clip(to: body)
    var lines = Path()
    var x: CGFloat = -30
    while x < 64 {
        lines.move(to: CGPoint(x: x + inkNoise(Int(x), seed) * 0.9, y: 44))
        lines.addQuadCurve(to: CGPoint(x: x + 30, y: 6), control: CGPoint(x: x + 13, y: 25 + inkNoise(Int(x) &+ 7, seed) * 2))
        x += 4.2
    }
    batter.stroke(lines, with: .color(TeaTheme.goldDeep.opacity(0.32)), lineWidth: 1)
    ctx.stroke(body, with: .color(TeaTheme.ink), style: StrokeStyle(lineWidth: 1.7, lineCap: .round, lineJoin: .round))

    // Tail creases.
    ctx.stroke(handPath([CGPoint(x: 49, y: 21), CGPoint(x: 56, y: 15)], amplitude: 0.4, seed: seed &+ 3),
               with: .color(TeaTheme.ink.opacity(0.7)), style: StrokeStyle(lineWidth: 1.1, lineCap: .round))
    ctx.stroke(handPath([CGPoint(x: 49, y: 27), CGPoint(x: 56, y: 33)], amplitude: 0.4, seed: seed &+ 4),
               with: .color(TeaTheme.ink.opacity(0.7)), style: StrokeStyle(lineWidth: 1.1, lineCap: .round))
    // Gill line and eye.
    ctx.stroke(handPath([CGPoint(x: 17, y: 19), CGPoint(x: 19, y: 24), CGPoint(x: 17, y: 29)], amplitude: 0.5, seed: seed &+ 5),
               with: .color(TeaTheme.ink.opacity(0.55)), style: StrokeStyle(lineWidth: 1.1, lineCap: .round))
    ctx.fill(Path(ellipseIn: CGRect(x: 10.2, y: 20.2, width: 3.6, height: 3.6)), with: .color(TeaTheme.ink))
    ctx.fill(Path(ellipseIn: CGRect(x: 10.9, y: 20.9, width: 1.1, height: 1.1)), with: .color(TeaTheme.card))
    // A batter drip under the belly.
    ctx.stroke(handPath([CGPoint(x: 22, y: 36), CGPoint(x: 24, y: 39), CGPoint(x: 26, y: 36)], amplitude: 0.4, seed: seed &+ 6),
               with: .color(TeaTheme.ink.opacity(0.5)), style: StrokeStyle(lineWidth: 1.1, lineCap: .round))
}

/// The battered fish itself — the shop sign of the whole app. Scribble-hatched
/// batter, ink outline, a wisp of steam off the fryer.
struct BatteredFishLogo: View {
    var height: CGFloat = 26
    var boil: Int = 0

    var body: some View {
        Canvas { context, dimensions in
            let s = dimensions.height / 44
            var ctx = context
            ctx.scaleBy(x: s, y: s)
            paintBatteredFish(&ctx, seed: 401 + boil * 7)
        }
        .frame(width: height * 64 / 44, height: height)
        .rotationEffect(.degrees(-2))
        .accessibilityHidden(true)
    }
}

/// Hand-lettered lowercase letterforms on a 92-tall box: baseline 70, x-height
/// 30–70, descenders to 92. Centre-lines only, like the numerals; the wordmark
/// strokes them in felt-tip ink.
private func letterStrokes(_ letter: Character) -> (strokes: [[CGPoint]], advance: CGFloat) {
    switch letter {
    case "c":
        return ([[CGPoint(x: 33, y: 36), CGPoint(x: 22, y: 29), CGPoint(x: 11, y: 36), CGPoint(x: 7, y: 50),
                  CGPoint(x: 11, y: 64), CGPoint(x: 22, y: 71), CGPoint(x: 33, y: 64)]], 40)
    case "h":
        return ([[CGPoint(x: 9, y: 10), CGPoint(x: 10, y: 40), CGPoint(x: 10, y: 70)],
                 [CGPoint(x: 10, y: 48), CGPoint(x: 16, y: 33), CGPoint(x: 26, y: 31), CGPoint(x: 32, y: 42), CGPoint(x: 33, y: 70)]], 42)
    case "i":
        return ([[CGPoint(x: 8, y: 34), CGPoint(x: 9, y: 52), CGPoint(x: 10, y: 70)]], 19)
    case "p":
        return ([[CGPoint(x: 8, y: 34), CGPoint(x: 10, y: 60), CGPoint(x: 12, y: 90)],
                 [CGPoint(x: 10, y: 42), CGPoint(x: 20, y: 31), CGPoint(x: 31, y: 36), CGPoint(x: 34, y: 50),
                  CGPoint(x: 29, y: 63), CGPoint(x: 18, y: 66), CGPoint(x: 10, y: 58)]], 42)
    case "y":
        return ([[CGPoint(x: 7, y: 32), CGPoint(x: 10, y: 50), CGPoint(x: 17, y: 61), CGPoint(x: 27, y: 63)],
                 [CGPoint(x: 33, y: 31), CGPoint(x: 33, y: 52), CGPoint(x: 29, y: 72), CGPoint(x: 21, y: 86), CGPoint(x: 12, y: 91)]], 40)
    case "t":
        return ([[CGPoint(x: 15, y: 12), CGPoint(x: 16, y: 40), CGPoint(x: 17, y: 60), CGPoint(x: 22, y: 69), CGPoint(x: 30, y: 66)],
                 [CGPoint(x: 5, y: 33), CGPoint(x: 16, y: 32), CGPoint(x: 28, y: 31)]], 34)
    case "e":
        return ([[CGPoint(x: 8, y: 50), CGPoint(x: 20, y: 48), CGPoint(x: 31, y: 45), CGPoint(x: 29, y: 34),
                  CGPoint(x: 19, y: 29), CGPoint(x: 9, y: 37), CGPoint(x: 7, y: 51), CGPoint(x: 12, y: 65),
                  CGPoint(x: 23, y: 71), CGPoint(x: 32, y: 66)]], 40)
    case "a":
        return ([[CGPoint(x: 30, y: 38), CGPoint(x: 19, y: 31), CGPoint(x: 9, y: 39), CGPoint(x: 7, y: 53),
                  CGPoint(x: 12, y: 66), CGPoint(x: 23, y: 68), CGPoint(x: 30, y: 58)],
                 [CGPoint(x: 31, y: 33), CGPoint(x: 31, y: 52), CGPoint(x: 32, y: 70), CGPoint(x: 37, y: 68)]], 43)
    case "f":
        return ([[CGPoint(x: 27, y: 13), CGPoint(x: 18, y: 16), CGPoint(x: 14, y: 28), CGPoint(x: 13, y: 48), CGPoint(x: 13, y: 70)],
                 [CGPoint(x: 4, y: 33), CGPoint(x: 15, y: 32), CGPoint(x: 27, y: 31)]], 32)
    case "s":
        return ([[CGPoint(x: 29, y: 36), CGPoint(x: 19, y: 30), CGPoint(x: 10, y: 36), CGPoint(x: 13, y: 46),
                  CGPoint(x: 23, y: 51), CGPoint(x: 29, y: 58), CGPoint(x: 25, y: 68), CGPoint(x: 14, y: 70),
                  CGPoint(x: 6, y: 64)]], 38)
    default:
        return ([], 20)
    }
}

/// Measures and strokes any word made of the available letterforms, in a
/// context already positioned at the word's top-left. Returns the pen advance.
@discardableResult
func paintHandWord(_ word: String, in context: inout GraphicsContext, origin: CGPoint,
                   scale: CGFloat, color: Color, seed: Int) -> CGFloat {
    var penX: CGFloat = 1
    for (index, letter) in word.enumerated() {
        let (strokes, advance) = letterStrokes(letter)
        let bounce = inkNoise(index, seed) * 1.6
        for (strokeIndex, stroke) in strokes.enumerated() {
            let points = stroke.map { CGPoint(x: origin.x + (penX + $0.x) * scale, y: origin.y + ($0.y + bounce) * scale) }
            let path = handPath(points, amplitude: 1.4, seed: seed &+ index &* 13 &+ strokeIndex &* 5)
            context.stroke(path, with: .color(color),
                           style: StrokeStyle(lineWidth: 7.5 * scale, lineCap: .round, lineJoin: .round))
        }
        if letter == "i" {
            let dot = CGRect(x: origin.x + (penX + 6.5) * scale, y: origin.y + (17 + bounce) * scale,
                             width: 5.4 * scale, height: 5.4 * scale)
            context.fill(Path(ellipseIn: dot), with: .color(color))
        }
        penX += advance
    }
    return penX * scale
}

func handWordWidth(_ word: String) -> CGFloat {
    word.reduce(1) { $0 + letterStrokes($1).advance }
}

/// "chippytea", written by hand in ink — the wordmark half of the shop sign.
struct HandWordmark: View {
    var height: CGFloat = 23
    var color: Color = TeaTheme.ink
    var boil: Int = 0

    var body: some View {
        let scale = height / 92
        Canvas { context, _ in
            var ctx = context
            paintHandWord("chippytea", in: &ctx, origin: .zero, scale: scale, color: color, seed: 500 + boil * 9)
        }
        .frame(width: (handWordWidth("chippytea") + 1) * scale, height: height)
        .accessibilityHidden(true)
    }
}

/// The menu-bar icon: the same little fish, ink strokes only, monochrome.
func fishTemplateImage(size: CGFloat = 18) -> NSImage {
    let image = NSImage(size: NSSize(width: size, height: size), flipped: true) { rect in
        guard let cg = NSGraphicsContext.current?.cgContext else { return true }
        let s = rect.width / 64
        cg.translateBy(x: 0, y: (rect.height - 44 * s) / 2)
        cg.scaleBy(x: s, y: s)
        cg.setStrokeColor(NSColor.black.cgColor)
        cg.setLineCap(.round)
        cg.setLineJoin(.round)
        cg.setLineWidth(5.5)
        cg.addPath(handPath(fishBodyPoints, closed: true, amplitude: 1.1, seed: 401).cgPath)
        cg.strokePath()
        cg.setLineWidth(4)
        cg.addPath(handPath([CGPoint(x: 17, y: 19), CGPoint(x: 19, y: 24), CGPoint(x: 17, y: 29)], amplitude: 0.5, seed: 406).cgPath)
        cg.strokePath()
        cg.setFillColor(NSColor.black.cgColor)
        cg.fillEllipse(in: CGRect(x: 9, y: 19, width: 6.5, height: 6.5))
        return true
    }
    image.isTemplate = true
    return image
}

// MARK: - Hand-lettered numerals

private struct GlyphStroke { let points: [CGPoint]; let closed: Bool }

/// Casual letterforms on a 60 × 92 box. Centre-lines only; the drawn weight
/// comes from stroking them, which is then gold-hatched and ink-outlined.
private func glyphStrokes(_ symbol: Int) -> [GlyphStroke] {
    switch symbol {
    case 0:
        return [GlyphStroke(points: (0..<15).map { index in
            let angle = -.pi / 2 + .pi * 2 * CGFloat(index) / 15
            return CGPoint(x: 30 + cos(angle) * 18, y: 47 + sin(angle) * 41)
        }, closed: true)]
    case 1:
        return [GlyphStroke(points: [CGPoint(x: 11, y: 25), CGPoint(x: 22, y: 14), CGPoint(x: 32, y: 7), CGPoint(x: 32, y: 46), CGPoint(x: 31, y: 85)], closed: false),
                GlyphStroke(points: [CGPoint(x: 14, y: 87), CGPoint(x: 30, y: 85), CGPoint(x: 48, y: 86)], closed: false)]
    case 2:
        return [GlyphStroke(points: [CGPoint(x: 9, y: 26), CGPoint(x: 14, y: 13), CGPoint(x: 28, y: 6), CGPoint(x: 44, y: 10),
                                     CGPoint(x: 51, y: 24), CGPoint(x: 44, y: 39), CGPoint(x: 30, y: 52), CGPoint(x: 16, y: 66),
                                     CGPoint(x: 8, y: 85), CGPoint(x: 31, y: 84), CGPoint(x: 53, y: 85)], closed: false)]
    case 3:
        return [GlyphStroke(points: [CGPoint(x: 11, y: 18), CGPoint(x: 25, y: 7), CGPoint(x: 43, y: 10), CGPoint(x: 51, y: 23),
                                     CGPoint(x: 43, y: 37), CGPoint(x: 29, y: 43), CGPoint(x: 44, y: 47), CGPoint(x: 53, y: 61),
                                     CGPoint(x: 48, y: 78), CGPoint(x: 30, y: 88), CGPoint(x: 11, y: 81)], closed: false)]
    case 4:
        return [GlyphStroke(points: [CGPoint(x: 44, y: 8), CGPoint(x: 26, y: 36), CGPoint(x: 7, y: 63), CGPoint(x: 31, y: 63), CGPoint(x: 56, y: 62)], closed: false),
                GlyphStroke(points: [CGPoint(x: 42, y: 27), CGPoint(x: 41, y: 60), CGPoint(x: 41, y: 88)], closed: false)]
    case 5:
        return [GlyphStroke(points: [CGPoint(x: 51, y: 9), CGPoint(x: 30, y: 10), CGPoint(x: 15, y: 11), CGPoint(x: 12, y: 42),
                                     CGPoint(x: 30, y: 34), CGPoint(x: 48, y: 42), CGPoint(x: 54, y: 61), CGPoint(x: 45, y: 81),
                                     CGPoint(x: 25, y: 88), CGPoint(x: 9, y: 79)], closed: false)]
    case 6:
        return [GlyphStroke(points: [CGPoint(x: 47, y: 10), CGPoint(x: 27, y: 18), CGPoint(x: 13, y: 38), CGPoint(x: 9, y: 60),
                                     CGPoint(x: 16, y: 79), CGPoint(x: 32, y: 88), CGPoint(x: 47, y: 80), CGPoint(x: 50, y: 63),
                                     CGPoint(x: 39, y: 51), CGPoint(x: 23, y: 50), CGPoint(x: 12, y: 60)], closed: false)]
    case 7:
        return [GlyphStroke(points: [CGPoint(x: 8, y: 12), CGPoint(x: 30, y: 9), CGPoint(x: 53, y: 10), CGPoint(x: 40, y: 48), CGPoint(x: 27, y: 88)], closed: false),
                GlyphStroke(points: [CGPoint(x: 17, y: 50), CGPoint(x: 30, y: 48), CGPoint(x: 43, y: 47)], closed: false)]
    case 8:
        return [GlyphStroke(points: (0..<11).map { index in
            let angle = -.pi / 2 + .pi * 2 * CGFloat(index) / 11
            return CGPoint(x: 30 + cos(angle) * 17, y: 27 + sin(angle) * 19)
        }, closed: true),
                GlyphStroke(points: (0..<12).map { index in
            let angle = -.pi / 2 + .pi * 2 * CGFloat(index) / 12
            return CGPoint(x: 30 + cos(angle) * 21, y: 67 + sin(angle) * 22)
        }, closed: true)]
    case 9:
        return [GlyphStroke(points: [CGPoint(x: 49, y: 46), CGPoint(x: 36, y: 53), CGPoint(x: 20, y: 49), CGPoint(x: 11, y: 34),
                                     CGPoint(x: 18, y: 16), CGPoint(x: 36, y: 8), CGPoint(x: 50, y: 18), CGPoint(x: 52, y: 40),
                                     CGPoint(x: 47, y: 63), CGPoint(x: 36, y: 82), CGPoint(x: 20, y: 89)], closed: false)]
    default: // "+"
        return [GlyphStroke(points: [CGPoint(x: 12, y: 48), CGPoint(x: 30, y: 47), CGPoint(x: 48, y: 48)], closed: false),
                GlyphStroke(points: [CGPoint(x: 30, y: 30), CGPoint(x: 30, y: 48), CGPoint(x: 30, y: 66)], closed: false)]
    }
}

private let glyphBox = CGSize(width: 60, height: 92)
let glyphAdvance: CGFloat = 62

/// Draws one hand-lettered glyph: gold body, diagonal pencil hatch, ink outline.
func drawGlyph(_ symbol: Int, in context: inout GraphicsContext, origin: CGPoint, scale: CGFloat, seed: Int,
               body: Color = TeaTheme.gold, hatch: Color = TeaTheme.goldDeep.opacity(0.42), outline: Color = TeaTheme.ink,
               weight: CGFloat = 10.5) {
    var region = Path()
    for (index, stroke) in glyphStrokes(symbol).enumerated() {
        let points = stroke.points.map { CGPoint(x: origin.x + $0.x * scale, y: origin.y + $0.y * scale) }
        let centre = handPath(points, closed: stroke.closed, amplitude: 1.7 * scale, seed: seed &+ index &* 37)
        region.addPath(centre.strokedPath(StrokeStyle(lineWidth: weight * scale, lineCap: .round, lineJoin: .round)))
    }
    context.fill(region, with: .color(body))
    var hatched = context
    hatched.clip(to: region)
    let box = region.boundingRect
    var lines = Path()
    var x = box.minX - box.height
    while x < box.maxX {
        lines.move(to: CGPoint(x: x, y: box.maxY))
        lines.addLine(to: CGPoint(x: x + box.height, y: box.minY))
        x += 4.5
    }
    hatched.stroke(lines, with: .color(hatch), lineWidth: 1)
    context.stroke(region, with: .color(outline), style: StrokeStyle(lineWidth: max(1.2, 1.6 * scale * 2), lineJoin: .round))
}

/// The balance: hand-lettered digits with "chips" written out beside them in
/// ink — "958 chips". Interpolates like a number, draws like a felt-tip.
struct AnimatedChipNumber: View, Animatable {
    var value: Double
    var digitHeight: CGFloat = 42
    var boil: Int = 0

    var animatableData: Double {
        get { value }
        set { value = newValue }
    }
    private var total: UInt64 {
        if value.isNaN || value <= 0 { return 0 }
        // Double rounds UInt64.max up to 2^64, which cannot be converted back.
        if value >= Double(UInt64.max) { return .max }
        return UInt64(value.rounded(.down))
    }
    /// The unit word's height at the reference digit height; it scales with the digits.
    private static let wordHeight: CGFloat = 18

    var body: some View {
        let digits = String(total).compactMap(\.wholeNumberValue)
        let word = total == 1 ? "chip" : "chips"
        let scale = digitHeight / glyphBox.height
        let wordScale = Self.wordHeight * min(1, digitHeight / 42) / 92
        let cell = glyphAdvance * scale
        let gap: CGFloat = 7
        let wordWidth = handWordWidth(word) * wordScale
        let width = CGFloat(digits.count) * cell + wordWidth + gap + 8
        Canvas { context, _ in
            var ctx = context
            var penX: CGFloat = 3
            // The word sits on the digits' baseline, like a unit written after a sum.
            let baseline: CGFloat = 4 + 87 * scale
            let wordY = baseline - 70 * wordScale
            for (index, digit) in digits.enumerated() {
                drawGlyph(digit, in: &ctx, origin: CGPoint(x: penX, y: 4), scale: scale,
                          seed: 700 &+ index &* 31 &+ digit &* 7 &+ boil &* 13)
                penX += cell
            }
            penX += gap
            paintHandWord(word, in: &ctx, origin: CGPoint(x: penX, y: wordY), scale: wordScale,
                          color: TeaTheme.ink, seed: 560 + boil * 9)
        }
        .frame(width: width, height: digitHeight + 10)
        .accessibilityHidden(true)
    }
}

// MARK: - The portion

/// A panel-wide open paper wrap with a heap of doodled chips and a pinch of
/// salt. At most 18 chips are rendered, so the cost stays constant however
/// large the balance grows.
struct ChipPortion: View {
    let chips: UInt64
    var landing = false
    var boil: Int = 0
    /// Draws at a fraction of its design size, for the compact chips strip.
    var scale: CGFloat = 1

    private static let rows: [(count: Int, lift: CGFloat, spread: CGFloat, length: CGFloat)] = [
        (6, 0, 0.52, 36), (5, 12, 0.43, 34), (4, 23, 0.33, 33), (2, 33, 0.20, 31), (1, 42, 0.0, 30)
    ]

    var body: some View {
        Canvas { canvas, size in
            var context = canvas
            context.scaleBy(x: scale, y: scale)
            let width = size.width / scale
            let baseY = size.height / scale * 0.66
            // At most eighteen chips are rendered, so the cost stays constant
            // however large the balance grows.
            let drawn = min(Int(min(chips, 18)), 18)
            let seed = 200 + boil * 7

            // The wrap behind the heap: unfolded paper, corners poking up.
            var sheet: [CGPoint] = [
                CGPoint(x: width * 0.140, y: baseY + 12),
                CGPoint(x: width * 0.072, y: baseY - 34),
                CGPoint(x: width * 0.200, y: baseY - 10),
                CGPoint(x: width * 0.335, y: baseY - 20),
                CGPoint(x: width * 0.470, y: baseY - 7),
                CGPoint(x: width * 0.600, y: baseY - 17),
                CGPoint(x: width * 0.760, y: baseY - 5),
                CGPoint(x: width * 0.930, y: baseY - 44),
                CGPoint(x: width * 0.862, y: baseY + 12)
            ]
            sheet.append(contentsOf: lineSamples(from: CGPoint(x: width * 0.84, y: baseY + 13),
                                                 to: CGPoint(x: width * 0.16, y: baseY + 13), step: 30))
            let back = handPath(sheet, closed: true, amplitude: 1.3, seed: seed &+ 21)
            context.fill(back, with: .color(TeaTheme.card))
            context.stroke(back, with: .color(TeaTheme.ink.opacity(0.75)), style: StrokeStyle(lineWidth: 1.4, lineCap: .round, lineJoin: .round))
            // Yesterday's headlines: faint ruled newsprint on the taller corner,
            // clipped so no line escapes the paper's edge.
            var newsprint = context
            newsprint.clip(to: back)
            for index in 0..<4 {
                let y = baseY - 36 + CGFloat(index) * 5.5
                let inset = CGFloat(index) * 0.006
                let ruled = handPath(lineSamples(from: CGPoint(x: width * (0.860 - inset), y: y), to: CGPoint(x: width * (0.935 + inset), y: y + 1), step: 8),
                                     amplitude: 0.4, seed: seed &+ 30 &+ index)
                newsprint.stroke(ruled, with: .color(TeaTheme.ink.opacity(0.22)), style: StrokeStyle(lineWidth: 1, lineCap: .round))
            }
            // A crease where the wrap was folded.
            let crease = handPath([CGPoint(x: width * 0.105, y: baseY - 14), CGPoint(x: width * 0.15, y: baseY + 4)], amplitude: 0.6, seed: seed &+ 35)
            context.stroke(crease, with: .color(TeaTheme.ink.opacity(0.2)), style: StrokeStyle(lineWidth: 1, lineCap: .round))

            if drawn == 0 {
                var ghost = context
                ghost.translateBy(x: width / 2, y: baseY - 8)
                ghost.rotate(by: .degrees(-9))
                paintChip(&ghost, center: .zero, length: 40, seed: seed &+ 3, pale: true)
            } else {
                var slots: [(point: CGPoint, length: CGFloat, index: Int)] = []
                var order = 0
                for (rowIndex, row) in Self.rows.enumerated() {
                    let span = width * row.spread
                    let ordered = (0..<row.count).sorted { lhs, rhs in
                        let middle = Double(row.count - 1) / 2
                        return abs(Double(lhs) - middle) < abs(Double(rhs) - middle)
                    }
                    for index in ordered {
                        let t = row.count == 1 ? 0.5 : Double(index) / Double(row.count - 1)
                        let x = width / 2 - span / 2 + span * CGFloat(t)
                        let wobble = CGFloat((rowIndex * 7 + index * 13) % 9) - 4
                        slots.append((CGPoint(x: x, y: baseY - row.lift + wobble * 0.4), row.length + inkNoise(order, seed) * 3, order))
                        order += 1
                    }
                }
                for slot in slots.prefix(drawn).sorted(by: { $0.point.y < $1.point.y }) {
                    var chip = context
                    chip.translateBy(x: slot.point.x, y: slot.point.y)
                    chip.rotate(by: .degrees(Double((slot.index * 29) % 44) - 22))
                    paintChip(&chip, center: .zero, length: slot.length, seed: seed &+ slot.index &* 3)
                }
                // A pinch of salt over the heap.
                for index in 0..<6 {
                    let x = width * (0.36 + CGFloat(index) * 0.055) + inkNoise(index, seed) * 5
                    let y = baseY - 50 - CGFloat((index * 11) % 14) + inkNoise(index &+ 20, seed) * 3
                    context.fill(Path(ellipseIn: CGRect(x: x, y: y, width: 1.8, height: 1.8)), with: .color(TeaTheme.ink.opacity(0.4)))
                }
            }

            // The front fold, hiding the bottoms of the chips: they sit in the wrap.
            var lip: [CGPoint] = lineSamples(from: CGPoint(x: width * 0.115, y: baseY + 5), to: CGPoint(x: width * 0.885, y: baseY + 3), step: 24)
            lip.append(contentsOf: [CGPoint(x: width * 0.845, y: baseY + 25), CGPoint(x: width * 0.50, y: baseY + 28), CGPoint(x: width * 0.16, y: baseY + 26)])
            let front = handPath(lip, closed: true, amplitude: 1.2, seed: seed &+ 40)
            context.fill(front, with: .color(TeaTheme.paperDeep))
            context.stroke(front, with: .color(TeaTheme.ink.opacity(0.8)), style: StrokeStyle(lineWidth: 1.5, lineCap: .round, lineJoin: .round))

            drawAsterisk(&context, at: CGPoint(x: width * 0.20, y: baseY - 46), radius: 6.5, color: TeaTheme.gold, seed: seed &+ 1)
            drawAsterisk(&context, at: CGPoint(x: width * 0.80, y: baseY - 52), radius: 4.5, color: TeaTheme.ink.opacity(0.45), seed: seed &+ 2)
        }
        .scaleEffect(x: landing ? 1.03 : 1, y: landing ? 0.96 : 1, anchor: .bottom)
        .accessibilityHidden(true)
    }
}

func drawAsterisk(_ context: inout GraphicsContext, at center: CGPoint, radius: CGFloat, color: Color, seed: Int) {
    for index in 0..<3 {
        let angle = CGFloat(index) * .pi / 3 + inkNoise(index, seed) * 0.15
        var stroke = Path()
        stroke.move(to: CGPoint(x: center.x - cos(angle) * radius, y: center.y - sin(angle) * radius))
        stroke.addQuadCurve(to: CGPoint(x: center.x + cos(angle) * radius, y: center.y + sin(angle) * radius),
                            control: CGPoint(x: center.x + inkNoise(index &+ 9, seed) * 1.4, y: center.y + inkNoise(index &+ 17, seed) * 1.4))
        context.stroke(stroke, with: .color(color), style: StrokeStyle(lineWidth: max(1, radius * 0.24), lineCap: .round))
    }
}

// MARK: - Collection burst

/// Exists only during a collection; there is no display link or animation timer at idle.
/// Chips tumble out from under the tape and fall into the wrap, trailing motion lines.
struct ChipCollectionOverlay: View {
    let amount: UInt64
    var anchorX: CGFloat
    @State private var started = Date()

    var body: some View {
        TimelineView(.animation(minimumInterval: 1.0 / 60.0)) { timeline in
            let elapsed = timeline.date.timeIntervalSince(started)
            Canvas { context, size in
                drawParticles(context: &context, size: size, elapsed: elapsed)
                drawTally(context: &context, size: size, elapsed: elapsed)
            }
        }
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }

    private func point(_ entry: CGPoint, _ control: CGPoint, _ end: CGPoint, _ t: CGFloat) -> CGPoint {
        let u = 1 - t
        return CGPoint(x: u * u * entry.x + 2 * u * t * control.x + t * t * end.x,
                       y: u * u * entry.y + 2 * u * t * control.y + t * t * end.y)
    }

    private func drawParticles(context: inout GraphicsContext, size: CGSize, elapsed: TimeInterval) {
        let count = min(24, max(6, Int(min(amount, 24))))
        let entry = CGPoint(x: min(max(anchorX, 24), size.width - 24), y: -6)
        for index in 0..<count {
            let delay = Double(index) * 0.034
            let progress = min(1, max(0, (elapsed - delay) / 0.86))
            guard progress > 0 && progress < 1 else { continue }
            let t = CGFloat(progress)
            let seed = CGFloat((index * 73 + 19) % 101) / 101
            let end = CGPoint(x: size.width * (0.17 + seed * 0.66), y: size.height * (0.80 + seed * 0.12))
            let control = CGPoint(x: entry.x + (end.x - entry.x) * (0.15 + seed * 0.35), y: size.height * (0.20 + seed * 0.16))
            let here = point(entry, control, end, t)
            let fade = min(1, progress * 9) * min(1, (1 - progress) * 9)
            let chipLength = 17 + seed * 10

            // Sketched motion lines trailing the chip.
            var trails = context
            trails.opacity = fade * 0.55
            for line in 0..<3 {
                let lag = CGFloat(0.05 + Double(line) * 0.035)
                let from = point(entry, control, end, max(0, t - lag - 0.05))
                let to = point(entry, control, end, max(0, t - lag))
                guard hypot(to.x - from.x, to.y - from.y) > 0.6 else { continue }
                let offset = (CGFloat(line) - 1) * chipLength * 0.24
                var trail = Path()
                trail.move(to: CGPoint(x: from.x + offset, y: from.y))
                trail.addQuadCurve(to: CGPoint(x: to.x + offset, y: to.y),
                                   control: CGPoint(x: (from.x + to.x) / 2 + offset * 1.4, y: (from.y + to.y) / 2))
                trails.stroke(trail, with: .color(TeaTheme.ink.opacity(0.40)), style: StrokeStyle(lineWidth: 1.2, lineCap: .round))
            }

            var particle = context
            particle.opacity = fade
            particle.translateBy(x: here.x, y: here.y)
            particle.rotate(by: .degrees(progress * 250 + Double(index * 23)))
            paintChip(&particle, center: .zero, length: chipLength, seed: 300 &+ index &* 5 &+ Int(elapsed * 6))
        }
    }

    /// A scrawled "+N" that pops in beside the balance and fades away.
    private func drawTally(context: inout GraphicsContext, size: CGSize, elapsed: TimeInterval) {
        let appear = min(1, max(0, (elapsed - 0.14) / 0.22))
        let leave = min(1, max(0, (elapsed - 1.35) / 0.45))
        guard appear > 0, leave < 1 else { return }
        let pop: CGFloat = appear < 1 ? 0.55 + appear * 0.60 : 1 - 0.05 * CGFloat(sin(min(1, (elapsed - 0.36) * 8)))
        var tally = context
        tally.opacity = Double(appear) * Double(1 - leave)
        tally.translateBy(x: min(size.width - 70, TeaTheme.panelPadding + 108), y: 22)
        tally.rotate(by: .degrees(-7))
        tally.scaleBy(x: pop, y: pop)
        let scale: CGFloat = 0.24
        let cell = glyphAdvance * scale
        var index = 0
        drawGlyph(10, in: &tally, origin: .zero, scale: scale, seed: 640, weight: 13)
        index += 1
        for character in String(amount) {
            guard let digit = character.wholeNumberValue else { continue }
            drawGlyph(digit, in: &tally, origin: CGPoint(x: CGFloat(index) * cell, y: 0), scale: scale, seed: 660 &+ index &* 11, weight: 13)
            index += 1
        }
    }
}

// MARK: - Doodles

/// A steaming mug — the app is called chippytea.
struct MugDoodle: View {
    var size: CGFloat = 56
    var body: some View {
        Canvas { context, dimensions in
            let s = min(dimensions.width, dimensions.height) / 56
            var ctx = context
            ctx.scaleBy(x: s, y: s)
            let ink = TeaTheme.ink.opacity(0.8)
            let stroke = StrokeStyle(lineWidth: 1.8, lineCap: .round, lineJoin: .round)
            let body = handPath([CGPoint(x: 12, y: 24), CGPoint(x: 14, y: 42), CGPoint(x: 20, y: 50), CGPoint(x: 36, y: 50),
                                 CGPoint(x: 42, y: 42), CGPoint(x: 44, y: 24), CGPoint(x: 28, y: 22), CGPoint(x: 12, y: 24)],
                                closed: true, amplitude: 0.8, seed: 71)
            ctx.fill(body, with: .color(TeaTheme.card))
            ctx.stroke(body, with: .color(ink), style: stroke)
            let handle = handPath(arcSamples(center: CGPoint(x: 45, y: 33), radius: 8, from: -1.1, to: 1.1, count: 7), amplitude: 0.6, seed: 73)
            ctx.stroke(handle, with: .color(ink), style: stroke)
            let tea = handPath(lineSamples(from: CGPoint(x: 15, y: 29), to: CGPoint(x: 41, y: 28), step: 9), amplitude: 0.7, seed: 75)
            ctx.stroke(tea, with: .color(TeaTheme.goldDeep.opacity(0.75)), style: StrokeStyle(lineWidth: 1.6, lineCap: .round))
            let saucer = handPath(lineSamples(from: CGPoint(x: 8, y: 53), to: CGPoint(x: 48, y: 53), step: 12), amplitude: 0.7, seed: 77)
            ctx.stroke(saucer, with: .color(ink), style: stroke)
            for index in 0..<3 {
                let x = 19 + CGFloat(index) * 9
                let steam = handPath([CGPoint(x: x, y: 17), CGPoint(x: x + 4, y: 11), CGPoint(x: x - 2, y: 6), CGPoint(x: x + 3, y: 1)],
                                     amplitude: 0.5, seed: 79 + index)
                ctx.stroke(steam, with: .color(TeaTheme.ink.opacity(0.4)), style: StrokeStyle(lineWidth: 1.5, lineCap: .round))
            }
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }
}

/// The startup disk as the shop's mug of tea: the tea level is the room left
/// on the drive, poured in when the page appears, steam boiling while the
/// panel is open. The numbers beside it carry the actual measurement.
struct StorageMug: View, Animatable {
    var fraction: Double
    var size: CGFloat = 44
    var boil: Int = 0

    var animatableData: Double {
        get { fraction }
        set { fraction = newValue }
    }

    var body: some View {
        Canvas { context, dimensions in
            let s = min(dimensions.width, dimensions.height) / 56
            var ctx = context
            ctx.scaleBy(x: s, y: s)
            let seed = 851 + boil * 7
            let ink = TeaTheme.ink.opacity(0.8)
            let stroke = StrokeStyle(lineWidth: 1.8, lineCap: .round, lineJoin: .round)
            let level = max(0, min(1, fraction))
            let mug = handPath([CGPoint(x: 12, y: 24), CGPoint(x: 14, y: 42), CGPoint(x: 20, y: 50), CGPoint(x: 36, y: 50),
                                CGPoint(x: 42, y: 42), CGPoint(x: 44, y: 24), CGPoint(x: 28, y: 22), CGPoint(x: 12, y: 24)],
                               closed: true, amplitude: 0.8, seed: 71)
            ctx.fill(mug, with: .color(TeaTheme.card))
            if level > 0.02 {
                // Full mug, full drive of room: the surface sits just under the
                // rim at 1 and on the mug's floor at 0.
                let surfaceY = 47 - (47 - 26) * level
                var tea = ctx
                tea.clip(to: mug)
                tea.fill(Path(CGRect(x: 10, y: surfaceY, width: 36, height: 52 - surfaceY)),
                         with: .color(TeaTheme.gold.opacity(0.42)))
                // A brewing swirl in the body of the tea.
                let swirl = handPath(arcSamples(center: CGPoint(x: 28, y: surfaceY + 9), radius: 6,
                                                from: .pi * 0.15, to: .pi * 1.1, count: 6),
                                     amplitude: 0.7, seed: seed &+ 3)
                tea.stroke(swirl, with: .color(TeaTheme.goldDeep.opacity(0.35)), style: StrokeStyle(lineWidth: 1.4, lineCap: .round))
                let surface = handPath(lineSamples(from: CGPoint(x: 12, y: surfaceY), to: CGPoint(x: 44, y: surfaceY), step: 8),
                                       amplitude: 0.9, seed: seed)
                tea.stroke(surface, with: .color(TeaTheme.goldDeep.opacity(0.8)), style: StrokeStyle(lineWidth: 1.6, lineCap: .round))
            }
            ctx.stroke(mug, with: .color(ink), style: stroke)
            let handle = handPath(arcSamples(center: CGPoint(x: 45, y: 33), radius: 8, from: -1.1, to: 1.1, count: 7), amplitude: 0.6, seed: 73)
            ctx.stroke(handle, with: .color(ink), style: stroke)
            let saucer = handPath(lineSamples(from: CGPoint(x: 8, y: 53), to: CGPoint(x: 48, y: 53), step: 12), amplitude: 0.7, seed: 77)
            ctx.stroke(saucer, with: .color(ink), style: stroke)
            for index in 0..<3 {
                let x = 19 + CGFloat(index) * 9
                let steam = handPath([CGPoint(x: x, y: 17), CGPoint(x: x + 4, y: 11), CGPoint(x: x - 2, y: 6), CGPoint(x: x + 3, y: 1)],
                                     amplitude: 0.6, seed: seed &+ 21 &+ index)
                ctx.stroke(steam, with: .color(TeaTheme.ink.opacity(0.4)), style: StrokeStyle(lineWidth: 1.5, lineCap: .round))
            }
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }
}

/// A magnifying glass doodle for the discovery empties.
struct MagnifierDoodle: View {
    var size: CGFloat = 56
    var body: some View {
        Canvas { context, dimensions in
            let s = min(dimensions.width, dimensions.height) / 56
            var ctx = context
            ctx.scaleBy(x: s, y: s)
            let ink = TeaTheme.ink.opacity(0.8)
            let lens = handPath(circleSamples(center: CGPoint(x: 24, y: 23), radius: 15, count: 14), closed: true, amplitude: 0.8, seed: 81)
            ctx.fill(lens, with: .color(TeaTheme.card))
            ctx.stroke(lens, with: .color(ink), style: StrokeStyle(lineWidth: 1.9, lineCap: .round))
            let handle = handPath(lineSamples(from: CGPoint(x: 35, y: 34), to: CGPoint(x: 50, y: 50), step: 8), amplitude: 0.6, seed: 83)
            ctx.stroke(handle, with: .color(ink), style: StrokeStyle(lineWidth: 3.2, lineCap: .round))
            let glint = handPath(arcSamples(center: CGPoint(x: 24, y: 23), radius: 9, from: .pi * 1.05, to: .pi * 1.5, count: 5), amplitude: 0.5, seed: 85)
            ctx.stroke(glint, with: .color(TeaTheme.biro.opacity(0.65)), style: StrokeStyle(lineWidth: 1.6, lineCap: .round))
            drawAsterisk(&ctx, at: CGPoint(x: 46, y: 13), radius: 5, color: TeaTheme.gold, seed: 87)
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }
}

// MARK: - Controls

enum InkButtonKind { case primary, quiet, destructive }

/// Every button is a drawn box. Pressing re-seeds the jitter, so it looks re-drawn.
struct InkButtonStyle: ButtonStyle {
    var kind: InkButtonKind = .quiet
    var fullWidth = false
    var compact = false
    var seed: Int = 31

    func makeBody(configuration: Configuration) -> some View {
        Chrome(configuration: configuration, kind: kind, fullWidth: fullWidth, compact: compact, seed: seed)
    }

    private struct Chrome: View {
        let configuration: Configuration
        let kind: InkButtonKind
        let fullWidth: Bool
        let compact: Bool
        let seed: Int
        @Environment(\.isEnabled) private var enabled
        @State private var hovering = false

        private var activeSeed: Int { seed &+ (configuration.isPressed ? 4 : 0) &+ (hovering ? 2 : 0) }
        private var line: Color {
            guard enabled else { return TeaTheme.inkSoft.opacity(0.35) }
            switch kind {
            case .primary: return TeaTheme.ink
            case .quiet: return hovering ? TeaTheme.ink : TeaTheme.ink.opacity(0.8)
            case .destructive: return TeaTheme.rust
            }
        }
        private var label: Color {
            guard enabled else { return TeaTheme.inkSoft.opacity(0.5) }
            switch kind {
            case .primary, .quiet: return TeaTheme.ink
            case .destructive: return TeaTheme.rust
            }
        }

        var body: some View {
            let shape = WobblyRect(radius: TeaTheme.controlRadius, amplitude: 0.9, seed: activeSeed, step: 9)
            configuration.label
                .font(compact ? TeaFont.control : TeaFont.bodySemibold)
                .foregroundStyle(label)
                .padding(.horizontal, compact ? 10 : 14).padding(.vertical, compact ? 6 : 10)
                .frame(maxWidth: fullWidth ? .infinity : nil)
                .background {
                    ZStack {
                        switch kind {
                        case .primary:
                            shape.fill(enabled ? TeaTheme.gold : TeaTheme.gold.opacity(0.25))
                            ScribbleHatch(shape: shape, color: TeaTheme.goldDeep.opacity(enabled ? 0.30 : 0.10), spacing: 5, seed: activeSeed)
                        case .quiet:
                            shape.fill(hovering && enabled ? TeaTheme.paperDeep : TeaTheme.card.opacity(0.9))
                        case .destructive:
                            shape.fill(hovering && enabled ? TeaTheme.rust.opacity(0.10) : TeaTheme.card.opacity(0.9))
                        }
                    }
                }
                .overlay(shape.stroke(line, style: StrokeStyle(lineWidth: TeaTheme.inkLine, lineCap: .round, lineJoin: .round)))
                .scaleEffect(configuration.isPressed ? 0.98 : 1)
                .contentShape(Rectangle())
                .onHover { hovering = $0 }
        }
    }
}

/// A small drawn pill. Active chips are ringed and lettered in ballpoint blue.
struct ChipButtonStyle: ButtonStyle {
    var active = false
    var seed: Int = 51

    func makeBody(configuration: Configuration) -> some View { Chrome(configuration: configuration, active: active, seed: seed) }

    private struct Chrome: View {
        let configuration: Configuration
        let active: Bool
        let seed: Int
        @State private var hovering = false
        var body: some View {
            let shape = WobblyPill(seed: seed &+ (configuration.isPressed ? 3 : 0) &+ (hovering ? 1 : 0))
            configuration.label
                .font(TeaFont.control)
                .foregroundStyle(active ? TeaTheme.biro : TeaTheme.inkSoft)
                .padding(.horizontal, 11).padding(.vertical, 5)
                .background(shape.fill(active ? TeaTheme.biro.opacity(0.07) : (hovering ? TeaTheme.paperDeep : .clear)))
                .overlay(shape.stroke(active ? TeaTheme.biro : TeaTheme.ink.opacity(0.35),
                                      style: StrokeStyle(lineWidth: active ? 1.5 : 1.1, lineCap: .round, lineJoin: .round)))
                .scaleEffect(configuration.isPressed ? 0.97 : 1)
                .contentShape(Capsule())
                .onHover { hovering = $0 }
        }
    }
}

/// A 26 × 26 glyph button, ringed on hover.
struct InkIconButtonStyle: ButtonStyle {
    var seed: Int = 61
    func makeBody(configuration: Configuration) -> some View { Chrome(configuration: configuration, seed: seed) }

    private struct Chrome: View {
        let configuration: Configuration
        let seed: Int
        @Environment(\.isEnabled) private var enabled
        @State private var hovering = false
        var body: some View {
            let shape = WobblyRect(radius: 7, amplitude: 0.8, seed: seed &+ (configuration.isPressed ? 3 : 0), step: 7)
            configuration.label
                .font(.system(size: 12, weight: .semibold, design: .rounded))
                .foregroundStyle(enabled ? TeaTheme.ink : TeaTheme.inkSoft.opacity(0.45))
                .frame(width: 26, height: 26)
                .background(shape.fill(hovering && enabled ? TeaTheme.paperDeep : .clear))
                .overlay(shape.stroke(hovering && enabled ? TeaTheme.ink.opacity(0.55) : TeaTheme.ink.opacity(0.25), lineWidth: 1.1))
                .scaleEffect(configuration.isPressed ? 0.94 : 1)
                .contentShape(Rectangle())
                .onHover { hovering = $0 }
        }
    }
}

/// Chrome for `Menu`, which cannot take a `ButtonStyle`.
struct InkMenuChrome: ViewModifier {
    var seed: Int = 63
    @State private var hovering = false
    func body(content: Content) -> some View {
        let shape = WobblyRect(radius: 7, amplitude: 0.8, seed: seed, step: 7)
        content
            .font(.system(size: 12, weight: .semibold, design: .rounded))
            .frame(width: 26, height: 26)
            .background(shape.fill(hovering ? TeaTheme.paperDeep : .clear))
            .overlay(shape.stroke(hovering ? TeaTheme.ink.opacity(0.55) : TeaTheme.ink.opacity(0.25), lineWidth: 1.1))
            .onHover { hovering = $0 }
    }
}

extension View {
    func inkMenuChrome(seed: Int = 63) -> some View { modifier(InkMenuChrome(seed: seed)) }
}

/// The tick that draws itself.
private struct CheckStroke: Shape {
    func path(in rect: CGRect) -> Path {
        handPath([CGPoint(x: rect.minX + rect.width * 0.18, y: rect.minY + rect.height * 0.52),
                  CGPoint(x: rect.minX + rect.width * 0.42, y: rect.minY + rect.height * 0.78),
                  CGPoint(x: rect.minX + rect.width * 0.86, y: rect.minY + rect.height * 0.18)],
                 amplitude: 0.5, seed: 95)
    }
}

/// A hand-drawn box; ticking it draws an ink checkmark.
struct InkCheckboxStyle: ToggleStyle {
    var seed: Int = 91
    var showsLabel = false

    func makeBody(configuration: Configuration) -> some View {
        Button { configuration.isOn.toggle() } label: {
            HStack(spacing: 7) {
                Chrome(on: configuration.isOn, seed: seed)
                if showsLabel { configuration.label }
            }
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(configuration.isOn ? [.isSelected] : [])
    }

    private struct Chrome: View {
        let on: Bool
        let seed: Int
        @Environment(\.isEnabled) private var enabled
        @State private var hovering = false
        var body: some View {
            let shape = WobblyRect(radius: 3.5, amplitude: 0.7, seed: seed &+ (hovering ? 2 : 0), step: 5)
            ZStack {
                shape.fill(on ? TeaTheme.biro.opacity(0.10) : (hovering ? TeaTheme.paperDeep : TeaTheme.card))
                shape.stroke(enabled ? TeaTheme.ink.opacity(on ? 0.85 : 0.55) : TeaTheme.inkSoft.opacity(0.3),
                             style: StrokeStyle(lineWidth: 1.3, lineCap: .round, lineJoin: .round))
                CheckStroke()
                    .trim(from: 0, to: on ? 1 : 0)
                    .stroke(TeaTheme.biro, style: StrokeStyle(lineWidth: 2.1, lineCap: .round, lineJoin: .round))
            }
            .frame(width: 16, height: 16)
            .opacity(enabled ? 1 : 0.5)
            .animation(.easeOut(duration: 0.15), value: on)
            .contentShape(Rectangle())
            .onHover { hovering = $0 }
        }
    }
}

/// Progress toward the next chip: one faint ruled line with a gold scribble drawn
/// over the part already earned. No labels — the caption above it says the numbers.
struct ChipProgressMeter: View {
    let fraction: Double
    var seed: Int = 101

    var body: some View {
        GeometryReader { geometry in
            ZStack(alignment: .leading) {
                WobblyLine(amplitude: 0.5, seed: seed)
                    .stroke(TeaTheme.ink.opacity(0.20), style: StrokeStyle(lineWidth: 1.4, lineCap: .round))
                if fraction > 0.005 {
                    WobblyLine(amplitude: 0.8, seed: seed &+ 3)
                        .stroke(TeaTheme.gold, style: StrokeStyle(lineWidth: 3, lineCap: .round))
                        .frame(width: max(6, geometry.size.width * min(1, fraction)))
                }
            }
            .frame(width: geometry.size.width, height: 4)
        }
        .frame(height: 4)
        .accessibilityHidden(true)
    }
}

/// A screen title with a single hand-drawn gold underline.
struct ScreenTitle: View {
    let text: String
    var seed: Int = 121
    var font: Font = TeaFont.title

    var body: some View {
        Text(text)
            .font(font).foregroundStyle(TeaTheme.ink)
            .overlay(alignment: .bottom) {
                WobblyLine(amplitude: 0.9, seed: seed)
                    .stroke(TeaTheme.gold, style: StrokeStyle(lineWidth: 2.6, lineCap: .round))
                    .frame(height: 4)
                    .rotationEffect(.degrees(-0.5))
                    .offset(y: 5)
            }
            .padding(.bottom, 3)
    }
}
