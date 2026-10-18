#!/usr/bin/env bash
#
# mcp-serve.sh — the logos plugin's MCP server launcher (FR-IN-11, FR-MC-01).
#
# Runs `logos serve --mcp` in an enrolled repository with a logos binary, and
# nothing anywhere else. `logos serve` creates a .logos/ store in whatever
# directory it starts in, and a plugin enabled at user scope starts its server in
# every project the host opens: without this check, opening a session in any
# directory would enrol it, and from then on every hook would treat it as an
# enrolled repository.
#
#   enrolled, logos on PATH     exec logos serve --mcp, in the project directory
#   not enrolled (no .logos/)   an empty MCP server: no tools, no instructions
#   enrolled, no logos on PATH  an empty MCP server whose instructions say so
#
# Why an empty server and not an exit: a server that closes before the MCP
# handshake is recorded by the host as failed under its name alone, with no
# project in the key, and skipped in the NEXT projects too — including enrolled
# ones — until that record expires. Answering the handshake keeps the server
# healthy everywhere. The empty server reads one JSON-RPC message per line (the
# stdio transport), answers initialize, tools/list and ping, refuses any other
# request, ignores notifications, and exits 0 at end of input. It never runs
# logos and writes nothing.

set -euo pipefail

case "${BASH_SOURCE[0]}" in
    */*) SCRIPT_DIR="${BASH_SOURCE[0]%/*}" ;;
    *) SCRIPT_DIR=. ;;
esac
# shellcheck source=SCRIPTDIR/../hooks/lib/common.sh
. "$SCRIPT_DIR/../hooks/lib/common.sh"

# serve_empty [INSTRUCTIONS] — the empty MCP server on stdin/stdout.
serve_empty() {
    local line method id version info=""
    [ -n "${1:-}" ] && info=",\"instructions\":$(logos_json_string "$1")"
    while IFS= read -r line || [ -n "$line" ]; do
        [[ $line =~ \"method\"[[:space:]]*:[[:space:]]*\"([^\"]*)\" ]] || continue
        method="${BASH_REMATCH[1]}"
        # No id: a notification, which takes no reply.
        [[ $line =~ \"id\"[[:space:]]*:[[:space:]]*(\"[^\"]*\"|-?[0-9]+) ]] || continue
        id="${BASH_REMATCH[1]}"
        case "$method" in
            initialize)
                version=2025-06-18
                if [[ $line =~ \"protocolVersion\"[[:space:]]*:[[:space:]]*\"([0-9A-Za-z.-]+)\" ]]; then
                    version="${BASH_REMATCH[1]}"
                fi
                printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"%s","capabilities":{"tools":{}},"serverInfo":{"name":"logos","version":"unavailable"}%s}}\n' \
                    "$id" "$version" "$info"
                ;;
            tools/list) printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[]}}\n' "$id" ;;
            ping) printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id" ;;
            *) printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32601,"message":"Method not found"}}\n' "$id" ;;
        esac
    done
    exit 0
}

PROJECT="$(logos_project_dir)"
logos_enrolled "$PROJECT" || serve_empty
BIN="$(logos_bin)" || serve_empty "The logos plugin is enabled in this repository, but no logos binary is on PATH, so the logos MCP tools are unavailable. Install logos, then start a new session."
cd "$PROJECT"
exec "$BIN" serve --mcp
