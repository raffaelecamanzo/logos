#!/usr/bin/env bash
#
# serve-fixture.sh — serve one checked-in fixture project through a TREE-BUILT
# `logos serve --ui`, for the Playwright browser tests (S-611, CR-203 §11).
# Playwright's `webServer` runs this; it is not meant to be run by hand, though
# it can be, to look at a fixture in a browser.
#
#   bash e2e/serve-fixture.sh single    <port>   # one repository
#   bash e2e/serve-fixture.sh workspace <port>   # a parent of two member repos
#
# The fixture is COPIED into e2e/.run/<kind>/ (gitignored) and each repository
# there is made a git repository, so the checked-in sources are never indexed in
# place, no `.logos/` store is ever written under them, and the project root
# logos resolves is the copy — never this repository.
#
# The binary is $LOGOS_E2E_BIN, else the tree's debug build. It must embed a
# real SPA build: `npm run build` in web/ui, THEN `cargo build -p logos`.
# scripts/gate.sh's ui-e2e leg does both. A missing binary exits 2 rather than
# falling back to whatever `logos` is on PATH, which is not this tree's code.

set -euo pipefail

KIND="${1:-}"
PORT="${2:-}"
case "$KIND" in
    single | workspace) ;;
    *)
        echo "usage: serve-fixture.sh {single|workspace} <port>" >&2
        exit 2
        ;;
esac
if [ -z "$PORT" ]; then
    echo "serve-fixture.sh: a port is required" >&2
    exit 2
fi

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(git -C "$HERE" rev-parse --show-toplevel)"
BIN="${LOGOS_E2E_BIN:-${CARGO_TARGET_DIR:-$ROOT/target}/debug/logos}"
if [ ! -x "$BIN" ]; then
    echo "serve-fixture.sh: no logos binary at $BIN" >&2
    echo "  build it from this tree: (cd web/ui && npm run build) && cargo build -p logos" >&2
    exit 2
fi

RUN="$HERE/.run/$KIND"
rm -rf "$RUN"
mkdir -p "$RUN"
# logos resolves a project root with `git rev-parse --show-toplevel`. The
# workspace parent is deliberately NOT a git repository, so without a ceiling
# that lookup climbs out of .run/ into THIS repository, and `init --workspace`
# and `serve` then act on the logos checkout itself. The ceiling stops git at
# .run/, so a non-repository resolves to itself.
export GIT_CEILING_DIRECTORIES="$HERE/.run"
cp -R "$HERE/fixtures/$KIND/." "$RUN/"

git_repo() { # make <dir> a one-commit git repository
    git -C "$1" -c init.defaultBranch=main init -q
    git -C "$1" add -A
    git -C "$1" -c user.email=e2e@logos -c user.name=logos-e2e -c commit.gpgsign=false \
        -c maintenance.auto=false -c gc.auto=0 commit -q -m fixture
}

# Fail closed BEFORE logos runs: the root it will resolve (git's toplevel for
# the copy) must be the copy itself for a repository, and must not exist for
# the workspace parent. Anything else means the lookup would land outside .run/.
expect_root() { # expect_root <wanted-toplevel-or-empty>
    local top
    top="$(git -C "$RUN" rev-parse --show-toplevel 2>/dev/null || true)"
    if [ "$top" != "$1" ]; then
        echo "serve-fixture.sh: $RUN resolves to project root '${top:-<none>}', not '${1:-<none>}'; refusing to run logos" >&2
        exit 2
    fi
}

if [ "$KIND" = single ]; then
    git_repo "$RUN"
    expect_root "$(cd "$RUN" && pwd -P)"
    # The fixture's later commits (more authors, a fix), so Files & Risk has
    # churn to rank and ownership to disperse; then rank it, which mines that
    # history — the files view reads the ranking, it never mines on a GET.
    bash "$HERE/fixtures/single.history.sh" "$RUN"
    "$BIN" --project "$RUN" index --quiet
    "$BIN" --project "$RUN" hotspots --quiet >/dev/null
else
    for member in "$RUN"/*/; do
        git_repo "${member%/}"
    done
    expect_root ""
    "$BIN" --project "$RUN" init --workspace --yes --quiet
    # Backstop: a manifest anywhere but the copy means the root escaped anyway.
    if [ ! -f "$RUN/logos.workspace.toml" ]; then
        echo "serve-fixture.sh: init --workspace wrote no manifest in $RUN; refusing to serve" >&2
        exit 2
    fi
fi

exec "$BIN" serve --ui --port "$PORT" --project "$RUN"
