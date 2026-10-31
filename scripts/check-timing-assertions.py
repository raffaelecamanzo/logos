#!/usr/bin/env python3
"""check-timing-assertions.py — fail on an elapsed-time assertion that bypasses
the one tolerance helper (S-804, NFR-MA-11).

WHY
  A wall-clock bound that holds on the machine it was written on fails on a
  loaded CI host (HF-11: `assert!(elapsed < Duration::from_millis(200))`, 212.9 ms
  on the shared runner). `logos_core::perf::scaled` widens a duration by
  `LOGOS_PERF_TOLERANCE` (default 1.15); this check makes "every elapsed-time
  upper bound goes through it" a property of the tree rather than a habit.

WHAT A SITE IS
  An `assert!` / `debug_assert!` invocation (any number of lines) whose condition
  holds a comparison between an elapsed time and something that is not one:
      assert!(start.elapsed() < Duration::from_secs(5));
      assert!(cfg!(coverage) || waited <= DEADLINE, "...");
      assert!(took.as_millis() < 500);
      assert!(report.duration_ms < 3_000);
  An elapsed time is `.elapsed()`, `duration_since(`, a `.duration_ms` /
  `.elapsed_ms`-style field, or a name the enclosing fn (or the file) bound from
  one by `let`, `const` or a plain assignment. A comparison against a duration
  constant (a `const` typed `Duration`, or named `*_MS`, `*_SECS`, `*_TIMEOUT`,
  `*_DEADLINE`, `*_BUDGET`) is a site whatever the other side is called. Both
  operands elapsed (a ratio of two measurements) is not a site.

  A deadline (`let deadline = Instant::now() + D`, `start + D` for an `Instant`
  `start`, or an `Instant` parameter) is the other operand of a liveness timeout,
  `assert!(Instant::now() < deadline)`: the duration it carries is `D`, so the
  scaling belongs on the line that builds it, `Instant::now() + scaled(D)`. An
  `Instant` parameter is scaled where its caller builds it.

  `assert_eq!`/`assert_ne!` and loop guards (`while start.elapsed() < d`,
  `if started.elapsed() > d { panic!() }`) are not assertions of a bound and are
  not read: the story leaves loop guards unchanged.

WHAT IS A HIT
  An UPPER bound (`elapsed < d`, `elapsed <= d`, `d > elapsed`, `!(elapsed > d)`)
  whose atom names no scaling: a call to `scaled(` / `scaled_ms(` / `tolerance(`,
  or a name bound from one. A LOWER bound (`waited >= grace`) is a site but never
  a hit: load only lengthens an elapsed time, so a minimum is not flaky, and
  scaling it would assert a wait the code never made.

  Syntactic, not semantic: the check reads text, and it reads a name's binding
  per fn, not per statement. A time that reaches a comparison through a function
  return, a closure parameter (`.all(|l| *l < D)`), a `for` or `if let` binding,
  or a fn parameter other than an `Instant` is not seen, and a name rebound
  through `scaled(` later in the fn reads as scaled throughout. Those are limits
  of the method, kept visible by `sites` in the summary line.

OUTPUT / EXIT
  One `path:line: ...` line per hit, then the denominator:
      timing-assertions: files=N sites=M scaled=S lower=L hits=H
  0 clean, 1 hits, 2 usage, 3 empty walk (no file, or no site, so a regex that
  matched nothing can never read as a pass).
"""
import argparse
import os
import re
import sys

SKIP_DIRS = {"target", "node_modules", ".git", ".worktrees", ".logos", ".agents", ".claude", "dist"}

ASSERT_RE = re.compile(r"(?<![\w!])(?:debug_)?assert!\s*\(")
FN_RE = re.compile(r"\bfn\s+\w+")
LET_RE = re.compile(r"\blet\s+(?:mut\s+)?([A-Za-z_]\w*)\s*(?::[^=;]*)?=(?![=>])")
LET_TUPLE_RE = re.compile(r"\blet\s+\(([^)=;]*)\)\s*(?::[^=;]*)?=(?![=>])")
CONST_RE = re.compile(r"\b(?:const|static)\s+([A-Za-z_]\w*)\s*:[^=;]*=(?![=>])")
DEADLINE_RHS_RE = re.compile(r"\bInstant::now\(\)\s*\+|\+\s*(?:std::time::)?Instant::now\(\)")
INSTANT_RHS_RE = re.compile(r"\bInstant::now\(\)")
INSTANT_PARAM_RE = re.compile(r"\b([A-Za-z_]\w*)\s*:\s*&?\s*(?:std::time::)?Instant\b")
# The char before the name's leading space may not be `:`: in `let x: Duration = ..`
# the type is not the thing assigned.
ASSIGN_RE = re.compile(r"(?<![\w.)\]<>=!+\-*/|&^%:])\s([A-Za-z_]\w*)\s*(?:[-+*/]?=)(?![=>])")
CONST_BOUND_RE = re.compile(r"\b(?:const|static)\s+([A-Z][A-Z0-9_]*)\s*:\s*([^=;]*)=")
BOUND_NAME_RE = re.compile(r"_(?:MS|SECS|MILLIS|TIMEOUT|DEADLINE|BUDGET)$")
# What a name binds from to become an elapsed time; a bare `Instant::now()` is a
# point in time, so it is a timing operand (TIMING_SRC_RE) but binds no timing name.
TAINT_SRC_RE = re.compile(
    r"\.elapsed\(\)|\bduration_since\(|\.(?:duration|elapsed|took)_(?:ms|us|secs|millis|nanos)\b"
)
TIMING_SRC_RE = re.compile(TAINT_SRC_RE.pattern + r"|\bInstant::now\(\)")
SCALING_RE = re.compile(r"\b(?:scaled|scaled_ms|tolerance)\s*\(")
WORD_RE = re.compile(r"[A-Za-z_]\w*")
# A name used as a name: `x` in `x.len()`, never `start` in `range.start`.
NAME_RE = re.compile(r"(?<![.\w])[A-Za-z_]\w*")
KEYWORDS = {"mut", "ref", "_"}
# A name that says what it holds: `let took = ran["duration_ms"]` reads a time
# although its source is a JSON key, which masking blanks.
TIMING_NAMES = {"took", "elapsed", "waited"}
# `(lo..hi).contains(&x)` bounds `x` above by `hi`.
RANGE_CONTAINS_RE = re.compile(r"\(([^()]*?)\.\.=?([^()]*)\)\s*\.contains\(\s*&?([^()]*)\)")


def raw_prefix_ok(src, i):
    """Whether the `r` at `i` opens a raw string (`r"`, `br"`, `cr"`) rather than
    ending an identifier such as `bar"`."""
    if i == 0:
        return True
    prev = src[i - 1]
    if prev in "bc" and (i < 2 or not (src[i - 2].isalnum() or src[i - 2] == "_")):
        return True
    return not (prev.isalnum() or prev == "_")


def _blank(out, a, b):
    for k in range(a, b):
        if out[k] != "\n":
            out[k] = " "


def _line_comment(src, out, i):
    j = src.find("\n", i)
    j = len(src) if j < 0 else j
    _blank(out, i, j)
    return j


def _block_comment(src, out, i):
    depth, j = 1, i + 2
    while j < len(src) and depth:
        if src.startswith("/*", j):
            depth, j = depth + 1, j + 2
        elif src.startswith("*/", j):
            depth, j = depth - 1, j + 2
        else:
            j += 1
    _blank(out, i, j)
    return j


def _raw_string(src, out, i):
    m = re.match(r'r(#*)"', src[i:i + 40])
    hashes, start = m.group(1), i + m.end()
    end = src.find('"' + hashes, start)
    end = len(src) if end < 0 else end
    _blank(out, start, end)
    return end + 1 + len(hashes)


def _string(src, out, i):
    j = i + 1
    while j < len(src) and src[j] != '"':
        j += 2 if src[j] == "\\" else 1
    _blank(out, i + 1, min(j, len(src)))
    return j + 1


def _char_or_lifetime(src, out, i):
    """A char literal ('x', '\\n', '\\'', '{') closes within a few characters; a
    lifetime ('a) does not, and is left alone."""
    n = len(src)
    if i + 1 < n and src[i + 1] == "\\":
        j = src.find("'", i + 2)
        if j > 0 and j - i <= 12:
            _blank(out, i + 1, j)
            return j + 1
    elif i + 2 < n and src[i + 2] == "'":
        _blank(out, i + 1, i + 2)
        return i + 3
    return i + 1


def mask(src):
    """`src` with comment text and string/char-literal contents blanked to spaces
    (newlines kept), so every later scan works on code alone and offsets map
    one-to-one back to the file."""
    out = list(src)
    i = 0
    while i < len(src):
        c = src[i]
        if src.startswith("//", i):
            i = _line_comment(src, out, i)
        elif src.startswith("/*", i):
            i = _block_comment(src, out, i)
        elif c == '"':
            i = _string(src, out, i)
        elif c == "r" and re.match(r'r#*"', src[i:i + 40]) and raw_prefix_ok(src, i):
            i = _raw_string(src, out, i)
        elif c == "'":
            i = _char_or_lifetime(src, out, i)
        else:
            i += 1
    return "".join(out)


def match_close(text, open_idx):
    """Index of the bracket closing the one at `open_idx`, or -1."""
    pairs = {"(": ")", "[": "]", "{": "}"}
    stack = []
    for k in range(open_idx, len(text)):
        ch = text[k]
        if ch in pairs:
            stack.append(pairs[ch])
        elif ch in ")]}":
            if not stack or stack.pop() != ch:
                return -1
            if not stack:
                return k
    return -1


def stmt_end(text, start):
    """End of the statement beginning at `start`: the next `;` at bracket depth 0."""
    depth = 0
    for k in range(start, len(text)):
        ch = text[k]
        if ch in "([{":
            depth += 1
        elif ch in ")]}":
            depth -= 1
            if depth < 0:
                return k
        elif ch == ";" and depth == 0:
            return k
    return len(text)


def _sep_at(text, k, seps):
    for sep in seps:
        if text.startswith(sep, k):
            return sep
    return None


def split_top(text, seps):
    """Split `text` on any of `seps` (strings) at bracket depth 0."""
    parts, depth, last, k = [], 0, 0, 0
    while k < len(text):
        ch = text[k]
        sep = _sep_at(text, k, seps) if depth == 0 and ch not in "([{)]}" else None
        if ch in "([{":
            depth += 1
        elif ch in ")]}":
            depth -= 1
        elif sep:
            parts.append(text[last:k])
            k += len(sep) - 1
            last = k + 1
        k += 1
    parts.append(text[last:])
    return parts


def strip_wrapping(atom):
    """(inner, negated): `!( a < b )` is `a < b`, negated; outer parens drop."""
    neg = False
    atom = atom.strip()
    while True:
        if atom.startswith("!") and not atom.startswith("!="):
            neg, atom = not neg, atom[1:].strip()
        elif atom.startswith("(") and match_close(atom, 0) == len(atom) - 1:
            atom = atom[1:-1].strip()
        else:
            return atom, neg


def atoms(cond):
    """The comparison atoms of a condition, split on top-level `&&` / `||`,
    descending into parenthesised groups. Yields (atom_text, negated)."""
    for part in split_top(cond, ["&&", "||"]):
        inner, neg = strip_wrapping(part)
        if inner != part.strip() and any(s in inner for s in ("&&", "||")):
            for sub, subneg in atoms(inner):
                yield sub, subneg != neg
        else:
            yield inner, neg


def comparison(atom):
    """(left, op, right) for the one top-level comparison in `atom`, or None."""
    rng = RANGE_CONTAINS_RE.fullmatch(atom.strip())
    if rng:
        return rng.group(3), "<", rng.group(2)
    depth, gdepth, k = 0, 0, 0
    while k < len(atom):
        ch = atom[k]
        if ch in "([{":
            depth += 1
        elif ch in ")]}":
            depth -= 1
        elif depth == 0 and ch in "<>":
            two = atom[k:k + 2]
            prev = atom[k - 1] if k else " "
            if ch == ">" and prev in "-=":  # `->`, `=>`
                k += 1
                continue
            if two in ("<<", ">>") or two == "<-":
                k += 2
                continue
            if ch == "<" and (atom[max(0, k - 2):k] == "::" or re.search(r"[A-Z]\w*$", atom[:k])):
                gdepth += 1  # a generic argument list, never a comparison
                k += 1
                continue
            if ch == ">" and gdepth:
                gdepth -= 1
                k += 1
                continue
            op = two if two in ("<=", ">=") else ch
            return atom[:k], op, atom[k + len(op):]
        k += 1
    return None


class Scope:
    """Identifier facts bound inside one fn body (or at file level): names bound
    from an elapsed time, from a scaling call, and instants (`Instant::now()`)
    that are a point in time rather than a duration."""

    def __init__(self):
        self.timing, self.scaled, self.instants = set(), set(), set()


def _body_end(text, k):
    """End of the fn body whose signature starts scanning at `k`, or -1: the body's
    `{` is the first one outside the parameter list, and `;` first means no body."""
    depth = 0
    while k < len(text):
        ch = text[k]
        if ch in "(<[":
            depth += 1
        elif ch in ")>]":
            depth -= 1
        elif ch == ";" and depth == 0:
            return -1
        elif ch == "{" and depth <= 0:
            return match_close(text, k)
        k += 1
    return -1


def fn_spans(text):
    spans = []
    for m in FN_RE.finditer(text):
        end = _body_end(text, m.end())
        if end > 0:
            spans.append((m.start(), end))
    return spans


def _tuple_bindings(text, m):
    """`let (a, b) = (x, y)` binds each name to its own element; any other
    right-hand side gives every name the whole of it."""
    rhs = text[m.end():stmt_end(text, m.end())]
    inner = rhs.strip()
    pattern = [t for t in m.group(1).split(",") if t.strip()]
    names = [WORD_RE.findall(t) for t in pattern]
    names = [[n for n in ns if n not in KEYWORDS] for ns in names]
    is_literal = inner.startswith("(") and match_close(inner, 0) == len(inner) - 1
    parts = split_top(inner[1:-1], [","]) if is_literal else []
    if len(parts) != len(names):
        yield m.start(), [n for ns in names for n in ns], rhs
        return
    for ns, part in zip(names, parts):
        yield m.start(), ns, part


def bindings(text):
    """(offset, [names], rhs) for every let / const / assignment."""
    for rx in (LET_RE, CONST_RE, ASSIGN_RE):
        for m in rx.finditer(text):
            yield m.start(), [m.group(1)], text[m.end():stmt_end(text, m.end())]
    for m in LET_TUPLE_RE.finditer(text):
        yield from _tuple_bindings(text, m)


def _owner_index(spans, off):
    """Index of the innermost fn span holding `off`, or None."""
    best = None
    for idx, (a, b) in enumerate(spans):
        if a <= off <= b and (best is None or a >= spans[best][0]):
            best = idx
    return best


def _absorb(owner, file_scope, names, rhs):
    """Fold one binding into its scope; whether any name set grew."""
    words = set(NAME_RE.findall(rhs))
    visible_instants = owner.instants | file_scope.instants
    measured = (
        bool(TAINT_SRC_RE.search(rhs))
        or bool(words & (owner.timing | file_scope.timing))
        or bool(TIMING_NAMES & set(names))
    )
    # A deadline (`Instant::now() + D`, `start + D`) and an instant
    # (`let start = Instant::now()`) are points in time, never elapsed
    # durations: they are the bound's side, not its measurement.
    is_deadline = bool(DEADLINE_RHS_RE.search(rhs)) or any(
        re.search(r"\b%s\s*\+" % re.escape(w), rhs) for w in words & visible_instants)
    if is_deadline or (INSTANT_RHS_RE.search(rhs) and not measured):
        target = owner.instants
    else:
        target = owner.timing if measured else None
    grew = False
    if target is not None and not set(names) <= target:
        target.update(names)
        grew = True
    if (SCALING_RE.search(rhs) or words & (owner.scaled | file_scope.scaled)) and not set(names) <= owner.scaled:
        owner.scaled.update(names)
        grew = True
    return grew


def build_scopes(text, spans):
    """One Scope per fn span plus the file scope, taint closed to a fixpoint."""
    file_scope = Scope()
    fn_scopes = [Scope() for _ in spans]
    found = []
    for off, names, rhs in bindings(text):
        idx = _owner_index(spans, off)
        found.append((file_scope if idx is None else fn_scopes[idx], names, rhs))
    # An `Instant` parameter is built by the caller, who scales it there.
    for idx, (a, _b) in enumerate(spans):
        sig = text[a:text.find("{", a)]
        fn_scopes[idx].scaled.update(INSTANT_PARAM_RE.findall(sig))
    for _ in range(4):
        if not any([_absorb(owner, file_scope, names, rhs) for owner, names, rhs in found]):
            break
    return file_scope, fn_scopes


def classify(side, timing_words):
    return bool(TIMING_SRC_RE.search(side)) or bool(set(NAME_RE.findall(side)) & timing_words)


def _judge(atom, neg, timing_words, scaled_words, bounds):
    """The kind of the comparison in `atom` ("scaled", "UNSCALED" or
    "lower bound"), or None when it bounds no elapsed time."""
    cmp_ = comparison(atom)
    if cmp_ is None:
        return None
    left, op, right = cmp_
    lt, rt = classify(left, timing_words), classify(right, timing_words)
    if not lt and not rt:
        # Neither side is visibly a time, but a duration constant on one side
        # makes the other the thing it bounds.
        lb = bool(set(NAME_RE.findall(left)) & bounds)
        rb = bool(set(NAME_RE.findall(right)) & bounds)
        lt, rt = (rb, lb) if lb != rb else (lt, rt)
    if lt == rt:  # no elapsed time, or a ratio of two measurements
        return None
    upper = (op in ("<", "<=")) == lt  # elapsed on the small side of the bound
    if neg:
        upper = not upper
    if not upper:
        return "lower bound"
    is_scaled = bool(SCALING_RE.search(atom)) or bool(set(WORD_RE.findall(atom)) & scaled_words)
    return "scaled" if is_scaled else "UNSCALED"


def _words_at(offset, spans, file_scope, fn_scopes):
    """(timing names, scaled names) visible at `offset`: the file's and those of
    every fn holding it."""
    timing, scaled = set(file_scope.timing), set(file_scope.scaled)
    for idx, (a, b) in enumerate(spans):
        if a <= offset <= b:
            timing |= fn_scopes[idx].timing
            scaled |= fn_scopes[idx].scaled
    return timing, scaled


def _holds_time(src):
    """Sound prefilter: a site needs an assert and a time source or duration
    constant (every timing name is bound from one), so a file with neither holds
    no site."""
    return "assert!" in src and any(
        t in src for t in (".elapsed()", "duration_since(", "Instant", "Duration", "_ms", "_MS"))


def scan_file(path, rel, listing):
    """(sites, scaled, lower, hits[(line, text)]) for one file."""
    with open(path, encoding="utf-8", errors="replace") as fh:
        src = fh.read()
    if not _holds_time(src):
        return 0, 0, 0, []
    text = mask(src)
    spans = fn_spans(text)
    file_scope, fn_scopes = build_scopes(text, spans)
    bounds = {m.group(1) for m in CONST_BOUND_RE.finditer(text)
              if "Duration" in m.group(2) or BOUND_NAME_RE.search(m.group(1))}
    counts = {"scaled": 0, "lower bound": 0, "UNSCALED": 0}
    hits = []
    for m in ASSERT_RE.finditer(text):
        close = match_close(text, m.end() - 1)
        if close < 0:
            continue
        cond = split_top(text[m.end():close], [","])[0]
        timing_words, scaled_words = _words_at(m.start(), spans, file_scope, fn_scopes)
        line = src.count("\n", 0, m.start()) + 1
        for atom, neg in atoms(cond):
            kind = _judge(atom, neg, timing_words, scaled_words, bounds)
            if kind is None:
                continue
            counts[kind] += 1
            shown = " ".join(atom.split())
            shown = shown if len(shown) <= 110 else shown[:107] + "..."
            if listing is not None:
                listing.append("%s:%d: %s: %s" % (rel, line, kind, shown))
            if kind == "UNSCALED":
                hits.append((line, shown))
    return sum(counts.values()), counts["scaled"], counts["lower bound"], hits


def walk(root):
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = sorted(d for d in dirnames if d not in SKIP_DIRS and not os.path.islink(os.path.join(dirpath, d)))
        for name in sorted(filenames):
            full = os.path.join(dirpath, name)
            if name.endswith(".rs") and not os.path.islink(full):
                yield full


def main(argv):
    ap = argparse.ArgumentParser(description="Fail on an elapsed-time assertion that bypasses the tolerance helper.")
    ap.add_argument("--root", default=os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."),
                    help="tree to scan (default: this repository)")
    ap.add_argument("--list", action="store_true", help="print every site with its classification")
    args = ap.parse_args(argv)
    root = os.path.abspath(args.root)
    if not os.path.isdir(root):
        print("check-timing-assertions.py: not a directory: %s" % root, file=sys.stderr)
        return 2
    files = sites = scaled = lower = 0
    hits = []
    listing = [] if args.list else None
    for full in walk(root):
        rel = os.path.relpath(full, root)
        s, sc, lo, h = scan_file(full, rel, listing)
        files += 1
        sites += s
        scaled += sc
        lower += lo
        hits.extend((rel, line, shown) for line, shown in h)
    for entry in listing or []:
        print(entry)
    for rel, line, shown in hits:
        print("%s:%d: elapsed-time upper bound not scaled through logos_core::perf::scaled: %s" % (rel, line, shown))
    print("timing-assertions: files=%d sites=%d scaled=%d lower=%d hits=%d" % (files, sites, scaled, lower, len(hits)))
    if files == 0 or sites == 0:
        return 3
    return 1 if hits else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
