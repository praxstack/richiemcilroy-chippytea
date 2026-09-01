#!/usr/bin/env python3
"""Time the lyric sheet against the recording and write lib/lyrics.ts.

    whisper public/save-your-mac-with-chippytea.mp3 --model small.en \
        --language en --word_timestamps True --output_format json --output_dir /tmp/w
    python3 scripts/align-lyrics.py /tmp/w/save-your-mac-with-chippytea.json

Whisper hears the song approximately ("Chippity", "17", "a space for free");
the lyric sheet is what is actually sung. This aligns the two word streams,
takes Whisper's timings for every matched word, and spreads unmatched words
evenly between their timed neighbours. Only the standard library is needed.
"""

from __future__ import annotations

import json
import re
import sys
from difflib import SequenceMatcher
from pathlib import Path

HERE = Path(__file__).resolve().parent
SHEET = HERE / "save-your-mac-with-chippytea.lyrics.txt"
OUT = HERE.parent / "lib" / "lyrics.ts"

# Whisper's spellings of things it cannot know.
ALIASES = {
    "chippytea": "chippity",
    "chippyteaaaa": "chippity",
    "17": "seventeen",
    "2": "two",
    "ohhhh": "oh",
    "ohh": "oh",
    "whoa": "oh",
    "oi": "i",
}
MIN_WORD = 0.12  # seconds a sung word is allowed to take at the very least


def key(word: str) -> str:
    cleaned = re.sub(r"[^a-z0-9]", "", word.lower())
    return ALIASES.get(cleaned, cleaned)


def read_sheet(path: Path):
    lines = []
    section = ""
    for raw in path.read_text().splitlines():
        text = raw.strip()
        if not text or text.startswith("#"):
            continue
        if text.startswith("[") and text.endswith("]"):
            section = text[1:-1].strip()
            continue
        lines.append({"section": section, "words": text.split()})
    return lines


def read_whisper(path: Path):
    data = json.loads(path.read_text())
    words = []
    for segment in data["segments"]:
        for w in segment.get("words", []):
            k = key(w["word"])
            if k:
                words.append({"key": k, "start": float(w["start"]), "end": float(w["end"])})
    return words


def similarity(a: str, b: str) -> float:
    if a == b:
        return 1.0
    return SequenceMatcher(None, a, b).ratio()


def align(sheet_keys, heard):
    """Best-scoring monotone alignment. A run of up to three sheet words may
    match a run of up to three heard words ("chippy tea" <-> "Chippity")."""
    n, m = len(sheet_keys), len(heard)
    NEG = float("-inf")
    best = [[NEG] * (m + 1) for _ in range(n + 1)]
    back: list[list[tuple | None]] = [[None] * (m + 1) for _ in range(n + 1)]
    best[0][0] = 0.0
    for i in range(n + 1):
        for j in range(m + 1):
            here = best[i][j]
            if here == NEG:
                continue
            if i < n and here - 0.6 > best[i + 1][j]:
                best[i + 1][j] = here - 0.6
                back[i + 1][j] = (i, j, "skip-sheet")
            if j < m and here - 0.4 > best[i][j + 1]:
                best[i][j + 1] = here - 0.4
                back[i][j + 1] = (i, j, "skip-heard")
            for a in (1, 2, 3):
                if i + a > n:
                    break
                sheet_run = "".join(sheet_keys[i : i + a])
                sheet_run = ALIASES.get(sheet_run, sheet_run)
                for b in (1, 2, 3):
                    if j + b > m or (a > 1 and b > 1):
                        continue
                    heard_run = "".join(w["key"] for w in heard[j : j + b])
                    heard_run = ALIASES.get(heard_run, heard_run)
                    score = similarity(sheet_run, heard_run)
                    if score < 0.72:
                        continue
                    gain = here + score * (a + b) / 2
                    if gain > best[i + a][j + b]:
                        best[i + a][j + b] = gain
                        back[i + a][j + b] = (i, j, "match")
    # Backtrack.
    groups = {}  # sheet index -> (start, end, run length, position in run)
    i, j = n, m
    while (i, j) != (0, 0):
        pi, pj, kind = back[i][j]
        if kind == "match":
            start = heard[pj]["start"]
            end = heard[j - 1]["end"]
            for offset in range(i - pi):
                groups[pi + offset] = (start, end, i - pi, offset)
        i, j = pi, pj
    return groups


def main(whisper_json: Path):
    lines = read_sheet(SHEET)
    heard = read_whisper(whisper_json)
    flat = [(li, wi, word) for li, line in enumerate(lines) for wi, word in enumerate(line["words"])]
    keys = [key(word) for _, _, word in flat]
    groups = align(keys, heard)

    # Words matched as a run share the run's time, split by letter count.
    times: list[list[float] | None] = [None] * len(flat)
    for index, (li, wi, word) in enumerate(flat):
        if index not in groups:
            continue
        start, end, run, offset = groups[index]
        run_words = [flat[index - offset + k][2] for k in range(run)]
        weights = [max(1, len(key(w))) for w in run_words]
        total = sum(weights)
        before = sum(weights[:offset])
        times[index] = [start + (end - start) * before / total, start + (end - start) * (before + weights[offset]) / total]

    # Unmatched words are spread between their timed neighbours.
    matched = [i for i, t in enumerate(times) if t is not None]
    if not matched:
        sys.exit("nothing aligned; is that the right transcript?")
    unmatched = [i for i, t in enumerate(times) if t is None]
    for index in unmatched:
        prev = max((i for i in matched if i < index), default=None)
        nxt = min((i for i in matched if i > index), default=None)
        lo = times[prev][1] if prev is not None else 0.0
        hi = times[nxt][0] if nxt is not None else lo + 2.0
        span_from = (prev + 1) if prev is not None else 0
        span_to = nxt if nxt is not None else len(flat)
        count = span_to - span_from
        slot = index - span_from
        width = max(hi - lo, MIN_WORD * count)
        times[index] = [lo + width * slot / count, lo + width * (slot + 1) / count]

    # Keep every word in order and give it a little room.
    last_end = 0.0
    for index in range(len(flat)):
        start, end = times[index]
        start = max(start, last_end)
        end = max(end, start + MIN_WORD)
        times[index] = [start, end]
        last_end = start + MIN_WORD * 0.5

    out_lines = []
    for li, line in enumerate(lines):
        words = []
        for wi, word in enumerate(line["words"]):
            index = next(i for i, (a, b, _) in enumerate(flat) if a == li and b == wi)
            start, end = times[index]
            words.append({"text": word, "start": round(start, 2), "end": round(end, 2)})
        out_lines.append({"section": line["section"], "words": words})

    unmatched_words = [flat[i][2] for i in unmatched]
    body = ",\n".join(
        "  { section: %s, words: [%s] }"
        % (
            json.dumps(line["section"]),
            ", ".join("w(%s, %s, %s)" % (json.dumps(w["text"]), w["start"], w["end"]) for w in line["words"]),
        )
        for line in out_lines
    )
    OUT.write_text(
        "// Generated by scripts/align-lyrics.py from the lyric sheet and a Whisper\n"
        "// word-level transcript of public/save-your-mac-with-chippytea.mp3.\n"
        "// Times are seconds into the recording. Do not edit by hand.\n\n"
        "export interface LyricWord {\n  text: string;\n  start: number;\n  end: number;\n}\n\n"
        "export interface LyricLine {\n  section: string;\n  words: LyricWord[];\n}\n\n"
        "const w = (text: string, start: number, end: number): LyricWord => ({ text, start, end });\n\n"
        "export const lyricLines: LyricLine[] = [\n" + body + ",\n];\n"
    )
    print(f"{len(flat)} words, {len(unmatched)} interpolated: {' '.join(unmatched_words) or 'none'}")
    print(f"wrote {OUT.relative_to(HERE.parent)}")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    main(Path(sys.argv[1]))
