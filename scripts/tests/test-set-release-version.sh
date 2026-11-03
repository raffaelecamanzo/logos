#!/usr/bin/env bash
#
# test-set-release-version.sh — prove scripts/lib/set-release-version.py, the step
# scripts/swe-build.sh runs to write a release version, leaves BOTH the root
# Cargo.toml [workspace.package] version and plugins/logos/.claude-plugin/plugin.json
# at the next version and changes nothing else in either file (CR-272, S-855).
#
# The real build hook is never run: it builds the UI and the binary. The cases run the
# helper it calls over fixtures under $TMPDIR, plus a structural check that the hook
# calls the helper before its release commit and stages plugin.json in that commit.
#
# Usage: bash scripts/tests/test-set-release-version.sh
# Exit:  0 all cases behaved, 1 a case did not

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
HELPER="$HERE/scripts/lib/set-release-version.py"
BUILD="$HERE/scripts/swe-build.sh"

WORK="${TMPDIR:-/tmp}/logos-set-release-version-test.$$"
trap 'rm -rf "$WORK"' EXIT INT TERM
mkdir -p "$WORK" || exit 1
WORK="$(cd "$WORK" && pwd -P)"

PASS=0
FAIL=0
ok() { echo "  ok    $1"; PASS=$((PASS + 1)); }
bad() { echo "  FAIL  $1"; FAIL=$((FAIL + 1)); }
expect() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: got '$2', wanted '$3'"; fi; }

PJ=plugins/logos/.claude-plugin/plugin.json

# The shape of the real files: a nested "version" (a dependency table in Cargo.toml, a
# server entry in plugin.json) must survive, and so must the odd spacing.
fixture() { # fixture <dir> <cargo version> <plugin version>
    mkdir -p "$1/plugins/logos/.claude-plugin"
    cat >"$1/Cargo.toml" <<TOML
[workspace]
members = ["cli"]

[workspace.package]
version = "$2"
edition = "2021"

[workspace.dependencies]
serde = { version = "1.0.0", features = ["derive"] }
TOML
    cat >"$1/$PJ" <<JSON
{
  "name": "logos",
  "version": "$3",
  "description": "A \"quoted\" description, with version \"9.9.9\" inside.",
  "keywords": ["a", "b"],
  "mcpServers": {
    "logos": {
      "command": "\${CLAUDE_PLUGIN_ROOT}/scripts/mcp-serve.sh",
      "version": "0.0.1",
      "args": []
    }
  },
  "metadata": {
    "minBinaryVersion": "1.20.0"
  }
}
JSON
}

echo "test-set-release-version.sh"

# --- 1. both files carry the next version, and nothing else moved ----------------
D="$WORK/ok"
fixture "$D" 1.22.3 1.22.3
cp "$D/Cargo.toml" "$WORK/cargo.before"
cp "$D/$PJ" "$WORK/plugin.before"
python3 "$HELPER" "$D" 1.22.4
expect "[rewrite] exits 0" "$?" 0
expect "[rewrite] Cargo.toml [workspace.package] version is the next version" \
    "$(awk '/^\[workspace\.package\]/{f=1;next} /^\[/{f=0} f && /^version *=/{print; exit}' "$D/Cargo.toml")" 'version = "1.22.4"'
expect "[rewrite] plugin.json top-level version is the next version" \
    "$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["version"])' "$D/$PJ")" 1.22.4
# Byte identity: undo the one value in each file and the originals must come back.
sed 's/^version = "1.22.4"$/version = "1.22.3"/' "$D/Cargo.toml" >"$WORK/cargo.restored"
cmp -s "$WORK/cargo.before" "$WORK/cargo.restored" && ok "[rewrite] Cargo.toml differs from the original in the version line only" \
    || bad "[rewrite] Cargo.toml differs from the original beyond the version line"
python3 -I -c 'import sys; t = open(sys.argv[1], newline="").read(); sys.stdout.write(t.replace("\"version\": \"1.22.4\"", "\"version\": \"1.22.3\"", 1))' "$D/$PJ" >"$WORK/plugin.restored"
cmp -s "$WORK/plugin.before" "$WORK/plugin.restored" && ok "[rewrite] plugin.json differs from the original in the top-level version value only (byte-identical otherwise)" \
    || bad "[rewrite] plugin.json differs from the original beyond the top-level version value"
expect "[rewrite] a nested plugin.json \"version\" and the minBinaryVersion are untouched" \
    "$(python3 -I -c 'import json,sys; d=json.load(open(sys.argv[1])); print(d["mcpServers"]["logos"]["version"], d["metadata"]["minBinaryVersion"], d["description"].count("9.9.9"))' "$D/$PJ")" "0.0.1 1.20.0 1"
expect "[rewrite] the dependency table's version in Cargo.toml is untouched" \
    "$(grep -c 'serde = { version = "1.0.0"' "$D/Cargo.toml")" 1

# --- 2. a plugin.json that was behind (the 1.20.5 incident) is brought level ------
D="$WORK/behind"
fixture "$D" 1.22.3 1.20.5
python3 "$HELPER" "$D" 1.22.4
expect "[rewrite] a plugin.json stuck at an older version is brought to the next version too" \
    "$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["version"])' "$D/$PJ")" 1.22.4

# --- 3. refusals leave both files untouched ---------------------------------------
D="$WORK/nokey"
fixture "$D" 1.22.3 1.22.3
python3 - "$D/$PJ" <<'PY'
import json, sys
p = sys.argv[1]; d = json.load(open(p)); del d["version"]; open(p, "w").write(json.dumps(d, indent=2) + "\n")
PY
cp "$D/Cargo.toml" "$WORK/nokey.cargo"; cp "$D/$PJ" "$WORK/nokey.plugin"
ERR="$(python3 "$HELPER" "$D" 1.22.4 2>&1)"; RC=$?
expect "[refuse] a plugin.json with no top-level version exits 1" "$RC" 1
case "$ERR" in *"plugin.json was not rewritten"*) ok "[refuse] ... and says plugin.json was not rewritten" ;; *) bad "[refuse] message: $ERR" ;; esac
cmp -s "$D/Cargo.toml" "$WORK/nokey.cargo" && cmp -s "$D/$PJ" "$WORK/nokey.plugin" \
    && ok "[refuse] ... and neither file was touched (Cargo.toml included)" || bad "[refuse] a file changed although the run failed"

D="$WORK/nocargo"
fixture "$D" 1.22.3 1.22.3
printf '[workspace]\nmembers = []\n' >"$D/Cargo.toml"
cp "$D/$PJ" "$WORK/nocargo.plugin"
python3 "$HELPER" "$D" 1.22.4 >/dev/null 2>&1; RC=$?
expect "[refuse] a Cargo.toml with no [workspace.package] version exits 1" "$RC" 1
cmp -s "$D/$PJ" "$WORK/nocargo.plugin" && ok "[refuse] ... and plugin.json was not touched" || bad "[refuse] plugin.json changed although Cargo.toml failed"

# [workspace.package] is a table header at the start of a line: a comment that mentions
# it, or a table with no version of its own followed by one that has, is not it.
D="$WORK/cargotable"
fixture "$D" 1.22.3 1.22.3
printf '# see [workspace.package]\nlicense = "x"\nversion = "7.7.7"\n\n[workspace.package]\nversion = "1.22.3"\n' >"$D/Cargo.toml"
python3 "$HELPER" "$D" 1.22.4 >/dev/null 2>&1
expect "[rewrite] a comment that names [workspace.package] is not its header" \
    "$(grep -c 'version = "7.7.7"' "$D/Cargo.toml") $(grep -c '^version = "1.22.4"' "$D/Cargo.toml")" "1 1"
printf '[workspace.package]\nedition = "2021"\n\n[workspace.dependencies.serde]\nversion = "1.0.0"\n' >"$D/Cargo.toml"
cp "$D/Cargo.toml" "$WORK/cargotable.before"
python3 "$HELPER" "$D" 1.22.4 >/dev/null 2>&1; RC=$?
expect "[refuse] a [workspace.package] with no version, followed by a table that has one, exits 1" "$RC" 1
cmp -s "$D/Cargo.toml" "$WORK/cargotable.before" && ok "[refuse] ... and the other table's version was not rewritten" || bad "[refuse] a version outside [workspace.package] was rewritten"

# A write that fails after the rewrites verified (a read-only plugin.json) must not
# leave Cargo.toml bumped alone. Skipped as root, which ignores file modes.
D="$WORK/rofile"
fixture "$D" 1.22.3 1.22.3
cp "$D/Cargo.toml" "$WORK/ro.cargo"
chmod 444 "$D/$PJ"
if [ "$(id -u)" -ne 0 ]; then
    python3 "$HELPER" "$D" 1.22.4 >/dev/null 2>&1; RC=$?
    expect "[refuse] a plugin.json that cannot be written exits 1" "$RC" 1
    cmp -s "$D/Cargo.toml" "$WORK/ro.cargo" && ok "[refuse] ... and Cargo.toml was put back, never half-bumped" \
        || bad "[refuse] Cargo.toml kept the new version although plugin.json could not be written"
fi
chmod 644 "$D/$PJ"

# --- file shapes the byte-identity promise covers ----------------------------------
# restore <file> <new> <old> -> the file with its first top-level "version": "<new>" put back.
revert() { python3 -I -c 'import sys; t = open(sys.argv[1], newline="").read(); sys.stdout.write(t.replace("\"version\": \"" + sys.argv[2] + "\"", "\"version\": \"" + sys.argv[3] + "\"", 1))' "$1" "$2" "$3"; }
# CRLF line endings survive in both files.
D="$WORK/crlf"
fixture "$D" 1.22.3 1.22.3
sed 's/$/\r/' "$D/$PJ" >"$D/p.tmp" && mv "$D/p.tmp" "$D/$PJ"
cp "$D/$PJ" "$WORK/crlf.before"
python3 "$HELPER" "$D" 1.22.4 >/dev/null 2>&1
revert "$D/$PJ" 1.22.4 1.22.3 >"$WORK/crlf.restored"
cmp -s "$WORK/crlf.before" "$WORK/crlf.restored" && ok "[rewrite] a CRLF plugin.json keeps its line endings and every other byte" \
    || bad "[rewrite] a CRLF plugin.json was changed beyond the version value"
# A tab-indented manifest is rewritten at its top level.
D="$WORK/tabs"
fixture "$D" 1.22.3 1.22.3
sed 's/^  /\t/' "$D/$PJ" >"$D/p.tmp" && mv "$D/p.tmp" "$D/$PJ"
cp "$D/$PJ" "$WORK/tabs.before"
python3 "$HELPER" "$D" 1.22.4 >/dev/null 2>&1; RC=$?
revert "$D/$PJ" 1.22.4 1.22.3 >"$WORK/tabs.restored"
expect "[rewrite] a tab-indented plugin.json is rewritten (exit)" "$RC" 0
cmp -s "$WORK/tabs.before" "$WORK/tabs.restored" && ok "[rewrite] ... changing only the version value" || bad "[rewrite] a tab-indented plugin.json was changed beyond the version value"
# A nested "version" that comes BEFORE the top-level one is not the one rewritten.
D="$WORK/nestedfirst"
fixture "$D" 1.22.3 1.22.3
cat >"$D/$PJ" <<'JSON'
{
  "name": "logos",
  "mcpServers": {
    "logos": {
      "version": "0.0.1"
    }
  },
  "version": "1.22.3"
}
JSON
python3 "$HELPER" "$D" 1.22.4 >/dev/null 2>&1; RC=$?
expect "[rewrite] a top-level version after a nested one is rewritten, the nested one is not" \
    "$RC $(python3 -I -c 'import json,sys; d=json.load(open(sys.argv[1])); print(d["version"], d["mcpServers"]["logos"]["version"])' "$D/$PJ")" "0 1.22.4 0.0.1"
# Two top-level "version" keys are ambiguous: refused, nothing touched.
D="$WORK/dupkey"
fixture "$D" 1.22.3 1.22.3
printf '{\n  "version": "1.22.3",\n  "name": "logos",\n  "version": "1.22.3"\n}\n' >"$D/$PJ"
cp "$D/$PJ" "$WORK/dup.before"; cp "$D/Cargo.toml" "$WORK/dup.cargo"
python3 "$HELPER" "$D" 1.22.4 >/dev/null 2>&1; RC=$?
expect "[refuse] two top-level version keys exit 1" "$RC" 1
cmp -s "$D/$PJ" "$WORK/dup.before" && cmp -s "$D/Cargo.toml" "$WORK/dup.cargo" && ok "[refuse] ... with neither file touched" || bad "[refuse] a file changed although the key was ambiguous"

for v in 1.22 1.2.3-rc1 1.2.3.4 v1.2.3; do
    python3 "$HELPER" "$WORK/ok" "$v" >/dev/null 2>&1; expect "[refuse] '$v' is not X.Y.Z: exit 2" "$?" 2
done

# --- 4. the hook wires the helper in and stages plugin.json -----------------------
# A structural check, not a run: the hook builds the UI and the binary. Whole-line
# matches, so a commented-out call, a `|| true` that swallows its failure, a changed
# argument or a `git add -n` cannot satisfy them.
HELPER_LINE="$(grep -n -x -F 'python3 scripts/lib/set-release-version.py "$ROOT" "$NEXT"' "$BUILD" | head -1 | cut -d: -f1)"
CHANGELOG_LINE="$(grep -n -x -F "python3 - \"\$NEXT\" <<'PY'" "$BUILD" | head -1 | cut -d: -f1)"
ADD_LINE="$(grep -n -x -F "git add Cargo.toml $PJ Cargo.lock CHANGELOG.md" "$BUILD" | head -1 | cut -d: -f1)"
COMMIT_LINE="$(grep -n -x -F 'git commit -q -m "release($NEXT)"' "$BUILD" | head -1 | cut -d: -f1)"
if [ -n "$HELPER_LINE" ] && [ -n "$CHANGELOG_LINE" ] && [ "$HELPER_LINE" -lt "$CHANGELOG_LINE" ]; then
    ok "[hook] swe-build.sh runs the helper as a bare command ($BUILD:$HELPER_LINE), before the CHANGELOG edit"
else
    bad "[hook] swe-build.sh does not run 'python3 scripts/lib/set-release-version.py \"\$ROOT\" \"\$NEXT\"' as a line of its own before the CHANGELOG edit (helper '$HELPER_LINE', changelog '$CHANGELOG_LINE')"
fi
if [ -n "$ADD_LINE" ] && [ -n "$COMMIT_LINE" ] && [ "$CHANGELOG_LINE" -lt "$ADD_LINE" ] && [ "$ADD_LINE" -lt "$COMMIT_LINE" ]; then
    ok "[hook] the release commit stages exactly Cargo.toml, plugin.json, Cargo.lock and CHANGELOG.md, right before committing"
else
    bad "[hook] swe-build.sh lacks 'git add Cargo.toml $PJ Cargo.lock CHANGELOG.md' as a line of its own before the release commit (add '$ADD_LINE', commit '$COMMIT_LINE')"
fi

echo
echo "passed $PASS, failed $FAIL"
[ "$FAIL" -eq 0 ] || exit 1
exit 0
