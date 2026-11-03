#!/usr/bin/env bash
# swe-build.sh — build contract: see sprint-coordinator references/build-hook-contract.md
#
# Builds the version the human tests at the sprint-review gate: bump the workspace
# version (and the plugin's, in plugin.json), build the FULL binary (real UI bundle, default features), commit
# `release(X.Y.Z)`, install to ~/.logos-bin/X.Y.Z and smoke the INSTALLED binary.
# Never tags or pushes — releasing stays with the user. It never touches PATH
# either: `logos` on PATH is the released one (Homebrew), which the gates, the git
# hooks and the Claude Code plugin run, so a sprint build is run by its path. Run
# against this repository it migrates .logos/logos.db under the PATH binary: test it
# on another project or a fixture.
#
# Publishing is a separate, later step: `scripts/publish-public.sh --version X.Y.Z`
# turns the release(X.Y.Z) tree into one commit over origin/main, gated by
# scripts/check-public-terms.sh, and with --push tags it vX.Y.Z and pushes both.
# The tag push starts .github/workflows/release.yml, which checks out the tag and
# builds with `dist --tag=vX.Y.Z`: the version this script writes to Cargo.toml
# and to plugins/logos/.claude-plugin/plugin.json must match the tag, or the workflow refuses it (and publish-public.sh refuses a
# tag that disagrees with it first). See docs/howto/ci-integration.md.
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
#    The plugin's version is the release's (CR-272): Cargo.toml and plugin.json are
#    written together by one helper, or neither is and the build stops here, before
#    anything is committed.
python3 scripts/lib/set-release-version.py "$ROOT" "$NEXT"
python3 - "$NEXT" <<'PY'
import sys, datetime
nxt = sys.argv[1]
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
cargo build -p logos --release

# 5. Release commit — versions (Cargo.toml and the plugin's plugin.json), changelog,
#    lockfile only (dist/ stays unstaged).
git add Cargo.toml plugins/logos/.claude-plugin/plugin.json Cargo.lock CHANGELOG.md
git commit -q -m "release($NEXT)"

# 6. Install from the REAL bundle, reusing the workspace target dir.
cargo install --path cli --root "$BIN_ROOT" --locked --force

# 7. ONLY NOW restore the committed placeholder. Doing this before step 6 embeds
#    placeholder-shell + real assets = hash mismatch = blank page (hit twice).
git checkout HEAD -- web/ui/dist/index.html
rm -rf web/ui/dist/assets web/ui/dist/.vite web/ui/dist/vendor

# 8. Smoke the INSTALLED binary over a tiny fixture project (the main repo's graph
#    needs minutes of reconcile before `serve --ui` binds).
SMOKE_DIR="$ROOT/target/swe-build-smoke"
rm -rf "$SMOKE_DIR"; mkdir -p "$SMOKE_DIR/src"
printf 'pub fn alpha() -> u32 { beta() }\npub fn beta() -> u32 { 1 }\n' > "$SMOKE_DIR/src/lib.rs"
( cd "$SMOKE_DIR" && git init -q && git add -A && git -c user.name=smoke -c user.email=smoke@localhost commit -qm fixture )
SMOKE=pass
if ! python3 "$ROOT/scripts/swe-build-smoke.py" --bin "$ARTIFACT" \
      --expect-version "$NEXT" --project "$SMOKE_DIR" --port 4999; then
  SMOKE=fail
fi

echo "{\"status\":\"built\",\"version\":\"$NEXT\",\"artifact\":\"$ARTIFACT\",\"smoke\":\"$SMOKE\",\"notes\":\"PATH unchanged (the released logos); run: $ARTIFACT serve --ui (in another project), then open http://127.0.0.1:4983\"}"
