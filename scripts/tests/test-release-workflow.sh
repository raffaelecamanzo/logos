#!/usr/bin/env bash
#
# test-release-workflow.sh — prove .github/workflows/release.yml is wired the way
# docs/howto/ci-integration.md says (S-732): the workflow cannot be run outside
# GitHub, so its structure is what can be checked here.
#
# The workflow is parsed as YAML (python3's yaml module, else ruby's, converted to
# JSON), and these hold:
#   - a `workflow_dispatch` takes a required string `tag` input;
#   - in `plan`, the guard (scripts/release-tag-guard.sh, id `guard`) runs on every
#     publishing run and comes before dist is installed and before any step that
#     uploads; the jobs' `tag`, `tag-flag` and `commit` outputs come from it only;
#   - `plan` and `host` check out the tag, every build job the commit `plan`
#     validated, and every job that runs dist runs scripts/release-dist-config.sh
#     first;
#   - in `host`, the term gate (scripts/check-release-text.sh) over the title, the
#     notes, the formula, the installer and dist-manifest.json, and the guard again (with --commit, so
#     a tag moved during the run is refused), both come before the GitHub Release
#     is created, which uses --verify-tag and the guard's commit;
#   - the formula and announce jobs cannot run after host refused or failed;
#   - build-spa's web-ui-dist bundle is fetched into web/ui/dist before every local
#     `dist build`, so an archive embeds the real web UI, not the placeholder;
#   - dist is installed in `plan` and `build-local-artifacts` by scripts/install-dist.sh
#     (checked, after the release pipeline checkout and before any `dist` line), no
#     `run:` line pipes a download into a shell or runs the matrix's own install
#     command, and a container matrix entry is refused by a step that ends in
#     `exit 1` (HF-4);
#   - no `dist` invocation selects cargo features;
#   - no `${{ }}` expression in a `run:` line reads the dispatch input, the event
#     payload, the triggering ref or `env.`, however it is spelled.
#
# Each check prints `ok` or `FAIL`; the denominator is the last line.
#
# Usage: bash scripts/tests/test-release-workflow.sh [WORKFLOW]
# Exit:  0 every check held, 1 one did not (or the file could not be parsed)

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WF="${1:-$HERE/.github/workflows/release.yml}"
[ -f "$WF" ] || { echo "cannot find $WF" >&2; exit 1; }
command -v python3 >/dev/null 2>&1 || { echo "python3 is required" >&2; exit 1; }

if python3 -c 'import yaml' >/dev/null 2>&1; then
    JSON="$(python3 -c 'import json, sys, yaml; print(json.dumps(yaml.safe_load(open(sys.argv[1])), default=str))' "$WF")" ||
        { echo "the workflow does not parse as YAML: $WF" >&2; exit 1; }
    PARSER="python3 yaml"
elif command -v ruby >/dev/null 2>&1; then
    JSON="$(ruby -ryaml -rjson -e 'puts JSON.generate(YAML.safe_load(File.read(ARGV[0])))' "$WF")" ||
        { echo "the workflow does not parse as YAML: $WF" >&2; exit 1; }
    PARSER="ruby yaml"
else
    echo "no YAML parser: python3's yaml module or ruby is required" >&2
    exit 1
fi

echo "test-release-workflow.sh"
echo "  workflow $WF (parsed with $PARSER)"
echo

# shellcheck disable=SC2016  # the python program is single-quoted on purpose.
printf '%s' "$JSON" | python3 -c '
import json, re, sys

wf = json.load(sys.stdin)
passed = failed = 0

def check(name, cond, detail=""):
    global passed, failed
    if cond:
        passed += 1
        print("  ok    " + name)
    else:
        failed += 1
        print("  FAIL  " + name + (": " + detail if detail else ""))

# YAML 1.1 reads a bare `on` as true.
on = next((wf[k] for k in ("on", True, "true", "True") if k in wf), None) or {}
jobs = wf.get("jobs", {})

def steps(job):
    return jobs.get(job, {}).get("steps", [])

def index(job, pred):
    for i, s in enumerate(steps(job)):
        if pred(s):
            return i
    return -1

def runs(s, needle):
    return needle in (s.get("run") or "")

def uses(s, action):
    return (s.get("uses") or "").startswith(action + "@")

def strict(s, allowed_if=None):
    """A refusal step that cannot be defeated: its run is one command line with no
    operator after it (`|| true`, `; :`, `&`, a pipe), it never continues on error,
    and it has no `if:` beyond the one it needs."""
    lines = [l for l in (s.get("run") or "").splitlines() if l.strip() and not l.strip().startswith("#")]
    one = len(lines) == 1 and not any(op in lines[0] for op in ("||", "&&", ";", "&", "|"))
    # `if: false` parses as a boolean, so presence is what counts, not truthiness.
    cond = str(s["if"]).replace(" ", "") if "if" in s else None
    return one and "continue-on-error" not in s and cond == allowed_if

# --- the triggers ---------------------------------------------------------------
tag_in = ((on.get("workflow_dispatch") or {}).get("inputs") or {}).get("tag") or {}
check("a dispatch takes a required string `tag` input",
      tag_in.get("required") is True and tag_in.get("type") == "string", str(tag_in))
check("a pushed version tag still starts the run",
      "**[0-9]+.[0-9]+.[0-9]+*" in ((on.get("push") or {}).get("tags") or []))
group = (wf.get("concurrency") or {}).get("group", "")
check("one run per tag at a time, for a push and a dispatch alike",
      "inputs.tag" in group and "github.ref_name" in group, group)

# --- plan: the guard first --------------------------------------------------------
g = index("plan", lambda s: s.get("id") == "guard")
guard = steps("plan")[g] if g >= 0 else {}
check("plan runs scripts/release-tag-guard.sh as the `guard` step",
      runs(guard, "scripts/release-tag-guard.sh \"$TAG\""), str(guard))
check("the guard runs on every run that publishes",
      (guard.get("if") or "").replace(" ", "") == "${{github.event_name!=\x27pull_request\x27}}",
      str(guard.get("if")))
check("the guard reads the tag from its environment, never from `run:` interpolation",
      "${{" not in (guard.get("run") or "")
      and "inputs.tag" in (guard.get("env") or {}).get("TAG", "")
      and "github.ref_name" in (guard.get("env") or {}).get("TAG", ""))
check("the plan guard\x27s step cannot pass on a refusal (one command, no continue-on-error, no other if:)",
      strict(guard, "${{github.event_name!=\x27pull_request\x27}}"), str(guard))
inst = index("plan", lambda s: (s.get("name") or "") == "Install dist")
up = index("plan", lambda s: uses(s, "actions/upload-artifact"))
pl = index("plan", lambda s: s.get("id") == "plan")
check("the guard comes before dist is installed, anything uploads and dist plans",
      0 <= g < inst and g < up and g < pl, "guard %d, install %d, upload %d, plan %d" % (g, inst, up, pl))
outs = jobs.get("plan", {}).get("outputs", {})
check("the tag, tag-flag and commit outputs come from the guard alone",
      all("steps.guard.outputs." in outs.get(k, "") and "github.ref_name" not in outs.get(k, "")
          and "inputs." not in outs.get(k, "") for k in ("tag", "tag-flag", "commit")), str(outs))
check("publishing is every run but a pull request",
      outs.get("publishing", "").replace(" ", "") == "${{github.event_name!=\x27pull_request\x27}}")
plan_step = steps("plan")[pl] if pl >= 0 else {}
check("dist hosts the guard\x27s tag, or only plans",
      "steps.guard.outputs.tag" in (plan_step.get("run") or "") and "github.ref_name" not in (plan_step.get("run") or ""))

# --- every job that builds builds the tag ---------------------------------------
building = ["plan", "build-spa", "build-local-artifacts", "build-global-artifacts", "host"]
for job in building:
    co = [s for s in steps(job) if uses(s, "actions/checkout") and not (s.get("with") or {}).get("path")]
    ref = (co[0].get("with") or {}).get("ref", "") if co else ""
    if job in ("plan", "host"):
        want = "inputs.tag" if job == "plan" else "needs.plan.outputs.tag"
        check("%s checks out the tag" % job, len(co) == 1 and want in ref and "refs/tags/" in ref, ref)
    else:
        check("%s checks out the commit plan validated, not the tag by name" % job,
              len(co) == 1 and ref.replace(" ", "") == "${{needs.plan.outputs.commit||\x27\x27}}", ref)
for job in ["plan", "build-local-artifacts", "build-global-artifacts", "host"]:
    side = index(job, lambda s: uses(s, "actions/checkout") and (s.get("with") or {}).get("path") == ".release-pipeline"
                 and (s.get("with") or {}).get("ref") == "${{ github.sha }}")
    cfg = index(job, lambda s: runs(s, "scripts/release-dist-config.sh --from .release-pipeline"))
    first_dist = index(job, lambda s: any(l.strip().startswith("dist ") for l in (s.get("run") or "").splitlines()))
    check("%s supplies the dist configuration before it runs dist" % job,
          0 <= side < cfg < first_dist, "pipeline %d, config %d, dist %d" % (side, cfg, first_dist))

# --- host: the term gate and the guard before the Release -------------------------
create = index("host", lambda s: runs(s, "gh release create"))
text = index("host", lambda s: runs(s, "scripts/check-release-text.sh"))
again = index("host", lambda s: runs(s, "scripts/release-tag-guard.sh"))
text_run = (steps("host")[text].get("run") or "") if text >= 0 else ""
check("host runs the term gate over the title, the notes, the formula, the installer and the manifest",
      all(f in text_run for f in ("title.txt", "notes.txt", "artifacts/*.rb", "artifacts/*-installer.sh",
                                  "artifacts/dist-manifest.json")), text_run)
check("the term gate and the guard both come before the GitHub Release is created",
      0 <= text < create and 0 <= again < create, "text %d, guard %d, create %d" % (text, again, create))
ag = steps("host")[again] if again >= 0 else {}
check("host\x27s term gate and guard steps cannot pass on a refusal",
      text >= 0 and strict(steps("host")[text]) and strict(ag), str([steps("host")[text] if text >= 0 else {}, ag]))
hif = (jobs.get("host", {}).get("if") or "").replace(" ", "")
check("host runs only on a run that publishes",
      "needs.plan.outputs.publishing==\x27true\x27" in hif, hif)
check("host\x27s guard refuses a tag that moved away from the validated commit",
      "--commit \"$COMMIT\"" in (ag.get("run") or "")
      and (ag.get("env") or {}).get("COMMIT") == "${{ needs.plan.outputs.commit }}", str(ag))
cs = steps("host")[create] if create >= 0 else {}
create_line = next((l for l in (cs.get("run") or "").splitlines() if l.strip().startswith("gh release create")), "")
check("the Release goes on the existing tag, at the guard\x27s commit",
      "--verify-tag" in create_line and "--target \"$RELEASE_COMMIT\"" in create_line
      and (cs.get("env") or {}).get("RELEASE_COMMIT") == "${{ needs.plan.outputs.commit }}"
      and "github.sha" not in json.dumps(cs.get("env") or {}))
check("host\x27s guard and the Release read the tag from plan\x27s guard output only",
      (ag.get("env") or {}).get("TAG") == "${{ needs.plan.outputs.tag }}"
      and (cs.get("env") or {}).get("TAG") == "${{ needs.plan.outputs.tag }}"
      and "gh release create \"$TAG\"" in create_line, str([ag.get("env"), cs.get("env")]))
check("the formula is pushed only after host succeeded",
      "host" in (jobs.get("publish-homebrew-formula", {}).get("needs") or []))
# A job whose `if:` carries a status function (always(), failure(), cancelled())
# runs after a failed `needs`, so it must test the success of host itself.
for job in ("publish-homebrew-formula", "announce"):
    jif = str(jobs.get(job, {}).get("if") or "").replace(" ", "")
    status_fn = any(f in jif for f in ("always(", "failure(", "cancelled("))
    check("%s cannot run after host refused or failed" % job,
          "host" in (jobs.get(job, {}).get("needs") or [])
          and (not status_fn or "needs.host.result==\x27success\x27" in jif), jif)

# --- the archives embed the SPA build-spa built -----------------------------------
spa_up = [s for s in steps("build-spa") if uses(s, "actions/upload-artifact")]
check("build-spa uploads web/ui/dist as web-ui-dist",
      any((s.get("with") or {}).get("name") == "web-ui-dist" and (s.get("with") or {}).get("path") == "web/ui/dist"
          for s in spa_up), str(spa_up))
check("build-local-artifacts waits for build-spa",
      "build-spa" in (jobs.get("build-local-artifacts", {}).get("needs") or []))
fetch = index("build-local-artifacts", lambda s: uses(s, "actions/download-artifact")
              and (s.get("with") or {}).get("name") == "web-ui-dist" and (s.get("with") or {}).get("path") == "web/ui/dist")
local_dist = index("build-local-artifacts", lambda s: any(l.strip().startswith("dist build")
                                                         for l in (s.get("run") or "").splitlines()))
check("build-local-artifacts fetches web-ui-dist into web/ui/dist before dist builds",
      0 <= fetch < local_dist, "fetch %d, dist build %d" % (fetch, local_dist))

# --- dist is installed verified, and no run: pipes a download into a shell (HF-4) ---
# Every job that runs `dist` installs it with scripts/install-dist.sh, taken from the
# `.release-pipeline` checkout and run before the first `dist` line, as one command.
# A job that gets dist from the cached artifact (`host`, `build-global-artifacts`) runs
# no installer and is not asked for one.
def release_pipeline_checkout(s):
    return uses(s, "actions/checkout") and (s.get("with") or {}).get("path") == ".release-pipeline"

def runs_dist(s):
    return any(l.strip().startswith("dist ") for l in (s.get("run") or "").splitlines())

for job in ("plan", "build-local-artifacts"):
    co = index(job, release_pipeline_checkout)
    ins = index(job, lambda s: (s.get("name") or "") == "Install dist")
    first = index(job, runs_dist)
    step = steps(job)[ins] if ins >= 0 else {}
    body = [l.strip() for l in (step.get("run") or "").splitlines() if l.strip() and not l.strip().startswith("#")]
    check("%s installs dist with scripts/install-dist.sh, as the one command of its `Install dist` step" % job,
          body == ["bash .release-pipeline/scripts/install-dist.sh"] and "continue-on-error" not in step and "if" not in step,
          str(step))
    check("%s installs dist after the release pipeline is checked out and before it runs dist" % job,
          0 <= co < ins < first, "checkout %d, install %d, first dist %d" % (co, ins, first))

pipe = re.compile(r"\b(curl|wget)\b[^\n]*\|\s*(sudo\s+)?(\S*/)?(ba|z|da)?sh\b|\b(ba|z|da)?sh\b\s+-c\s+[\"\x27]?\$\(\s*(curl|wget)\b")
piped = [(j, s.get("name") or s.get("id") or "?", l.strip()) for j in jobs for s in steps(j)
         for l in (s.get("run") or "").splitlines() if not l.strip().startswith("#") and pipe.search(l)]
check("no run: line pipes a download into a shell", not piped, str(piped))
matrix_cmd = [(j, s.get("name") or "?") for j in jobs for s in steps(j) if "matrix.install_dist" in (s.get("run") or "")]
check("no run: line executes the matrix\x27s own dist install command, which pipes a download into `sh`", not matrix_cmd, str(matrix_cmd))

# A matrix entry that runs in a container would build without Rust: the step that
# once piped rustup into a shell is a refusal, under the same `if:`, that fails.
ref = [s for s in steps("build-local-artifacts") if "matrix.container" in str(s.get("if", ""))]
ref_body = [l.strip() for l in ((ref[0].get("run") or "") if ref else "").splitlines() if l.strip() and not l.strip().startswith("#")]
check("build-local-artifacts refuses a container matrix entry: exactly one step under `if: matrix.container`, which ends in `exit 1`",
      len(ref) == 1 and str(ref[0]["if"]).replace(" ", "") == "${{matrix.container}}"
      and bool(ref_body) and ref_body[-1] == "exit 1" and not any("||" in l for l in ref_body)
      and "continue-on-error" not in ref[0] and "rustup" not in " ".join(ref_body), str(ref))

# --- features come only from [package.metadata.dist] ------------------------------
feat = [(j, l.strip()) for j in jobs for s in steps(j) for l in (s.get("run") or "").splitlines()
        if l.strip().startswith("dist ")
        and any(f in l for f in ("--features", "--all-features", "--no-default-features"))]
check("no dist invocation selects cargo features (they come from [package.metadata.dist])", not feat, str(feat))

# --- nothing interpolates untrusted refs into a shell -----------------------------
# Every ${{ ... }} expression in every run:, however it is spelled or composed:
# none may read the dispatch input, the event payload or the ref, nor `env.`, which
# can carry any of them in from the env: of a step (read it as $VAR in the shell instead).
untrusted = re.compile(r"\binputs\.|github\.event\.|github\.ref\b|github\.ref_name|github\.head_ref|\benv\.")
bad = [(j, s.get("name") or s.get("id") or "?", e) for j in jobs for s in steps(j)
       for e in re.findall(r"\$\{\{(.*?)\}\}", s.get("run") or "", re.S) if untrusted.search(e)]
check("no run: expression reads the dispatch input, the event payload, the ref or env.", not bad, str(bad))

print()
print("passed %d, failed %d" % (passed, failed))
sys.exit(1 if failed else 0)
'
