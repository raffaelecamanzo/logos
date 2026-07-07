# Logos User Guide

> Logos — structural code intelligence for AI-assisted development.

Logos builds a queryable code graph (symbols, calls, imports, routes) from your
source tree and serves it through two surfaces: a **CLI** for humans and
scripts, and an **MCP server** for AI coding agents such as Claude Code. It
runs fully offline, writes nothing outside the project's `.logos/` directory,
and supports twelve languages out of the box: Rust, Python,
TypeScript/JavaScript, Go, Java, C, C++, C#, Kotlin, Scala, Ruby, and PHP.

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
the **coverage tier** ingests external LCOV/Cobertura reports for freshness-checked
per-file coverage and the *untested-hotspots* join. Both are **non-gated and
advisory** — they never move the quality signal and the quality gate never reads
`history.db`.

A **source wiki** layers over the graph from a third store (`.logos/wiki.db`):
anchored, human-readable pages about the codebase, searchable (FTS5) and
self-pruning. It is **gate-immune** — the quality gate never opens `wiki.db`, so
the signal is byte-identical whether a wiki exists or not. Since CR-062 the wiki
serves **three tiers** ([ADR-57]): an *extracted* tier live-rendered from the
graph; a *presented* tier the binary assembles **deterministically** from the
project's authored `docs/specs/**` and `docs/howto/**` sources (`logos wiki
materialize`) — copied verbatim, `generator = logos:doc-present`, never
paraphrased; and a *generated* tier of LLM prose. When the project ships an SRS
(`docs/specs/architecture.md` + a requirement — "SRS mode"), the Design/Specs and
User Guide pages are presented and the connected agent generates only the
Summary/Overview tier; otherwise the agent infers the full set from the code
graph. Generated prose is written **in-process** when the Wiki tab is opened
(`--features ui` builds), on the same `rig` agent substrate as Chat and honoring a
dedicated `[wiki].model`. The CLI surface is
`logos wiki write|read|search|status|generate|materialize|delete` plus `logos wiki
skill --emit` and `logos wiki hook --emit`; `write`/`read`/`search`/`status`/`materialize`
have payload-identical MCP twins (five wiki tools).

## Contents

1. [Installation](installation.md) — building the `logos` binary, feature flags, verification
2. [Configuration](configuration.md) — `.logos/`, `config.toml`, `rules.toml`, logging, query overrides
3. [Usage](usage.md) — indexing workflow, navigation, agent/MCP setup, the web UI dashboard, worktree-based development, exit codes, scripting
4. [Commands](commands.md) — the full 28-subcommand reference (incl. `wiki`, `hotspots`, `coverage`)
5. [Metrics](metrics.md) — the ten-dimension quality metrics engine and the 0–10000 signal
6. [Error handling](error-handling.md) — the fail-soft / fail-loud contract, exit codes, and troubleshooting
7. [CI integration](ci-integration.md) — the freshen / enforce / report / bless loop as a copy-pasteable CI recipe (enforce with `check`, report with `scan --json`, bless with `gate --save` at release only)

## Quick start

```bash
# Build (one time)
cargo build -p logos --release

# Set up and index your project
cd /path/to/your/project
logos init -i --hooks   # policy files + .mcp.json injection + git hooks
logos index             # build the code graph

# Navigate
logos search "handler"
logos context "add pagination to the users endpoint"
logos affected src/lib.rs

# Govern
logos check             # evaluate rules.toml — exit 1 on violations
logos gate --save       # save baseline; re-run without --save in CI
```

Restart your agent after `logos init -i` — the `logos:*` MCP tools appear
automatically.

## Design guarantees worth knowing up front

- **Offline by construction** — no network crate exists in the dependency
  graph (enforced by a fitness test). Nothing phones home.
- **`.logos/` is the only write target** — derived databases live there, and
  the only non-derived writes are the policy files `.logos/config.toml` /
  `.logos/rules.toml` when you explicitly Save them (CLI, or the Config view in
  the web UI). Your source tree is never touched.
- **Never-fabricate resolution** — a call edge is created only when exactly
  one candidate matches; ambiguity yields *no* edge rather than a guessed one.
- **Stdout discipline** — `--json` output is always parseable; logs go to
  stderr; in MCP mode stdout carries JSON-RPC frames exclusively.
- **Fail soft where safe, fail loud on correctness** — a single bad file never
  aborts an index (it is skipped with a warning), but a missing or corrupt index
  is refused outright with an actionable message. One auditable contract across
  both surfaces — see [Error handling](error-handling.md).
- **One node per symbol, structurally enforced** — the graph store holds exactly
  one node per `symbol_id` (a `UNIQUE` constraint) and node insertion is an
  idempotent upsert, so incremental `sync` can never accumulate duplicate or
  orphan rows. `logos doctor` verifies this invariant cheaply and **hard-fails the
  gate** on drift (independent of the signal); `logos verify` deep-checks against a
  fresh shadow reindex. See [commands.md](commands.md#doctor).
- **Deterministic** — same tree in, same graph and same metrics out,
  bit-for-bit.
- **Evidence tiers never move the gate** — `hotspots` and `coverage` live in a
  separate `.logos/history.db` the gate never opens, so the 0–10000 signal is
  byte-identical before and after they run, and after `history.db` is deleted.
- **The wiki never moves the gate** — generated pages live in a separate
  `.logos/wiki.db` the gate never opens, so the signal is byte-identical whether
  the wiki is absent, populated, or stale.
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
