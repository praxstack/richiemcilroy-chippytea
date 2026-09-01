// The karaoke backdrop, painted on one canvas behind the lyric: battered fish
// swimming past, chips raining down, fountaining out of the wrap on every
// chorus and flying in wherever the page is tapped, plus a run of sight gags
// cued by the words themselves ("seventeen copies of the same old brew" is
// seventeen mugs). Everything is drawn with the app's ink primitives.

import {
  Pt,
  tea,
  inkA,
  goldDeepA,
  inkNoise,
  handPath2D,
  lineSamples,
  arcSamples,
  circleSamples,
  roundedRectSamples,
} from "./ink";
import { fishArt } from "./art";
import { paintChip, paintWrap, drawAsterisk } from "./draw";
import { lyricLines } from "./lyrics";

type Ctx = CanvasRenderingContext2D;

const GRAVITY = 1500;
const WRAP_W = 340;
const WRAP_H = 180;
const MAX_CHIPS = 90;

export interface SceneFrame {
  /** Seconds into the song. */
  time: number;
  /** performance.now(), for boiling and idle bobbing. */
  now: number;
  /** Seconds since the previous frame. */
  dt: number;
  /** Bass energy 0…1 from the analyser, or a steady stand-in. */
  level: number;
  playing: boolean;
  /** Index into lyricLines of the line on screen. */
  line: number;
  /** Chips heaped in the wrap. */
  wrapChips: number;
  /** Where the hand-lettered score sits, in canvas px, for the biro ring. */
  scoreBox: { x: number; y: number; w: number; h: number } | null;
  reduced: boolean;
}

interface Fish {
  lane: number;
  x: number;
  height: number;
  speed: number;
  phase: number;
}

interface Chip {
  x: number;
  y: number;
  vx: number;
  vy: number;
  rot: number;
  spin: number;
  len: number;
  seed: number;
  /** Canvas y below which the chip has landed and vanishes. */
  floor: number;
  thrown: boolean;
}

interface Gag {
  line: number;
  /** Seconds the gag stays after its line ends. */
  linger: number;
  draw: (ctx: Ctx, t: number, u: number) => void;
}

interface FishPaths {
  body: Path2D;
  batter: Path2D;
  details: { path: Path2D; color: string; width: number }[];
  steam: { path: Path2D; color: string; width: number }[];
  eye: { x: number; y: number; w: number };
  glint: { x: number; y: number; w: number };
}

function stroke(ctx: Ctx, path: Path2D, color: string, width: number) {
  ctx.strokeStyle = color;
  ctx.lineWidth = width;
  ctx.lineCap = "round";
  ctx.lineJoin = "round";
  ctx.stroke(path);
}

/// An underdamped spring's progress toward 1, as SwiftUI does it.
function springTo(t: number, response: number, damping = 0.45) {
  if (t <= 0) return 0;
  const wn = (2 * Math.PI) / response;
  const wd = wn * Math.sqrt(1 - damping * damping);
  return 1 - Math.exp(-damping * wn * t) * (Math.cos(wd * t) + ((damping * wn) / wd) * Math.sin(wd * t));
}

const clamp01 = (v: number) => Math.max(0, Math.min(1, v));

const lineText = (index: number) =>
  index >= 0 && index < lyricLines.length
    ? lyricLines[index].words.map((word) => word.text).join(" ").toLowerCase()
    : "";
const lineStart = (index: number) => lyricLines[index]?.words[0]?.start ?? 0;
const lineEnd = (index: number) => {
  const words = lyricLines[index]?.words;
  return words ? words[words.length - 1].end : 0;
};
const firstLineWith = (phrase: string) => lyricLines.findIndex((_, i) => lineText(i).includes(phrase));

// MARK: - Doodles

function doodleMug(ctx: Ctx, x: number, y: number, size: number, seed: number) {
  const s = size / 56;
  ctx.save();
  ctx.translate(x, y);
  ctx.scale(s, s);
  const body = handPath2D(
    [
      { x: 12, y: 24 }, { x: 14, y: 42 }, { x: 20, y: 50 }, { x: 36, y: 50 },
      { x: 42, y: 42 }, { x: 44, y: 24 }, { x: 28, y: 22 }, { x: 12, y: 24 },
    ],
    true, 0.8, seed
  );
  ctx.fillStyle = tea.card;
  ctx.fill(body);
  stroke(ctx, body, inkA(0.8), 1.8);
  stroke(ctx, handPath2D(arcSamples({ x: 45, y: 33 }, 8, -1.1, 1.1, 7), false, 0.6, seed + 2), inkA(0.8), 1.8);
  stroke(ctx, handPath2D(lineSamples({ x: 15, y: 29 }, { x: 41, y: 28 }, 9), false, 0.7, seed + 4), goldDeepA(0.75), 1.6);
  stroke(ctx, handPath2D(lineSamples({ x: 8, y: 53 }, { x: 48, y: 53 }, 12), false, 0.7, seed + 6), inkA(0.8), 1.8);
  for (let i = 0; i < 2; i++) {
    const sx = 22 + i * 10;
    stroke(
      ctx,
      handPath2D([{ x: sx, y: 17 }, { x: sx + 4, y: 11 }, { x: sx - 2, y: 6 }], false, 0.5, seed + 8 + i),
      inkA(0.4), 1.5
    );
  }
  ctx.restore();
}

function doodleFolder(ctx: Ctx, x: number, y: number, width: number, seed: number, label?: string) {
  const h = width * 0.72;
  const tab = handPath2D(
    [{ x: 0, y: h * 0.16 }, { x: 0, y: 0 }, { x: width * 0.38, y: 0 }, { x: width * 0.46, y: h * 0.16 }],
    false, 0.9, seed
  );
  const body = handPath2D(roundedRectSamples(0, h * 0.14, width, h * 0.86, 4, 9), true, 1, seed + 1);
  ctx.save();
  ctx.translate(x, y);
  ctx.fillStyle = tea.card;
  ctx.fill(body);
  ctx.fillStyle = "rgba(242, 182, 60, 0.55)";
  ctx.fillRect(1, h * 0.14, width - 2, h * 0.16);
  stroke(ctx, tab, tea.ink, 1.5);
  stroke(ctx, body, tea.ink, 1.5);
  if (label) {
    ctx.fillStyle = tea.ink;
    ctx.font = `600 ${Math.max(9, width * 0.13)}px ui-rounded, -apple-system, system-ui, sans-serif`;
    ctx.textAlign = "center";
    ctx.textBaseline = "middle";
    ctx.fillText(label, width / 2, h * 0.64);
  }
  ctx.restore();
}

function doodlePolaroid(ctx: Ctx, x: number, y: number, width: number, tilt: number, seed: number, fish: FishPaths) {
  const h = width * 1.18;
  ctx.save();
  ctx.translate(x, y);
  ctx.rotate(tilt);
  const frame = handPath2D(roundedRectSamples(-width / 2, -h / 2, width, h, 2, 9), true, 1, seed);
  ctx.fillStyle = tea.card;
  ctx.fill(frame);
  const photoW = width * 0.84;
  const photoH = width * 0.8;
  const photo = handPath2D(roundedRectSamples(-photoW / 2, -h / 2 + width * 0.08, photoW, photoH, 1.5, 9), true, 0.7, seed + 1);
  ctx.fillStyle = tea.paperDeep;
  ctx.fill(photo);
  ctx.save();
  ctx.clip(photo);
  // The holiday snap: the fish, on its holidays.
  const scale = (photoW * 0.7) / 64;
  ctx.translate(-photoW * 0.35, -h / 2 + width * 0.08 + photoH * 0.28);
  ctx.scale(scale, scale);
  drawFishPaths(ctx, fish);
  ctx.restore();
  // Sun in the corner.
  ctx.fillStyle = tea.gold;
  ctx.beginPath();
  ctx.arc(photoW * 0.3, -h / 2 + width * 0.2, width * 0.07, 0, Math.PI * 2);
  ctx.fill();
  stroke(ctx, photo, inkA(0.5), 1);
  stroke(ctx, frame, tea.ink, 1.5);
  ctx.restore();
}

function doodleBin(ctx: Ctx, x: number, y: number, width: number, lid: number, seed: number) {
  const h = width * 1.15;
  ctx.save();
  ctx.translate(x, y);
  const body = handPath2D(
    [
      { x: -width * 0.42, y: 0 }, { x: -width * 0.36, y: h }, { x: width * 0.36, y: h }, { x: width * 0.42, y: 0 },
    ],
    true, 1, seed
  );
  ctx.fillStyle = tea.card;
  ctx.fill(body);
  stroke(ctx, body, tea.ink, 1.6);
  for (let i = -1; i <= 1; i++) {
    stroke(
      ctx,
      handPath2D(lineSamples({ x: i * width * 0.18, y: h * 0.15 }, { x: i * width * 0.16, y: h * 0.85 }, 10), false, 0.6, seed + 3 + i),
      inkA(0.35), 1.2
    );
  }
  // The lid, hinged at its left edge.
  ctx.save();
  ctx.translate(-width * 0.5, 0);
  ctx.rotate(-lid);
  const lidPath = handPath2D(roundedRectSamples(0, -width * 0.12, width, width * 0.12, 3, 8), true, 0.8, seed + 7);
  ctx.fillStyle = tea.paperDeep;
  ctx.fill(lidPath);
  stroke(ctx, lidPath, tea.ink, 1.6);
  stroke(ctx, handPath2D(lineSamples({ x: width * 0.4, y: -width * 0.12 }, { x: width * 0.6, y: -width * 0.12 }, 5), false, 0.4, seed + 9), tea.ink, 2);
  ctx.restore();
  ctx.restore();
}

function doodleCursor(ctx: Ctx, x: number, y: number, size: number, pressed: number, seed: number) {
  ctx.save();
  ctx.translate(x, y);
  const s = size / 20;
  ctx.scale(s, s);
  const arrow = handPath2D(
    [{ x: 0, y: 0 }, { x: 0, y: 17 }, { x: 4.5, y: 13 }, { x: 8, y: 20 }, { x: 11, y: 18.5 }, { x: 7.5, y: 12 }, { x: 13, y: 12 }],
    true, 0.5, seed
  );
  ctx.fillStyle = tea.card;
  ctx.fill(arrow);
  stroke(ctx, arrow, tea.ink, 1.6);
  if (pressed > 0) {
    // Click ripples, drawn from the tip.
    for (let ring = 0; ring < 2; ring++) {
      const r = 5 + ring * 4 + pressed * 9;
      const path = handPath2D(circleSamples({ x: 0, y: 0 }, r, 12, ring), true, 0.6, seed + ring);
      stroke(ctx, path, goldDeepA(0.7 * (1 - pressed)), 1.4);
    }
  }
  ctx.restore();
}

function doodleLaptop(ctx: Ctx, x: number, y: number, width: number, seed: number, fish: FishPaths, smile: number) {
  const h = width * 0.62;
  ctx.save();
  ctx.translate(x, y);
  const screen = handPath2D(roundedRectSamples(-width / 2, -h, width, h, 6, 10), true, 1, seed);
  ctx.fillStyle = tea.card;
  ctx.fill(screen);
  stroke(ctx, screen, tea.ink, 1.6);
  const glass = handPath2D(roundedRectSamples(-width / 2 + 6, -h + 6, width - 12, h - 12, 3, 10), true, 0.7, seed + 1);
  ctx.fillStyle = tea.paper;
  ctx.fill(glass);
  stroke(ctx, glass, inkA(0.4), 1);
  ctx.save();
  ctx.clip(glass);
  const scale = (width * 0.42) / 64;
  ctx.translate(-width * 0.21, -h + 12);
  ctx.scale(scale, scale);
  drawFishPaths(ctx, fish);
  ctx.restore();
  // A big satisfied smile under the fish.
  const grin = handPath2D(arcSamples({ x: 0, y: -h * 0.36 }, width * 0.14, Math.PI * 0.15, Math.PI * 0.85, 6), false, 0.6, seed + 4);
  stroke(ctx, grin, tea.ink, 2 * smile + 0.4);
  const base = handPath2D(
    [{ x: -width * 0.6, y: 0 }, { x: -width * 0.56, y: width * 0.08 }, { x: width * 0.56, y: width * 0.08 }, { x: width * 0.6, y: 0 }],
    true, 0.8, seed + 2
  );
  ctx.fillStyle = tea.paperDeep;
  ctx.fill(base);
  stroke(ctx, base, tea.ink, 1.6);
  ctx.restore();
}

function doodleMagnifier(ctx: Ctx, x: number, y: number, r: number, seed: number) {
  ctx.save();
  ctx.translate(x, y);
  const lens = handPath2D(circleSamples({ x: 0, y: 0 }, r, 16), true, 0.9, seed);
  ctx.fillStyle = "rgba(255, 253, 246, 0.7)";
  ctx.fill(lens);
  stroke(ctx, lens, tea.ink, 2);
  stroke(ctx, handPath2D(lineSamples({ x: r * 0.72, y: r * 0.72 }, { x: r * 1.7, y: r * 1.7 }, 6), false, 0.6, seed + 1), tea.ink, r * 0.22);
  stroke(ctx, handPath2D(arcSamples({ x: 0, y: 0 }, r * 0.62, Math.PI * 1.1, Math.PI * 1.5, 4), false, 0.4, seed + 2), inkA(0.35), 1.2);
  ctx.restore();
}

function doodleReceipt(ctx: Ctx, x: number, y: number, width: number, tilt: number, seed: number) {
  const h = width * 1.5;
  ctx.save();
  ctx.translate(x, y);
  ctx.rotate(tilt);
  const edge: Pt[] = [];
  edge.push(...lineSamples({ x: 0, y: 0 }, { x: width, y: 0 }, 10));
  for (let yy = 0; yy <= h; yy += 6) edge.push({ x: width + (yy % 12 === 0 ? 0 : 2), y: yy });
  edge.push(...lineSamples({ x: width, y: h }, { x: 0, y: h }, 10));
  for (let yy = h; yy >= 0; yy -= 6) edge.push({ x: yy % 12 === 0 ? 0 : -2, y: yy });
  const slip = handPath2D(edge, true, 0.6, seed);
  ctx.fillStyle = tea.card;
  ctx.fill(slip);
  stroke(ctx, slip, tea.ink, 1.3);
  for (let i = 0; i < 5; i++) {
    const yy = h * 0.22 + i * h * 0.13;
    const len = width * (0.45 + ((i * 37) % 40) / 100);
    stroke(ctx, handPath2D(lineSamples({ x: width * 0.14, y: yy }, { x: width * 0.14 + len, y: yy }, 6), false, 0.5, seed + 2 + i), inkA(0.4), 1.1);
  }
  ctx.fillStyle = tea.rust;
  ctx.font = `700 ${width * 0.2}px ui-rounded, -apple-system, system-ui, sans-serif`;
  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  ctx.fillText("tax", width / 2, h * 0.11);
  ctx.restore();
}

function drawFishPaths(ctx: Ctx, fish: FishPaths) {
  for (const s of fish.steam) stroke(ctx, s.path, s.color, s.width);
  ctx.fillStyle = tea.gold;
  ctx.fill(fish.body);
  ctx.save();
  ctx.clip(fish.body);
  stroke(ctx, fish.batter, goldDeepA(0.32), 1);
  ctx.restore();
  stroke(ctx, fish.body, tea.ink, 1.7);
  for (const d of fish.details) stroke(ctx, d.path, d.color, d.width);
  ctx.fillStyle = tea.ink;
  ctx.beginPath();
  ctx.arc(fish.eye.x + fish.eye.w / 2, fish.eye.y + fish.eye.w / 2, fish.eye.w / 2, 0, Math.PI * 2);
  ctx.fill();
  ctx.fillStyle = tea.card;
  ctx.beginPath();
  ctx.arc(fish.glint.x + fish.glint.w / 2, fish.glint.y + fish.glint.w / 2, fish.glint.w / 2, 0, Math.PI * 2);
  ctx.fill();
}

function drawSalt(ctx: Ctx, x: number, y: number, spread: number, count: number, seed: number) {
  ctx.fillStyle = inkA(0.45);
  for (let i = 0; i < count; i++) {
    const px = x + inkNoise(i, seed) * spread;
    const py = y + inkNoise(i + 40, seed) * spread * 0.5;
    ctx.beginPath();
    ctx.arc(px, py, 1, 0, Math.PI * 2);
    ctx.fill();
  }
}

// MARK: - The scene

export class KaraokeScene {
  private w = 0;
  private h = 0;
  private fish: Fish[];
  private fishPaths: FishPaths[];
  private chips: Chip[] = [];
  private gags: Gag[];
  private lastRain = 0;
  private lastFountain = 0;
  private firedAt = new Map<string, number>();
  private leapAt = -10;
  private lastLine = -1;
  /** Called with a canvas point when a thrown chip lands in the wrap. */
  onLand: ((x: number, y: number) => void) | null = null;

  constructor() {
    this.fish = [
      { lane: 0.12, x: 0.2, height: 58, speed: 30, phase: 0.4 },
      { lane: 0.25, x: 0.72, height: 38, speed: 44, phase: 2.1 },
      { lane: 0.5, x: 0.05, height: 30, speed: 24, phase: 4.2 },
      { lane: 0.7, x: 0.55, height: 50, speed: 36, phase: 1.3 },
      { lane: 0.84, x: 0.9, height: 34, speed: 52, phase: 3.3 },
    ];
    this.fishPaths = [0, 1, 2].map((phase) => {
      const art = fishArt(401 + phase * 7);
      return {
        body: new Path2D(art.bodyD),
        batter: new Path2D(art.batterD),
        details: art.details.map((d) => ({ path: new Path2D(d.d), color: d.color, width: d.width })),
        steam: art.steam.map((d) => ({ path: new Path2D(d.d), color: d.color, width: d.width })),
        eye: art.eye,
        glint: art.glint,
      };
    });
    this.gags = this.buildGags();
  }

  resize(w: number, h: number) {
    const first = this.w === 0;
    this.w = w;
    this.h = h;
    if (first) for (const fish of this.fish) fish.x *= w;
  }

  /// Back to the top of the song: gags may fire again, the wrap starts empty.
  reset() {
    this.firedAt.clear();
    this.chips = [];
    this.lastLine = -1;
  }

  /** Scale for the doodles: 1 on a laptop screen, smaller on a phone. */
  private unit() {
    return Math.max(0.55, Math.min(1.15, Math.min(this.w, this.h) / 720));
  }

  private wrapBox() {
    const u = this.unit();
    const width = WRAP_W * u;
    const height = WRAP_H * u;
    const x = this.w / 2 - width / 2;
    const y = this.h - 96 - height;
    return { x, y, width, height, u, landX: this.w / 2, landY: y + height * 0.66 };
  }

  private once(key: string, time: number, action: () => void) {
    const at = this.firedAt.get(key);
    // Refire only if the song was wound back past it.
    if (at !== undefined && time >= at && time - at < 8) return;
    this.firedAt.set(key, time);
    action();
  }

  /// A chip thrown from a canvas point; it arcs into the wrap.
  throwChip(x: number, y: number) {
    const wrap = this.wrapBox();
    const flight = 0.75 + Math.random() * 0.2;
    const targetX = wrap.landX + (Math.random() - 0.5) * wrap.width * 0.4;
    const vx = (targetX - x) / flight;
    const vy = (wrap.landY - y) / flight - 0.5 * GRAVITY * flight;
    this.spawn({
      x, y, vx, vy,
      rot: Math.random() * Math.PI,
      spin: (Math.random() - 0.5) * 14,
      len: (30 + Math.random() * 12) * wrap.u,
      seed: Math.floor(Math.random() * 1000),
      floor: wrap.landY,
      thrown: true,
    });
  }

  /// Chips fired up out of the wrap, falling back in.
  private fountain(count: number, power = 1) {
    const wrap = this.wrapBox();
    for (let i = 0; i < count; i++) {
      this.spawn({
        x: wrap.landX + (Math.random() - 0.5) * wrap.width * 0.5,
        y: wrap.landY - 20,
        vx: (Math.random() - 0.5) * 420 * power,
        vy: -(520 + Math.random() * 360) * power,
        rot: Math.random() * Math.PI,
        spin: (Math.random() - 0.5) * 16,
        len: (24 + Math.random() * 16) * wrap.u,
        seed: Math.floor(Math.random() * 1000),
        floor: wrap.landY + 4,
        thrown: false,
      });
    }
  }

  /// A burst from an arbitrary point (the "pop out" and the finale).
  private burst(x: number, y: number, count: number, power = 1, floor = this.h + 40) {
    const u = this.unit();
    for (let i = 0; i < count; i++) {
      const angle = -Math.PI / 2 + (Math.random() - 0.5) * Math.PI * 0.9;
      const speed = (380 + Math.random() * 420) * power;
      this.spawn({
        x, y,
        vx: Math.cos(angle) * speed,
        vy: Math.sin(angle) * speed,
        rot: Math.random() * Math.PI,
        spin: (Math.random() - 0.5) * 18,
        len: (22 + Math.random() * 16) * u,
        seed: Math.floor(Math.random() * 1000),
        floor,
        thrown: false,
      });
    }
  }

  private spawn(chip: Chip) {
    if (this.chips.length >= MAX_CHIPS) this.chips.shift();
    this.chips.push(chip);
  }

  private buildGags(): Gag[] {
    const gags: Gag[] = [];
    const add = (phrase: string, linger: number, draw: Gag["draw"]) => {
      const line = firstLineWith(phrase);
      if (line >= 0) gags.push({ line, linger, draw });
    };

    // Three holiday snaps, tumbling in from the top corner.
    add("holiday snaps", 2.2, (ctx, t, u) => {
      for (let i = 0; i < 3; i++) {
        const s = clamp01(springTo(t - 0.15 - i * 0.28, 0.6));
        if (s <= 0) continue;
        const x = this.w * (0.78 + i * 0.07) - (1 - s) * 60;
        const y = this.h * 0.12 + i * 26 * u - (1 - s) * 90;
        ctx.save();
        ctx.globalAlpha = Math.min(1, s * 1.5);
        doodlePolaroid(ctx, x, y, 78 * u, (-0.22 + i * 0.16) * (0.6 + 0.4 * s), 611 + i, this.fishPaths[i % 3]);
        ctx.restore();
      }
    });

    // A tax receipt, the mishap.
    add("tax mishaps", 1.8, (ctx, t, u) => {
      const s = clamp01(springTo(t - 1.1, 0.55));
      if (s <= 0) return;
      const x = this.w * 0.1;
      const y = this.h * 0.1 - (1 - s) * 50;
      ctx.save();
      ctx.globalAlpha = Math.min(1, s * 1.5);
      ctx.translate(x, y);
      ctx.scale(0.7 + 0.3 * s, 0.7 + 0.3 * s);
      doodleReceipt(ctx, 0, 0, 62 * u, -0.12, 641);
      ctx.restore();
    });

    // The bin, lid flipping up as the chunky files fly in.
    add("bin them", 1.6, (ctx, t, u) => {
      const s = clamp01(springTo(t, 0.5));
      const lidT = clamp01(springTo(t - 1.3, 0.6));
      const x = this.w * 0.85;
      const y = this.h * 0.56;
      ctx.save();
      ctx.globalAlpha = Math.min(1, s * 1.5);
      ctx.translate(x, y);
      ctx.scale(0.6 + 0.4 * s, 0.6 + 0.4 * s);
      doodleBin(ctx, 0, 0, 62 * u, lidT * 1.25, 651);
      ctx.restore();
      if (t > 1.35) {
        this.once("bin-fill", t, () => {
          for (let i = 0; i < 6; i++) {
            const flight = 0.55 + i * 0.05;
            const fromX = this.w * (0.35 + Math.random() * 0.3);
            const fromY = this.h * 0.2;
            this.spawn({
              x: fromX, y: fromY,
              vx: (x - fromX) / flight,
              vy: (y - fromY) / flight - 0.5 * GRAVITY * flight,
              rot: 0, spin: 9, len: 26 * u, seed: 700 + i, floor: y + 10, thrown: false,
            });
          }
        });
      }
    });

    // Click-click: two cursors, either side of the words.
    add("click-click", 1, (ctx, t, u) => {
      const s = clamp01(springTo(t, 0.45));
      if (s <= 0) return;
      const presses = [0.05, 0.55];
      [this.w * 0.14, this.w * 0.84].forEach((x, i) => {
        const at = t - presses[i];
        const pressed = at > 0 && at < 0.45 ? at / 0.45 : 0;
        const dip = at > 0 && at < 0.12 ? 1 : 0;
        ctx.save();
        ctx.globalAlpha = Math.min(1, s * 1.5);
        doodleCursor(ctx, x, this.h * 0.3 + dip * 4, 40 * u * s, pressed, 661 + i);
        ctx.restore();
      });
    });

    // Watch those little chips pop out!
    add("pop out", 0, (_ctx, t) => {
      if (t > 0.9) this.once("pop-out", t, () => this.burst(this.w / 2, this.h * 0.6, 18, 1.1));
    });

    // The folder called "New Folder Two".
    add("new folder", 2.4, (ctx, t, u) => {
      const s = clamp01(springTo(t - 1.4, 0.55));
      if (s <= 0) return;
      ctx.save();
      ctx.globalAlpha = Math.min(1, s * 1.5);
      ctx.translate(this.w * 0.1, this.h * 0.14);
      ctx.rotate(-0.06);
      ctx.scale(0.6 + 0.4 * s, 0.6 + 0.4 * s);
      doodleFolder(ctx, 0, 0, 108 * u, 671, "New Folder 2");
      ctx.restore();
    });

    // Seventeen copies of the same old brew.
    add("seventeen", 2, (ctx, t, u) => {
      const mugs = 17;
      for (let i = 0; i < mugs; i++) {
        const s = clamp01(springTo(t - 0.3 - i * 0.11, 0.5));
        if (s <= 0) continue;
        const row = i < 9 ? 0 : 1;
        const col = row === 0 ? i : i - 9;
        const count = row === 0 ? 9 : 8;
        const x = this.w * (0.5 + (col - (count - 1) / 2) * 0.085);
        const y = this.h * (0.1 + row * 0.085) + (row === 1 ? 6 : 0);
        ctx.save();
        ctx.globalAlpha = Math.min(1, s * 1.5);
        ctx.translate(x, y);
        ctx.scale(s, s);
        doodleMug(ctx, -20 * u, -20 * u, 40 * u, 681 + i);
        ctx.restore();
      }
    });

    // The magnifier, sweeping across for the files you don't need.
    add("finds the files", 0.2, (ctx, t, u) => {
      const s = clamp01(springTo(t, 0.5));
      const duration = 2.4;
      const p = clamp01(t / duration);
      const x = this.w * (0.2 + 0.6 * p);
      const y = this.h * 0.14 + Math.sin(p * Math.PI * 3) * 14 * u;
      ctx.save();
      ctx.globalAlpha = Math.min(1, s * 1.5) * (1 - clamp01((t - duration) / 0.3));
      doodleMagnifier(ctx, x, y, 22 * u * s, 691);
      ctx.restore();
    });

    // Round and round the folders go.
    add("round and round", 1.2, (ctx, t, u) => {
      const s = clamp01(springTo(t, 0.7));
      const radius = Math.min(this.w * 0.42, this.h * 0.34) * s;
      for (let i = 0; i < 5; i++) {
        const angle = -Math.PI / 2 + (i * Math.PI * 2) / 5 + t * 1.4;
        const x = this.w / 2 + Math.cos(angle) * radius;
        const y = this.h * 0.47 + Math.sin(angle) * radius * 0.55;
        ctx.save();
        ctx.globalAlpha = Math.min(1, s * 1.5);
        ctx.translate(x, y);
        ctx.rotate(Math.sin(t * 2 + i) * 0.1);
        doodleFolder(ctx, -22 * u, -16 * u, 44 * u, 701 + i);
        ctx.restore();
      }
    });

    // One more chip: a big one, dropped into the wrap.
    add("one more chip", 0, (_ctx, t) => {
      if (t > 0.2) {
        this.once("one-more", t, () => {
          const wrap = this.wrapBox();
          this.spawn({
            x: this.w / 2 - 40, y: -40, vx: 30, vy: 40, rot: -0.4, spin: 3,
            len: 74 * wrap.u, seed: 711, floor: wrap.landY, thrown: false,
          });
        });
      }
    });

    // Your Mac's got room for another year: a Mac, grinning.
    add("another year", 2.6, (ctx, t, u) => {
      const s = clamp01(springTo(t - 0.2, 0.6));
      if (s <= 0) return;
      const smile = clamp01((t - 2) / 0.4);
      ctx.save();
      ctx.globalAlpha = Math.min(1, s * 1.5);
      ctx.translate(this.w * 0.84, this.h * 0.3);
      ctx.scale(0.6 + 0.4 * s, 0.6 + 0.4 * s);
      doodleLaptop(ctx, 0, 0, 120 * u, 721, this.fishPaths[1], smile);
      ctx.restore();
    });

    return gags;
  }

  /// Draws one frame. The caller clears nothing: the paper is CSS underneath.
  draw(ctx: Ctx, frame: SceneFrame) {
    const { w, h } = this;
    if (w === 0 || h === 0) return;
    ctx.clearRect(0, 0, w, h);
    const u = this.unit();
    const dt = frame.reduced ? 0 : Math.min(0.05, frame.dt);
    const boil = frame.reduced ? 0 : Math.floor(frame.now / 167) % 3;
    const text = lineText(frame.line);
    const section = lyricLines[frame.line]?.section ?? "";
    const chorus = section.includes("chorus");
    const lineActive = frame.playing && frame.time >= lineStart(frame.line) && frame.time <= lineEnd(frame.line) + 0.3;
    const finale = text.includes("hooray") && frame.time >= lineStart(frame.line);
    const finaleT = finale ? frame.time - lineStart(frame.line) : -1;

    if (frame.line !== this.lastLine) {
      this.lastLine = frame.line;
      if (text.startsWith("whoa") || text.includes("hooray") || text.startsWith("oi")) this.leapAt = frame.now;
    }

    // Chips: a gentle rain in the verses, a fountain on every chorus.
    if (frame.playing && !frame.reduced) {
      if (!chorus && frame.now - this.lastRain > 900 / (0.6 + frame.level)) {
        this.lastRain = frame.now;
        this.spawn({
          x: Math.random() * w, y: -30,
          vx: (Math.random() - 0.5) * 30, vy: 40 + Math.random() * 50,
          rot: Math.random() * Math.PI, spin: (Math.random() - 0.5) * 3,
          len: (20 + Math.random() * 14) * u, seed: Math.floor(Math.random() * 1000),
          floor: this.wrapBox().landY, thrown: false,
        });
      }
      if (chorus && lineActive && frame.now - this.lastFountain > 260 - frame.level * 120) {
        this.lastFountain = frame.now;
        this.fountain(2, 0.85 + frame.level * 0.5);
      }
      if (finale && finaleT < 3.2) {
        if (frame.now - this.lastFountain > 70) {
          this.lastFountain = frame.now;
          this.burst(w * (0.2 + Math.random() * 0.6), h * 0.75, 3, 1.3);
        }
      }
    }

    // Fish, swimming left (the drawing faces left), bobbing to the song.
    const leap = frame.reduced ? 1 : clamp01((frame.now - this.leapAt) / 900);
    for (const [index, fish] of this.fish.entries()) {
      const tempo = chorus ? 2.2 : 1;
      if (frame.playing) fish.x -= fish.speed * tempo * dt;
      if (fish.x < -fish.height * 2) fish.x = w + fish.height * 1.5;
      const bob = frame.reduced ? 0 : Math.sin(frame.now / 640 + fish.phase) * (chorus ? 12 : 6);
      const beat = frame.playing && !frame.reduced ? -frame.level * 10 : 0;
      const hop = leap < 1 ? -Math.sin(leap * Math.PI) * 70 * (0.6 + 0.4 * ((index * 7) % 3) / 2) : 0;
      const y = fish.lane * h + bob + beat + hop;
      const scale = (fish.height * u) / 44;
      ctx.save();
      ctx.translate(fish.x, y);
      ctx.rotate(frame.reduced ? 0 : Math.sin(frame.now / 640 + fish.phase) * 0.06 + (leap < 1 ? -Math.sin(leap * Math.PI) * 0.3 : 0));
      ctx.scale(scale, scale);
      ctx.translate(-32, -22);
      drawFishPaths(ctx, this.fishPaths[(boil + index) % 3]);
      ctx.restore();
    }

    // Sight gags, cued by their lines, holding on a little after.
    for (const gag of this.gags) {
      const start = lineStart(gag.line);
      const end = lineEnd(gag.line) + gag.linger;
      if (frame.time < start || frame.time > end) continue;
      gag.draw(ctx, frame.reduced ? 10 : frame.time - start, u);
    }
    if (frame.scoreBox && text.includes("high score") && frame.time >= lineStart(frame.line)) {
      const t = frame.time - lineStart(frame.line);
      const s = frame.reduced ? 1 : clamp01(t / 0.6);
      const box = frame.scoreBox;
      const ring = handPath2D(
        [
          ...Array.from({ length: 18 }, (_, i) => {
            const a = -0.4 + (Math.PI * 2 * i) / 18;
            return { x: box.x + box.w / 2 + Math.cos(a) * (box.w / 2 + 14), y: box.y + box.h / 2 + Math.sin(a) * (box.h / 2 + 12) };
          }),
          ...Array.from({ length: 7 }, (_, i) => {
            const a = -0.4 + (Math.PI * 0.8 * i) / 7;
            return { x: box.x + box.w / 2 + 2 + Math.cos(a) * (box.w / 2 + 11), y: box.y + box.h / 2 - 1 + Math.sin(a) * (box.h / 2 + 9) };
          }),
        ],
        false, 1.2, 13 + boil
      );
      ctx.save();
      ctx.setLineDash([2000]);
      ctx.lineDashOffset = 2000 * (1 - s);
      stroke(ctx, ring, tea.biro, 2);
      ctx.restore();
    }

    // Chips in flight.
    const g = GRAVITY;
    const wrap = this.wrapBox();
    this.chips = this.chips.filter((chip) => {
      // Reduced motion completes throws without a flight, and drops decorative
      // particles instead of leaving them suspended forever with dt = 0.
      if (frame.reduced) {
        if (chip.thrown) this.onLand?.(wrap.landX, wrap.landY);
        return false;
      }
      chip.vy += g * dt;
      chip.x += chip.vx * dt;
      chip.y += chip.vy * dt;
      chip.rot += chip.spin * dt;
      const landed = chip.y >= chip.floor && chip.vy > 0;
      if (landed) {
        if (chip.thrown) this.onLand?.(chip.x, chip.y);
        return false;
      }
      return chip.x > -80 && chip.x < w + 80 && chip.y < h + 80;
    });
    for (const chip of this.chips) {
      ctx.save();
      ctx.translate(chip.x, chip.y);
      ctx.rotate(chip.rot);
      paintChip(ctx, 0, 0, chip.len, chip.seed + boil);
      ctx.restore();
    }

    // The wrap, full of what you've earned.
    ctx.save();
    ctx.translate(wrap.x, wrap.y);
    paintWrap(ctx, wrap.width, wrap.height, frame.wrapChips, boil);
    ctx.restore();

    // The finale: asterisks and salt everywhere.
    if (finale && finaleT < 4 && !frame.reduced) {
      for (let i = 0; i < 9; i++) {
        const seed = 800 + i * 3 + boil;
        const x = w * (0.08 + ((i * 37 + boil * 11) % 84) / 100);
        const y = h * (0.08 + ((i * 53 + boil * 17) % 70) / 100);
        drawAsterisk(ctx, { x, y }, (7 + (i % 3) * 3) * u, i % 2 ? tea.gold : inkA(0.5), seed);
      }
      drawSalt(ctx, w / 2, h * 0.5, w * 0.8, 40, 900 + boil);
    } else if (chorus && lineActive) {
      drawAsterisk(ctx, { x: w * 0.12, y: h * 0.38 }, 7 * u, tea.gold, 820 + boil);
      drawAsterisk(ctx, { x: w * 0.9, y: h * 0.62 }, 5 * u, inkA(0.45), 823 + boil);
    }
  }
}
