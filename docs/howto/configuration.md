# Configuration

Logos works with **zero configuration** — every setting has a sensible
default. When you do need control, configuration lives in two human-editable
TOML files under `.logos/`, designed to be **checked into version control**
(they are policy that travels with the repo; the derived databases are not).

You can edit both files by hand, or interactively in the web UI's **Config**
view (`logos serve --ui`, then `/config`): it offers typed fields plus a raw
pane, validates on Save (an invalid edit is rejected with the offending key
named and the file left byte-identical), and exposes an explicit **Apply &
reindex** step that reconciles the graph / re-evaluates the gate — Save alone
only writes the file. See [usage.md](usage.md#interacting-with-the-dashboard).

## The `.logos/` directory

Created automatically by the first `logos index`:

| Path | What it is | Check in? |
|---|---|---|
| `.logos/logos.db` | The code-graph index (SQLite). Derived — rebuildable any time with `logos index`. | No |
| `.logos/history.db` | The evidence store (SQLite): mined git history + ingested coverage, on its own forward-only migration track. Derived — created on demand by `logos hotspots` / `logos coverage ingest`; the gate never opens it. | No |
| `.logos/telemetry.db` | Local usage/performance events feeding `logos stats`. Never leaves your machine. | No |
| `.logos/wiki.db` | The source-wiki store (SQLite): generated pages + FTS5 search, on its own forward-only migration track. Derived — created on demand by the first `logos wiki write` / `logos wiki status`; the gate never opens it. | No |
| `.logos/chat.db` | The Chat conversation store (SQLite): threads, messages, and per-turn agent memory, on its own forward-only migration track. Derived — created on demand by the first Chat turn in the `--features ui` web surface; never present in the default offline binary. | No |
| `.logos/config.toml` | Indexing & resolution policy (optional). | Yes |
| `.logos/secrets.toml` | The chat API key (optional). The one secret Logos stores — **gitignored** (added by `logos init`), written owner-only (`0600`), never committed and never travels into worktrees. | No |
| `.logos/rules.toml` | Architecture contract: layers, boundaries, budgets (optional). | Yes |
| `.logos/hooks/` | Managed git hook scripts: freshness `post-commit`, `post-checkout`, `post-merge` plus the enforcing `pre-push` gate. Installed by `logos init --hooks`. | Yes |
| `.logos/plugins/` | On-disk language-query overrides (optional, advanced). | Yes |

Recommended `.gitignore` entries:

```gitignore
.logos/*.db
.logos/*.db-*
.logos/secrets.toml
```

`logos init` adds the `secrets.toml` entry for you; it is listed here so a
hand-rolled `.gitignore` keeps the chat API key out of version control.

## `config.toml` — indexing and resolution policy

Absent file (or absent fields) = defaults. **Unknown keys fail loud with
exit 2** — a typo never silently does nothing.

```toml
# Optional code-language admission allowlist (grammar names from `logos languages`).
# Omit it (or use []) to index every compiled-in code language out of the box —
# the twelve below are the default set. List a subset to restrict indexing; dropping
# a grammar from a broader list purges its now-unadmitted nodes on the next reconcile.
languages = ["rust", "python", "typescript", "go", "java", "c", "cpp", "c-sharp", "kotlin", "scala", "ruby", "php"]

# Discovery: a file is indexed only if it matches an include glob (default:
# everything) and no exclude glob. Excludes are unioned with .gitignore.
# Default exclude (CR-029/FR-CF-05): ["docs/planning/**", "docs/security/**",
# "notes/**"] — planning/security/notes prose is pruned from code AND docs out
# of the box. Set `exclude = []` (or your own globs) to replace it wholesale.
include = ["**"]
exclude = ["generated/**", "**/*.pb.go"]

# Files larger than this many bytes are skipped with a notice. Default: 2 MiB.
max_file_size = 2097152

# Optional hints biasing framework route/component extraction.
framework_hints = ["fastapi", "spring"]

[semantics]
# Directory NAMES pruned anywhere in the tree during discovery. Listing your
# own replaces the default set wholesale. A NAME matches at any depth, so these
# are build/output/tooling conventions — anything that must be path-anchored
# (e.g. docs/planning) is an `exclude` glob instead.
# Default (CR-029/FR-CF-05): target, node_modules, dist, build, vendor, .git,
# .logos, .agents, .claude, __pycache__, .venv, venv, .tox, .mypy_cache,
# .pytest_cache, bin, obj, .gradle, out, Pods, .next, .svelte-kit, coverage,
# cmake-build-debug, cmake-build-release.
ignored_dirs = ["target", "node_modules", "dist", "build", "vendor", ".git", ".logos"]

# Dead-code reachability roots, matched by node name. Exported symbols and
# framework routes are ALWAYS roots; this adds named entry points on top.
# Default: ["main"]
entry_points = ["main", "lambda_handler"]

[resolution]
# Binder aggressiveness: "strict" | "balanced" (default) | "aggressive".
# See below.
policy = "balanced"

[watcher]
# Debounce window (ms) for the file watcher under `serve --mcp`. Default: 300 (OQ-04).
debounce_ms = 300

[documentation]
# Whether markdown documentation is discovered and indexed. Default: true.
# When false, no DocFile/DocSection node is produced.
enabled = true
# Which markdown files count as documentation (anchored globs, default below):
# docs/**/*.md, any top-level *.md, and a root README*.
include = ["docs/**/*.md", "*.md", "README*"]
exclude = ["docs/archive/**"]
# swe-skills typed-node enrichment: "auto" (default) | "enabled" | "disabled".
# auto promotes FR-*/ADR-*/S-NNN artifacts to typed Requirement/Adr/Story nodes
# only when the convention files are detected; a plain repo yields only generic
# DocFile/DocSection nodes.
typed_enrichment = "auto"

[config_artifacts]
# Whether config & infra artifacts (YAML/JSON/TOML, Dockerfile/Makefile/Shell,
# Protobuf/GraphQL, Terraform/SQL, OpenAPI) are discovered and indexed.
# Default: true. When false, no ConfigFile/ConfigSection or typed-anchor node
# is produced.
enabled = true
# Candidate include globs (default ["**"]); the real gate is each plugin's
# extension/filename claim, so this is for project-level narrowing.
include = ["**"]
# Claimed files matching any exclude are skipped. Default excludes the noisy
# generated lock files below — override (e.g. exclude = []) to re-admit them.
exclude = ["**/package-lock.json", "**/Cargo.lock", "**/yarn.lock", "**/pnpm-lock.yaml", "**/*.min.json"]

[coverage_ingest]
# Automatic coverage-ingest policy (CR-036/FR-CV-10). Configures WHEN coverage is
# ingested, never which sources are admitted — so it is excluded from the
# admission fingerprint and can never move the (non-gated) quality signal.
# Omit the whole table to keep manual `coverage ingest` as the only path.
#
# Extra glob(s) that EXTEND the built-in convention artifact paths the `serve`
# watcher admits as coverage artifacts (root-relative, non-anchored code-glob
# semantics). A configured glob is re-admitted through the `target/`-class ignore
# filter as an explicit allow-list exception. Default: [] (conventions alone).
artifact_glob = ["coverage/custom-lcov.info"]
# Format for the auto-ingest / refresh path: "auto" (default, content-detected),
# "lcov", or "cobertura". The manual `coverage ingest --format` flag is unaffected.
format = "auto"
# Optional command `logos coverage refresh` runs (via `sh -c`, cwd = project root)
# to regenerate the artifact before ingesting it. This is the ONLY place Logos
# ever spawns a coverage subprocess, and only on explicit `coverage refresh` —
# NEVER on the serve/watcher path (ADR-38/NFR-SE-01). Omit it and `coverage
# refresh` errors loudly rather than guessing a command.
refresh_cmd = "cargo llvm-cov --lcov --output-path target/coverage/lcov.info"
```

### Narrowing admission self-corrects the graph

Admission is the set of files any of the above tables let in. When you **narrow**
it — add a code/doc/config `exclude`, set `[documentation] enabled = false`, or
drop a language from `languages` — the previously-admitted nodes and edges are
no longer derivable, so a stale graph would otherwise keep serving them. Logos
**reconciles on config change**: the next `logos index` (or `logos reconcile`,
or the first navigation call after the change) detects the narrowed admission
via a config fingerprint and **purges** the now-unadmitted nodes/edges through
the same capture-before-delete path used for deleted files — the reconciled
graph is byte-identical to a fresh index under the new config. The purge runs on
both the write path (`index`/`reconcile`) and the navigation read prologue
(`search`/`query`/`context`/…), so a navigation call never returns a symbol the
current config excludes. **Widening** admission (removing an exclude, re-enabling
a layer) is picked up by the normal incremental sync — the re-admitted files are
re-indexed. Unchanged config does **zero** purge work (the fingerprint matches),
so there is no cost on the common path.

### Choosing a resolution policy

Every policy preserves the **never-fabricate invariant**: a call edge is
created only when the candidate search yields *exactly one* existing symbol.
The policy widens the search, never the acceptance rule.

| Policy | Behavior | Trade-off |
|---|---|---|
| `strict` | Scope-proven bindings only (local → module → imports → explicit paths). Receiver-method calls (`x.f()`) never bind. | Maximum precision, lowest coverage. |
| `balanced` *(default)* | Strict, plus two exactly-one-candidate workspace fallbacks: unique module-path suffix, and a receiver-method call whose name is workspace-unique. | The sweet spot for most projects. |
| `aggressive` | Balanced, plus a bare identifier binds on a workspace-unique name. | Highest coverage; still deterministic. |

Changing the policy needs no migration — resolution re-evaluates the whole
unresolved-reference ledger on every run, so just `logos index` again.

## Documentation — indexing markdown

The `[documentation]` table decides **whether** markdown is indexed and
**which** files count as documentation; everything downstream — extraction into
`DocFile`/`DocSection` nodes, doc→code link resolution, blake3 dirty-detection,
git hooks, and the watcher — is the same machinery code rides. The doc globs use
**anchored** semantics (distinct from the code `include`/`exclude` globs), so the
default top-level `*.md` and `README*` mean exactly the top-level files, not any
`*.md` anywhere.

- `enabled` *(default `true`)* — set `false` to turn doc indexing off entirely;
  no `DocFile`/`DocSection` node is produced.
- `include` / `exclude` *(defaults `["docs/**/*.md", "*.md", "README*"]` / empty)*
  — a markdown file is admitted only if it matches an include and no exclude.
- `typed_enrichment` *(default `"auto"`)* — `auto` promotes swe-skills
  convention artifacts (`docs/specs/requirements/FR-*.md`, `ADR-*`, `S-NNN`
  stories) to typed `Requirement`/`Adr`/`Story` nodes **only when those
  convention files are detected**; `enabled` forces promotion, `disabled` keeps
  every doc generic. This is additive and never required — a plain repo produces
  only `DocFile`/`DocSection` nodes.

Documentation is **metric-neutral by construction**: doc nodes and doc edges are
excluded from every quality metric and governance constraint, so adding or
removing documentation leaves the `gate`/`session_end` signal byte-identical
(the same way test code is excluded — see [metrics.md](metrics.md)).

### External docs behind a git-ignored symlink

Some repos keep their working docs *outside* the code tree and expose them
through an in-repo directory-symlink — e.g. `docs/specs → ../logos-docs/specs`
— while **git-ignoring** that symlink so the external docs never enter version
control. A `.swe-skills` file at the repo root **sanctions** one such external
docs root. Discovery follows a sanctioned, *contained* doc symlink one hop and
indexes the markdown behind it **even when the symlink is git-ignored** — so
your specs, planning, and request docs are graphed on the same checkout that
keeps them out of git. Only the sanctioned root is followed (never inferred),
only the documentation subtree is walked (source-code symlinks are still
skipped wholesale), and a target that escapes the sanctioned containment is
**refused, not followed**.

When a doc directory-symlink under your include set ends up **unindexed** —
because no `.swe-skills` sanction exists, or the target escapes containment —
`index`/`sync` emit a warning naming the path and reason, and `logos doctor`
surfaces the same list in its `doc_symlink_warnings` field
([commands.md § doctor](commands.md#doctor)). This is purely diagnostic: it
never fails the gate, flips `doctor`'s `ok`, or changes an exit status — it just
flags documentation you probably meant to index but isn't.

## Configuration & artifact graph — indexing config and infra files

The `[config_artifacts]` table controls a third indexing layer (beside code and
documentation): the **config & artifact graph**. Ten artifact grammars ship —
YAML, JSON, TOML, Dockerfile, Makefile, Shell, Protobuf, GraphQL, Terraform, and
SQL — discovered by **extension or basename** (e.g. `Dockerfile`, `Makefile`,
`GNUmakefile`). Every config file becomes a `ConfigFile` root with a bounded tree
of `ConfigSection` nodes; richer formats also emit **typed anchors** —
`DockerfileStage`, `MakeTarget`, `ShellFunction`, `ProtoMessage`/`ProtoService`,
`GqlType`, `TfBlock`, `SqlObject`. An OpenAPI document (any `.yaml`/`.json` with a
top-level version-bearing `openapi:`/`swagger:` key, regardless of filename) is
**content-sniffed** and promoted: its `ConfigFile` is tagged `openapi` and emits
`ApiPath` (per path template) + `ApiOperation` (per HTTP method) anchors.

- `enabled` *(default `true`)* — set `false` to turn the whole layer off; no
  `ConfigFile`/`ConfigSection` or typed-anchor node is produced.
- `include` / `exclude` *(defaults `["**"]` / the lock-file set)* — a file must
  match an include and be claimed by a plugin; the default excludes drop the
  noisy generated lock files (`package-lock.json`, `Cargo.lock`, `yarn.lock`,
  `pnpm-lock.yaml`, `*.min.json`). Set `exclude = []` to re-admit a lock file.

Two structural rules are **fixed, not configurable**: the `ConfigSection` walk is
**depth-bounded at 2** (a section and one level of nested section — deeper nesting
is deliberately invisible, so output is deterministic regardless of file size),
and constructs a grammar cannot parse are **skipped, never guessed** (e.g. a
T-SQL `CREATE PROCEDURE`, which `tree-sitter-sequel` cannot parse, yields no
`SqlObject` — the never-fabricate floor).

Like documentation, the config layer is **metric-neutral by construction**:
config nodes are excluded from every quality metric, the DSM, cycle detection,
and dead-code analysis, so adding or removing any config artifact leaves the
`gate`/`session_end` signal byte-identical. The layer is `Contains`-only — it
emits no reference edges (path→handler, HCL `var.x`, SQL foreign keys); those
arrive with cross-artifact resolution (CR-011).

## `[chat]` — the agentic Chat tab

The `[chat]` table configures the web UI's **Chat** tab: an LLM-backed assistant
that answers compound questions about your codebase by planning, dispatching
read-only subagents over the graph/governance/source tools, and streaming back a
synthesized answer (see [usage.md](usage.md#the-chat-tab)).

These settings **only affect the `--features ui` build**. The default `logos`
binary ships no web surface and no networking crate, so it parses `[chat]` as
policy but can never act on it — there is no chat and no outbound call in the
offline binary. The API key is **not** in this table; it lives in the gitignored
`.logos/secrets.toml` (see below).

`[chat]` is optional and every key defaults, so an absent table is all-defaults
and a partial table fills the rest. Like every other section, an unknown key or
an out-of-range value **fails loud with exit 2** and leaves the file
byte-identical (no partial write).

```toml
[chat]
# Provider family: "openai" (default) | "anthropic".
provider = "openai"
# The model identifier passed to the provider — a Claude model id, or an
# OpenRouter / OpenAI-compatible model slug. No default: an unset model is the
# "configure first" signal the Chat tab reads as "not yet usable".
model = "anthropic/claude-sonnet-4"
# The OpenAI-compatible endpoint. Default: https://openrouter.ai/api/v1
# (OpenRouter). Applies to the "openai" provider; the "anthropic" provider uses
# its own native endpoint and ignores this key.
base_url = "https://openrouter.ai/api/v1"
# Maximum tokens to request per completion. Optional — omit to let the provider
# apply its own default.
max_tokens = 4096
# Sampling temperature in [0.0, 2.0]. Optional — omit to let the provider apply
# its own default.
temperature = 0.2

# ── Budget tree (per turn) ──────────────────────────────────────────────────
# Global per-turn tool-call ceiling. Default: 48. Must be ≥ 1.
max_tool_calls = 48
# Per-subagent tool-call cap. Default: 16. Must be in [1, max_tool_calls] — a cap
# above the global ceiling can never bind and is rejected at load.
max_subagent_tool_calls = 16
# Maximum planner replans per turn. Default: 3. 0 is valid (a single plan pass,
# no replanning).
max_replans = 3

# ── Provider resilience (retry) ─────────────────────────────────────────────
# Transient provider faults (transport errors, HTTP 429/5xx, and unclassified
# deserialization hiccups on a 2xx gateway body) are retried with bounded
# exponential backoff + jitter. Auth failures are never retried. On exhaustion
# the original classified error is returned unchanged.
# Number of retries after the first attempt. Default: 2. 0 disables retry (a
# single attempt). An out-of-range count fails loud at load.
max_provider_retries = 2
# Base backoff in milliseconds for the exponential delay. Default: 200. Must be
# ≥ 1 — a value of 0 fails loud at load.
provider_retry_base_ms = 200

# ── Per-role model overrides (optional) ─────────────────────────────────────
# Each role with no override falls back to the top-level `model` above. The
# roster is fixed, so the keys are an enumerated set: a typo'd role fails loud.
[chat.models]
planner            = "anthropic/claude-sonnet-4"
graph_navigator    = "openai/gpt-4o-mini"
governance_analyst = "openai/gpt-4o-mini"
source_reader      = "openai/gpt-4o-mini"
synthesizer        = "anthropic/claude-sonnet-4"
```

### `[chat]` keys

| Key | Type | Default | Effect |
|---|---|---|---|
| `provider` | `"openai"` \| `"anthropic"` | `"openai"` | Which provider family the agent talks to. |
| `model` | string | *(unset)* | Model id / slug passed to the provider. Unset = the configure-first state (the Chat tab shows no composer). |
| `base_url` | string | `https://openrouter.ai/api/v1` | OpenAI-compatible endpoint. Applies to `"openai"` only; must be non-empty. |
| `max_tokens` | integer | *(unset)* | Max tokens per completion. If set, must be ≥ 1. |
| `temperature` | float | *(unset)* | Sampling temperature. If set, must be in `[0.0, 2.0]`. |
| `max_tool_calls` | integer | `48` | Budget tree: global per-turn tool-call ceiling. Must be ≥ 1. |
| `max_subagent_tool_calls` | integer | `16` | Budget tree: per-subagent tool-call cap. Must be in `[1, max_tool_calls]`. |
| `max_replans` | integer | `3` | Budget tree: max planner replans per turn. `0` = a single plan pass. |
| `max_provider_retries` | integer | `2` | Retries after the first attempt for a transient provider fault. `0` disables retry. Out-of-range fails loud. Inherited by the wiki generator. |
| `provider_retry_base_ms` | integer | `200` | Base backoff (ms) for the exponential + jitter retry delay. Must be ≥ 1 (`0` fails loud). Inherited by the wiki generator. |

The `[chat.models]` table maps a fixed set of roles — `planner`,
`graph_navigator`, `governance_analyst`, `source_reader`, `synthesizer` — each to
a model string. Every key is optional; an omitted role uses the top-level
`model`. An unknown role key fails loud at load.

### The budget tree

Every Chat turn runs under three nested bounds, so a turn always terminates and
its cost is bounded:

- **`max_tool_calls`** is the global ceiling on tool calls across the whole turn
  (planner plus every subagent). It is a **hard** bound: when it is reached the
  turn stops taking new tool work. Rather than emitting a bare halt, the
  orchestrator runs one tool-free synthesis pass over what it already gathered
  and returns a **best-effort answer marked `[bounded — …]`**; only when nothing
  was gathered does it report an honest bare halt. It never fabricates a result.
- **`max_subagent_tool_calls`** caps the calls any single subagent may make
  before it must hand back to the planner. This is a **soft** bound: a subagent
  that reaches its cap closes cleanly, summarizes what it found tool-free, and
  returns a partial observation marked `[bounded — …]` the turn continues from —
  it does not halt the turn. The cap cannot exceed the global ceiling (a cap
  above it can never bind), so that constraint is enforced at load. Subagents are
  also **budget-aware**: their prompt names the cap and the calls remaining and
  steers them to prefer the breadth-efficient `context` tool, so they reach the
  cap less often.
- **`max_replans`** caps how many times the planner may revise its plan after
  observing subagent results. `0` means the planner produces one plan and does
  not replan. Like the global ceiling, reaching it yields a best-effort bounded
  answer when observations exist, or an honest bare halt when they do not.

### Recoverable-fault degradation

Beyond the budget tree, a turn degrades gracefully rather than dying when a
**recoverable** fault occurs — it never fabricates a result:

- **Transient provider faults are retried** at the model seam per
  `max_provider_retries` / `provider_retry_base_ms` (above). Auth failures are
  never retried; on exhaustion the original error is returned. The wiki generator
  inherits this policy.
- **Tool errors become self-correcting observations.** A tool error (e.g. a
  `read` of a missing path) or an out-of-domain tool request is fed back to the
  model as an observation it can adapt from, not a turn-fatal error. A run of
  consecutive tool errors is bounded: past the cap the subagent soft-closes with
  a `[bounded — …]` summary.
- **A security-sandbox refusal is turn-fatal, never routed around (CR-064/S-266).**
  The one exception to the self-correcting-observation rule: when a source tool
  refuses a path that escapes the project root (a containment violation, detected
  via a typed dispatch-error variant — not error-text matching), the turn ends with
  an honest `event: error` naming the refusal ("escapes the project root") and
  produces **no** fabricated `event: final_answer`. Benign faults (missing file, bad
  arg) still take the recoverable route-around path above; only containment refusals
  abort the turn, so the answer can never be composed over a sandbox bypass.
- **A recoverable step fault degrades the step, not the turn.** When a subagent
  step still fails after retries, the orchestrator records a
  `[unavailable — the {role} step could not complete: …]` note and continues to
  the turn's remaining steps, then answers best-effort over what was gathered. If
  nothing usable was gathered (an all-`[unavailable]` turn), it halts honestly.
  A structural fault or a failure of the final answer-composer stays turn-fatal.

### The API key — `.logos/secrets.toml`

The chat provider needs an API key, and it is the **one secret Logos stores**.
It lives in `.logos/secrets.toml`, separate from the checked-in policy files:

```toml
[chat]
api_key = "sk-..."
```

- **Gitignored.** `logos init` adds `.logos/secrets.toml` to `.gitignore`, so
  the key is never committed and never travels into git worktrees.
- **Owner-only on disk.** The file is written with `0600` permissions (owner
  read/write only) — the key is never even briefly group- or world-readable.
- **Masked everywhere, never echoed.** Anywhere the key is surfaced — the Config
  tab, logs, error contexts — it shows only **presence and the last 4
  characters** (e.g. `…1234`). The raw value is never returned in an HTTP
  response, written to a log, or rendered on a page.

You normally never edit this file by hand. Set the key in the web UI's **Config**
tab: the **chat API key** field (under `.logos/secrets.toml`) is a write-only
password input — enter a key and **Save key** to store or replace it, or save it
empty to clear it. The field shows the masked presence of an existing key but
never reveals it. See
[usage.md](usage.md#editing-config-config--the-one-mutating-view).

## `[wiki]` — the source-wiki generation model

The `[wiki]` table selects a **dedicated model for source-wiki generation**,
distinct from the interactive Chat model. Wiki synthesis (batch page generation)
and interactive chat have different cost/latency profiles, so you may want a
cheaper or higher-throughput model to (re)write wiki pages than the one you use
for conversational reasoning.

Like `[chat]`, these settings **only affect the `--features ui` build** — the
default `logos` binary parses `[wiki]` as policy but never acts on it (no
outbound call in the offline binary). Wiki generation runs **in-process** when
you open the Wiki tab in the web UI; there is no headless `claude -p` autogen
hook anymore (see [usage.md](usage.md#the-wiki-tab) and
[CR-047](../requests/CR-047-internal-wiki-generation-on-agent-substrate.md)).

The model settings — `provider`, `base_url`, and the API key — are **inherited
from `[chat]` / `.logos/secrets.toml`**: there is no separate wiki provider,
endpoint, or secret. When `[wiki].model` is omitted, wiki generation falls back
to `[chat].model`.

```toml
[wiki]
# The model id / slug used for wiki page generation. Optional — when omitted,
# the wiki uses [chat].model. Inherits provider/base_url/api_key and the
# provider-retry policy (max_provider_retries / provider_retry_base_ms) from [chat].
model = "anthropic/claude-haiku-4"
# How many graph revisions an already-generated, anchorless prose page may drift
# before it is re-queued for regeneration. Optional. Default: 5. Must be ≥ 1.
revision_stale_threshold = 5
```

### `[wiki]` keys

| Key | Type | Default | Effect |
|---|---|---|---|
| `model` | string | *(unset → falls back to `[chat].model`)* | Model id / slug used for wiki generation. If set, must be non-empty. |
| `revision_stale_threshold` | integer | `5` | Re-queue dampening: how many graph revisions an already-generated, anchorless prose page may drift before it is re-queued for regeneration. Must be ≥ 1 (`0` fails loud at load). |

`[wiki]` is optional and under `deny_unknown_fields`: an unknown key, a blank
`model`, or a `revision_stale_threshold` of `0` **fails loud with exit 2** and
leaves the file byte-identical (no partial write), exactly like every other
section. It is editable from the web UI's **Config** tab through the same
validated atomic write-back as the rest of `config.toml`.

#### Regeneration cadence dampening

`revision_stale_threshold` bounds the cost of the honest revision-stale signal.
Anchorless prose pages (Overview and the consolidated documentation categories)
have no code anchor, so any graph-revision advance makes them *revision-stale*.
Without dampening they would be re-queued for regeneration on **every** commit;
the threshold makes them re-queue only once their drift reaches
`revision_stale_threshold` revisions. Two things stay true regardless of the
threshold:

- **`wiki status` never lies.** The `revision_stale_count` and per-page freshness
  verdict always report the true staleness — dampening governs *re-queue cadence
  only*, never what is reported. A page can be counted as revision-stale yet not
  yet re-queued.
- **The work-list stays a pure offline read.** Computing the (dampened) queue
  performs no `wiki.db` write, no LLM call, and no network access.

Set it to `1` to restore the undamped behavior (re-queue on every revision
advance). First-time page generation is never suppressed — a page that has never
been built is always queued, independent of the threshold.

## `rules.toml` — the architecture contract

Declares the rules that `logos check` and `logos gate` enforce. The loader
validates this file on every invocation (invalid TOML, unknown keys, or
non-compiling globs → exit 2); enforcement runs via `logos check` and
`logos gate` — see [commands.md](commands.md#quality--governance).

`[constraints]` have **no code default** — every key is optional, and an
omitted key is simply "not enforced". The table below is not a set of
defaults; it is the curated **recommended baseline** ([`Constraints::recommended`],
[CR-067](../requests/CR-067-config-default-surfacing.md)) — a reasonable
starting point if you want to opt into these budgets, not a value Logos
assumes. The web Config editor states this distinction explicitly next to each
field (`unset → not enforced` + the recommended number, [FR-UI-12](../specs/requirements/FR-UI-12.md)),
because setting a constraint far outside this baseline with no visible
reference point is exactly the footgun [CR-067] closes (e.g. `max_fan_in = 7`
silently flags every foundational module).

```toml
[constraints]
# The values below are the recommended baseline (Constraints::recommended()),
# not a code default — every key is optional and unset = not enforced.
max_cycles        = 0      # maximum allowed dependency cycles
max_cc            = 15     # maximum cyclomatic complexity per function
max_fn_lines      = 80     # maximum lines per function
no_god_files      = 40     # max symbols per file before it's flagged
max_fan_in        = 30     # max distinct neighbouring modules depending on any one module
max_fan_out       = 30     # max distinct neighbouring modules any one module depends on
max_dead          = 0      # max project-wide dead functions (absolute form)
max_duplicates    = 0      # max project-wide duplicate functions

# Structural budgets (CR-005) — the hard-gate counterparts of the five
# structural metric dimensions. Each is optional and production-scoped.
max_nesting_depth = 4      # max block-nesting depth for any one function
max_brain_methods = 0      # max project-wide "brain methods" (all three floors)
max_clone_ratio   = 0.0    # max fraction of functions in a near-clone group (0.0–1.0)
no_god_containers = true   # if true, any god container (by method count or span) fails check

# Ordered layers: a higher-order layer may not depend on a lower one.
[[layers]]
name  = "domain"
paths = ["src/domain/**"]
order = 1

[[layers]]
name  = "infrastructure"
paths = ["src/infra/**"]
order = 2

# Explicit forbidden dependencies between named layers, with a rationale.
[[boundaries]]
from   = "domain"
to     = "infrastructure"
reason = "domain stays persistence-agnostic"

# Glob-level import bans. Unlike [[boundaries]] (which name layers), `from`/`to`
# are path globs: any import or reference edge from a `from`-matched file into a
# `to`-matched file is a violation, and a `forbidden_dependency` edge is
# materialised for it. Finer than boundaries — it can fence a dependency to a
# region of the tree.
[[forbidden_imports]]
from   = "src/web/**"
to     = "src/db/**"
reason = "the web layer must not import the db directly"

# Coverage contract. Every EXPORTED function or method under a `paths` glob must
# be reachable by a transitive `calls` path from some test. Unreached exported
# symbols are violations; non-exported symbols are exempt.
[[require_tested]]
paths  = ["src/api/**"]
reason = "the public API must have a test path"

# Documentation contract. Every EXPORTED symbol under a `paths` glob must be
# referenced by at least one DocSection. The documentation analog of
# [[require_tested]]: a targeted documentation gate, not total documentation.
[[require_documented]]
paths  = ["src/api/**"]
reason = "the public API surface must be documented"
```

Every constraint is optional — an omitted constraint is simply not enforced.
The table above is the recommended baseline, not a default; see the note above
the example. The **coupling budgets** (`max_fan_in`/`max_fan_out`) count a **module's**
distinct neighbouring modules — inbound / outbound — over the canonical
dependency graph (every edge kind except containment and member access), rolled
up to module grain and production-scoped (test-only modules are excluded before
the rollup, matching every other metric). A shared helper called from many
symbols in one module counts that module once, not once per call site — so the
budget flags genuine cross-module coupling, not name popularity (CR-065). The
**redundancy budgets** (`max_dead`/`max_duplicates`)
cap the project-wide count of dead / duplicate functions. The **structural

> **`max_dead` — absolute or delta-from-baseline.** `max_dead` accepts two
> shapes. The absolute integer above (`max_dead = 0`) caps the total dead count.
> The **delta-from-baseline** form pins a blessed steady-state and fails only
> when the count *rises* above it — useful when a codebase carries a known,
> human-reviewed residue of genuinely-dead code and you want to catch *new* dead
> code without first driving the count to zero:
>
> ```toml
> [constraints]
> max_dead = { baseline = 74, delta = 0 }   # fail if dead > baseline + delta
> ```
>
> `delta` is optional (defaults to `0`). You re-bless the `baseline` exactly as
> you re-bless the metric gate baseline: confirm the steady-state count on a
> freshly-indexed tree, then record it. A typo'd key fails loud at load. The
> two forms are interchangeable and backward-compatible — an unchanged
> `max_dead = N` keeps working.
>
> **Dead-code is only computed for languages whose plugin declares the
> reachability capability.** A callable whose language does *not* declare it
> reports `is_dead = NULL` ("not computed") rather than a guessed `true`, and
> NULL callables are excluded from the `max_dead` count. Today only Rust
> declares the capability; other languages report NULL until their binder
> coverage is proven, so `max_dead` never penalises a language Logos cannot yet
> resolve precisely.


budgets** (`max_nesting_depth`, `max_brain_methods`, `max_clone_ratio`,
`no_god_containers`) are the hard-gate counterparts of the five structural
metric dimensions — they let `logos check` *fail* on a structural problem the
[metrics signal](metrics.md#structural-metrics-610) only *measures*. Like every
other constraint they are production-scoped (test code is excluded) and
deterministic: violations are reported in a fixed order. They are evaluated by
`logos check` alongside layers and boundaries, and are **orthogonal to the
quality gate** — adding or removing them never moves the `gate`/`session_end`
signal.

`max_clone_ratio` is a fraction and is **range-validated**: a value outside
`[0.0, 1.0]` fails at load with exit 2, as does a non-positive
`[metric_thresholds]` value — a misconfiguration is a loud error, never a silent
skew.

`[[forbidden_imports]]` v1 covers **resolved intra-workspace** edges — both the
importing and imported files are indexed in the project. Banning an external
package (e.g. `to = "rusqlite"`) is not yet supported: the import target must be
a resolved graph edge, and external references live in the unresolved-reference
ledger rather than as edges. An invalid glob in any rule fails `logos check` at
load with exit 2.

`[[require_tested]]` turns "the public API must have a test" into an enforceable
gate: every **exported** Function or Method whose file matches a `paths` glob
must be reachable by transitive `calls` from some test node (the same `is_test`
definition and reachability `test-gaps` uses — see [commands.md](commands.md#test-gaps---limit-n)).
An exported symbol that no test transitively calls is reported with its `reason`.
**Non-exported symbols are exempt** (only the public surface is contracted), as
are non-callables. Each violation states the honest caveat — this is *static*
call-graph reachability, not execution coverage, so a symbol reached only through
dynamic dispatch reads as untested; scope each contract to `paths` where static
reachability holds. Multiple contracts are evaluated independently, and re-runs
are byte-identical.

`[[require_documented]]` is the documentation analog: every **exported** symbol
whose file matches a `paths` glob must be referenced by at least one
`DocSection` (a doc→code edge into it). It is a **targeted** gate, not a demand
that everything be documented — only the surface you opt in via `paths` is
contracted. Unreferenced exported symbols are reported with the `reason`;
non-exported symbols are exempt. The same gap set is available read-only via
[`logos doc-gaps`](commands.md#doc-gaps---limit-n). Because documentation is
metric-neutral, this contract gates `logos check` without ever moving the
quality signal.

### `[metric_thresholds]` — tuning the structural dimensions

The five structural metric dimensions (Nesting, Conciseness, Cohesion, Focus,
Uniqueness — see [metrics.md](metrics.md#structural-metrics-610)) use detection
thresholds you can tune. Every key is optional; an omitted key keeps its
documented default. The effective set (defaults composed with your overrides) is
hashed into every snapshot, so editing one triggers a one-time informational
re-baseline on the next `gate` rather than a silent shift in the signal.

```toml
[metric_thresholds]
nesting_depth    = 4     # Nesting: depth above which a function is "deeply nested"
brain_complexity = 15    # Conciseness: cyclomatic-complexity floor of a brain method (T_cc)
brain_lines      = 100   # Conciseness: line-count floor of a brain method (T_loc)
brain_nesting    = 3     # Conciseness: nesting floor of a brain method (T_bn)
god_methods      = 20    # Focus: method-count above which a container is "god"
god_span         = 500   # Focus: line-span above which a container is "god"
clone_similarity = 0.85  # Uniqueness: Jaccard similarity above which two functions near-clone (0–1]
clone_min_tokens = 50    # Uniqueness: minimum function token-length to be near-clone-eligible
```

A brain method (Conciseness) must trip **all three** of `brain_complexity`,
`brain_lines`, and `brain_nesting` at once. A god container (Focus) trips on
**either** `god_methods` **or** `god_span`. The `god_methods`/`god_span` keys
back both the Focus dimension and the `no_god_containers` budget, so the metric
and the gate count the same containers by construction. Every integer threshold
must be positive — a non-positive value fails at load with exit 2.

**Tuning a threshold alone never makes `logos check`/`logos gate` fail** — it
only recalibrates the (always-computed) quality signal for that dimension. The
gate turns on a dimension only when its **paired `[constraints]` budget** is
also set: `nesting_depth` pairs with `max_nesting_depth`; `god_methods`/
`god_span` pair with `no_god_containers`; `brain_complexity`/`brain_lines`/
`brain_nesting` pair with `max_brain_methods`; `clone_similarity`/
`clone_min_tokens` pair with `max_clone_ratio`. This is the second [CR-067]
incident this document now heads off: setting `brain_lines = 3` alone changes
nothing observable in `logos check`, because `brain_lines` is only one leg of
the three-part brain-method definition and feeds the Conciseness signal / the
(unset) `max_brain_methods` count — never a standalone gate check. Set the
matching constraint if you want the threshold to actually fail the gate.

[CR-067]: ../requests/CR-067-config-default-surfacing.md

The two **near-clone** keys tune the Uniqueness dimension. `clone_similarity` is
the Jaccard similarity at/above which two functions are grouped as near-clones;
it is **range-validated** to the half-open interval `(0, 1]` — a value at or
below 0, or above 1, fails at load with exit 2. `clone_min_tokens` is the
minimum normalized token-length a function needs to be near-clone-eligible (so
trivial boilerplate is never flagged); it must be a positive integer. Both keys
are folded into the **same hashed effective set** as the structural thresholds,
so tuning either one re-baselines the gate exactly like editing `nesting_depth`
— no rebuild, never a silent shift. With the defaults (`0.85` / `50`) the
effective-thresholds hash is unchanged, so an untuned project does not
re-baseline on upgrade.

### `[history]` / `[coverage]` — the evidence tiers

Two optional `rules.toml` tables tune the **non-gated** evidence tiers
(`logos hotspots`, `logos coverage`). Every key is optional with a documented
default, and — like `[metric_thresholds]` — the effective set is hashed into
every evidence snapshot, so changing a key is recorded with the data it
produced. Because the gate never reads `history.db`, **editing these tables can
never move the quality signal**; they only shape advisory output.

```toml
[history]
# HEAD-anchored churn window, in calendar months. The cutoff is computed from
# the HEAD committer timestamp, never the wall clock. Default: 12.
window_months = 12
# Mega-commit cap for co-change pairing: a commit touching more than this many
# files is skipped for pairing only (it still counts toward churn). Default: 50.
co_change_max_commit_files = 50
# Case-insensitive fix-commit message patterns for the defect heuristic. Always
# rendered with an explicit "heuristic" label downstream.
# Default: ["(?i)\\bfix(es|ed)?\\b", "(?i)\\bbug\\b", "(?i)\\bhotfix\\b"]
defect_patterns = ["(?i)\\bfix(es|ed)?\\b", "(?i)\\bbug\\b", "(?i)\\bhotfix\\b"]

[coverage]
# Path prefixes stripped from a coverage-report path before longest-unique-suffix
# matching against indexed files. Lets absolute build-dir paths (cargo-llvm-cov,
# pytest-cov) bind to repo-relative files. Default: [] (empty).
path_strip_prefixes = ["/home/ci/build/", "target/llvm-cov-target/"]
```

`window_months` must be ≥ 1 and `defect_patterns` must be non-empty — a
violation fails at load with exit 2, the same loud-failure contract as every
other table. Both tables are governed by the never-fabricate rule: a file with
no in-window history is omitted from the hotspot board (never zero-scored), and
a report path that matches no single indexed file is reported `unmatched`.

## Logging and telemetry

- **`RUST_LOG`** controls diagnostic verbosity on **stderr** (e.g.
  `RUST_LOG=debug logos index`). Stdout is never polluted: `--json` output
  and MCP frames stay machine-parseable at any log level.
- **Telemetry is local-only**: events (tool name, duration, ok/failure,
  surface, timestamp — never paths or source content) are appended to
  `.logos/telemetry.db` and feed `logos stats`. Recording activates only
  when `.logos/` already exists, never blocks a command, and degrades
  silently if the database is unwritable.

## Advanced: overriding language queries

Each language plugin's tree-sitter queries (symbol extraction, reference
extraction, framework detection) are embedded in the binary but can be
**shadowed by on-disk copies**:

```
.logos/plugins/<language>/queries/symbols.scm
.logos/plugins/<language>/queries/references.scm
.logos/plugins/<language>/queries/frameworks.scm
.logos/plugins/<language>/queries/smells.scm
```

A file present at one of these paths replaces the embedded query for that
capability at startup (a non-compiling query fails fast and names the file).
`smells.scm` is the optional fourth query — the test-quality smell patterns
surfaced in the `logos test-gaps` appendix; a language without one simply
reports `n/a` for smells.
This is the escape hatch for teaching Logos project-specific conventions
without rebuilding. The embedded queries under `logos-core/plugins/` serve as
reference starting points — each header documents the captures and the known
v1 limitations.
