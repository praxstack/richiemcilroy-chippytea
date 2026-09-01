// Bundled by gen-og.ts and inlined into the Open Graph page: paints the
// hand-lettered balance and the wrap of chips with the app's own painters.
import { paintBalance, paintWrap } from "../lib/draw";

function canvasAt(id: string, width: number, height: number) {
  const canvas = document.getElementById(id) as HTMLCanvasElement;
  const dpr = window.devicePixelRatio || 1;
  canvas.width = Math.round(width * dpr);
  canvas.height = Math.round(height * dpr);
  canvas.style.width = `${width}px`;
  canvas.style.height = `${height}px`;
  const ctx = canvas.getContext("2d")!;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  return ctx;
}

const balance = canvasAt("balance", 220, 84);
paintBalance(balance, 958, 62, 0);

// The wrap is designed at 340 × 104 (chips are fixed-size); scale it up whole.
const wrap = canvasAt("wrap", 400, 130);
const scale = 400 / 340;
wrap.translate(0, 4);
wrap.scale(scale, scale);
paintWrap(wrap, 340, 104, 18, 0);
