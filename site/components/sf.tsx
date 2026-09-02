// Crisp SF-symbol stand-ins. The app renders real SF Symbols for these; on
// the web they are redrawn as plain rounded strokes, deliberately not wobbled —
// in the app only drawn chrome boils, glyphs stay crisp.

const strokeProps = {
  fill: "none",
  stroke: "currentColor",
  strokeLinecap: "round",
  strokeLinejoin: "round",
} as const;

interface GlyphProps {
  size?: number;
  className?: string;
}

/// info.circle / info.circle.fill
export function InfoCircle({ size = 12, filled = false, className }: GlyphProps & { filled?: boolean }) {
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" className={className} aria-hidden="true">
      <circle cx={8} cy={8} r={6.4} {...strokeProps} strokeWidth={1.3} fill={filled ? "currentColor" : "none"} />
      <g stroke={filled ? "#FAF5EA" : "currentColor"}>
        <circle cx={8} cy={4.9} r={0.4} strokeWidth={1.5} fill={filled ? "#FAF5EA" : "currentColor"} />
        <path d="M8 7.4v3.8" fill="none" strokeWidth={1.5} strokeLinecap="round" />
      </g>
    </svg>
  );
}

export function ChevronLeft({ size = 10, className }: GlyphProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" className={className} aria-hidden="true">
      <path d="M10.2 2.8 L5.2 8 L10.2 13.2" {...strokeProps} strokeWidth={2} />
    </svg>
  );
}

export function XMark({ size = 10, className }: GlyphProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" className={className} aria-hidden="true">
      <path d="M4 4 L12 12 M12 4 L4 12" {...strokeProps} strokeWidth={1.7} />
    </svg>
  );
}

export function ExclamationCircle({ size = 12, className }: GlyphProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" className={className} aria-hidden="true">
      <circle cx={8} cy={8} r={6.4} {...strokeProps} strokeWidth={1.3} />
      <path d="M8 4.6v4" {...strokeProps} strokeWidth={1.5} />
      <circle cx={8} cy={11.2} r={0.4} stroke="currentColor" strokeWidth={1.5} fill="currentColor" />
    </svg>
  );
}

export function TrashGlyph({ size = 12, className }: GlyphProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" className={className} aria-hidden="true">
      <g {...strokeProps} strokeWidth={1.3}>
        <path d="M2.8 4.4h10.4" />
        <path d="M6 4.2v-1a0.9 0.9 0 0 1 0.9 -0.9h2.2a0.9 0.9 0 0 1 0.9 0.9v1" />
        <path d="M4.2 4.6l0.7 8.2a1 1 0 0 0 1 0.9h4.2a1 1 0 0 0 1 -0.9l0.7-8.2" />
        <path d="M6.6 7v4.3 M9.4 7v4.3" />
      </g>
    </svg>
  );
}

/// arrow.turn.down.right — the consequence bullet.
export function ArrowTurnDownRight({ size = 11, className }: GlyphProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" className={className} aria-hidden="true">
      <g {...strokeProps} strokeWidth={1.5}>
        <path d="M3.5 2.8v5.4a2.2 2.2 0 0 0 2.2 2.2h6.6" />
        <path d="M9.4 7.4 L12.5 10.4 L9.4 13.4" />
      </g>
    </svg>
  );
}

export function Magnifier({ size = 15, className }: GlyphProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" className={className} aria-hidden="true">
      <g {...strokeProps} strokeWidth={1.5}>
        <circle cx={6.8} cy={6.8} r={4.6} />
        <path d="M10.3 10.3 L14 14" />
      </g>
    </svg>
  );
}

/// clock.arrow.circlepath — the Activity tab.
export function ClockArrow({ size = 15, className }: GlyphProps) {
  // The ring sweeps clockwise from the gap at the upper left; the arrowhead
  // points back into the gap, the way the SF symbol reads "history".
  const r = 5.8;
  const cx = 8;
  const cy = 8.2;
  const a0 = -Math.PI * 0.78;
  const a1 = Math.PI * 1.06;
  const sx = cx + Math.cos(a0) * r;
  const sy = cy + Math.sin(a0) * r;
  const ex = cx + Math.cos(a1) * r;
  const ey = cy + Math.sin(a1) * r;
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" className={className} aria-hidden="true">
      <g {...strokeProps} strokeWidth={1.4}>
        <path d={`M${sx.toFixed(2)} ${sy.toFixed(2)} A${r} ${r} 0 1 1 ${ex.toFixed(2)} ${ey.toFixed(2)}`} />
        <path d={`M${(sx - 2.6).toFixed(2)} ${(sy - 0.6).toFixed(2)} L${sx.toFixed(2)} ${sy.toFixed(2)} L${(sx - 0.4).toFixed(2)} ${(sy + 2.7).toFixed(2)}`} />
        <path d={`M${cx} ${cy - 3.1} V${cy} L${cx + 2.4} ${cy + 1.4}`} strokeWidth={1.5} />
      </g>
    </svg>
  );
}

/// gearshape — the Settings tab.
export function Gear({ size = 15, className }: GlyphProps) {
  const teeth = 7;
  const outer = 6.6;
  const inner = 5;
  let d = "";
  for (let i = 0; i < teeth; i++) {
    const center = (i / teeth) * Math.PI * 2 - Math.PI / 2;
    const half = Math.PI / teeth;
    const toothHalf = half * 0.44;
    const gapHalf = half * 0.52;
    const at = (angle: number, radius: number) =>
      `${(8 + Math.cos(angle) * radius).toFixed(2)} ${(8 + Math.sin(angle) * radius).toFixed(2)}`;
    d += `${i === 0 ? "M" : "L"}${at(center - toothHalf, outer)}`;
    d += `L${at(center + toothHalf, outer)}`;
    d += `L${at(center + gapHalf, inner)}`;
    d += `L${at(center + half * 2 - gapHalf, inner)}`;
  }
  d += "Z";
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" className={className} aria-hidden="true">
      <path d={d} {...strokeProps} strokeWidth={1.3} />
      <circle cx={8} cy={8} r={2} {...strokeProps} strokeWidth={1.3} />
    </svg>
  );
}

/// hammer — Cargo build artifacts.
export function Hammer({ size = 12, className }: GlyphProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" className={className} aria-hidden="true">
      <g {...strokeProps} strokeWidth={1.3}>
        <path d="M2.6 6.2 L6.4 2.4 a4.4 4.4 0 0 1 5 0.7 l1.6 1.6 -1.4 1.4 -1.1-0.4 -1.7 1.7 z" />
        <path d="M7.6 6.6 L13 12 a0.9 0.9 0 0 1 -1.3 1.3 L6.3 8" strokeWidth={1.4} />
      </g>
    </svg>
  );
}

/// shippingbox — installed dependencies.
export function ShippingBox({ size = 12, className }: GlyphProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" className={className} aria-hidden="true">
      <g {...strokeProps} strokeWidth={1.3}>
        <path d="M8 1.9 L14 4.7 V11.3 L8 14.1 L2 11.3 V4.7 Z" />
        <path d="M2 4.7 L8 7.5 L14 4.7 M8 7.5 V14.1" />
        <path d="M5 3.3 L11 6.1" />
      </g>
    </svg>
  );
}

/// arrow.down.doc — a download to review.
export function ArrowDownDoc({ size = 12, className }: GlyphProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" className={className} aria-hidden="true">
      <g {...strokeProps} strokeWidth={1.3}>
        <path d="M4 1.9 h4.8 L12 5.1 V13.2 a0.9 0.9 0 0 1 -0.9 0.9 H4.9 a0.9 0.9 0 0 1 -0.9 -0.9 Z" />
        <path d="M8.8 1.9 V5.1 H12" />
        <path d="M8 6.9 v4.4 M6 9.5 L8 11.5 L10 9.5" strokeWidth={1.4} />
      </g>
    </svg>
  );
}
