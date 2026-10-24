#!/usr/bin/env bash
#
# test-ci-pins.sh — prove .github/workflows/ci.yml runs every action pinned to a
# commit with a read-only token, that .github/workflows/release.yml pins its
# actions the same way, and that .github/dependabot.yml keeps them current (S-774,
# HF-4, NFR-MA-10, NFR-MA-04). The workflows cannot be run outside GitHub, so
# their text is what can be checked here.
#
# Every non-comment `uses:` line of each workflow must:
#   - name `owner/repo[/path]@<40 lowercase hex>`, never a tag, a branch or a
#     docker:// image (a `./` local action is in-repo and exempt);
#   - carry a trailing `# vX.Y.Z` comment naming the release the commit was
#     resolved from. dtolnay/rust-toolchain publishes no release tag but `v1`, so
#     it alone may carry `# vN`;
# and, beyond the `uses:` lines:
#   - every dtolnay/rust-toolchain step passes `toolchain: stable` in its own
#     `with:` block (the commit is not the `stable` branch, so it has no default);
#     the step's `uses:` is its first key, as every step here writes it;
#   - actions/setup-node is on v5 or later, the first major off Node 20;
#   - the workflow has a top-level `permissions:` block. ci.yml's grants `contents:
#     read` and nothing wider than `read`. release.yml publishes the release, so
#     its block grants `contents: write` and nothing else wider than `read`; a
#     quoted key, as cargo-dist writes it, is read like a bare one.
# Every line naming a `uses:` key must be one the check read, so a spelling it does
# not parse (a quoted key, `uses :`, a flow mapping) fails rather than escapes.
# Every workflow under .github/workflows and every composite action under
# .github/actions must be classified: checked as above (ci.yml, release.yml), or
# exempt with a reason (none is). A file that is neither fails, and so does a
# checked or exempt one that no longer exists, so a new workflow cannot run
# unpinned actions unseen.
# .github/dependabot.yml must be `version: 2`, update exactly one ecosystem,
# `github-actions`, and hold each release back with a `cooldown:` of at least one
# `default-days`. A pin's comment is checked for its shape, not for naming the
# release its commit belongs to: that needs the network (see the recipe in
# docs/howto/contributing.md).
#
# The checker is then run over copies of the real workflows with one defect each (a
# tag, a short SHA, an upper-case SHA, a missing or malformed comment, a removed
# permissions block, a widened one, a missing toolchain input, setup-node on v4,
# a second dependabot ecosystem, a missing cooldown) and must fail every one of
# them, naming the defect: a check that cannot fail proves nothing.
#
# Usage: bash scripts/tests/test-ci-pins.sh
# Exit:  0 every case behaved, 1 one did not
#
# bash 3.2 compatible (no mapfile, no associative arrays, no ${var,,}).

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CI="$HERE/.github/workflows/ci.yml"
REL="$HERE/.github/workflows/release.yml"
DEP="$HERE/.github/dependabot.yml"
[ -f "$CI" ] || { echo "cannot find .github/workflows/ci.yml (looked in $HERE)" >&2; exit 1; }
[ -f "$REL" ] || { echo "cannot find .github/workflows/release.yml (looked in $HERE)" >&2; exit 1; }
[ -f "$DEP" ] || { echo "cannot find .github/dependabot.yml (looked in $HERE)" >&2; exit 1; }

WORK="${TMPDIR:-/tmp}/logos-ci-pins-test.$$"
trap 'rm -rf "$WORK"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
mkdir -p "$WORK" || exit 1

PASS=0
FAIL=0
OUT=""
ok() { echo "  ok    $1"; PASS=$((PASS + 1)); }
bad() {
    echo "  FAIL  $1"
    printf '%s\n' "$OUT" | sed 's/^/        /'
    FAIL=$((FAIL + 1))
}

# check_workflow FILE [WANT] — print one `<file>:LINE: <defect>` per violation, exit
# 1 if there is any. WANT is the `contents:` permission the top-level block must
# grant: `read` (the default) or `write` (release.yml). A step's `with:` block is
# the lines indented deeper than its `uses:` key, up to the first line that is not.
check_workflow() {
    local want="${2:-read}"
    awk -v want="$want" -v fname="$(basename "$1")" '
    function indent(s) { match(s, /^ */); return RLENGTH }
    function bad(n, msg) { printf "%s:%d: %s\n", fname, n, msg; rc = 1 }
    function close_step() {
        if (in_step && is_toolchain && !has_toolchain_stable)
            bad(step_line, "dtolnay/rust-toolchain step passes no `toolchain: stable` input")
        in_step = 0
    }
    BEGIN { rc = 0 }
    {
        line = $0
        sub(/\r$/, "", line)
        if (line ~ /^[ \t]*$/ || line ~ /^[ \t]*#/) next

        # top-level `permissions:` — a column-0 key, then its indented entries.
        if (line ~ /^[^ \t]/) {
            in_perm = 0
            if (line ~ /^permissions:/) {
                saw_perm = 1; in_perm = 1
                rest = line; sub(/^permissions:[ \t]*/, "", rest); sub(/[ \t]*#.*$/, "", rest)
                if (rest != "") { perm_wide = 1; perm_line = NR }   # `read-all`, `write-all`, `{}` …
            }
        } else if (in_perm) {
            ent = line; sub(/^[ \t]+/, "", ent); sub(/[ \t]*#.*$/, "", ent); gsub(/[ \t]+/, " ", ent); gsub(/"/, "", ent)
            if (ent == "contents: " want) perm_ok = 1
            else if (ent !~ /^[A-Za-z-]+: (read|none)$/) { perm_wide = 1; perm_line = NR }
        }

        if (in_step && indent(line) < step_indent) close_step()
        if (in_step) {
            if (in_with && indent(line) <= with_indent) in_with = 0
            if (!in_with && indent(line) == step_indent && line ~ /^[ \t]*with:[ \t]*(#.*)?$/) { in_with = 1; with_indent = indent(line) }
            else if (in_with && line ~ /^[ \t]*toolchain:[ \t]*["\x27]?stable["\x27]?[ \t]*(#.*)?$/) has_toolchain_stable = 1
        }

        # Every line that names a `uses:` key, however it is spelled, is counted: one
        # the form below does not read (a quoted key, a space before the colon, a
        # flow mapping) would otherwise be an unchecked action.
        if (line ~ /(^|[^A-Za-z0-9_-])["\x27]?uses["\x27]?[ \t]*:/) n_named++

        if (line ~ /^[ \t]*(-[ \t]+)?uses:[ \t]/) {
            u = line
            sub(/^[ \t]*(-[ \t]+)?uses:[ \t]*/, "", u)
            cmt = ""
            if (match(u, /[ \t]+#/)) { cmt = substr(u, RSTART); sub(/^[ \t]+#[ \t]*/, "", cmt); sub(/[ \t]+$/, "", cmt); u = substr(u, 1, RSTART - 1) }
            sub(/[ \t]+$/, "", u)
            n_uses++
            if (u ~ /^\.\//) next
            at = index(u, "@")
            name = (at ? substr(u, 1, at - 1) : u); ref = (at ? substr(u, at + 1) : "")
            if (ref !~ /^[0-9a-f]+$/ || length(ref) != 40) { bad(NR, "`" u "` is not pinned to a 40-hex commit"); }
            else if (name == "dtolnay/rust-toolchain") {
                if (cmt !~ /^v[0-9]+$/ && cmt !~ /^v[0-9]+\.[0-9]+\.[0-9]+$/) bad(NR, "`" name "` has no `# vN` or `# vX.Y.Z` comment (found `" cmt "`)")
            } else if (cmt !~ /^v[0-9]+\.[0-9]+\.[0-9]+$/) bad(NR, "`" name "` has no `# vX.Y.Z` comment (found `" cmt "`)")
            if (name == "actions/setup-node") {
                major = cmt; sub(/^v/, "", major); sub(/\..*$/, "", major)
                if (major !~ /^[0-9]+$/ || major + 0 < 5) bad(NR, "actions/setup-node is on `" cmt "`, a major that targets Node 20")
            }
            close_step()
            in_step = 1; in_with = 0; step_line = NR
            if (line ~ /^[ \t]*-/) step_indent = index(line, "uses:") - 1; else step_indent = indent(line)
            is_toolchain = (name == "dtolnay/rust-toolchain"); has_toolchain_stable = 0
        }
    }
    END {
        close_step()
        if (!saw_perm) bad(0, "no top-level `permissions:` block")
        else if (!perm_ok) bad(0, "top-level `permissions:` does not grant `contents: " want "`")
        if (perm_wide) bad(perm_line, "top-level `permissions:` grants more than read")
        if (n_uses == 0) bad(0, "no `uses:` line found (nothing checked is not a pass)")
        if (n_named != n_uses) bad(0, n_named " lines name a `uses:` key but " n_uses " were read: a form this check does not read")
        exit rc
    }' "$1"
}

# check_dependabot FILE — exit 1 unless version 2 with exactly github-actions and a
# cooldown of at least one day.
check_dependabot() {
    local f="$1" eco n
    grep -Eq '^version:[[:space:]]*2[[:space:]]*(#.*)?$' "$f" || { echo "dependabot.yml: not \`version: 2\`"; return 1; }
    n="$(grep -v '^[[:space:]]*#' "$f" | grep -o 'package-ecosystem' | wc -l | tr -d ' ')"
    [ "$n" -eq 1 ] || { echo "dependabot.yml: $n package-ecosystem entries, want exactly 1"; return 1; }
    eco="$(grep -E '^[[:space:]]*-?[[:space:]]*package-ecosystem:' "$f" | sed -E 's/^[^:]*:[[:space:]]*//; s/[[:space:]]*#.*$//; s/^"(.*)"$/\1/')"
    [ "$eco" = "github-actions" ] || { echo "dependabot.yml: ecosystem is \`$eco\`, want github-actions"; return 1; }
    grep -v '^[[:space:]]*#' "$f" | grep -Eq '^[[:space:]]+cooldown:[[:space:]]*$' || { echo "dependabot.yml: no \`cooldown:\` block"; return 1; }
    grep -v '^[[:space:]]*#' "$f" | grep -Eq '^[[:space:]]+default-days:[[:space:]]*[1-9][0-9]*[[:space:]]*(#.*)?$' ||
        { echo "dependabot.yml: \`cooldown:\` has no \`default-days\` of at least 1"; return 1; }
    return 0
}

echo "test-ci-pins.sh"
echo "  workflows $CI"
echo "            $REL"
echo

# The workflows this suite checks, and those it does not, each with its reason.
# release.yml is cargo-dist output with this repository's edits; its actions are
# pinned by hand (docs/howto/contributing.md), so it is checked like ci.yml, bar
# the `contents: write` it needs to publish.
CHECKED_WORKFLOWS=".github/workflows/ci.yml .github/workflows/release.yml"
EXEMPT_WORKFLOWS=""

# classify_workflows DIR — print one line per workflow or composite action under
# DIR/.github that is neither checked nor exempt, and per checked or exempt file
# that is gone; exit 1 if there is any.
classify_workflows() {
    local dir="$1" rc=0 f rel known k
    for f in "$dir"/.github/workflows/*.yml "$dir"/.github/workflows/*.yaml \
             "$dir"/.github/actions/*/action.yml "$dir"/.github/actions/*/action.yaml; do
        [ -e "$f" ] || continue
        rel="${f#"$dir"/}"
        known=0
        for k in $CHECKED_WORKFLOWS $EXEMPT_WORKFLOWS; do [ "$k" = "$rel" ] && known=1; done
        if [ "$known" -eq 0 ]; then echo "unclassified: $rel is neither checked nor exempt"; rc=1; fi
    done
    for k in $CHECKED_WORKFLOWS; do
        [ -e "$dir/$k" ] || { echo "checked but missing: $k"; rc=1; }
    done
    for k in $EXEMPT_WORKFLOWS; do
        [ -e "$dir/$k" ] || { echo "exempt but missing: $k"; rc=1; }
    done
    return "$rc"
}

# --- the repository as it is ---------------------------------------------------
OUT="$(classify_workflows "$HERE" 2>&1)"; RC=$?
if [ "$RC" -eq 0 ]; then ok "every workflow and composite action is checked or exempt with a reason"; else bad "a workflow is unclassified"; fi

OUT="$(check_workflow "$CI" 2>&1)"; RC=$?
if [ "$RC" -eq 0 ]; then ok "ci.yml: every action pinned, read-only token, toolchain input, setup-node off Node 20"; else bad "ci.yml carries a defect"; fi

OUT="$(check_workflow "$REL" write 2>&1)"; RC=$?
if [ "$RC" -eq 0 ]; then ok "release.yml: every action pinned, a \`contents: write\` token and nothing wider, setup-node off Node 20"; else bad "release.yml carries a defect"; fi

# release.yml checked as ci.yml is: its write token is the one thing that differs.
OUT="$(check_workflow "$REL" read 2>&1)"; RC=$?
if [ "$RC" -eq 1 ] && printf '%s\n' "$OUT" | grep -q 'does not grant `contents: read`'; then ok "release.yml: a read-only check refuses its write token, so the level is really checked"; else bad "release.yml passed the read-only check (exit $RC)"; fi

NUSES="$(grep -Ec '^[[:space:]]*(-[[:space:]]+)?uses:[[:space:]]' "$CI")"
if [ "$NUSES" -ge 1 ]; then ok "ci.yml: $NUSES uses: lines checked (a count of zero would not be a pass)"; else OUT="no uses: line"; bad "ci.yml has no uses: line"; fi

NRELUSES="$(grep -Ec '^[[:space:]]*(-[[:space:]]+)?uses:[[:space:]]' "$REL")"
if [ "$NRELUSES" -ge 1 ]; then ok "release.yml: $NRELUSES uses: lines checked"; else OUT="no uses: line"; bad "release.yml has no uses: line"; fi

OUT="$(check_dependabot "$DEP" 2>&1)"; RC=$?
if [ "$RC" -eq 0 ]; then ok "dependabot.yml: version 2, github-actions only, with a cooldown"; else bad "dependabot.yml carries a defect"; fi

# --- mutation: each defect must be caught and named -----------------------------
FIRST_CHECKOUT_SHA="$(grep -Eo 'actions/checkout@[0-9a-f]{40}' "$CI" | head -1 | sed 's/.*@//')"
FIRST_SETUP_NODE_SHA="$(grep -Eo 'actions/setup-node@[0-9a-f]{40}' "$CI" | head -1 | sed 's/.*@//')"
[ -n "$FIRST_CHECKOUT_SHA" ] && [ -n "$FIRST_SETUP_NODE_SHA" ] || { echo "cannot find the checkout and setup-node pins in ci.yml" >&2; exit 1; }

# expect_caught NAME FILE PATTERN — the checker must exit 1 and print PATTERN.
expect_caught() {
    OUT="$(check_workflow "$2" "${WANT:-read}" 2>&1)"; RC=$?
    if [ "$RC" -eq 1 ] && printf '%s\n' "$OUT" | grep -qE -- "$3"; then ok "caught: $1"; else bad "not caught ($1): exit $RC, wanted /$3/"; fi
}
# mutate NAME PERL-EXPR PATTERN — apply a perl -p edit (BSD sed has no `0,/re/`
# first-match address, perl is already a dependency of the other suites) to a copy
# of ci.yml (or of $SRC, checked for the `contents:` level $WANT) and expect it caught. `$d ||= s///` edits the first match only.
# An optional fourth argument replaces perl's `-p` (`-0777 -p` edits across lines).
mutate() {
    local f="$WORK/m.$$.yml"
    perl ${4:--p} -e "$2" "${SRC:-$CI}" >"$f"
    if cmp -s "${SRC:-$CI}" "$f"; then OUT="the mutation changed nothing: $2"; bad "mutation inert ($1)"; return; fi
    expect_caught "$1" "$f" "$3"
}
# mutate_ok NAME PERL-EXPR [PERL-FLAGS] — the same edit, which the checker must accept.
mutate_ok() {
    local f="$WORK/ok.$$.yml"
    perl ${3:--p} -e "$2" "${SRC:-$CI}" >"$f"
    if cmp -s "${SRC:-$CI}" "$f"; then OUT="the mutation changed nothing: $2"; bad "mutation inert ($1)"; return; fi
    OUT="$(check_workflow "$f" "${WANT:-read}" 2>&1)"; RC=$?
    if [ "$RC" -eq 0 ]; then ok "accepted: $1"; else bad "refused ($1): exit $RC"; fi
}

mutate "an action on a tag" '$d ||= s/actions\/checkout@[0-9a-f]{40} # v[0-9.]+/actions\/checkout\@v4/' 'not pinned to a 40-hex'
mutate "an action on a branch" '$d ||= s/dtolnay\/rust-toolchain@[0-9a-f]{40} # v1/dtolnay\/rust-toolchain\@stable/' 'not pinned to a 40-hex'
mutate "a 39-hex SHA" "\$d ||= s/($FIRST_CHECKOUT_SHA)/substr(\$1, 0, 39)/e" 'not pinned to a 40-hex'
mutate "a 41-hex SHA" "\$d ||= s/$FIRST_CHECKOUT_SHA/${FIRST_CHECKOUT_SHA}0/" 'not pinned to a 40-hex'
mutate "an upper-case SHA" "\$d ||= s/($FIRST_CHECKOUT_SHA)/\\U\$1/" 'not pinned to a 40-hex'
mutate "a pinned action with no version comment" '$d ||= s/(actions\/checkout@[0-9a-f]{40}) # v[0-9.]+/$1/' 'no `# vX.Y.Z` comment'
mutate "a pinned action with a branch comment" '$d ||= s/(actions\/checkout@[0-9a-f]{40}) # v[0-9.]+/$1 # main/' 'no `# vX.Y.Z` comment'
mutate "a floating-major comment on a release-tagged action" '$d ||= s/(actions\/checkout@[0-9a-f]{40}) # v[0-9.]+/$1 # v4/' 'no `# vX.Y.Z` comment'
mutate "a missing top-level permissions block" '$skip = 1 if /^permissions:$/; if ($skip) { $skip = 0 if /^  contents: read$/; $_ = "" }' 'no top-level `permissions:` block'
mutate "a permissions block that is not read" 's/^  contents: read$/  contents: write/' 'does not grant `contents: read`'
mutate "a widened permissions block" 's/^  contents: read$/  contents: read\n  id-token: write/' 'grants more than read'
mutate "a read-all shorthand" 's/^permissions:$/permissions: write-all/' 'grants more than read'
# Each of the three rust-toolchain steps is checked on its own with: block, so a
# step that loses the input is caught while the others still pass it.
for K in 1 2 3; do
    mutate "rust-toolchain step $K of 3 with no toolchain input" "if (/^ *toolchain: stable\$/ && ++\$c == $K) { s/^ *toolchain: stable\\n// }" 'passes no `toolchain: stable`'
done
mutate "toolchain: stable under env:, not with:" 's/(dtolnay\/rust-toolchain\@[0-9a-f]{40} # v1\n\s+)with:/$1env:/' 'passes no `toolchain: stable`' '-0777 -p'
mutate "a toolchain that merely starts with stable" '$d ||= s/^( +)toolchain: stable$/$1toolchain: stable-x/' 'passes no `toolchain: stable`'
mutate "a dtolnay/rust-toolchain pin with no comment" '$d ||= s/(dtolnay\/rust-toolchain\@[0-9a-f]{40}) # v1$/$1/' 'no `# vN` or `# vX.Y.Z` comment'
mutate "a dtolnay/rust-toolchain pin with a branch comment" '$d ||= s/(dtolnay\/rust-toolchain\@[0-9a-f]{40}) # v1$/$1 # master/' 'no `# vN` or `# vX.Y.Z` comment'
mutate "a comment that only ends in a release" '$d ||= s/(actions\/checkout\@[0-9a-f]{40}) # v[0-9.]+/$1 # v1.2.3-rc/' 'no `# vX.Y.Z` comment'
mutate "a bare action with no ref" '$d ||= s/actions\/checkout\@[0-9a-f]{40} # v[0-9.]+/actions\/checkout/' 'not pinned to a 40-hex'
mutate "a docker:// image" '$d ||= s/actions\/checkout\@[0-9a-f]{40} # v[0-9.]+/docker:\/\/alpine:3/' 'not pinned to a 40-hex'
mutate_ok "setup-node on v5.0.0, the first major off Node 20" 's/(actions\/setup-node\@[0-9a-f]{40}) # v[0-9.]+/$1 # v5.0.0/'
mutate_ok "a comment line inside the permissions block" 's/^  contents: read$/  # id-token: write\n  contents: read/'
mutate_ok "a double space and a trailing comment in the permissions block" 's/^  contents: read$/  contents:  read # why/'
mutate_ok "CRLF line endings" 's/\n/\r\n/'
mutate_ok "toolchain quoted in the with: block" '$d ||= s/^( +)toolchain: stable$/$1toolchain: "stable"/'

# A `uses:` spelled a way the step parser does not read is refused, not skipped.
mutate "a uses key with a space before the colon" '$d ||= s/uses: actions\/cache\@[0-9a-f]{40} # v[0-9.]+/uses : actions\/cache\@v3/' 'a form this check does not read'
mutate "a quoted uses key" '$d ||= s/uses: actions\/cache\@[0-9a-f]{40} # v[0-9.]+/"uses": actions\/cache\@v3/' 'a form this check does not read'
f="$WORK/flow.yml"
{ cat "$CI"; echo '      - {uses: actions/checkout@v4}'; } >"$f"
expect_caught "a flow-mapping step" "$f" 'a form this check does not read'

# Nothing to check is a failure, not a pass.
f="$WORK/nouses.yml"
printf 'name: CI\npermissions:\n  contents: read\njobs: {}\n' >"$f"
expect_caught "a workflow with no uses: line" "$f" 'no `uses:` line found'

# bare and docker:// refs and a ./ local action, appended to the real workflow.
f="$WORK/local.yml"
{ cat "$CI"; echo '      - uses: ./.github/actions/local'; } >"$f"
OUT="$(check_workflow "$f" 2>&1)"; RC=$?
if [ "$RC" -eq 0 ]; then ok "accepted: a ./ local action needs no pin"; else bad "a ./ local action was refused"; fi
mutate "a rust-toolchain toolchain input that is not stable" '$d ||= s/^( +)toolchain: stable$/$1toolchain: nightly/' 'passes no `toolchain: stable`'
mutate "setup-node on Node 20 (v4)" "s/(actions\\/setup-node\\@$FIRST_SETUP_NODE_SHA) # v[0-9.]+/\$1 # v4.4.0/" 'targets Node 20'

# A permissions block only at job level is not the workflow default.
f="$WORK/joblevel.yml"
perl -pe '$skip = 1 if /^permissions:$/; if ($skip) { $skip = 0 if /^  contents: read$/; $_ = "" } $d ||= s/^    runs-on:/    permissions:\n      contents: read\n    runs-on:/' "$CI" >"$f"
expect_caught "permissions only at job level" "$f" 'no top-level `permissions:` block'

# An unpinned `uses:` inside a comment is not an action and must pass; the same
# line outside a comment is refused by the cases above.
f="$WORK/commented.yml"
{ cat "$CI"; echo '# uses: actions/checkout@v4'; } >"$f"
OUT="$(check_workflow "$f" 2>&1)"; RC=$?
if [ "$RC" -eq 0 ]; then ok "a commented-out unpinned uses: is not a violation"; else bad "a commented-out uses: was flagged"; fi

# A near miss: a name that merely contains dtolnay/rust-toolchain is not exempt
# from the `# vX.Y.Z` rule.
mutate "a lookalike of the rust-toolchain exemption" 's/dtolnay\/rust-toolchain\@([0-9a-f]{40}) # v1$/dtolnay\/rust-toolchain-fork\@$1 # v1/' 'no `# vX.Y.Z` comment'

# --- the same defects in a copy of release.yml ----------------------------------
SRC="$REL" WANT=write
REL_CHECKOUT_SHA="$(grep -Eo 'actions/checkout@[0-9a-f]{40}' "$REL" | head -1 | sed 's/.*@//')"
REL_SETUP_NODE_SHA="$(grep -Eo 'actions/setup-node@[0-9a-f]{40}' "$REL" | head -1 | sed 's/.*@//')"
[ -n "$REL_CHECKOUT_SHA" ] && [ -n "$REL_SETUP_NODE_SHA" ] || { echo "cannot find the checkout and setup-node pins in release.yml" >&2; exit 1; }
mutate "release.yml: an action on a tag" '$d ||= s/actions\/checkout@[0-9a-f]{40} # v[0-9.]+/actions\/checkout\@v6/' 'not pinned to a 40-hex'
mutate "release.yml: an upload-artifact on a major tag" '$d ||= s/actions\/upload-artifact@[0-9a-f]{40} # v[0-9.]+/actions\/upload-artifact\@v7/' 'not pinned to a 40-hex'
mutate "release.yml: a download-artifact on a branch" '$d ||= s/actions\/download-artifact@[0-9a-f]{40} # v[0-9.]+/actions\/download-artifact\@main/' 'not pinned to a 40-hex'
mutate "release.yml: a 39-hex SHA" "\$d ||= s/($REL_CHECKOUT_SHA)/substr(\$1, 0, 39)/e" 'not pinned to a 40-hex'
mutate "release.yml: an upper-case SHA" "\$d ||= s/($REL_CHECKOUT_SHA)/\\U\$1/" 'not pinned to a 40-hex'
mutate "release.yml: a pinned action with no version comment" '$d ||= s/(actions\/checkout@[0-9a-f]{40}) # v[0-9.]+/$1/' 'no `# vX.Y.Z` comment'
mutate "release.yml: a pinned action with a floating-major comment" '$d ||= s/(actions\/upload-artifact@[0-9a-f]{40}) # v[0-9.]+/$1 # v7/' 'no `# vX.Y.Z` comment'
mutate "release.yml: setup-node on Node 20 (v4)" "s/(actions\\/setup-node\\@$REL_SETUP_NODE_SHA) # v[0-9.]+/\$1 # v4.4.0/" 'targets Node 20'
mutate "release.yml: a uses key with a space before the colon" '$d ||= s/uses: actions\/download-artifact\@[0-9a-f]{40} # v[0-9.]+/uses : actions\/download-artifact\@v8/' 'a form this check does not read'
mutate "release.yml: no top-level permissions block" '$skip = 1 if /^permissions:$/; if ($skip) { $skip = 0 if /^  "contents": "write"$/; $_ = "" }' 'no top-level `permissions:` block'
mutate "release.yml: a permissions block that is only read" 's/^  "contents": "write"$/  "contents": "read"/' 'does not grant `contents: write`'
mutate "release.yml: a permissions block widened with id-token" 's/^  "contents": "write"$/  "contents": "write"\n  "id-token": "write"/' 'grants more than read'
mutate "release.yml: a permissions block widened with packages" 's/^  "contents": "write"$/  "contents": "write"\n  packages: write/' 'grants more than read'
mutate "release.yml: a write-all shorthand" 's/^permissions:$/permissions: write-all/' 'grants more than read'
mutate_ok "release.yml: the permissions keys spelled bare" 's/^  "contents": "write"$/  contents: write/'
unset SRC WANT

# --- classification mutations: a new workflow or action is never unseen ---------
classify_caught() {
    OUT="$(classify_workflows "$2" 2>&1)"; RC=$?
    if [ "$RC" -eq 1 ] && printf '%s\n' "$OUT" | grep -qE -- "$3"; then ok "caught: $1"; else bad "not caught ($1): exit $RC, wanted /$3/"; fi
}
repo_copy() {
    rm -rf "$WORK/repo"; mkdir -p "$WORK/repo"
    cp -R "$HERE/.github" "$WORK/repo/.github"
}
repo_copy
printf 'name: lint\npermissions:\n  contents: read\njobs:\n  l:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: actions/checkout@v4\n' >"$WORK/repo/.github/workflows/lint.yml"
classify_caught "a new workflow" "$WORK/repo" 'unclassified: .github/workflows/lint.yml'
repo_copy
mkdir -p "$WORK/repo/.github/actions/setup"
printf 'runs:\n  using: composite\n  steps:\n    - uses: actions/cache@v4\n' >"$WORK/repo/.github/actions/setup/action.yml"
classify_caught "a new composite action" "$WORK/repo" 'unclassified: .github/actions/setup/action.yml'
repo_copy
mv "$WORK/repo/.github/workflows/ci.yml" "$WORK/repo/.github/workflows/ci.yaml"
classify_caught "a checked workflow renamed to .yaml" "$WORK/repo" 'unclassified: .github/workflows/ci.yaml'
repo_copy
rm "$WORK/repo/.github/workflows/release.yml"
classify_caught "a checked workflow that is gone" "$WORK/repo" 'checked but missing: .github/workflows/release.yml'

# --- dependabot mutations ---------------------------------------------------------
dep_caught() {
    OUT="$(check_dependabot "$2" 2>&1)"; RC=$?
    if [ "$RC" -eq 1 ] && printf '%s\n' "$OUT" | grep -qE -- "$3"; then ok "caught: $1"; else bad "not caught ($1): exit $RC, wanted /$3/"; fi
}
f="$WORK/dep-cargo.yml"
{ cat "$DEP"; printf '  - package-ecosystem: cargo\n    directory: /\n    schedule:\n      interval: weekly\n'; } >"$f"
dep_caught "a second ecosystem (cargo)" "$f" '2 package-ecosystem entries'
f="$WORK/dep-npm.yml"
sed -E 's/package-ecosystem: github-actions/package-ecosystem: npm/' "$DEP" >"$f"
dep_caught "npm in place of github-actions" "$f" 'ecosystem is `npm`'
f="$WORK/dep-empty.yml"
printf 'version: 2\nupdates: []\n' >"$f"
dep_caught "no ecosystem at all" "$f" '0 package-ecosystem entries'
f="$WORK/dep-flow.yml"
{ cat "$DEP"; printf '  - {package-ecosystem: cargo, directory: /, schedule: {interval: weekly}}\n'; } >"$f"
dep_caught "a second ecosystem in flow style" "$f" '2 package-ecosystem entries'
f="$WORK/dep-quoted.yml"
{ cat "$DEP"; printf '  - "package-ecosystem": cargo\n'; } >"$f"
dep_caught "a second ecosystem with a quoted key" "$f" '2 package-ecosystem entries'
f="$WORK/dep-prefix.yml"
sed -E 's/package-ecosystem: github-actions/package-ecosystem: github-actions-foo/' "$DEP" >"$f"
dep_caught "an ecosystem that only starts with github-actions" "$f" 'ecosystem is `github-actions-foo`'
f="$WORK/dep-v20.yml"
sed -E 's/^version: 2/version: 20/' "$DEP" >"$f"
dep_caught "dependabot version 20" "$f" 'not `version: 2`'
f="$WORK/dep-nocool.yml"
grep -v -e 'cooldown:' -e 'default-days:' "$DEP" >"$f"
dep_caught "a github-actions entry with no cooldown" "$f" 'no `cooldown:` block'
f="$WORK/dep-zero.yml"
sed -E 's/default-days: 7/default-days: 0/' "$DEP" >"$f"
dep_caught "a cooldown of zero days" "$f" 'no `default-days` of at least 1'
f="$WORK/dep-empty-cool.yml"
grep -v 'default-days:' "$DEP" >"$f"
dep_caught "a cooldown block with no default-days" "$f" 'no `default-days` of at least 1'
f="$WORK/dep-commented-cool.yml"
sed -E 's/^( +)cooldown:/\1# cooldown:/; s/^( +)default-days:/\1# default-days:/' "$DEP" >"$f"
dep_caught "a cooldown that is only commented out" "$f" 'no `cooldown:` block'
f="$WORK/dep-v1.yml"
sed -E 's/^version: 2/version: 1/' "$DEP" >"$f"
dep_caught "dependabot version 1" "$f" 'not `version: 2`'

echo
echo "passed $PASS, failed $FAIL"
[ "$FAIL" -eq 0 ] || exit 1
exit 0
