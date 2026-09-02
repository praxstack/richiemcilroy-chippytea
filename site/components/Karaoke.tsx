"use client";

// The chippytea song, sung along to. A taped slip on the page invites you to
// play it; pressing it opens the karaoke: the lyric one line at a time with
// every word filling gold as it is sung and a chip bouncing over the words,
// fish and chips carrying on behind, a hand-lettered chip count for every word
// you get through, and a chip thrown into the wrap wherever you tap. Every
// section of the song announces itself: a stamp slams onto the page, the paper
// changes colour, the dot grid moves, and the words dance differently (bouncing
// on the beat in the choruses, trembling through the pre-chorus, swaying on the
// bridge). Closing fades the whole thing out, song included; opening it again
// starts afresh.

import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { tea, inkA, goldDeepA, handPathD, circleSamples, lineSamples, roundedRectSamples } from "@/lib/ink";
import { paintBalance } from "@/lib/draw";
import { getAudioContext, playChime } from "@/lib/chime";
import { lyricLines } from "@/lib/lyrics";
import { KaraokeScene } from "@/lib/karaokeScene";
import { runOfLine, sectionRuns, progressThrough } from "@/lib/sections";
import { Tape } from "./art";
import { InkBox } from "./InkBox";

const SONG_SRC = "/save-your-mac-with-chippytea.mp3";
const SCORE_W = 132;
const SCORE_H = 40;
const strokeProps = { fill: "none", strokeLinecap: "round", strokeLinejoin: "round" } as const;

/** Seconds between beats when there is no analyser to hear them. */
const FALLBACK_BEAT = 0.5;
const WASHES = ["chorus", "last-chorus", "bridge", "verse", "pre-chorus"] as const;

// MARK: - The lyric's clock

const lineStartAt = (index: number) => lyricLines[index].words[0].start;
const lineEndAt = (index: number) => {
  const words = lyricLines[index].words;
  return words[words.length - 1].end;
};
const allWordStarts = lyricLines.flatMap((line) => line.words.map((word) => word.start));

/// The line to show at a moment: the one being sung, or, in a gap, the next.
function lineAt(time: number): number {
  let index = 0;
  while (index < lyricLines.length - 1 && lineStartAt(index + 1) <= time) index += 1;
  if (time > lineEndAt(index) + 0.6 && index + 1 < lyricLines.length) return index + 1;
  return index;
}

/// How many words have started by a moment.
function wordsSungAt(time: number): number {
  let lo = 0;
  let hi = allWordStarts.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (allWordStarts[mid] <= time) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

const clamp01 = (v: number) => Math.max(0, Math.min(1, v));
const clock = (seconds: number) => {
  const whole = Math.max(0, Math.floor(seconds));
  return `${Math.floor(whole / 60)}:${String(whole % 60).padStart(2, "0")}`;
};

// MARK: - Drawn bits

/// The play button: a drawn ring, gold when it matters, with the glyph inside.
function PlayRing({
  size,
  playing,
  seed,
  phases = 1,
}: {
  size: number;
  playing: boolean;
  seed: number;
  phases?: number;
}) {
  const art = useMemo(() => {
    const c = 17;
    return Array.from({ length: phases }, (_, phase) => ({
      ring: handPathD(circleSamples({ x: c, y: c }, 15.2, 16, -0.5), true, 1, seed + phase * 3),
      play: handPathD([{ x: 13.5, y: 11 }, { x: 24.5, y: 17 }, { x: 13.5, y: 23 }, { x: 13.5, y: 11 }], true, 0.6, seed + 20 + phase),
      pause: [
        handPathD([{ x: 13.5, y: 11.5 }, { x: 13.8, y: 17 }, { x: 13.5, y: 22.5 }], false, 0.5, seed + 30 + phase),
        handPathD([{ x: 20.5, y: 11.5 }, { x: 20.2, y: 17 }, { x: 20.5, y: 22.5 }], false, 0.5, seed + 40 + phase),
      ],
    }));
  }, [seed, phases]);
  return (
    <svg width={size} height={size} viewBox="0 0 34 34" aria-hidden="true">
      {art.map((phase, i) => (
        <g key={i} className={phases > 1 ? `bp bp${i}` : undefined}>
          <path d={phase.ring} fill={tea.gold} stroke={tea.ink} strokeWidth={1.5} strokeLinecap="round" strokeLinejoin="round" />
          {playing ? (
            phase.pause.map((d, j) => <path key={j} d={d} stroke={tea.ink} strokeWidth={2.6} {...strokeProps} />)
          ) : (
            <path d={phase.play} fill={tea.ink} stroke={tea.ink} strokeWidth={1.4} strokeLinejoin="round" />
          )}
        </g>
      ))}
    </svg>
  );
}

/// A chip lying flat, as SVG: the bouncing ball and the slider's thumb.
function ChipGlyph({ length = 26, seed = 7 }: { length?: number; seed?: number }) {
  const art = useMemo(() => {
    const thickness = length * 0.3;
    return {
      body: handPathD(roundedRectSamples(0, -thickness / 2, length, thickness, thickness * 0.42, 4), true, length * 0.03, seed),
      thickness,
    };
  }, [length, seed]);
  return (
    <g>
      <path d={art.body} fill={tea.gold} />
      <path d={art.body} stroke={tea.ink} strokeWidth={Math.max(1, length * 0.045)} {...strokeProps} />
      <path
        d={`M${length * 0.18} ${-art.thickness * 0.1}Q${length / 2} ${-art.thickness * 0.4} ${length * 0.75} ${-art.thickness * 0.15}`}
        stroke={goldDeepA(0.5)}
        strokeWidth={1}
        {...strokeProps}
      />
    </g>
  );
}

/// Three chips standing on end, the slip's little equaliser. They dance on hover.
function EqChips() {
  const chips = useMemo(
    () =>
      [0.72, 1, 0.58].map((height, i) => {
        const length = 18 * height;
        const thickness = 5.4;
        return {
          d: handPathD(roundedRectSamples(-thickness / 2, -length, thickness, length, thickness * 0.42, 4), true, 0.5, 91 + i * 7),
          x: 6 + i * 9,
        };
      }),
    []
  );
  return (
    <svg width={30} height={22} viewBox="0 0 30 22" className="shrink-0" aria-hidden="true">
      {chips.map((chip, i) => (
        <g key={i} transform={`translate(${chip.x} 21) rotate(${i % 2 ? 6 : -5})`}>
          <g className="eq-chip" style={{ transformBox: "fill-box", transformOrigin: "50% 100%", animationDelay: `${i * 0.12}s` }}>
            <path d={chip.d} fill={tea.gold} />
            <path d={chip.d} stroke={tea.ink} strokeWidth={1} {...strokeProps} />
          </g>
        </g>
      ))}
    </svg>
  );
}

// MARK: - The slip on the page

export function Karaoke() {
  const [open, setOpen] = useState(false);
  const buttonRef = useRef<HTMLSpanElement>(null);

  const close = useCallback(() => {
    setOpen(false);
  }, []);

  return (
    <>
      <span ref={buttonRef} className="relative inline-block -rotate-1">
        <Tape uid="song" className="absolute -top-3 left-9 z-[2] -rotate-6 scale-90" />
        <InkBox
          variant="quiet"
          seed={57}
          radius={11}
          onClick={() => setOpen(true)}
          ariaLabel="Play the chippytea song, karaoke style"
          className="song-slip boil-hover py-3 pl-3.5 pr-4 text-left drop-shadow-[0_1.5px_2px_rgba(51,48,43,0.12)]"
        >
          <span className="flex items-center gap-3">
            <PlayRing size={42} playing={false} seed={45} phases={3} />
            <span className="flex flex-col gap-[3px]">
              <span className="text-[14px] font-semibold leading-tight text-ink">Play the chippytea song</span>
              <span className="text-[11.5px] font-normal leading-tight text-ink-soft">Sing along. Two minutes. Chips everywhere.</span>
            </span>
            <EqChips />
          </span>
        </InkBox>
      </span>
      {open ? <KaraokeOverlay onClose={close} triggerRef={buttonRef} /> : null}
    </>
  );
}

// MARK: - The karaoke

function KaraokeOverlay({
  onClose,
  triggerRef,
}: {
  onClose: () => void;
  triggerRef: React.RefObject<HTMLSpanElement | null>;
}) {
  const rootRef = useRef<HTMLDivElement>(null);
  const audioRef = useRef<HTMLAudioElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const scoreRef = useRef<HTMLCanvasElement>(null);
  const lineRef = useRef<HTMLDivElement>(null);
  const ballRef = useRef<SVGSVGElement>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  const timeRef = useRef<HTMLSpanElement>(null);
  const barRef = useRef<HTMLDivElement>(null);
  const barFillRef = useRef<SVGPathElement>(null);
  const barThumbRef = useRef<SVGGElement>(null);
  const seekRef = useRef<HTMLInputElement>(null);
  const wordRefs = useRef<(HTMLButtonElement | null)[]>([]);
  const goldRefs = useRef<(HTMLSpanElement | null)[]>([]);
  const renderedLine = useRef(-1);
  const sceneRef = useRef<KaraokeScene | null>(null);
  const analyserRef = useRef<AnalyserNode | null>(null);
  const gainRef = useRef<GainNode | null>(null);
  const sourceRef = useRef<MediaElementAudioSourceNode | null>(null);
  const landedRef = useRef(0);
  const bonusRef = useRef<Set<number>>(new Set());
  const scoreDrawn = useRef({ value: -1, at: 0 });
  const chimeAt = useRef(0);
  const leavingRef = useRef(false);
  const closeTimeoutRef = useRef(0);
  const fadeRafRef = useRef(0);

  const [lineIdx, setLineIdx] = useState(() => lineAt(0));
  const [playing, setPlaying] = useState(false);
  const [ended, setEnded] = useState(false);
  const [thrown, setThrown] = useState(0);
  const [leaving, setLeaving] = useState(false);
  const [duration, setDuration] = useState(0);
  const [barW, setBarW] = useState(0);
  const [reduced] = useState(
    () => typeof window !== "undefined" && window.matchMedia("(prefers-reduced-motion: reduce)").matches
  );

  const line = lyricLines[lineIdx];
  const prevLine = lyricLines[lineIdx - 1];
  const nextLine = lyricLines[lineIdx + 1];
  const runIdx = runOfLine[lineIdx] ?? 0;
  const run = sectionRuns[runIdx];

  // Drawn chrome for the controls.
  const closeArt = useMemo(
    () => ({
      ring: handPathD(circleSamples({ x: 18, y: 18 }, 16, 16, -0.5), true, 1, 77),
      cross: [
        handPathD([{ x: 12, y: 12 }, { x: 18, y: 18.3 }, { x: 24, y: 24 }], false, 0.5, 79),
        handPathD([{ x: 24, y: 12 }, { x: 18.2, y: 18 }, { x: 12, y: 24 }], false, 0.5, 81),
      ],
    }),
    []
  );
  const barTrack = useMemo(
    () => (barW > 2 ? handPathD(lineSamples({ x: 1, y: 0 }, { x: barW - 1, y: 0 }, 14), false, 0.5, 113) : ""),
    [barW]
  );

  // The scene lives as long as the karaoke does.
  if (!sceneRef.current) sceneRef.current = new KaraokeScene();

  // Lock the page behind, focus the close, and put the keys to work.
  useEffect(() => {
    const root = rootRef.current;
    if (!root) return;
    const previousOverflow = document.documentElement.style.overflow;
    document.documentElement.style.overflow = "hidden";
    const keepFocusInside = (event: FocusEvent) => {
      if (!root.contains(event.target as Node)) {
        (closeRef.current ?? root).focus({ preventScroll: true });
      }
    };
    document.addEventListener("focusin", keepFocusInside);
    (closeRef.current ?? root).focus({ preventScroll: true });
    return () => {
      window.clearTimeout(closeTimeoutRef.current);
      cancelAnimationFrame(fadeRafRef.current);
      document.removeEventListener("focusin", keepFocusInside);
      document.documentElement.style.overflow = previousOverflow;
      triggerRef.current?.querySelector("button")?.focus({ preventScroll: true });
    };
  }, [triggerRef]);

  const beginClose = useCallback(() => {
    if (leavingRef.current) return;
    leavingRef.current = true;
    setLeaving(true);
    const audio = audioRef.current;
    const gain = gainRef.current;
    const ctx = gain?.context;
    if (gain && ctx) {
      gain.gain.cancelScheduledValues(ctx.currentTime);
      gain.gain.setValueAtTime(gain.gain.value, ctx.currentTime);
      gain.gain.linearRampToValueAtTime(0, ctx.currentTime + 0.45);
    } else if (audio) {
      const from = audio.volume;
      const started = performance.now();
      const step = (now: number) => {
        const p = clamp01((now - started) / 450);
        audio.volume = from * (1 - p);
        if (p < 1) fadeRafRef.current = requestAnimationFrame(step);
      };
      fadeRafRef.current = requestAnimationFrame(step);
    }
    // The fade-out animation ends the overlay; this is the belt to its braces.
    closeTimeoutRef.current = window.setTimeout(onClose, 700);
  }, [onClose]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      const onControl = !!target?.closest("button, input, a");
      if (event.key === "Tab") {
        const root = rootRef.current;
        if (!root) return;
        const controls = Array.from(root.querySelectorAll<HTMLElement>("button, input, a[href], [tabindex]"))
          .filter((element) => element.tabIndex >= 0 && !element.matches(":disabled") && !element.closest("[inert]") && element.getClientRects().length > 0);
        const index = controls.indexOf(document.activeElement as HTMLElement);
        if (event.shiftKey ? index <= 0 : index < 0 || index === controls.length - 1) {
          event.preventDefault();
          const next = event.shiftKey ? controls[controls.length - 1] : controls[0];
          (next ?? root).focus({ preventScroll: true });
        }
      } else if (event.key === "Escape") {
        event.preventDefault();
        beginClose();
      } else if (event.key === " " && !onControl) {
        event.preventDefault();
        togglePlay();
      } else if (event.key === "ArrowRight" && !onControl) {
        seekBy(5);
      } else if (event.key === "ArrowLeft" && !onControl) {
        seekBy(-5);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [beginClose]);

  // Wire the song through a gain (for the fade) and an analyser (for the beat), then play.
  useEffect(() => {
    const audio = audioRef.current;
    if (!audio) return;
    try {
      const ctx = getAudioContext();
      sourceRef.current ??= ctx.createMediaElementSource(audio);
      const gain = ctx.createGain();
      const analyser = ctx.createAnalyser();
      // Fine enough that the first few bins are the kick drum, not the vocal.
      analyser.fftSize = 512;
      analyser.smoothingTimeConstant = 0.3;
      sourceRef.current.connect(gain);
      gain.connect(analyser);
      analyser.connect(ctx.destination);
      gainRef.current = gain;
      analyserRef.current = analyser;
    } catch {
      // Without WebAudio the song still plays; the fish keep their own time.
    }
    audio.currentTime = 0;
    void audio.play().catch(() => {});
    return () => {
      audio.pause();
      try {
        sourceRef.current?.disconnect();
        gainRef.current?.disconnect();
        analyserRef.current?.disconnect();
      } catch {
        // Already gone.
      }
      gainRef.current = null;
      analyserRef.current = null;
    };
  }, []);

  // Size the backdrop canvas to the screen.
  useLayoutEffect(() => {
    const canvas = canvasRef.current;
    const scene = sceneRef.current;
    if (!canvas || !scene) return;
    const fit = () => {
      const dpr = Math.min(2, window.devicePixelRatio || 1);
      const w = window.innerWidth;
      const h = window.innerHeight;
      canvas.width = Math.round(w * dpr);
      canvas.height = Math.round(h * dpr);
      canvas.style.width = `${w}px`;
      canvas.style.height = `${h}px`;
      canvas.getContext("2d")?.setTransform(dpr, 0, 0, dpr, 0, 0);
      scene.resize(w, h);
      if (barRef.current) setBarW(Math.round(barRef.current.offsetWidth));
    };
    fit();
    window.addEventListener("resize", fit);
    return () => window.removeEventListener("resize", fit);
  }, []);

  useLayoutEffect(() => {
    renderedLine.current = lineIdx;
    // A focused word disappears when the next lyric replaces it.
    if (rootRef.current && (ended || !rootRef.current.contains(document.activeElement))) {
      closeRef.current?.focus({ preventScroll: true });
    }
  }, [lineIdx, ended]);

  // A thrown chip landing in the wrap counts, with the chime.
  useEffect(() => {
    const scene = sceneRef.current;
    if (!scene) return;
    scene.onLand = () => {
      landedRef.current += 1;
      const now = performance.now();
      if (now - chimeAt.current > 140) {
        chimeAt.current = now;
        playChime();
      }
    };
    return () => {
      scene.onLand = null;
    };
  }, []);

  // The frame loop: word fills, the bouncing chip, the score, the scene.
  useEffect(() => {
    const scene = sceneRef.current;
    const canvas = canvasRef.current;
    const ctx = canvas?.getContext("2d");
    if (!scene || !ctx) return;
    const bins = analyserRef.current ? new Float32Array(analyserRef.current.frequencyBinCount) : null;
    let raf = 0;
    let last = performance.now();
    let lastDraw = 0;
    let level = 0;
    let previousBass = -100;
    let typicalBass = -100;
    let lastBeatAt = -1;
    let lastBeatSlot = -1;
    let kick = 0;
    let sway = 1;
    let lastRun = -1;
    const frame = (now: number) => {
      raf = requestAnimationFrame(frame);
      const audio = audioRef.current;
      if (!audio) return;
      if (now - lastDraw < 32) return;
      const dt = (now - last) / 1000;
      last = now;
      lastDraw = now;
      const time = audio.currentTime;
      const isPlaying = !audio.paused && !audio.ended;

      // The beat: the bass, in decibels, jumping a few dB above its recent
      // run. (Byte data clips at -30 dB, which this song sits above.) Without
      // WebAudio, a steady stand-in.
      const analyser = analyserRef.current;
      let target = isPlaying ? 0.45 : 0;
      let beat = false;
      if (analyser && bins && isPlaying) {
        analyser.getFloatFrequencyData(bins);
        let sum = 0;
        for (let i = 1; i <= 3; i++) sum += Math.max(-100, bins[i]);
        const bass = sum / 3;
        target = clamp01((bass + 62) / 34);
        if (bass > -60 && bass > typicalBass + 3 && bass - previousBass > 1 && now - lastBeatAt > 300) {
          beat = true;
          lastBeatAt = now;
        }
        typicalBass += (bass - typicalBass) * 0.06;
        previousBass = bass;
      } else if (isPlaying) {
        const slot = Math.floor(time / FALLBACK_BEAT);
        if (slot !== lastBeatSlot) {
          lastBeatSlot = slot;
          beat = true;
        }
      }
      level += (target - level) * 0.35;

      const li = lineAt(time);
      const runIndex = runOfLine[li] ?? 0;
      const run = sectionRuns[runIndex];
      if (runIndex !== lastRun) {
        lastRun = runIndex;
        sway = 1;
      }
      if (beat) {
        kick = 1;
        sway = -sway;
      } else {
        kick *= Math.exp(-dt * 8);
      }
      const root = rootRef.current;
      if (root) {
        root.style.setProperty("--kick", reduced ? "0" : kick.toFixed(3));
        root.style.setProperty("--sway", reduced ? "0" : String(sway));
        root.style.setProperty("--charge", run.key === "pre-chorus" ? progressThrough(run, time).toFixed(3) : "0");
      }

      if (li !== renderedLine.current) {
        setLineIdx(li);
      } else {
        paintWords(time, now);
      }

      const sung = wordsSungAt(time);
      for (const [index, lyric] of lyricLines.entries()) {
        if (!bonusRef.current.has(index) && time >= lineStartAt(index) && lyricText(lyric).includes("high score")) {
          bonusRef.current.add(index);
        }
      }
      const score = sung + landedRef.current + bonusRef.current.size * 10;
      paintScore(score, now);

      const scoreBox = scoreRef.current
        ? (() => {
            const rect = scoreRef.current.getBoundingClientRect();
            return { x: rect.left, y: rect.top, w: rect.width, h: rect.height };
          })()
        : null;
      scene.draw(ctx, {
        time,
        now,
        dt,
        level,
        beat,
        playing: isPlaying,
        line: li,
        wrapChips: Math.floor(sung / 8) + landedRef.current,
        scoreBox,
        reduced,
      });

      if (timeRef.current) timeRef.current.textContent = `${clock(time)} / ${clock(audio.duration || 0)}`;
      const total = audio.duration || 0;
      if (barFillRef.current && barW > 2 && total > 0) {
        const p = clamp01(time / total);
        const fillTo = 1 + Math.max(1, (barW - 2) * p);
        barFillRef.current.setAttribute("d", handPathD(lineSamples({ x: 1, y: 0 }, { x: fillTo, y: 0 }, 14), false, 0.7, 117));
        barThumbRef.current?.setAttribute("transform", `translate(${Math.max(0, Math.min(barW - 22, p * (barW - 22)))} 0) rotate(-8)`);
        if (seekRef.current && document.activeElement !== seekRef.current) seekRef.current.value = String(time);
      }
    };

    const lyricText = (lyric: (typeof lyricLines)[number]) => lyric.words.map((word) => word.text).join(" ").toLowerCase();

    const paintWords = (time: number, now: number) => {
      const words = lyricLines[renderedLine.current]?.words;
      const container = lineRef.current;
      if (!words || !container) return;
      let active = -1;
      words.forEach((word, index) => {
        const el = wordRefs.current[index];
        const gold = goldRefs.current[index];
        if (!el || !gold) return;
        const next = words[index + 1];
        const fillEnd = next ? Math.min(Math.max(word.end, word.start + 0.15), next.start) : Math.max(word.end, word.start + 0.15);
        const p = clamp01((time - word.start) / Math.max(0.05, fillEnd - word.start));
        const state = time < word.start ? "todo" : p >= 1 && (!next || time >= next.start) ? "done" : "active";
        if (state === "active") active = index;
        if (el.dataset.state !== state) el.dataset.state = state;
        gold.style.clipPath = `inset(-0.2em ${((1 - p) * 100).toFixed(1)}% -0.25em -0.1em)`;
      });
      // The bouncing chip: arcs from each word to the next as it starts.
      const ball = ballRef.current;
      if (!ball) return;
      if (reduced) {
        ball.style.opacity = "0";
        return;
      }
      let k = -1;
      for (let i = 0; i < words.length; i++) if (words[i].start <= time) k = i;
      const centre = (index: number) => {
        const el = wordRefs.current[index];
        return el ? { x: el.offsetLeft + el.offsetWidth / 2, y: el.offsetTop } : null;
      };
      let x: number;
      let y: number;
      let rot: number;
      if (k < 0) {
        const c = centre(0);
        if (!c) return;
        x = c.x;
        y = c.y - 7 - Math.abs(Math.sin(now / 260)) * 7;
        rot = -8;
      } else if (k >= words.length - 1 || !centre(k + 1)) {
        const c = centre(k);
        if (!c) return;
        x = c.x;
        y = c.y - 6 - Math.abs(Math.sin(now / 260)) * 4;
        rot = k * 180;
      } else {
        const a = centre(k)!;
        const b = centre(k + 1)!;
        const span = Math.max(0.08, words[k + 1].start - words[k].start);
        const p = clamp01((time - words[k].start) / span);
        const arc = Math.min(16, 6 + Math.hypot(b.x - a.x, b.y - a.y) * 0.1);
        x = a.x + (b.x - a.x) * p;
        y = a.y + (b.y - a.y) * p - 6 - Math.sin(p * Math.PI) * arc;
        rot = k * 180 + p * 180;
      }
      ball.style.opacity = active >= 0 || k >= 0 || time < words[0].start ? "1" : "0";
      ball.style.transform = `translate(${(x - 16).toFixed(1)}px, ${(y - 7).toFixed(1)}px) rotate(${rot.toFixed(0)}deg)`;
    };

    const paintScore = (value: number, now: number) => {
      const canvas = scoreRef.current;
      const sctx = canvas?.getContext("2d");
      if (!canvas || !sctx) return;
      const changed = value !== scoreDrawn.current.value;
      if (changed) scoreDrawn.current = { value, at: now };
      const boiling = now - scoreDrawn.current.at < 600;
      if (!changed && !boiling && canvas.dataset.settled === "1") return;
      canvas.dataset.settled = boiling ? "0" : "1";
      const dpr = Math.min(2, window.devicePixelRatio || 1);
      if (canvas.width !== SCORE_W * dpr) {
        canvas.width = SCORE_W * dpr;
        canvas.height = SCORE_H * dpr;
      }
      sctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      sctx.clearRect(0, 0, SCORE_W, SCORE_H);
      paintBalance(sctx, value, 30, boiling && !reduced ? Math.floor(now / 167) % 3 : 0);
    };

    raf = requestAnimationFrame(frame);
    return () => cancelAnimationFrame(raf);
  }, [barW, reduced]);

  const togglePlay = () => {
    const audio = audioRef.current;
    if (!audio) return;
    if (audio.paused) void audio.play().catch(() => {});
    else audio.pause();
  };

  const seekTo = (time: number) => {
    const audio = audioRef.current;
    if (!audio) return;
    audio.currentTime = Math.max(0, Math.min(audio.duration || time, time));
    setEnded(false);
  };
  const seekBy = (delta: number) => {
    const audio = audioRef.current;
    if (audio) seekTo(audio.currentTime + delta);
  };

  const again = () => {
    const audio = audioRef.current;
    const scene = sceneRef.current;
    if (!audio) return;
    landedRef.current = 0;
    bonusRef.current = new Set();
    scene?.reset();
    setThrown(0);
    setEnded(false);
    audio.currentTime = 0;
    void audio.play().catch(() => {});
  };

  const throwFrom = (event: React.PointerEvent<HTMLDivElement>) => {
    if (ended || leaving) return;
    const target = event.target as HTMLElement | null;
    if (target?.closest("[data-ui]")) return;
    const scene = sceneRef.current;
    if (!scene) return;
    scene.throwChip(event.clientX, event.clientY);
    setThrown((n) => n + 1);
  };

  const wordsSung = Math.min(allWordStarts.length, wordsSungAt(audioRef.current?.currentTime ?? 0));

  return (
    <div
      ref={rootRef}
      role="dialog"
      aria-modal="true"
      aria-label="Sing along with the chippytea song"
      tabIndex={-1}
      className={`karaoke-root fixed inset-0 z-50 select-none overflow-hidden bg-paper text-ink outline-none ${leaving ? "leaving" : ""}`}
      data-reduced={reduced ? "1" : undefined}
      data-section={run.key}
      onAnimationEnd={(event) => {
        if (leaving && event.animationName === "karaoke-out") onClose();
      }}
      onPointerDown={throwFrom}
    >
      <audio
        ref={audioRef}
        src={SONG_SRC}
        preload="auto"
        onPlay={() => setPlaying(true)}
        onPause={() => setPlaying(false)}
        onEnded={() => {
          setPlaying(false);
          setEnded(true);
        }}
        onLoadedMetadata={(event) => setDuration(event.currentTarget.duration)}
      />
      {/* The paper: a dot grid that moves with the section, washed in its colour. */}
      <div className="karaoke-dots" aria-hidden="true" />
      {WASHES.map((wash) => (
        <div key={wash} className={`karaoke-wash wash-${wash}`} aria-hidden="true" />
      ))}
      <canvas ref={canvasRef} className="pointer-events-none absolute inset-0" aria-hidden="true" />

      {/* The section, stamped onto the page as it arrives. */}
      {!ended ? (
        <div key={runIdx} className="section-stamp" aria-hidden="true">
          <Tape uid={`stamp${runIdx}`} className="absolute -top-3 left-1/2 z-[2] -translate-x-1/2 -rotate-3" />
          <InkBox as="div" variant="card" seed={171 + runIdx} radius={12} className="px-6 py-3 text-[clamp(22px,4.6vw,42px)] font-bold leading-tight drop-shadow-[0_3px_6px_rgba(51,48,43,0.16)]">
            {run.label}
          </InkBox>
        </div>
      ) : null}

      {/* Top: the chip count, the section, the way out. */}
      <div className="pointer-events-none absolute inset-x-0 top-0 z-10 flex items-start justify-between gap-3 px-4 pt-4 sm:px-6 sm:pt-5">
        <div className="flex flex-col gap-1">
          <canvas
            ref={scoreRef}
            width={SCORE_W}
            height={SCORE_H}
            style={{ width: SCORE_W, height: SCORE_H }}
            aria-hidden="true"
          />
          <span className="pl-1 text-[11px] text-ink-soft">one a word, one a throw</span>
        </div>
        <div key={runIdx} className="section-tag absolute left-1/2 top-[58px] -translate-x-1/2 -rotate-1 sm:top-5">
          <InkBox as="div" variant="card" seed={131} radius={8} className="whitespace-nowrap px-3 py-1.5 text-[12.5px] font-medium">
            {run.label}
          </InkBox>
        </div>
        <button
          ref={closeRef}
          type="button"
          data-ui
          onClick={beginClose}
          aria-label="Close the karaoke"
          className="pointer-events-auto cursor-pointer border-none bg-transparent p-0 active:scale-95"
        >
          <svg width={36} height={36} viewBox="0 0 36 36" aria-hidden="true">
            <path d={closeArt.ring} {...strokeProps} fill={tea.card} stroke={tea.ink} strokeWidth={1.5} />
            {closeArt.cross.map((d, i) => (
              <path key={i} d={d} stroke={tea.ink} strokeWidth={2.2} {...strokeProps} />
            ))}
          </svg>
        </button>
      </div>

      {/* The lyric. */}
      <div
        inert={ended}
        className={`karaoke-halo pointer-events-none absolute inset-x-0 top-[17%] bottom-[24%] flex flex-col items-center justify-center gap-4 px-5 text-center transition-opacity duration-300 sm:gap-6 ${ended ? "opacity-0" : ""}`}
      >
        <p className="m-0 max-w-[26em] text-[clamp(14px,2.2vw,20px)] font-medium leading-snug text-ink/45" aria-hidden="true">
          {prevLine ? prevLine.words.map((word) => word.text).join(" ") : " "}
        </p>
        <div className="kline-stage mt-2 sm:mt-3">
        <div
          key={lineIdx}
          ref={lineRef}
          className="kline relative max-w-[18em] text-[clamp(27px,5.4vw,54px)] font-bold leading-[1.3] tracking-[-0.01em]"
          aria-label={line.words.map((word) => word.text).join(" ")}
        >
          {line.words.map((word, index) => (
            <button
              key={index}
              type="button"
              data-ui
              data-state="todo"
              ref={(el) => {
                wordRefs.current[index] = el;
              }}
              onClick={() => seekTo(word.start)}
              className="kw pointer-events-auto"
              aria-label={`Skip to “${word.text}”`}
            >
              <span className="kw-ink">{word.text}</span>
              <span
                className="kw-gold"
                aria-hidden="true"
                ref={(el) => {
                  goldRefs.current[index] = el;
                }}
              >
                {word.text}
              </span>
            </button>
          ))}
          <svg ref={ballRef} width={32} height={14} className="kw-ball" aria-hidden="true">
            <g transform="translate(1 7)">
              <ChipGlyph length={30} seed={5} />
            </g>
          </svg>
        </div>
        </div>
        <p className="m-0 max-w-[26em] text-[clamp(15px,2.6vw,24px)] font-semibold leading-snug text-ink/55" aria-hidden="true">
          {nextLine ? nextLine.words.map((word) => word.text).join(" ") : " "}
        </p>
      </div>

      {/* A hint, until the first chip goes flying. */}
      {thrown === 0 && !ended ? (
        <div className="pointer-events-none absolute left-4 top-[104px] rotate-2 sm:top-auto sm:bottom-[88px]">
          <Tape uid="hint" className="absolute -top-2.5 left-5 z-[2] -rotate-3 scale-75" />
          <InkBox as="div" variant="card" seed={141} radius={8} className="px-3 py-2 text-[12px] text-ink-soft">
            tap anywhere to throw a chip
          </InkBox>
        </div>
      ) : null}

      {/* The controls. */}
      <div data-ui inert={ended} className="absolute inset-x-0 bottom-0 flex items-center gap-3 px-4 pb-5 pt-2 sm:px-6 sm:pb-6">
        <button
          type="button"
          onClick={togglePlay}
          aria-label={playing ? "Pause the song" : "Play the song"}
          className="cursor-pointer border-none bg-transparent p-0 active:scale-95"
        >
          <PlayRing size={40} playing={playing} seed={45} />
        </button>
        <div ref={barRef} className="relative h-6 min-w-0 flex-1">
          {barW > 2 ? (
            <svg width={barW} height={24} className="absolute left-0 top-0" aria-hidden="true">
              <g transform="translate(0 12)">
                <path d={barTrack} stroke={inkA(0.22)} strokeWidth={1.6} {...strokeProps} />
                <path ref={barFillRef} d="" stroke={tea.gold} strokeWidth={3} {...strokeProps} />
                <g ref={barThumbRef} transform="translate(0 0) rotate(-8)">
                  <ChipGlyph length={22} seed={11} />
                </g>
              </g>
            </svg>
          ) : null}
          <input
            ref={seekRef}
            type="range"
            min={0}
            max={duration || 1}
            step={0.05}
            defaultValue={0}
            onChange={(event) => seekTo(Number(event.target.value))}
            aria-label="Song position"
            className="absolute inset-0 h-full w-full cursor-pointer opacity-0"
          />
        </div>
        <span ref={timeRef} className="min-w-[78px] text-right text-[12px] tabular-nums text-ink-soft">
          0:00 / {clock(duration)}
        </span>
      </div>

      {/* The end. */}
      {ended ? (
        <div data-ui className="absolute inset-0 flex items-center justify-center px-5">
          <div className="animate-panel-in relative -rotate-1">
            <Tape uid="end" className="absolute -top-3 left-1/2 z-[2] -translate-x-1/2 -rotate-2" />
            <InkBox as="div" variant="card" seed={151} radius={12} className="max-w-[360px] px-6 pb-5 pt-6 text-center drop-shadow-[0_2px_5px_rgba(51,48,43,0.14)]">
              <p className="m-0 text-[22px] font-bold leading-tight">That&rsquo;s the lot.</p>
              <p className="mb-0 mt-2 text-[13.5px] leading-[1.5] text-ink-soft">
                You got through {wordsSung} words and threw {thrown} {thrown === 1 ? "chip" : "chips"}.
                {thrown === 0 ? " Not one chip thrown. Next time, mate." : " Lovely."}
              </p>
              <div className="mt-4 flex items-center justify-center gap-3">
                <InkBox variant="primary" seed={161} onClick={again} className="px-5 py-2.5 text-[14px]">
                  Again
                </InkBox>
                <InkBox variant="quiet" seed={163} onClick={beginClose} className="px-4 py-2.5 text-[13.5px]">
                  Back to the page
                </InkBox>
              </div>
            </InkBox>
          </div>
        </div>
      ) : null}
    </div>
  );
}
