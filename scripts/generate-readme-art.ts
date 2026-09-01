// Run from the repository root: bun scripts/generate-readme-art.ts
// Reuse the landing page's original drawing paths. No fonts, images or network.
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { fishArt, glyphStrokes, tapeArt, wordArt } from "../site/lib/art";
import { handPathD, inkNoise, lineSamples, roundedRectSamples, tea } from "../site/lib/ink";

const output = fileURLToPath(new URL("../docs/assets/", import.meta.url));
const check = process.argv.includes("--check");
const stroke = 'fill="none" stroke-linecap="round" stroke-linejoin="round"';
const themes = {
  light: { paper: tea.paper, card: tea.card, fold: tea.paperDeep, ink: tea.ink, soft: "#6F695F", blue: tea.biro, rule: "#D8D1C3" },
  dark: { paper: "#252925", card: "#2E332D", fold: "#393E35", ink: "#F3EDDE", soft: "#BEB9AA", blue: "#A8BAEC", rule: "#50574B" },
} as const;
type Theme = (typeof themes)[keyof typeof themes];

function path(d: string, color: string, width = 1.6, extra = "") {
  return `<path d="${d}" stroke="${color}" stroke-width="${width}" ${stroke} ${extra}/>`;
}

function fish(id: string, theme: Theme, steam = true) {
  const art = fishArt(401, steam);
  return `<g>
    <defs><clipPath id="${id}"><path d="${art.bodyD}"/></clipPath></defs>
    ${art.steam.map((s) => path(s.d, theme.soft, s.width)).join("")}
    <path d="${art.bodyD}" fill="${tea.gold}"/>
    ${path(art.batterD, tea.goldDeep, 0.9, `clip-path="url(#${id})" opacity=".4"`)}
    ${path(art.bodyD, theme.ink, 1.7)}
    ${art.details.map((s) => path(s.d, tea.ink, s.width)).join("")}
    <circle cx="12" cy="22" r="1.8" fill="${tea.ink}"/>
    <circle cx="11.45" cy="21.45" r=".55" fill="${tea.card}"/>
  </g>`;
}

function word(value: string, color: string, seed = 500) {
  const art = wordArt(value, seed);
  return `<g>${art.strokes.map((d) => path(d, color, 7.5)).join("")}${art.dots.map((d) => `<circle cx="${d.x}" cy="${d.y}" r="${d.r}" fill="${color}"/>`).join("")}</g>`;
}

function tape(theme: Theme) {
  const art = tapeArt(41);
  return `<g transform="translate(744 34) rotate(-5 28 12)">
    <defs><clipPath id="tape"><path d="${art.outlineD}"/></clipPath></defs>
    <path d="${art.outlineD}" fill="${tea.gold}" opacity=".65"/>
    ${art.streaks.map((s) => path(s.d, tea.goldDeep, 1.5, 'clip-path="url(#tape)" opacity=".25"')).join("")}
    ${path(art.outlineD, theme.ink, 0.6, 'opacity=".25"')}
  </g>`;
}

function chip(seed: number, length = 39) {
  const h = length * 0.24;
  const d = handPathD(roundedRectSamples(-length / 2, -h / 2, length, h, h * 0.42, 5), true, length * 0.025, seed);
  return `<g>
    <path d="${d}" fill="${tea.gold}"/>
    ${path(`M${-length * .37} ${-h * .18}Q0 ${-h * .4} ${length * .34} ${-h * .12}`, tea.goldDeep, 1, 'opacity=".45"')}
    ${path(`M${-length * .36} ${h * .23}Q0 ${h * .1} ${length * .38} ${h * .2}`, tea.goldDeep, 1.2, 'opacity=".6"')}
    ${path(d, tea.ink, 1.45)}
    ${path(`M${-length * .32} ${-h * .24}Q${-length * .2} ${-h * .4} ${-length * .1} ${-h * .24}`, "#FFFDF6", 1.2)}
  </g>`;
}

function sparkle(x: number, y: number, size: number, color: string) {
  return `<g transform="translate(${x} ${y})">${[0, 60, 120].map((angle) => path(`M${-size} 0Q0 1 ${size} 0`, color, 1.8, `transform="rotate(${angle})"`)).join("")}</g>`;
}

function number(value: string, theme: Theme) {
  return [...value].map((digit, index) => {
    const paths = glyphStrokes(Number(digit)).map((s, i) => handPathD(s.points, s.closed, 1.7, 900 + index * 31 + i * 37));
    return `<g transform="translate(${index * 62} 0)">${paths.map((d) => path(d, theme.ink, 14)).join("")}${paths.map((d) => path(d, "url(#hatch)", 10.5)).join("")}</g>`;
  }).join("");
}

function paper(theme: Theme) {
  const back = handPathD([
    { x: 30, y: 96 }, { x: 14, y: 45 }, { x: 63, y: 76 }, { x: 102, y: 69 },
    { x: 144, y: 80 }, { x: 189, y: 72 }, { x: 235, y: 82 }, { x: 297, y: 40 },
    { x: 274, y: 99 }, ...lineSamples({ x: 270, y: 100 }, { x: 32, y: 100 }, 10),
  ], true, 1.3, 221);
  const front = handPathD([
    ...lineSamples({ x: 28, y: 92 }, { x: 280, y: 91 }, 10),
    { x: 267, y: 119 }, { x: 154, y: 124 }, { x: 39, y: 120 },
  ], true, 1.2, 240);
  const slots = [
    [82, 70, -18], [123, 67, 9], [164, 72, -8], [206, 70, 14],
    [63, 88, 12], [108, 86, -5], [152, 89, 8], [195, 86, -16], [235, 87, 8],
    [106, 52, -7], [153, 49, -17], [194, 52, 10],
  ];
  return `<g class="paper" transform="translate(618 205)">
    <path d="${back}" fill="${theme.card}"/>
    ${path(back, theme.ink, 1.6)}
    ${[0, 1, 2, 3].map((i) => path(`M${269 - i * 2} ${53 + i * 6}l${16 - i} -2`, theme.rule, 1)).join("")}
    ${slots.map(([x, y, angle], i) => `<g transform="translate(${x} ${y}) rotate(${angle})">${chip(300 + i * 3, 40 + inkNoise(i, 300) * 3)}</g>`).join("")}
    <path d="${front}" fill="${theme.fold}"/>
    ${path(front, theme.ink, 1.7)}
    ${path("M58 109Q97 105 130 109M150 111Q188 108 213 109", theme.rule, 1)}
    ${sparkle(45, 32, 5.4, tea.gold)}${sparkle(260, 24, 4.2, theme.soft)}
  </g>`;
}

function logo(theme: Theme) {
  return `<svg xmlns="http://www.w3.org/2000/svg" width="430" height="112" viewBox="0 0 430 112" role="img" aria-labelledby="title">
  <title id="title">chippytea: a battered fish and hand-lettered wordmark</title>
  <g transform="translate(4 18) scale(1.5)">${fish("logo-fish", theme)}</g>
  <g transform="translate(120 5) scale(.93)">${word("chippytea", theme.ink)}</g>
</svg>\n`;
}

function hero(theme: Theme) {
  const panel = handPathD(roundedRectSamples(593, 49, 354, 331, 17, 16), true, 1.2, 81);
  const card = handPathD(roundedRectSamples(52, 188, 346, 92, 12, 15), true, 1.1, 83);
  return `<svg xmlns="http://www.w3.org/2000/svg" width="1000" height="430" viewBox="0 0 1000 430" role="img" aria-labelledby="title desc">
  <title id="title">A bit more room. A bit more tea.</title>
  <desc id="desc">An illustrated cleanup: review an old node_modules folder, then turn 1.2 GB of credited space into 12 golden chips in a paper tray. Example numbers, not a live scan.</desc>
  <defs>
    <pattern id="dots" width="18" height="18" patternUnits="userSpaceOnUse"><circle cx="3" cy="3" r=".8" fill="${theme.ink}" opacity=".07"/></pattern>
    <pattern id="hatch" width="5" height="5" patternUnits="userSpaceOnUse" patternTransform="rotate(35)"><rect width="5" height="5" fill="${tea.gold}"/><path d="M0 0v5" stroke="${tea.goldDeep}" stroke-width="1" opacity=".45"/></pattern>
  </defs>
  <style>
    text { font-family: ui-rounded, -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif; fill: ${theme.ink}; }
    .soft { fill: ${theme.soft}; }
    .biro { fill: ${theme.blue}; }
    .flight { opacity: 0; animation: chip-flight 7.2s ease-in-out infinite; }
    .f2 { animation-delay: .15s; } .f3 { animation-delay: .3s; }
    .f4 { animation-delay: .45s; } .f5 { animation-delay: .6s; }
    .tray { transform-origin: 772px 318px; animation: settle 7.2s ease-out infinite; }
    .arrow { stroke-dasharray: 5 7; animation: ink-flow 7.2s ease-in-out infinite; }
    @keyframes chip-flight {
      0%, 18% { opacity: 0; transform: translate(448px, 211px) rotate(-35deg) scale(.65); }
      21% { opacity: 1; }
      28% { transform: translate(618px, 147px) rotate(95deg) scale(1); }
      39% { opacity: 1; transform: translate(772px, 271px) rotate(205deg) scale(.9); }
      41%, 100% { opacity: 0; transform: translate(772px, 280px) rotate(220deg) scale(.7); }
    }
    @keyframes settle {
      0%, 38%, 52%, 100% { transform: scale(1); }
      43% { transform: scale(1.04, .94); }
      47% { transform: scale(.99, 1.025); }
    }
    @keyframes ink-flow { 0%, 15%, 55%, 100% { stroke-dashoffset: 0; } 40% { stroke-dashoffset: -36; } }
    @media (prefers-reduced-motion: reduce) {
      .flight { display: none; animation: none; }
      .tray, .arrow { animation: none; }
    }
  </style>
  <rect width="1000" height="430" rx="20" fill="${theme.paper}"/>
  <rect width="1000" height="430" rx="20" fill="url(#dots)"/>
  <text x="52" y="87" font-size="31" font-weight="700">A bit more room.</text>
  <text x="52" y="125" font-size="31" font-weight="700">A bit more tea.</text>
  ${path(handPathD(lineSamples({ x: 52, y: 135 }, { x: 267, y: 136 }, 13), false, .8, 121), tea.gold, 3)}
  <text x="52" y="163" font-size="16" class="soft">A careful clean. A small reward.</text>
  <path d="${card}" fill="${theme.card}"/>
  ${path(card, theme.ink, 1.5)}
  <g transform="translate(70 208)">
    ${path("M0 4Q0 1 3 1h10l5 5h14q3 0 3 3v24q0 3-3 3H3q-3 0-3-3Z", theme.blue, 1.6)}
    ${path("M3 12Q17 10 32 12", theme.blue, 1.2)}
  </g>
  <text x="123" y="221" font-size="19" font-weight="600">node_modules</text>
  <text x="123" y="246" font-size="14" class="soft">old project · 1.2 GB estimated</text>
  <text x="72" y="302" font-size="14" class="biro">Review first. You choose what goes.</text>
  ${path("M418 234Q475 249 540 216", theme.blue, 1.8, 'class="arrow"')}
  ${path("M529 210l14 4-7 13", theme.blue, 1.8)}
  <path d="${panel}" fill="${theme.card}"/>
  ${path(panel, theme.ink, 1.7)}
  ${tape(theme)}
  <g transform="translate(613 67) scale(.5)">${fish("panel-fish", theme, false)}</g>
  <g transform="translate(653 60) scale(.255)">${word("chippytea", theme.ink)}</g>
  <g transform="translate(617 110) scale(.64)">${number("12", theme)}</g>
  <g transform="translate(706 135) scale(.34)">${word("chips", theme.ink, 560)}</g>
  <text x="619" y="194" font-size="14" class="soft">1.2 GB credited. A little more for the paper.</text>
  <g class="tray">${paper(theme)}</g>
  <text x="772" y="357" text-anchor="middle" font-size="14" class="biro">your tea, in the paper</text>
  ${[0, 1, 2, 3, 4].map((i) => `<g class="flight f${i + 1}">${chip(501 + i * 7, 35)}</g>`).join("")}
  <text x="52" y="385" font-size="13" class="soft">100 MB saved = 1 chip. Every reviewed cleanup counts.</text>
</svg>\n`;
}

if (!check) mkdirSync(output, { recursive: true });
for (const [name, theme] of Object.entries(themes)) {
  for (const [file, markup] of [[`chippytea-${name}.svg`, logo(theme)], [`readme-${name}.svg`, hero(theme)]]) {
    const svg = markup.replace(/[ \t]+$/gm, "");
    const destination = `${output}/${file}`;
    if (check) {
      if (readFileSync(destination, "utf8") !== svg) throw new Error(`${file} is stale. Run bun scripts/generate-readme-art.ts`);
    } else {
      writeFileSync(destination, svg);
    }
    console.log(`${check ? "Checked" : "Wrote"} docs/assets/${file}`);
  }
}
