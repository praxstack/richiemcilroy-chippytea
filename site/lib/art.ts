// The drawn things themselves — fish, wordmark, tape, letterforms, numerals —
// ported point-for-point from native/Chippytea/CoinScene.swift. Pure data and
// SVG path builders; nothing here touches the DOM, so the server can draw too.

import { Pt, handPathD, inkNoise, lineSamples } from "./ink";

/// The fish: 64 × 44 design box, battered gold body, forked tail, one open eye.
export const fishBodyPoints: Pt[] = [
  { x: 3, y: 26 }, { x: 7, y: 20 }, { x: 14, y: 15 }, { x: 25, y: 12 },
  { x: 35, y: 13 }, { x: 43, y: 17 }, { x: 48, y: 20 }, { x: 59, y: 12 },
  { x: 55, y: 25 }, { x: 59, y: 36 }, { x: 48, y: 28 }, { x: 41, y: 33 },
  { x: 29, y: 36 }, { x: 15, y: 34 }, { x: 6, y: 31 },
];

export interface Stroke {
  d: string;
  color: string;
  width: number;
}

export interface FishArt {
  bodyD: string;
  batterD: string;
  steam: Stroke[];
  details: Stroke[];
  eye: { x: number; y: number; w: number };
  glint: { x: number; y: number; w: number };
}

import { inkA, goldDeepA } from "./ink";

export function fishArt(seed: number, steam = true): FishArt {
  const steamStrokes: Stroke[] = [];
  if (steam) {
    const wisps: Pt[][] = [
      [{ x: 30, y: 9 }, { x: 33, y: 5 }, { x: 29, y: 1 }],
      [{ x: 40, y: 8 }, { x: 43, y: 4 }, { x: 40, y: 1 }],
    ];
    wisps.forEach((wisp, index) => {
      steamStrokes.push({ d: handPathD(wisp, false, 0.5, seed + index), color: inkA(0.38), width: 1.3 });
    });
  }

  // Scribbled batter, clipped to the body by the component.
  let batterD = "";
  let x = -30;
  while (x < 64) {
    const wob = inkNoise(Math.floor(x), seed) * 0.9;
    const midY = 25 + inkNoise(Math.floor(x) + 7, seed) * 2;
    batterD += `M${(x + wob).toFixed(2)} 44Q${(x + 13).toFixed(2)} ${midY.toFixed(2)} ${(x + 30).toFixed(2)} 6`;
    x += 4.2;
  }

  const details: Stroke[] = [
    // Tail creases.
    { d: handPathD([{ x: 49, y: 21 }, { x: 56, y: 15 }], false, 0.4, seed + 3), color: inkA(0.7), width: 1.1 },
    { d: handPathD([{ x: 49, y: 27 }, { x: 56, y: 33 }], false, 0.4, seed + 4), color: inkA(0.7), width: 1.1 },
    // Gill line.
    { d: handPathD([{ x: 17, y: 19 }, { x: 19, y: 24 }, { x: 17, y: 29 }], false, 0.5, seed + 5), color: inkA(0.55), width: 1.1 },
    // A batter drip under the belly.
    { d: handPathD([{ x: 22, y: 36 }, { x: 24, y: 39 }, { x: 26, y: 36 }], false, 0.4, seed + 6), color: inkA(0.5), width: 1.1 },
  ];

  return {
    bodyD: handPathD(fishBodyPoints, true, 1.1, seed),
    batterD,
    steam: steamStrokes,
    details,
    eye: { x: 10.2, y: 20.2, w: 3.6 },
    glint: { x: 10.9, y: 20.9, w: 1.1 },
  };
}

/// Hand-lettered lowercase letterforms on a 92-tall box: baseline 70, x-height
/// 30–70, descenders to 92. Centre-lines only.
export function letterStrokes(letter: string): { strokes: Pt[][]; advance: number } {
  switch (letter) {
    case "c":
      return { strokes: [[{ x: 33, y: 36 }, { x: 22, y: 29 }, { x: 11, y: 36 }, { x: 7, y: 50 }, { x: 11, y: 64 }, { x: 22, y: 71 }, { x: 33, y: 64 }]], advance: 40 };
    case "h":
      return { strokes: [[{ x: 9, y: 10 }, { x: 10, y: 40 }, { x: 10, y: 70 }], [{ x: 10, y: 48 }, { x: 16, y: 33 }, { x: 26, y: 31 }, { x: 32, y: 42 }, { x: 33, y: 70 }]], advance: 42 };
    case "i":
      return { strokes: [[{ x: 8, y: 34 }, { x: 9, y: 52 }, { x: 10, y: 70 }]], advance: 19 };
    case "p":
      return { strokes: [[{ x: 8, y: 34 }, { x: 10, y: 60 }, { x: 12, y: 90 }], [{ x: 10, y: 42 }, { x: 20, y: 31 }, { x: 31, y: 36 }, { x: 34, y: 50 }, { x: 29, y: 63 }, { x: 18, y: 66 }, { x: 10, y: 58 }]], advance: 42 };
    case "y":
      return { strokes: [[{ x: 7, y: 32 }, { x: 10, y: 50 }, { x: 17, y: 61 }, { x: 27, y: 63 }], [{ x: 33, y: 31 }, { x: 33, y: 52 }, { x: 29, y: 72 }, { x: 21, y: 86 }, { x: 12, y: 91 }]], advance: 40 };
    case "t":
      return { strokes: [[{ x: 15, y: 12 }, { x: 16, y: 40 }, { x: 17, y: 60 }, { x: 22, y: 69 }, { x: 30, y: 66 }], [{ x: 5, y: 33 }, { x: 16, y: 32 }, { x: 28, y: 31 }]], advance: 34 };
    case "e":
      return { strokes: [[{ x: 8, y: 50 }, { x: 20, y: 48 }, { x: 31, y: 45 }, { x: 29, y: 34 }, { x: 19, y: 29 }, { x: 9, y: 37 }, { x: 7, y: 51 }, { x: 12, y: 65 }, { x: 23, y: 71 }, { x: 32, y: 66 }]], advance: 40 };
    case "a":
      return { strokes: [[{ x: 30, y: 38 }, { x: 19, y: 31 }, { x: 9, y: 39 }, { x: 7, y: 53 }, { x: 12, y: 66 }, { x: 23, y: 68 }, { x: 30, y: 58 }], [{ x: 31, y: 33 }, { x: 31, y: 52 }, { x: 32, y: 70 }, { x: 37, y: 68 }]], advance: 43 };
    case "f":
      return { strokes: [[{ x: 27, y: 13 }, { x: 18, y: 16 }, { x: 14, y: 28 }, { x: 13, y: 48 }, { x: 13, y: 70 }], [{ x: 4, y: 33 }, { x: 15, y: 32 }, { x: 27, y: 31 }]], advance: 32 };
    case "s":
      return { strokes: [[{ x: 29, y: 36 }, { x: 19, y: 30 }, { x: 10, y: 36 }, { x: 13, y: 46 }, { x: 23, y: 51 }, { x: 29, y: 58 }, { x: 25, y: 68 }, { x: 14, y: 70 }, { x: 6, y: 64 }]], advance: 38 };
    default:
      return { strokes: [], advance: 20 };
  }
}

export function handWordWidth(word: string): number {
  let w = 1;
  for (const ch of word) w += letterStrokes(ch).advance;
  return w;
}

export interface WordArt {
  strokes: string[];
  dots: { x: number; y: number; r: number }[];
  width: number;
}

/// A word written by hand in the 92-unit letter space. Stroke width is 7.5 there.
export function wordArt(word: string, seed: number): WordArt {
  const strokes: string[] = [];
  const dots: { x: number; y: number; r: number }[] = [];
  let penX = 1;
  let index = 0;
  for (const letter of word) {
    const { strokes: letterPaths, advance } = letterStrokes(letter);
    const bounce = inkNoise(index, seed) * 1.6;
    letterPaths.forEach((stroke, strokeIndex) => {
      const points = stroke.map((p) => ({ x: penX + p.x, y: p.y + bounce }));
      strokes.push(handPathD(points, false, 1.4, seed + index * 13 + strokeIndex * 5));
    });
    if (letter === "i") dots.push({ x: penX + 6.5 + 2.7, y: 17 + bounce + 2.7, r: 2.7 });
    penX += advance;
    index += 1;
  }
  return { strokes, dots, width: penX };
}

/// Marigold washi tape, 56 × 24, torn at both ends.
export function tapeArt(seed: number): { outlineD: string; streaks: { d: string }[] } {
  const w = 56;
  const h = 24;
  const outline: Pt[] = [];
  outline.push(...lineSamples({ x: 5, y: 1.5 }, { x: w - 5, y: 1.5 }, 9));
  outline.push(
    { x: w - 4, y: h * 0.18 }, { x: w - 1, y: h * 0.34 },
    { x: w - 5, y: h * 0.52 }, { x: w - 1.5, y: h * 0.7 }, { x: w - 4.5, y: h * 0.88 }
  );
  outline.push(...lineSamples({ x: w - 5, y: h - 1.5 }, { x: 5, y: h - 1.5 }, 9));
  outline.push(
    { x: 4, y: h * 0.84 }, { x: 1, y: h * 0.66 },
    { x: 5, y: h * 0.5 }, { x: 1.5, y: h * 0.32 }, { x: 4, y: h * 0.16 }
  );
  const streaks: { d: string }[] = [];
  for (let i = 0; i < 5; i++) {
    const x = i * 13 + 4 + inkNoise(i, seed) * 1.5;
    streaks.push({ d: `M${x.toFixed(2)} -2L${(x + 7).toFixed(2)} ${h + 2}` });
  }
  return { outlineD: handPathD(outline, true, 0.7, seed), streaks };
}

/// Casual numerals on a 60 × 92 box. Centre-lines only; weight comes from stroking.
export function glyphStrokes(symbol: number): { points: Pt[]; closed: boolean }[] {
  switch (symbol) {
    case 0:
      return [{
        points: Array.from({ length: 15 }, (_, i) => {
          const angle = -Math.PI / 2 + (Math.PI * 2 * i) / 15;
          return { x: 30 + Math.cos(angle) * 18, y: 47 + Math.sin(angle) * 41 };
        }),
        closed: true,
      }];
    case 1:
      return [
        { points: [{ x: 11, y: 25 }, { x: 22, y: 14 }, { x: 32, y: 7 }, { x: 32, y: 46 }, { x: 31, y: 85 }], closed: false },
        { points: [{ x: 14, y: 87 }, { x: 30, y: 85 }, { x: 48, y: 86 }], closed: false },
      ];
    case 2:
      return [{ points: [{ x: 9, y: 26 }, { x: 14, y: 13 }, { x: 28, y: 6 }, { x: 44, y: 10 }, { x: 51, y: 24 }, { x: 44, y: 39 }, { x: 30, y: 52 }, { x: 16, y: 66 }, { x: 8, y: 85 }, { x: 31, y: 84 }, { x: 53, y: 85 }], closed: false }];
    case 3:
      return [{ points: [{ x: 11, y: 18 }, { x: 25, y: 7 }, { x: 43, y: 10 }, { x: 51, y: 23 }, { x: 43, y: 37 }, { x: 29, y: 43 }, { x: 44, y: 47 }, { x: 53, y: 61 }, { x: 48, y: 78 }, { x: 30, y: 88 }, { x: 11, y: 81 }], closed: false }];
    case 4:
      return [
        { points: [{ x: 44, y: 8 }, { x: 26, y: 36 }, { x: 7, y: 63 }, { x: 31, y: 63 }, { x: 56, y: 62 }], closed: false },
        { points: [{ x: 42, y: 27 }, { x: 41, y: 60 }, { x: 41, y: 88 }], closed: false },
      ];
    case 5:
      return [{ points: [{ x: 51, y: 9 }, { x: 30, y: 10 }, { x: 15, y: 11 }, { x: 12, y: 42 }, { x: 30, y: 34 }, { x: 48, y: 42 }, { x: 54, y: 61 }, { x: 45, y: 81 }, { x: 25, y: 88 }, { x: 9, y: 79 }], closed: false }];
    case 6:
      return [{ points: [{ x: 47, y: 10 }, { x: 27, y: 18 }, { x: 13, y: 38 }, { x: 9, y: 60 }, { x: 16, y: 79 }, { x: 32, y: 88 }, { x: 47, y: 80 }, { x: 50, y: 63 }, { x: 39, y: 51 }, { x: 23, y: 50 }, { x: 12, y: 60 }], closed: false }];
    case 7:
      return [
        { points: [{ x: 8, y: 12 }, { x: 30, y: 9 }, { x: 53, y: 10 }, { x: 40, y: 48 }, { x: 27, y: 88 }], closed: false },
        { points: [{ x: 17, y: 50 }, { x: 30, y: 48 }, { x: 43, y: 47 }], closed: false },
      ];
    case 8:
      return [
        {
          points: Array.from({ length: 11 }, (_, i) => {
            const angle = -Math.PI / 2 + (Math.PI * 2 * i) / 11;
            return { x: 30 + Math.cos(angle) * 17, y: 27 + Math.sin(angle) * 19 };
          }),
          closed: true,
        },
        {
          points: Array.from({ length: 12 }, (_, i) => {
            const angle = -Math.PI / 2 + (Math.PI * 2 * i) / 12;
            return { x: 30 + Math.cos(angle) * 21, y: 67 + Math.sin(angle) * 22 };
          }),
          closed: true,
        },
      ];
    case 9:
      return [{ points: [{ x: 49, y: 46 }, { x: 36, y: 53 }, { x: 20, y: 49 }, { x: 11, y: 34 }, { x: 18, y: 16 }, { x: 36, y: 8 }, { x: 50, y: 18 }, { x: 52, y: 40 }, { x: 47, y: 63 }, { x: 36, y: 82 }, { x: 20, y: 89 }], closed: false }];
    default: // "+"
      return [
        { points: [{ x: 12, y: 48 }, { x: 30, y: 47 }, { x: 48, y: 48 }], closed: false },
        { points: [{ x: 30, y: 30 }, { x: 30, y: 48 }, { x: 30, y: 66 }], closed: false },
      ];
  }
}

export const glyphBox = { width: 60, height: 92 };
export const glyphAdvance = 62;
