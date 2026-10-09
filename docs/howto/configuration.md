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

## Workspace federation files — the manifest and the warm sidecar

A **workspace** is a parent directory of sibling repositories, federated by
[`logos init --workspace`](commands.md#init--i---hooks---workspace---yes---exclude-glob).
Its configuration lives at the *parent*, one level above the per-member `.logos/`
directories documented above — a member keeps its own `.logos/` and its own
`config.toml`/`rules.toml`, unchanged.

| Path (at the workspace root) | What it is | Check in? |
|---|---|---|
| `logos.workspace.toml` | The manifest: approved members plus your hand-written `default` / `autodiscover` / `[workspace.warm]` / `[workspace.member.<name>]` / `[[links]]` / `[governance]`. Re-running `init --workspace` preserves their keys and values; it re-serialises the file, so comments and formatting are not kept. Editable in the app — see [Editing the manifest from the app](#editing-the-manifest-from-the-app). | Yes |
| `.logos.workspace.warm.json` | Machine-written record of the last background warm's per-member outcome. Derived, host-local, safe to delete. | No |
| `.mcp.json` | Gains a single `logos-workspace` server key, deliberately distinct from a member's own `logos` key so neither shadows the other. | Yes |
| `.logos/config.toml` | *(optional)* The **workspace-level** `[chat]` policy that every member declaring none inherits (and a `[wiki].model`, inherited by a member that inherits this policy) — see [Workspace-level chat configuration](#workspace-level-chat-configuration). Never a member: a `.logos/` at the root is not admitted. | Yes |
| `.logos/secrets.toml` | *(optional)* The **workspace-level** chat API key, inherited the same way. `0600`, masked everywhere. | **No** — ignored by the root's own managed `.logos/.gitignore` (written with the first workspace-tier save), and by the root `.gitignore` block when the root is a git working tree |
| `.gitignore` | A managed block ignoring the warm sidecar and `.logos/secrets.toml`, maintained **only when the root is a git working tree** — a parent-of-repos root that is not a repository gains no file (`init --workspace` then reports `root_ignore: skipped`, and the credential stays out through `.logos/.gitignore`). | Yes |

**Unknown keys in the manifest fail loud.** It parses under `deny_unknown_fields`,
so a typo rejects the whole file rather than being silently ignored — the same
posture as `config.toml`.

### Editing the manifest from the app

With `logos serve --ui` running at the workspace root, the manifest is editable in the
workspace **Config** view (`/workspace-config`, see
[usage.md](usage.md#the-web-ui-dashboard)) or through two routes:

| Route | Does |
|---|---|
| `GET /api/v1/workspace/manifest` | The literal manifest `content`, its load `fingerprint`, the parse verdict (`parsed`, or `null` with the parser's `error` when it no longer parses — still a `200`, so the editor can repair it) and `governance_in_effect` (whether the `[governance]` family on disk is the one this serve evaluates). |
| `POST /api/v1/workspace/manifest/save` | Form `content=` (the whole manifest) and `fingerprint=` (the one the read returned; `400` without it). `200` `written` or `unchanged` (identical to disk, nothing written); `409` `conflict` when the file changed on disk since the read, with the document now on disk, and nothing written; `422` for a document the parser rejects, file byte-identical. |

The save carries the same same-origin + intent-token guard as every mutating route (a
bare `curl` gets `403`); both routes answer `404 not a workspace` under a single-root
server. Only fields you declare are written — the editor never adds a table the manifest
did not have. The write touches no member's `.logos/`, starts no member engine and
reindexes nothing. That property is about the write path: the serve process's own
telemetry, including one `config_read` / `config_write` event per manifest read or save,
goes to `<serve root>/.logos/telemetry.db`. That is the workspace root's store when
`serve` starts there, and the launching member's when `serve` starts inside a member,
as it is for every other route. **The running serve keeps the manifest it started with**: new
members, warm concurrency and changed `[governance]` rules take effect after
`logos serve` is restarted, and until then `governance_in_effect` is `false` and the
view says the findings beside the rules are over the rules it started with. Workspace
governance is advisory and never moves a member's gated signal.

### `[workspace.warm]` — bounding the background index

`init --workspace` returns immediately and hands indexing to a **single**
detached supervisor that warms at most K members concurrently — one supervisor
for the whole workspace, not one process per member. K defaults to
`max(1, cores / 4)`, capped at **4**, deliberately conservative because it is
derived for a host Logos knows nothing about.

```toml
members = ["archive-api", "mailbox-api"]

[workspace.warm]
# optional; default = max(1, cores / 4) capped at 4; explicit range 1..=16
concurrency = 2
```

An explicit value is accepted in `1..=16` and **rejected at parse time** outside
it, with the legal range named in the error. The ceiling is four times the
derived cap: high enough to let someone who knows their host overrule a
conservative default, low enough that a slipped digit fails instead of forking 200
indexers. Know what you are buying — each of the K is a full `logos index` child
that is itself parallel over your cores, so K costs roughly **K × cores** worker
threads, not K.

### `[workspace.member.<name>] kind` — documentation and mock members

A workspace often holds members that are not services: a repository of
architecture documents that keeps copies of every service's OpenAPI spec, or a
mock that stands in for an external API. Their spec copies read as
contract-surface **consumers**, so they fill the coverage headline with rows
that say nothing about the product. On the reference estate, one documentation
repository held 411 of the 868 contract-surface rows.

Declare such a member's kind, and its contract-surface rows are reported
**apart**:

```toml
[workspace]
members = ["software-architecture-documents", "pecserver-mock", "mailbox-api"]

[workspace.member.software-architecture-documents]
kind = "documentation"

[workspace.member.pecserver-mock]
kind = "mock"   # a stand-in provider of the API it mocks, never a consumer
```

- **The key.** Each table key is a member path as written in `members`. It can
  also be an autodiscovered directory name. `"./docs"` and `"docs/"` both mean
  `docs`. `kind` takes `"documentation"`, `"mock"` or `"platform"` (see
  [below](#kind--platform--build-hubs)). Any other value, or any
  other key in the table, is **rejected at parse time** (exit 2), and the error
  names the key and the legal values. A `kind` declared for a name that is no
  resolved member is ignored, with a warning. `init --workspace` preserves every
  table's keys and values — it re-serialises the manifest, so a comment such as
  the one on `pecserver-mock` above does not survive a re-run — and never writes
  a table itself: a kind is declared by you, never inferred.
- **What moves.** A declared member's contract-surface rows leave
  `coverage.by_intake.contract_surface`, the four headline counts and
  `spec_conformance_ratio`. They appear under `coverage.declared_apart`: the
  rows themselves, their four-bucket `counts`, each declared member with its
  `kind` and `rows`, and a `summary` line that states the count over its
  denominator, e.g. `"411 of 868 contract-surface rows reported apart from 1
  declared member (documentation: 1); the headline and spec_conformance_ratio
  exclude them"`. `spec_conformance_summary` itself ends with the same count
  (`"…; 411 of 868 contract-surface rows reported apart by declared member
  kind"`), so a surface that renders only that line still shows the
  population shrank. The reachability rider carries the same count as
  `declared_apart: {rows, contract_surface_rows}`. The member's routes still
  bind other members' calls. Its invocation rows still count, so
  `resolved_cross_service_edges` and `egress_resolution` never move by a
  declaration.
- **Candidates.** `workspace status` lists, under `kind_candidates`, the
  undeclared members that hold API documents and no runnable source (no file
  in a code grammar). The list is a hint for you to review; it classifies
  nothing. A candidate's rows stay in the headline until you declare it.
- **Nothing declared, nothing changes.** With no `[workspace.member]` table and
  no candidate, every surface renders exactly as before. `declared_apart` and
  `kind_candidates` are absent, not empty.

### `kind = "platform"` — build hubs

Members build against each other: a module inherits a parent POM, depends on a
shared library, imports a BOM. Logos reads those Maven and Gradle coordinates
into a **build-dependency relation**, `builds-against(A → B, kind, scope,
artifact)`, with its own headline, `build_dependency_pairs`, by kind (`parent`,
`dependency`, `managed`, `bom-import`) beside the references it was joined from.
It is **never a runtime coupling**: no build edge enters
`resolved_cross_service_edges`, `egress_resolution` or the bridge edge set.

A workspace usually has one or two members nearly everything builds against —
on the reference estate, a parent POM of 50 members and a shared library 32
depend on. Declare them `platform`, and the edges **into** them leave the
headline for `build_dependency.platform_apart`, over the same denominator:

```toml
[workspace.member.poste-pec-starter]
kind = "platform"

[workspace.member.poste-pec-common]
kind = "platform"
```

- **Only inbound build edges move.** A platform member's contract surface, its
  runtime coupling and its own outbound build edges are an ordinary member's.
- **Candidates.** `workspace status --json` lists, under
  `build_dependency.platform_candidates`, the undeclared members that at least
  2 members, and at least a quarter of the other members read, build against —
  each with its `in_degree` and the `of` it is stated over. It classifies
  nothing; a candidate stays in the headline until you declare it.
- **A coordinate two members produce resolves to neither.** It is listed under
  `build_dependency.collisions` with both producers, and every reference to it
  is counted as `to_collision`, never guessed onto one of them.
- **No build manifest, nothing changes.** `build_dependency` is absent from the
  status payload when every member was read and none holds a `pom.xml` or
  `build.gradle(.kts)`. A member that has never been indexed has not been read,
  so it keeps the section present, reason `"build facts not yet extracted"`,
  until its first `logos index`. A member whose facts could not be read keeps the
  section present. It is named under `build_dependency.members.unread`, and
  `build_dependency.members.unread_reasons` gives the reason.
- **An upgraded store is "not yet extracted" until it is fully re-read.** A
  member indexed by a release before the build relation carries no build facts
  until `logos index` or `logos health` runs in it. A partial `logos sync` does
  not count, even one that names a manifest. Until then the member is listed
  under `members.unread` with the reason `"build facts not yet extracted"`, and
  the section stays present. It never reads as a member with no manifest. A
  member with no build manifest reads as read, with 0 manifests, once one full
  re-read has run. Re-read every member after upgrading to see its pairs. Do it
  before starting `logos serve`, or restart the serve afterwards: its cached
  build relation (`xservice build-deps` and the map's build layer) does not see
  a re-read made by another process.

### Vendored specs — declared contracts and named externals

A member that keeps a copy of an API spec it does not implement is saying which
API it talks to. Logos reads that as a **declared-contract relation**,
`declares-contract(A → C)` with provenance `vendored-spec`. There is nothing to
configure beyond the `kind` declarations above: the relation is built in memory
on the first cross-service query, from spec documents already indexed. Nothing is
persisted and no store is migrated.

- **What counts as vendored.** Every OpenAPI document a member holds is judged
  against the coverage tier's own verdict on each of its operations. A document
  whose holder provides ≥ 90 % of its operations is the member's **own** spec. A
  document whose holder provides **none** of them is **vendored**, and declares
  one contract. Anything in between (`partial`), or a document with no keyed
  operation (`unjudged`), counts toward nothing.
- **To a member, by document identity.** A vendored document declares a contract
  to the member whose own spec holds ≥ 90 % of its operations (method and
  positional template). The score and the matched document are named, e.g.
  `webmail`'s `mailbox-aggregator.yaml` matches 31 of 31 operations of
  `mailbox-aggregator-api`'s own `src/main/resources/openapi/v1.yaml`. Two members
  at the same best score resolve to **neither**; the tie is listed under
  `collisions` and the document falls through to a named external.
- **Otherwise, to a named external.** Vendored copies that contain one another
  (≥ 90 % either way) are grouped into one **named external** — an API no member's
  own spec is. It is named by the spec's `info.title`, unless that is springdoc's
  default `OpenAPI definition`, else by the document's file stem (on the reference
  estate one such copy is named `v1`). **Names are not unique** — the estate has
  two groups titled `PSS` — so an external is identified by the `member:path` of
  the first copy in its group, e.g. `pecserver-facade:src/main/resources/pec-server/pec-server-api_v1.yaml`.
- **Member kinds.** A `kind = "mock"` member is a **stand-in provider**: its
  copies join the external they stand in for (listed under `stand_ins`), it
  declares nothing and it is never the member a document identifies. A
  `kind = "documentation"` member's copies stay out of the relation altogether.
  A `platform` member is an ordinary one.
- **Declared, never observed.** The relation has its own headline,
  `declared_contract_pairs`, over every spec document read (`own`, `vendored`,
  `partial`, `unjudged`, `mock`, `documentation`, which always sum to
  `documents`). It never enters `resolved_cross_service_edges`,
  `egress_resolution`, a coverage bucket or the bridge edge set. Where identity
  names a member, the holder's contract-surface ties that include that member are
  reported as resolved on the relation (`resolved_ties`); the coverage row itself
  stays ambiguous.

On the reference estate (83 members) the headline reads
`6 declared contract pairs (1 by document identity, 5 to named externals) from 7 vendored of 41 spec documents; 5 named externals; 15 contract-surface ties resolved by document identity`.

#### The external join: a no-provider call bound under a committed base path

A REST call whose provider is outside the workspace is `no-provider-in-workspace`.
When its member declares a named external, Logos tries to say **which** external
it calls. The call is bound to that external when its composed path, joined under a
base path the member's committed sources prove, **equals exactly** one operation
of an external the member itself declares.

The base path comes from the **base-url key**: an application-configuration key
that commits a URL (or a `${…}` indirection) in the same namespace as a key the
call's target names. Its path is then read from:

- **committed deploy overlays**, when any overrides the key: Helm values files
  (`values` in a YAML file's stem — `values.yaml`, `values_TEMPLATE.yaml`) and
  `docker-compose*.yml`, among the files the discovery walk admits (hidden
  directories such as `.helm/` are not), and never under a `docs/`,
  `documentation/`, `examples/`, `tutorial(s)/` or `src/test/` tree. An
  overriding overlay replaces the application value; all path-committing
  overlays must agree. Spring's environment-variable relaxed binding applies, so
  an overlay's `PECSERVER_BASEURL` overrides `pec-server.base-url`;
- **the application configuration** otherwise. A base URL with no path
  (`http://localhost:8083`) is a proven empty base path.

A call that cannot be bound is **refused**, with its reason: `no-declared-external`
(the member declares no external the path could reach), `external-not-declared-by-member`
(only another member vendors it), `no-base-key` (no key commits a base URL; a
literal target names none), `base-path-uncommitted` (the key is committed only as
an environment indirection, or a `${…}` sits in the URL's path), `base-paths-disagree`
(each path named with its file and key), `suffix-only` (the joined path is only
the tail of an operation — never admitted), `several-matches`, or `no-match`.

**The row does not move.** A bound call stays `no-provider-in-workspace` in every
count; the binding is reported beside it under `coverage.bound_external`, with the
external, the member's copy, the matched operation and the base path with its
origin and every file and key. It never becomes a bridge edge. On the reference
estate: `21 of 32 invocation no-provider-in-workspace REST rows bound to a named external their own member declares (refused: 10 no declared external, 1 no match)`
— 20 under `/prov` from four agreeing `deploy-*/values.yaml`, one under an
application-config base URL.

**No vendored spec, nothing changes.** `declared_contracts` is absent when no
member holds a vendored or `mock`-held spec; `bound_external` is absent when no
member declares a named external. See
[`workspace status`](commands.md#workspace-status) and
[`xservice route-providers`](commands.md#xservice-workspace-federation-queries).

### The warm sidecar

`.logos.workspace.warm.json` is how a *failed* background warm stays observable
after the terminal that started it is closed. Without it, a member whose index
genuinely failed reports `deferred` — "never attempted" — because the only
evidence lived in a supervisor process that has since exited and whose stderr the
detached spawn discards.

Four properties are worth knowing, because they are what make the file safe to
ignore:

- **A live index always outranks the record.** If a member holds an index, it reads
  `warm` even against a recorded failure, so a member indexed later by a re-run or
  by lazy first-query indexing needs nothing cleared.
- **It records successes as well as failures,** which is what lets a member that
  indexed cleanly but holds no supported-language file read `warm` rather than
  being misreported as never attempted.
- **A corrupt, truncated or future-version file reads as no record at all** — never
  an error. A bad sidecar can cost you evidence; it cannot fail a command. It is
  repaired only by the next warm, never on the read path.
- **It is never written inside a member's `.logos/`.** The supervisor holds no member
  store, and that is a literal property rather than an accident of layout.

Deleting it is always safe: status derivation falls back to index presence, which
is exactly the behavior that predates the file.

**Keeping it out of git is arranged, not left to you — on a tracked root.** The
workspace root can legitimately be a git working tree, since `logos.workspace.toml`
beside it is meant to be committed, and a `git add -A` there would otherwise pick the
sidecar up. So when the workspace root **is** a git working tree, `logos init
--workspace` maintains a managed `.gitignore` block at that root:

```gitignore
# Generated by `logos init --workspace` (FR-WS-02): the background warm's
# per-member outcome record and the workspace chat's conversations (FR-WS-34)
# are host-local state, and the workspace chat credential (FR-WS-30) is a
# secret — none of them travels.
# logos:managed:begin — regenerated by `logos init --workspace`; edit outside this block
/.logos.workspace.warm.json
/.logos/secrets.toml
/.logos/chat.db*
# logos:managed:end
```

The second and third entries are the [workspace chat](#the-workspace-chat--its-own-configuration-and-history)'s
credential and conversation store (`chat.db` plus SQLite's `-wal`/`-shm`). A root
enabled by an older build gains them on the next `logos init --workspace` run; the
header comment is written only when the file is first created, so an existing one
keeps its old wording.

The leading `/` is load-bearing, not decoration. An unanchored `.logos.workspace.warm.json`
matches at *every* depth below the file, and a workspace root's depth is the whole
estate: a member admitted by its `.logos/logos.db` alone carries no `.git` of its own,
so it shares the root's repository and an unanchored entry would reach inside it and
hide a same-named file there. A member that is its own git root is shielded by its own
repository boundary; that one is not. The sidecar exists at exactly one place — beside
the manifest — so the pattern says exactly that.

Same discipline as the generated `.logos/.gitignore`: an existing `.gitignore` is
appended to *within* the markers, your own lines are never rewritten, and a re-run
does not duplicate the block.

**When the root is not a repository, nothing is written there** — the canonical
parent-of-repos layout is deliberately not a git repo, and an inert ignore file in a
directory Logos was asked to federate rather than own would be litter. Two cases stay
manual, both harmless because a live index outranks the record (a stale one committed
on another machine cannot mislabel a member that has an index):

- A root that becomes a repository *after* enablement gains the entry on the next
  `logos init --workspace` run; until then, add the line above yourself — with its
  leading `/`, for the reason given above.
- A sidecar already committed is not retroactively un-tracked — mutating your git
  index is not something an `init` should do unasked. `git rm --cached
  .logos.workspace.warm.json` if you want it gone.

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
# Default exclude (CR-029/FR-CF-05, CR-154): ["docs/planning/**",
# "docs/security/**", "notes/**", "**/*.min.js"] — planning/security/notes prose
# is pruned from code AND docs, and minified JavaScript at any depth is kept out
# of code admission, out of the box. Set `exclude = []` (or your own globs) to
# replace it wholesale — see "Minified JavaScript and vendored copies" below.
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

# Import roots per language (S-519) — replaces the detected ones for a language
# whose plugin declares `import_roots` (Python: `src/` when it holds a package,
# else the repository root). Relative directories; "." is the repository root.
# A file under no listed root is keyed from the repository root. Default: none.
[resolution.import_roots]
python = ["lib", "tools/src"]

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

### Minified JavaScript and vendored copies

`**/*.min.js` is in the default code `exclude`, at the root and at every nested
depth: a minified file is never meaningfully navigable, and on a workspace with
vendored front-end libraries it can carry a large share of the TypeScript-language
access and method-call rows the resolver then cannot bind. `logos index` says
how many files the glob kept out, so the exclusion is never silent. The line is an
advisory **note**: it appears under `notes` in `index --json` (and in the human
output), never under `warnings`, so a CI step that scans `warnings` does not trip
on a default working as intended. The reconcile-backed readouts — `scan`, `check`,
`gate`, `dsm`, `doc_gaps` and the config-apply result — carry it on their own
`notes` field too, and omit that field entirely when there is nothing to note:

```text
73 minified JavaScript file(s) excluded from indexing by the `**/*.min.js` exclude glob (set your own `exclude` in .logos/config.toml to re-admit them)
```

- **`exclude` replaces the default, it does not add to it.** A `config.toml`
  that sets its own `exclude` (say `exclude = ["generated/**"]`) **re-admits
  `*.min.js`**. To keep them out while adding your own globs, restate the glob:
  `exclude = ["generated/**", "**/*.min.js"]`. The count covers only files the
  glob alone kept out, so it is absent while the glob is not in your `exclude`
  and never includes a minified file another of your globs already excludes.
- **Non-minified vendored copies are not detected.** Only the `*.min.js`
  filename is excluded — no heuristic guesses that `tinymce.js` or `bootstrap.js`
  is third-party. Prune those yourself with `exclude`, naming the directory that
  holds them (`exclude = ["docs/planning/**", "docs/security/**", "notes/**",
  "**/*.min.js", "styleguide/ui-kit/**"]`, restating whichever defaults you still
  want).
- **Upgrading narrows admission**, so the next `index`/`reconcile` purges the
  minified files' nodes from an existing graph (see the next section).

### Ignore files — `.gitignore` and `.ignore` at every depth

Besides `exclude` and `ignored_dirs`, discovery honours your ignore files:

- **Which files are read.** Every `.gitignore` and `.ignore` from the project
  root down to a file's directory, plus `<root>/.git/info/exclude`. They apply
  whether or not the project is a git repository. Logos never reads the global
  gitignore (`core.excludesFile`) or any ignore file above the project root.
- **Git's precedence.** A rule in a deeper file overrides a shallower one, and a
  `!negation` re-includes what a shallower rule excluded. As in git, a negation
  cannot re-include a file whose directory is excluded: `out/` in the root
  `.gitignore` keeps out everything under `out/`, whatever `out/.gitignore` says.
  An `.ignore` rule outranks a `.gitignore` rule at any depth.
- **One answer everywhere.** `logos index`, `scan`, every `sync`, the git hooks
  and the `serve` watcher admit the same files. Before 1.15.3 the watcher and
  partial syncs read only the root's ignore files. A file ignored by a nested
  `.gitignore` (a front-end subproject's `dist/`, a test harness's bundle) was
  indexed when written while `serve` ran, and it counted in rule findings and
  metrics until the next full reconcile.
- **Edits apply live.** With `serve` running, a change to a `.gitignore` or
  `.ignore` at any depth applies to the next watcher batch, with no restart. If
  the new rule excludes files that are already indexed under that directory,
  they leave the graph in that batch. Files written under an ignored directory
  are never indexed. A `.git/info/exclude` edit also applies to the next write,
  but files it excludes that are already indexed leave the graph only at the
  next full reconcile (`logos scan` or `logos index`): the watcher never syncs a
  path inside `.git/`.
- **Removing a rule** re-admits the files it excluded when each is next written,
  or at the next full reconcile (`logos scan` or `logos index`).
- **`logos doctor`** reports an indexed file that an ignore file now excludes as
  admission drift (`unadmitted_files`). `logos index` purges it.

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
| `strict` | Scope-proven bindings only (local → module → imports → explicit paths). | Maximum precision, lowest coverage. |
| `balanced` *(default)* | Strict, plus an exactly-one-candidate workspace fallback for path calls: a unique module-path suffix. | The sweet spot for most projects. |
| `aggressive` | Balanced, plus a bare identifier binds on a workspace-unique name. | Highest coverage; still deterministic. |

No policy changes how a receiver-method call (`x.f()`) binds: it binds by its
receiver's shape — `this.f()` / `self.f()` to the caller's own class, `super.f()`
through a proven base class, a call on any other receiver nowhere
([FR-RS-12](../specs/requirements/FR-RS-12.md)) unless its file proves the
receiver's type (Java, [FR-RS-10](../specs/requirements/FR-RS-10.md); Rust,
[FR-RS-42](../specs/requirements/FR-RS-42.md)), which no policy widens either.

Changing the policy needs no migration — resolution re-evaluates the whole
unresolved-reference ledger on every run, so just `logos index` again. The same
holds for `[resolution.import_roots]`.

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

# ── Prior-turn window (follow-up turns) ─────────────────────────────────────
# A follow-up turn sees the thread's earlier turns (user + assistant, oldest
# first) in the planner's prompt and the Synthesizer's instruction — bounded by
# BOTH keys below; the oldest whole turns are dropped first and the prompt says
# how many were omitted.
# How many of the most recent earlier turns to show. Default: 6. In [1, 50].
history_max_turns = 6
# Character ceiling on the earlier turns' text. Default: 16000. In [1, 200000].
history_max_chars = 16000

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

# ── Extra read roots (optional) ─────────────────────────────────────────────
# Directories the Source-Reader may read THROUGH symlinks in this project — for
# docs/ folders symlinked into a sibling repo. Each entry is relative to the
# root whose config.toml declares this table, or absolute. Default: none, and
# the sandbox is exactly the project root. See "Reading symlinked docs" below.
# read_roots = ["../logos-docs"]

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
| `history_max_turns` | integer | `6` | Prior-turn window: how many of the thread's most recent earlier turns a follow-up shows the planner and Synthesizer. Must be in `[1, 50]`. |
| `history_max_chars` | integer | `16000` | Prior-turn window: character ceiling on those turns' text. Must be in `[1, 200000]`. Whole turns only — an earlier answer is never cut mid-text, so a single turn larger than the ceiling is omitted. |
| `max_provider_retries` | integer | `2` | Retries after the first attempt for a transient provider fault. `0` disables retry. Out-of-range fails loud. Inherited by the wiki generator. |
| `provider_retry_base_ms` | integer | `200` | Base backoff (ms) for the exponential + jitter retry delay. Must be ≥ 1 (`0` fails loud). Inherited by the wiki generator. |
| `read_roots` | list of strings | `[]` | Extra directories the Source-Reader may read, reached **only through symlinks inside the project** — see [Reading symlinked docs](#reading-symlinked-docs--read_roots). Relative to the declaring root, or absolute. A blank entry fails loud at load. |

The `[chat.models]` table maps a fixed set of roles — `planner`,
`graph_navigator`, `governance_analyst`, `source_reader`, `synthesizer` — each to
a model string. Every key is optional; an omitted role uses the top-level
`model`. An unknown role key fails loud at load.

### Reading symlinked docs — `read_roots`

The Chat tab's Source-Reader reads files with sandboxed `read` / `grep` / `glob`
tools confined to the project root. A project that keeps its docs in a sibling
repo — `docs/planning → ../logos-docs/planning` — therefore could not read them:
the symlink resolves outside the root, the `read` is refused as a sandbox escape,
and that refusal ends the turn. Declare the sibling repo instead:

```toml
[chat]
read_roots = ["../logos-docs"]
```

What this admits, and what it does not:

- **Reached through the project only.** The agent still names project-relative
  paths (`docs/planning/sprint-log.md`). An absolute path or a `..` component is
  refused exactly as before, so a read root is reachable only through a symlink
  inside the project that resolves into it — never by naming it.
- **Only the declared directories.** A symlink to any other directory is still a
  sandbox escape and still ends the turn ("resolves outside the project root").
  Containment is by path component, so
  `../logos-docs-private` is not under `../logos-docs`.
- **`grep` and `glob` see them too**, even when the symlink is git-ignored (this
  repo's `/docs/planning` is). The walks follow a symlink only when its target is
  under a declared read root — every other symlink is still skipped — and report
  every hit under its in-project path (`docs/planning/…`), so two links to one
  directory are both listed. A link whose target is, or encloses, a directory
  the walk reached it through is not followed, so a link cycle terminates; one
  call follows at most 256 links and says `truncated` past that.
- **`ignored_dirs` still apply** inside a read root, and so does the read root's
  own `.gitignore` (from the read root down; never a directory above it): a
  walk through `docs/planning` does not list what `../logos-docs/.gitignore`
  keeps out of git. As in the project tree, `read` of a named gitignored file
  still works; only the walks skip it.
- **Resolved when a turn starts.** A relative entry resolves against the root
  whose `config.toml` declares the `[chat]` table. An entry that does not exist,
  or is not a directory, fails the turn with a message naming it (`could not
  open the source sandbox: [chat] read_roots entry "../logos-docs" … does not
  exist`) — it is never silently dropped.
- **Its content can be sent to the endpoint.** The Chat tab's consent banner and
  status band name every declared read root.

`read_roots` has no typed control in the Config tab; edit it in the raw TOML
pane. Typed edits to the other `[chat]` fields leave it byte-for-byte intact.

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

### Follow-up turns: the prior-turn window

The planner's prompt is the current question plus this turn's observations, so
without a window a follow-up such as "and for mailbox-manager?" would be answered
as if nothing had been said. A follow-up turn is therefore shown a **bounded
window of the thread's earlier turns** — the user's messages and the assistant's
answers, oldest first — both in the planner's prompt and in the Synthesizer's
instruction. They are read from the conversation itself (the thread's stored
messages), and are context only: the prompt tells both roles that a claim about
the codebase must still rest on this turn's observations, not on an earlier
answer.

- **`history_max_turns`** and **`history_max_chars`** bound the window together:
  the newest turns are kept until either bound would be breached, and the oldest
  go first. When anything is dropped, the prompt states how many earlier turns
  were omitted — the window never passes itself off as the whole conversation.
  Turns are kept whole, so a single turn larger than `history_max_chars` is
  omitted rather than cut. Both are validated at load like the budget keys; a
  value of `0`, or above the maximum, fails loud.
- The first turn of a thread has no earlier turns and is prompted exactly as
  before. A deleted conversation contributes nothing.
- **Regenerate** replaces: a regenerated turn takes its predecessor's place in the
  window rather than appearing twice. A turn that was halted or failed has no
  answer, so the window shows its question with no answer rather than inventing
  one.

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
[usage.md](usage.md#interacting-with-the-dashboard).

### Workspace-level chat configuration

In a [workspace](#workspace-federation-files--the-manifest-and-the-warm-sidecar),
the Chat tab and wiki generation run against the **selected member**, so without a
workspace tier every member would need its own `[chat]` and its own key. Instead,
declare them **once at the workspace root**, in the same two files and the same
format as a member:

```text
<workspace-root>/.logos/config.toml     # [chat] model / provider / base_url / budgets
<workspace-root>/.logos/secrets.toml    # [chat] api_key   (gitignored, 0600)
```

**Inheritance is per half, and the member wins wherever it declares — except that a
member's own key is never sent to a workspace endpoint:**

| Member declares… | Policy (`[chat]` table) comes from | Key comes from |
|---|---|---|
| neither | workspace | workspace |
| `[chat] model` only | member (**the whole table** — no field is taken from the workspace) | workspace |
| a key only | workspace | **workspace** — the member's own key is **not used** (withheld); with no workspace key the member is configure-first |
| both | member | member |

- **The policy half is atomic, keyed on `model`.** A member that declares a non-blank
  `[chat] model` uses its own table *entire* — its `base_url`, provider and budgets —
  and draws nothing from the workspace table. A member with no `model` inherits the
  workspace table *entire*. A blank `model = ""` counts as undeclared.
- **Keys cross the boundary in one direction only.** A member that declares its own
  `model` + `base_url` but no key sends the **workspace** key to **its own** endpoint —
  declare a key on that member too if its endpoint must not receive the shared key.
  The reverse never happens: when the policy is inherited from the workspace, the key
  comes from the workspace or not at all, so a member's own key never reaches a
  workspace endpoint. The Chat tab, the member Config tab and a refused request say
  when a member's key is withheld this way; setting a `[chat] model` on the member
  makes it use its own key.
- **`read_roots` travel with the table.** An inherited table's `read_roots` resolve
  against the **workspace root** that declared them, not the member; the Chat tab
  says so beside them. They still admit only what a symlink inside the member
  reaches.
- **Wiki generation inherits the same way.** The wiki model resolves in this order:
  the member's own `[wiki].model`; else the **workspace root's** `[wiki].model`, but
  only while the member **inherits the workspace `[chat]` policy**; else the effective
  chat model. A member that declares its own `[chat] model` owns its endpoint, and a
  workspace model name may not exist there, so it never receives the workspace wiki
  model. The provider, endpoint and key always come from the effective chat resolution
  above.
- **Single-root projects are unchanged**: with no workspace there is no inherited
  tier, and nothing reads above the project root.
- **Fail-loud:** an invalid workspace-root `config.toml` or `secrets.toml` makes the
  Config read (`GET /api/v1/config`) of every member that would inherit from it
  answer `500`, naming the file. The workspace tier's own read does **not** fail: it
  delivers the broken `config.toml` for repair — the literal document, `parsed: null`
  and a fault naming the file and the line/column (or the offending key), never a
  fragment of the file. In workspace mode the member Chat and Config tabs show that
  failure with a link to the workspace Config view. Repair it there, or by saving a valid
  document through the write route below — a save validates the **new** content,
  never the broken one — or by editing the file by hand. A broken workspace
  `secrets.toml` is named the same way and its contents are never shown, but it has
  no in-app repair: the key writer refuses it (`422`) and leaves it byte-identical, so
  fix or delete that file by hand.

**Where you see the effective values.** The member Config tab shows an inherited
chat value as a read-only note and still edits and saves **only the member's own
document** — saving never copies an inherited value into the member's
`config.toml`. The workspace tier is edited in the workspace **Config** view (the
Workspace section of the sidebar, beside the manifest), by editing the two files, or
through the web API while `logos serve --ui` is running at the workspace root:

| Route | Does |
|---|---|
| `GET /api/v1/workspace/config` | Reads the workspace tier: `config` (the literal `content`, its load `fingerprint`, `exists`, `parsed` — `null` with an `error` when the file is invalid), the masked `chat_key` (`null` with a `chat_key_error` when `secrets.toml` cannot be read) and `effective_chat` (origins read `member` = declared at this root, `unset` = not declared; `null` when either file is invalid). No `rules.toml` is read at this root. |
| `POST /api/v1/workspace/config/save` | Writes `<workspace-root>/.logos/config.toml` (form `content=` and `fingerprint=` — the one the read returned; `400` without it). Validated before write (`422`, file untouched), atomic replace; a document identical to disk is `unchanged` and writes nothing; a file changed on disk since the read is refused `409` with the document now on disk, and nothing is written. `rules` is refused (workspace governance lives in the manifest). |
| `POST /api/v1/workspace/config/secret` | Writes `<workspace-root>/.logos/secrets.toml` (form `api_key=`, blank clears), `0600`, response masked. |

The two POST routes carry the same same-origin + intent-token guard as every
mutating route (a bare `curl` gets `403`), all three answer `404 not a workspace`
under a single-root server, and none offers an apply/reindex action — saving at the
workspace root starts no member engine and triggers no reindex.

### The workspace chat — its own configuration and history

In a workspace serve (`logos serve --ui` at the workspace root, `agents` build) there
is a second chat beside each member's: the **workspace chat**, which answers for the
workspace first and narrows to a member when a question names one. It is the **Chat**
entry in the app's Workspace section, at `/workspace-chat` (see
[usage.md](usage.md#the-workspace-chat)); in a workspace serve the member Chat is not
offered, and `/chat` lands there. Two things set it apart from a member's chat:

- **Its configuration comes from the workspace tier alone** — the
  `<workspace-root>/.logos/config.toml` `[chat]` table and
  `<workspace-root>/.logos/secrets.toml` key above. No member's `[chat]` configures
  it (a member's table is read only for its `read_roots`, below), so a member that
  declares a complete `[chat]` of its own does **not** configure the workspace
  chat. With either half missing at the workspace root, a turn answers
  the configure-first message naming the workspace root and the missing half, and
  points at the workspace **Config** view. Every other `[chat]` key — budgets,
  retries, `history_max_turns` / `history_max_chars` — is the workspace table's too.
- **Its history lives at the workspace root**, in `<workspace-root>/.logos/chat.db`.
  No member's `.logos/chat.db` is created or changed by a workspace turn, and the
  member chats keep their own histories — reachable in a `--standalone` serve of the
  member, which is a single-root serve. At a workspace root that is a git working
  tree the managed root `.gitignore` above keeps the store out of git; elsewhere the
  workspace root's own `.logos/.gitignore`, written by the first save through the
  routes above, does.

When its source tools read a named member, they read through **that member's**
sandbox — its root, its `ignored_dirs` and the `read_roots` its own chat would use.
A member that declares no `[chat] model` inherits the workspace table whole, so the
workspace table's `read_roots` (resolved against the workspace root) apply to it and
its own `read_roots` do not.

These read roots are checked **when a turn starts**, before anything is recorded:
first the workspace table's against the workspace root (always, even when no member
inherits the table), then each member's effective ones in the workspace's member
order. An entry that does not exist, or is not a directory, fails the
turn with the member chat's message, naming the root that declared it, the entry and
why — `could not open the source sandbox of the workspace root /work/shop: [chat]
read_roots entry "no-such-docs" … does not exist`, or `… of the workspace member web:
…` for a member that owns its `[chat]`. The refused turn creates no conversation.
Only a bad entry fails the turn: a member whose `config.toml` or `secrets.toml` cannot
be read, or whose directory is gone, is skipped, and the fault is reported on that
member's own source calls instead. The check reads config files only and starts no
member engine.

| Route | Does |
|---|---|
| `POST /workspace/chat` | One turn (form `q=`, optional `thread=`), streamed as Server-Sent Events with `Accept: text/event-stream`, else the buffered answer — the `POST /chat` contract. `?repo=` is ignored. |
| `GET /api/v1/workspace/chat/threads` | The workspace chat's conversations, most recent first. |
| `GET /api/v1/workspace/chat/threads/{id}` | One conversation's messages; `404` for an unknown id. |
| `POST /api/v1/workspace/chat/threads/{id}/delete` | Deletes one conversation; `404` for an unknown id. |
| `GET /api/v1/workspace/config/read-roots` | Every member's effective `[chat] read_roots`, with where its policy came from (`policy_origin`) and the root they resolve against (`declared_by`: `member` or `workspace`; `null` when that member's chat config cannot be read). Config reads only — no member engine is started. |

The two POST routes carry the same-origin + intent-token guard, and every route above
answers `404 not a workspace` under a single-root server. A build without `agents` has
no chat routes at all; the read-roots route, which reads config only, is served in
every `ui` build.

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
endpoint, or secret. In a workspace they come from the *effective* chat
resolution, so a member inheriting the workspace `[chat]` and key can generate
its wiki too (see [Workspace-level chat configuration](#workspace-level-chat-configuration)). When `[wiki].model` is omitted, wiki generation falls back
to the workspace root's `[wiki].model` if the member inherits the workspace `[chat]`
policy, and otherwise to `[chat].model`.

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
cap the count of dead / duplicate functions. Both budgets are **production-scoped**:
functions Logos classifies as test code (`is_test`) are excluded from the count,
so the budgets agree by construction with the Redundancy metric they mirror and
with the sibling structural budgets (a dead or duplicated *test* never counts
against them). The **structural

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
definition and static call-graph reachability the quality metrics use).
Violations surface through `logos check` / the Rule findings surface.
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
duplicate_min_tokens = 50  # Redundancy: minimum body token-length to be an exact duplicate
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

`duplicate_min_tokens` tunes the **Redundancy** dimension and the
`max_duplicates` budget — the *exact*-duplicate detector, not the near-clone one.
A function is an exact duplicate only if it **has a body** and that body has at
least `duplicate_min_tokens` normalized tokens (identifiers, literals and
comments are normalized away, so this counts structure: operators, keywords and
punctuation). The comparison is inclusive. It exists so that the 26
four-line `getId() { return 7; }` overrides of one interface method, or a pair
of abstract/interface declarations, do not read as copy-paste; two renamed
copies of a real 14-line function still do. Lowering it (say to `5`) makes short
bodies count again; a bodyless declaration is never a duplicate, whatever the
value. It must be a positive integer (non-positive fails at load with exit 2),
and it joins the same hashed effective set as the other keys, so editing it
triggers the announced one-time re-baseline. It is independent of
`clone_min_tokens`, which gates near-clone shingling only — near-clone groups
are unchanged by it. A store indexed before the body facts were recorded keeps
its previous duplicate verdicts until its files are re-extracted (the next
`logos scan` or `logos index`), rather than reading every function as bodyless.

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
.logos/plugins/<language>/queries/invocations.scm
.logos/plugins/<language>/queries/brokers.scm
.logos/plugins/<language>/queries/properties.scm
```

A file present at one of these paths replaces the embedded query for that
capability at startup (a non-compiling query fails fast and names the file).
This is the escape hatch for teaching Logos project-specific conventions
without rebuilding. The embedded queries under `logos-core/plugins/` serve as
reference starting points — each header documents the captures and the known
v1 limitations.

### Module models (`[module_model]`)

By default Logos keys a file's module by its path: the directory before
the last `src/` names the crate and every later directory is a module. So
`src/main/java/com/x/Svc.java` would be `main::java::com::x::Svc`, and
`import com.x.Svc` could never reach it. A plugin names the model its language
uses instead:

```toml
[module_model]
kind = "namespace"   # or "package", or "path" (the default)
```

| `kind` | Keyed by | Shipped for |
|---|---|---|
| `path` | the file's path (the default when the table is omitted) — under an **import root** when the plugin declares them | Rust, Python (import roots), Go, TypeScript, … |
| `package` | the path after a **source root** named in `[package_modules]` | Java |
| `namespace` | the namespace or package the file **declares** | PHP, C#, Kotlin, Scala |

**`path`.** Two keys refine it, both optional:

```toml
[module_model]
kind = "path"
package_stems = ["__init__"]   # a package file names its directory
import_roots = ["src"]         # candidate roots; the repository root is the fallback
```

- `package_stems` — a file with one of these stems names its **directory**
  rather than adding a module of its own: Rust declares `mod`, `lib` and `main`,
  Python `__init__`. A stem no plugin declares is just a name, so a JavaScript
  `main.js` is the module `main`.
- `import_roots` — the file is keyed by its path under the import root that
  holds it. A candidate is chosen when a package sits beneath it (for Python, a
  directory with an `__init__.py` under `src/`); otherwise the repository root
  is the import root, and a file outside every root is keyed from it too. A
  root is a directory of the repository, matched from the path's start. Every
  directory under a root is a module, so a namespace package with no
  `__init__.py` still descends. A relative import keeps its level: `from .rules
  import Rule` is the importing file's own package, `from .. import x` its
  parent. An import of a name a package's `__init__.py` re-exports without
  declaring binds to the package. `pyproject.toml`'s `package-dir` is not read
  — declare such roots in `.logos/config.toml` instead (below).

Python records one import row per imported name (`from a import b, c` is two
rows), and its imports reach only Python modules: neither a module path nor a
fallback crosses into another language's files. An `as` import (`from m import a
as b`) binds the import but gives the file no name `a`, and a name imported twice
(a `try`/`except` compat import) binds a call only where both imports agree. Two
`as` names of one target (`from m import X as A` and `from m import X as B` in one
scope) both bind, and so do `import numpy` beside `import numpy as np` and the
Kotlin and Scala `import … as …` twins.

**`family`.** Languages that can name each other's types declare one **interop
family** (any kind), and the type and namespace lookups never cross it:

```toml
[module_model]
kind = "namespace"
family = "jvm"   # Java, Kotlin and Scala share it
```

The fully-qualified type index and the namespace index are partitioned by
family, so a Java import still reaches a Kotlin class while a C# `using
App.Models;` never binds a PHP file declaring `namespace App\Models;`. An
import-root language (Python) is keyed under its family's own crate, so its whole
module tree stays inside the family too. A plugin that declares no family is its
own. The policy-gated workspace fallbacks of the other models (`balanced` suffix
match, `aggressive` unique name) are not partitioned.

**`package`.** The source roots live in their own table:

```toml
[module_model]
kind = "package"

[package_modules]
source_roots = ["src/main/java", "src/test/java"]
```

A file under one of these roots is keyed by the path after the root, so the file
above is `com.x.Svc`, the name its imports spell. A descriptor that declares
`[package_modules]` without `[module_model]` is read as `package`, as it was
before the table existed; declaring the roots under any other kind is refused.

**`namespace`.** The plugin's `symbols` query captures each namespace or package
declaration's name with `@module.namespace`. A file's identity is the namespace
its top-level declarations sit in — PHP's `namespace App\Models;`, C#'s
file-scoped `namespace App.Models;` or block `namespace App.Models { … }` (nested
blocks compose), Kotlin's and Scala's `package` (Scala's chained clauses
compose). The directory plays no part, so a PSR-4 tree, a C# project whose
namespaces differ from its folders, and a Kotlin Multiplatform `commonMain`
source set all bind the same way. Composer maps and `.csproj` files are not read.
A file declaring no namespace is in the global one; a file whose top-level
declarations sit in two different namespaces keeps its path key, rather than
naming one set of its types wrongly.

With either model:

- a single-type import binds to the **type** it names (a Java static import to
  the member), and is final for that name — it shadows a same-named type a
  wildcard would bring in;
- a type in the file's own package or namespace binds without an import;
- a wildcard brings the package's or type's members into scope: Java's `a.b.*`
  and `static a.b.C.*`, Kotlin's `a.b.*`, Scala's `a.b._`, and C#'s plain
  `using A.B;`. Under the `namespace` model the wildcard itself binds to every
  other file declaring that namespace; a Java package wildcard binds nothing;
- C#'s `global using A.B;` applies to every C# file under the directory of the
  file that declares it — by .NET convention the project root;
- a type declared under one fully-qualified name twice (`src/main` and
  `src/test`, or a Scala class and its companion object) stays unbound rather
  than being guessed, and so do library imports (JDK, Spring, PSR, `System`);
- two single-type imports of one simple name from different packages
  (`import a.Helper; import b.Helper;`) make the name ambiguous: a call or
  `extends` through it (`Helper.util()`, `Helper.Inner`) binds nothing and is
  recorded as `type-ambiguous`. Imports that reach one declaration, and a verbatim
  repeat, bind as before, and an import of a name no type in the repository
  carries never shadows one that does.

**`enclosing_namespaces` (`namespace` model only).** C# resolves a type name by
walking outward through the namespaces that enclose the one it is written in. A
plugin opts in, and the C# plugin does:

```toml
[module_model]
kind = "namespace"
enclosing_namespaces = true   # refused under any other kind
```

A simple type name, or the head of a qualified one, that the file's own namespace
does not supply is then looked up in each enclosing namespace, nearest first
(`A.B`, then `A`, for code in `A.B.C`), before any `using` namespace. One type
decides a level; two at one level bind nothing (`type-ambiguous`) and the walk
never falls through to an outer level. Only the file's own interop family is
read, and the global namespace is never a level. Two known limits, shared with
the same-namespace lookup:

- types are matched by name only, so a generic `Result<T>` in an enclosing
  namespace is taken for a non-generic `Result` a `using` supplies;
- a `using` written inside a namespace block is read after the enclosing
  namespaces, where C# reads it before them.

### Call targets (`class_call_instantiates`, `macros_callable`)

A call binds a function or a method. Two `plugin.toml` keys let it bind more:

```toml
# Python, Kotlin, Scala: `Foo()` constructs a `Foo`.
class_call_instantiates = true

# C: `f(x)` may expand a `#define f(x)`.
macros_callable = true
```

- With `class_call_instantiates`, a call whose one candidate is a class records an
  **`Instantiates`** edge to that class, not a `Calls` edge. A Python
  `Check(project=p)` after `from hc.api.models import Check` instantiates
  `hc/api/models.py`'s `Check`.
- With `macros_callable`, a call whose one candidate is a macro records `Calls` to
  that macro node. C headers belong to the C++ plugin, so a macro defined in a
  `.h` file is no candidate.

The rule that binds every call still holds: exactly one candidate, or nothing. Two
classes of one name stay unbound. So do a function and a class (or macro) of one
name in one scope, and a name that matches no declaration. Kotlin and Scala read
a bare `Foo()` inside a class as a call on the current instance, so it binds among
that type's own members and never reaches the class. Both keys default to
`false`, and a plugin that declares neither binds its calls as before.

### Supertypes (`@ref.extends`, `@ref.implements`, `supertype_kind_follows_target`)

A type's supertypes are captured by two `references` query captures and bound
through the module model the language declares — the package rungs, a declared
namespace, or the path modules:

- `@ref.extends` records an **`Extends`** row: a class's base class, an
  interface's super-interfaces;
- `@ref.implements` records an **`Implements`** row: a class's interfaces, and a
  PHP class's `use`d traits.

Python (`class A(B)`), PHP (`extends`, `implements`, trait `use`), C#
(`base_list`), Kotlin (the supertype list) and Java capture them. A supertype
binds only to the one in-repository type its file's scope names — its own
module, its imports, its namespace or package, its wildcards — and never by a
workspace-wide name guess, under any binding policy. A library base (`TestCase`,
`IDisposable`, `\Psr\Log\LoggerInterface`) stays unbound. PHP's leading `\` and
C#'s `global::` make a name fully qualified: `namespace Foo; class Exception
extends \Exception` never names itself. An `Extends` binds a class (an interface,
from an interface); an `Implements` binds an interface or a trait.

C# and Kotlin write the base class and the interfaces in one list (`class A : B,
IC`, `class A : Base(), Iface`), so their descriptors declare:

```toml
# C#, Kotlin: the supertype list does not say which entry is the class.
supertype_kind_follows_target = true
```

Each entry is then captured as `@ref.extends`, and a class's entry binds the one
class, interface or trait it names. Its edge kind follows the target:
**`Extends`** to a class, **`Implements`** to an interface. The key defaults to
`false`, where each clause binds the kind it spells.

A proven `Extends` is also what a call on the current instance climbs: a Python
`self.m()`, PHP `$this->m()`, C# `this.M()` or unqualified Kotlin/C# `m()` reaches
an inherited `m`, and Python's `super().m()`, PHP's `parent::m()`, C#'s `base.M()`
and Kotlin's `super.m()` bind the nearest supertype that declares exactly one
`m`. An `Implements` is not part of that chain, so `base.M()` in a class whose only
supertype is an interface binds nothing (a language declaring
`inherits_interface_bodies`, below, visits interfaces after the chain). The walk reads one base class per level,
so it never climbs through a Python class with several bases (its MRO decides) or
a PHP class that uses a trait (the trait's method outranks the inherited one).
Python's `super(A, self)`, Kotlin's `super<T>` and `super@Outer` name another
starting point and stay unbound. A class's header never names the class itself:
`from unittest import TestCase` then `class TestCase(TestCase)` names the import.
A PHP `namespace\X` supertype is not captured. Rust's `impl Trait for X` methods
bind their trait as before.

Java and Kotlin classes inherit the bodies of the interface members they
implement, so their descriptors declare:

```toml
# Java, Kotlin: a class inherits its interfaces' `default` bodies.
inherits_interface_bodies = true
```

With it, a call no class of the `Extends` chain answers goes on to the
interfaces the chain's classes implement, then their super-interfaces, nearest
level first. A superclass method always beats an interface default. Only a
member with a body is a candidate there. An abstract member never is, nor one
the `symbols` query marks `@item.uninherited` (Java's `static` and `private`
interface methods, Kotlin's `private` interface functions and an interface's
`companion object` functions); a plugin whose `symbols` query has no
`@item.uninherited` capture is refused at load. Two unrelated
defaults of one name bind nothing, and arity applies. A chain that crosses a
class whose base class is not in the graph reaches no interface, since that base
may declare the method. The key defaults to `false`: C#'s default interface
member is reachable only through an interface-typed receiver, so C# does not
declare it.

### Receivers (`implicit_receiver`, `[wrapper_methods]`)

`implicit_receiver` says what a bare call (`f()`, no receiver written) can
mean inside a method. Omitting the key and writing `"none"` read the same for
extraction, but only an explicit `"none"` also tells the binder that a bare
call never reaches an instance member:

```toml
# Go, Rust, Python, PHP, TypeScript (and its .js files), TSX: a method is reached only through a
# receiver, so a bare `f()` binds a free or imported `f`, never a method `f`.
implicit_receiver = "none"
```

With it, a bare `f()` skips every member of a class-like container and every
callable with a recorded self type (a Go or Rust method). It binds the free,
imported or nested function `f` when there is one, and otherwise stays unbound —
never a self-loop to the method it sits in. Java declares nothing, so its bare
in-class call still binds the method it means (`this.m()`).

`[wrapper_methods]` is for a language whose receiver typing peels wrappers off a
declared type (Rust: `x: Arc<T>` proves `T`). It names, per wrapper, the
methods the wrapper provides itself:

```toml
[wrapper_methods]
Arc = ["clone", "as_ref", "borrow"]
```

A call through that wrapper to one of these methods (`x.clone()` on an
`Arc<T>`) binds nothing — it is `Arc::clone`, whatever `T` defines — and is
counted `external-type`. A wrapper with no entry provides nothing. The keys are
the wrappers extraction peels (`Box`, `Arc`, `Rc`); another key never matches.
Declaring the table also makes `status` report the language's `call_residue`
([Commands](commands.md)). It defaults to empty.

### Arity (`overloaded_calls`, `arity_unchecked_extensions`, `implicit_call_falls_through`, `implicit_root_members`)

Every callable records how many arguments it accepts, and every call how many it
passes. A `self`, `super` or typed-receiver call in any language binds only a
candidate whose parameter count admits the call ([Usage](usage.md)). Four
`plugin.toml` keys decide the rest:

```toml
# Java, Kotlin, Scala, C#, C++: a bare `f(x)` also binds only an `f` that
# admits one argument, and a scope whose `f`s admit none is passed over.
overloaded_calls = true

# TypeScript (`js`, `mjs`, `cjs`) and TSX (`jsx`): JavaScript enforces no
# arity, so a call in these files is never filtered. The range is still recorded.
arity_unchecked_extensions = ["js", "mjs", "cjs"]

# Kotlin only: an unqualified in-class call that no member admits goes on to
# the top-level functions and imports in scope ...
implicit_call_falls_through = true
# ... except these names, which every class inherits from a root (`Any`) the
# graph never holds.
implicit_root_members = ["equals", "hashCode", "toString"]
```

- `overloaded_calls` defaults to `false`: a bare call binds by name alone. It
  decides only the bare-call rung; the receiver walks filter in every language.
- Each `arity_unchecked_extensions` entry must be one of the plugin's own
  `extensions`, written bare. It defaults to empty.
- `implicit_call_falls_through` requires `implicit_receiver = "self"` and a
  references query that captures supertypes; a plugin that breaks either rule
  is refused at load. The fall-through is taken only when every supertype of the
  class is in the graph. Java, C#, Scala and C++ leave it off, because a member
  hides every outer name even when no overload applies.
- `implicit_root_members` is accepted only beside `implicit_call_falls_through`.

A call that two candidates admit stays unbound (`overload-ambiguous`), since logos
reads no argument types. A call that nothing admits is counted
`no-applicable-overload` in `status`'s `call_residue`.

### Methods through `impl` blocks (`impl_block_lookup`)

```toml
# Rust: a call to a type's method binds through one associated-item lookup.
impl_block_lookup = true
```

A language that declares it decides `Self::m()`, `self.m()`, a method call on a
proven receiver and a written `T::m()` the same way: among the functions of
every `impl` block whose self type resolves to `T`, from any crate and through
`pub use` re-exports, plus the default bodies of the traits `T` implements that
its impl does not override (an empty `impl Tr for T {}` included). A method call
never reaches an associated function, an inherent function beats a trait's, and
a trait's function counts only where the trait is in scope. A method call that
finds nothing on `T` retries on `T`'s `Deref` target, each type once and at most
8 hops; a path call never does. `<T as Tr>::m()` reads `T`'s impl of `Tr` (or
`Tr`'s default), and `self.m()` in a trait's default body or a written
`Tr::m(x)` reaches every impl of the method plus the default. Every unbound
call of the language then carries a reason in `status`'s `call_residue`, so its
`unclassified` reads `0` on a fresh index ([Commands](commands.md)). The
language's symbols query must record its impl blocks (the `@item.impl`
captures, `@item.impl.target` for a `Deref` target). It defaults to `false`: a written `T::m()`
binds among the functions of `T`'s module, as before. Only Rust declares it.

### Outbound HTTP client calls (`invocations`)

`invocations.scm` is the **consumer** side of cross-service coupling: it captures
outbound HTTP client call sites so they can bind another workspace member's route
(the provider side, `frameworks.scm`). Ten plugins ship it — Rust, Java, Kotlin,
TypeScript, TSX, Go, Python, C#, Ruby and PHP. C, C++ and Scala ship no
`frameworks` capability and therefore no client side either; `logos languages`
reports `invocations` **absent** for them rather than empty-but-present, so
"this language has no client capture" is distinguishable from "it captured
nothing".

Two descriptor fields in `plugin.toml` drive the arm:

```toml
capabilities = ["symbols", "references", "frameworks", "invocations"]

# The ledger gate: a file is scanned for client calls only if it references
# one of these. Without it, a broad `<receiver>.<method>("/x")` anchor would
# turn any incidental "/"-shaped collection key into a fabricated edge.
http_client_detectors = ["org::springframework::web::client", "java::net::http"]

# Optional: normalizes a non-canonical verb spelling to an HTTP method.
[invocation_methods]
getasync = "GET"
postasync = "POST"
```

Declaring the capability **requires** at least one `http_client_detectors` entry —
a descriptor declaring `invocations` with an empty detector list, or with a query
file that will not load, fails validation at startup rather than degrading to
silent no-capture.

**What is deliberately not captured.** A path composed at runtime — `${api.base}/users`,
a template literal, an f-string, a bare variable — emits **no** reference and is
reported as `base-url-runtime`; a static path that will not normalize is reported
as `path-not-composed`. Logos never guesses the composed value, so on a codebase
whose call sites all build their URLs from configuration you should expect few or
no captures, and that is the honest answer rather than a defect. Route
*registrations* (`app.get("/x", handler)`, `Route::get(...)`, a FastAPI decorator)
are excluded structurally in every language — capturing one would bind another
member's real route and invent a cross-service edge.

**A refused call site is recorded, not dropped.** A `base-url-runtime` site leaves
one keyless row per declining declaration, so `workspace status` counts it as an
unbound reference under that reason instead of the site disappearing, and
`logos status`'s unresolved-reference count includes it too. That is why enabling
this capability on such a codebase makes the unbound count *rise* — the sites were
always there and were previously not counted. A recorded refusal names no target,
so it can never become a cross-service edge or a promoted node, and none of these
figures is a gate input. Two things it still cannot show you: a call your
language's query never matched (each stated capture ceiling is named in that
language's `invocations.scm` — some declare them in the header, others in a
"Stated coverage ceilings" section at the foot of the file) and the
`path-not-composed` half, which is reported only when a target was stored.
