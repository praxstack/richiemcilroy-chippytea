import { unstable_cache } from "next/cache";
import {
  createDownloadResponse,
  createReleaseLookup,
  DOWNLOAD_REVALIDATE_SECONDS,
  LATEST_RELEASE_API,
} from "@/lib/download";

const lookupRelease = createReleaseLookup(() =>
  fetch(LATEST_RELEASE_API, {
    headers: {
      Accept: "application/vnd.github+json",
      "User-Agent": "Chippytea-download",
      "X-GitHub-Api-Version": "2022-11-28",
    },
    cache: "no-store",
    redirect: "error",
    signal: AbortSignal.timeout(8_000),
  }),
);

// Store the validated URL or null, including 404s and transient failures.
// Revalidation can serve the previous result while it refreshes in the background.
const getReleaseDownload = unstable_cache(
  lookupRelease,
  ["chippytea-release-download-v1", LATEST_RELEASE_API],
  { revalidate: DOWNLOAD_REVALIDATE_SECONDS },
);

export async function GET() {
  return createDownloadResponse(getReleaseDownload);
}
