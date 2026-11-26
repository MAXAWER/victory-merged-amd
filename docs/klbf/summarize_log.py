"""Summarize KLBF timing samples for a gameplay report (Python 3).

    py -3 docs/klbf/summarize_log.py run.log --lines 1200 2500

Choose inclusive line numbers around one scene, or pass a log already trimmed
to that scene. The output gives medians of logged samples, not per-frame lows.
"""

import argparse
import codecs
import io
import re
from pathlib import Path
from statistics import median


NUMBER = r"(\d+(?:\.\d+)?)"
HOST = re.compile(
    rf"{NUMBER} fps, {NUMBER} ms/frame \({NUMBER} preparing:"
)
FRAMEGEN = re.compile(
    rf"ffx: frame generation: {NUMBER} real fps, {NUMBER} fps presented"
)
STATE = re.compile(r"\bon=(true|false)\b")
ANSI = re.compile(r"\x1b\[[0-9;]*m")


def summarize(lines, first, last):
    host = []
    framegen = {}
    for number, line in enumerate(lines, 1):
        if last is not None and number > last:
            break
        if number < first:
            continue
        line = ANSI.sub("", line)
        match = HOST.search(line)
        if match:
            host.append(tuple(map(float, match.groups())))
            continue
        match = FRAMEGEN.search(line)
        if match:
            state = STATE.search(line)
            key = state[1] if state else "unspecified"
            framegen.setdefault(key, []).append(tuple(map(float, match.groups())))

    report = []
    if host:
        fps, ms, cpu = map(median, zip(*host))
        report.append(f"Host timing ({len(host)} samples, medians): {fps:.1f} FPS, "
                      f"{ms:.1f} ms/frame, {cpu:.1f} ms/frame preparing on CPU")
    for key, samples in framegen.items():
        real, presented = map(median, zip(*samples))
        report.append(f"Frame generation on={key} ({len(samples)} samples, medians): "
                      f"{real:.1f} real FPS, {presented:.1f} presented FPS")
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("log", type=Path)
    parser.add_argument("--lines", nargs=2, type=int, metavar=("FIRST", "LAST"),
                        help="inclusive 1-based line range for a scene")
    args = parser.parse_args()
    first, last = args.lines or (1, None)
    if first < 1 or (last is not None and last < first):
        parser.error("expected 1 <= FIRST <= LAST")
    try:
        with args.log.open("rb") as raw:
            prefix = raw.peek(2)[:2]
            encoding = "utf-16" if prefix in (codecs.BOM_UTF16_LE, codecs.BOM_UTF16_BE) else "utf-8-sig"
            with io.TextIOWrapper(raw, encoding=encoding, errors="replace") as lines:
                report = summarize(lines, first, last)
    except OSError as error:
        parser.exit(1, f"{error}\n")
    if not report:
        parser.exit(1, "No KLBF timing samples found in the selected lines.\n")
    print("\n".join(report))


if __name__ == "__main__":
    main()
