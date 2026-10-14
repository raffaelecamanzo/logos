#!/usr/bin/env bash
#
# release-dist-config.sh — give a checked-out release tag the cargo-dist
# configuration it needs to build, when the tag predates it (S-732).
#
#   bash scripts/release-dist-config.sh --from PIPELINE_DIR [--into TAG_DIR]
#
# .github/workflows/release.yml builds the tag it is run for. A dispatch can name
# a tag cut while this repository carried no release pipeline (v1.16.2; v1.16.1
# has the same shape but is superseded and not released): its tree has no dist-workspace.toml, no `[package.metadata.dist]` in
# cli/Cargo.toml and no `[profile.dist]` in Cargo.toml, so `dist` finds nothing to
# build. Every job that runs `dist` checks out the workflow's own commit beside the
# tag (PIPELINE_DIR) and runs this script, which supplies each missing piece from
# there and touches nothing the tag already has:
#
#   dist-workspace.toml       copied when the tag has none
#   [profile.dist]            appended to Cargo.toml when the tag has none
#   [package.metadata.dist]   appended to cli/Cargo.toml when the tag has none
#
# A table is copied as PIPELINE_DIR spells it: from its header line down to the
# next table header, without the comment and blank lines that open the next one.
# On a tag that carries the pipeline (every tag after it returned, and the pull
# request runs) all three are already there and the script changes nothing. It
# never edits a version, a dependency or a feature list, so the build is the tag's
# own source and default features.
#
# Prints one line per piece: `kept` (the tag has it) or `added` (supplied).
# Exit: 0 done; 1 PIPELINE_DIR lacks a piece to supply; 2 usage.

set -euo pipefail

FROM=""
INTO="."

usage() {
    echo "usage: bash scripts/release-dist-config.sh --from PIPELINE_DIR [--into TAG_DIR]" >&2
    exit 2
}

while [ $# -gt 0 ]; do
    case "$1" in
        --from | --into)
            [ $# -ge 2 ] || usage
            if [ "$1" = --from ]; then FROM="$2"; else INTO="$2"; fi
            shift 2
            ;;
        *) usage ;;
    esac
done
[ -n "$FROM" ] || usage
for d in "$FROM" "$INTO"; do
    [ -d "$d" ] || { echo "release-dist-config.sh: not a directory: $d" >&2; exit 2; }
done
for f in Cargo.toml cli/Cargo.toml; do
    [ -f "$INTO/$f" ] || { echo "release-dist-config.sh: $INTO has no $f" >&2; exit 2; }
done

missing() {
    echo "release-dist-config.sh: $FROM has no $1 to supply" >&2
    exit 1
}

# table <file> <header>: the table under <header>, as described above. Empty
# when the file has no such header line.
table() {
    awk -v want="$2" '
        function is_header(l) { return l ~ /^[ \t]*\[/ }
        on && is_header($0) { exit }
        $0 == want { on = 1 }
        on { buf[++n] = $0 }
        END {
            while (n > 0 && buf[n] ~ /^[ \t]*(#.*)?$/) n--
            for (i = 1; i <= n; i++) print buf[i]
        }' "$1"
}

if [ -f "$INTO/dist-workspace.toml" ]; then
    echo "dist-workspace.toml: kept"
else
    [ -f "$FROM/dist-workspace.toml" ] || missing dist-workspace.toml
    /bin/cp "$FROM/dist-workspace.toml" "$INTO/dist-workspace.toml"
    echo "dist-workspace.toml: added"
fi

# supply <file> <header>
supply() {
    local file="$1" header="$2" block
    if grep -Fxq -- "$header" "$INTO/$file"; then
        echo "$file $header: kept"
        return
    fi
    [ -f "$FROM/$file" ] || missing "$file"
    block="$(table "$FROM/$file" "$header")"
    [ -n "$block" ] || missing "$header in $file"
    printf '\n# Supplied by scripts/release-dist-config.sh from the release workflow'"'"'s commit.\n%s\n' \
        "$block" >>"$INTO/$file"
    echo "$file $header: added"
}

supply Cargo.toml '[profile.dist]'
supply cli/Cargo.toml '[package.metadata.dist]'
