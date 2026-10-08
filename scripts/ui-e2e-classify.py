#!/usr/bin/env python3
"""ui-e2e-classify.py — the verdict of scripts/gate.sh's `ui-e2e` leg (S-611).

Reads Playwright's JSON report, the run's exit status and its log, and prints ONE
line for gate.sh to `read`:

    <verdict> <truncation> <listed> <ran> <passed> <failed> <skipped>

`listed` is every test Playwright reported; `ran` is those that executed
(passed + failed). gate.sh records them as the unit's expected / started /
finished, so scripts/verify-evidence.sh re-derives the same verdict from them.

Every way the leg could read green while checking nothing is a failure:
  missing_browser  the log says the browser executable is absent
  killed           the run hit the wall-clock watchdog (exit 142, SIGALRM)
  no_binaries      no report, an unreadable or oddly shaped report, or nothing ran
  skipped          a test was listed but skipped: a layout check not run
A failed test, or a non-zero exit with a clean report, is a plain fail.

Exit status: 0 whenever it printed a line (the verdict is the line, not the
status); 2 on a usage error.
"""

import argparse
import json
import sys

MISSING_BROWSER = ("Executable doesn't exist", "npx playwright install")
SIGALRM_EXIT = 128 + 14


def counts(report_path):
    """(passed, failed, skipped) from the report, or None when unusable."""
    try:
        with open(report_path) as fh:
            stats = json.load(fh).get("stats")
        passed = int(stats["expected"])
        failed = int(stats.get("unexpected", 0)) + int(stats.get("flaky", 0))
        skipped = int(stats.get("skipped", 0))
    except (OSError, ValueError, TypeError, KeyError, AttributeError):
        return None
    if min(passed, failed, skipped) < 0:
        return None
    return passed, failed, skipped


def classify(report_path, rc, log_text):
    got = counts(report_path)
    passed, failed, skipped = got if got else (0, 0, 0)
    listed, ran = passed + failed + skipped, passed + failed

    if any(marker in log_text for marker in MISSING_BROWSER):
        trunc = "missing_browser"
    elif rc == SIGALRM_EXIT:
        trunc = "killed"
    elif got is None or ran == 0:
        trunc = "no_binaries"
    elif skipped > 0:
        trunc = "skipped"
    else:
        trunc = "none"

    verdict = "pass" if trunc == "none" and failed == 0 and rc == 0 else "fail"
    return verdict, trunc, listed, ran, passed, failed, skipped


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("report", help="Playwright JSON report (may be absent)")
    ap.add_argument("rc", type=int, help="exit status of the Playwright run")
    ap.add_argument("log", help="the run's log file (may be absent)")
    args = ap.parse_args(argv)
    try:
        with open(args.log, errors="replace") as fh:
            log_text = fh.read()
    except OSError:
        log_text = ""
    print(" ".join(str(v) for v in classify(args.report, args.rc, log_text)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
