"use client";

// The shop radio: a drawn slip taped to the corner of the page, playing
// "Save Your Mac with Chippytea". While it plays, five chips stand up and
// bounce to the actual audio — an AnalyserNode, not a fake loop — and the
// volume knob is, of course, a chip.

import { useEffect, useMemo, useRef, useState } from "react";
import { tea, inkA, goldDeepA, handPathD, roundedRectSamples, circleSamples, lineSamples } from "@/lib/ink";
import { paintChip } from "@/lib/draw";
import { getAudioContext } from "@/lib/chime";
import { Tape } from "./art";

const EQ_W = 52;
const EQ_H = 34;
const SLIDER_W = 76;

/// A little chip lying flat, as SVG — the volume slider's thumb.
function ChipThumb({ x }: { x: number }) {
  const art = useMemo(() => {
    const length = 16;
    const thickness = length * 0.3;
    const body = handPathD(
      roundedRectSamples(0, -thickness / 2, length, thickness, thickness * 0.42, 4),
      true,
      length * 0.03,
      7
    );
    return { body, thickness };
  }, []);
  return (
    <g transform={`translate(${x} 0) rotate(-8)`}>
      <path d={art.body} fill={tea.gold} />
      <path d={art.body} stroke={tea.ink} strokeWidth={1} fill="none" strokeLinecap="round" strokeLinejoin="round" />
      <path d={`M3 ${-art.thickness * 0.1}Q8 ${-art.thickness * 0.4} 12 ${-art.thickness * 0.15}`} stroke={goldDeepA(0.5)} strokeWidth={1} fill="none" strokeLinecap="round" />
    </g>
  );
}

export function MusicPlayer() {
  const audioRef = useRef<HTMLAudioElement>(null);
  const eqRef = useRef<HTMLCanvasElement>(null);
  const analyserRef = useRef<AnalyserNode | null>(null);
  const wiredRef = useRef(false);
  const rafRef = useRef(0);
  const [playing, setPlaying] = useState(false);
  const [volume, setVolume] = useState(0.75);
  const [pressSeed, setPressSeed] = useState(0);

  // The play button: a drawn ring; pressing re-seeds it, as if redrawn.
  const ring = useMemo(
    () => handPathD(circleSamples({ x: 17, y: 17 }, 15.2, 16, -0.5), true, 1, 45 + pressSeed * 3 + (playing ? 1 : 0)),
    [pressSeed, playing]
  );
  const playGlyph = useMemo(
    () => handPathD([{ x: 13.5, y: 11 }, { x: 24, y: 17 }, { x: 13.5, y: 23 }, { x: 13.5, y: 11 }], true, 0.6, 51),
    []
  );
  const pauseGlyphs = useMemo(
    () => [
      handPathD([{ x: 13.5, y: 11.5 }, { x: 13.8, y: 17 }, { x: 13.5, y: 22.5 }], false, 0.5, 53),
      handPathD([{ x: 20.5, y: 11.5 }, { x: 20.2, y: 17 }, { x: 20.5, y: 22.5 }], false, 0.5, 55),
    ],
    []
  );
  const sliderTrack = useMemo(
    () => handPathD(lineSamples({ x: 1, y: 0 }, { x: SLIDER_W - 1, y: 0 }, 12), false, 0.5, 107),
    []
  );
  const sliderFillWidth = Math.max(2, (SLIDER_W - 2) * volume);
  const sliderFill = useMemo(
    () => handPathD(lineSamples({ x: 1, y: 0 }, { x: 1 + sliderFillWidth, y: 0 }, 12), false, 0.7, 109),
    [sliderFillWidth]
  );

  const drawEq = (levels: number[], boil: number) => {
    const canvas = eqRef.current;
    const ctx = canvas?.getContext("2d");
    if (!canvas || !ctx) return;
    const dpr = Math.min(2, window.devicePixelRatio || 1);
    if (canvas.width !== EQ_W * dpr) {
      canvas.width = EQ_W * dpr;
      canvas.height = EQ_H * dpr;
    }
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, EQ_W, EQ_H);
    levels.forEach((level, index) => {
      const length = 9 + level * 21;
      ctx.save();
      ctx.translate(6 + index * 10, EQ_H - 2 - length / 2);
      ctx.rotate(-Math.PI / 2 + ((index % 2 === 0 ? 1 : -1) * 4 * Math.PI) / 180);
      paintChip(ctx, 0, 0, length, 7 + index * 7 + boil * 5);
      ctx.restore();
    });
  };

  // Idle: the chips rest low and still.
  useEffect(() => {
    if (!playing) drawEq([0.22, 0.38, 0.16, 0.32, 0.12], 0);
  }, [playing]);

  const toggle = async () => {
    const audio = audioRef.current;
    if (!audio) return;
    setPressSeed((s) => s + 1);
    if (playing) {
      audio.pause();
      return;
    }
    try {
      const ctx = getAudioContext();
      if (!wiredRef.current) {
        const source = ctx.createMediaElementSource(audio);
        const analyser = ctx.createAnalyser();
        analyser.fftSize = 64;
        analyser.smoothingTimeConstant = 0.75;
        source.connect(analyser);
        analyser.connect(ctx.destination);
        analyserRef.current = analyser;
        wiredRef.current = true;
      }
    } catch {
      // If WebAudio is unavailable the song still plays, chips just rest.
    }
    audio.volume = volume;
    await audio.play().catch(() => {});
  };

  useEffect(() => {
    if (!playing) {
      cancelAnimationFrame(rafRef.current);
      return;
    }
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
      drawEq([0.5, 0.7, 0.4, 0.6, 0.35], 0);
      return;
    }
    const analyser = analyserRef.current;
    const bins = analyser ? new Uint8Array(analyser.frequencyBinCount) : null;
    let last = 0;
    const frame = (now: number) => {
      rafRef.current = requestAnimationFrame(frame);
      if (now - last < 33) return; // ~30 fps is plenty for hand animation
      last = now;
      let levels = [0.4, 0.6, 0.5, 0.55, 0.35];
      if (analyser && bins) {
        analyser.getByteFrequencyData(bins);
        const band = (from: number, to: number) => {
          let sum = 0;
          for (let i = from; i <= to; i++) sum += bins[i];
          return sum / (to - from + 1) / 255;
        };
        levels = [band(1, 2), band(3, 5), band(6, 9), band(10, 15), band(16, 24)].map((v) => Math.min(1, v * 1.5));
      }
      drawEq(levels, Math.floor(now / 167) % 3);
    };
    rafRef.current = requestAnimationFrame(frame);
    return () => cancelAnimationFrame(rafRef.current);
  }, [playing]);

  const changeVolume = (v: number) => {
    setVolume(v);
    if (audioRef.current) audioRef.current.volume = v;
  };

  return (
    <div className="fixed bottom-4 left-4 z-40 -rotate-1">
      <Tape uid="rt" className="absolute -top-2.5 right-6 z-[2] rotate-2 scale-75" />
      <div className="relative rounded-[11px] border-[1.4px] border-solid border-ink bg-card px-3.5 py-2.5 drop-shadow-[0_2px_5px_rgba(51,48,43,0.14)]">
        <audio
          ref={audioRef}
          src="/save-your-mac-with-chippytea.mp3"
          preload="none"
          onPlay={() => setPlaying(true)}
          onPause={() => setPlaying(false)}
          onEnded={() => setPlaying(false)}
        />
        <div className="flex items-center gap-3">
          <button
            type="button"
            onClick={toggle}
            aria-label={playing ? "Pause the shop radio" : "Play the shop radio"}
            className="cursor-pointer border-none bg-transparent p-0 active:scale-95"
          >
            <svg width={34} height={34} aria-hidden="true">
              <path d={ring} fill={playing ? tea.gold : tea.card} stroke={tea.ink} strokeWidth={1.5} strokeLinecap="round" strokeLinejoin="round" />
              {playing ? (
                pauseGlyphs.map((d, i) => (
                  <path key={i} d={d} stroke={tea.ink} strokeWidth={2.4} fill="none" strokeLinecap="round" />
                ))
              ) : (
                <path d={playGlyph} fill={tea.ink} stroke={tea.ink} strokeWidth={1.4} strokeLinejoin="round" />
              )}
            </svg>
          </button>
          <div className="flex flex-col gap-1">
            <div className="max-w-[168px] text-[11.5px] font-semibold leading-tight text-ink">
              Save Your Mac with Chippytea
            </div>
            <div className="flex items-center gap-2">
              <span className="text-[9.5px] text-ink-soft">the shop radio</span>
              <span className="relative inline-block" style={{ width: SLIDER_W, height: 14 }}>
                <svg width={SLIDER_W} height={14} className="absolute left-0 top-0" aria-hidden="true">
                  <g transform="translate(0 7)">
                    <path d={sliderTrack} stroke={inkA(0.22)} strokeWidth={1.4} fill="none" strokeLinecap="round" />
                    <path d={sliderFill} stroke={tea.gold} strokeWidth={2.6} fill="none" strokeLinecap="round" />
                    <ChipThumb x={Math.max(0, Math.min(SLIDER_W - 16, volume * (SLIDER_W - 16)))} />
                  </g>
                </svg>
                <input
                  type="range"
                  min={0}
                  max={1}
                  step={0.01}
                  value={volume}
                  onChange={(e) => changeVolume(Number(e.target.value))}
                  aria-label="Radio volume"
                  className="absolute inset-0 h-full w-full cursor-pointer opacity-0"
                />
              </span>
            </div>
          </div>
          <canvas ref={eqRef} width={EQ_W} height={EQ_H} style={{ width: EQ_W, height: EQ_H }} className="shrink-0" aria-hidden="true" />
        </div>
      </div>
    </div>
  );
}
