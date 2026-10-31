#!/usr/bin/env bash
#
# test-contributing-docs.sh — prove the contributor-facing documents say what the
# repository does (S-727): the two issue forms are well-formed, and what
# CONTRIBUTING.md, docs/howto/contributing.md and docs/howto/README.md name
# exists. A document that points at a script, a flag, a heading, a CI job or a
# label that is not there sends a contributor the wrong way, and nothing else
# reads it.
#
# What is checked (every check prints `ok` or `FAIL`; the denominator is the last
# line, `passed N, failed M`):
#   - .github/ISSUE_TEMPLATE/{bug,feature}.yml, parsed as YAML: only the keys an
#     issue form allows, a non-empty string name and description, a list of string
#     labels (and assignees), no duplicate form name, and per body element a known
#     type, only that type's attributes with the right types, a string label, a
#     well-formed unique id, `validations` that are `{required: bool}`, markdown
#     elements with a value and without an id or validations, unique non-empty
#     dropdown options with an in-range default, checkbox options with a non-empty
#     label and no other key but `required`, and at least one interactive element;
#     CONTRIBUTING.md names each form by its `name`;
#   - in the three documents: every relative link resolves, and so does its
#     #anchor (against the target's headings, outside code fences); every repo
#     path named in a code span or fence, written repo-relative (no leading `./`)
#     under a top-level directory of the repository, exists; every `bash X.sh`
#     names a file that exists; every flag of scripts/publish-sprint.sh given in a
#     command that runs it (`\`-continuations joined), or alone in a code span of a
#     section that names the script, is one the script accepts (its case arms,
#     compared as whole flags); every exit code the howto restates is one the
#     script's header documents, and the code it gives for `--help` is the one the
#     script returns;
#   - CI jobs: every job in .github/workflows/ci.yml (its `name`, else its id; the
#     part before ` (`, the matrix) is named, as a code span, in CONTRIBUTING.md
#     and in the howto; and in a paragraph that names a job, every job-like span
#     (`a · b`, or a hyphenated word) is a job that exists;
#   - labels: the set is the howto's "Status labels" table, the one committed
#     place that defines them. Every label quoted in either contributing document
#     is in that set, and CONTRIBUTING.md quotes exactly that set. A label
#     misspelled the same way in both documents is not caught: nothing outside the
#     documents says what the labels are. Likewise an exit code is checked against
#     the set the header documents, not against what that case does;
#   - the howto's release flow lists `bash scripts/gate.sh ci` before
#     `bash scripts/publish-sprint.sh --sprint N --push` (S-807);
#   - `bash scripts/publish-sprint.sh --help` exits 2 and prints `--sprint`;
#   - the run was not vacuous: links, anchors, paths, bash targets, flags, jobs,
#     labels, forms, elements, exit codes and the release-flow order each counted
#     at least one check (an exit-code floor of three: the header, one restated
#     code, the `--help` claim).
#
# The suite has no negative fixtures of its own: its ability to fail was shown by
# editing a tree and running it (the optional ROOT argument names any tree).
#
# YAML is read with python3's yaml module, else ruby's (converted to JSON), as
# scripts/tests/test-release-workflow.sh does; CONTRIBUTING_DOCS_PARSER=ruby
# forces the ruby path.
#
# Usage: bash scripts/tests/test-contributing-docs.sh [ROOT]
# Exit:  0 every check held, 1 one did not (or a file could not be read)

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
ROOT="${1:-$HERE}"
[ -d "$ROOT" ] || { echo "cannot find $ROOT" >&2; exit 1; }
command -v python3 >/dev/null 2>&1 || { echo "python3 is required" >&2; exit 1; }

echo "test-contributing-docs.sh"
echo "  root $ROOT"
echo

# shellcheck disable=SC2016  # the python program is single-quoted on purpose.
python3 -I - "$ROOT" <<'PY'
import json, os, re, shutil, subprocess, sys

root = sys.argv[1]
os.chdir(root)

passed = failed = 0
cats = {}


def check(cat, name, cond, detail=""):
    global passed, failed
    cats[cat] = cats.get(cat, 0) + 1
    if cond:
        passed += 1
        print("  ok    " + name)
    else:
        failed += 1
        print("  FAIL  " + name + (": " + detail if detail else ""))


def read(path):
    try:
        with open(path, encoding="utf-8") as fh:
            return fh.read()
    except OSError as e:
        check("files", "read " + path, False, str(e))
        return ""


# One place decides how YAML is read: PyYAML, else ruby (JSON-converted).
try:
    import yaml
except ImportError:
    yaml = None
use_ruby = yaml is None or os.environ.get("CONTRIBUTING_DOCS_PARSER") == "ruby"
if use_ruby and not shutil.which("ruby"):
    print("no YAML parser: python3's yaml module or ruby is required", file=sys.stderr)
    sys.exit(1)


UNPARSED = object()


def load_yaml(path):
    """The file as parsed YAML (None for an empty document); UNPARSED, after one failed check, if it does not parse."""
    try:
        if not use_ruby:
            with open(path, encoding="utf-8") as fh:
                return yaml.safe_load(fh)
        r = subprocess.run(
            ["ruby", "-ryaml", "-rjson", "-e", "puts JSON.generate(YAML.safe_load(File.read(ARGV[0])))", path],
            capture_output=True, text=True, stdin=subprocess.DEVNULL, timeout=60)
        if r.returncode != 0:
            raise ValueError(r.stderr.strip())
        return json.loads(r.stdout)
    except Exception as e:  # unreadable or unparseable: one failed check, then skip the file
        check("files", "parse " + path, False, str(e))
        return UNPARSED


def spans(text):
    return re.findall(r"`([^`\n]+)`", text)


def nonempty_str(x):
    return isinstance(x, str) and bool(x.strip())


def str_list(x):
    return isinstance(x, list) and all(isinstance(i, str) for i in x)


# --- the issue forms ----------------------------------------------------------------
TOP = {"name", "description", "body", "assignees", "labels", "title", "projects", "type"}
ATTR = {
    "markdown": {"value"},
    "textarea": {"label", "description", "placeholder", "value", "render"},
    "input": {"label", "description", "placeholder", "value"},
    "dropdown": {"label", "description", "multiple", "options", "default"},
    "checkboxes": {"label", "description", "options"},
}
STRING_ATTR = {"label", "description", "placeholder", "value", "render"}
names = []
for form in ("bug", "feature"):
    path = ".github/ISSUE_TEMPLATE/%s.yml" % form
    d = load_yaml(path)
    if d is UNPARSED:
        continue
    if not isinstance(d, dict):
        check("forms", form + ": the form is a mapping", False, type(d).__name__)
        continue
    f = form + ": "
    check("forms", f + "only keys an issue form allows", not set(d) - TOP, str(set(d) - TOP))
    for k in ("name", "description"):
        check("forms", f + "`%s` is a non-empty string" % k, nonempty_str(d.get(k)), repr(d.get(k)))
    check("forms", f + "has `body`", "body" in d)
    if nonempty_str(d.get("name")):
        names.append(d["name"])
    if "title" in d:
        check("forms", f + "title is a string", isinstance(d["title"], str), repr(d["title"]))
    for k in ("labels", "assignees"):
        if k in d:
            check("forms", f + "%s is a list of strings" % k, str_list(d[k]), repr(d[k]))
    body = d.get("body")
    if not isinstance(body, list):
        check("forms", f + "body is a list", False, repr(body))
        continue
    ids, interactive = [], 0
    for i, e in enumerate(body):
        if not isinstance(e, dict):
            check("elements", f + "element %d is a mapping" % i, False, repr(e))
            continue
        t = e.get("type")
        w = f + "element %d (%s) " % (i, t)
        if not (isinstance(t, str) and t in ATTR):
            check("elements", w + "has a known type", False)
            continue
        a = e.get("attributes")
        a = {} if a is None else a
        if not isinstance(a, dict):
            check("elements", w + "attributes is a mapping", False, repr(a))
            continue
        check("elements", w + "attributes belong to its type", not set(a) - ATTR[t], str(set(a) - ATTR[t]))
        check("elements", w + "string attributes are strings",
              all(isinstance(a[k], str) for k in STRING_ATTR & set(a)), repr(a))
        check("elements", w + "keys are type/id/attributes/validations",
              not set(e) - {"type", "id", "attributes", "validations"}, str(set(e)))
        if t == "markdown":
            check("elements", w + "has a value and no id or validations",
                  nonempty_str(a.get("value")) and "validations" not in e and "id" not in e)
        else:
            interactive += 1
            if t != "checkboxes":
                check("elements", w + "label is a string", isinstance(a.get("label"), str))
        if "id" in e:
            ok = isinstance(e["id"], str) and re.fullmatch(r"[A-Za-z0-9_-]+", e["id"]) is not None
            check("elements", w + "id is well-formed", ok, repr(e["id"]))
            ids.append(e["id"] if ok else repr(e["id"]))
        v = e.get("validations")
        if v is not None:
            check("elements", w + "validations is {required: bool}",
                  isinstance(v, dict) and set(v) == {"required"} and isinstance(v["required"], bool), repr(v))
        if t == "dropdown":
            o = a.get("options")
            ok = bool(o) and isinstance(o, list) and all(isinstance(x, str) and x for x in o) and len(set(o)) == len(o)
            check("elements", w + "options are unique non-empty strings", ok, repr(o))
            if "multiple" in a:
                check("elements", w + "multiple is a bool", isinstance(a["multiple"], bool), repr(a["multiple"]))
            if "default" in a:
                dflt = a["default"]
                check("elements", w + "default is an index into options",
                      ok and isinstance(dflt, int) and not isinstance(dflt, bool) and 0 <= dflt < len(o), repr(dflt))
        if t == "checkboxes":
            opts = a.get("options")
            ok = isinstance(opts, list) and bool(opts) and all(
                isinstance(x, dict) and nonempty_str(x.get("label")) and not set(x) - {"label", "required"}
                and isinstance(x.get("required", False), bool) for x in opts)
            check("elements", w + "checkbox options are {label, required?}", ok, repr(opts))
    check("forms", f + "ids are unique", len(ids) == len(set(ids)), str(ids))
    check("forms", f + "has an interactive element", interactive > 0)
check("forms", "form names are unique", len(set(names)) == len(names), str(names))

# --- what the documents name --------------------------------------------------------
ps = read("scripts/publish-sprint.sh")
accepted = set()
for arm in re.findall(r"^\s+(-{1,2}[A-Za-z-]+(?:\s*\|\s*-{1,2}[A-Za-z-]+)*)\)", ps, re.M):
    accepted.update(re.split(r"\s*\|\s*", arm))
check("flags", "publish-sprint.sh accepts flags to compare against", "--sprint" in accepted, str(sorted(accepted)))
header = []  # the comment lines after "# Exit codes:", up to the first line that is not one
for ln in (ps.split("# Exit codes:", 1) + [""])[1].splitlines()[1:]:
    if not ln.startswith("#"):
        break
    header.append(ln)
header_exits = set(re.findall(r"^#\s+(\d)\s+\S", "\n".join(header), re.M))
check("exits", "publish-sprint.sh's header documents exit codes", bool(header_exits))

# The live --help run is read once: the flags and exit-code checks below use it.
help_rc = help_out = None
try:
    r = subprocess.run(["bash", "scripts/publish-sprint.sh", "--help"], capture_output=True, text=True,
                       stdin=subprocess.DEVNULL, timeout=60)
    help_rc, help_out = r.returncode, r.stdout + r.stderr
except (OSError, subprocess.TimeoutExpired) as e:
    check("help", "publish-sprint.sh --help runs", False, str(e))

ci = load_yaml(".github/workflows/ci.yml")
ci_jobs = set()
jobs = ci.get("jobs") if isinstance(ci, dict) else None
for jid, j in (jobs.items() if isinstance(jobs, dict) else ()):
    name = j.get("name") if isinstance(j, dict) else None
    ci_jobs.add((name if isinstance(name, str) else str(jid)).split(" (")[0])
check("jobs", "ci.yml defines jobs", bool(ci_jobs), str(ci)[:200])

DOCS = ["CONTRIBUTING.md", "docs/howto/contributing.md", "docs/howto/README.md"]
HOWTO = DOCS[1]
CONTRIB = DOCS[:2]
TEXT = {d: read(d) for d in DOCS}
JOB_LIKE = re.compile(r"[a-z0-9]+(?:-[a-z0-9]+)+(?: \(.*\))?|[^`]* · [^`]*")
LABEL = re.compile(r"status:\S+|change-request\S*")
# Top-level directories a path in a document may start with (not build output or hidden state).
TOP_DIRS = sorted(x for x in os.listdir(".") if os.path.isdir(x) and not x.startswith(".") and x not in ("target", "node_modules"))
TOP_DIRS.append(".github")
# A path is a token under one of those directories; a glob or placeholder (`test-*.sh`, `<n>`) is not a path.
PATH_TOKEN = re.compile(r"(?<![\w/.-])((?:\./)?(?:%s)/[A-Za-z0-9_./-]*[A-Za-z0-9_])(?![A-Za-z0-9_./-]*[*<>{}\[?])"
                        % "|".join(map(re.escape, TOP_DIRS)))
FENCE = re.compile(r"^ {0,3}(`{3,}|~{3,})[^\n]*\n(.*?)^ {0,3}\1[`~]*[ \t]*$", re.S | re.M)


def slug(h):
    h = re.sub(r"\[([^\]]*)\]\([^)]*\)", r"\1", h)  # a link in a heading contributes its text
    return re.sub(r"[^a-z0-9 _-]", "", h.lower()).strip().replace(" ", "-")


def strip_fences(t):
    return FENCE.sub("", t)


heads = {}


def anchors(path):
    """The anchors of a document's headings outside fences; a repeated heading gets -1, -2, ..."""
    if path not in heads:
        seen, out = {}, set()
        for h in re.findall(r"^#+\s+(.+)$", strip_fences(read(path)), re.M):
            s = slug(h)
            out.add(s if s not in seen else "%s-%d" % (s, seen[s]))
            seen[s] = seen.get(s, 0) + 1
        heads[path] = out
    return heads[path]


for doc in DOCS:
    t = TEXT[doc]
    base = os.path.dirname(doc)
    sp = spans(t)
    check("docs", doc + " is not empty", bool(t.strip()))
    prose = re.sub(r"`[^`\n]*`", "``", strip_fences(t))  # a link written as an example is not a link
    for m in sorted(set(re.findall(r"\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)", prose))):
        if re.match(r"[a-z][a-z0-9+.-]*:", m):
            continue
        path, _, frag = m.partition("#")
        tgt = os.path.normpath(path.lstrip("/") if path.startswith("/") else os.path.join(base, path)) if path else doc
        ok = os.path.exists(tgt)
        check("links", "%s: link %s resolves" % (doc, m), ok)
        if ok and frag and tgt.endswith(".md"):
            check("anchors", "%s: anchor %s exists" % (doc, m), frag in anchors(tgt))

    fences = [body for _, body in FENCE.findall(t)]
    toks, bashes = set(), set()
    for c in sp + fences:
        toks.update(PATH_TOKEN.findall(c))
        bashes.update(re.findall(r"\bbash\s+([^\s`*<>{}\[?]+\.sh)\b", c))
    for tok in sorted(toks):
        check("paths", "%s: path %s is repo-relative and exists" % (doc, tok),
              not tok.startswith("./") and os.path.exists(tok))
    for m in sorted(bashes):
        check("bash", "%s: bash target %s exists" % (doc, m), os.path.isfile(m))

    cmds = set()  # flags after publish-sprint.sh in a command (`\` continuations joined), up to the next command
    for cmd in re.sub(r"\\\n\s*", " ", t).splitlines():
        if "publish-sprint.sh" in cmd:
            rest = re.split(r"`|&&|;|\|", cmd.split("publish-sprint.sh", 1)[1])[0]
            cmds.update(re.findall(r"(?<![\w-])(--[A-Za-z0-9]+(?:-[A-Za-z0-9]+)*)", rest))
    for sec in re.split(r"^#+\s.*$", t, flags=re.M):  # a flag alone in a code span, in a section about the script
        if "publish-sprint.sh" in sec:
            cmds.update(c for c in spans(strip_fences(sec)) if re.fullmatch(r"-{1,2}[A-Za-z][A-Za-z0-9-]*", c))
    for fl in sorted(cmds):
        check("flags", "%s: flag %s is one publish-sprint.sh accepts" % (doc, fl), fl in accepted)

    if doc in CONTRIB:
        named = {s.split(" (")[0] for s in sp}
        for job in sorted(ci_jobs):
            check("jobs", "%s names the CI job `%s`" % (doc, job), job in named)
        for para in re.split(r"\n\s*\n", t):
            psp = spans(para)
            if any(s.split(" (")[0] in ci_jobs for s in psp):
                for s in sorted({s.split(" (")[0] for s in psp if JOB_LIKE.fullmatch(s)}):
                    check("jobs", "%s: the quoted job `%s` exists in ci.yml" % (doc, s), s in ci_jobs)

for form, n in zip(("bug", "feature"), names):
    check("forms", "CONTRIBUTING.md names the %s form `%s`" % (form, n), n in TEXT["CONTRIBUTING.md"])

# exit codes the howto restates: each is one the header documents; the `--help` one is the live one
for code in sorted(set(re.findall(r"\bexits?\s+(\d)\b", TEXT[HOWTO]))):
    check("exits", "howto exit code %s is documented in publish-sprint.sh's header" % code, code in header_exits)
claim = re.search(r"`--help`[^()]*\(and exits?\s+(\d)", TEXT[HOWTO])
check("exits", "the howto's exit code for `--help` is the one the script returns",
      bool(claim) and help_rc is not None and int(claim.group(1)) == help_rc,
      "claims %s, script returns %s" % (claim.group(1) if claim else None, help_rc))

# the release flow runs the Linux leg before it publishes (S-807): publish-sprint.sh --push
# refuses without that evidence, so the howto lists `gate.sh ci` first
flow = TEXT[HOWTO]
gate_at = flow.find("bash scripts/gate.sh ci")
push_at = flow.find("bash scripts/publish-sprint.sh --sprint N --push")
check("flow", "the howto's release flow lists `bash scripts/gate.sh ci` before `publish-sprint.sh --push`",
      0 <= gate_at < push_at, "gate.sh ci at %d, publish-sprint.sh --push at %d" % (gate_at, push_at))

# --- labels: the howto's table is the set -----------------------------------------------
sec = re.search(r"^## Status labels\n(.*?)(?=^## |\Z)", TEXT[HOWTO], re.S | re.M)
table = {s for s in spans(sec.group(1)) if LABEL.fullmatch(s)} if sec else set()
check("labels", "the howto's Status labels section defines labels", bool(table))
quoted = {}
for doc in CONTRIB:
    quoted[doc] = {s for s in spans(TEXT[doc]) if LABEL.fullmatch(s)}
    for lb in sorted(quoted[doc]):
        check("labels", "%s: label %s is in the howto's table" % (doc, lb), lb in table)
check("labels", "CONTRIBUTING.md quotes exactly the howto's label set",
      quoted["CONTRIBUTING.md"] == table, str(sorted(table)))

# --- the script the documents send a contributor to ----------------------------------------
if help_rc is not None:
    check("help", "publish-sprint.sh --help exits 2 and names --sprint",
          help_rc == 2 and "--sprint" in help_out, "exit %d" % help_rc)

# --- a run that checked nothing proves nothing -------------------------------------------------
FLOOR = {"links": 1, "anchors": 1, "paths": 1, "bash": 1, "flags": 1, "jobs": 1, "labels": 1,
         "forms": 1, "elements": 1, "exits": 3, "help": 1, "flow": 1}
for cat, floor in FLOOR.items():
    check("vacuity", "the run counted at least %d %s check(s)" % (floor, cat), cats.get(cat, 0) >= floor)
print()
print("categories: " + ", ".join("%s %d" % (k, v) for k, v in sorted(cats.items())))
print("passed %d, failed %d" % (passed, failed))
sys.exit(1 if failed else 0)
PY
