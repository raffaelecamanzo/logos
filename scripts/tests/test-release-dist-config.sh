#!/usr/bin/env bash
#
# test-release-dist-config.sh — prove scripts/release-dist-config.sh gives a tag
# that predates the release pipeline exactly the cargo-dist configuration this
# repository carries, and changes nothing on a tag that already has it (S-732).
#
# The pipeline side is this repository's own Cargo.toml, cli/Cargo.toml and
# dist-workspace.toml. The tag side is the same two manifests with the two dist
# tables cut out (by python here, not by the script under test), which is the
# shape v1.16.1 and v1.16.2 have. Every manifest the script writes is then parsed
# as TOML, so an append that broke the file fails here rather than in a release.
#
# Usage: bash scripts/tests/test-release-dist-config.sh
# Exit:  0 all cases behaved, 1 a case did not

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRIPT="$HERE/scripts/release-dist-config.sh"
[ -f "$SCRIPT" ] || { echo "cannot find scripts/release-dist-config.sh (looked in $HERE)" >&2; exit 1; }
python3 -c 'import tomllib' >/dev/null 2>&1 || { echo "python3 >= 3.11 (tomllib) is required" >&2; exit 1; }

WORK="${TMPDIR:-/tmp}/logos-release-dist-config-test.$$"
trap 'rm -rf "$WORK"' EXIT INT TERM
mkdir -p "$WORK" || exit 1

PASS=0
FAIL=0
ok() { echo "  ok    $1"; PASS=$((PASS + 1)); }
bad() {
    echo "  FAIL  $1"
    printf '%s\n' "${OUT:-}" | sed 's/^/        /'
    FAIL=$((FAIL + 1))
}

# The pipeline: what this repository carries.
PIPE="$WORK/pipeline"
mkdir -p "$PIPE/cli"
/bin/cp "$HERE/Cargo.toml" "$PIPE/Cargo.toml"
/bin/cp "$HERE/cli/Cargo.toml" "$PIPE/cli/Cargo.toml"
/bin/cp "$HERE/dist-workspace.toml" "$PIPE/dist-workspace.toml"

# old_tag <dir>: the manifests with both dist tables cut out, no dist-workspace.toml.
old_tag() {
    mkdir -p "$1/cli"
    python3 - "$PIPE" "$1" <<'PY' || exit 1
import re, sys
src, dst = sys.argv[1], sys.argv[2]
def cut(text, header):
    # The header line and its key lines, up to the next blank/comment/header line.
    new, n = re.subn(r'(?m)^' + re.escape(header) + r'\n(?:[^\n#\[][^\n]*\n)*', '', text)
    if n != 1:
        sys.exit("expected one %s table, cut %d" % (header, n))
    return new
for rel, header in (("Cargo.toml", "[profile.dist]"), ("cli/Cargo.toml", "[package.metadata.dist]")):
    text = open("%s/%s" % (src, rel)).read()
    open("%s/%s" % (dst, rel), "w").write(cut(text, header))
PY
}

# toml_check <dir>: both manifests parse, and carry the dist settings.
toml_check() {
    python3 - "$1" <<'PY'
import sys, tomllib
d = sys.argv[1]
root = tomllib.load(open(d + "/Cargo.toml", "rb"))
cli = tomllib.load(open(d + "/cli/Cargo.toml", "rb"))
assert root["profile"]["dist"] == {"inherits": "release"}, root["profile"].get("dist")
assert cli["package"]["metadata"]["dist"] == {"dist": True}, cli["package"].get("metadata")
assert "features" not in cli["package"]["metadata"]["dist"]
assert root["workspace"]["package"]["version"], "the version survived"
PY
}

run() { # run <args>... -> OUT, RC
    OUT="$(bash "$SCRIPT" "$@" 2>&1)"
    RC=$?
}

echo "test-release-dist-config.sh"
echo "  work $WORK"
echo

# --- a tag that predates the pipeline gets all three pieces -------------------

TAG="$WORK/tag"
old_tag "$TAG"
if grep -Fq '[profile.dist]' "$TAG/Cargo.toml" || grep -Fq '[package.metadata.dist]' "$TAG/cli/Cargo.toml"; then
    bad "the fixture tag still carries a dist table"
else
    ok "the fixture tag carries no dist table (the v1.16.2 shape)"
fi
/bin/cp "$TAG/Cargo.toml" "$WORK/before-root.toml"
/bin/cp "$TAG/cli/Cargo.toml" "$WORK/before-cli.toml"

run --from "$PIPE" --into "$TAG"
want="$(printf '%s\n' 'dist-workspace.toml: added' 'Cargo.toml [profile.dist]: added' 'cli/Cargo.toml [package.metadata.dist]: added')"
if [ "$RC" -eq 0 ] && [ "$OUT" = "$want" ]; then ok "a tag without the pipeline gets all three pieces"; else bad "exit $RC"; fi

if cmp -s "$PIPE/dist-workspace.toml" "$TAG/dist-workspace.toml"; then
    ok "dist-workspace.toml is the pipeline's, byte for byte"
else
    bad "dist-workspace.toml differs from the pipeline's"
fi

if OUT="$(toml_check "$TAG" 2>&1)"; then ok "both manifests parse as TOML and carry the dist settings"; else bad "the manifests do not parse or lack the settings"; fi

# Only appended: every byte the tag had is still there, in place.
# prefix <before> <after>: <after> starts with every byte of <before>.
# (BSD cmp reports EOF on the shorter file even under -n, so the prefix is cut first.)
prefix() { head -c "$(wc -c <"$1" | tr -d ' ')" "$2" | cmp -s - "$1"; }
if prefix "$WORK/before-root.toml" "$TAG/Cargo.toml" && prefix "$WORK/before-cli.toml" "$TAG/cli/Cargo.toml"; then
    ok "the tag's manifests are only appended to"
else
    bad "the tag's manifests were edited, not appended to"
fi

# The appended text is the table and nothing of the section after it.
added="$(tail -n 3 "$TAG/cli/Cargo.toml")"
want="$(printf '%s\n' "# Supplied by scripts/release-dist-config.sh from the release workflow's commit." '[package.metadata.dist]' 'dist = true')"
if [ "$added" = "$want" ]; then ok "the table is copied without the next section's comments"; else OUT="$added"; bad "cli/Cargo.toml ends with something else"; fi

# --- a second run, and a tag that carries the pipeline, change nothing ---------

sum() { cat "$1/Cargo.toml" "$1/cli/Cargo.toml" "$1/dist-workspace.toml" | cksum; }
s1="$(sum "$TAG")"
run --from "$PIPE" --into "$TAG"
want="$(printf '%s\n' 'dist-workspace.toml: kept' 'Cargo.toml [profile.dist]: kept' 'cli/Cargo.toml [package.metadata.dist]: kept')"
if [ "$RC" -eq 0 ] && [ "$OUT" = "$want" ] && [ "$(sum "$TAG")" = "$s1" ]; then
    ok "a second run keeps everything and changes nothing"
else
    bad "a second run: exit $RC"
fi

NEW="$WORK/new"
mkdir -p "$NEW/cli"
/bin/cp "$PIPE/Cargo.toml" "$NEW/Cargo.toml"
/bin/cp "$PIPE/cli/Cargo.toml" "$NEW/cli/Cargo.toml"
printf '# the tag'"'"'s own\n' >"$NEW/dist-workspace.toml"
s1="$(sum "$NEW")"
run --from "$PIPE" --into "$NEW"
if [ "$RC" -eq 0 ] && [ "$OUT" = "$want" ] && [ "$(sum "$NEW")" = "$s1" ]; then
    ok "a tag with its own configuration keeps it, even where it differs"
else
    bad "a tag that carries the pipeline: exit $RC"
fi

# The workflow runs it from the tag's checkout without --into: the default is "."
T4="$WORK/tag4"
old_tag "$T4"
OUT="$(cd "$T4" && bash "$SCRIPT" --from "$PIPE" 2>&1)"
RC=$?
want="$(printf '%s\n' 'dist-workspace.toml: added' 'Cargo.toml [profile.dist]: added' 'cli/Cargo.toml [package.metadata.dist]: added')"
if [ "$RC" -eq 0 ] && [ "$OUT" = "$want" ] && toml_check "$T4" >/dev/null 2>&1; then
    ok "without --into it supplies the current directory, as the workflow calls it"
else
    bad "without --into: exit $RC"
fi

# --- refusals ------------------------------------------------------------------

EMPTY="$WORK/empty-pipeline"
mkdir -p "$EMPTY/cli"
printf '[workspace]\nmembers = []\n' >"$EMPTY/Cargo.toml"
printf '[package]\nname = "logos"\n' >"$EMPTY/cli/Cargo.toml"
/bin/cp "$PIPE/dist-workspace.toml" "$EMPTY/dist-workspace.toml"
T2="$WORK/tag2"
old_tag "$T2"
run --from "$EMPTY" --into "$T2"
if [ "$RC" -eq 1 ] && printf '%s\n' "$OUT" | grep -qF "has no [profile.dist] in Cargo.toml to supply"; then
    ok "a pipeline without the table to supply is refused"
else
    bad "a pipeline without the table: exit $RC"
fi

T3="$WORK/tag3"
old_tag "$T3"
/bin/rm -f "$EMPTY/dist-workspace.toml"
run --from "$EMPTY" --into "$T3"
if [ "$RC" -eq 1 ] && printf '%s\n' "$OUT" | grep -qF "has no dist-workspace.toml to supply"; then
    ok "a pipeline without dist-workspace.toml is refused"
else
    bad "a pipeline without dist-workspace.toml: exit $RC"
fi

run --into "$TAG"
if [ "$RC" -eq 2 ]; then ok "no --from is a usage error"; else bad "no --from: exit $RC"; fi
run --from "$PIPE" --into "$WORK/no-such-dir"
if [ "$RC" -eq 2 ]; then ok "a missing directory is a usage error"; else bad "a missing directory: exit $RC"; fi
mkdir -p "$WORK/bare"
run --from "$PIPE" --into "$WORK/bare"
if [ "$RC" -eq 2 ] && printf '%s\n' "$OUT" | grep -qF "has no Cargo.toml"; then
    ok "a directory that is not a checkout is a usage error"
else
    bad "a directory without manifests: exit $RC"
fi

# --- features come only from [package.metadata.dist] --------------------------
# The repository's dist-workspace.toml is what release-dist-config.sh supplies, so
# a features key there would reach every release, past the cli crate's table.
OUT="$(python3 - "$HERE/dist-workspace.toml" <<'PY' 2>&1
import sys, tomllib
dist = tomllib.load(open(sys.argv[1], "rb")).get("dist", {})
keys = [k for k in ("features", "all-features", "default-features") if k in dist]
assert not keys, "dist-workspace.toml [dist] selects features: %s" % keys
PY
)"
RC=$?
if [ "$RC" -eq 0 ]; then
    ok "dist-workspace.toml selects no cargo features"
else
    bad "dist-workspace.toml selects cargo features"
fi

echo
echo "passed $PASS, failed $FAIL"
[ "$FAIL" -eq 0 ] || exit 1
exit 0
