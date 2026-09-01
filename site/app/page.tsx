import { existsSync } from "node:fs";
import { join } from "node:path";
import { ShopSign, Underlined, Dash, Rule } from "@/components/art";
import { InkBox } from "@/components/InkBox";
import { DemoPanel } from "@/components/DemoPanel";
import { MusicPlayer } from "@/components/MusicPlayer";

const GITHUB = "https://github.com/richiemcilroy/chippytea";
const BUILD = `${GITHUB}#build-and-run`;
const hasSoundtrack = existsSync(join(process.cwd(), "public/save-your-mac-with-chippytea.mp3"));

const cleanupSteps: { text: string; seed: number }[] = [
  { text: "Scan folders you choose for old build files, dependencies and installers.", seed: 211 },
  { text: "Review each suggestion, its estimated size and what removing it would mean.", seed: 213 },
  { text: "Move files to Trash, or choose permanent removal for eligible build files and dependencies. Downloads and installers are Trash-only.", seed: 214 },
  { text: "Review the outcome in the cleanup history. Files in Trash still take up space.", seed: 215 },
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
              Free up space
              <br />
              <Underlined>on your Mac.</Underlined>
            </h1>
            <p className="max-w-[46ch] text-ink/88">
              Chippytea is an ultra-performant, native macOS app that clears space on your Mac.
              Built with SwiftUI and Rust, it helps you find old build folders, project dependencies
              and installers you may no longer need. Review their estimated sizes, see what removing
              them means, and choose what to keep or remove.
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
              An illustrated demo using fictional files and example cleanup results.
              Select an item to review it. Nothing on your Mac is scanned or changed.
            </p>
          </div>
        </section>

        <section className="mt-[72px] flex justify-center">
          <InkBox as="div" variant="card" seed={3} radius={12} className="max-w-[600px] px-[22px] pb-4 pt-[18px] drop-shadow-[0_1.5px_1.5px_rgba(51,48,43,0.1)]">
            <h2 className="mb-2.5 text-sm font-semibold">How it works</h2>
            {cleanupSteps.map((line) => (
              <p className="mb-[7px] mt-0 flex items-start gap-2 text-[12.5px] leading-[1.55] text-ink-soft" key={line.seed}>
                <Dash seed={line.seed} className="mt-[7px] shrink-0" />
                <span>{line.text}</span>
              </p>
            ))}
            <details className="mt-4 text-[12.5px] leading-[1.55] text-ink-soft">
              <summary className="cursor-pointer font-medium text-ink">About the chip counter</summary>
              <p className="mb-[7px] mt-2">
                The fish and chips are a decorative counter for credited cleanup.
                Eligible permanent cleanup adds one chip per 100&nbsp;MB of conservatively
                credited space. Smaller amounts carry forward, and a thousand chips is shown as a fish.
              </p>
              <p className="m-0">
                Chips stay on your Mac and have no monetary value. Moving files to Trash
                adds no chips; estimated sizes never update the counter.
              </p>
            </details>
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
