"use client";

// Every button and card is a drawn box, per InkButtonStyle in the app:
// wobbly outline, gold scribble-hatch for the primary action, and pressing
// re-seeds the jitter so the box looks freshly redrawn.

import { useLayoutEffect, useMemo, useRef, useState, useId } from "react";
import { tea, inkA, goldDeepA, inkSoftA, inkNoise, handPathD, roundedRectSamples } from "@/lib/ink";

type Variant = "primary" | "quiet" | "destructive" | "card" | "well";

interface Props {
  /** "div" renders a plain drawn card — no pointer chrome, valid around nested buttons. */
  as?: "div";
  variant?: Variant;
  seed?: number;
  radius?: number;
  href?: string;
  onClick?: () => void;
  disabled?: boolean;
  className?: string;
  children: React.ReactNode;
  ariaLabel?: string;
}

function hatchD(w: number, h: number, seed: number): string {
  let d = "";
  let x = -h;
  let index = 0;
  while (x < w) {
    const wobble = inkNoise(index, seed) * 0.9;
    d += `M${(x + wobble).toFixed(1)} ${h}Q${(x + h / 2 + wobble * 2).toFixed(1)} ${(h / 2).toFixed(1)} ${(x + h - wobble).toFixed(1)} 0`;
    x += 5;
    index += 1;
  }
  return d;
}

export function InkBox({
  as,
  variant = "quiet",
  seed = 31,
  radius = 9,
  href,
  onClick,
  disabled,
  className = "",
  children,
  ariaLabel,
}: Props) {
  const ref = useRef<HTMLElement | null>(null);
  const [size, setSize] = useState({ w: 0, h: 0 });
  const [hover, setHover] = useState(false);
  const [press, setPress] = useState(false);
  const clipId = useId();

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => setSize({ w: Math.round(el.offsetWidth), h: Math.round(el.offsetHeight) });
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  const activeSeed = seed + (press ? 4 : 0) + (hover ? 2 : 0);
  const drawn = size.w > 4 && size.h > 4;
  const art = useMemo(() => {
    if (!drawn) return null;
    const step = variant === "card" ? 11 : variant === "well" ? 12 : 9;
    const amplitude = variant === "card" ? 1.0 : variant === "well" ? 0.7 : 0.9;
    const d = handPathD(
      roundedRectSamples(0.8, 0.8, size.w - 1.6, size.h - 1.6, radius, step),
      true,
      amplitude,
      activeSeed
    );
    return { d, hatch: variant === "primary" ? hatchD(size.w, size.h, activeSeed) : null };
  }, [drawn, size.w, size.h, radius, activeSeed, variant]);

  const line = disabled
    ? inkSoftA(0.35)
    : variant === "destructive"
      ? tea.rust
      : variant === "well"
        ? inkA(0.28)
        : variant === "quiet" && !hover
          ? inkA(0.8)
          : tea.ink;
  const lineWidth = variant === "well" ? 1.1 : 1.4;
  const fill =
    variant === "primary"
      ? disabled
        ? "rgba(242, 182, 60, 0.25)"
        : tea.gold
      : variant === "card"
        ? tea.card
        : variant === "well"
          ? "rgba(241, 234, 219, 0.7)"
          : variant === "destructive"
            ? hover && !disabled
              ? "rgba(180, 85, 61, 0.10)"
              : "rgba(255, 253, 246, 0.9)"
            : hover && !disabled
              ? tea.paperDeep
              : "rgba(255, 253, 246, 0.9)";

  const svg = art ? (
    <svg className="pointer-events-none absolute inset-0" width={size.w} height={size.h} aria-hidden="true">
      <path d={art.d} fill={fill} />
      {art.hatch && (
        <>
          <clipPath id={clipId}>
            <path d={art.d} />
          </clipPath>
          <path
            d={art.hatch}
            stroke={goldDeepA(disabled ? 0.1 : 0.3)}
            strokeWidth={1}
            fill="none"
            clipPath={`url(#${clipId})`}
          />
        </>
      )}
      <path d={art.d} stroke={line} strokeWidth={lineWidth} fill="none" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  ) : null;

  const shared = {
    className: [
      "relative inline-flex cursor-pointer items-center justify-center border-none bg-transparent p-0 font-semibold no-underline transition-transform duration-100 ease-out",
      variant === "destructive" ? "text-rust" : "text-ink",
      "active:scale-[0.98] disabled:cursor-default disabled:text-ink-soft/50",
      drawn ? "" : "rounded-[9px] border-[1.4px] border-solid border-ink/80",
      className,
    ].join(" "),
    onPointerEnter: () => setHover(true),
    onPointerLeave: () => {
      setHover(false);
      setPress(false);
    },
    onPointerDown: () => setPress(true),
    onPointerUp: () => setPress(false),
    onPointerCancel: () => setPress(false),
    "aria-label": ariaLabel,
  };

  if (as === "div") {
    return (
      <div
        ref={ref as React.Ref<HTMLDivElement>}
        className={[
          "relative block text-ink",
          drawn ? "" : "rounded-[9px] border-[1.4px] border-solid border-ink/80",
          className,
        ].join(" ")}
        aria-label={ariaLabel}
      >
        {svg}
        <span className="relative block">{children}</span>
      </div>
    );
  }

  if (href) {
    return (
      <a ref={ref as React.Ref<HTMLAnchorElement>} href={href} {...shared}>
        {svg}
        <span className="relative">{children}</span>
      </a>
    );
  }
  return (
    <button
      ref={ref as React.Ref<HTMLButtonElement>}
      type="button"
      onClick={onClick}
      disabled={disabled}
      {...shared}
    >
      {svg}
      <span className="relative">{children}</span>
    </button>
  );
}
