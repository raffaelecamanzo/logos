#!/usr/bin/env bash
#
# test-check-offline-cargo.sh — prove scripts/check-offline-cargo.py fails an
# offline cargo call that resolves every target platform, and only that (S-805,
# NFR-MA-11).
#
# Each case copies fixture files (scripts/tests/fixtures/offline-cargo/*.rs.fixture,
# named so no tool reads them as source) into a throwaway root and runs the check
# there. The pinned shapes, in the order the story states them:
#   - HF-12's original argument list (`metadata --offline`, no platform)   caught
#   - the filtered one (`--filter-platform <host>`)                        passes
#   - a plain `cargo tree --offline`                                       passes
#   - `cargo tree --offline --target all`                                  caught
# and the controls around them: --no-deps, the other spellings of a named platform,
# flags one character from the real ones, the builder-chain and one-string forms of
# a call, text that is not code, and the empty walk. A check that flagged every
# offline call would catch every bypass, so each "passes" case reads its
# denominator too: the site was seen and judged, not skipped.
#
# Usage: bash scripts/tests/test-check-offline-cargo.sh
# Exit:  0 all cases behaved, 1 a case did not

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CHECK="$HERE/scripts/check-offline-cargo.py"
FIXTURES="$HERE/scripts/tests/fixtures/offline-cargo"

if [ ! -f "$CHECK" ] || [ ! -d "$FIXTURES" ]; then
    echo "cannot find scripts/check-offline-cargo.py and its fixtures (looked in $HERE)" >&2
    exit 1
fi

WORK="${TMPDIR:-/tmp}"
WORK="${WORK%/}/logos-offline-cargo-test.$$"
trap 'rm -rf "$WORK"' EXIT INT TERM
mkdir -p "$WORK" || exit 1

PASS=0
FAIL=0
N=0

# root <fixture-name>... -> sets ROOT: a fresh tree holding each fixture as
# src/<name>.rs
root() {
    N=$((N + 1))
    ROOT="$WORK/root$N"
    mkdir -p "$ROOT/src"
    for name in "$@"; do
        cp "$FIXTURES/$name.rs.fixture" "$ROOT/src/$name.rs"
    done
}

# run -> sets OUT and RC
run() {
    OUT="$(python3 "$CHECK" --root "$ROOT" 2>&1)"
    RC=$?
}

# expect <name> <wanted-exit> <summary line that must appear> [<text that must appear>]...
expect() {
    local name="$1" want="$2" summary="$3" must
    shift 3
    if [ "$RC" -ne "$want" ]; then
        echo "  FAIL  $name: exit $RC, wanted $want"
        printf '%s\n' "$OUT" | sed 's/^/        /'
        FAIL=$((FAIL + 1))
        return
    fi
    if ! printf '%s\n' "$OUT" | grep -qxF -- "$summary"; then
        echo "  FAIL  $name: exit $RC correct, but no line reading '$summary'"
        printf '%s\n' "$OUT" | sed 's/^/        /'
        FAIL=$((FAIL + 1))
        return
    fi
    for must in "$@"; do
        if ! printf '%s\n' "$OUT" | grep -qF -- "$must"; then
            echo "  FAIL  $name: no output naming '$must'"
            printf '%s\n' "$OUT" | sed 's/^/        /'
            FAIL=$((FAIL + 1))
            return
        fi
    done
    echo "  ok    $name"
    PASS=$((PASS + 1))
}

echo "test-check-offline-cargo.sh"
echo "  work $WORK"
echo

# --- the shapes the story pins ---------------------------------------------------

root hf12-original
run
expect "HF-12's original argument list is caught, with file and line" 1 \
    "offline-cargo: files=1 sites=1 metadata=1 tree=0 hits=1" \
    "src/hf12-original.rs:3:"

root hf12-filtered
run
expect "the same call with --filter-platform <host> passes" 0 \
    "offline-cargo: files=1 sites=1 metadata=1 tree=0 hits=0"

root tree-plain
run
expect "a plain cargo tree --offline (the host-only tree) passes" 0 \
    "offline-cargo: files=1 sites=1 metadata=0 tree=1 hits=0"

root tree-target-all
run
expect "cargo tree --offline --target all is caught in both spellings" 1 \
    "offline-cargo: files=1 sites=2 metadata=0 tree=2 hits=2" \
    "src/tree-target-all.rs:2:" "src/tree-target-all.rs:3:"

# --- the controls ----------------------------------------------------------------

root tree-target-triple
run
expect "cargo tree --offline --target <triple> passes, and so does another flag's 'all'" 0 \
    "offline-cargo: files=1 sites=3 metadata=0 tree=3 hits=0"

root metadata-no-deps
run
expect "--no-deps resolves no graph, so it needs no platform" 0 \
    "offline-cargo: files=1 sites=1 metadata=1 tree=0 hits=0"

root metadata-platform-spellings
run
expect "--filter-platform=<t>, --target <t> and --target=<t> each name a platform" 0 \
    "offline-cargo: files=1 sites=3 metadata=3 tree=0 hits=0"

root near-miss-flags
run
expect "flags one character from the platform filter name nothing" 1 \
    "offline-cargo: files=1 sites=3 metadata=3 tree=0 hits=3" \
    "src/near-miss-flags.rs:3:" "src/near-miss-flags.rs:4:" "src/near-miss-flags.rs:5:"

root chain-form
run
expect "a .arg() chain is read to its end: the unfiltered one is caught, the filtered one passes" 1 \
    "offline-cargo: files=1 sites=2 metadata=2 tree=0 hits=1" \
    "src/chain-form.rs:2:"

root shell-literal
run
expect "a one-string call is read word by word" 1 \
    "offline-cargo: files=1 sites=4 metadata=2 tree=2 hits=2" \
    "src/shell-literal.rs:2:" "src/shell-literal.rs:5:"

root flag-before-subcommand
run
expect "--offline before the subcommand, or in a later .arg()/.args() of the chain, is read: 4 caught, the filtered one passes" 1 \
    "offline-cargo: files=1 sites=5 metadata=5 tree=0 hits=4" \
    "src/flag-before-subcommand.rs:3:" "src/flag-before-subcommand.rs:4:" \
    "src/flag-before-subcommand.rs:5:" "src/flag-before-subcommand.rs:6:"

root escapes
run
expect "an escape is the character it stands for: \\n ends a command, a line continuation joins one" 1 \
    "offline-cargo: files=1 sites=3 metadata=2 tree=1 hits=2" \
    "src/escapes.rs:3:" "src/escapes.rs:4:"

root shell-commands
run
expect "a shell line is judged command by command; a path, +toolchain, \$CARGO and a leading flag are read" 1 \
    "offline-cargo: files=1 sites=5 metadata=2 tree=3 hits=3" \
    "src/shell-commands.rs:3:" "src/shell-commands.rs:4:" "src/shell-commands.rs:7:"

root literals
run
expect "a quote char, an escaped one and raw strings open no string; lines stay true after them and a nested block comment" 1 \
    "offline-cargo: files=1 sites=7 metadata=1 tree=6 hits=7" \
    "src/literals.rs:5:" "src/literals.rs:9:" "src/literals.rs:12:" "src/literals.rs:15:" \
    "src/literals.rs:18:" "src/literals.rs:23:" "src/literals.rs:28:"

root shell-near-miss control-site
run
expect "mycargo, metadatax, treehouse and a command after echo are no site" 0 \
    "offline-cargo: files=2 sites=1 metadata=0 tree=1 hits=0"

root prose control-site
run
expect "a doc attribute, an expect()/panic!() text and an assertion message quoting a command are no site" 0 \
    "offline-cargo: files=2 sites=1 metadata=0 tree=1 hits=0"

root statement-bounds
run
expect "a \"metadata\" comparison borrows no later flag, and -p metadata is not a second subcommand" 0 \
    "offline-cargo: files=1 sites=1 metadata=0 tree=1 hits=0"

root not-code
run
expect "comments, a quote char literal, .get(\"metadata\"), \"tree\" and calls without --offline are no site" 3 \
    "offline-cargo: files=1 sites=0 metadata=0 tree=0 hits=0"

root not-code control-site
run
expect "... and beside a real site they add nothing" 0 \
    "offline-cargo: files=2 sites=1 metadata=0 tree=1 hits=0"

N=$((N + 1))
ROOT="$WORK/empty"
mkdir -p "$ROOT/src" "$ROOT/target"
printf 'fn a() {}\n' >"$ROOT/target/skipped.rs"
run
expect "a tree with no offline cargo call is an empty walk, and target/ is not walked" 3 \
    "offline-cargo: files=0 sites=0 metadata=0 tree=0 hits=0"

N=$((N + 1))
ROOT="$WORK/skipped"
mkdir -p "$ROOT/.worktrees/other" "$ROOT/node_modules/pkg" "$ROOT/.git" "$ROOT/.logos" \
    "$ROOT/.agents" "$ROOT/.claude" "$ROOT/dist"
for d in .worktrees/other node_modules/pkg .git .logos .agents .claude dist; do
    cp "$FIXTURES/hf12-original.rs.fixture" "$ROOT/$d/bypass.rs"
done
# A symlinked directory and a symlinked file are not followed: their targets sit
# outside the root, so only the links could lead the walk to them.
mkdir -p "$WORK/outside"
cp "$FIXTURES/hf12-original.rs.fixture" "$WORK/outside/bypass.rs"
ln -s "$WORK/outside" "$ROOT/linked-dir"
ln -s "$WORK/outside/bypass.rs" "$ROOT/linked-file.rs"
run
expect "another checkout under .worktrees/, and node_modules/, .git/, .logos/, .agents/, .claude/, dist/ and symlinks are not walked" 3 \
    "offline-cargo: files=0 sites=0 metadata=0 tree=0 hits=0"

N=$((N + 1))
ROOT="$WORK/no-such-root"
run
expect "a root that is not a directory is a usage error" 2 \
    "check-offline-cargo.py: not a directory: $ROOT"

# The tree this check guards: every offline call in it already names its platform,
# and the walk read real files and real sites, not an empty set.
ROOT="$HERE"
run
if [ "$RC" -eq 0 ] && printf '%s\n' "$OUT" | grep -Eq '^offline-cargo: files=[1-9][0-9]* sites=[1-9][0-9]* metadata=[0-9]+ tree=[0-9]+ hits=0$'; then
    echo "  ok    this repository's offline cargo calls all name their platform"
    PASS=$((PASS + 1))
else
    echo "  FAIL  this repository's offline cargo calls: exit $RC"
    printf '%s\n' "$OUT" | sed 's/^/        /'
    FAIL=$((FAIL + 1))
fi

echo
echo "passed $PASS, failed $FAIL"
[ "$FAIL" -eq 0 ] || exit 1
exit 0
