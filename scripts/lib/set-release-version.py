#!/usr/bin/env python3
"""set-release-version.py — write one release version into the two files that carry it
(CR-272, S-855, FR-IN-11): the root Cargo.toml [workspace.package] version and the
top-level "version" of plugins/logos/.claude-plugin/plugin.json.

scripts/swe-build.sh calls this in its version step. Both edits are exact-match
substitutions on the text, never a round-trip through a parser, so every other byte of
both files is left as written (key order, indentation, inline arrays, the final
newline). For plugin.json only the top-level key is reachable: a "version" nested under
another key is indented deeper and never matches.

Nothing is left written unless both rewrites succeed and verify (a failing write is
rolled back): Cargo.toml must change
(or already carry the version) inside [workspace.package]; plugin.json must still parse,
carry the version at the top level, and equal the old document in every other key and in
key order. Otherwise both files stay as they were and the exit status is non-zero.

Usage: set-release-version.py ROOT X.Y.Z
Exit:  0 both files carry X.Y.Z, 1 a file was not rewritten (neither was touched),
       2 usage error
"""
import json
import os
import re
import sys

SEMVER = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+")
CARGO_VERSION = re.compile(r'(?m)(^\[workspace\.package\]\n(?:[^\[\n].*\n)*?version\s*=\s*")[^"]+(")')
# One level of indentation (spaces or a tab) is where a top-level member of the
# manifest sits; the pattern is anchored to the line start so a nested key never matches.
PLUGIN_VERSION = re.compile(r'^([ \t]+)"version"[ \t]*:[ \t]*"([^"\\]*)"', re.MULTILINE)

PLUGIN_JSON = os.path.join("plugins", "logos", ".claude-plugin", "plugin.json")


def rewrite_cargo(text: str, version: str) -> str:
    new, n = CARGO_VERSION.subn(lambda m: m.group(1) + version + m.group(2), text, count=1)
    if n != 1:
        raise ValueError("no [workspace.package] version to rewrite")
    return new


def rewrite_plugin(text: str, version: str) -> str:
    old = json.loads(text)
    if not isinstance(old, dict) or not isinstance(old.get("version"), str):
        raise ValueError('no top-level "version" string')
    found = list(PLUGIN_VERSION.finditer(text))
    if not found:
        raise ValueError('the "version" line was not found')
    shallowest = min(len(m.group(1)) for m in found)
    top = [m for m in found if len(m.group(1)) == shallowest]
    if len(top) != 1:
        raise ValueError('the top-level "version" key is ambiguous')
    new = text[: top[0].start(2)] + version + text[top[0].end(2):]
    expected = dict(old, version=version)
    check = json.loads(new)
    if check != expected or list(check) != list(old):
        raise ValueError("the substitution changed more than the version")
    return new


def put(path: str, text: str) -> None:
    with open(path, "w", encoding="utf-8", newline="") as f:
        f.write(text)


def write_all(plan) -> int:
    """Write every changed file of the plan; if one write fails, put back the files
    already written, so the pair is never half-bumped (a read-only file, a full disk)."""
    written = []
    try:
        for path, text, new in plan:
            if new != text:
                put(path, new)
                written.append((path, text))
    except OSError as e:
        for path, text in written:
            try:
                put(path, text)
            except OSError:
                print(f"set-release-version: could not restore {path}", file=sys.stderr)
        print(f"set-release-version: a write failed, nothing was changed: {e}", file=sys.stderr)
        return 1
    return 0


def main() -> int:
    if len(sys.argv) != 3 or not SEMVER.fullmatch(sys.argv[2]):
        print(__doc__.split("\n\n")[-1].strip(), file=sys.stderr)
        return 2
    root, version = sys.argv[1], sys.argv[2]
    plan = []
    for rel, fn in (("Cargo.toml", rewrite_cargo), (PLUGIN_JSON, rewrite_plugin)):
        path = os.path.join(root, rel)
        try:
            with open(path, encoding="utf-8", newline="") as f:
                text = f.read()
            plan.append((path, text, fn(text, version)))
        except (OSError, ValueError) as e:
            print(f"set-release-version: {rel} was not rewritten: {e}", file=sys.stderr)
            return 1
    return write_all(plan)


if __name__ == "__main__":
    sys.exit(main())
