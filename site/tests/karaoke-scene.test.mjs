// Run with `bun test tests/karaoke-scene.test.mjs` to resolve the scene's TS imports.
import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { KaraokeScene } from "../lib/karaokeScene.ts";

// Record drawing commands; these tests exercise the real scene without a DOM.
class RecordedPath {
  constructor(svg) { this.commands = svg ? [["svg", svg]] : []; }
  moveTo(...args) { this.commands.push(["moveTo", ...args]); }
  lineTo(...args) { this.commands.push(["lineTo", ...args]); }
  quadraticCurveTo(...args) { this.commands.push(["quadraticCurveTo", ...args]); }
  closePath() { this.commands.push(["closePath"]); }
}

const originalPath = globalThis.Path2D;
before(() => { globalThis.Path2D = RecordedPath; });
after(() => {
  if (originalPath === undefined) delete globalThis.Path2D;
  else globalThis.Path2D = originalPath;
});

function draw(scene, overrides = {}) {
  const operations = [];
  const ctx = new Proxy({ globalAlpha: 1 }, {
    get(target, key) {
      if (key in target) return target[key];
      return (...args) => operations.push([key, ...args]);
    },
  });
  scene.draw(ctx, {
    time: 0.5, now: 1000, dt: 1 / 30, level: 0.2,
    playing: false, line: 0, wrapChips: 0, scoreBox: null,
    reduced: true, ...overrides,
  });
  return operations;
}

test("reduced-motion throws land once and leave no frozen particles", () => {
  const scene = new KaraokeScene();
  scene.resize(1024, 768);
  const landings = [];
  scene.onLand = (x, y) => landings.push({ x, y });
  for (let i = 0; i < 5; i++) scene.throwChip(100 + i * 20, 100);
  draw(scene);
  assert.equal(landings.length, 5);
  assert.ok(landings.every(({ x, y }) => x === 512 && y > 500));
  assert.equal(scene.chips.length, 0);
  draw(scene, { now: 2000 });
  assert.equal(landings.length, 5);

  // "Watch those little chips pop out" still cues its decorative burst.
  draw(scene, { time: 45.5, line: 11, now: 3000, playing: true });
  assert.equal(scene.chips.length, 0);
  assert.equal(landings.length, 5);
});

test("reduced-motion artwork stays still as the clock and bass change", () => {
  const scene = new KaraokeScene();
  scene.resize(1024, 768);
  const first = draw(scene, { playing: true, now: 1000, level: 0.1 });
  const next = draw(scene, { playing: true, now: 2200, level: 0.9 });
  assert.deepEqual(next, first);
});

test("normal-motion throws retain their flight and land exactly once", () => {
  const scene = new KaraokeScene();
  scene.resize(1024, 768);
  let landings = 0;
  scene.onLand = () => { landings += 1; };
  scene.throwChip(200, 100);
  draw(scene, { reduced: false, dt: 0 });
  assert.equal(landings, 0);
  for (let frame = 0; frame < 60; frame++) {
    draw(scene, { reduced: false, now: 1000 + frame * 1000 / 30 });
  }
  assert.equal(landings, 1);
  assert.equal(scene.chips.length, 0);
});
