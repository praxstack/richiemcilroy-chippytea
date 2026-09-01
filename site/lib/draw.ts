// Canvas painters for the live parts of the page — the chip, the wrap, the
// hand-lettered balance, the collect burst. Ported from CoinScene.swift.

import {
  Pt,
  tea,
  inkA,
  goldDeepA,
  inkSoftA,
  inkNoise,
  handPath2D,
  lineSamples,
  arcSamples,
  roundedRectSamples,
  fishAndChips,
} from "./ink";
import { fishBodyPoints, letterStrokes, handWordWidth, glyphStrokes, glyphAdvance } from "./art";

type Ctx = CanvasRenderingContext2D;

function stroke(ctx: Ctx, path: Path2D, color: string, width: number) {
  ctx.strokeStyle = color;
  ctx.lineWidth = width;
  ctx.lineCap = "round";
  ctx.lineJoin = "round";
  ctx.stroke(path);
}

// MARK: - The chip

/// One hand-cut chip, drawn lying flat about (cx, cy). Rotate the context to
/// angle it. The pale version is the dashed ghost of a chip yet to come.
export function paintChip(ctx: Ctx, cx: number, cy: number, length: number, seed: number, pale = false) {
  const thickness = length * 0.3;
  const rx = cx - length / 2;
  const ry = cy - thickness / 2;
  const body = handPath2D(
    roundedRectSamples(rx, ry, length, thickness, thickness * 0.42, 5),
    true,
    length * 0.03,
    seed
  );
  if (pale) {
    ctx.save();
    ctx.setLineDash([length * 0.14, length * 0.11]);
    stroke(ctx, body, inkSoftA(0.55), Math.max(1, length * 0.05));
    ctx.restore();
    return;
  }
  ctx.fillStyle = tea.gold;
  ctx.fill(body);
  ctx.save();
  ctx.clip(body);
  // Two fried streaks running the length, and a crisper tip where the fryer caught it.
  for (let lane = 0; lane < 2; lane++) {
    const y = cy + (lane === 0 ? -1 : 1) * thickness * 0.28;
    const streak = new Path2D();
    streak.moveTo(rx + length * 0.1, y + inkNoise(lane, seed) * 0.8);
    streak.quadraticCurveTo(
      cx,
      y + inkNoise(lane + 9, seed) * 1.8,
      rx + length - length * 0.1,
      y + inkNoise(lane + 5, seed) * 0.8
    );
    stroke(ctx, streak, goldDeepA(lane === 0 ? 0.3 : 0.48), Math.max(0.9, thickness * 0.16));
  }
  const tip = handPath2D(
    arcSamples({ x: rx + length - thickness * 0.45, y: cy }, thickness * 0.32, -Math.PI / 2, Math.PI / 2, 5),
    false,
    0.5,
    seed + 3
  );
  stroke(ctx, tip, goldDeepA(0.55), Math.max(0.9, thickness * 0.18));
  ctx.restore();
  stroke(ctx, body, tea.ink, Math.max(1, length * 0.042));
  const shine = new Path2D();
  shine.moveTo(rx + length * 0.15, ry + thickness * 0.3);
  shine.quadraticCurveTo(rx + length * 0.28, ry + thickness * 0.12, rx + length * 0.42, ry + thickness * 0.24);
  stroke(ctx, shine, "rgba(255, 255, 255, 0.8)", Math.max(0.9, thickness * 0.15));
}

// MARK: - The fish

/// The battered fish in its 64 × 44 design box; scale the context first.
export function paintFish(ctx: Ctx, seed: number, steam = true) {
  if (steam) {
    const wisps: Pt[][] = [
      [{ x: 30, y: 9 }, { x: 33, y: 5 }, { x: 29, y: 1 }],
      [{ x: 40, y: 8 }, { x: 43, y: 4 }, { x: 40, y: 1 }],
    ];
    wisps.forEach((wisp, index) => {
      stroke(ctx, handPath2D(wisp, false, 0.5, seed + index), inkA(0.38), 1.3);
    });
  }
  const body = handPath2D(fishBodyPoints, true, 1.1, seed);
  ctx.fillStyle = tea.gold;
  ctx.fill(body);
  ctx.save();
  ctx.clip(body);
  const lines = new Path2D();
  let x = -30;
  while (x < 64) {
    lines.moveTo(x + inkNoise(Math.floor(x), seed) * 0.9, 44);
    lines.quadraticCurveTo(x + 13, 25 + inkNoise(Math.floor(x) + 7, seed) * 2, x + 30, 6);
    x += 4.2;
  }
  stroke(ctx, lines, goldDeepA(0.32), 1);
  ctx.restore();
  stroke(ctx, body, tea.ink, 1.7);
  stroke(ctx, handPath2D([{ x: 49, y: 21 }, { x: 56, y: 15 }], false, 0.4, seed + 3), inkA(0.7), 1.1);
  stroke(ctx, handPath2D([{ x: 49, y: 27 }, { x: 56, y: 33 }], false, 0.4, seed + 4), inkA(0.7), 1.1);
  stroke(ctx, handPath2D([{ x: 17, y: 19 }, { x: 19, y: 24 }, { x: 17, y: 29 }], false, 0.5, seed + 5), inkA(0.55), 1.1);
  ctx.fillStyle = tea.ink;
  ctx.beginPath();
  ctx.ellipse(12, 22, 1.8, 1.8, 0, 0, Math.PI * 2);
  ctx.fill();
  ctx.fillStyle = tea.card;
  ctx.beginPath();
  ctx.ellipse(11.45, 21.45, 0.55, 0.55, 0, 0, Math.PI * 2);
  ctx.fill();
  stroke(ctx, handPath2D([{ x: 22, y: 36 }, { x: 24, y: 39 }, { x: 26, y: 36 }], false, 0.4, seed + 6), inkA(0.5), 1.1);
}

// MARK: - Hand lettering

/// Strokes a word in the 92-unit letter space; returns the pen advance in px.
export function paintHandWord(
  ctx: Ctx,
  word: string,
  originX: number,
  originY: number,
  scale: number,
  color: string,
  seed: number
): number {
  let penX = 1;
  let index = 0;
  for (const letter of word) {
    const { strokes, advance } = letterStrokes(letter);
    const bounce = inkNoise(index, seed) * 1.6;
    strokes.forEach((strokePoints, strokeIndex) => {
      const points = strokePoints.map((p) => ({
        x: originX + (penX + p.x) * scale,
        y: originY + (p.y + bounce) * scale,
      }));
      stroke(ctx, handPath2D(points, false, 1.4, seed + index * 13 + strokeIndex * 5), color, 7.5 * scale);
    });
    if (letter === "i") {
      ctx.fillStyle = color;
      ctx.beginPath();
      ctx.ellipse(
        originX + (penX + 6.5 + 2.7) * scale,
        originY + (17 + bounce + 2.7) * scale,
        2.7 * scale,
        2.7 * scale,
        0,
        0,
        Math.PI * 2
      );
      ctx.fill();
    }
    penX += advance;
    index += 1;
  }
  return penX * scale;
}

// MARK: - Hand-lettered numerals

/// Continuous diagonal pencil hatch over gold, used as the numerals' stroke
/// paint. One tile, cached; drawn at 2x so it stays crisp on retina.
let hatchPattern: CanvasPattern | null = null;
function goldHatch(ctx: Ctx): CanvasPattern | string {
  if (hatchPattern) return hatchPattern;
  const spacing = 4.5;
  const tile = spacing * Math.SQRT2;
  const res = 2;
  const canvas = document.createElement("canvas");
  canvas.width = Math.round(tile * res);
  canvas.height = Math.round(tile * res);
  const tc = canvas.getContext("2d");
  if (!tc) return tea.gold;
  tc.scale(res, res);
  tc.fillStyle = tea.gold;
  tc.fillRect(0, 0, tile, tile);
  tc.strokeStyle = goldDeepA(0.42);
  tc.lineWidth = 1;
  tc.beginPath();
  for (const o of [-tile, 0, tile]) {
    tc.moveTo(o, tile - o);
    tc.lineTo(o + tile, -o);
  }
  tc.stroke();
  const pattern = ctx.createPattern(canvas, "repeat");
  if (!pattern) return tea.gold;
  pattern.setTransform(new DOMMatrix().scale(1 / res));
  hatchPattern = pattern;
  return pattern;
}

/// One hand-lettered glyph: gold body with pencil hatch, ink outline. The
/// outline is a fat ink understroke, so crossings merge exactly as the app's
/// stroked-region outline does.
export function drawGlyph(
  ctx: Ctx,
  symbol: number,
  originX: number,
  originY: number,
  scale: number,
  seed: number,
  weight = 10.5
) {
  const paths = glyphStrokes(symbol).map((s, index) =>
    handPath2D(
      s.points.map((p) => ({ x: originX + p.x * scale, y: originY + p.y * scale })),
      s.closed,
      1.7 * scale,
      seed + index * 37
    )
  );
  const band = weight * scale;
  const outline = Math.max(1.2, 1.6 * scale * 2);
  for (const p of paths) stroke(ctx, p, tea.ink, band + outline);
  const paint = goldHatch(ctx);
  for (const p of paths) {
    ctx.strokeStyle = paint;
    ctx.lineWidth = band;
    ctx.lineCap = "round";
    ctx.lineJoin = "round";
    ctx.stroke(p);
  }
}

/// The balance line: "0 fish 958 chips" — digits drawn like a felt-tip, the
/// denominations written out in ink on the same baseline.
export function paintBalance(ctx: Ctx, value: number, digitHeight: number, boil: number) {
  const order = fishAndChips(value);
  const fishDigits = [...String(order.fish)].map(Number);
  const chipDigits = [...String(order.chips)].map(Number);
  const scale = digitHeight / 92;
  const wordHeight = 18;
  const wordScale = wordHeight / 92;
  const cell = glyphAdvance * scale;
  const gap = 7;
  let penX = 3;
  const baseline = 4 + 87 * scale;
  const wordY = baseline - 70 * wordScale;
  fishDigits.forEach((digit, index) => {
    drawGlyph(ctx, digit, penX, 4, scale, 900 + index * 31 + digit * 7 + boil * 13);
    penX += cell;
  });
  penX += gap;
  paintHandWord(ctx, "fish", penX, wordY, wordScale, tea.ink, 530 + boil * 9);
  penX += handWordWidth("fish") * wordScale + gap * 2;
  chipDigits.forEach((digit, index) => {
    drawGlyph(ctx, digit, penX, 4, scale, 700 + index * 31 + digit * 7 + boil * 13);
    penX += cell;
  });
  penX += gap;
  paintHandWord(ctx, "chips", penX, wordY, wordScale, tea.ink, 560 + boil * 9);
}

// MARK: - The wrap

export function drawAsterisk(ctx: Ctx, at: Pt, radius: number, color: string, seed: number) {
  for (let index = 0; index < 3; index++) {
    const angle = (index * Math.PI) / 3 + inkNoise(index, seed) * 0.15;
    const p = new Path2D();
    p.moveTo(at.x - Math.cos(angle) * radius, at.y - Math.sin(angle) * radius);
    p.quadraticCurveTo(
      at.x + inkNoise(index + 9, seed) * 1.4,
      at.y + inkNoise(index + 17, seed) * 1.4,
      at.x + Math.cos(angle) * radius,
      at.y + Math.sin(angle) * radius
    );
    stroke(ctx, p, color, Math.max(1, radius * 0.24));
  }
}

const wrapRows: { count: number; lift: number; spread: number; length: number }[] = [
  { count: 6, lift: 0, spread: 0.52, length: 36 },
  { count: 5, lift: 12, spread: 0.43, length: 34 },
  { count: 4, lift: 23, spread: 0.33, length: 33 },
  { count: 2, lift: 33, spread: 0.2, length: 31 },
  { count: 1, lift: 42, spread: 0.0, length: 30 },
];

/// The open paper wrap with its heap of chips and a pinch of salt. At most two
/// fish and eighteen chips are rendered, however large the balance grows.
export function paintWrap(ctx: Ctx, width: number, height: number, totalChips: number, boil: number) {
  const baseY = height * 0.66;
  const order = fishAndChips(totalChips);
  const fishDrawn = Math.min(order.fish, 2);
  const drawn = Math.min(order.chips, 18);
  const seed = 200 + boil * 7;

  // The wrap behind the heap: unfolded paper, corners poking up.
  const sheet: Pt[] = [
    { x: width * 0.14, y: baseY + 12 },
    { x: width * 0.072, y: baseY - 34 },
    { x: width * 0.2, y: baseY - 10 },
    { x: width * 0.335, y: baseY - 20 },
    { x: width * 0.47, y: baseY - 7 },
    { x: width * 0.6, y: baseY - 17 },
    { x: width * 0.76, y: baseY - 5 },
    { x: width * 0.93, y: baseY - 44 },
    { x: width * 0.862, y: baseY + 12 },
  ];
  sheet.push(...lineSamples({ x: width * 0.84, y: baseY + 13 }, { x: width * 0.16, y: baseY + 13 }, 30));
  const back = handPath2D(sheet, true, 1.3, seed + 21);
  ctx.fillStyle = tea.card;
  ctx.fill(back);
  stroke(ctx, back, inkA(0.75), 1.4);
  // Yesterday's headlines: faint ruled newsprint on the taller corner.
  ctx.save();
  ctx.clip(back);
  for (let index = 0; index < 4; index++) {
    const y = baseY - 36 + index * 5.5;
    const inset = index * 0.006;
    const ruled = handPath2D(
      lineSamples({ x: width * (0.86 - inset), y }, { x: width * (0.935 + inset), y: y + 1 }, 8),
      false,
      0.4,
      seed + 30 + index
    );
    stroke(ctx, ruled, inkA(0.22), 1);
  }
  ctx.restore();
  // A crease where the wrap was folded.
  stroke(
    ctx,
    handPath2D([{ x: width * 0.105, y: baseY - 14 }, { x: width * 0.15, y: baseY + 4 }], false, 0.6, seed + 35),
    inkA(0.2),
    1
  );

  if (fishDrawn === 0 && drawn === 0) {
    ctx.save();
    ctx.translate(width / 2, baseY - 8);
    ctx.rotate((-9 * Math.PI) / 180);
    paintChip(ctx, 0, 0, 40, seed + 3, true);
    ctx.restore();
  } else {
    // The supper's fish lie in the wrap first, so the chips heap over them.
    for (let index = 0; index < fishDrawn; index++) {
      ctx.save();
      const placement = index === 0
        ? { x: width * 0.38, y: baseY - 26, angle: -8 }
        : { x: width * 0.63, y: baseY - 18, angle: 7 };
      ctx.translate(placement.x, placement.y);
      ctx.rotate((placement.angle * Math.PI) / 180);
      const fishScale = 30 / 44;
      ctx.scale(fishScale, fishScale);
      ctx.translate(-32, -22);
      paintFish(ctx, seed + 60 + index * 9, false);
      ctx.restore();
    }
    const slots: { x: number; y: number; length: number; index: number }[] = [];
    let order2 = 0;
    wrapRows.forEach((row, rowIndex) => {
      const span = width * row.spread;
      const middle = (row.count - 1) / 2;
      const ordered = Array.from({ length: row.count }, (_, i) => i).sort(
        (a, b) => Math.abs(a - middle) - Math.abs(b - middle)
      );
      for (const index of ordered) {
        const t = row.count === 1 ? 0.5 : index / (row.count - 1);
        const x = width / 2 - span / 2 + span * t;
        const wobble = ((rowIndex * 7 + index * 13) % 9) - 4;
        slots.push({
          x,
          y: baseY - row.lift + wobble * 0.4,
          length: row.length + inkNoise(order2, seed) * 3,
          index: order2,
        });
        order2 += 1;
      }
    });
    for (const slot of slots.slice(0, drawn).sort((a, b) => a.y - b.y)) {
      ctx.save();
      ctx.translate(slot.x, slot.y);
      ctx.rotate((((slot.index * 29) % 44) - 22) * (Math.PI / 180));
      paintChip(ctx, 0, 0, slot.length, seed + slot.index * 3);
      ctx.restore();
    }
    // A pinch of salt over the heap.
    ctx.fillStyle = inkA(0.4);
    for (let index = 0; index < 6; index++) {
      const x = width * (0.36 + index * 0.055) + inkNoise(index, seed) * 5;
      const y = baseY - 50 - ((index * 11) % 14) + inkNoise(index + 20, seed) * 3;
      ctx.beginPath();
      ctx.ellipse(x + 0.9, y + 0.9, 0.9, 0.9, 0, 0, Math.PI * 2);
      ctx.fill();
    }
  }

  // The front fold, hiding the bottoms of the chips: they sit in the wrap.
  const lip: Pt[] = lineSamples({ x: width * 0.115, y: baseY + 5 }, { x: width * 0.885, y: baseY + 3 }, 24);
  lip.push(
    { x: width * 0.845, y: baseY + 25 },
    { x: width * 0.5, y: baseY + 28 },
    { x: width * 0.16, y: baseY + 26 }
  );
  const front = handPath2D(lip, true, 1.2, seed + 40);
  ctx.fillStyle = tea.paperDeep;
  ctx.fill(front);
  stroke(ctx, front, inkA(0.8), 1.5);

  drawAsterisk(ctx, { x: width * 0.2, y: baseY - 46 }, 6.5, tea.gold, seed + 1);
  drawAsterisk(ctx, { x: width * 0.8, y: baseY - 52 }, 4.5, inkA(0.45), seed + 2);
}
