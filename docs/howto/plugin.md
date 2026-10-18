# The Claude Code plugin and the review loop

> Logos ships a [Claude Code](https://claude.com/claude-code) plugin from this
> repository. It carries the `logos` MCP server and the session-start readout,
> and closes a loop around each editing turn: a **Stop check** measures what the
> turn did to your quality contract and scans the files it changed for secrets, a
> **review skill** judges each regression symbol by symbol, a **security review
> skill** triages the security findings the branch introduced and fixes only what
> an agent may close, and a **guardrail** asks you before an agent relaxes the
> contract to make a number go away.

The plugin is a convenience layer over commands you can run yourself. Nothing in
it measures anything the binary does not: every number comes from `logos check`,
`logos review`, `logos offenders`, `logos quality-report` and the `security` commands,
and the plugin's scripts are bash (plus `python3` in the review skills) that call
them. It works
only with a `logos` binary of **1.20.0 or newer** on `PATH` (the
`metadata.minBinaryVersion` in `plugins/logos/.claude-plugin/plugin.json`), and
only in a repository that holds a `.logos/` directory.

## What the plugin carries

| Part | Host event | What it does | Off switch |
|---|---|---|---|
| MCP server `logos` | session start | `logos serve --mcp`, with every `logos:*` tool. In a directory with no `.logos/` it is an empty server: no tools, no `logos` process, no store created. | disable the plugin |
| Session-start readout | `SessionStart` (`startup`, `resume`, `clear`) | The signal, the baseline and the recorded violations, from [`logos quality-report --hook-json`](commands.md#quality-report---hook-json). An older binary gets a one-line handshake naming both versions instead. | `LOGOS_QUALITY_REPORT_DISABLE=1` |
| Guardrail | `PreToolUse` | Returns `ask` when an agent would relax the quality contract. See [The guardrail](#the-guardrail). | none, by design |
| Stop check | `Stop` | Runs `logos check` and `logos review` within a budget, scans the files the turn changed for secrets when scan consent is recorded, and blocks the stop once on a regression. See [The Stop check](#the-stop-check). | `LOGOS_STOP_HOOK_DISABLE=1` |
| `/logos:review` | slash command | Starts the `logos-reviewer` subagent, which runs the `logos-review` skill. See [The review skill](#the-review-skill). | do not invoke it |
| `/logos:sec-review` | slash command | Runs the `logos-sec-review` skill: one verdict per security finding the branch introduced, and a fix only by re-scan. See [The security review skill](#the-security-review-skill). | do not invoke it |

Every hook is **report-only unless it found a regression**: each one exits 0,
and a directory with no `.logos/` is left completely alone. "Enrolled" means a
`.logos/` directory in the project directory the host names; a session started
in a subdirectory of an enrolled repository is not enrolled.

## Installing

You need a `logos` binary of 1.20.0 or newer on `PATH` (see
[Installation](installation.md)), a git repository, and a Claude Code recent
enough for the one-step install below (2.1.275 or newer, the floor
`claude plugin validate` reports for it). From the project root:

```bash
claude plugin marketplace add raffaelecamanzo/logos
claude plugin install logos@logos
logos init -i
logos index
```

The first command adds this repository as the marketplace `logos`, and the
second installs the plugin `logos@logos` for you (user scope; add
`--scope project` to enable it for this project only). From inside a Claude Code
session the same install is one step:

```text
/plugin install logos --marketplace raffaelecamanzo/logos
```

`logos init -i` is what enrols the project. With the plugin enabled it writes the
plugin's enrolment (`extraKnownMarketplaces.logos` and `enabledPlugins`) into
`.claude/settings.json`, so commit that file and every teammate is prompted to
install the plugin. Because the plugin carries the MCP server and the readout,
`-i` writes no `.mcp.json` entry and no readout hook. The managed `CLAUDE.md`
block is still written. Start a new Claude Code session afterwards. The plugin's
tools appear as `mcp__plugin_logos_logos__<tool>`.

Check the result with:

```bash
claude plugin list
logos doctor
```

`doctor` reports a `plugin_warnings` array. It is empty when the plugin and the
project agree, and it names two things otherwise:

- a **duplicate registration**: a `logos` server declared both by `.mcp.json` and
  by the plugin, so every tool is listed twice (`mcp__logos__*` and
  `mcp__plugin_logos_logos__*`). `logos init -i` removes the legacy entry;
- a **version skew**: the binary is older than the `minBinaryVersion` of the
  plugin manifest that applies here. Upgrade `logos`.

Both are advisory: they never flip `ok` or change the exit code.

**What happens elsewhere.** A plugin enabled at user scope starts in every
project you open. In a directory with no `.logos/` every hook exits 0 with no
output and the server answers the MCP handshake with an empty tool list, so
opening a session there creates no store and enrols nothing. In an enrolled
repository with no `logos` on `PATH`, the readout says so and names the minimum,
and the server's instructions say its tools are unavailable.

## Migrating from `init -i`

Before the plugin, `logos init -i` wired the agent side itself: a `logos` entry
in `.mcp.json`, the session-start readout hook in `.claude/settings.json` with
its script under `.claude/hooks/`, and a copy of any skill it materialized. A
project that has all of those and then enables the plugin lists every tool twice
and prints the readout twice.

Re-run `init -i` once the plugin is enabled:

```bash
logos init -i
```

The plugin counts as enabled when `enabledPlugins` holds `"logos@logos": true`
in `.claude/settings.local.json`, `.claude/settings.json` or your user
`settings.json` (under `$CLAUDE_CONFIG_DIR`, else `~/.claude`). The most specific
of those that holds a boolean decides, so a project that sets it to `false`
overrides a user who enabled it. The run enrols the plugin and removes the legacy
entries, each as its own `removed` row:

| Row | Action |
|---|---|
| `.claude/settings.json` | `updated`: gains `extraKnownMarketplaces.logos` and `enabledPlugins` |
| `.mcp.json` | `removed`: the `logos` entry, only while it holds exactly what `init` wrote |
| `.claude/hooks/logos-quality-open.sh` | `removed`: the readout script and its `SessionStart` entry |

A skill copy and its `.claude/skills` pointer go the same way. Foreign content in
those files is never touched, and a second run reports every row `unchanged`. With
the plugin absent, `init -i` prints and writes exactly what it did before. The
full reference is [`init`](commands.md#init--i---hooks---workspace---yes---exclude-glob).

**`init -i` does not install git hooks.** It offers them on a terminal and the
default is no; `logos init --hooks` installs them. The git hooks and the plugin
are independent: the hooks keep the graph fresh on commit, checkout and merge and
gate a push, and the plugin works at the agent's turn boundary.

## Uninstalling

```bash
logos init --uninstall
claude plugin uninstall logos@logos
```

`init --uninstall` removes everything Logos wrote into the project: the
`CLAUDE.md` block, the `logos` `.mcp.json` entry, the readout hook and script,
the plugin enrolment in `.claude/settings.json`, skill copies, and the managed
git hooks (unsetting `core.hooksPath` only if it still points at `.logos/hooks`).
An entry goes only while it holds exactly what `init` wrote, and a file only when
nothing else is left in it. `.logos/` stays; delete it yourself to drop the
index. The second command removes the plugin from your user settings. To switch
the plugin off without removing it, run `claude plugin disable logos@logos`.

## The review loop

```text
 edit ──▶ agent stops ──▶ Stop check: check + review + a security scan (15 s budget)
                              │
              clean ◀─────────┴────────▶ regression, or a new secret: the stop is
                                          blocked once, with each entry and a
                                          pointer to the skill
                                                 │
                                    /logos:review or /logos:sec-review: one verdict
                                    per entry, fix only what an agent may close,
                                    prove each fix by re-measurement or re-scan
```

Two commands carry the quality half of the loop, and both are usable on their own.

- [`logos offenders`](commands.md#offenders---path-glob---dimension-d) lists every
  offender the last snapshot recorded, uncapped, with each entry's symbol id,
  value and threshold. It reads the snapshot and writes nothing, so run
  `logos scan` first.
- [`logos review --base <ref>`](commands.md#review---base-ref) compares the working
  tree with the **merge-base** of `HEAD` and the base (default: the repository's
  default branch) and classifies every offender and every `check` violation as
  `new`, `worsened`, `improved`, `resolved` or `unchanged`, each with the value
  before and after, its threshold and a [trust label](#trust-labels). It exits 1
  only when it could not measure, and it never changes the exit code of `check`
  or `gate`.

```bash
logos review --json                  # against the default branch
logos review --base main --json      # against a named base
logos offenders --dimension nesting --json
```

### The Stop check

When the agent ends a turn, `stop.sh` measures what the turn did to the quality
contract, within a budget. It stays silent unless there is something to say.

1. **It runs only when tracked source changed** since its previous run. The
   fingerprint covers `HEAD`, the uncommitted diff and every untracked file git
   does not ignore. Documentation and media files (`md`, `markdown`, `txt`, `rst`,
   `adoc`, `pdf`, images, `lock`) and `.logos/` do not count, so a turn that
   touched only those is not measured by `check` and `review`. The
   [security leg](#the-security-leg) still runs on it, because a key in a README
   is still a key.
2. **It runs three legs behind one budget:** `check` first (the contract's own
   verdict, which also reconciles the working tree so the next leg reads a fresh
   graph), then `review`, then [`security`](#the-security-leg). The budget is
   **15 seconds** by default, shared by all three legs.
3. **A clean turn prints nothing** and the stop proceeds.
4. **A regression blocks the stop once.** The reason lists each `new` or
   `worsened` entry with its symbol, value, threshold and trust label, each
   failing error-severity rule, and a pointer to `/logos:review`. It names at most
   12 of them and counts the rest ("... and N more"). A failing rule blocks. Of
   the `review` entries, only one labelled `resolved` blocks; an entry the graph
   could not fully resolve is listed and never blocks by itself. A new secret, or a
   new finding at `high` or above in a file the turn changed, blocks too, and the
   reason points to `/logos:sec-review`.
5. **It blocks once.** The host does not stop a blocked agent from being blocked
   again by itself, so the hook reads `stop_hook_active` from its payload and
   answers the next stop in the same chain with silence.
6. **A leg that was not measured never blocks.** Over budget, or unable to run, it
   is reported as "not measured" and the stop proceeds. A regression the other leg
   did find still blocks, with the unmeasured leg named.

**The before-state is not built inside the budget.** `review` compares with the
merge-base, and a cold merge-base costs one full index of that tree. The hook runs
`review` only when it can tell a before-state is ready: one it built here under
the same `logos` version, contract and configuration, or, when `check` evaluated
no rule, a snapshot recorded at the merge-base. Otherwise it starts the build
detached, at most one at a time, says "not measured", and measures the next turn.
A build that failed is not retried until the contract, the configuration or
`logos` changes.

The hook keeps its records in `.logos/stop-hook/`, a directory that ignores
itself, and needs `git` on `PATH` (it exits 0 silently without it). Set
`LOGOS_STOP_HOOK_BUDGET_MS` to change the budget in milliseconds; the plugin
declares a 30-second timeout for the hook, so a value above `25000` is held at
`25000`, and a leg still running then is reported as not measured. Set
`LOGOS_STOP_HOOK_DISABLE=1` to turn the hook off, security leg included.

#### The security leg

The third leg scans what the turn changed for secrets and serious findings, and
only with consent already recorded. It reads and reports; it never asks for
anything.

- **The files** are the regular files that changed since the last scan whose
  findings the hook judged, committed or not (so a turn that committed its work is
  scanned too), and the untracked files git does not ignore. Documentation is
  included and `.logos/` is not.
- **The scan** is the scoped re-scan, `logos security scan --scanner <name> --path <file>`
  ([security.md](security.md#re-scanning-a-few-files)): Gitleaks, then Semgrep the
  same way when it is installed and consented. Without Semgrep the leg goes on with
  Gitleaks alone. The scan ingests what it finds, as any re-scan does.
- **Consent is a precondition.** The scan checks it itself: asked to run without
  recorded consent it exits 2 before any scanner process starts, a version probe
  included, and the hook reads that refusal. It does not ask `logos security status`,
  which probes every installed scanner. The leg then reports "security not
  measured: no consent" and the stop proceeds. The hook never records consent;
  run `logos security scan --consent` yourself ([Consent](security.md#consent)).
- **New is the scan's verdict, not the hook's.** A finding in a scanned file whose
  fingerprint the store did not hold before the scan. A secret the scan finds
  again keeps its recorded fingerprint and is not new, also when only its lines
  moved and its column span is unchanged and unique; one with no such match reads
  as new. Suppressed findings are never listed.
- **It blocks** on a new secret-class finding, whatever its severity (Gitleaks
  reports secrets as medium), and on a new finding at `high` or `critical`. The
  reason lists each one's rule, file, line, scanner and severity with the stored
  message, which was redacted at ingest; nothing is read from the scanned files, so
  no secret value reaches it.
- **Silence is not a clean bill.** It means no new finding in those files. A
  store resting on scoped runs alone still reads `stale` (`scoped_only`) in
  `logos security status`. A leg that could not scan says so (see below); a turn
  with no regular changed file to scan is silent.
- **A leg that was not measured never blocks by itself.** No consent, Gitleaks not
  installed or disabled, the shared budget spent, or a file whose stored findings
  are more than the hook reads: each is reported as "security not measured" with
  its reason, and the stop proceeds. A secret that the leg did find still blocks,
  with the unmeasured part named.

### The review skill

`/logos:review [--base <ref>] [debt]` starts the `logos-reviewer` subagent, so the
review's reads and re-runs stay out of your conversation, and returns its report.
The subagent follows the `logos-review` skill, in which every step with a
yes-or-no answer is a script that prints a denominator line saying what it looked
at: a run that looked at nothing is not a run that found nothing.

1. **Resolve the base** and the merge-base. A branch with no commit and no edit
   past its merge-base reports `NO ENTRIES` and stops.
2. **Run the delta** (`logos review`). The first run on a cold cache indexes the
   merge-base tree once. A delta that was not measured is reported with its reason,
   never guessed.
3. **Look up test reach** for each entry's symbol, by test-marked files.
4. **Gate:** one row per entry with its proposed verdict.

Every entry gets exactly one verdict:

| Verdict | Means | Edited? |
|---|---|---|
| `untrusted` | the entry's label is not `resolved` | never |
| `test-first` | trusted, but no test reaches the symbol | never; the report names the characterisation test to write |
| `no-action-warranted` | with a reason: below threshold, cold file, cost exceeds benefit, intentional, already resolved or improved, or no single symbol | no |
| `fix` | trusted, covered by tests, and worth the change | yes, one behaviour-preserving change per symbol |

The skill may move a proposed `fix` to `no-action-warranted`. It may not move any
other verdict to `fix`: an untrusted or untested entry does not become editable
because the code looks wrong. It edits only `fix` symbols, inside an edit scope it
checks afterwards, and it never commits, pushes or blesses a baseline.

**A fix is claimed only when the delta says so.** The skill re-runs the delta and
compares each edited entry with its state before:

- `proven`: the entry now reads `resolved` or `improved`, with trust `resolved`;
- `cleared`: the entry was `new` on this branch and is no longer listed. The
  merge-base never had it, so the delta cannot say `resolved`, and the report says
  `cleared` and nothing stronger;
- `not proven`: anything else. The report says `UNPROVEN` and the edit is undone.

A fix also needs a passing test run with its count, `P passed, F failed (of T)`; a
runner that prints no count is no evidence. The skill never cites the aggregate
quality number, before or after, because it is not what was measured.

The report ends with the scripts' denominators and is itself checked by a script
(`ENTRY`, `FIXED` and `UNPROVEN` lines, and no mention of the aggregate). With the
word `debt`, the skill reads the recorded "where to start" list instead of the
branch, cross-ranks it with churn, and applies the same verdicts, edit scope and
proof.

The skill needs `python3`, `logos` 1.20.0 or newer and a `.logos/` store; without
them a script exits 3 and the review stops saying what is missing.

### The security review skill

`/logos:sec-review [--base <ref>]` runs the `logos-sec-review` skill in your
conversation, with the same base as `/logos:review`. It is a triage, not a
clearance: it reads the security section of the branch delta
([security.md](security.md#the-security-section-of-review---base)), gives every
`introduced` or `touched` finding one verdict, edits only what an agent may close,
and proves each edit by asking the scanner that found it. Like the review skill,
every step with a yes-or-no answer is a script that prints a denominator line, and
the skill needs `python3`, a `logos` that has `logos security scan --scanner <name> --path <file>`,
and a `.logos/` store; without them a script exits 3 and the review stops saying
what is missing.

1. **Resolve the base** and the merge-base. A branch with no commit and no edit
   past it reports `NO ENTRIES`.
2. **Read the security section.** A project with no stored scan reports
   `NOT MEASURED` and asks you to run `logos security scan --consent`: the skill
   never records consent. The first run on a cold cache indexes the merge-base tree
   once, and the merge-base tree is never scanned.
3. **Triage.** One row per introduced or touched finding, with its proposed verdict
   and the reason. A script proposes the verdict from the finding's own class and
   confidence.

Every entry gets exactly one verdict:

<!-- doc-test: review-verdicts -->

| Verdict | The finding | The agent | Edited? |
|---|---|---|---|
| `fix` | A true positive with `resolved` reachability confidence that is neither secret-class nor a dependency finding | Edits it, then proves the edit by re-scan | yes |
| `escalate-secret` | Secret-class | Reports "rotate the credential, and the value remains in git history". The entry is never closed as fixed | no |
| `escalate-dependency` | A dependency finding | Proposes the version change with its changelog risk and blast radius, and you approve | no |
| `false-positive` | Judged not exploitable in this occurrence | Drafts an ignore-file entry with a reason; writing it is held by the guardrail | no |
| `untrusted` | Reachability confidence `unresolved` or `unbound` (also: no bound file, or a file edited since the scan) | Reports it | never |

A script sets `escalate-secret`, `escalate-dependency` and `untrusted` and the
skill may not change them. It confirms a proposed `fix` or moves it to
`false-positive`, and it may not move any other verdict to `fix`. These entries
carry the reachability confidence of the security page, not the
[trust labels](#trust-labels) of the quality delta.

**What it may and may not close.**

- It edits only `fix` code, inside an edit scope it checks afterwards: one
  behaviour-preserving change per finding, in the style the code already uses. It
  never commits, pushes or blesses a baseline.
- **A fix is claimed only by proof.** The skill re-scans the changed files with the
  same scanner, `logos security scan --scanner <name> --path <file>`, and holds
  that the entry's fingerprint is absent, that no finding at or above its severity
  appears that the delta did not already know, and that the project's tests pass,
  with a count: `P passed, F failed (of T)`. A scanner that cannot be narrowed runs
  on the whole project. Anything less reads `UNPROVEN` and the edit is undone, and
  removing a finding by deleting the code that held it is not a fix.
- **A secret is never closed.** It is not edited under this skill and never
  re-scanned as proof; the report says to rotate the credential, because the value
  stays in git history. It does not print the value either: the delta holds only
  the redacted message.
- **A dependency is proposed, never edited**, and a human approves the change.
- **A suppression is drafted once.** The skill prints the `[[ignore]]` entry and
  adds it to `.logos/security-ignore.toml` with the host's own Edit tool, once. The
  guardrail answers with `ask`, and the report records the entry as held,
  approved or declined. The skill stops there and does not look for another way
  around the question.
- It never records scan consent, installs a scanner, or relaxes the quality
  contract.

**A clean review is not a claim that the code is secure.** The report's strongest
claim names the scanners that ran, their versions and the paths, and lists the
OWASP categories that no enabled scanner with a stored report covers. It never says the code is secure,
whatever the findings look like, and "not exploitable here" is said of one
occurrence, never of the branch. The report's `ENTRY`, `FIXED`, `UNPROVEN`,
`ROTATE`, `DEPENDENCY`, `SUPPRESS`, `SCANNED` and `COVERAGE` lines are checked by a
script, which also refuses any line that claims safety.

## The guardrail

The `PreToolUse` hook returns the permission decision `ask`, with a reason naming
the file or command, when an agent tool call would:

- write, edit or delete `.logos/rules.toml`, `.logos/rules-baseline.json` or
  `.logos/security-ignore.toml` (under any `.logos/`, so a workspace member's
  contract is guarded too); or
- run `logos gate --save` or `logos check --bless`, as a shell command, or through
  `check --bless`'s MCP twin `bless_rules` (under the plugin's or a project
  `.mcp.json`'s prefix); or
- run `logos security ingest`, as a shell command (every spelling `gate --save`
  is caught in), or through its MCP twin `security_ingest` (under either prefix):
  an empty SARIF report ingested for a scanner clears a failing `[security]` gate,
  required scanners included, so an agent does not do it unseen. Its read-only
  neighbours (`security findings`, `security status`, `security scan`) pass.

The human then decides: the quality contract is what the review loop measures
against, so an agent does not relax it unseen. Every other call passes untouched,
including a Read of a guarded file and a shell command that only reads one
(`cat .logos/rules.toml`). A shell command that names a guarded file as a word of
its own, or glued to an option (`dd of=…`), asks unless its command is a known
reader and the file is not a redirect target, so an unrecognised command asks.
So does a writer given the `.logos` directory itself, or a glob or brace list
that can reach a guarded name (`rm -rf .logos`). A
payload the hook cannot read **asks**; it never fails toward allow.

It is a guardrail against loosening the contract by habit or by mistake, not a
sandbox: a command that assembles the path at run time (an interpreter one-liner,
a path piped into `xargs`) is not seen. There is no environment variable that
turns it off. The way to stop it asking is to disable the plugin, which is the
decision it exists to put in your hands.

## Trust labels

Every entry of the branch delta carries the trust label of its file. The label
says whether the graph could see what a claim about the entry rests on, and the
Stop check and the review skill act on it:

| Label | Means | Review verdict | Stop check |
|---|---|---|---|
| `resolved` | the file is indexed as it stands and its language binds relations across files | may be `fix` or `test-first` | counts; `new` and `worsened` entries block |
| `same-file-only` | the language's calls resolve within files only, so callers and tests across files cannot be seen | `untrusted` | listed, never alone blocks |
| `stale` | the file changed on disk since the last sync, so the graph does not match the tree | `untrusted`: run `logos sync`, then review again | listed, never alone blocks |

Beyond these three, a language the graph binds no call across files for reads
`no-resolved-edges`: the other tag [`logos status`](commands.md#status) reports for
such a language. The skill treats it, and any label it does not know, as
`untrusted`.

`review` reads the graph as it stands and never reconciles, so an edit not yet
synced reads `stale`. The Stop check's `check` leg reconciles first, and the skill
runs `logos sync` on the files it edits before it re-measures.

## Off switches and environment variables

| Variable | Read by | Effect |
|---|---|---|
| `LOGOS_QUALITY_REPORT_DISABLE=1` | the session-start readout, plugin and `init -i` hook alike | silences the readout |
| `LOGOS_STOP_HOOK_DISABLE=1` | the Stop check | the hook exits 0 with no output, every leg included (the security leg too) |
| `LOGOS_STOP_HOOK_BUDGET_MS=<ms>` | the Stop check | the shared budget of its legs; default `15000`, a non-number reads as the default |
| `LOGOS_PROJECT_DIR=<dir>` | the review skill's scripts | the project to review when `--project` is not given; default the working directory |
| `LOGOS_INVOKER` | `logos` itself; set by the plugin, not by you | each hook exports its own (`hook:stop`, `hook:pre-tool-use`, `hook:session-start`) and each skill's scripts theirs (`skill:logos-review`, `skill:logos-sec-review`; a value already set is kept, so a `logos-review` script run by `logos-sec-review` stays the latter). Every `logos` call they make records it, and `logos stats --json` counts calls by it in `calls_by_tool_invoker` |

The guardrail has no switch. To turn the whole plugin off, run
`claude plugin disable logos@logos`; `logos init --uninstall` removes the
enrolment from the project.

## Troubleshooting

| You see | Cause | Do |
|---|---|---|
| every `logos` tool listed twice | the legacy `.mcp.json` entry beside the plugin | `logos init -i` removes it |
| the readout prints twice | the legacy readout hook beside the plugin | `logos init -i` removes it |
| "the binary on PATH … is older than the 1.20.0 this plugin needs" | `logos` is older than `minBinaryVersion` | upgrade `logos`; the readout is skipped until then |
| the Stop check says "not measured … being built in the background" | the merge-base before-state was cold | nothing: the next turn is measured |
| every entry reads `stale` | edits not yet synced | `logos sync`, then `logos review` |
| the Stop check never runs | no `.logos/`, or no `git`, or no commit yet, or `LOGOS_STOP_HOOK_DISABLE=1`; or only documentation or media changed, which `check` and `review` skip (the security leg still runs on it, and is silent when it finds nothing new) | expected; run `logos review` yourself |
| the Stop check says "security not measured: no consent" | no scan consent is recorded for this project, so the security leg scanned nothing | run `logos security scan --consent` yourself; the hook never records it |
| the Stop check says "security not measured: Gitleaks is not installed" | the leg's scanner is missing | `logos security install <scanner> --consent`, naming gitleaks |
| `/logos:sec-review` reports `NOT MEASURED` | no security report is stored | run `logos security scan --consent`, then review again |
