#!/usr/bin/env bash
# Re-record the Gitleaks 8.18.4 reports of the `neighbours/` scenario from the
# real binary: secrets whose match touches a byte that is not a delimiter.
#
#   record-neighbours.sh <gitleaks-8.18.4-binary> <empty-scratch-dir>
#
# Gitleaks' generic rule starts its match at the keyword, so in `MY_API_KEY`
# the byte before the match is `_`; in `"…".strip()` the byte after it is
# `.`. Each secret is written in C1 on a line that is not the first of the
# file (the late column reading), then edited alone on its line — the first
# line of its `-U0` hunk (the correct reading) — `MY_API_KEY` in C2, `TOKEN`
# in C3. The tests rebuild the repository (`neighbours_repo` in
# logos-core/src/history/security/identity_tests.rs) commit for commit.
#
#   neighbours/history.sarif   full scan (git history) at C3
#   neighbours/scoped.sarif    scoped run (`--no-git --source conf`) at C3
#
# Never run by a test (Sprint 106 Decision 40): the tests replay the files.
set -euo pipefail

if [ "$#" -ne 2 ]; then
    echo "usage: $0 <gitleaks-binary> <empty-scratch-dir>" >&2
    exit 2
fi
GITLEAKS="$1"
SCRATCH="$2"
HERE="$(cd "$(dirname "$0")" && pwd)/neighbours"
"$GITLEAKS" version | grep -qx '8.18.4' || { echo "need Gitleaks 8.18.4" >&2; exit 2; }
mkdir -p "$HERE"

REPO="$SCRATCH/repo"
mkdir -p "$REPO/conf"
export GIT_CEILING_DIRECTORIES="$SCRATCH"
g() { git -C "$REPO" -c user.name='Fixture Dev' -c user.email=dev@example.test \
    -c commit.gpgsign=false -c maintenance.auto=false -c gc.auto=0 "$@"; }
commit() { GIT_AUTHOR_DATE="$1" GIT_COMMITTER_DATE="$1" g commit -q -m "$2"; }
scan() { # scan <out> <args…>
    local out="$1"; shift
    (cd "$REPO" && "$GITLEAKS" detect --redact --no-banner --exit-code 0 \
        --report-format sarif --report-path "$out" "$@")
}

PREFIXED='MY_API_KEY = "lgs_pr3fixed_K9wQ2xT7vR4mZ8"'
TRAILING='TOKEN = "lgs_tr4iling_P5nW8qL3kD6sJ2".strip()'

g init -q -b main
printf 'import os\n\n%s\n%s\n' "$PREFIXED" "$TRAILING" > "$REPO/conf/settings.py"
g add -A
commit '2026-10-01T09:00:00+00:00' 'C1: settings'
printf 'import os\n\n%s  # rotate me\n%s\n' "$PREFIXED" "$TRAILING" > "$REPO/conf/settings.py"
g add -A
commit '2026-10-02T09:00:00+00:00' 'C2: note the key rotation'
printf 'import os\n\n%s  # rotate me\n%s  # and me\n' "$PREFIXED" "$TRAILING" > "$REPO/conf/settings.py"
g add -A
commit '2026-10-03T09:00:00+00:00' 'C3: note the token rotation'
scan "$HERE/scoped.sarif" --no-git --source conf
scan "$HERE/history.sarif" --source .

g log --format='%H %s'
