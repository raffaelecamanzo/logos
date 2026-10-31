# Logos User Guide

> Logos — structural code intelligence for AI-assisted development.

Logos builds a queryable code graph (symbols, calls, imports, routes) from your
source tree and serves it through two surfaces: a **CLI** for humans and
scripts, and an **MCP server** for AI coding agents such as Claude Code. It
opens no network connection itself, writes outside the project's `.logos/`
directory only for setup you ask for — the agent-integration files of
`logos init -i`, the scanners `logos security install` downloads under your
consent, and a coverage dev dependency you consent to adding — and supports thirteen languages out of the box: Rust, Python,
TypeScript, JavaScript, Go, Java, C, C++, C#, Kotlin, Scala, Ruby, and PHP.

## What it is primarily for: scoping work before you decompose it

The graph's primary value is answering the questions a *plan* is made of, before
the work is split into tasks and before anyone decides what to read:

- **What does this task touch?** — `logos context "<task in one sentence>"`
  returns a ranked, code-carrying bundle in one call, instead of a sequence of
  speculative file reads.
- **Can these two work items run in parallel?** — `logos impact-intersection
  --item a=<symbol>,… --item b=<symbol>,…` reports the pairs whose transitive
  impact sets intersect, and the symbols they share, so colliding items can be
  placed in different iterations rather than collide at merge.
- **Has this already been done here?** — `logos precedent <symbol-or-file>`
  returns the code that plays the same structural role — sharing a supertype, a
  registration, or a call shape — each result naming why it is analogous. Ranked
  by counted graph facts, never a similarity score. An empty answer gives its
  reason, and where the target's language binds no call across a file boundary
  that reason says the structure is unresolved there rather than absent — see
  [where the relational claims hold](#where-the-relational-claims-hold).
- **How large is the change really?** — `logos impact <symbol>` for a symbol's
  transitive blast radius, `logos affected <file>…` for the reverse-transitive
  file closure of a changed set.
- **Which branches collide, and did the merge carry everything?** — `logos
  branch-overlap --ref a --ref b` reports symbols more than one ref modifies;
  add `--merge <ref>` and it reports what a branch changed that the merge result
  does not have.

Navigating by structure *while you edit* — `search`, `node`, `callers`,
`callees`, `explore` — is the secondary mode, covered in
[Usage](usage.md) and [Commands](commands.md).

### Where the relational claims hold

Relational answers hold only on the languages the graph resolves. The answers
that traverse resolved edges — callers, callees, impact, affected, precedent,
impact intersection and branch overlap — say which: each carries a
`resolution_denominator` whose `languages` rows give each anchor language's
`calls` resolution. A node answer's edge list carries none, so read it against
the project's rows named below. Where a row has a `cross_file_edges` figure, the
graph beats grep for that language, as completely as its `bound` share of
`references` — a partial ratio is a partial answer. Where its
`cross_file_absence` reads `same-file-only` or `no-resolved-edges`, the graph
binds no call across a file boundary there: an empty or short answer is the
index's reach, not evidence of absence, and grep is the better tool. Callers can
live in another language than their target, so read the whole project's rows as
well — `resolution_by_language` in the status readout. For this project that is
`logos status --json`.

> **This ordering is a hypothesis under measurement, not a settled finding.** It
> comes from this project's own telemetry, where the task-scoping call fired about
> once per agent session — evidence that a navigate-while-you-code framing
> under-delivered, not evidence that this framing works.
>
> Judge it from `logos stats --json`, never by hand: its `calls_by_class` cross-tab
> gives every call a tool class, split by dev-vs-`main` origin, and states its own
> coverage limits in the payload. When this framing shipped, navigation was **2.64%
> of lifetime calls on this project** (1.47% of dev-origin ones). Re-read the
> cross-tab for the project in front of you rather than trusting that number.

## What it indexes

Markdown documentation is indexed as a first-class layer too — `DocFile`/
`DocSection` nodes with doc→code traceability (and typed
`Requirement`/`Adr`/`Story` nodes on swe-skills repos), so you can ask which
code implements a requirement and which docs a change touches. Documentation is
**metric-neutral** — it never moves the quality signal.

A third layer indexes **config and infrastructure artifacts** — ten formats
(YAML, JSON, TOML, Dockerfile, Makefile, Shell, Protobuf, GraphQL, Terraform,
SQL) become `ConfigFile`/`ConfigSection` nodes plus typed anchors
(`DockerfileStage`, `TfBlock`, `SqlObject`, `ProtoMessage`, …), and OpenAPI specs
are content-sniffed into `ApiPath`/`ApiOperation` anchors. This layer is
**metric-neutral** too — adding or removing a config file never moves the signal.

Two **evidence tiers** layer over the graph from a second store (`.logos/history.db`):
the **temporal tier** mines git history for churn-and-complexity *hotspots*, and
the **coverage tier** ingests LCOV, Cobertura, JaCoCo and Go coverprofile reports (or runs the
project's own coverage tools under a per-project execution consent) for freshness-checked
per-file coverage and the *untested-hotspots* join. Both are **non-gated and
advisory** — they never move the quality signal and the quality gate never reads
`history.db`.

## Contents

1. [Installation](installation.md) — building the `logos` binary, the default and slim builds, feature flags, verification
2. [Configuration](configuration.md) — `.logos/`, `config.toml`, `rules.toml`, logging, query overrides
3. [Usage](usage.md) — indexing workflow, navigation, evidence tiers (hotspots, coverage, security findings), agent/MCP setup, the web UI dashboard, worktree-based development, exit codes, scripting
4. [Commands](commands.md) — the full 42-subcommand reference (incl. the planning-time `logos impact-intersection`, `logos precedent`, `logos branch-overlap`, plus `logos hotspots`, the `logos coverage …` and `logos security …` families and `logos hook --emit`, the session-start quality-report hook)
5. [Metrics](metrics.md) — the ten-dimension quality metrics engine and the 0–10000 signal
6. [Error handling](error-handling.md) — the fail-soft / fail-loud contract, exit codes, and troubleshooting
7. [CI integration](ci-integration.md) — the freshen / enforce / report / bless loop as a copy-pasteable CI recipe (enforce with `check`, report with `scan --json`, bless with `gate --save` at release only)
8. [Contributing](contributing.md) — the per-sprint pull request (`scripts/publish-sprint.sh`), the issue status labels, and the owner's GitHub settings checklist; contributors start at [CONTRIBUTING.md](../../CONTRIBUTING.md)
9. [Claude Code plugin](plugin.md) — installing the plugin, migrating from `init -i`, `init --uninstall`, the Stop check with its security leg and its off switch, the `/logos:review` and `/logos:sec-review` skills and their verdicts, the guardrail, and the trust labels
10. [Security](security.md) — the security section end to end: installing the nine scanners and their tested versions, consent and the network, scans and re-scans, reachability confidence, the `[security]` gate with its ratchet and exit 5, the ignore file, reviewing what a branch introduced (the five verdicts, and that a clean review is not a claim of security), and the Security page
11. [Coverage runs](coverage.md) — the seven coverage tools with their markers, invocations and tested versions, the execution consent and what it does and does not protect against, `coverage run` and `coverage refresh`, the managed `cargo-llvm-cov` install and the dependency consent, the Coverage page, the Workspace Coverage page and the user-level overrides
12. [Development](development.md) — for maintainers: the hand-run harnesses kept outside every gate: `scripts/verify-scanner-exclusions.sh` and how to read the scanner-exclusion evidence it commits, and `scripts/verify-sync-history.sh`, the on-demand review check that replays real commits through `logos sync` and compares the result with a fresh index

## Quick start

```bash
# Build (one time)
cargo build -p logos --release

# Set up and index your project
cd /path/to/your/project
logos init -i --hooks   # policy files + .mcp.json injection + git hooks
logos index             # build the code graph

# Scope the work, before decomposing it
logos context "add pagination to the users endpoint"
logos precedent src/api/users.rs
logos impact-intersection --item pag=list_users --item auth=require_session

# Navigate, while coding
logos search "handler"
logos affected src/lib.rs

# Govern
logos check             # evaluate rules.toml — exit 1 on violations
logos gate --save       # save baseline; re-run without --save in CI
```

Restart your agent after `logos init -i` — the `logos:*` MCP tools appear
automatically. On Claude Code, [the plugin](plugin.md) carries them instead, with
the review loop around each editing turn.

## Design guarantees worth knowing up front

- **Offline by construction** — no outbound-HTTP client crate exists in the
  dependency graph, and the slim build links no socket stack at all (both
  enforced by a fitness test). Nothing phones home. The downloads are
  `logos security install` and `logos coverage install` (and Install on the Security
  and Coverage pages), which run the system `curl` and `uv` under a consent you give;
  the scanners themselves may reach the network under the scan consent, and a
  [coverage run](coverage.md) may fetch the project's own dependencies under its own
  ([Logos makes no network connection](security.md#logos-makes-no-network-connection)).
- **`.logos/` is the only write target, except for setup you ask for** — derived
  databases live there, and the only non-derived writes are the policy files
  `.logos/config.toml` / `.logos/rules.toml` when you explicitly Save them (CLI, or
  the Config view in the web UI). Plain `logos init` writes only inside `.logos/`
  too. `logos init -i` also writes the agent-integration files — `.mcp.json`,
  `CLAUDE.md` and `.claude/settings.json`, or the plugin's enrolment in that last
  file when the [plugin](plugin.md) is enabled — and `--hooks` sets
  `core.hooksPath`; `logos init --uninstall` removes them all. The security
  consents and the scanners Logos installs live in your user-level directories,
  never in a repository ([Consent](security.md#consent)). Your source tree is
  never touched, except by a coverage dev dependency you consent to
  ([coverage.md](coverage.md#installing-cargo-llvm-cov-and-adding-a-missing-dependency)),
  which stays uncommitted in your working tree.
- **Never-fabricate resolution** — a call edge is created only when exactly
  one candidate matches; ambiguity yields *no* edge rather than a guessed one.
- **Stdout discipline** — `--json` output is always parseable; logs go to
  stderr; in MCP mode stdout carries JSON-RPC frames exclusively.
- **Fail soft where safe, fail loud on correctness** — a single bad file never
  aborts an index (it is skipped with a warning), but a missing or corrupt index
  is refused outright with an actionable message. One auditable contract across
  both surfaces — see [Error handling](error-handling.md).
- **One node per symbol, structurally enforced** — the graph store holds exactly
  one node per `symbol_id` (a `UNIQUE` constraint), and a symbol names one node:
  a file whose declaration would take a symbol another node already holds fails
  to persist, with the reason recorded, instead of being folded into it. A
  re-extracted file replaces its own nodes, so incremental `sync` can never
  accumulate duplicate or orphan rows. `logos doctor` verifies this invariant cheaply and **hard-fails the
  gate** on drift (independent of the signal); `logos verify` deep-checks against a
  fresh shadow reindex. See [commands.md](commands.md#doctor).
- **Deterministic** — same tree in, same graph and same metrics out,
  bit-for-bit.
- **Evidence tiers never move the signal** — `hotspots`, `coverage` and `security` findings live in a
  separate `.logos/history.db` the quality signal never reads, so the 0–10000 signal is
  byte-identical before and after they run, and after `history.db` is deleted. Security
  findings gate a merge only when `rules.toml` carries a `[security]` table
  ([security.md](security.md#gating-a-merge-the-security-table)).
- **The graph self-corrects on config change** — narrowing admission (an added
  `exclude`, a disabled layer) purges the now-unadmitted nodes on the next
  index/reconcile/navigation, so a narrowed config can never keep serving stale
  results; unchanged config does zero purge work.
- **Sync and the watcher admit exactly what a fresh `index` would** — a single
  `AdmissionAuthority` (gitignore + nested-`.git`-boundary + `ignored_dirs` +
  globs + size) backs `discover`, incremental `sync`, and the filesystem
  watcher's classifier, so a gitignored path or a nested worktree can never be
  ingested by any path; a full-walk reconcile purges any stored file the
  authority now rejects even if it is still on disk. `logos doctor` also runs
  this as an **always-on admission tripwire** and hard-fails the gate on drift,
  the same way it hard-fails on structural drift. See
  [error-handling.md](error-handling.md#troubleshooting-common-errors) and
  [usage.md](usage.md#worktree-based-development).
