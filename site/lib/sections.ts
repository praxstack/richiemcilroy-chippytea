// The song's sections, as runs of consecutive lyric lines: the intro, each
// chorus, the verses, the pre-chorus and the bridge. The karaoke's DOM and its
// canvas both key their animation off these, so they are worked out once here.

import { lyricLines } from "./lyrics";

export interface SectionRun {
  /** The section name as the lyric sheet has it ("last chorus"). */
  section: string;
  /** The same as a CSS-friendly token ("last-chorus"). */
  key: string;
  /** What the stamp says when the section arrives. */
  label: string;
  /** First and last line indices of the run. */
  from: number;
  to: number;
  /** Seconds into the song the run's first word starts and its last word ends. */
  start: number;
  end: number;
}

const LABELS: Record<string, string> = {
  intro: "here we go",
  chorus: "chorus, everybody!",
  "verse 1": "verse one",
  "pre-chorus": "ready?",
  "verse 2": "verse two",
  bridge: "clap along!",
  "last chorus": "last chorus, louder!",
};

export const sectionKey = (section: string) => section.trim().toLowerCase().replace(/\s+/g, "-");

export const sectionRuns: SectionRun[] = [];
/** The run each lyric line belongs to. */
export const runOfLine: number[] = [];

for (const [index, line] of lyricLines.entries()) {
  const current = sectionRuns[sectionRuns.length - 1];
  const words = line.words;
  if (current && current.section === line.section) {
    current.to = index;
    current.end = words[words.length - 1].end;
  } else {
    sectionRuns.push({
      section: line.section,
      key: sectionKey(line.section),
      label: LABELS[line.section] ?? line.section,
      from: index,
      to: index,
      start: words[0].start,
      end: words[words.length - 1].end,
    });
  }
  runOfLine[index] = sectionRuns.length - 1;
}

/** 0…1 through a run, for the pre-chorus's build-up. */
export function progressThrough(run: SectionRun, time: number): number {
  const span = Math.max(0.1, run.end - run.start);
  return Math.max(0, Math.min(1, (time - run.start) / span));
}
