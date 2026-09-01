"use client";

// The tray panel, laid out exactly as the app's home screen (Views.swift,
// CoinsPage): a fixed 380 × 620 page — masthead, then the hero (balance,
// caption, credited line, meter, the 104-pt wrap with its pending-caption
// line), then the storage strip, a divider, "Make a bit of room." and one
// card of suggestion rows split by drawn dividers. The collect overlay covers
// the hero only, with ChipCollectionOverlay's verbatim geometry: entry at
// y = −6, control at 0.20–0.36 H, landing at 0.80–0.92 H, the scrawled +N at
// (128, 22). Counter eases over 1.35 s from t = 0.03, the wrap squashes at
// t = 0.81 and springs back at t = 0.98, chips fly 0.86 s each, 34 ms apart.
// A pretend Mac underneath. Cleanup results are examples, not storage measurements.

import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { tea, inkA, handPathD, roundedRectSamples, lineSamples, arcSamples, space } from "@/lib/ink";
import { paintBalance, paintWrap, paintChip, drawGlyph } from "@/lib/draw";
import { glyphAdvance } from "@/lib/art";
import { playChime } from "@/lib/chime";
import { ShopSign, Tape, Underlined, MugDoodle, StorageMugSvg, Rule } from "./art";
import { InkBox } from "./InkBox";

const PANEL_H = 620;
const WRAP_HEIGHT = 104;
const BALANCE_HEIGHT = 56;
const PAD = 20;
const START_BALANCE = 958;
const SCRAP_BYTES = 37e6;
const FREE_BYTES = 212e9;
const TOTAL_BYTES = 494e9;

interface Suggestion {
  id: string;
  title: string;
  path: string;
  bytes: number;
  quietDays: number;
  seed: number;
  kind: "build" | "dependencies" | "download";
}

// Fictional examples of the app's supported suggestions. Downloads stay Trash-only.
const POOL: Suggestion[] = [
  { id: "target", title: "Rust build folder", path: "~/Projects/old-tool/target", bytes: 2.4e9, quietDays: 12, seed: 61, kind: "build" },
  { id: "node", title: "Old project dependencies", path: "~/Projects/old-website/node_modules", bytes: 1.2e9, quietDays: 34, seed: 67, kind: "dependencies" },
  { id: "installer", title: "Old installer", path: "~/Downloads/old-installer.dmg", bytes: 580e6, quietDays: 41, seed: 73, kind: "download" },
  { id: "target-side", title: "Side project build folder", path: "~/Projects/side-project/target", bytes: 3.1e9, quietDays: 9, seed: 79, kind: "build" },
  { id: "node-tool", title: "Unused tool dependencies", path: "~/Projects/old-tool/node_modules", bytes: 900e6, quietDays: 28, seed: 83, kind: "dependencies" },
  { id: "installer-tool", title: "Another old installer", path: "~/Downloads/old-tool.pkg", bytes: 340e6, quietDays: 16, seed: 89, kind: "download" },
];

const chipsFor = (s: Suggestion) => s.kind === "download" ? 0 : Math.floor(s.bytes / 100e6);

type RowState = "idle" | "cleaning" | "trashed";

function setup(canvas: HTMLCanvasElement, w: number, h: number) {
  const dpr = Math.min(2, window.devicePixelRatio || 1);
  canvas.width = Math.round(w * dpr);
  canvas.height = Math.round(h * dpr);
  canvas.style.width = `${w}px`;
  canvas.style.height = `${h}px`;
  const ctx = canvas.getContext("2d");
  if (ctx) ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  return ctx;
}

/// An underdamped spring's progress toward 1, SwiftUI-style (response, damping 0.45).
function springTo(t: number, response: number, damping = 0.45) {
  if (t <= 0) return 0;
  const wn = (2 * Math.PI) / response;
  const wd = wn * Math.sqrt(1 - damping * damping);
  return 1 - Math.exp(-damping * wn * t) * (Math.cos(wd * t) + ((damping * wn) / wd) * Math.sin(wd * t));
}

const easeOut = (t: number) => 1 - (1 - Math.min(1, Math.max(0, t))) ** 2;

function quadPoint(entry: { x: number; y: number }, control: { x: number; y: number }, end: { x: number; y: number }, t: number) {
  const u = 1 - t;
  return {
    x: u * u * entry.x + 2 * u * t * control.x + t * t * end.x,
    y: u * u * entry.y + 2 * u * t * control.y + t * t * end.y,
  };
}

function Meter({ width, fraction }: { width: number; fraction: number }) {
  const base = useMemo(
    () => handPathD(lineSamples({ x: 1, y: 2 }, { x: width - 1, y: 2 }, 16), false, 0.5, 101),
    [width]
  );
  const earnedWidth = Math.max(6, width * Math.min(1, fraction));
  const earned = useMemo(
    () => handPathD(lineSamples({ x: 1, y: 2 }, { x: 1 + earnedWidth, y: 2 }, 16), false, 0.8, 104),
    [earnedWidth]
  );
  return (
    <svg width={width} height={4} className="mt-[5px] block" aria-hidden="true">
      <path d={base} stroke={inkA(0.2)} strokeWidth={1.4} fill="none" strokeLinecap="round" />
      {fraction > 0.005 && <path d={earned} stroke={tea.gold} strokeWidth={3} fill="none" strokeLinecap="round" />}
    </svg>
  );
}

/// The progress ring while cleanup runs in the background — a drawn arc, spun.
function InkSpinner() {
  const d = useMemo(() => handPathD(arcSamples({ x: 7, y: 7 }, 5.2, -Math.PI / 2, Math.PI * 0.9, 8), false, 0.5, 87), []);
  return (
    <svg width={14} height={14} className="animate-spin motion-reduce:animate-none" aria-hidden="true">
      <path d={d} stroke={inkA(0.7)} strokeWidth={1.6} fill="none" strokeLinecap="round" />
    </svg>
  );
}

const strokeProps = { fill: "none", strokeLinecap: "round", strokeLinejoin: "round" } as const;

/// The tab bar: Chips is home and you're on it — the chip bundle in gold with
/// a hand-scribbled biro ring. The other screens sit this demo out.
function TabsBar() {
  const art = useMemo(() => {
    const E = 22;
    const poses = [
      { dx: -0.28, dy: 0.06, tilt: -11, len: 0.74 },
      { dx: 0.29, dy: 0.08, tilt: 12, len: 0.68 },
      { dx: 0, dy: 0, tilt: 2, len: 0.94 },
    ];
    const chips = poses.map((pose, i) => {
      const L = E * pose.len;
      const T = L * 0.3;
      return {
        d: handPathD(roundedRectSamples(-L / 2, -T / 2, L, T, T * 0.42, 5), true, L * 0.03, 7 + i * 7),
        transform: `translate(${13 + pose.dx * E} ${13 + pose.dy * E}) rotate(${-90 + pose.tilt})`,
        outlineW: Math.max(1, L * 0.055),
      };
    });
    // The scribbled ring around the active tab, per ScribbleRing.
    const ringSamples = [
      ...Array.from({ length: 15 }, (_, i) => {
        const a = -0.5 + (Math.PI * 2 * i) / 15;
        return { x: 13 + Math.cos(a) * 12, y: 13 + Math.sin(a) * 12 };
      }),
      ...Array.from({ length: 6 }, (_, i) => {
        const a = -0.5 + (Math.PI * 0.75 * i) / 6;
        return { x: 13.6 + Math.cos(a) * 11.3, y: 12.6 + Math.sin(a) * 11.3 };
      }),
    ];
    const ring = handPathD(ringSamples, false, 1, 13);
    const magnifier =
      handPathD(
        Array.from({ length: 14 }, (_, i) => {
          const a = (Math.PI * 2 * i) / 14;
          return { x: 11 + Math.cos(a) * 5.5, y: 10 + Math.sin(a) * 5.5 };
        }),
        true,
        0.6,
        81
      ) + handPathD(lineSamples({ x: 15, y: 14.5 }, { x: 20, y: 20 }, 4), false, 0.5, 83);
    const clock =
      handPathD(
        Array.from({ length: 14 }, (_, i) => {
          const a = (Math.PI * 2 * i) / 14;
          return { x: 13 + Math.cos(a) * 7.5, y: 13 + Math.sin(a) * 7.5 };
        }),
        true,
        0.6,
        91
      ) +
      handPathD([{ x: 13, y: 9 }, { x: 13, y: 13.5 }], false, 0.4, 93) +
      handPathD([{ x: 13, y: 13.5 }, { x: 16.5, y: 15 }], false, 0.4, 95);
    const sliders =
      handPathD(lineSamples({ x: 6, y: 10 }, { x: 20, y: 10 }, 5), false, 0.5, 97) +
      handPathD(lineSamples({ x: 6, y: 16.5 }, { x: 20, y: 16.5 }, 5), false, 0.5, 99);
    return { chips, ring, magnifier, clock, sliders };
  }, []);

  const tab = (label: string, active: boolean, content: React.ReactNode) => (
    <span
      className={`flex flex-1 cursor-default items-center justify-center pb-[3px] pt-[5px] ${active ? "" : "opacity-90"}`}
      title={active ? label : `${label}: just the Chips screen in this demo`}
    >
      {content}
    </span>
  );

  return (
    <div className="relative z-[5] shrink-0">
      <div className="px-5">
        <Rule />
      </div>
      <div className="flex items-center px-4 pb-1.5 pt-0.5" role="presentation">
        {tab(
          "Chips",
          true,
          <svg width={26} height={26} aria-hidden="true">
            <path d={art.ring} stroke={tea.biro} strokeWidth={1.4} {...strokeProps} />
            {art.chips.map((chip, i) => (
              <g key={i} transform={chip.transform}>
                <path d={chip.d} fill={tea.gold} />
                <path d={chip.d} stroke={tea.ink} strokeWidth={chip.outlineW} {...strokeProps} />
              </g>
            ))}
          </svg>
        )}
        {tab(
          "Find space",
          false,
          <svg width={26} height={26} aria-hidden="true">
            <path d={art.magnifier} stroke={tea.inkSoft} strokeWidth={1.6} {...strokeProps} />
          </svg>
        )}
        {tab(
          "Activity",
          false,
          <svg width={26} height={26} aria-hidden="true">
            <path d={art.clock} stroke={tea.inkSoft} strokeWidth={1.5} {...strokeProps} />
          </svg>
        )}
        {tab(
          "Settings",
          false,
          <svg width={26} height={26} aria-hidden="true">
            <path d={art.sliders} stroke={tea.inkSoft} strokeWidth={1.6} {...strokeProps} />
            <circle cx={10} cy={10} r={2.2} fill={tea.paper} stroke={tea.inkSoft} strokeWidth={1.4} />
            <circle cx={16} cy={16.5} r={2.2} fill={tea.paper} stroke={tea.inkSoft} strokeWidth={1.4} />
          </svg>
        )}
      </div>
    </div>
  );
}

export function DemoPanel() {
  const shellRef = useRef<HTMLDivElement>(null);
  const heroRef = useRef<HTMLDivElement>(null);
  const balRef = useRef<HTMLCanvasElement>(null);
  const wrapRef = useRef<HTMLCanvasElement>(null);
  const overlayRef = useRef<HTMLCanvasElement>(null);
  const ctxRef = useRef<{
    bal: CanvasRenderingContext2D | null;
    wrap: CanvasRenderingContext2D | null;
    overlay: CanvasRenderingContext2D | null;
  }>({ bal: null, wrap: null, overlay: null });

  const [panelW, setPanelW] = useState(380);
  const [balance, setBalance] = useState(START_BALANCE);
  const [pendingAmount, setPendingAmount] = useState(0);
  const [items, setItems] = useState<{ suggestion: Suggestion; state: RowState }[]>(() =>
    POOL.slice(0, 3).map((suggestion) => ({ suggestion, state: "idle" }))
  );
  const [expanded, setExpanded] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const poolIndexRef = useRef(3);
  const stateRef = useRef({ balance: START_BALANCE, panelW: 380, heroH: 0 });
  const rafRef = useRef(0);
  const timersRef = useRef<ReturnType<typeof setTimeout>[]>([]);
  const wrapKeyRef = useRef("");

  const innerW = panelW - PAD * 2;

  const drawBalance = (value: number, boil: number) => {
    const ctx = ctxRef.current.bal;
    if (!ctx) return;
    ctx.clearRect(0, 0, stateRef.current.panelW, BALANCE_HEIGHT);
    paintBalance(ctx, value, 42, boil);
  };

  const drawWrap = (chips: number, boil: number, squash = 0) => {
    const key = `${chips}|${boil}|${squash.toFixed(3)}`;
    if (key === wrapKeyRef.current) return;
    wrapKeyRef.current = key;
    const ctx = ctxRef.current.wrap;
    if (!ctx) return;
    const w = stateRef.current.panelW - PAD * 2;
    ctx.clearRect(0, 0, w, WRAP_HEIGHT);
    if (squash !== 0) {
      const sx = 1 + 0.03 * squash;
      const sy = 1 - 0.04 * squash;
      ctx.save();
      ctx.translate((w / 2) * (1 - sx), WRAP_HEIGHT * (1 - sy));
      ctx.scale(sx, sy);
      paintWrap(ctx, w, WRAP_HEIGHT, chips, boil);
      ctx.restore();
    } else {
      paintWrap(ctx, w, WRAP_HEIGHT, chips, boil);
    }
  };

  useLayoutEffect(() => {
    const shell = shellRef.current;
    if (!shell) return;
    const measure = () => setPanelW(Math.min(380, Math.floor(shell.offsetWidth)));
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(shell);
    return () => observer.disconnect();
  }, []);

  useLayoutEffect(() => {
    stateRef.current.panelW = panelW;
    const hero = heroRef.current;
    if (!hero || !balRef.current || !wrapRef.current || !overlayRef.current) return;
    stateRef.current.heroH = hero.offsetHeight;
    ctxRef.current.bal = setup(balRef.current, panelW - PAD * 2, BALANCE_HEIGHT);
    ctxRef.current.wrap = setup(wrapRef.current, panelW - PAD * 2, WRAP_HEIGHT);
    ctxRef.current.overlay = setup(overlayRef.current, panelW, hero.offsetHeight);
    wrapKeyRef.current = "";
    drawBalance(stateRef.current.balance, 0);
    drawWrap(stateRef.current.balance, 0);
  }, [panelW]);

  // The one-second ink flourish when the panel first appears.
  useEffect(() => {
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    let step = 0;
    const id = setInterval(() => {
      step += 1;
      if (step > 6) {
        clearInterval(id);
        return;
      }
      drawWrap(stateRef.current.balance, step % 3);
    }, 167);
    return () => clearInterval(id);
  }, []);

  useEffect(
    () => () => {
      cancelAnimationFrame(rafRef.current);
      timersRef.current.forEach(clearTimeout);
    },
    []
  );

  const later = (fn: () => void, ms: number) => {
    timersRef.current.push(setTimeout(fn, ms));
  };

  const removeItem = (id: string) => {
    setItems((current) => current.filter((item) => item.suggestion.id !== id));
  };

  /// The collect moment — ChipCollectionOverlay, verbatim, over the hero.
  const runBurst = (amount: number) => {
    const from = stateRef.current.balance;
    const to = from + amount;
    stateRef.current.balance = to;
    setBalance(to);
    playChime();

    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
      drawBalance(to, 0);
      drawWrap(to, 0);
      return 0;
    }

    const start = performance.now();
    const count = Math.min(24, Math.max(6, amount));
    const endAt = Math.max(1.95, count * 0.034 + 0.86 + 0.1);
    const w = stateRef.current.panelW;

    const frame = (now: number) => {
      const elapsed = (now - start) / 1000;
      const boil = elapsed < 1.002 ? (Math.floor(elapsed / 0.167) + 1) % 3 : 0;

      // The counter rolls up in hand-lettered digits.
      const value = from + (to - from) * easeOut((elapsed - 0.03) / 1.35);
      drawBalance(value, boil);

      // The wrap squashes as the chips land, then springs back.
      let squash = 0;
      if (elapsed >= 0.81 && elapsed < 0.98) squash = springTo(elapsed - 0.81, 0.2);
      else if (elapsed >= 0.98) squash = springTo(0.17, 0.2) * (1 - springTo(elapsed - 0.98, 0.34));
      if (Math.abs(squash) < 0.004 && elapsed > 1.6) squash = 0;
      drawWrap(to, boil, squash);

      // Chips tumble out from under the tape, trailing sketched motion lines.
      const octx = ctxRef.current.overlay;
      if (octx) {
        const H = stateRef.current.heroH;
        octx.clearRect(0, 0, w, H);
        const entry = { x: Math.min(Math.max(w / 2, 24), w - 24), y: -6 };
        for (let index = 0; index < count; index++) {
          const progress = (elapsed - index * 0.034) / 0.86;
          if (progress <= 0 || progress >= 1) continue;
          const t = progress;
          const seed = ((index * 73 + 19) % 101) / 101;
          const end = { x: w * (0.17 + seed * 0.66), y: H * (0.8 + seed * 0.12) };
          const control = { x: entry.x + (end.x - entry.x) * (0.15 + seed * 0.35), y: H * (0.2 + seed * 0.16) };
          const here = quadPoint(entry, control, end, t);
          const fade = Math.min(1, progress * 9) * Math.min(1, (1 - progress) * 9);
          const chipLength = 17 + seed * 10;

          octx.save();
          octx.globalAlpha = fade * 0.55;
          for (let line = 0; line < 3; line++) {
            const lag = 0.05 + line * 0.035;
            const a = quadPoint(entry, control, end, Math.max(0, t - lag - 0.05));
            const b = quadPoint(entry, control, end, Math.max(0, t - lag));
            if (Math.hypot(b.x - a.x, b.y - a.y) <= 0.6) continue;
            const offset = (line - 1) * chipLength * 0.24;
            octx.strokeStyle = inkA(0.4);
            octx.lineWidth = 1.2;
            octx.lineCap = "round";
            octx.beginPath();
            octx.moveTo(a.x + offset, a.y);
            octx.quadraticCurveTo((a.x + b.x) / 2 + offset * 1.4, (a.y + b.y) / 2, b.x + offset, b.y);
            octx.stroke();
          }
          octx.restore();

          octx.save();
          octx.globalAlpha = fade;
          octx.translate(here.x, here.y);
          octx.rotate(((progress * 250 + index * 23) * Math.PI) / 180);
          paintChip(octx, 0, 0, chipLength, 300 + index * 5 + Math.floor(elapsed * 6));
          octx.restore();
        }

        // A scrawled "+N" pops in beside the balance and fades away.
        const appear = Math.min(1, Math.max(0, (elapsed - 0.14) / 0.22));
        const leave = Math.min(1, Math.max(0, (elapsed - 1.35) / 0.45));
        if (appear > 0 && leave < 1) {
          const pop = appear < 1 ? 0.55 + appear * 0.6 : 1 - 0.05 * Math.sin(Math.min(1, (elapsed - 0.36) * 8));
          octx.save();
          octx.globalAlpha = appear * (1 - leave);
          octx.translate(Math.min(w - 70, PAD + 108), 22);
          octx.rotate((-7 * Math.PI) / 180);
          octx.scale(pop, pop);
          const scale = 0.24;
          const cell = glyphAdvance * scale;
          drawGlyph(octx, 10, 0, 0, scale, 640, 13);
          [...String(amount)].forEach((character, i) => {
            drawGlyph(octx, Number(character), (i + 1) * cell, 0, scale, 660 + (i + 1) * 11, 13);
          });
          octx.restore();
        }
      }

      if (elapsed < endAt) {
        rafRef.current = requestAnimationFrame(frame);
      } else {
        ctxRef.current.overlay?.clearRect(0, 0, w, stateRef.current.heroH);
        drawBalance(to, 0);
        drawWrap(to, 0);
        setBusy(false);
        setPendingAmount(0);
      }
    };
    rafRef.current = requestAnimationFrame(frame);
    return endAt;
  };

  /// Tap two of two: permanent cleanup. The payoff is automatic.
  const cleanUp = (suggestion: Suggestion) => {
    if (busy || suggestion.kind === "download") return;
    setBusy(true);
    setExpanded(null);
    setPendingAmount(chipsFor(suggestion));
    setItems((current) =>
      current.map((item) => (item.suggestion.id === suggestion.id ? { ...item, state: "cleaning" as RowState } : item))
    );
    const endAt = runBurst(chipsFor(suggestion));
    later(() => removeItem(suggestion.id), 1500);
    if (endAt === 0) {
      // Reduced motion settles the credit immediately, without ceremony.
      setBusy(false);
      setPendingAmount(0);
    }
  };

  /// Trash is the recoverable alternative: a quiet fading note, never sound.
  const trash = (suggestion: Suggestion) => {
    if (busy) return;
    setExpanded(null);
    setItems((current) =>
      current.map((item) => (item.suggestion.id === suggestion.id ? { ...item, state: "trashed" as RowState } : item))
    );
    later(() => removeItem(suggestion.id), 2000);
  };

  const restock = () => {
    const next: { suggestion: Suggestion; state: RowState }[] = [];
    for (let i = 0; i < 3; i++) {
      next.push({ suggestion: POOL[(poolIndexRef.current + i) % POOL.length], state: "idle" });
    }
    poolIndexRef.current = (poolIndexRef.current + 3) % POOL.length;
    setItems(next);
  };

  const creditedBytes = balance * 100e6 + SCRAP_BYTES;
  const paperCaption = pendingAmount > 0 ? `Up to ${pendingAmount} chips pending` : " ";
  const borderD = useMemo(
    () => handPathD(roundedRectSamples(1, 1, panelW - 2, PANEL_H - 2, 13, 26), true, 1.3, 11),
    [panelW]
  );

  return (
    <div className="w-full max-w-[380px]" ref={shellRef}>
      <div className="animate-panel-in motion-reduce:animate-none relative mx-auto mt-3" style={{ width: panelW }}>
        <Tape uid="pt" className="absolute -top-3 left-1/2 z-[5] -translate-x-1/2 -rotate-3" />
        <div
          className="paper-dots relative flex flex-col overflow-hidden rounded-[13px] drop-shadow-[0_2px_6px_rgba(51,48,43,0.13)]"
          style={{ height: PANEL_H }}
        >
          <svg className="pointer-events-none absolute inset-0 z-[6]" width={panelW} height={PANEL_H} aria-hidden="true">
            <path d={borderD} fill="none" stroke={tea.ink} strokeWidth={1.4} strokeLinecap="round" strokeLinejoin="round" />
          </svg>

          <div className="shrink-0 px-5 pb-1 pt-[16px]">
            <ShopSign fishHeight={20} wordHeight={17} uid="pm" />
          </div>

          {/* The hero: balance to wrap. The collect overlay covers exactly this. */}
          <div className="relative shrink-0 px-5 pt-1.5" ref={heroRef}>
            <canvas ref={balRef} className="block" aria-label={`${balance} chips in the paper`} role="img" />
            <div className="text-[11px] text-ink-soft">your tea, in the paper</div>
            <div className="mt-[9px] text-[11px] tabular-nums text-ink-soft">
              {space(creditedBytes)} credited · {space(100e6 - SCRAP_BYTES)} to your next chip
            </div>
            <Meter width={innerW} fraction={SCRAP_BYTES / 100e6} />
            <canvas ref={wrapRef} className="mt-2 block" aria-hidden="true" />
            <div className="mt-[3px] h-[14px] text-[11px] leading-[14px] text-ink-soft">{paperCaption}</div>
            <canvas ref={overlayRef} className="pointer-events-none absolute inset-0 z-[4]" aria-hidden="true" />
          </div>

          <div className="flex min-h-0 flex-1 flex-col gap-[7px] overflow-y-auto px-5 pb-[15px] pt-2.5">
            <div className="flex items-center gap-2.5">
              <StorageMugSvg fraction={FREE_BYTES / TOTAL_BYTES} size={40} uid="sm" />
              <div>
                <div className="text-xs font-semibold">{space(FREE_BYTES)} free on this Mac</div>
                <div className="text-[10px] text-ink-soft">
                  of {space(TOTAL_BYTES)} · room for a good few chips yet
                </div>
              </div>
            </div>
            <Rule />

            <h3 className="my-0 text-[13.5px] font-semibold">
              <Underlined seed={181} strokeWidth={3.4}>
                Make a bit of room.
              </Underlined>
            </h3>

            {items.length > 0 ? (
              <InkBox as="div" variant="card" seed={187} radius={12} className="w-full">
                {items.map(({ suggestion, state }, index) => (
                  <div key={suggestion.id}>
                    {index > 0 && (
                      <div className="px-2.5">
                        <Rule />
                      </div>
                    )}
                    {state === "cleaning" ? (
                      <div className="flex items-center gap-2 px-3 py-2">
                        <InkSpinner />
                        <span className="text-xs font-semibold">Cleaning up…</span>
                        <span className="ml-auto whitespace-nowrap text-[10px] text-ink-soft">
                          {space(suggestion.bytes)} estimated
                        </span>
                      </div>
                    ) : state === "trashed" ? (
                      <div className="animate-[fade-note_2s_ease-in-out_both] px-3 py-2 text-[11px] text-ink-soft motion-reduce:animate-none">
                        Moved to Trash. Nothing is freed until Trash empties, and no chips are earned.
                      </div>
                    ) : (
                      <>
                        <button
                          type="button"
                          onClick={() => setExpanded(expanded === suggestion.id ? null : suggestion.id)}
                          className="flex w-full cursor-pointer items-baseline justify-between gap-2 border-none bg-transparent px-3 py-2 text-left font-sans"
                          aria-expanded={expanded === suggestion.id}
                        >
                          <span className="text-xs font-semibold text-ink">{suggestion.title}</span>
                          <span className="whitespace-nowrap text-[10px] text-ink-soft">
                            {space(suggestion.bytes)} estimated
                          </span>
                        </button>
                        <div className={`reveal ${expanded === suggestion.id ? "open" : ""}`}>
                          <div>
                            <div className="px-3 pb-2">
                              <div className="overflow-hidden text-ellipsis whitespace-nowrap font-mono text-[9px] text-ink-soft">
                                {suggestion.path}
                              </div>
                              <div className="mt-1 text-[10px] text-ink-soft">
                                {suggestion.kind === "download"
                                  ? `Quiet for ${suggestion.quietDays} days · Trash only`
                                  : `Quiet for ${suggestion.quietDays} days · permanent · up to ${chipsFor(suggestion)} chips, if credited`}
                              </div>
                              <div className="mt-1 text-[10px] text-ink-soft">
                                {suggestion.kind === "build"
                                  ? "Compiled files will be removed. You will need to rebuild."
                                  : suggestion.kind === "dependencies"
                                    ? "Dependencies will be removed. You will need to reinstall them."
                                    : "Check the installer yourself before moving it to Trash."}
                              </div>
                              <div className="mt-1.5 flex flex-wrap gap-1.5">
                                {suggestion.kind !== "download" ? (
                                  <InkBox
                                    variant="primary"
                                    seed={205 + suggestion.seed}
                                    onClick={() => cleanUp(suggestion)}
                                    disabled={busy}
                                    className="px-3 py-1 text-[11px]"
                                  >
                                    Delete permanently
                                  </InkBox>
                                ) : null}
                                <InkBox
                                  variant="quiet"
                                  seed={101 + suggestion.seed}
                                  onClick={() => trash(suggestion)}
                                  disabled={busy}
                                  className="px-3 py-1 text-[11px]"
                                >
                                  Move to Trash
                                </InkBox>
                              </div>
                            </div>
                          </div>
                        </div>
                      </>
                    )}
                  </div>
                ))}
              </InkBox>
            ) : (
              <InkBox as="div" variant="card" seed={191} radius={12} className="w-full">
                <div className="flex items-center gap-2.5 px-2.5 py-2.5">
                  <MugDoodle size={32} />
                  <span className="text-xs font-semibold">Nothing to clean up. Stick the kettle on.</span>
                  <InkBox variant="quiet" seed={141} onClick={restock} className="ml-auto shrink-0 px-2.5 py-1.5 text-[11px]">
                    Have another nosey
                  </InkBox>
                </div>
              </InkBox>
            )}
          </div>

          <TabsBar />
        </div>
      </div>
    </div>
  );
}
