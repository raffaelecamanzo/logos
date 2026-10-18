#!/usr/bin/env bash
#
# session-start.sh — the logos plugin's SessionStart hook: the quality readout,
# behind the minimum-binary-version handshake (FR-IN-07, FR-IN-11, ADR-81).
#
#   not enrolled (no .logos/)   exit 0, no output — and logos is never run
#   no logos on PATH            one message saying so, naming the minimum
#   logos older than minimum    one message naming both versions; no readout
#   version unreadable          one message saying so, naming the minimum
#   otherwise                   `logos --json quality-report --hook-json`
#
# Report-only: every path exits 0, so a session is never blocked. The readout
# itself is built in the binary with serde_json; this script only launches it.
# The handshake messages are the one JSON this script writes, through
# logos_hook_message, from fixed text and the versions it read.
#
# An older binary gets the handshake instead of the readout, not as well: the
# host reads stdout as JSON only when it is one object, so the two cannot be
# combined without parsing the binary's output here.
#
#   off-switch: export LOGOS_QUALITY_REPORT_DISABLE=1

set -euo pipefail

case "${BASH_SOURCE[0]}" in
    */*) HOOK_DIR="${BASH_SOURCE[0]%/*}" ;;
    *) HOOK_DIR=. ;;
esac
# shellcheck source=SCRIPTDIR/lib/common.sh
. "$HOOK_DIR/lib/common.sh"
# Every logos call this hook makes is stamped with it (FR-OB-15): `stats --json`
# counts the calls the plugin caused apart from the agent's own.
export LOGOS_INVOKER=hook:session-start

[ "${LOGOS_QUALITY_REPORT_DISABLE:-0}" = "1" ] && exit 0

PROJECT="$(logos_project_dir)"
logos_enrolled "$PROJECT" || exit 0

MIN="$(logos_min_binary_version)" || MIN=""
NEEDS="logos${MIN:+ $MIN or newer}"

if ! BIN="$(logos_bin)"; then
    logos_hook_message SessionStart \
        "logos: no logos binary was found on PATH. This plugin needs $NEEDS; install it, then start a new session." \
        "The logos plugin is enabled in this repository, but no logos binary is on PATH, so there is no quality readout and the logos MCP tools are unavailable. The plugin needs $NEEDS."
    exit 0
fi

if [ -n "$MIN" ]; then
    if ! VERSION="$(logos_binary_version "$BIN")"; then
        logos_hook_message SessionStart \
            "logos: could not read the version of $BIN. This plugin needs $NEEDS; the quality readout is skipped." \
            "The logos binary at $BIN did not report a version in the form 'logos X.Y.Z'. This plugin needs $NEEDS. The session-start quality readout was skipped."
        exit 0
    fi
    if ! logos_version_ge "$VERSION" "$MIN"; then
        logos_hook_message SessionStart \
            "logos: the binary on PATH ($BIN) is $VERSION, older than the $MIN this plugin needs. Upgrade logos; the quality readout is skipped until then." \
            "The logos binary on PATH is version $VERSION; this plugin needs $MIN or newer. The session-start quality readout was skipped, and plugin features that need the newer binary may fail until logos is upgraded."
        exit 0
    fi
fi

# Report on the project the host named; the hook's own cwd is not dependable.
cd "$PROJECT" 2>/dev/null || exit 0

# Not `exec`: a failure of the readout is swallowed, so a binary that cannot
# produce it degrades to silence rather than to a visible failed hook.
"$BIN" --json quality-report --hook-json 2>/dev/null || exit 0

exit 0
