#!/usr/bin/env bash
#
# single.history.sh — the git history of the `single` fixture after its first
# commit (S-616). serve-fixture.sh runs it inside the copy, so Files & Risk has
# churn to rank and two authors to disperse ownership over: the two files under
# the long `analysis/…/scoped/` path change together, by a second author, and
# one of them takes a fix commit (the Defect heuristic).
#
#   bash e2e/fixtures/single.history.sh <repo>

set -euo pipefail

REPO="${1:?usage: single.history.sh <repo>}"
SCOPED="src/analysis/structural/resolution/scoped"

commit_as() { # commit_as <name> <email> <subject> <file>...
    local name="$1" email="$2" subject="$3"
    shift 3
    git -C "$REPO" add -- "$@"
    git -C "$REPO" -c user.email="$email" -c user.name="$name" -c commit.gpgsign=false \
        -c maintenance.auto=false -c gc.auto=0 commit -q -m "$subject"
}

printf '// Revised: names keep their first eight characters per part.\n' >>"$REPO/$SCOPED/binder.rs"
printf '// Revised: a leading underscore marks a private name.\n' >>"$REPO/$SCOPED/lookup.rs"
commit_as "Ada Second" ada@e2e.logos "Refine the scoped lookup and binder" "$SCOPED/binder.rs" "$SCOPED/lookup.rs"

printf '// Fixed: an empty name binds to a placeholder.\n' >>"$REPO/$SCOPED/binder.rs"
commit_as logos-e2e e2e@logos "fix: bind an empty name" "$SCOPED/binder.rs"

printf '// Revised: lookup and binder change together again.\n' | tee -a "$REPO/$SCOPED/binder.rs" >>"$REPO/$SCOPED/lookup.rs"
commit_as "Ada Second" ada@e2e.logos "Keep lookup and binder in step" "$SCOPED/binder.rs" "$SCOPED/lookup.rs"
