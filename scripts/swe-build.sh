#!/usr/bin/env bash
# swe-build.sh — build contract: see sprint-coordinator references/build-hook-contract.md
#
# Builds the version the human tests at the sprint-review gate: bump the workspace
# version, build the FULL binary (real UI bundle + `--features agents`), commit
# `release(X.Y.Z)`, install to ~/.logos-bin/X.Y.Z, PATH-promote it, and smoke the
# INSTALLED binary. Never tags or pushes — releasing stays with the user.
#
# Usage: swe-build.sh [--dry-run] [--bump patch|minor]
set -euo pipefail
DRY=0 BUMP=patch
while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run) DRY=1; shift ;;
    --bump)    BUMP="${2:-}"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
[[ "$BUMP" == patch || "$BUMP" == minor ]] || { echo "--bump must be patch or minor" >&2; exit 2; }
ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

# Single source of truth: root Cargo.toml [workspace.package] version (cli inherits it).
CUR="$(awk '/^\[workspace\.package\]/{f=1;next} /^\[/{f=0} f && /^version *=/{gsub(/[" ]/,"",$0); sub(/version=/,""); print; exit}' Cargo.toml)"
[[ "$CUR" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "cannot read [workspace.package] version (got '$CUR')" >&2; exit 1; }
IFS=. read -r MA MI PA <<<"$CUR"
if [[ "$BUMP" == minor ]]; then NEXT="$MA.$((MI + 1)).0"; else NEXT="$MA.$MI.$((PA + 1))"; fi

if [[ $DRY -eq 1 ]]; then
  echo "would build $NEXT (from $CUR, $BUMP bump)"
  echo "{\"status\":\"built\",\"version\":\"$NEXT\"}"
  exit 0
fi

BIN_ROOT="$HOME/.logos-bin/$NEXT"
ARTIFACT="$BIN_ROOT/bin/logos"
export CARGO_TARGET_DIR="$ROOT/target"

# 1. Version source + changelog: the coordinator writes release notes under
#    `## [Unreleased]`; they become this version's section.
python3 - "$NEXT" <<'PY'
import re, sys, datetime
nxt = sys.argv[1]
p = "Cargo.toml"; s = open(p).read()
s2 = re.sub(r'(\[workspace\.package\]\n(?:[^\[\n].*\n)*?version\s*=\s*")[^"]+(")', r'\g<1>' + nxt + r'\2', s, count=1)
if s2 == s: sys.exit("version not rewritten in Cargo.toml")
open(p, "w").write(s2)
p = "CHANGELOG.md"; c = open(p).read()
head = "## [Unreleased]\n"
if head not in c: sys.exit("CHANGELOG.md has no '## [Unreleased]' heading")
c = c.replace(head, head + "\n## [" + nxt + "] — " + datetime.date.today().isoformat() + "\n", 1)
open(p, "w").write(c)
PY

# 2. Real UI bundle — required even for a non-web sprint (the committed
#    dist/index.html is a stale placeholder).
( cd web/ui && npm ci && npm run build )
# 3. rust-embed may not recompile just because dist/ changed.
touch web/src/spa.rs
# 4. The full build the user runs.
cargo build -p logos --release --features agents

# 5. Release commit — version, changelog, lockfile only (dist/ stays unstaged).
git add Cargo.toml Cargo.lock CHANGELOG.md
git commit -q -m "release($NEXT)"

# 6. Install from the REAL bundle, reusing the workspace target dir.
cargo install --path cli --root "$BIN_ROOT" --locked --features agents --force

# 7. ONLY NOW restore the committed placeholder. Doing this before step 6 embeds
#    placeholder-shell + real assets = hash mismatch = blank page (hit twice).
git checkout HEAD -- web/ui/dist/index.html
rm -rf web/ui/dist/assets web/ui/dist/.vite web/ui/dist/vendor

# 8. PATH-promote (the PATH entry is a symlink; .mcp.json points at it unversioned).
mkdir -p "$HOME/.local/bin"
ln -sf "$ARTIFACT" "$HOME/.local/bin/logos"
hash -r

# 9. Smoke the INSTALLED binary over a tiny fixture project (the main repo's graph
#    needs minutes of reconcile before `serve --ui` binds).
SMOKE_DIR="$ROOT/target/swe-build-smoke"
rm -rf "$SMOKE_DIR"; mkdir -p "$SMOKE_DIR/src"
printf 'pub fn alpha() -> u32 { beta() }\npub fn beta() -> u32 { 1 }\n' > "$SMOKE_DIR/src/lib.rs"
( cd "$SMOKE_DIR" && git init -q && git add -A && git -c user.name=smoke -c user.email=smoke@localhost commit -qm fixture )
SMOKE=pass
if ! python3 "$ROOT/scripts/swe-build-smoke.py" --bin "$HOME/.local/bin/logos" \
      --expect-version "$NEXT" --project "$SMOKE_DIR" --port 4999; then
  SMOKE=fail
fi

echo "{\"status\":\"built\",\"version\":\"$NEXT\",\"artifact\":\"$ARTIFACT\",\"smoke\":\"$SMOKE\",\"notes\":\"~/.local/bin/logos -> $NEXT; run: logos serve --ui (in your project), then open http://127.0.0.1:4983\"}"
