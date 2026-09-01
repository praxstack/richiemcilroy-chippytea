import { existsSync } from "node:fs";
import { join } from "node:path";
import { ShopSign, Underlined, Dash, Rule } from "@/components/art";
import { InkBox } from "@/components/InkBox";
import { DemoPanel } from "@/components/DemoPanel";
import { MusicPlayer } from "@/components/MusicPlayer";

const GITHUB = "https://github.com/richiemcilroy/chippytea";
const BUILD = `${GITHUB}#build-and-run`;
const hasSoundtrack = existsSync(join(process.cwd(), "public/save-your-mac-with-chippytea.mp3"));

// The app's own "How chips work" slip, word for word.
const slipLines: { text: string; seed: number }[] = [
  { text: "Clean something up for good and the freed space is measured conservatively, then credited.", seed: 211 },
  { text: "Every 100 MB credited earns one chip. Anything smaller is scraps, carried forward, never lost.", seed: 213 },
  { text: "A thousand chips is a battered fish: the full supper. The counter shows both.", seed: 214 },
  { text: "Moving files to Trash earns no chips; nothing is freed until Trash empties.", seed: 215 },
  { text: "Chips stay on your Mac and have no monetary value. They’re just your tea.", seed: 217 },
];

export default function Page() {
  return (
    <div className="mx-auto max-w-[1040px] px-7">
      <header className="flex items-center justify-between pb-1.5 pt-7">
        <ShopSign uid="hd" fishHeight={28} wordHeight={24} />
        <a
          className="text-[13px] font-medium text-ink/80 no-underline hover:text-ink hover:underline hover:decoration-gold hover:decoration-2 hover:underline-offset-4"
          href={GITHUB}
        >
          GitHub
        </a>
      </header>

      <main>
        <section className="mt-6 grid grid-cols-[minmax(0,1fr)] items-center gap-11 min-[900px]:mt-10 min-[900px]:grid-cols-[minmax(0,1fr)_404px] min-[900px]:gap-14">
          <div>
            <h1 className="mb-[18px] text-[clamp(34px,4.6vw,46px)] font-bold leading-[1.14] tracking-[-0.01em]">
              Room on your Mac,
              <br />
              <Underlined>chips in the paper.</Underlined>
            </h1>
            <p className="max-w-[46ch] text-ink/88">
              Chippytea sits in your menu bar and looks for stale build folders and old installers.
              You decide what you no longer need. It shows you what will happen
              before anything moves, and the space you free comes back as chips: one for every 100&nbsp;MB,
              counted honestly.
            </p>
            <div className="mt-[26px]">
              <InkBox variant="primary" href={BUILD} seed={31} className="px-6 py-3 text-[15px]">
                Build from source
              </InkBox>
              <div className="mt-3 flex flex-col gap-[3px] text-[12.5px] text-ink-soft">
                <span>Free &amp; open source &middot; Apple Silicon &middot; macOS 14 or later</span>
                <span>No account, no telemetry, nothing deleted on its own.</span>
              </div>
            </div>
          </div>

          <div className="flex w-full flex-col items-center">
            <DemoPanel />
            <p className="mx-auto mt-3.5 max-w-[350px] text-center text-xs leading-[1.55] text-ink-soft">
              An illustrated demo with example files and successful cleanup results.
              Try the chips, or move the installer to Trash. Nothing of yours is touched.
            </p>
          </div>
        </section>

        <section className="mt-[72px] flex justify-center">
          <InkBox as="div" variant="card" seed={3} radius={12} className="max-w-[600px] px-[22px] pb-4 pt-[18px] drop-shadow-[0_1.5px_1.5px_rgba(51,48,43,0.1)]">
            <h2 className="mb-2.5 text-sm font-semibold">How chips work</h2>
            {slipLines.map((line) => (
              <p className="mb-[7px] mt-0 flex items-start gap-2 text-[12.5px] leading-[1.55] text-ink-soft" key={line.seed}>
                <Dash seed={line.seed} className="mt-[7px] shrink-0" />
                <span>{line.text}</span>
              </p>
            ))}
          </InkBox>
        </section>
      </main>

      <footer className="mt-14 pb-24 min-[900px]:pb-11">
        <Rule />
        <div className="mt-4 flex flex-wrap items-baseline justify-between gap-4 text-xs text-ink-soft">
          <span>Native SwiftUI up front, Rust underneath. Free, MIT licensed.</span>
          <a
            className="text-ink/80 hover:text-ink hover:decoration-gold hover:decoration-2 hover:underline-offset-4"
            href={GITHUB}
          >
            Source on GitHub
          </a>
        </div>
      </footer>

      {hasSoundtrack ? <MusicPlayer /> : null}
    </div>
  );
}
