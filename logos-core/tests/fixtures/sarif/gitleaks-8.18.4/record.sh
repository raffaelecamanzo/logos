#!/usr/bin/env bash
# Re-record the Gitleaks 8.18.4 reports in this directory from the real binary.
#
#   record.sh <gitleaks-8.18.4-binary> <empty-scratch-dir>
#
# Builds the fixture repository the tests rebuild (`gitleaks_repo` in
# logos-core/src/history/security/identity_tests.rs) — same files, identity
# and dates, so the commit SHAs in the reports are the ones the test repository
# gets — and runs Gitleaks with the registry's own arguments (`--redact`):
#
#   history.sarif     full scan (git history) at C3
#   scoped-c1.sarif   scoped run (`--no-git --source tests/fixtures`) at C1
#   scoped-c3.sarif   scoped run at C3
#
# Never run by a test (Sprint 106 Decision 40): the tests replay the files.
set -euo pipefail

if [ "$#" -ne 2 ]; then
    echo "usage: $0 <gitleaks-binary> <empty-scratch-dir>" >&2
    exit 2
fi
GITLEAKS="$1"
SCRATCH="$2"
HERE="$(cd "$(dirname "$0")" && pwd)"
"$GITLEAKS" version | grep -qx '8.18.4' || { echo "need Gitleaks 8.18.4" >&2; exit 2; }

REPO="$SCRATCH/repo"
mkdir -p "$REPO"
export GIT_CEILING_DIRECTORIES="$SCRATCH"
g() { git -C "$REPO" -c user.name='Fixture Dev' -c user.email=dev@example.test \
    -c commit.gpgsign=false -c maintenance.auto=false -c gc.auto=0 "$@"; }
commit() { GIT_AUTHOR_DATE="$1" GIT_COMMITTER_DATE="$1" g commit -q -m "$2"; }

KEY='API_KEY = "lgs_pl4nted_S3cr3t_q7Vx9Rz2Kw"'
CONSTANTS='X_1 = 1
X_2 = 2
X_3 = 3
X_4 = 4
X_5 = 5
X_6 = 6
X_7 = 7
X_8 = 8
X_9 = 9
X_10 = 10'

scan() { # scan <out> <args…>
    local out="$1"; shift
    (cd "$REPO" && "$GITLEAKS" detect --redact --no-banner --exit-code 0 \
        --report-format sarif --report-path "$out" "$@")
}

g init -q -b main
mkdir -p "$REPO/app" "$REPO/tests/fixtures" "$REPO/legacy"
printf 'print("hello")\n' > "$REPO/app/main.py"
printf 'import os\n\n%s\n%s\n' "$KEY" "$CONSTANTS" > "$REPO/tests/fixtures/settings.py"
printf 'CLIENT_TOKEN = "lgs_l3gacy_R8wY2pQ5nM4kT7"\n' > "$REPO/legacy/client.py"
g add -A
commit '2026-10-01T09:00:00+00:00' 'C1: add the api client'
scan "$HERE/scoped-c1.sarif" --no-git --source tests/fixtures

printf 'import os\n\n%s  # rotate me\n%s\n' "$KEY" "$CONSTANTS" > "$REPO/tests/fixtures/settings.py"
g rm -q legacy/client.py
g add -A
commit '2026-10-02T09:00:00+00:00' 'C2: note the rotation'

printf 'import os\n\n%s\n%s  # rotate me\n' "$CONSTANTS" "$KEY" > "$REPO/tests/fixtures/settings.py"
g add -A
commit '2026-10-03T09:00:00+00:00' 'C3: move the key below the constants'
scan "$HERE/scoped-c3.sarif" --no-git --source tests/fixtures
scan "$HERE/history.sarif" --source .

g log --format='%H %s'
