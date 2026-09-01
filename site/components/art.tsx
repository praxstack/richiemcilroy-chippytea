// Static drawn chrome, rendered as inline SVG on the server. The boiling is
// three pre-drawn phases cycled by CSS — no JavaScript touches any of this.

import { tea, inkA, goldDeepA, handPathD, lineSamples, arcSamples } from "@/lib/ink";
import { fishArt, wordArt, tapeArt, handWordWidth } from "@/lib/art";

const strokeProps = {
  fill: "none",
  strokeLinecap: "round",
  strokeLinejoin: "round",
} as const;

export function FishSvg({
  height = 26,
  uid,
  phases = 3,
  className,
}: {
  height?: number;
  uid: string;
  phases?: number;
  className?: string;
}) {
  return (
    <svg
      width={(height * 64) / 44}
      height={height}
      viewBox="0 0 64 44"
      className={className}
      aria-hidden="true"
    >
      {Array.from({ length: phases }, (_, phase) => {
        const art = fishArt(401 + phase * 7);
        const clipId = `${uid}f${phase}`;
        return (
          <g key={phase} className={`bp bp${phase}`}>
            <clipPath id={clipId}>
              <path d={art.bodyD} />
            </clipPath>
            {art.steam.map((s, i) => (
              <path key={i} d={s.d} stroke={s.color} strokeWidth={s.width} {...strokeProps} />
            ))}
            <path d={art.bodyD} fill={tea.gold} />
            <path
              d={art.batterD}
              stroke={goldDeepA(0.32)}
              strokeWidth={1}
              fill="none"
              clipPath={`url(#${clipId})`}
            />
            <path d={art.bodyD} stroke={tea.ink} strokeWidth={1.7} {...strokeProps} />
            {art.details.map((s, i) => (
              <path key={i} d={s.d} stroke={s.color} strokeWidth={s.width} {...strokeProps} />
            ))}
            <circle cx={art.eye.x + art.eye.w / 2} cy={art.eye.y + art.eye.w / 2} r={art.eye.w / 2} fill={tea.ink} />
            <circle
              cx={art.glint.x + art.glint.w / 2}
              cy={art.glint.y + art.glint.w / 2}
              r={art.glint.w / 2}
              fill={tea.card}
            />
          </g>
        );
      })}
    </svg>
  );
}

export function WordmarkSvg({
  height = 23,
  color = tea.ink,
  phases = 3,
  className,
}: {
  height?: number;
  color?: string;
  phases?: number;
  className?: string;
}) {
  const unitWidth = handWordWidth("chippytea") + 1;
  return (
    <svg
      width={(unitWidth * height) / 92}
      height={height}
      viewBox={`0 0 ${unitWidth} 92`}
      className={className}
      aria-hidden="true"
    >
      {Array.from({ length: phases }, (_, phase) => {
        const art = wordArt("chippytea", 500 + phase * 9);
        return (
          <g key={phase} className={`bp bp${phase}`}>
            {art.strokes.map((d, i) => (
              <path key={i} d={d} stroke={color} strokeWidth={7.5} {...strokeProps} />
            ))}
            {art.dots.map((dot, i) => (
              <circle key={i} cx={dot.x} cy={dot.y} r={dot.r} fill={color} />
            ))}
          </g>
        );
      })}
    </svg>
  );
}

/// The shop sign: battered fish beside the hand-lettered wordmark. Boils on hover.
export function ShopSign({
  fishHeight = 26,
  wordHeight = 23,
  uid,
  className = "",
}: {
  fishHeight?: number;
  wordHeight?: number;
  uid: string;
  className?: string;
}) {
  return (
    <span
      className={`boil-hover inline-flex items-center gap-2.5 ${className}`}
      role="img"
      aria-label="chippytea"
    >
      <FishSvg height={fishHeight} uid={uid} className="-rotate-2" />
      <WordmarkSvg height={wordHeight} />
    </span>
  );
}

/// A strip of marigold washi tape, torn at both ends. Flourishes once on load.
export function Tape({ uid, className = "" }: { uid: string; className?: string }) {
  return (
    <svg width={56} height={24} viewBox="0 0 56 24" className={`boil-load ${className}`} aria-hidden="true">
      {Array.from({ length: 3 }, (_, phase) => {
        const art = tapeArt(41 + phase * 5);
        const clipId = `${uid}t${phase}`;
        return (
          <g key={phase} className={`bp bp${phase}`}>
            <clipPath id={clipId}>
              <path d={art.outlineD} />
            </clipPath>
            <path d={art.outlineD} fill="rgba(242, 182, 60, 0.62)" />
            <g clipPath={`url(#${clipId})`}>
              {art.streaks.map((s, i) => (
                <path key={i} d={s.d} stroke={goldDeepA(0.16)} strokeWidth={2.4} fill="none" />
              ))}
            </g>
            <path d={art.outlineD} stroke={goldDeepA(0.45)} strokeWidth={1.1} fill="none" />
          </g>
        );
      })}
    </svg>
  );
}

/// A phrase with one wobbly gold stroke drawn under it, like a screen title.
export function Underlined({
  children,
  seed = 121,
  strokeWidth = 2.6,
}: {
  children: React.ReactNode;
  seed?: number;
  strokeWidth?: number;
}) {
  const d = handPathD(lineSamples({ x: 1, y: 4 }, { x: 299, y: 4 }, 16), false, 0.9, seed);
  return (
    <span className="relative inline-block">
      {children}
      <svg
        viewBox="0 0 300 8"
        preserveAspectRatio="none"
        className="absolute left-0 -bottom-[0.14em] h-2 w-full -rotate-[0.5deg]"
        aria-hidden="true"
      >
        <path d={d} stroke={tea.gold} strokeWidth={strokeWidth} {...strokeProps} />
      </svg>
    </span>
  );
}

/// A short gold dash — the slip's bullet.
export function Dash({ seed, className = "" }: { seed: number; className?: string }) {
  const d = handPathD(lineSamples({ x: 1, y: 1 }, { x: 7, y: 1 }, 3), false, 0.5, seed);
  return (
    <svg width={8} height={3} viewBox="0 0 8 2" className={className} aria-hidden="true">
      <path d={d} stroke={tea.goldDeep} strokeWidth={2} {...strokeProps} />
    </svg>
  );
}

/// A ruled line drawn by hand — the divider.
export function Rule({ className = "" }: { className?: string }) {
  const d = handPathD(lineSamples({ x: 1, y: 1 }, { x: 599, y: 1 }, 16), false, 0.7, 9);
  return (
    <svg viewBox="0 0 600 2" preserveAspectRatio="none" className={`block h-0.5 w-full ${className}`} aria-hidden="true">
      <path d={d} stroke={inkA(0.12)} strokeWidth={1.1} {...strokeProps} />
    </svg>
  );
}

/// The startup disk as the shop's mug of tea: the tea level is the room left
/// on the drive. Static pour, per StorageMug in the app.
export function StorageMugSvg({
  fraction,
  size = 44,
  uid,
  className = "",
}: {
  fraction: number;
  size?: number;
  uid: string;
  className?: string;
}) {
  const level = Math.max(0, Math.min(1, fraction));
  const surfaceY = 47 - (47 - 26) * level;
  const mugD = handPathD(
    [
      { x: 12, y: 24 }, { x: 14, y: 42 }, { x: 20, y: 50 }, { x: 36, y: 50 },
      { x: 42, y: 42 }, { x: 44, y: 24 }, { x: 28, y: 22 }, { x: 12, y: 24 },
    ],
    true,
    0.8,
    71
  );
  const handleD = handPathD(arcSamples({ x: 45, y: 33 }, 8, -1.1, 1.1, 7), false, 0.6, 73);
  const saucerD = handPathD(lineSamples({ x: 8, y: 53 }, { x: 48, y: 53 }, 12), false, 0.7, 77);
  const swirlD = handPathD(
    arcSamples({ x: 28, y: surfaceY + 9 }, 6, Math.PI * 0.15, Math.PI * 1.1, 6),
    false,
    0.7,
    854
  );
  const surfaceD = handPathD(lineSamples({ x: 12, y: surfaceY }, { x: 44, y: surfaceY }, 8), false, 0.9, 851);
  const clipId = `${uid}mug`;
  return (
    <svg width={size} height={size} viewBox="0 0 56 56" className={className} aria-hidden="true">
      <clipPath id={clipId}>
        <path d={mugD} />
      </clipPath>
      <path d={mugD} fill={tea.card} />
      {level > 0.02 && (
        <g clipPath={`url(#${clipId})`}>
          <rect x={10} y={surfaceY} width={36} height={52 - surfaceY} fill="rgba(242, 182, 60, 0.42)" />
          <path d={swirlD} stroke={goldDeepA(0.35)} strokeWidth={1.4} {...strokeProps} />
          <path d={surfaceD} stroke={goldDeepA(0.8)} strokeWidth={1.6} {...strokeProps} />
        </g>
      )}
      <path d={mugD} stroke={inkA(0.8)} strokeWidth={1.8} {...strokeProps} />
      <path d={handleD} stroke={inkA(0.8)} strokeWidth={1.8} {...strokeProps} />
      <path d={saucerD} stroke={inkA(0.8)} strokeWidth={1.8} {...strokeProps} />
      {[0, 1, 2].map((i) => {
        const x = 19 + i * 9;
        const steamD = handPathD(
          [{ x, y: 17 }, { x: x + 4, y: 11 }, { x: x - 2, y: 6 }, { x: x + 3, y: 1 }],
          false,
          0.6,
          872 + i
        );
        return <path key={i} d={steamD} stroke={inkA(0.4)} strokeWidth={1.5} {...strokeProps} />;
      })}
    </svg>
  );
}

/// The steaming mug — the app's home empty-state doodle, one static phase.
export function MugDoodle({ size = 52, className = "" }: { size?: number; className?: string }) {
  const bodyD = handPathD(
    [
      { x: 12, y: 24 }, { x: 14, y: 42 }, { x: 20, y: 50 }, { x: 36, y: 50 },
      { x: 42, y: 42 }, { x: 44, y: 24 }, { x: 28, y: 22 }, { x: 12, y: 24 },
    ],
    true,
    0.8,
    71
  );
  const handleD = handPathD(arcSamples({ x: 45, y: 33 }, 8, -1.1, 1.1, 7), false, 0.6, 73);
  const teaD = handPathD(lineSamples({ x: 15, y: 29 }, { x: 41, y: 28 }, 9), false, 0.7, 75);
  const saucerD = handPathD(lineSamples({ x: 8, y: 53 }, { x: 48, y: 53 }, 12), false, 0.7, 77);
  return (
    <svg width={size} height={size} viewBox="0 0 56 56" className={className} aria-hidden="true">
      <path d={bodyD} fill={tea.card} />
      <path d={bodyD} stroke={inkA(0.8)} strokeWidth={1.8} {...strokeProps} />
      <path d={handleD} stroke={inkA(0.8)} strokeWidth={1.8} {...strokeProps} />
      <path d={teaD} stroke={goldDeepA(0.75)} strokeWidth={1.6} {...strokeProps} />
      <path d={saucerD} stroke={inkA(0.8)} strokeWidth={1.8} {...strokeProps} />
      {[0, 1, 2].map((i) => {
        const x = 19 + i * 9;
        const steamD = handPathD(
          [{ x, y: 17 }, { x: x + 4, y: 11 }, { x: x - 2, y: 6 }, { x: x + 3, y: 1 }],
          false,
          0.5,
          79 + i
        );
        return <path key={i} d={steamD} stroke={inkA(0.4)} strokeWidth={1.5} {...strokeProps} />;
      })}
    </svg>
  );
}
