// Writes the share image and the touch icon by drawing the page's own art in
// headless Chrome, so they match the site pixel for pixel:
//
//   bun scripts/gen-og.ts
//
//   app/opengraph-image.png   1200 × 630 at 2× (Open Graph and Twitter cards)
//   app/apple-icon.png        180 × 180 (iOS home screen, Safari bookmarks)
//
// Needs Google Chrome or Chromium (CHROME_PATH overrides the search). No
// fonts are downloaded: the headline uses the system font, as the page does.

import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { existsSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { FishSvg, WordmarkSvg, Tape } from "../components/art";
import { tea, inkA, inkNoise, handPathD, lineSamples, roundedRectSamples } from "../lib/ink";
import { site } from "../lib/site";

const W = 1200;
const H = 630;
const appDir = fileURLToPath(new URL("../app/", import.meta.url));
const stroke = 'fill="none" stroke-linecap="round" stroke-linejoin="round"';
const font = `ui-rounded, "SF Pro Rounded", -apple-system, BlinkMacSystemFont, "SF Pro Text", "Segoe UI", system-ui, sans-serif`;

function findChrome(): string {
  const candidates = [
    process.env.CHROME_PATH,
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
    "/Applications/Google Chrome Canary.app/Contents/MacOS/Google Chrome Canary",
    "/usr/bin/google-chrome",
    "/usr/bin/chromium",
    "/usr/bin/chromium-browser",
  ];
  const playwright = join(homedir(), "Library/Caches/ms-playwright");
  if (existsSync(playwright)) {
    for (const dir of readdirSync(playwright).filter((d) => /^chromium-\d+$/.test(d)).sort().reverse()) {
      candidates.push(join(playwright, dir, "chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing"));
    }
  }
  const found = candidates.find((c) => c && existsSync(c));
  if (!found) throw new Error("No Chrome found. Install Google Chrome or set CHROME_PATH.");
  return found;
}

/// The primary button's gold scribble hatch, per InkBox.
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

/// A drawn box the size of its contents, per InkBox: wobbly outline, optional hatch.
function inkBox(w: number, h: number, radius: number, seed: number, variant: "primary" | "card" | "quiet") {
  const d = handPathD(roundedRectSamples(0.8, 0.8, w - 1.6, h - 1.6, radius, variant === "card" ? 11 : 9), true, variant === "card" ? 1 : 0.9, seed);
  const fill = variant === "primary" ? tea.gold : variant === "card" ? tea.card : "rgba(255, 253, 246, 0.9)";
  const hatch =
    variant === "primary"
      ? `<clipPath id="h${seed}"><path d="${d}"/></clipPath><path d="${hatchD(w, h, seed)}" stroke="rgba(192, 127, 23, 0.3)" stroke-width="1" fill="none" clip-path="url(#h${seed})"/>`
      : "";
  return `<svg width="${w}" height="${h}" style="position:absolute;left:0;top:0">
    <path d="${d}" fill="${fill}"/>${hatch}
    <path d="${d}" stroke="${variant === "quiet" ? inkA(0.8) : tea.ink}" stroke-width="1.4" ${stroke}/>
  </svg>`;
}

function underline(seed = 121) {
  const d = handPathD(lineSamples({ x: 1, y: 4 }, { x: 299, y: 4 }, 16), false, 0.9, seed);
  return `<svg viewBox="0 0 300 8" preserveAspectRatio="none" style="position:absolute;left:0;bottom:-.13em;height:.13em;width:100%;transform:rotate(-.5deg)"><path d="${d}" stroke="${tea.gold}" stroke-width="2.6" ${stroke}/></svg>`;
}

function meter(width: number, fraction: number) {
  const base = handPathD(lineSamples({ x: 1, y: 2 }, { x: width - 1, y: 2 }, 16), false, 0.5, 101);
  const earned = handPathD(lineSamples({ x: 1, y: 2 }, { x: 1 + width * fraction, y: 2 }, 16), false, 0.8, 104);
  return `<svg width="${width}" height="4" style="display:block"><path d="${base}" stroke="${inkA(0.2)}" stroke-width="1.4" ${stroke}/><path d="${earned}" stroke="${tea.gold}" stroke-width="3" ${stroke}/></svg>`;
}

function rule(width: number, seed = 9) {
  const d = handPathD(lineSamples({ x: 1, y: 1 }, { x: width - 1, y: 1 }, 16), false, 0.7, seed);
  return `<svg width="${width}" height="2" style="display:block"><path d="${d}" stroke="${inkA(0.12)}" stroke-width="1.1" ${stroke}/></svg>`;
}

const css = `
  html, body { margin: 0; width: ${W}px; height: ${H}px; overflow: hidden; }
  body {
    position: relative; color: ${tea.ink}; font-family: ${font};
    -webkit-font-smoothing: antialiased; text-rendering: geometricPrecision;
    background-color: ${tea.paper};
    background-image: radial-gradient(circle, rgba(51, 48, 43, 0.05) 0.85px, transparent 0.95px);
    background-size: 18px 18px;
  }
  .abs { position: absolute; }
  .bp1, .bp2 { display: none; }
  .soft { color: ${tea.inkSoft}; }
  h1 { margin: 0; font-size: 74px; line-height: 1.1; font-weight: 700; letter-spacing: -0.01em; }
  p { margin: 0; }
`;

async function paintScript(): Promise<string> {
  const built = await Bun.build({
    entrypoints: [fileURLToPath(new URL("./og-paint.ts", import.meta.url))],
    target: "browser",
    format: "iife",
    minify: true,
  });
  if (!built.success) throw new Error(built.logs.map(String).join("\n"));
  return built.outputs[0].text();
}

async function shareCard(): Promise<string> {
  const sign =
    renderToStaticMarkup(createElement(FishSvg, { height: 46, uid: "og", phases: 1 })) +
    renderToStaticMarkup(createElement(WordmarkSvg, { height: 40, phases: 1 }));
  const tape = renderToStaticMarkup(createElement(Tape, { uid: "ogt" }));
  const cardW = 436;
  const cardH = 470;
  const inner = cardW - 52;
  return `<!doctype html><html lang="en-GB"><head><meta charset="utf-8"><style>${css}</style></head><body>
  <div class="abs" style="left:64px;top:52px;display:flex;align-items:center;gap:14px">
    <span style="display:inline-flex;transform:rotate(-2deg)">${sign.slice(0, sign.indexOf("</svg>") + 6)}</span>
    ${sign.slice(sign.indexOf("</svg>") + 6)}
  </div>

  <h1 class="abs" style="left:64px;top:150px;width:610px">Free up space<br><span style="position:relative;display:inline-block">on your Mac.${underline()}</span></h1>
  <p class="abs" style="left:64px;top:356px;width:560px;font-size:25px;line-height:1.42;color:rgba(51,48,43,.88)">
    Finds old build folders, dependencies and installers you no longer need. You choose what goes.
  </p>

  <div class="abs" style="left:64px;top:462px;display:flex;align-items:center;gap:26px">
    <span style="position:relative;display:inline-flex;align-items:center;justify-content:center;width:238px;height:60px;font-size:22px;font-weight:600">
      ${inkBox(238, 60, 10, 31, "primary")}<span style="position:relative">Download for Mac</span>
    </span>
    <span style="font-size:19px;color:rgba(51,48,43,.8);text-decoration:underline;text-decoration-color:${tea.gold};text-decoration-thickness:2px;text-underline-offset:5px">Build from source</span>
  </div>
  <p class="abs soft" style="left:64px;top:552px;font-size:18px;line-height:1.5">Free &amp; open source &middot; Apple Silicon &middot; macOS 14 or later</p>

  <div class="abs" style="left:700px;top:84px;width:${cardW}px;height:${cardH}px;filter:drop-shadow(0 2px 5px rgba(51,48,43,.14))">
    ${inkBox(cardW, cardH, 14, 3, "card")}
    <div class="abs" style="left:${cardW / 2 - 36}px;top:-12px;transform:rotate(2deg) scale(1.3)">${tape}</div>
    <canvas id="balance" class="abs" style="left:24px;top:34px"></canvas>
    <p class="abs soft" style="left:26px;top:120px;font-size:15px">your tea, in the paper</p>
    <p class="abs soft" style="left:26px;top:154px;font-size:14.5px">96 GB credited &middot; 63 MB to your next chip</p>
    <div class="abs" style="left:26px;top:180px">${meter(inner, 0.37)}</div>
    <canvas id="wrap" class="abs" style="left:18px;top:198px"></canvas>
    <div class="abs" style="left:26px;top:338px">${rule(inner)}</div>
    <p class="abs" style="left:26px;top:352px;font-size:16.5px;font-weight:600"><span style="position:relative;display:inline-block">Make a bit of room.${underline(123)}</span></p>
    <div class="abs" style="left:26px;top:388px;width:${inner}px;height:54px">
      ${inkBox(inner, 54, 9, 63, "quiet")}
      <div class="abs" style="left:16px;top:0;height:54px;display:flex;align-items:center;font-size:15px;font-weight:600">Rust build folder</div>
      <div class="abs soft" style="right:16px;top:0;height:54px;display:flex;align-items:center;font-size:13px">2.4 GB estimated</div>
    </div>
  </div>
  <script>${await paintScript()}</script>
  </body></html>`;
}

function touchIcon(): string {
  const icon = readFileSync(join(appDir, "icon.svg"), "utf8");
  return `<!doctype html><html><head><meta charset="utf-8"><style>
    html, body { margin: 0; width: 180px; height: 180px; overflow: hidden; }
    body { background: ${tea.paper}; display: flex; align-items: center; justify-content: center; }
    svg { width: 150px; height: 150px; }
  </style></head><body>${icon}</body></html>`;
}

function shoot(chrome: string, html: string, out: string, width: number, height: number, scale: number) {
  const dir = mkdtempSync(join(tmpdir(), "chippytea-og-"));
  const page = join(dir, "page.html");
  writeFileSync(page, html);
  const result = spawnSync(
    chrome,
    [
      "--headless=new",
      "--disable-gpu",
      "--hide-scrollbars",
      "--no-first-run",
      "--no-default-browser-check",
      `--user-data-dir=${join(dir, "profile")}`,
      `--window-size=${width},${height}`,
      `--force-device-scale-factor=${scale}`,
      "--virtual-time-budget=4000",
      `--screenshot=${out}`,
      `file://${page}`,
    ],
    { stdio: "pipe", timeout: 60000 }
  );
  rmSync(dir, { recursive: true, force: true });
  if (result.status !== 0 || !existsSync(out)) {
    throw new Error(`Chrome failed for ${out}:\n${result.stderr?.toString() ?? ""}`);
  }
}

const chrome = findChrome();
const og = join(appDir, "opengraph-image.png");
shoot(chrome, await shareCard(), og, W, H, 2);
console.log(`wrote app/opengraph-image.png (${W * 2} × ${H * 2}, ${Math.round(statSync(og).size / 1024)} KB) for ${site.name}`);
const apple = join(appDir, "apple-icon.png");
shoot(chrome, touchIcon(), apple, 180, 180, 1);
console.log(`wrote app/apple-icon.png (180 × 180, ${Math.round(statSync(apple).size / 1024)} KB)`);
