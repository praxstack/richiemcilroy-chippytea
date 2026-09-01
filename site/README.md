# The chippytea landing page

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

The **Download for Mac** button calls `/download`. It checks the latest stable GitHub release and redirects only to its exact uploaded, nonempty `chippytea-<version>-universal.dmg`. It rejects drafts, prereleases, duplicate installers, and unexpected download URLs. Both valid links and unavailable results are cached server-side, with a five-minute revalidation interval. A previous result may be served while the cache refreshes. GitHub requests time out after eight seconds.

If no valid installer is available, the route returns a `503` page with retry, release, and source-build links. The homepage also keeps a **Build from source** link. A working website does not by itself mean a signed, notarized app has been published.

Production hosting needs a Next.js runtime for `/download`, not a static-only export. Build from a fresh checkout or reviewed public-source snapshot, without local `.env`, `.next`, or `.vercel` files. Include the committed soundtrack and root `LICENSE` when packaging a site-only snapshot.

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

## Soundtrack and karaoke

The project-owned **Save Your Mac with chippytea** soundtrack is committed at `public/save-your-mac-with-chippytea.mp3` and published with the maintainer's permission. Fresh checkouts and production deployments include it. The **Play the chippytea song** slip appears in the hero when that file exists at build time.

Pressing the slip opens the karaoke (`components/Karaoke.tsx`): the lyric a line at a time with each word wiped gold as it is sung, a chip bouncing over the words, fish and chips behind (`lib/karaokeScene.ts`), a hand-lettered chip count, and a chip thrown into the wrap wherever the page is tapped. Escape or the drawn cross fades it out, song included; opening it again starts from the top.

Word timings live in `lib/lyrics.ts`, generated from the lyric sheet in `scripts/save-your-mac-with-chippytea.lyrics.txt` and a Whisper word-level transcript of the recording:

```sh
whisper public/save-your-mac-with-chippytea.mp3 --model base.en --language en \
  --word_timestamps True --output_format json --output_dir /tmp/whisper
python3 scripts/align-lyrics.py /tmp/whisper/save-your-mac-with-chippytea.json
```

Edit the sheet, not the generated file, if the words change.

Permission to publish this soundtrack does not grant rights to independently supplied replacements. Check ownership and remove private metadata before publishing another recording. Original code and drawing paths use the root [MIT license](../LICENSE); the soundtrack's rights are separate from that code license.
