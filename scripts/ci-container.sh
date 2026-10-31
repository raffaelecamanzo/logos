#!/usr/bin/env bash
#
# ci-container.sh — the in-container entry of `bash scripts/gate.sh ci` (S-806,
# NFR-MA-11, CR-251). It runs INSIDE the Linux container gate.sh starts, from the
# certified tree extracted at /work, and does the steps of .github/workflows/ci.yml's
# test job that a developer machine can hide a failure in:
#
#   1. the script suites        every scripts/tests/test-*.sh, all of them run
#                               even after one fails, as ci.yml's step does
#   2. the web UI build         `npm ci && npm run build` in web/ui, so the `web`
#                               tests run over the real hashed bundle
#   3. the workspace tests      `cargo test --workspace --no-fail-fast`, with
#                               LOGOS_PERF_TOLERANCE as the host passes it (CI's 3)
#
# THIS SCRIPT RECORDS, IT DOES NOT JUDGE. Each leg's combined output goes to
# $CI_OUT/<leg>.log and its exit status to $CI_OUT/<leg>.rc; gate.sh, on the host,
# counts binaries and tests from the logs and writes the evidence. So a leg that
# fails does not stop the next (one run names every red leg), the exit status here
# is non-zero only when the container could not be prepared, and a leg that never
# got to run leaves no .rc, which the host reads as a failure.
#
# THE LEGS RUN AS AN UNPRIVILEGED USER, as GitHub's runner does: a suite that
# checks a permission is refused (chmod 000, a read-only directory) passes as root
# and fails there. Preparation runs as root (packages, Node, the user); the legs
# re-enter this script as `ci` (CI_PHASE=legs).
#
# THE CARGO HOME IS EMPTY because the container is: gate.sh starts it with --rm and
# mounts only $CARGO_TARGET_DIR (a named volume) and $CI_OUT. Every dependency is
# fetched for THIS platform, as on GitHub's runner, which is what reproduces a test
# that asks for another platform's crates offline (HF-12).
#
# Env (set by gate.sh): CI_OUT, CARGO_TARGET_DIR, LOGOS_PERF_TOLERANCE (required);
#   CI_CARGO_JOBS (default 2: bounds the compile, which is what exhausts memory),
#   CI_TEST_THREADS (default 4: ubuntu-latest's core count).
# Usage (in the container, from the tree's root): bash scripts/ci-container.sh
# Exit:  0 the legs ran (read their .rc) · 1 the container could not be prepared

set -uo pipefail

: "${CI_OUT:?CI_OUT is required}"
: "${CARGO_TARGET_DIR:?CARGO_TARGET_DIR is required}"
: "${LOGOS_PERF_TOLERANCE:?LOGOS_PERF_TOLERANCE is required}"
JOBS="${CI_CARGO_JOBS:-2}"
THREADS="${CI_TEST_THREADS:-4}"
export LOGOS_PERF_TOLERANCE CARGO_TARGET_DIR CARGO_TERM_COLOR=never
# The legs run as the HOST's uid, so the output directory (0700, the host user's)
# is writable by them and by no one else, and what they leave is the host's own.
# A host root has no unprivileged uid to give: 1000 stands in.
CI_UID="${CI_UID:-1000}"
CI_GID="${CI_GID:-$CI_UID}"
[ "$CI_UID" != 0 ] || { CI_UID=1000 CI_GID=1000; }

# Node is what ci.yml's setup-node installs ("22"); the image carries none. An
# exact release and its checksum, so the toolchain is as pinned as the image is.
NODE_VERSION=22.23.4
NODE_SHA256=f7a1e486dbc8b286847815b477d4fb5e9fb963ed4489c3e1588853d685f589f9
NODE_TARBALL="node-v$NODE_VERSION-linux-arm64.tar.gz"
NODE_DIR="/opt/node-v$NODE_VERSION-linux-arm64"

mkdir -p "$CI_OUT" || exit 1

fail() { # message — the container cannot be prepared: the host sees no .rc
    echo "ci-container.sh: $1" >&2
    exit 1
}

# ----------------------------------------------------------------------- prepare
prepare() {
    local need=() cmd
    # zip builds the tools_optin fixtures; yaml is the parser the workflow suites
    # read with. GitHub's runner ships both, the rust image neither.
    for cmd in git perl python3 curl zip; do
        command -v "$cmd" >/dev/null 2>&1 || need+=("$cmd")
    done
    python3 -c 'import yaml' >/dev/null 2>&1 || need+=(python3-yaml)
    if [ "${#need[@]}" -gt 0 ]; then
        echo "installing: ${need[*]}"
        apt-get update -qq && apt-get install -y -qq --no-install-recommends "${need[@]}" ||
            fail "could not install ${need[*]}"
    fi

    curl -fsSL -o "/tmp/$NODE_TARBALL" "https://nodejs.org/dist/v$NODE_VERSION/$NODE_TARBALL" ||
        fail "could not download $NODE_TARBALL"
    echo "$NODE_SHA256  /tmp/$NODE_TARBALL" | sha256sum -c - >/dev/null ||
        fail "$NODE_TARBALL does not match its pinned sha256"
    tar -xzf "/tmp/$NODE_TARBALL" -C /opt || fail "could not unpack $NODE_TARBALL"
    [ -x "$NODE_DIR/bin/node" ] || fail "$NODE_TARBALL did not unpack to $NODE_DIR"

    # The unprivileged user, owning the tree it tests and the build volume's root
    # (not recursively: what an earlier run left there is already its own).
    useradd -m -u "$CI_UID" -U ci 2>/dev/null || useradd -m -u "$CI_UID" -g "$CI_GID" ci ||
        fail "could not create the ci user"
    chown -R "$CI_UID" /work && chown "$CI_UID" "$CARGO_TARGET_DIR" ||
        fail "could not hand the tree and the build volume to the ci user"
}

if [ "${CI_PHASE:-prepare}" = prepare ]; then
    # A subshell, so `fail`'s exit leaves it and the log is shown (the container's
    # output is the host's ci-container.log); the legs' own logs are never this one.
    ( prepare ) >"$CI_OUT/prep.log" 2>&1 || {
        cat "$CI_OUT/prep.log" >&2
        exit 1
    }
    # The legs, as `ci`, in a fresh environment but for what they need. `--clear-groups`
    # drops root's; the cargo home is the new user's own, empty directory.
    exec setpriv --reuid="$CI_UID" --regid="$CI_GID" --clear-groups \
        env CI_PHASE=legs HOME=/home/ci CARGO_HOME=/home/ci/.cargo \
        PATH="$NODE_DIR/bin:/usr/local/cargo/bin:$PATH" \
        bash scripts/ci-container.sh
fi

# ---------------------------------------------------------------- the legs, as ci
# The tree arrives without .git (it is `git archive` output). The script suites and
# the term gate ask git about the tree they run in, as on a CI checkout.
git init -q . && git add -A &&
    git -c user.name=ci -c user.email=ci@localhost commit -q -m "tree under test" ||
    fail "could not make the tree a git repository"
{
    echo "user    $(id -un) ($(id -u))"
    echo "rust    $(rustc --version)"
    echo "node    $(node --version)"
    echo "python  $(python3 --version)"
} >"$CI_OUT/legs-env.log" 2>&1

# leg <name> <command...> — run it with stdin closed, its output in $CI_OUT/<name>.log,
# and its exit status in $CI_OUT/<name>.rc written last (so an .rc is a finished leg).
leg() {
    local name="$1" rc
    shift
    "$@" </dev/null >"$CI_OUT/$name.log" 2>&1
    rc=$?
    echo "$rc" >"$CI_OUT/$name.rc"
    echo "leg $name: exit $rc"
}

# ------------------------------------------------------------------ script suites
# Every suite runs even after one fails. One suite's log and status each, named for
# the suite, so the host can match them to the tree's roster.
run_suites() {
    local suite name
    for suite in scripts/tests/test-*.sh; do
        [ -e "$suite" ] || break
        name="$(basename "$suite" .sh)"
        leg "script-suites--$name" bash "$suite"
    done
}

ui_build() {
    (cd web/ui && npm ci && npm run build)
}

workspace_tests() {
    cargo test --workspace --jobs "$JOBS" --no-fail-fast -- --test-threads="$THREADS"
}

run_suites
leg ui-build ui_build
leg test workspace_tests

exit 0
