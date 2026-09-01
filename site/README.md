# The Chippytea landing page

Next.js 16 on Bun. The page uses the app's paper, ink, and hand-drawn fish and chips. It needs no API keys, downloaded fonts, or tracking scripts.

## Run locally

```sh
cd site
bun install --frozen-lockfile
bun run dev
```

`bun run build` checks the production build. `bun run start` serves it locally.

With Node 22.22 or newer, run `node --test tests/download.test.mjs` to check release selection and failure handling. CI runs these tests before building the site.

Dependency updates are currently manual. As of 1 September 2026, Dependabot cannot read the version 2 `bun.lock` written by Bun 1.4. Keep the lockfile committed and verify updates with a frozen install and production build.

## Download routing

The **Download for Mac** button calls `/download`. It checks the latest stable GitHub release and redirects only to its exact uploaded, nonempty `Chippytea-<version>-universal.dmg`. It rejects drafts, prereleases, duplicate installers, and unexpected download URLs. Both valid links and unavailable results are cached server-side, with a five-minute revalidation interval. A previous result may be served while the cache refreshes. GitHub requests time out after eight seconds.

If no valid installer is available, the route returns a `503` page with retry, release, and source-build links. The homepage also keeps a **Build from source** link. A working website does not by itself mean a signed, notarized app has been published.

Production hosting needs a Next.js runtime for `/download`, not a static-only export. Build from a fresh checkout or reviewed public-source snapshot, without local `.env`, `.next`, `.vercel`, or soundtrack files. Include the root `LICENSE` when packaging a site-only snapshot.

## The illustrated demo

The demo runs entirely in memory with fictional files and successful example outcomes. It does not scan your Mac or remove anything. It shows supported developer artifacts, while downloads are Trash-only and earn no chips. It is an illustration of the interaction, not evidence of measured storage recovery.

The native policy lives in [SUGGESTIONS.md](../docs/SUGGESTIONS.md).

## The same drawing, in three places

- `lib/ink.ts` and `lib/art.ts` share the native app's seeded paths and letterforms.
- `lib/draw.ts` paints the chips, paper, and balance on canvas.
- `components/DemoPanel.tsx` handles the interactive example.
- `lib/chime.ts` synthesizes the short collection sound.
- `bun scripts/gen-icon.ts` regenerates the fish favicon.
- `bun ../scripts/generate-readme-art.ts` regenerates the repository's light and dark SVGs.

The landing page keeps its cream-paper theme. The README has separate light and dark artwork. Both respect reduced-motion preferences.

## Optional soundtrack

The shop radio appears only when `public/save-your-mac-with-chippytea.mp3` exists at build time. That local soundtrack is ignored by Git while redistribution rights are unconfirmed; a fresh checkout works without it.

Do not publish a replacement track without permission to redistribute it. Remove private metadata and document its license first. Original code and drawing paths use the root [MIT license](../LICENSE); that does not establish rights to an independently supplied recording.
