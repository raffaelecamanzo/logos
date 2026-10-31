#!/usr/bin/env python3
"""check-offline-cargo.py — fail on an offline cargo call in a test source that
needs crates a one-host build never downloads (S-805, NFR-MA-11).

WHY
  `cargo metadata --offline` resolves the dependency graph of EVERY target
  platform unless it is told which one: Android's crates included, which a build
  on one host never fetches. It passed on the machine that wrote it and failed on
  CI's runner (HF-12). `cargo tree --offline --target all` asks for the same
  all-platform resolution. This check makes "an offline call names its platform"
  a property of the tree, as check-timing-assertions.py does for elapsed-time
  bounds.

WHAT A SITE IS
  A cargo invocation in a `.rs` file that carries `--offline`, read from its string
  literals. A statement (the tokens between `;`, `{` and `}`) holding a literal
  `"metadata"` or `"tree"` is one invocation of that subcommand (the first such
  literal), and every literal of the statement is an argument of it, whichever side
  of the subcommand it sits on:
      cargo(&["metadata", "--offline", "--format-version", "1"])
      Command::new(c).args(["tree", "--offline"])
      Command::new(c).arg("--offline").arg("metadata")
  A literal that is a shell command line is read per command (split on newline, `&&`,
  `||`, `;` and `|`): a command that starts `cargo` (or `$CARGO`, or a path ending in
  `/cargo`), skips `+toolchain` and flags, and reaches `metadata` or `tree`:
      "cargo tree --offline"    "cargo metadata --no-deps && cargo tree --offline"
  Prose that mentions a command is no site when it does not start with `cargo`, or is
  a doc attribute or the text of `.expect(..)` / `panic!(..)`; an assertion message
  that opens with `cargo metadata --offline` would be read as a command. Comments are
  not read. A call without
  `--offline` is not a site: it may use the network, so it is not the failure this
  guards.

WHAT IS A HIT
  - `metadata --offline` with neither `--filter-platform` nor `--target` (either
    spelling: `--filter-platform host` / `--filter-platform=host`). `--no-deps`
    is exempt: it resolves no graph, so no target's crates are needed.
  - `tree --offline` with `--target all` (or `--target=all`). Plain
    `cargo tree --offline` resolves the host only and passes, as does
    `--target <triple>`.

  Syntactic, not semantic: the check reads literals. An argument pushed onto the
  list in a later statement (`args.push("--target")`) or an argument list held in a
  variable is not seen, and the tool is not checked to be cargo (`uv tree --offline`
  reads as a cargo site); `sites` in the summary line keeps that reach visible.

OUTPUT / EXIT
  One `path:line: ...` line per hit, then the denominator:
      offline-cargo: files=N sites=M metadata=A tree=T hits=H
  0 clean, 1 hits, 2 usage, 3 empty walk (no file, or no site, so a scanner that
  matched nothing can never read as a pass).
"""
import argparse
import os
import re
import sys

SKIP_DIRS = {"target", "node_modules", ".git", ".worktrees", ".logos", ".agents", ".claude", "dist"}
SUBCOMMANDS = ("metadata", "tree")
SHELL_SPLIT_RE = re.compile(r"\n|&&|\|\||[;|]")
SHELL_CARGO_RE = re.compile(r'^(?:.*/)?cargo$|^"?\$\{?CARGO\}?"?$')
RAW_OPEN_RE = re.compile(r'b?r(#*)"')
MESSAGE_MACROS = ("panic", "unreachable", "todo", "unimplemented")
ESCAPES = {"n": "\n", "t": "\t", "r": "\r"}
IDENT_CHAR = re.compile(r"[A-Za-z0-9_]")


# Each scanner takes (text, i, line) and returns None when text[i] does not start
# its construct, else (token or None, next i, next line).
def scan_space(text, i, line):
    if not text[i].isspace():
        return None
    return None, i + 1, line + (text[i] == "\n")


def scan_line_comment(text, i, line):
    if not text.startswith("//", i):
        return None
    end = text.find("\n", i)
    return None, len(text) if end < 0 else end, line


def scan_block_comment(text, i, line):
    if not text.startswith("/*", i):
        return None
    depth, n = 0, len(text)
    while i < n:
        if text.startswith("/*", i):
            depth, i = depth + 1, i + 2
        elif text.startswith("*/", i):
            depth, i = depth - 1, i + 2
            if depth == 0:
                break
        else:
            line, i = line + (text[i] == "\n"), i + 1
    return None, i, line


def scan_raw_string(text, i, line):
    if text[i] not in "rb" or (i > 0 and IDENT_CHAR.match(text[i - 1])):
        return None
    m = RAW_OPEN_RE.match(text, i)
    if not m:
        return None
    close = '"' + m.group(1)
    end = text.find(close, m.end())
    end = len(text) if end < 0 else end
    return ("str", text[m.end():end], line), min(len(text), end + len(close)), line + text.count("\n", i, end)


def scan_string(text, i, line):
    if text[i] != '"':
        return None
    j, n, buf, start = i + 1, len(text), [], line
    while j < n and text[j] != '"':
        if text[j] == "\\" and j + 1 < n and text[j + 1] == "\n":
            # A line continuation drops the newline and the indent after it.
            line, j = line + 1, j + 2
            while j < n and text[j] in " \t\r\n":
                line, j = line + (text[j] == "\n"), j + 1
            continue
        if text[j] == "\\" and j + 1 < n:
            buf.append(ESCAPES.get(text[j + 1], text[j + 1]))
            j += 2
            continue
        line += text[j] == "\n"
        buf.append(text[j])
        j += 1
    return ("str", "".join(buf), start), j + 1, line


def scan_char_literal(text, i, line):
    """A char literal ('"', '\\n', 'x') or a lifetime ('a): only the first may hold a
    quote that would open a bogus string."""
    if text[i] != "'":
        return None
    n = len(text)
    if i + 2 < n and text[i + 1] == "\\":
        j = text.find("'", i + 3)
        return None, n if j < 0 else j + 1, line
    return None, i + (3 if i + 2 < n and text[i + 2] == "'" else 1), line


def scan_ident(text, i, line):
    if not IDENT_CHAR.match(text[i]):
        return None
    j = i
    while j < len(text) and IDENT_CHAR.match(text[j]):
        j += 1
    return ("id", text[i:j], line), j, line


SCANNERS = (scan_space, scan_line_comment, scan_block_comment, scan_raw_string,
            scan_string, scan_char_literal, scan_ident)


def tokenize(text):
    """Return a list of (kind, value, line): 'str' literals (unescaped enough to
    compare), 'id' identifiers and 'p' single punctuation characters. Comments,
    char literals and whitespace are dropped."""
    i, line, out = 0, 1, []
    while i < len(text):
        for scan in SCANNERS:
            step = scan(text, i, line)
            if step:
                break
        else:
            step = (("p", text[i], line), i + 1, line)
        tok, i, line = step
        if tok:
            out.append(tok)
    return out


def statements(toks):
    """The tokens between statement boundaries: `;`, `{` and `}`."""
    cur = []
    for tok in toks:
        if tok[0] == "p" and tok[1] in ";{}":
            if cur:
                yield cur
            cur = []
        else:
            cur.append(tok)
    if cur:
        yield cur


def shell_commands(literal):
    """(subcommand, words) of each command of a shell line that runs cargo
    metadata or cargo tree."""
    for segment in SHELL_SPLIT_RE.split(literal):
        words = segment.split()
        if not words or not SHELL_CARGO_RE.match(words[0]):
            continue
        sub = next((w for w in words[1:] if not w.startswith(("-", "+"))), None)
        if sub in SUBCOMMANDS:
            yield sub, words[1:]


def has_flag(args, *names):
    return any(a == n or a.startswith(n + "=") for a in args for n in names)


def target_all(args):
    return any(a == "--target=all" for a in args) or any(
        a == "--target" and k + 1 < len(args) and args[k + 1] == "all" for k, a in enumerate(args)
    )


def judge(sub, args):
    """The reason an offline `sub` call with these literal args is a hit, or None."""
    if sub == "metadata":
        if has_flag(args, "--no-deps", "--filter-platform", "--target"):
            return None
        return "`cargo metadata --offline` without --filter-platform or --target resolves every target platform's crates"
    if target_all(args):
        return "`cargo tree --offline --target all` resolves every target platform's crates"
    return None


def is_message(stmt, k):
    """Whether stmt[k] is the text of `.expect("..")` or `panic!("..")`: prose that
    may quote a command, not run one."""
    if k < 2 or stmt[k - 1][:2] != ("p", "("):
        return False
    before = stmt[k - 2]
    if before[:2] == ("id", "expect"):
        return True
    return before[:2] == ("p", "!") and k >= 3 and stmt[k - 3][1] in MESSAGE_MACROS


def shell_invocations(stmt):
    """(line, subcommand, words) of each shell-line literal of the statement."""
    if stmt[0][:2] == ("p", "#"):
        return  # an attribute: `#[doc = "cargo metadata --offline .."]` is prose
    for k, (kind, val, line) in enumerate(stmt):
        if kind == "str" and not is_message(stmt, k):
            for sub, words in shell_commands(val):
                yield line, sub, words


def invocations(toks):
    """(line, subcommand, args) of each cargo metadata / tree call in the tokens."""
    for stmt in statements(toks):
        strs = [(val, line) for kind, val, line in stmt if kind == "str"]
        first = next((k for k, (val, _) in enumerate(strs) if val in SUBCOMMANDS), None)
        if first is not None:
            yield strs[first][1], strs[first][0], [v for k, (v, _) in enumerate(strs) if k != first]
        yield from shell_invocations(stmt)


def scan_file(path):
    """-> (sites, metadata, tree, hits[(line, reason)])"""
    try:
        with open(path, encoding="utf-8", errors="replace") as fh:
            toks = tokenize(fh.read())
    except OSError:
        return 0, 0, 0, []
    sites = meta = tree = 0
    hits = []
    for line, sub, args in invocations(toks):
        if "--offline" not in args:
            continue
        sites += 1
        meta += sub == "metadata"
        tree += sub == "tree"
        reason = judge(sub, args)
        if reason:
            hits.append((line, reason))
    return sites, meta, tree, hits


def is_rust_file(path):
    return path.endswith(".rs") and not os.path.islink(path)


def walk(root):
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = sorted(d for d in dirnames if d not in SKIP_DIRS and not os.path.islink(os.path.join(dirpath, d)))
        yield from filter(is_rust_file, (os.path.join(dirpath, name) for name in sorted(filenames)))


def main(argv):
    ap = argparse.ArgumentParser(description="Fail on an offline cargo call that resolves every target platform.")
    ap.add_argument("--root", default=os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."),
                    help="tree to scan (default: this repository)")
    args = ap.parse_args(argv)
    root = os.path.abspath(args.root)
    if not os.path.isdir(root):
        print("check-offline-cargo.py: not a directory: %s" % root, file=sys.stderr)
        return 2
    files = sites = meta = tree = 0
    hits = []
    for full in walk(root):
        rel = os.path.relpath(full, root)
        s, m, t, h = scan_file(full)
        files += 1
        sites += s
        meta += m
        tree += t
        hits.extend((rel, line, reason) for line, reason in h)
    for rel, line, reason in hits:
        print("%s:%d: %s" % (rel, line, reason))
    print("offline-cargo: files=%d sites=%d metadata=%d tree=%d hits=%d" % (files, sites, meta, tree, len(hits)))
    if files == 0 or sites == 0:
        return 3
    return 1 if hits else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
