import assert from "node:assert/strict";
import { test } from "node:test";
import {
  createDownloadResponse,
  createReleaseLookup,
  DOWNLOAD_REVALIDATE_SECONDS,
  resolveReleaseDownload,
} from "../lib/download.ts";

const REPOSITORY = "https://github.com/richiemcilroy/chippytea";

function responseFromFetch(fetchLatestRelease) {
  return createDownloadResponse(createReleaseLookup(fetchLatestRelease));
}

function release(version = "0.1.0") {
  const name = `chippytea-${version}-universal.dmg`;
  return {
    tag_name: `v${version}`,
    draft: false,
    prerelease: false,
    assets: [
      {
        name,
        state: "uploaded",
        size: 25_000_000,
        browser_download_url: `${REPOSITORY}/releases/download/v${version}/${name}`,
      },
      { name: "appcast.xml" },
      { name: `chippytea-${version}-universal.zip` },
      { name: "release-notes.md" },
      { name: "release.json" },
      { name: "SHA256SUMS" },
    ],
  };
}

test("selects the exact versioned DMG, including subsequent releases and version bounds", () => {
  for (const version of ["0.1.0", "0.1.1", "1.2.3", "0.0.0", "9999.99.99"]) {
    const latest = release(version);
    assert.equal(resolveReleaseDownload(latest), latest.assets[0].browser_download_url);
  }
});

test("rejects malformed, noncanonical and unsupported release versions", () => {
  for (const tag of [
    "0.1.0", "V0.1.0", "v0.1", "v0.1.0.1", "v0.1.0-beta.1", "v0.1.0+build.1",
    "v00.1.0", "v0.01.0", "v0.1.00", "v10000.0.0", "v0.100.0", "v0.0.100",
    "v-1.0.0", " v0.1.0", "v0.1.0 ", "v0.1.0\n", "v0.1.0\r\n", "v０.1.0",
    "", null, 100,
  ]) {
    assert.equal(resolveReleaseDownload({ ...release(), tag_name: tag }), null, String(tag));
  }
});

test("requires a stable published release shape and well-formed asset entries", () => {
  for (const value of [null, undefined, [], {}, "release", 42, false]) {
    assert.equal(resolveReleaseDownload(value), null);
  }
  for (const patch of [
    { draft: true }, { draft: undefined }, { draft: "false" },
    { prerelease: true }, { prerelease: undefined }, { prerelease: 0 },
    { assets: undefined }, { assets: null }, { assets: {} }, { assets: [] },
    { assets: [null] }, { assets: [release().assets[0], {}] },
  ]) {
    assert.equal(resolveReleaseDownload({ ...release(), ...patch }), null);
  }
});

test("requires exactly one uploaded, nonempty DMG for the release version", () => {
  for (const patch of [
    { name: "chippytea-0.1.1-universal.dmg" },
    { name: "chippytea-0.1.0-arm64.dmg" },
    { name: "chippytea-0.1.0-universal.zip" },
    { state: "starter" }, { state: undefined },
    { size: 0 }, { size: -1 }, { size: 1.5 }, { size: "25000000" },
    { size: NaN }, { size: Infinity }, { size: Number.MAX_SAFE_INTEGER + 1 },
    { size: undefined }, { browser_download_url: undefined },
  ]) {
    const latest = release();
    Object.assign(latest.assets[0], patch);
    assert.equal(resolveReleaseDownload(latest), null);
  }
  const duplicate = release();
  duplicate.assets.push({ ...duplicate.assets[0] });
  assert.equal(resolveReleaseDownload(duplicate), null);
});

test("rejects redirects outside the exact immutable GitHub release asset URL", () => {
  const expected = release().assets[0].browser_download_url;
  for (const url of [
    expected.replace("https:", "http:"),
    expected.replace("github.com/", "github.com.evil.example/"),
    expected.replace("github.com/", "github.com@evil.example/"),
    expected.replace("github.com/", "github.com:443/"),
    expected.replace("richiemcilroy/", "other-owner/"),
    expected.replace("chippytea/", "other-repo/"),
    expected.replace("download/v0.1.0/", "download/v0.1.1/"),
    expected.replace("download/v0.1.0/", "latest/download/"),
    expected.replace("chippytea-", "%63hippytea-"),
    `${expected}?download=1`, `${expected}#fragment`, `${expected}\n`,
    "javascript:alert(1)", "//evil.example/installer.dmg",
  ]) {
    const latest = release();
    latest.assets[0].browser_download_url = url;
    assert.equal(resolveReleaseDownload(latest), null, url);
  }
});

test("returns a temporary uncacheable redirect and follows newly published versions", async () => {
  for (const version of ["0.1.0", "0.1.1"]) {
    const latest = release(version);
    let calls = 0;
    const response = await responseFromFetch(async () => {
      calls += 1;
      return Response.json(latest);
    });
    assert.equal(calls, 1);
    assert.equal(response.status, 307);
    assert.equal(response.headers.get("Location"), latest.assets[0].browser_download_url);
    assert.equal(response.headers.get("Cache-Control"), "no-store");
    assert.equal(await response.text(), "");
  }
});

async function assertUnavailable(fetchRelease) {
  const response = await responseFromFetch(fetchRelease);
  assert.equal(response.status, 503);
  assert.equal(response.headers.get("Location"), null);
  assert.equal(response.headers.get("Cache-Control"), "no-store");
  assert.equal(response.headers.get("Retry-After"), "300");
  assert.match(response.headers.get("Content-Type"), /^text\/html;/);
  assert.match(response.headers.get("Content-Security-Policy"), /default-src 'none'/);
  const html = await response.text();
  assert.ok(html.includes(`href="${REPOSITORY}/releases"`));
  assert.ok(html.includes(`href="${REPOSITORY}#build-and-run"`));
  assert.ok(html.includes("temporarily unavailable"));
  return html;
}

test("fails closed for unavailable, rate-limited, redirected or unexpected upstream statuses", async () => {
  for (const status of [201, 302, 403, 404, 429, 500]) {
    await assertUnavailable(async () => Response.json(release(), { status }));
  }
});

test("fails closed for network errors, timeouts, malformed JSON and invalid release metadata", async () => {
  await assertUnavailable(async () => { throw new TypeError("Network failed"); });
  await assertUnavailable(async () => { throw new DOMException("Timed out", "TimeoutError"); });
  await assertUnavailable(async () => new Response("{invalid JSON"));
  await assertUnavailable(async () => Response.json({ ...release(), draft: true }));
  await assertUnavailable(async () => Response.json({ ...release(), assets: [] }));
});

test("never reflects untrusted release content, links or errors in the unavailable page", async () => {
  const injected = "<script>alert('untrusted-release')</script>";
  const latest = release();
  latest.assets[0].browser_download_url = `https://evil.example/${injected}`;
  const html = await assertUnavailable(async () => Response.json({ ...latest, body: injected }));
  assert.ok(!html.includes("evil.example"));
  assert.ok(!html.includes(injected));
  const errorHTML = await assertUnavailable(async () => { throw new Error(injected); });
  assert.equal(errorHTML, html);
});

test("backs off sequential unavailable lookups, including HTTP, network and parse failures", async () => {
  for (const failure of [
    async () => new Response(null, { status: 404 }),
    async () => new Response(null, { status: 403 }),
    async () => new Response(null, { status: 429 }),
    async () => new Response(null, { status: 500 }),
    async () => new Response("{invalid JSON"),
    async () => Response.json({ ...release(), draft: true }),
    async () => { throw new TypeError("Network failed"); },
    async () => { throw new DOMException("Timed out", "TimeoutError"); },
  ]) {
    let calls = 0;
    const lookup = createReleaseLookup(async () => { calls += 1; return failure(); }, () => 0);
    for (let request = 0; request < 65; request += 1) {
      const response = await createDownloadResponse(lookup);
      assert.equal(response.status, 503);
      assert.equal(response.headers.get("Cache-Control"), "no-store");
      assert.equal(response.headers.get("Location"), null);
    }
    assert.equal(calls, 1);
  }
});

test("coalesces concurrent cold and expired requests, then recovers from an unavailable release", async () => {
  let now = 0;
  let calls = 0;
  let completeFetch;
  const lookup = createReleaseLookup(() => {
    calls += 1;
    return new Promise((resolve) => { completeFetch = resolve; });
  }, () => now);

  const missing = Array.from({ length: 64 }, () => lookup());
  assert.equal(calls, 1);
  completeFetch(new Response(null, { status: 404 }));
  assert.deepEqual(await Promise.all(missing), Array(64).fill(null));

  now = DOWNLOAD_REVALIDATE_SECONDS * 1000 - 1;
  assert.equal(await lookup(), null);
  assert.equal(calls, 1);

  now += 1;
  const available = Array.from({ length: 64 }, () => createDownloadResponse(lookup));
  assert.equal(calls, 2);
  const next = release("0.1.1");
  completeFetch(Response.json(next));
  for (const response of await Promise.all(available)) {
    assert.equal(response.status, 307);
    assert.equal(response.headers.get("Location"), next.assets[0].browser_download_url);
    assert.equal(response.headers.get("Cache-Control"), "no-store");
  }
});

test("expires successful outcomes too and replaces them with failures without sharing Response objects", async () => {
  let now = 0;
  let calls = 0;
  let latest = release();
  const lookup = createReleaseLookup(async () => {
    calls += 1;
    return latest ? Response.json(latest) : new Response(null, { status: 403 });
  }, () => now);

  const first = await createDownloadResponse(lookup);
  first.headers.set("X-Request-Only", "first");
  const second = await createDownloadResponse(lookup);
  assert.equal(calls, 1);
  assert.equal(second.status, 307);
  assert.equal(second.headers.get("X-Request-Only"), null);

  latest = null;
  now += DOWNLOAD_REVALIDATE_SECONDS * 1000;
  assert.equal((await createDownloadResponse(lookup)).status, 503);
  assert.equal(calls, 2);

  latest = release("0.1.1");
  assert.equal((await createDownloadResponse(lookup)).status, 503);
  assert.equal(calls, 2);
  now += DOWNLOAD_REVALIDATE_SECONDS * 1000;
  assert.equal((await createDownloadResponse(lookup)).headers.get("Location"), latest.assets[0].browser_download_url);
  assert.equal(calls, 3);
});

test("cache infrastructure errors fail closed without an uncached request fallback", async () => {
  const response = await createDownloadResponse(async () => { throw new Error("Cache unavailable"); });
  assert.equal(response.status, 503);
  assert.equal(response.headers.get("Location"), null);
  assert.equal(response.headers.get("Cache-Control"), "no-store");
  assert.ok(!(await response.text()).includes("Cache unavailable"));
});
