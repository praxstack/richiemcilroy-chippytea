const REPOSITORY_URL = "https://github.com/richiemcilroy/chippytea";
const RELEASES_URL = `${REPOSITORY_URL}/releases`;
export const LATEST_RELEASE_API = "https://api.github.com/repos/richiemcilroy/chippytea/releases/latest";
export const DOWNLOAD_REVALIDATE_SECONDS = 300;

// Keep these bounds aligned with the release pipeline's CFBundleVersion policy.
const RELEASE_TAG = /^v((?:0|[1-9][0-9]{0,3})\.(?:0|[1-9][0-9]?)\.(?:0|[1-9][0-9]?))$/;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function resolveReleaseDownload(release: unknown): string | null {
  if (
    !isRecord(release) ||
    release.draft !== false ||
    release.prerelease !== false ||
    typeof release.tag_name !== "string" ||
    !Array.isArray(release.assets) ||
    !release.assets.every((asset) => isRecord(asset) && typeof asset.name === "string")
  ) {
    return null;
  }

  const tag = RELEASE_TAG.exec(release.tag_name);
  // JavaScript's $ also matches before a final newline; require the entire tag.
  if (!tag || tag[0] !== release.tag_name) return null;

  const filename = `Chippytea-${tag[1]}-universal.dmg`;
  const assets = release.assets.filter((asset) => asset.name === filename);
  if (assets.length !== 1) return null;

  const asset = assets[0];
  const expectedURL = `${RELEASES_URL}/download/${release.tag_name}/${filename}`;
  if (
    asset.state !== "uploaded" ||
    typeof asset.size !== "number" ||
    !Number.isSafeInteger(asset.size) ||
    asset.size <= 0 ||
    asset.browser_download_url !== expectedURL
  ) {
    return null;
  }

  // Construct the destination ourselves instead of forwarding an upstream URL.
  return expectedURL;
}

async function fetchReleaseDownload(
  fetchLatestRelease: () => Promise<Response>,
): Promise<string | null> {
  try {
    const upstream = await fetchLatestRelease();
    if (upstream.status !== 200) return null;
    return resolveReleaseDownload(await upstream.json());
  } catch {
    // Cache failures as data, rather than repeatedly retrying a rejected promise.
    return null;
  }
}

export function createReleaseLookup(
  fetchLatestRelease: () => Promise<Response>,
  now: () => number = () => performance.now(),
): () => Promise<string | null> {
  let recent: { value: string | null; expiresAt: number } | undefined;
  let inFlight: Promise<string | null> | undefined;

  return () => {
    if (recent && now() < recent.expiresAt) return Promise.resolve(recent.value);
    if (inFlight) return inFlight;

    // Protect one warm instance from concurrent misses and failed cache writes.
    // The route also uses Next's shared Data Cache; this is not a global lock.
    inFlight = fetchReleaseDownload(fetchLatestRelease)
      .then((value) => {
        recent = { value, expiresAt: now() + DOWNLOAD_REVALIDATE_SECONDS * 1000 };
        return value;
      })
      .finally(() => {
        inFlight = undefined;
      });
    return inFlight;
  };
}

const UNAVAILABLE_PAGE = `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Download temporarily unavailable: Chippytea</title>
  <style>
    body { margin: 0; padding: 3rem 1.5rem; background: #fffdf6; color: #33302b; font: 1rem/1.6 system-ui, sans-serif; }
    main { max-width: 36rem; margin: 8vh auto; }
    h1 { font-size: 1.8rem; line-height: 1.25; }
    a { color: inherit; text-underline-offset: .2em; }
    a:hover { text-decoration-color: #b07916; }
  </style>
</head>
<body>
  <main>
    <h1>The Mac download is temporarily unavailable.</h1>
    <p>We couldn’t confirm a published Mac installer. Please try again in a few minutes.</p>
    <p><a href="/download">Try again</a> · <a href="${RELEASES_URL}">View releases</a> · <a href="${REPOSITORY_URL}#build-and-run">Build from source</a></p>
    <p><a href="/">Back to Chippytea</a></p>
  </main>
</body>
</html>`;

function unavailableResponse(): Response {
  return new Response(UNAVAILABLE_PAGE, {
    status: 503,
    headers: {
      "Cache-Control": "no-store",
      "Content-Type": "text/html; charset=utf-8",
      "Content-Security-Policy": "default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
      "Referrer-Policy": "no-referrer",
      "Retry-After": String(DOWNLOAD_REVALIDATE_SECONDS),
      "X-Content-Type-Options": "nosniff",
    },
  });
}

export async function createDownloadResponse(
  getDownload: () => Promise<string | null>,
): Promise<Response> {
  try {
    const destination = await getDownload();
    if (!destination) return unavailableResponse();

    return new Response(null, {
      status: 307,
      headers: {
        Location: destination,
        "Cache-Control": "no-store",
      },
    });
  } catch {
    // A cache infrastructure error must not bypass caching and hit GitHub again.
    return unavailableResponse();
  }
}
