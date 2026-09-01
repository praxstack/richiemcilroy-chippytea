// Writes app/icon.svg: the battered fish, drawn by the same hand as the app.
import { fishArt } from "../lib/art";
import { tea } from "../lib/ink";

const art = fishArt(401, false);
const stroke = `fill="none" stroke-linecap="round" stroke-linejoin="round"`;
const details = art.details
  .map((s) => `<path d="${s.d}" stroke="${s.color}" stroke-width="${s.width}" ${stroke}/>`)
  .join("\n  ");

const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="-2 -12 68 68">
  <clipPath id="b"><path d="${art.bodyD}"/></clipPath>
  <path d="${art.bodyD}" fill="${tea.gold}"/>
  <path d="${art.batterD}" stroke="rgba(192, 127, 23, 0.32)" stroke-width="1" fill="none" clip-path="url(#b)"/>
  <path d="${art.bodyD}" stroke="${tea.ink}" stroke-width="2.4" ${stroke}/>
  ${details}
  <circle cx="12" cy="22" r="1.8" fill="${tea.ink}"/>
  <circle cx="11.45" cy="21.45" r="0.55" fill="${tea.card}"/>
</svg>
`;

await Bun.write(new URL("../app/icon.svg", import.meta.url), svg);
console.log("wrote app/icon.svg");
