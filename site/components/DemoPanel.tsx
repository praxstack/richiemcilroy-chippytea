"use client";

// The tray panel, laid out exactly as the app's home screen (Views.swift,
// CoinsPage): masthead, the saved-space headline, the storage mug, "Make a
// bit of room." with the engine-style suggestion rows, then "Your chips." —
// the 156 × 64 wrap beside the hand-lettered count, caption and meter.
// Choosing "Clean up" opens ReviewTakeover verbatim (header, item card,
// consequence well, the permanent/Trash footer). Confirming runs the cleanup
// spinner, then ChipCollectionOverlay's geometry: entry at y = −6 under the
// tape, control at 0.22–0.38 of the landing height, chips landing in the
// wrap's heap rect (+40, +22, 76 × 16), the scrawled +N at (W − 92, wrap − 32).
// Counter eases over 1.35 s from t = 0.03, the wrap squashes at t = 0.81 and
// springs back at t = 0.98, chips fly 0.86 s each, 34 ms apart.
// Numbers are fictional examples, not storage measurements.

import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import {
  tea,
  inkA,
  handPathD,
  roundedRectSamples,
  lineSamples,
  circleSamples,
  space,
  chipsPhrase,
} from "@/lib/ink";
import { paintBalance, paintWrap, paintChip, drawGlyph } from "@/lib/draw";
import { glyphAdvance } from "@/lib/art";
import { playChime } from "@/lib/chime";
import { FishSvg, WordmarkSvg, Tape, Underlined, Dash, MugDoodle, StorageMugSvg, Rule } from "./art";
import { InkBox } from "./InkBox";
import {
  InfoCircle,
  ChevronLeft,
  XMark,
  ExclamationCircle,
  TrashGlyph,
  ArrowTurnDownRight,
  Magnifier,
  ClockArrow,
  Gear,
  Hammer,
  ShippingBox,
  ArrowDownDoc,
} from "./sf";

const PANEL_W = 380;
const PANEL_H = 620;
const PAD = 20;
const WRAP_W = 156;
const WRAP_H = 64;
const DIGIT_H = 28;
const NUM_H = DIGIT_H + 10;
const CHIP_BYTES = 100e6;

// The starting ledger: 85.43 GB saved → 854 chips with 33.5 MB of scraps,
// so "66.5 MB to your next chip"; 38.69 of 994.66 GB free — time for a tidy.
const START_CREDITED = 85_433_500_000;
const START_FREE = 38_690_000_000;
const TOTAL_BYTES = 994_660_000_000;

type Kind = "cargo" | "node" | "download";

// The engine's own words for each rule, verbatim from core/src/scanner.rs.
const KIND_COPY: Record<Kind, { category: string; explanation: string; consequence: string }> = {
  cargo: {
    category: "Build artifacts",
    explanation: "Cargo project evidence and a standard target marker identify this project's default build output.",
    consequence: "The next build must recompile. Compiled applications and binaries inside this target directory are removed too.",
  },
  node: {
    category: "Dependencies",
    explanation:
      "A project manifest and a supported npm, Yarn classic, Bun or pnpm lockfile identify this installed dependency directory. Workspace membership is verified when the lock belongs to an ancestor.",
    consequence:
      "Dependencies must be reinstalled from the owning project or workspace before it runs. Network access and compatible installation options may be needed. Linked source files outside this directory are preserved.",
  },
  download: {
    category: "Review a download",
    explanation:
      "A large local download in your authorized Downloads location. Review its contents; age and size do not prove it is no longer needed.",
    consequence:
      "Review the file before moving it to Trash. You can restore it while it remains in Trash. Personal files never earn chips.",
  },
};

interface Candidate {
  id: string;
  title: string;
  path: string;
  kind: Kind;
  bytes: number;
  logicalBytes: number;
  fileCount: number;
}

// Fictional examples of the suggestions the engine recommends. Downloads stay Trash-only.
const POOL: Candidate[] = [
  { id: "old-tool-target", title: "old-tool build artifacts", path: "~/Projects/old-tool/target", kind: "cargo", bytes: 2.4e9, logicalBytes: 2.31e9, fileCount: 41_320 },
  { id: "old-website-node", title: "old-website dependencies", path: "~/Projects/old-website/node_modules", kind: "node", bytes: 1.2e9, logicalBytes: 1.13e9, fileCount: 96_412 },
  { id: "helper-dmg", title: "helper-app.dmg", path: "~/Downloads/helper-app.dmg", kind: "download", bytes: 580e6, logicalBytes: 579.7e6, fileCount: 1 },
  { id: "side-project-target", title: "side-project build artifacts", path: "~/Projects/side-project/target", kind: "cargo", bytes: 3.1e9, logicalBytes: 2.96e9, fileCount: 58_204 },
  { id: "old-tool-node", title: "old-tool dependencies", path: "~/Projects/old-tool/node_modules", kind: "node", bytes: 900e6, logicalBytes: 868e6, fileCount: 74_951 },
  { id: "design-kit-pkg", title: "design-kit.pkg", path: "~/Downloads/design-kit.pkg", kind: "download", bytes: 340e6, logicalBytes: 339.5e6, fileCount: 1 },
];

const chipsFor = (candidate: Candidate) => Math.floor(candidate.bytes / CHIP_BYTES);

/// StorageStrip's flavour line, thresholds verbatim.
function flavour(fraction: number) {
  if (fraction > 0.5) return "plenty of room in the pot";
  if (fraction > 0.2) return "room for a good few chips yet";
  if (fraction > 0.08) return "getting cosy in here";
  return "nearly full — time for a tidy";
}

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

const strokeProps = { fill: "none", strokeLinecap: "round", strokeLinejoin: "round" } as const;

/// ScribbleRing's samples: a circle scribbled around, then partly retraced.
function scribbleRingD(w: number, h: number, seed: number, amplitude = 1) {
  const center = { x: w / 2, y: h / 2 };
  const radius = Math.min(w, h) / 2 - 1;
  const samples = [
    ...circleSamples(center, radius, 15, -0.5),
    ...circleSamples({ x: center.x + 0.6, y: center.y - 0.4 }, radius * 0.94, 6, -0.5, Math.PI * 0.75),
  ];
  return handPathD(samples, false, amplitude, seed);
}

/// ChipProgressMeter: one faint ruled line, a gold scribble over the part earned.
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
    <svg width={width} height={4} className="block" aria-hidden="true">
      <path d={base} stroke={inkA(0.2)} strokeWidth={1.4} fill="none" strokeLinecap="round" />
      {fraction > 0.005 && <path d={earned} stroke={tea.gold} strokeWidth={3} fill="none" strokeLinecap="round" />}
    </svg>
  );
}

/// CleanupProgressRing: activity, not a percentage — a scribbled ring, spun.
function CleanupSpinner() {
  const ringD = useMemo(() => scribbleRingD(22, 22, 143, 0.4), []);
  return (
    <span className="relative block h-[22px] w-[22px]">
      <svg width={22} height={22} className="absolute inset-0" aria-hidden="true">
        <path d={ringD} stroke={inkA(0.12)} strokeWidth={1.4} {...strokeProps} />
      </svg>
      <svg
        width={22}
        height={22}
        className="absolute inset-0 animate-spin [animation-duration:1.4s] motion-reduce:animate-none"
        aria-hidden="true"
      >
        <path
          d={ringD}
          stroke={tea.biro}
          strokeWidth={1.6}
          pathLength={100}
          strokeDasharray="60 40"
          strokeDashoffset={-5}
          {...strokeProps}
        />
      </svg>
    </span>
  );
}

/// ChipsDoodle: three chips standing upright, golden. The tab icon and the slip's.
function ChipsDoodleSvg({ size = 17, seed = 23, className }: { size?: number; seed?: number; className?: string }) {
  const chips = useMemo(() => {
    const poses = [
      { dx: -0.28, dy: 0.06, tilt: -11, len: 0.74 },
      { dx: 0.29, dy: 0.08, tilt: 12, len: 0.68 },
      { dx: 0, dy: 0, tilt: 2, len: 0.94 },
    ];
    return poses.map((pose, i) => {
      const L = size * pose.len;
      const T = L * 0.3;
      return {
        d: handPathD(roundedRectSamples(-L / 2, -T / 2, L, T, T * 0.42, 5), true, L * 0.03, seed + i * 7),
        transform: `translate(${size / 2 + pose.dx * size} ${size / 2 + pose.dy * size}) rotate(${-90 + pose.tilt})`,
        outlineW: Math.max(1, L * 0.055),
      };
    });
  }, [size, seed]);
  return (
    <svg width={size} height={size} className={className} aria-hidden="true">
      {chips.map((chip, i) => (
        <g key={i} transform={chip.transform}>
          <path d={chip.d} fill={tea.gold} />
          <path d={chip.d} stroke={tea.ink} strokeWidth={chip.outlineW} {...strokeProps} />
        </g>
      ))}
    </svg>
  );
}

/// ArtifactIcon: the suggestion's glyph in a drawn box — biro-tinted for
/// developer artifacts, gold for a download.
function ArtifactIcon({ kind, size = 26, seed, className = "" }: { kind: Kind; size?: number; seed: number; className?: string }) {
  const d = useMemo(
    () => handPathD(roundedRectSamples(0.8, 0.8, size - 1.6, size - 1.6, 8, 7), true, 0.7, seed),
    [size, seed]
  );
  const developer = kind !== "download";
  const glyphSize = Math.round(size * 0.5);
  return (
    <span className={`relative inline-flex items-center justify-center text-ink ${className}`} style={{ width: size, height: size }} aria-hidden="true">
      <svg width={size} height={size} className="absolute inset-0">
        <path d={d} fill={developer ? "rgba(63, 95, 168, 0.10)" : "rgba(242, 182, 60, 0.22)"} />
        <path d={d} stroke={inkA(0.4)} strokeWidth={1.1} {...strokeProps} />
      </svg>
      <span className="relative flex">
        {kind === "cargo" ? <Hammer size={glyphSize} /> : kind === "node" ? <ShippingBox size={glyphSize} /> : <ArrowDownDoc size={glyphSize} />}
      </span>
    </span>
  );
}

/// InkCheckboxStyle: a hand-drawn box; ticking it draws an ink checkmark.
function InkCheckbox({ on, onToggle, seed, children }: { on: boolean; onToggle: () => void; seed: number; children: React.ReactNode }) {
  const boxD = useMemo(() => handPathD(roundedRectSamples(0.8, 0.8, 14.4, 14.4, 3.5, 5), true, 0.7, seed), [seed]);
  const checkD = useMemo(
    () => handPathD([{ x: 2.9, y: 8.3 }, { x: 6.7, y: 12.5 }, { x: 13.8, y: 2.9 }], false, 0.5, 95),
    []
  );
  return (
    <button
      type="button"
      role="checkbox"
      aria-checked={on}
      onClick={onToggle}
      className="flex cursor-pointer items-center gap-[7px] self-start border-none bg-transparent p-0 text-left font-sans text-[10px] text-ink"
    >
      <svg width={16} height={16} aria-hidden="true">
        <path d={boxD} fill={on ? "rgba(63, 95, 168, 0.10)" : tea.card} stroke={inkA(on ? 0.85 : 0.55)} strokeWidth={1.3} strokeLinecap="round" strokeLinejoin="round" />
        <path
          d={checkD}
          stroke={tea.biro}
          strokeWidth={2.1}
          pathLength={1}
          strokeDasharray={1}
          strokeDashoffset={on ? 0 : 1}
          className="transition-[stroke-dashoffset] duration-150 ease-out motion-reduce:transition-none"
          {...strokeProps}
        />
      </svg>
      {children}
    </button>
  );
}

/// HowChipsWorkSlip: the slip of paper the (i) opens — four honest lines.
function HowChipsWorkSlip({ onClose }: { onClose: () => void }) {
  const lines = [
    { text: "Clean something up for good and the freed space is measured conservatively, then counted as saved.", seed: 211 },
    { text: "Every 100 MB saved earns one chip. Anything smaller is scraps — carried forward, never lost.", seed: 213 },
    { text: "Moving files to Trash earns no chips; nothing is freed until Trash empties.", seed: 215 },
    { text: "Chips stay on your Mac and have no monetary value. They’re just your tea.", seed: 217 },
  ];
  return (
    <InkBox as="div" variant="card" seed={209} radius={12} className="mt-2 w-full">
      <div className="flex flex-col gap-1.5 p-2.5">
        <div className="flex items-center gap-[7px]">
          <ChipsDoodleSvg size={15} seed={43} />
          <span className="text-xs font-semibold">How chips work</span>
          <button
            type="button"
            onClick={onClose}
            className="ml-auto flex cursor-pointer border-none bg-transparent p-0.5 text-ink-soft hover:text-ink"
            aria-label="Close how chips work"
          >
            <XMark size={10} />
          </button>
        </div>
        {lines.map((line) => (
          <div key={line.seed} className="flex items-start gap-[7px]">
            <Dash seed={line.seed} className="mt-[5px] shrink-0" />
            <span className="text-[10px] leading-[1.45] text-ink-soft">{line.text}</span>
          </div>
        ))}
      </div>
    </InkBox>
  );
}

/// The bottom bar: three labeled tabs, a drawn divider, the settings gear.
/// Only the chippytea tab is home in this demo; the rest sit it out.
function TabsBar() {
  const art = useMemo(
    () => ({
      topLine: handPathD(lineSamples({ x: 1, y: 1.5 }, { x: 359, y: 1.5 }, 16), false, 0.8, 171),
      divider: handPathD(lineSamples({ x: 1, y: 1 }, { x: 21, y: 1 }, 16), false, 0.6, 167),
      ring: scribbleRingD(28, 25, 168),
    }),
    []
  );

  const tab = (label: string, selected: boolean, iconOnly: boolean, icon: React.ReactNode) => (
    <span
      className={`flex cursor-default flex-col items-center gap-0.5 pb-[3px] pt-[5px] ${
        iconOnly ? "px-2" : "flex-1 px-0.5"
      } ${selected ? "text-biro" : "text-ink-soft hover:bg-[rgba(241,234,219,0.8)]"}`}
      title={selected ? label : `${label}: just the home screen in this demo`}
    >
      <span className="relative flex h-[26px] items-center justify-center" style={iconOnly ? { width: 30 } : undefined}>
        {selected && (
          <svg width={28} height={25} className="absolute" aria-hidden="true">
            <path d={art.ring} stroke={tea.biro} strokeWidth={1.4} {...strokeProps} />
          </svg>
        )}
        <span className="relative flex">{icon}</span>
      </span>
      {!iconOnly && <span className="whitespace-nowrap text-[10px] leading-[12px]">{label}</span>}
    </span>
  );

  return (
    <div className="relative z-[1] shrink-0 bg-[rgba(241,234,219,0.55)]" role="presentation">
      <svg viewBox="0 0 360 3" preserveAspectRatio="none" className="absolute left-0 top-0 block h-[3px] w-full" aria-hidden="true">
        <path d={art.topLine} stroke={inkA(0.3)} strokeWidth={1.2} {...strokeProps} />
      </svg>
      <div className="flex items-center px-2.5 pb-[5px] pt-1">
        {tab("chippytea", true, false, <ChipsDoodleSvg size={17} seed={23} />)}
        {tab("Find space", false, false, <Magnifier size={15} />)}
        {tab("Activity", false, false, <ClockArrow size={15} />)}
        <span className="flex w-3 shrink-0 justify-center" aria-hidden="true">
          <svg width={2} height={22}>
            <path d={art.divider} stroke={inkA(0.22)} strokeWidth={1.1} transform="rotate(90 1 1)" {...strokeProps} />
          </svg>
        </span>
        {tab("Settings", false, true, <Gear size={15} />)}
      </div>
    </div>
  );
}

/// ReviewItemCard: the exact item and its consequences, before you confirm.
function ReviewItemCard({ candidate }: { candidate: Candidate }) {
  const copy = KIND_COPY[candidate.kind];
  const chips = chipsFor(candidate);
  const permanentEligible = candidate.kind !== "download";
  return (
    <InkBox as="div" variant="card" seed={321} radius={12} className="w-full drop-shadow-[0_1.5px_1.5px_rgba(51,48,43,0.10)]">
      <div className="flex flex-col gap-[7px] p-[9px]">
        <div className="flex items-start gap-2.5">
          <ArtifactIcon kind={candidate.kind} size={30} seed={326} className="shrink-0" />
          <div className="flex min-w-0 flex-col gap-0.5">
            <span className="truncate text-xs font-semibold">{candidate.title}</span>
            <span className="break-all font-mono text-[9px] leading-[1.4] text-ink-soft">{candidate.path}</span>
          </div>
          <div className="ml-auto flex shrink-0 flex-col items-end">
            <span className="text-xs font-bold tabular-nums">{space(candidate.bytes)}</span>
            <span className="text-[10px] text-ink-soft">estimated</span>
          </div>
        </div>
        <span className="text-[10px] leading-[1.45] text-ink-soft">{copy.explanation}</span>
        <InkBox as="div" variant="well" seed={330} radius={8} className="w-full">
          <div className="flex items-start gap-[5px] p-2">
            <ArrowTurnDownRight size={11} className="mt-px shrink-0" />
            <span className="text-[10px] leading-[1.45]">{copy.consequence}</span>
          </div>
        </InkBox>
        <div className="flex items-baseline gap-1.5 text-[10px] text-ink-soft">
          <span className="min-w-0 truncate whitespace-nowrap tabular-nums">
            {space(candidate.logicalBytes)} file size · {candidate.fileCount.toLocaleString("en-US")} files
          </span>
          {permanentEligible && (
            <span className="ml-auto shrink-0 whitespace-nowrap text-[9.5px] text-gold-deep">
              {chips === 0 ? "Permanent: scraps towards your next chip" : `Permanent: up to ${chipsPhrase(chips)}, if credited`}
            </span>
          )}
        </div>
      </div>
    </InkBox>
  );
}

/// ReviewTakeover: one last look before anything is removed.
function ReviewTakeover({
  candidate,
  shown,
  onBack,
  onDelete,
  onTrash,
}: {
  candidate: Candidate;
  shown: boolean;
  onBack: () => void;
  onDelete: (dontAsk: boolean) => void;
  onTrash: () => void;
}) {
  const [dontAsk, setDontAsk] = useState(false);
  const permanentEligible = candidate.kind !== "download";
  const footerLine = useMemo(() => handPathD(lineSamples({ x: 1, y: 1.5 }, { x: 359, y: 1.5 }, 16), false, 0.8, 339), []);
  return (
    <div
      className={`paper-dots absolute inset-0 z-[5] flex flex-col rounded-[13px] transition-[transform,opacity] duration-300 ease-out motion-reduce:transition-[opacity] ${
        shown ? "translate-x-0 opacity-100" : "translate-x-full opacity-0 motion-reduce:translate-x-0"
      }`}
      role="dialog"
      aria-label="One last look"
    >
      <div className="flex shrink-0 flex-col items-start gap-[7px] px-5 pb-[9px] pt-2.5">
        <InkBox variant="quiet" seed={311} onClick={onBack} className="px-2.5 py-1.5 text-[11px]" ariaLabel="Back to findings">
          <span className="flex items-center gap-1">
            <ChevronLeft size={9} />
            Back
          </span>
        </InkBox>
        <div className="flex flex-col gap-[3px]">
          <h3 className="my-0 text-[16px] font-bold leading-[1.2]">
            <Underlined seed={313}>One last look.</Underlined>
          </h3>
          <span className="text-[10px] tabular-nums text-ink-soft">1 item · {space(candidate.bytes)} estimated on disk</span>
        </div>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto px-5 pb-3 pt-0.5">
        <div className="flex flex-col gap-2">
          <ReviewItemCard candidate={candidate} />
          <div className="flex items-start gap-[7px] pt-px text-ink-soft">
            <InfoCircle size={12} className="mt-px shrink-0" />
            <span className="text-[10px] leading-[1.45]">
              Every item is checked again before removal. Changed items need a fresh review. Freed space and chips may be
              lower than estimates.
            </span>
          </div>
        </div>
      </div>
      <div className="relative shrink-0 bg-[rgba(241,234,219,0.7)] px-5 pb-2.5 pt-2">
        <svg viewBox="0 0 360 3" preserveAspectRatio="none" className="absolute left-0 top-0 block h-[3px] w-full" aria-hidden="true">
          <path d={footerLine} stroke={inkA(0.35)} strokeWidth={1.2} {...strokeProps} />
        </svg>
        <div className="flex flex-col gap-2">
          <div className="flex items-start gap-2">
            <span className="mt-px shrink-0">{permanentEligible ? <ExclamationCircle size={13} /> : <TrashGlyph size={13} />}</span>
            <div className="flex flex-col gap-0.5">
              <span className="text-xs font-semibold">{permanentEligible ? "Permanent cleanup" : "Move to Trash"}</span>
              <span className="text-[10px] leading-[1.4] text-ink-soft">
                {permanentEligible
                  ? "Deletes these developer files without Trash or restore. Freed space and chips may be zero."
                  : "Recoverable from Activity until Trash is emptied. Moving files there does not free space or earn chips."}
              </span>
            </div>
          </div>
          {permanentEligible ? (
            <>
              <InkCheckbox on={dontAsk} onToggle={() => setDontAsk((v) => !v)} seed={341}>
                Don’t ask again
              </InkCheckbox>
              <InkBox variant="destructive" seed={333} onClick={() => onDelete(dontAsk)} className="w-full px-3.5 py-2.5 text-xs text-rust">
                Delete permanently
              </InkBox>
              <Rule />
              <div className="flex flex-col gap-[5px]">
                <InkBox variant="quiet" seed={337} onClick={onTrash} className="w-full px-2.5 py-1.5 text-[11px]">
                  Move to Trash instead
                </InkBox>
                <span className="text-[10px] text-ink-soft">Recoverable from Activity until Trash is emptied.</span>
              </div>
            </>
          ) : (
            <>
              <InkBox variant="primary" seed={333} onClick={onTrash} className="w-full px-3.5 py-2.5 text-xs">
                Move to Trash
              </InkBox>
              <span className="text-[10px] leading-[1.4] text-ink-soft">
                For permanent cleanup, select only eligible developer artifacts. Personal downloads go to Trash only.
              </span>
            </>
          )}
        </div>
      </div>
    </div>
  );
}

type Busy = { step: "cleaning" | "burst"; permanent: boolean; chips: number };

export function DemoPanel() {
  const shellRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const wrapRef = useRef<HTMLCanvasElement>(null);
  const numRef = useRef<HTMLCanvasElement>(null);
  const overlayRef = useRef<HTMLCanvasElement>(null);
  const ctxRef = useRef<{
    wrap: CanvasRenderingContext2D | null;
    num: CanvasRenderingContext2D | null;
    overlay: CanvasRenderingContext2D | null;
  }>({ wrap: null, num: null, overlay: null });

  const [panelW, setPanelW] = useState(PANEL_W);
  const [credited, setCredited] = useState(START_CREDITED);
  const [free, setFree] = useState(START_FREE);
  const [remaining, setRemaining] = useState<Candidate[]>(POOL);
  const [review, setReview] = useState<Candidate | null>(null);
  const [reviewShown, setReviewShown] = useState(false);
  const [confirmFirst, setConfirmFirst] = useState(true);
  const [showHow, setShowHow] = useState(false);
  const [busy, setBusy] = useState<Busy | null>(null);
  const stateRef = useRef({ credited: START_CREDITED, panelW: PANEL_W, numW: PANEL_W - PAD * 2 - WRAP_W - 12 });
  const rafRef = useRef(0);
  const timersRef = useRef<ReturnType<typeof setTimeout>[]>([]);
  const wrapKeyRef = useRef("");
  const numKeyRef = useRef("");

  const suggestions = remaining.slice(0, 3);
  const balance = Math.floor(credited / CHIP_BYTES);
  const fractional = credited - balance * CHIP_BYTES;
  const numW = Math.max(60, panelW - PAD * 2 - WRAP_W - 12);

  const reduced = () => window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  const drawWrap = (chips: number, boil: number, squash = 0) => {
    const key = `${chips}|${boil}|${squash.toFixed(3)}`;
    if (key === wrapKeyRef.current) return;
    wrapKeyRef.current = key;
    const ctx = ctxRef.current.wrap;
    if (!ctx) return;
    ctx.clearRect(0, 0, WRAP_W, WRAP_H);
    ctx.save();
    if (squash !== 0) {
      // ChipPortion's landing: scaleEffect(x: 1.03, y: 0.96, anchor: .bottom).
      const sx = 1 + 0.03 * squash;
      const sy = 1 - 0.04 * squash;
      ctx.translate((WRAP_W / 2) * (1 - sx), WRAP_H * (1 - sy));
      ctx.scale(sx, sy);
    }
    // The wrap draws at half its design size, per ChipPortion(scale: 0.5).
    ctx.scale(0.5, 0.5);
    paintWrap(ctx, WRAP_W * 2, WRAP_H * 2, chips, boil);
    ctx.restore();
  };

  const drawNumber = (value: number, boil: number) => {
    const whole = Math.max(0, Math.floor(value));
    const key = `${whole}|${boil}`;
    if (key === numKeyRef.current) return;
    numKeyRef.current = key;
    const ctx = ctxRef.current.num;
    if (!ctx) return;
    ctx.clearRect(0, 0, stateRef.current.numW, NUM_H);
    paintBalance(ctx, whole, DIGIT_H, boil);
  };

  useLayoutEffect(() => {
    const shell = shellRef.current;
    if (!shell) return;
    const measure = () => setPanelW(Math.min(PANEL_W, Math.floor(shell.offsetWidth)));
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(shell);
    return () => observer.disconnect();
  }, []);

  useLayoutEffect(() => {
    stateRef.current.panelW = panelW;
    stateRef.current.numW = Math.max(60, panelW - PAD * 2 - WRAP_W - 12);
    const content = contentRef.current;
    if (!content || !wrapRef.current || !numRef.current || !overlayRef.current) return;
    ctxRef.current.wrap = setup(wrapRef.current, WRAP_W, WRAP_H);
    ctxRef.current.num = setup(numRef.current, stateRef.current.numW, NUM_H);
    ctxRef.current.overlay = setup(overlayRef.current, content.clientWidth, content.clientHeight);
    wrapKeyRef.current = "";
    numKeyRef.current = "";
    const chips = Math.floor(stateRef.current.credited / CHIP_BYTES);
    drawWrap(chips, 0);
    drawNumber(chips, 0);
  }, [panelW]);

  // The one-second ink flourish when the panel first appears.
  useEffect(() => {
    if (reduced()) return;
    let step = 0;
    const id = setInterval(() => {
      step += 1;
      if (step > 6) {
        clearInterval(id);
        return;
      }
      const chips = Math.floor(stateRef.current.credited / CHIP_BYTES);
      drawWrap(chips, step % 3);
      drawNumber(chips, step % 3);
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

  const openReview = (candidate: Candidate) => {
    setReview(candidate);
    requestAnimationFrame(() => requestAnimationFrame(() => setReviewShown(true)));
  };

  const closeReview = () => {
    setReviewShown(false);
    later(() => setReview(null), 300);
  };

  /// The collect moment — ChipCollectionOverlay's geometry, over the home content.
  const runBurst = (from: number, to: number) => {
    const amount = to - from;
    playChime();

    const overlay = ctxRef.current.overlay;
    const content = contentRef.current;
    const wrapEl = wrapRef.current;
    if (!overlay || !content || !wrapEl) {
      drawNumber(to, 0);
      drawWrap(to, 0);
      setBusy(null);
      return;
    }
    const w = stateRef.current.panelW;
    const contentRect = content.getBoundingClientRect();
    const wrapRect = wrapEl.getBoundingClientRect();
    const wrapX = wrapRect.left - contentRect.left;
    const wrapY = wrapRect.top - contentRect.top + content.scrollTop;
    const landing = { x: wrapX + 40, y: wrapY + 22, w: 76, h: 16 };
    const tallyOrigin = { x: w - 92, y: Math.max(0, wrapY - 32) };
    const entry = { x: Math.min(Math.max(w / 2, 24), w - 24), y: -6 };
    const H = content.clientHeight;

    const start = performance.now();
    const count = Math.min(24, Math.max(6, amount));
    const endAt = Math.max(1.95, count * 0.034 + 0.86 + 0.1);

    const frame = (now: number) => {
      const elapsed = (now - start) / 1000;
      const boil = elapsed < 1.002 ? (Math.floor(elapsed / 0.167) + 1) % 3 : 0;

      // The counter rolls up in hand-lettered digits.
      drawNumber(from + amount * easeOut((elapsed - 0.03) / 1.35), boil);

      // The wrap squashes as the chips land, then springs back.
      let squash = 0;
      if (elapsed >= 0.81 && elapsed < 0.98) squash = springTo(elapsed - 0.81, 0.2);
      else if (elapsed >= 0.98) squash = springTo(0.17, 0.2) * (1 - springTo(elapsed - 0.98, 0.34));
      if (Math.abs(squash) < 0.004 && elapsed > 1.6) squash = 0;
      drawWrap(to, boil, squash);

      // Chips tumble out from under the tape, trailing sketched motion lines.
      overlay.clearRect(0, 0, w, H);
      for (let index = 0; index < count; index++) {
        const progress = (elapsed - index * 0.034) / 0.86;
        if (progress <= 0 || progress >= 1) continue;
        const t = progress;
        const seed = ((index * 73 + 19) % 101) / 101;
        const depth = ((index * 37 + 11) % 101) / 101;
        const end = { x: landing.x + landing.w * seed, y: landing.y + landing.h * depth };
        const control = { x: entry.x + (end.x - entry.x) * (0.15 + seed * 0.35), y: end.y * (0.22 + seed * 0.16) };
        const here = quadPoint(entry, control, end, t);
        const fade = Math.min(1, progress * 9) * Math.min(1, (1 - progress) * 9);
        const chipLength = 17 + seed * 10;

        overlay.save();
        overlay.globalAlpha = fade * 0.55;
        for (let line = 0; line < 3; line++) {
          const lag = 0.05 + line * 0.035;
          const a = quadPoint(entry, control, end, Math.max(0, t - lag - 0.05));
          const b = quadPoint(entry, control, end, Math.max(0, t - lag));
          if (Math.hypot(b.x - a.x, b.y - a.y) <= 0.6) continue;
          const offset = (line - 1) * chipLength * 0.24;
          overlay.strokeStyle = inkA(0.4);
          overlay.lineWidth = 1.2;
          overlay.lineCap = "round";
          overlay.beginPath();
          overlay.moveTo(a.x + offset, a.y);
          overlay.quadraticCurveTo((a.x + b.x) / 2 + offset * 1.4, (a.y + b.y) / 2, b.x + offset, b.y);
          overlay.stroke();
        }
        overlay.restore();

        overlay.save();
        overlay.globalAlpha = fade;
        overlay.translate(here.x, here.y);
        overlay.rotate(((progress * 250 + index * 23) * Math.PI) / 180);
        paintChip(overlay, 0, 0, chipLength, 300 + index * 5 + Math.floor(elapsed * 6));
        overlay.restore();
      }

      // A scrawled "+N" pops in beside "Your chips." and fades away.
      const appear = Math.min(1, Math.max(0, (elapsed - 0.14) / 0.22));
      const leave = Math.min(1, Math.max(0, (elapsed - 1.35) / 0.45));
      if (appear > 0 && leave < 1) {
        const pop = appear < 1 ? 0.55 + appear * 0.6 : 1 - 0.05 * Math.sin(Math.min(1, (elapsed - 0.36) * 8));
        overlay.save();
        overlay.globalAlpha = appear * (1 - leave);
        overlay.translate(tallyOrigin.x, tallyOrigin.y);
        overlay.rotate((-7 * Math.PI) / 180);
        overlay.scale(pop, pop);
        const scale = 0.24;
        const cell = glyphAdvance * scale;
        drawGlyph(overlay, 10, 0, 0, scale, 640, 13);
        [...String(amount)].forEach((character, i) => {
          drawGlyph(overlay, Number(character), (i + 1) * cell, 0, scale, 660 + (i + 1) * 11, 13);
        });
        overlay.restore();
      }

      if (elapsed < endAt) {
        rafRef.current = requestAnimationFrame(frame);
      } else {
        overlay.clearRect(0, 0, w, H);
        drawNumber(to, 0);
        drawWrap(to, 0);
        setBusy(null);
      }
    };
    rafRef.current = requestAnimationFrame(frame);
  };

  const finishCleanup = (candidate: Candidate, permanent: boolean) => {
    if (!permanent) {
      setBusy(null);
      return;
    }
    const from = Math.floor(stateRef.current.credited / CHIP_BYTES);
    const newCredited = stateRef.current.credited + candidate.bytes;
    const to = Math.floor(newCredited / CHIP_BYTES);
    stateRef.current.credited = newCredited;
    setCredited(newCredited);
    setFree((value) => value + candidate.bytes);
    if (to - from <= 0 || reduced()) {
      // Reduced motion settles the credit immediately, without ceremony.
      drawNumber(to, 0);
      drawWrap(to, 0);
      setBusy(null);
      return;
    }
    setBusy({ step: "burst", permanent: true, chips: to - from });
    runBurst(from, to);
  };

  const startCleanup = (candidate: Candidate, permanent: boolean) => {
    setRemaining((current) => current.filter((item) => item.id !== candidate.id));
    setBusy({ step: "cleaning", permanent, chips: chipsFor(candidate) });
    later(() => finishCleanup(candidate, permanent), permanent ? 1500 : 1200);
  };

  /// Tap one: the row's button. With confirmations off, eligible items delete
  /// straight away — otherwise the review takes over, exactly as in the app.
  const requestCleanup = (candidate: Candidate) => {
    if (busy) return;
    if (!confirmFirst && candidate.kind !== "download") {
      startCleanup(candidate, true);
      return;
    }
    openReview(candidate);
  };

  const confirmDelete = (candidate: Candidate, dontAsk: boolean) => {
    if (busy) return;
    if (dontAsk) setConfirmFirst(false);
    closeReview();
    startCleanup(candidate, true);
  };

  const confirmTrash = (candidate: Candidate) => {
    if (busy) return;
    closeReview();
    startCleanup(candidate, false);
  };

  const restock = () => setRemaining(POOL);

  const caption = busy
    ? busy.permanent
      ? busy.step === "cleaning"
        ? `Up to ${chipsPhrase(busy.chips)} pending`
        : `${space(CHIP_BYTES - fractional)} to your next chip`
      : "Moving to Trash…"
    : balance === 0
      ? "Your first chip’s still in the fryer."
      : `${space(CHIP_BYTES - fractional)} to your next chip`;

  const emptyMessage = busy?.step === "cleaning" ? "Your cleanup is in progress." : "Nothing to clean up. Stick the kettle on.";

  const borderD = useMemo(
    () => handPathD(roundedRectSamples(1, 1, panelW - 2, PANEL_H - 2, 13, 26), true, 1.3, 11),
    [panelW]
  );

  return (
    <div className="w-full max-w-[380px]" ref={shellRef}>
      <div className="animate-panel-in motion-reduce:animate-none relative mx-auto mt-3" style={{ width: panelW }}>
        <Tape uid="pt" className="absolute -top-3 left-1/2 z-[7] -translate-x-1/2 -rotate-3" />
        <div
          className="paper-dots relative flex flex-col overflow-hidden rounded-[13px] drop-shadow-[0_2px_6px_rgba(51,48,43,0.13)]"
          style={{ height: PANEL_H }}
        >
          <svg className="pointer-events-none absolute inset-0 z-[6]" width={panelW} height={PANEL_H} aria-hidden="true">
            <path d={borderD} fill="none" stroke={tea.ink} strokeWidth={1.4} strokeLinecap="round" strokeLinejoin="round" />
          </svg>

          {/* The masthead: the battered fish and the wordmark, ink boiling on hover. */}
          <div className="relative z-[1] shrink-0 px-5 pt-[9px]">
            <span className="boil-hover flex items-center gap-[9px]" role="img" aria-label="chippytea">
              <FishSvg height={26} uid="pm" className="-rotate-2" />
              <WordmarkSvg height={24} className="mt-1" />
            </span>
            {busy?.step === "cleaning" && (
              <span
                className="absolute right-5 top-2"
                title="Cleaning up… Your reviewed cleanup is in progress."
                role="img"
                aria-label="Cleanup in progress"
              >
                <CleanupSpinner />
              </span>
            )}
          </div>

          {/* Home. The burst overlay covers exactly this, reaching up under the tape. */}
          <div className="relative min-h-0 flex-1 overflow-y-auto" ref={contentRef}>
            <div className="flex flex-col gap-2 px-5 pt-2">
              <div className="flex flex-col gap-[5px]">
                <h3 className="my-0 text-[20px] font-bold leading-[1.2] tabular-nums">
                  <Underlined seed={175}>{space(credited)} saved.</Underlined>
                </h3>
                <p className="my-0 text-[10px] leading-[1.4] text-ink-soft">
                  Freed for good by cleanups you reviewed, measured after each one.
                </p>
              </div>

              <div
                className="flex items-center gap-[11px]"
                title="The startup disk’s available space, as macOS reports it. Other volumes and reserved space are counted in the total."
              >
                <StorageMugSvg fraction={free / TOTAL_BYTES} size={44} uid="sm" className="shrink-0" />
                <div className="flex min-w-0 flex-col gap-[1px]">
                  <span className="text-xs font-semibold tabular-nums">{space(free)} free on this Mac</span>
                  <span className="truncate text-[10px] text-ink-soft">
                    of {space(TOTAL_BYTES)} · {flavour(free / TOTAL_BYTES)}
                  </span>
                </div>
              </div>

              <Rule />

              <h3 className="my-0 text-[13.5px] font-semibold leading-[1.25]">
                <Underlined seed={181}>Make a bit of room.</Underlined>
              </h3>

              {suggestions.length > 0 ? (
                <InkBox as="div" variant="card" seed={187} radius={12} className="w-full drop-shadow-[0_1.5px_1.5px_rgba(51,48,43,0.10)]">
                  {suggestions.map((candidate, index) => (
                    <div key={candidate.id}>
                      {index > 0 && (
                        <div className="px-2.5">
                          <Rule />
                        </div>
                      )}
                      <div
                        className="flex items-center gap-[9px] px-[11px] py-2"
                        title={`${KIND_COPY[candidate.kind].category} · ${space(candidate.bytes)} estimated`}
                      >
                        <ArtifactIcon kind={candidate.kind} seed={221 + index * 8} className="shrink-0" />
                        <span className="min-w-0 flex-1 truncate text-xs font-semibold">{candidate.title}</span>
                        <span className="shrink-0 text-xs font-bold tabular-nums">{space(candidate.bytes)}</span>
                        <InkBox
                          variant={!confirmFirst && candidate.kind !== "download" ? "destructive" : "quiet"}
                          seed={224 + index * 8}
                          onClick={() => requestCleanup(candidate)}
                          disabled={!!busy}
                          className={`shrink-0 px-2.5 py-1.5 text-[11px] ${
                            !confirmFirst && candidate.kind !== "download" ? "text-rust" : ""
                          }`}
                          ariaLabel={
                            !confirmFirst && candidate.kind !== "download"
                              ? `Delete ${candidate.title} permanently`
                              : `Review cleanup for ${candidate.title}`
                          }
                        >
                          {!confirmFirst && candidate.kind !== "download" ? "Delete" : "Clean up"}
                        </InkBox>
                      </div>
                    </div>
                  ))}
                </InkBox>
              ) : (
                <InkBox as="div" variant="card" seed={191} radius={12} className="w-full drop-shadow-[0_1.5px_1.5px_rgba(51,48,43,0.10)]">
                  <div className="flex items-center gap-2.5 p-2.5">
                    <MugDoodle size={32} className="shrink-0" />
                    <span className="text-xs font-semibold">{emptyMessage}</span>
                    {!busy && (
                      <InkBox variant="quiet" seed={141} onClick={restock} className="ml-auto shrink-0 px-2.5 py-1.5 text-[11px]">
                        Have another nosey
                      </InkBox>
                    )}
                  </div>
                </InkBox>
              )}

              <Rule className="mt-1" />
            </div>

            {/* Your chips: the wrap beside the hand-lettered count. */}
            <div className="flex flex-col gap-1.5 px-5 pb-[15px] pt-2.5">
              <div className="flex items-center gap-1.5">
                <h3 className="my-0 text-[13.5px] font-semibold leading-[1.25]">
                  <Underlined seed={199}>Your chips.</Underlined>
                </h3>
                <button
                  type="button"
                  onClick={() => setShowHow((value) => !value)}
                  className={`flex cursor-pointer border-none bg-transparent p-0.5 ${showHow ? "text-biro" : "text-ink-soft hover:text-ink"}`}
                  title="How chips work"
                  aria-label="How chips work"
                  aria-expanded={showHow}
                >
                  <InfoCircle size={12} filled={showHow} />
                </button>
              </div>
              <div className="flex items-center gap-3">
                <canvas ref={wrapRef} className="block shrink-0" aria-hidden="true" />
                <div className="flex min-w-0 flex-1 flex-col gap-[5px]">
                  <canvas ref={numRef} className="block" role="img" aria-label={`${chipsPhrase(balance)} in the paper`} />
                  <span className="whitespace-nowrap text-[10px] leading-[12px] tabular-nums text-ink-soft">{caption}</span>
                  <Meter width={numW} fraction={fractional / CHIP_BYTES} />
                </div>
              </div>
              {showHow && <HowChipsWorkSlip onClose={() => setShowHow(false)} />}
            </div>

            <canvas ref={overlayRef} className="pointer-events-none absolute inset-0 z-[4]" aria-hidden="true" />
          </div>

          <TabsBar />

          {review && (
            <ReviewTakeover
              candidate={review}
              shown={reviewShown}
              onBack={closeReview}
              onDelete={(dontAsk) => confirmDelete(review, dontAsk)}
              onTrash={() => confirmTrash(review)}
            />
          )}
        </div>
      </div>
    </div>
  );
}
