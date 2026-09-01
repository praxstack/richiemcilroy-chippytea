// The ink primitives, ported from native/Chippytea/CoinScene.swift.
// Every drawn edge on this page comes through handPath, same as the app.

export interface Pt {
  x: number;
  y: number;
}

export const tea = {
  paper: "#FAF5EA",
  paperDeep: "#F1EADB",
  card: "#FFFDF6",
  ink: "#33302B",
  inkSoft: "#7A736A",
  gold: "#F2B63C",
  goldDeep: "#C07F17",
  biro: "#3F5FA8",
  rust: "#B4553D",
} as const;

export const inkA = (a: number) => `rgba(51, 48, 43, ${a})`;
export const goldDeepA = (a: number) => `rgba(192, 127, 23, ${a})`;
export const inkSoftA = (a: number) => `rgba(122, 115, 106, ${a})`;

/// Deterministic, seed-stable jitter in −1…1. Cycling the seed makes a stroke "boil".
export function inkNoise(index: number, seed: number): number {
  let h = (Math.imul(index, 374761393) + Math.imul(seed, 668265263) + 1442695041) | 0;
  h ^= h >>> 13;
  h = Math.imul(h, 1274126177);
  h ^= h >>> 16;
  return ((h >>> 0) % 2001) / 1000 - 1;
}

interface PathSink {
  move(p: Pt): void;
  quad(control: Pt, to: Pt): void;
  line(p: Pt): void;
  close(): void;
}

/// Smooths a sampled outline through quadratic midpoints after perturbing every sample.
function emitHand(points: Pt[], closed: boolean, amplitude: number, seed: number, sink: PathSink) {
  if (points.length <= 2) {
    if (points.length === 0) return;
    sink.move(points[0]);
    for (let i = 1; i < points.length; i++) sink.line(points[i]);
    return;
  }
  const j: Pt[] = new Array(points.length);
  for (let i = 0; i < points.length; i++) {
    j[i] = {
      x: points[i].x + inkNoise(i * 2, seed) * amplitude,
      y: points[i].y + inkNoise(i * 2 + 1, seed) * amplitude,
    };
  }
  const mid = (a: Pt, b: Pt): Pt => ({ x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 });
  if (closed) {
    const n = j.length;
    sink.move(mid(j[n - 1], j[0]));
    for (let i = 0; i < n; i++) sink.quad(j[i], mid(j[i], j[(i + 1) % n]));
    sink.close();
  } else {
    sink.move(j[0]);
    for (let i = 1; i < j.length - 1; i++) sink.quad(j[i], mid(j[i], j[i + 1]));
    sink.line(j[j.length - 1]);
  }
}

const f = (v: number) => Math.round(v * 100) / 100;

export function handPathD(points: Pt[], closed = false, amplitude = 1.1, seed = 1): string {
  let d = "";
  emitHand(points, closed, amplitude, seed, {
    move: (p) => (d += `M${f(p.x)} ${f(p.y)}`),
    quad: (c, p) => (d += `Q${f(c.x)} ${f(c.y)} ${f(p.x)} ${f(p.y)}`),
    line: (p) => (d += `L${f(p.x)} ${f(p.y)}`),
    close: () => (d += "Z"),
  });
  return d;
}

export function handPath2D(points: Pt[], closed = false, amplitude = 1.1, seed = 1): Path2D {
  const path = new Path2D();
  emitHand(points, closed, amplitude, seed, {
    move: (p) => path.moveTo(p.x, p.y),
    quad: (c, p) => path.quadraticCurveTo(c.x, c.y, p.x, p.y),
    line: (p) => path.lineTo(p.x, p.y),
    close: () => path.closePath(),
  });
  return path;
}

export function lineSamples(from: Pt, to: Pt, step = 12): Pt[] {
  const distance = Math.max(1, Math.hypot(to.x - from.x, to.y - from.y));
  const count = Math.max(2, Math.floor(distance / step));
  const out: Pt[] = [];
  for (let i = 0; i <= count; i++) {
    const t = i / count;
    out.push({ x: from.x + (to.x - from.x) * t, y: from.y + (to.y - from.y) * t });
  }
  return out;
}

export function circleSamples(center: Pt, radius: number, count = 16, from = 0, sweep = Math.PI * 2): Pt[] {
  const out: Pt[] = [];
  for (let i = 0; i < count; i++) {
    const angle = from + (sweep * i) / count;
    out.push({ x: center.x + Math.cos(angle) * radius, y: center.y + Math.sin(angle) * radius });
  }
  return out;
}

export function arcSamples(center: Pt, radius: number, from: number, to: number, count = 10): Pt[] {
  const out: Pt[] = [];
  for (let i = 0; i <= count; i++) {
    const angle = from + ((to - from) * i) / count;
    out.push({ x: center.x + Math.cos(angle) * radius, y: center.y + Math.sin(angle) * radius });
  }
  return out;
}

export function roundedRectSamples(
  x: number,
  y: number,
  w: number,
  h: number,
  radius: number,
  step = 10
): Pt[] {
  const r = Math.max(0, Math.min(radius, Math.min(w, h) / 2));
  const points: Pt[] = [];
  const edge = (a: Pt, b: Pt) => {
    const distance = Math.max(1, Math.hypot(b.x - a.x, b.y - a.y));
    const count = Math.max(1, Math.floor(distance / step));
    for (let i = 0; i < count; i++) {
      const t = i / count;
      points.push({ x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t });
    }
  };
  const corner = (c: Pt, from: number, to: number) => {
    const count = Math.max(2, Math.floor(r / 4) + 2);
    for (let i = 0; i < count; i++) {
      const angle = from + ((to - from) * i) / count;
      points.push({ x: c.x + Math.cos(angle) * r, y: c.y + Math.sin(angle) * r });
    }
  };
  const HP = Math.PI / 2;
  edge({ x: x + r, y }, { x: x + w - r, y });
  corner({ x: x + w - r, y: y + r }, -HP, 0);
  edge({ x: x + w, y: y + r }, { x: x + w, y: y + h - r });
  corner({ x: x + w - r, y: y + h - r }, 0, HP);
  edge({ x: x + w - r, y: y + h }, { x: x + r, y: y + h });
  corner({ x: x + r, y: y + h - r }, HP, Math.PI);
  edge({ x, y: y + h - r }, { x, y: y + r });
  corner({ x: x + r, y: y + r }, Math.PI, 1.5 * Math.PI);
  return points;
}

/// One fish is a thousand chips — the full supper.
export function fishAndChips(totalChips: number): { fish: number; chips: number } {
  const total = Math.max(0, Math.floor(totalChips));
  return { fish: Math.floor(total / 1000), chips: total % 1000 };
}

/// "1.2 GB" / "300 MB" — decimal units, the way the app writes sizes.
export function space(bytes: number): string {
  if (bytes >= 1e9) {
    const gb = bytes / 1e9;
    return `${gb >= 10 ? Math.round(gb) : Math.round(gb * 10) / 10} GB`;
  }
  return `${Math.round(bytes / 1e6)} MB`;
}
