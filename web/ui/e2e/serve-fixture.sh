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
    # The one coverage tool the copy has (S-803): a custom `refresh_cmd` that fails
    # and names the failing test, so coverage.project.spec.ts can run it from the
    # Coverage page and read the cause. It runs through `sh -c` (linked into the
    # scannerless PATH below) and uses shell builtins only. Written after the
    # commit, so the copy's history is the fixture's.
    mkdir -p "$RUN/.logos"
    cat >>"$RUN/.logos/config.toml" <<'TOML'
[coverage_ingest]
refresh_cmd = "echo 'running 1 test'; echo 'FAIL tests::checkout_totals_include_tax: expected 42, got 41' >&2; exit 3"
TOML
    "$BIN" --project "$RUN" index --quiet
    "$BIN" --project "$RUN" hotspots --quiet >/dev/null
    # A stored security report (S-670), so the Security page renders findings in
    # graph context rather than only its no-scan state. Ingesting reads and parses
    # the report; no scanner runs.
    "$BIN" --project "$RUN" security ingest "$HERE/fixtures/single.security.sarif" >/dev/null
    # An OSV-Scanner report over the copy's `web/package-lock.json` (S-810): its one
    # finding is on a package the lockfile marks `"dev": true`.
    "$BIN" --project "$RUN" security ingest "$HERE/fixtures/single.dev.security.sarif" >/dev/null
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
    # A stored security report in `api` only (S-674), so Workspace security
    # renders a scanned member beside a never-scanned one (`web`). Ingesting
    # reads and parses the report; no scanner runs.
    "$BIN" --project "$RUN/api" security ingest "$HERE/fixtures/workspace.security.sarif" >/dev/null
fi

# The security surfaces look for scanners on PATH, in the managed-tools directory
# and under the user config directory. Serve with all three pointed away from this
# machine's, so the Security page (S-670) reads only the fixture's stored report
# and starts no version probe of a scanner installed here: a first run of a large
# scanner binary can outlast a page's ready wait, and no spec may depend on what
# the host has installed. PATH keeps git alone, which serve needs, and `sh`, which
# the fixture's coverage command runs through.
HOST_GIT="$(command -v git)"
SCANNERLESS="$HERE/.run/$KIND-scannerless"
rm -rf "$SCANNERLESS"
mkdir -p "$SCANNERLESS/bin"
ln -s "$HOST_GIT" "$SCANNERLESS/bin/git"
# The coverage run's custom command goes through `sh -c` (S-803).
ln -s /bin/sh "$SCANNERLESS/bin/sh"
exec env PATH="$SCANNERLESS/bin" LOGOS_CONFIG_HOME="$SCANNERLESS/config" LOGOS_DATA_HOME="$SCANNERLESS/tools" \
    "$BIN" serve --ui --port "$PORT" --project "$RUN"
