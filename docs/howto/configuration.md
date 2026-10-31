# Configuration

Logos works with **zero configuration** — every setting has a sensible
default. When you do need control, configuration lives in two human-editable
TOML files under `.logos/`, designed to be **checked into version control**
(they are policy that travels with the repo; the derived databases are not).
Settings that decide which programs Logos runs are the exception: they live in
a [user-level config](#user-level-config--settings-no-repository-can-choose)
outside every repository.

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
| `.logos/history.db` | The evidence store (SQLite): mined git history + ingested coverage, on its own forward-only migration track. Derived — created on demand by `logos hotspots` / `logos coverage ingest` / `logos security scan` / `logos security ingest`; the gate never opens it, except to judge findings under an opt-in `[security]` table. | No |
| `.logos/telemetry.db` | Local usage/performance events feeding `logos stats`. Never leaves your machine. | No |
| `.logos/security/` | SARIF reports written by the security scanners Logos runs, and CodeQL's databases. Disposable — a rescan rewrites them. | No |
| `.logos/config.toml` | Indexing & resolution policy (optional). | Yes |
| `.logos/rules.toml` | Architecture contract: layers, boundaries, budgets (optional). | Yes |
| `.logos/hooks/` | Managed git hook scripts: freshness `post-commit`, `post-checkout`, `post-merge` plus the enforcing `pre-push` gate. Installed by `logos init --hooks`. | Yes |
| `.logos/plugins/` | On-disk language-query overrides (optional, advanced). | Yes |
| `.logos/review-cache/` | The branch review's merge-base before-states, one JSON file per commit, newest 16 kept. Derived — `logos review` rebuilds it; the generated `.logos/.gitignore` ignores it. | No |
| `.logos/stop-hook/` | The [plugin's](plugin.md#the-stop-check) Stop check's records: the fingerprint of the last measured turn, the before-state markers and a build lock. Machine-local; the directory ignores itself. | No |

Recommended `.gitignore` entries:

```gitignore
.logos/*.db
.logos/*.db-*
```

`logos init` writes the equivalent entries into `.logos/.gitignore` for you; they
are listed here for a hand-rolled root `.gitignore`.

## Workspace federation files — the manifest and the warm sidecar

A **workspace** is a parent directory of sibling repositories, federated by
[`logos init --workspace`](commands.md#init--i---hooks---workspace---yes---exclude-glob).
Its configuration lives at the *parent*, one level above the per-member `.logos/`
directories documented above — a member keeps its own `.logos/` and its own
`config.toml`/`rules.toml`, unchanged.

| Path (at the workspace root) | What it is | Check in? |
|---|---|---|
| `logos.workspace.toml` | The manifest: approved members plus your hand-written `default` / `autodiscover` / `exclude` / `[workspace.warm]` / `[workspace.member.<name>]` / `[[links]]` / `[governance]`. Re-running `init --workspace` preserves their keys and values; it re-serialises the file, so comments and formatting are not kept. Editable in the app — see [Editing the manifest from the app](#editing-the-manifest-from-the-app). | Yes |
| `.logos.workspace.warm.json` | Machine-written record of the last background warm's per-member outcome. Derived, host-local, safe to delete. | No |
| `.mcp.json` | Gains a single `logos-workspace` server key, deliberately distinct from a member's own `logos` key so neither shadows the other. | Yes |
| `.gitignore` | A managed block ignoring the warm sidecar and the other host-local files under the root's `.logos/`, maintained **only when the root is a git working tree** — a parent-of-repos root that is not a repository gains no file (`init --workspace` then reports `root_ignore: skipped`). | Yes |

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

### `[workspace] exclude` — keeping a directory out for good

A fork of a service re-exposes the routes of the service it was forked from. The
cross-service bridge binds a reference only when **exactly one** member provides
it, so the fork turns every shared route into an ambiguous reference, and the
coupling disappears from the answers without anything visibly failing. Leaving
the fork out of the workspace once is not enough if the next `init --workspace`
or an `autodiscover` brings it back; `exclude` makes the decision last.

```toml
[workspace]
name = "shop"
exclude = ["*-fork", "legacy-billing"]

[workspace.autodiscover]
```

- **The key.** A list of globs, matched with the engine `init --workspace
  --exclude` always used, against the **member name**: the directory's path
  relative to the workspace root (`svc-fork`, `services/svc-fork`). `*` matches
  across `/`. A glob that does not compile, or that tries to leave the workspace
  (`..`, an absolute path), is **rejected at parse time** (exit 2), and the error
  names `workspace.exclude` and the glob. The key is optional; a manifest without
  it behaves exactly as before and `init --workspace` never writes it.
- **What it filters.** The directories `autodiscover` would add, and the
  candidates `init --workspace` proposes. An excluded directory is not a member,
  so it is absent from the cross-service fan-out, the bridge, the coverage
  figures and every other per-member answer.
- **An explicit `members` entry wins.** An entry in `members` that matches a glob
  is **kept**: an exclusion that overrode a line you wrote would be a silent
  contradiction. Every workspace load logs a warning that names the entry and the
  glob, and `workspace status` carries the same under `exclusion.conflicts`. To
  exclude such a member, remove it from `members`.
- **`init --workspace --exclude <glob>` persists.** Each `--exclude` glob is added
  to `exclude` (sorted, de-duplicated) and none is ever removed, so the next
  `init --workspace --yes`, with or without the flag, neither proposes nor adds
  the directory. Edit the file by hand to drop a glob. A run that would leave
  the workspace with no member at all writes nothing, the flag included.
  A member that is only autodiscovered, and that a new `--exclude` glob
  matches, is left out of the written `members` rather than pinned there; an
  explicit `members` entry that matches stays, with the warning above.
- **It is visible.** `workspace status`, in both forms, gains an `exclusion`
  block: the declared `globs`, the `excluded` directories (the autodiscovered ones the
  globs kept out; empty when `autodiscover` is off, since nothing is
  autodiscovered then), and the `conflicts` above. A glob broad enough to
  swallow a real service shows up there rather than as a member that is quietly
  missing. The block is
  absent when the manifest declares no `exclude`.

### `[[links]]` — declaring a coupling the bridge cannot see

The bridge binds what it can read: a literal URL, a literal topic. A call whose
URL is composed at runtime binds nothing, and a human who knows the coupling can
declare it:

```toml
[[links]]
relation = "http_call"          # or "grpc_call", "publishes"; anything else is exit 2
from = "web::fetch_cart"         # <member>::<canonical symbol or name>
to = "api::get_cart"
```

Each endpoint must resolve to **exactly one** node in its own member, and the
two members must differ. A link that does is one bridge edge labelled
`asserted`; one that does not is reported with its reason (`unknown-member`,
`member-degraded`, `no-match`, `ambiguous`, `same-member`), and one that repeats
an edge already held — the same two endpoints, a route and the handler it serves
counting as one — is reported `redundant`. `workspace status` lists every
link under [`links`](commands.md#links--what-each-declared-links-entry-became).
An asserted edge is a claim, never evidence: it moves no coverage figure and no
governance check. It does show up, labelled `asserted`, in `xservice callers` /
`impact`, in `workspace reachability` (as `live-via-asserted-link`) and dashed on
the web service map. A member `[workspace] exclude` removed is not a member, so a
link naming it is `unknown-member`.

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
# logos:managed:begin — regenerated by `logos init --workspace`; edit outside this block
/.logos.workspace.warm.json
…
# logos:managed:end
```

A short header comment above the markers says why the entries are there. The first
entry is the sidecar; the entries after it (the `…`) keep the other host-local files
under the root's `.logos/` out of git. A root enabled by an older build gains any
newer entry on the next `logos init --workspace` run; the header comment is written
only when the file is first created, so an existing one keeps its old wording.

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

**Tables an extension owns.** Some top-level tables of `config.toml` are
reserved for optional extensions, and a reserved table is never an error. Logos
ignores one that no active extension claims and the Config editor keeps it in the
file when it saves. Where the core does not read the table itself, Logos prints one
notice on stderr naming it (`notice: … declares [<table>], which no active
extension claims: ignored, and kept in the file`).

Every other unknown table or key still fails with the same error as before.

**Typed fields for an extension's table.** An active extension can give the web
Config views typed fields for the table it owns: groups of fields, each group with an
optional heading and an optional read-only note, each field an integer, a number, a
true/false choice, a list (one entry per line), a string, or a pick from a fixed set
of values, shown with its label, help, placeholder and the default that applies
when the key is unset. The groups render after the built-in fields:

- in a member's **Config** view, in the `config.toml` form — saved by its **Save**,
  together with everything else in that form;
- in the workspace **Config** view, in a *Workspace settings* group over the
  workspace root's `.logos/config.toml` — saved against the fingerprint its read
  returned (`GET /api/v1/workspace/config`, `POST /api/v1/workspace/config/save`),
  so a file changed on disk since the load is refused with `409` and shown, as the
  manifest's save does. The group appears only while an extension contributes such
  fields.

A field is a view of the one document: it reads its value from the text in the raw
pane and writes its one key back into that text, so the raw pane is always what
**Save** writes and no extension writes the file itself. Clearing a field removes its
key. A value the field cannot show without changing it — a hexadecimal integer, a
multi-line or padded string, an inline table, an empty or mixed list — is shown as
written, read-only, and left untouched; edit it in the raw pane. A pick-from-a-set
value outside its set is shown as written and stays as written until you choose
another. Logos validates the saved document as it always does, so a value the
extension rejects is refused with `422` and nothing is written. A build that composes
no extension shows none of these fields, and no *Workspace settings* group.

```toml
# Optional code-language admission allowlist (grammar names from `logos languages`).
# Omit it (or use []) to index every compiled-in code language out of the box —
# the fourteen below are the default set. List a subset to restrict indexing; dropping
# a grammar from a broader list purges its now-unadmitted nodes on the next reconcile.
# JavaScript (`.js`, `.mjs`, `.cjs`, `.jsx`) is its own grammar, `javascript`: a list
# naming `typescript` or `tsx` admits no JavaScript file unless it names `javascript` too.
languages = ["rust", "python", "typescript", "tsx", "javascript", "go", "java", "c", "cpp", "c-sharp", "kotlin", "scala", "ruby", "php"]

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
# "lcov", "cobertura", "jacoco", or "gocover". The manual `coverage ingest --format` flag is unaffected.
format = "auto"
# Optional command `logos coverage refresh` runs (via `sh -c`, cwd = project root)
# to regenerate the artifact before ingesting it; `coverage run` runs it last, as the
# `custom` entry. It runs only on an explicit `coverage refresh` or `coverage run`,
# NEVER on the serve/watcher path (ADR-38/NFR-SE-01), and only under the project's
# coverage execution consent, which pins this exact text: the first run, and the first
# after the text changes, needs `--consent`. Omit it and `coverage refresh` errors
# loudly rather than guessing a command.
refresh_cmd = "cargo llvm-cov --lcov --output-path target/coverage/lcov.info"
# Which registry tools may run: a repository can only switch one off. What a tool runs
# as (binary path, extra arguments, timeout) is set in the user-level config.
tools = { gradle = false, jest = false }
```

The `refresh_cmd` runs through a shell and is committed with the repository, so
a run asks for your consent before it starts it and again whenever its text
changes: [coverage.md](coverage.md#the-execution-consent) says what that consent
covers and what it does not. The seven built-in coverage tools, the user-level
overrides and the whole of `coverage run` are in [coverage.md](coverage.md);
[Coverage tools](#coverage-tools--which-may-run-and-as-what) below has the
configuration reference.

### Minified JavaScript and vendored copies

`**/*.min.js` is in the default code `exclude`, at the root and at every nested
depth: a minified file is never meaningfully navigable, and on a workspace with
vendored front-end libraries it can carry a large share of the JavaScript-language
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
  and the `serve` watcher admit the same files. Up to 1.15.2 the watcher and
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
**reconciles on config change**: the next `logos index` (or the first navigation
call after the change) detects the narrowed admission
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
receiver's type (Java, [FR-RS-10](../specs/requirements/FR-RS-10.md); TypeScript,
[FR-RS-21](../specs/requirements/FR-RS-21.md); Python,
[FR-RS-24](../specs/requirements/FR-RS-24.md); Rust,
[FR-RS-42](../specs/requirements/FR-RS-42.md)), which no policy widens either.

Changing the policy needs no migration — resolution re-evaluates the whole
unresolved-reference ledger on every run, so just `logos index` again. The same
holds for `[resolution.import_roots]`.

### `[project] id` — the repository's identity

`[project] id` names the repository: one **coordinate** that stays the same in
the main checkout, every linked worktree, every clone and CI
([FR-CF-10](../specs/requirements/FR-CF-10.md)).

```toml
[project]
id = "github.com/acme/payments"
```

`logos init` writes the key when it is absent, set to the derived default below,
and never changes it afterwards. A re-run leaves the file byte-identical. A
`config.toml` that already exists keeps every byte you wrote: the table is
appended at the end, or the key goes on the line after an existing, empty
`[project]` header. Edit the value to pin a different coordinate.

**Without the key**, the same coordinate is derived each time it is needed:

1. The `origin` remote, normalised. The scheme, credentials, port, query,
   fragment and `.git` suffix are dropped and the host is lowercased, so a token
   in a remote URL is never written. `git@h:o/r.git`,
   `https://h/o/r`, `https://user@h/o/r.git` and `ssh://git@h/o/r` all give
   `h/o/r`. A local remote (a path, or `file://`) that exists is followed: a
   clone made from a checkout, a cache or a mirror resolves what that repository
   resolves, its own remote included. A local remote that no longer exists gives
   its last component, without `.git`.
2. With no `origin`, the directory name of the repository's **main** worktree,
   found through git's common directory. A linked worktree therefore resolves
   like its main checkout, never by its own directory name. A bare repository's
   worktree takes the bare repository's name, without `.git`.
3. Outside git, the project directory's own name.

A derived value never fails validation: a character outside the allowed set is
replaced with `-`.

**Validation.** The value must be non-empty and use only letters, digits and
`. _ / @ : -`, with no spaces, and must not be a bare `.`, which in a symbol
marks an empty field. Any other value fails the load with exit 2 naming
`project.id`:

```text
invalid value for project.id: "my repo" is not a project coordinate: it must be non-empty, not `.`, and use only letters, digits and `._/@:-` (no spaces)
```

`id` is the table's only key. Any other key under `[project]` is an unknown
field, rejected with exit 2 like an unknown key in any other table.

**When `.logos/` is git-ignored** (many repositories ignore it whole), the id
`init` writes stays on your machine. `init` says so on its `[project] id` row,
and names the default other checkouts and CI derive instead. To make the id
travel, commit `.logos/config.toml`.

The coordinate in force is recorded in the store by `logos init`, by every
full `logos index` and by the migration described below, and `logos status`
reports it as `project_id`. Changing the key does not re-admit or purge any
file.

**Every symbol carries it.** A node's symbol names its repository in the SCIP
package slot, so the same declaration in two repositories has two symbols:

```text
logos . github.com/acme/payments . src/`lib.rs`/run().
```

Broker topics (`logos . github.com/acme/payments . topic/orders#`) and the
policy nodes `rules.toml` declares (`logos policy github.com/acme/payments . layer/domain/`)
carry it too. A full `logos index` builds every symbol under the coordinate in
force. An incremental sync keeps the coordinate the store's symbols already
carry, so one store never mixes two. A sync that writes the first file into a
store, such as a commit hook running before any index, takes the coordinate in
force, as an index would.

**Opening a store moves it to the coordinate in force.** A store built by an
earlier release carries no coordinate. After a change of `[project] id` (or,
without one, of the `origin` remote) a store's symbols still carry the old one.
The first command that opens such a store migrates it in place, before anything
syncs into it:

- every symbol takes the coordinate;
- symbol rows that no node references any more are removed;
- every file is marked for re-reading, so the next reconcile (`logos scan`,
  `logos check`) re-extracts it.

Node and edge counts, snapshot history and the gate baseline are unchanged. The
migration is one transaction: if it is interrupted, the store stays as it was
and the next open runs it again. A store already at the coordinate opens without
a write. A linked worktree seeded from main's store migrates its copy before its
first sync. While `.logos/config.toml` does not load, the store is left as it is.
Without an `id`, a store that already records a coordinate also keeps it while
the derived value is only a fallback — git cannot answer for the repository (git
missing from `PATH`, or refusing the repository's ownership), so only the
directory name is left, or the local checkout `origin` names is away, so only its
last path component is left. Following either would move the store back and
forth as the condition comes and goes. A relative local `origin` is read from
the main checkout, so a linked worktree derives what its main checkout derives.

### `[security_scan]` — which scanners may run

Logos runs the security scanners that are installed, and `logos security install`
can install all of them under your consent, Dependency-Check and CodeQL only
when named ([security.md](security.md#installing-scanners)). Nine are registered:
`semgrep`, `trivy`, `gitleaks`, `osv-scanner`, `bandit`, `gosec`, `checkov`,
`codeql` and `dependency-check`. Each is enabled by default and runs only if it
is installed. The repository can switch a scanner off:

```toml
[security_scan.codeql]
enabled = false

[security_scan.dependency-check]
enabled = false
```

**`enabled` is the only key.** This file travels with every clone, so it must
not be able to choose what Logos runs. A `path`, `extra_args`, `command` or any
other key under `[security_scan.<scanner>]`, a bare key directly under
`[security_scan]`, or a scanner name that is not registered fails the load with
exit 2, before any scanner is started. The error names the file, the line and
the key; for a `path` key it ends:

```text
unknown field `path`, expected `enabled`
```

The binary path and extra flags are set in the [user-level config](#user-level-config--settings-no-repository-can-choose)
instead.

### `.logos/security-ignore.toml` — suppressions

Accepted security risk is recorded in a committed file. Each entry names one
finding, every finding under a path, or one rule of one scanner in every file,
with a reason and an optional expiry:

```toml
[[ignore]]
fingerprint = "<the finding's 64-character fingerprint>"
reason = "test fixture, never deployed"
expires = 2026-12-31          # optional; suppresses through this day inclusive
```

- **The fingerprint** is the one `logos security findings --json` shows for the
  finding. It is derived from the rule, the file and the scanner's identity for
  the result, so a suppression never leaks onto the same line in another file.
  Renaming a file gives its findings new fingerprints.
- **`reason` is required.** An entry without one, a value that is not a
  fingerprint, a date that is not a calendar date, or a repeated fingerprint
  rejects the whole file with exit 2, naming the line. A suppression that
  silently did nothing would be worse than none.
- **A path instead of a fingerprint.** `path = "tests/fixtures"` suppresses
  every finding at or under a path the glob matches, relative to the project
  root. `*` stays within a segment, `**` crosses segments, and a matched
  directory covers what is under it. `rule = "<rule id>"` and
  `scanner = "<scanner>"` narrow the entry; the scanner is matched in either
  case. A glob covering the whole project (`*`, `**`, `.`, or one matching
  every name at the root, such as `?*`), one escaping the
  root, `fingerprint` and `path` in the same entry, or `rule`/`scanner` beside a
  fingerprint rejects the file with exit 2, naming the line. When several
  entries name a finding, the first unexpired one labels it, fingerprint
  entries first. See
  [security.md](security.md#one-entry-for-a-whole-path).
- **A rule in every file.** `rule = "<rule id>"` and `scanner = "<scanner>"` with
  no `path` and no `fingerprint` suppress that rule's findings for that scanner
  wherever they are, listed as suppressed by a rule-wide entry with the reason.
  `expires` works as for any entry, and the gate leaves the findings out as it
  does any suppression. A `rule` without a `scanner`, or a `scanner` without a
  `rule`, exits 2 naming the missing key. See
  [security.md](security.md#one-entry-for-a-rule-in-every-file).
- **Expiry.** From the day after `expires` the entry no longer suppresses: the
  finding comes back, and the entry is listed as `expired`.
- **Counts.** `security status` and `security findings` list every entry under
  `suppressions`, with `suppressed`, the number of findings it suppresses now.
- **Orphans.** An entry that matches no stored finding is listed as `orphaned`
  (the finding was fixed, its scanner has not run here, or its glob matches
  nothing). An entry
  naming a fingerprint a finding had before it was re-keyed (a Gitleaks finding's
  earlier, per-commit one, an OSV-Scanner one from before history.db
  migration 10, or a dependency finding's from before migration 13) still
  suppresses it and carries `should_name`: the fingerprint
  that finding has now. The file is never rewritten.
- **Visible, never silent.** A suppressed finding is still returned by
  `security findings`, with its reason, under its code scope (one in test code
  with `--scope test` or `--scope all`, else counted in `hidden_by_scope`);
  `--suppression active` hides it.
- **Written from the Security page.** Its Suppress… and Unsuppress write the
  file in one canonical spelling: one `[[ignore]]` table per entry, the
  fingerprint entries sorted by fingerprint, then the path entries sorted by
  path, rule and scanner, then the rule-wide entries sorted by rule and scanner,
  a path without a leading `./` or a trailing `/`, and
  no comments. A file spelled any other way (the comment in the
  example above is enough) is refused with an error that says so, rather than
  rewritten without your comments. Edit such a file by hand.

The file is optional (absent means nothing is suppressed), travels with the
repository (the managed `.logos/.gitignore` does not ignore it), and is read on
every `security findings` and `security status`. See
[commands.md](commands.md#security-findings).

## User-level config — settings no repository can choose

Settings that decide what Logos **runs** live in one file per user, outside
every repository, so a cloned repository can never set them.

**Where it is.** The first of these that is set to an absolute path wins:

| Variable | Directory |
|---|---|
| `LOGOS_CONFIG_HOME` | `$LOGOS_CONFIG_HOME` |
| `XDG_CONFIG_HOME` | `$XDG_CONFIG_HOME/logos` |
| `HOME` | `$HOME/.config/logos` (on macOS too) |
| `APPDATA` (Windows) | `%APPDATA%\logos` |

An unset, empty or relative value is skipped. The file is `config.toml` in that
directory. It is optional: a missing directory or file means no overrides.
`LOGOS_CONFIG_HOME` exists so tests and sandboxes can point Logos at a
throwaway directory. The record of which scanners you allowed to reach the
network is kept in the same directory (see [Scan consent](#scan-consent) below).

A file that exists is parsed as strictly as `.logos/config.toml`: an unknown
table or key fails with exit 2.

**`[security_scan.<scanner>]`** — overrides for one registered scanner:

```toml
[security_scan.semgrep]
# Run this binary instead of the first `semgrep` on PATH. Must be absolute.
path = "/opt/semgrep/bin/semgrep"
# Extra flags, one argument per entry (no shell splitting). They are inserted
# at a fixed place in the scanner's command line, before its path arguments.
extra_args = ["--exclude", "vendor"]

[security_scan.codeql]
# For CodeQL the extra flags go to `database analyze`, never `database create`.
extra_args = ["--threads=4"]
```

`path` must be absolute; a path that does not exist reports the scanner as not
installed, without falling back to `PATH`. An empty `extra_args` entry, a
scanner name that is not registered, and an `enabled` key (switching scanners is
the repository's call) all fail with exit 2.

**`NVD_API_KEY` — Dependency-Check's NVD API key.** Not a config key: an
environment variable, read from the process that runs the scan — your shell for
`logos security scan`, the environment `logos serve` was started with for MCP and
the web (export it before starting `serve`). No repository file, `.env` or config
key can supply it, and an empty value sets none. Logos hands it to
Dependency-Check in a properties file of its own,
`.logos/security/nvd-api-key.properties` (mode 0600, passed as `--propertyfile`,
deleted when the run ends), so the key is never on the command line. The
alternative is your own `extra_args = ["--nvdApiKey", "<key>"]` (or
`--propertyfile`/`-P` with a file of yours) under
`[security_scan.dependency-check]` here; when they carry one, Logos adds no file.
`security status` says `nvd_api_key: set` or `not_set` on the Dependency-Check
row, never the key ([security.md](security.md#dependency-checks-nvd-api-key)).

**How a scanner is found and run.** Without a `path` override, Logos takes the
first match on `PATH`, using only the entries that are absolute and outside the
project: an empty or relative entry (`.`, `bin`) and an absolute one inside the
project (an activated in-repo virtualenv, `node_modules/.bin`) are skipped. The
scanner runs with that same reduced `PATH`, so a tool it starts by name (`git`,
`go`, `java`) is never one the repository holds. When neither finds the scanner,
Logos uses the copy it manages itself, if the version this release pins is
installed in the user-level tools directory (`$LOGOS_DATA_HOME`, else
`$XDG_DATA_HOME/logos/tools`, else `~/.local/share/logos/tools`); a tools
directory inside the project is ignored. A scanner is started directly
with its argument list, never through a shell, with the project root as its
working directory, and its stdout and stderr are captured up to a bound (64 KiB
and 16 KiB): past it, each stream keeps its beginning and its end, with one line naming the
bytes left out between them. On Linux and macOS it runs in a process group of its own, and when
it exits the group is killed, so nothing it started outlives the run. Each
command line is fixed in Logos:

| Scanner | Binary | Version probe | Command line (`OUT` is under `.logos/security/`) |
|---|---|---|---|
| Semgrep | `semgrep` | `--version` | `scan --config p/default --sarif --output OUT --metrics off --disable-version-check --quiet --exclude .logos` *extra* *unignored* |
| Trivy | `trivy` | `--version` | `fs --scanners vuln,secret,misconfig --format sarif --output OUT --skip-dirs .logos` *ignored* `--config .logos/security/empty-config.yaml` *extra* `.` |
| Gitleaks | `gitleaks` | `version` | `detect --redact --no-banner --exit-code 0 --report-format sarif --report-path OUT` *extra* `--source .` |
| OSV-Scanner | `osv-scanner` | `--version` | `--format sarif --output-file OUT` *extra* `--recursive` *unignored* |
| Bandit | `bandit` | `--version` | `--recursive --format sarif --output OUT --exit-zero --quiet --exclude ./.logos`(`,`*ignored*) *extra* `.` |
| gosec | `gosec` | `-version` | `-fmt sarif -out OUT -no-fail -quiet` *ignored* *extra* `./...` (with `GOTOOLCHAIN=local`) |
| Checkov | `checkov` | `--version` | `--output sarif --output-file-path OUTDIR --soft-fail --quiet` [`--skip-framework secrets`, with Gitleaks enabled and installed] `--skip-path .logos` *ignored* *extra* `--directory .` |
| CodeQL | `codeql` | `version --format=terse` | per language: `database create DB --language=L --build-mode=none --source-root=. --overwrite`, then `database analyze DB --format=sarif-latest --output OUT` *extra*, with `OUT` in `.logos/security/codeql/`; Logos then merges every language's runs into one `codeql.sarif`, so a project's languages are one snapshot |
| Dependency-Check | `dependency-check` (or `dependency-check.sh`) | `--version` | `--scan . --format SARIF --out OUTDIR --exclude **/.logos/**` *ignored* `--disableYarnAudit --disablePnpmAudit --disableBundleAudit --disableGolangMod --disableMixAudit` [`--propertyfile .logos/security/nvd-api-key.properties`, with `NVD_API_KEY` set and no key in *extra*] *extra* |

*ignored* is the top-most directories the project's own ignore files exclude
(`.gitignore` and `.ignore` at any depth, and `.git/info/exclude`), each passed
the way that scanner's flag takes it: `--skip-dirs target` for Trivy, appended
to Bandit's one comma list (`./.logos,./target`), `--skip-path
'^(\./|/abs/root/)?target(/|$)'` for Checkov (anchored at the project root,
whether Checkov walks it as `.` or as its absolute path), `--exclude
target/**` for Dependency-Check and `-exclude-dir '^target(/|$)'` for gosec (the
directory and every directory below it, relative to the project root), up to
64 KiB of arguments, shallowest first. A Bandit, Checkov or gosec run on named
paths passes none, so it reads exactly the paths you named; Trivy and
Dependency-Check, which a re-scan of paths runs over the whole project, still
pass them.

*unignored* is `.` when the project ignores nothing, and otherwise every path
outside those directories, with a directory that holds one named by its entries
(`.gitignore app go.mod src targets` beside an ignored `target/`). It is what
keeps Semgrep and OSV-Scanner out of ignored directories in a git checkout, a
linked worktree and a copy that is not a git repository alike. A Semgrep run on
named paths takes those paths instead; OSV-Scanner, which a re-scan of paths
runs over the whole project, still takes the list. A symbolic link is never
named. Past 64 KiB of arguments, when a path starts with `-` or is not UTF-8, or
when nothing lies outside the ignored directories, it is `.` again, and the scan
says so. Gitleaks
reads committed history, and CodeQL gets no exclusion.
[Security](security.md#running-scans-and-reading-the-results) lists the edges,
and how each scan states what it did with ignored directories.

The commands were written against Semgrep 1.85, Trivy 0.74, Gitleaks 8.18,
OSV-Scanner 1.9, Bandit 1.7.10, gosec 2.21, Checkov 3.2, CodeQL 2.19 and
Dependency-Check 10.0; the Dependency-Check Logos installs, 13.0.0, still takes
every flag above; OSV-Scanner's output flag is the 2.x spelling, so a 1.9.x copy
fails until it is replaced. A release whose flags have since changed fails, and the
scanner's own error is reported. Logos treats a Dependency-Check on `PATH` below
13.0.0 as below its minimum, as it does any scanner below the version it pins
as tested: older releases broke on changes to the NVD data they read. Detecting a scanner runs only its version
probe, with the update-check opt-outs Semgrep (`SEMGREP_ENABLE_VERSION_CHECK=0`)
and Trivy (`TRIVY_SKIP_VERSION_CHECK=true`) honour set on it and on every scan.

**A repository cannot make a scanner run its code.** Some scanners read a
config file from the directory they scan, and some of those files can name code
for the scanner to run. Logos closes the ones it knows:

- **Checkov** loads `.checkov.yaml` / `.checkov.yml` from the scanned directory,
  and that file can name external checks, which are Python Checkov runs. When
  either file is at the project root, Checkov is not started; it is reported
  failed with `not run: the repository's .checkov.yaml can make checkov run code
  it names; remove it, or run checkov in CI and ingest its SARIF`.
- **Trivy** would read `trivy.yaml` from its working directory, which can point
  its database and cache at other hosts. Logos passes `--config` with an empty
  file it writes itself (`.logos/security/empty-config.yaml`).
- **Dependency-Check**'s Yarn, pnpm, bundle-audit, Go-module and mix_audit
  analyzers start the project's package manager or toolchain (Yarn runs a
  committed `yarnPath`), so they are switched off.
- **gosec** runs with `GOTOOLCHAIN=local`, so a `go.mod` cannot make Go fetch and
  run another toolchain.

Config files that only tune findings (`.gitleaks.toml`, `.semgrepignore`,
`.trivyignore`, `.bandit`) are still read. Scan output goes only to
`.logos/security/`: if `.logos` or `.logos/security` is a symbolic link the scan
is refused, and a link in place of any report, output directory or CodeQL
database below it is removed, never written or deleted through.
`.logos/security/` is in the `.logos/.gitignore` that `logos init` maintains: a
report can quote a secret, so it is never meant to be committed.

**CodeQL never builds your project.** It runs only with `--build-mode=none`, and
only on JavaScript/TypeScript, Python and Ruby, whose extraction reads source and
nothing else. Every other language is reported `requires a build — ingest a CI
report` and nothing is started for it: Go, Kotlin, Swift and C/C++ extract only
from a build, and the build-free mode of Java, C# and Rust still evaluates the
project to resolve its dependencies (through Maven or Gradle, MSBuild, or Cargo
and its `.cargo/config.toml`). Run CodeQL for those in CI and ingest its SARIF. A
project with none of these languages is reported not applicable for CodeQL.

### Scan consent

A scanner may reach the network to refresh its rules or its vulnerability
database (Semgrep's registry rules, Trivy's and OSV-Scanner's databases,
Dependency-Check's NVD feed). So the first `logos security scan` in a project
refuses with exit 2 until you consent, naming the scanners it would start and
the flag that records your consent:

```text
error: invalid value for consent: none is recorded in this project for semgrep, trivy — a scanner may
reach the network to refresh its rules or vulnerability database. Run `logos security scan --consent`
in a terminal to record it (an MCP client cannot); later scans will not ask again
```

`--consent` records every installed, enabled scanner the record does not name
yet, then scans. Later scans do not ask. A scanner you install or enable later
is not in the record, so the next scan asks again, for that scanner alone.
Nothing starts before the check, not even a version probe. Only the command line
records consent: the MCP `security_scan` tool refuses a `consent` argument and,
until you have consented, fails with the message above.

The record is `security-consent.toml` in the user-level directory, never in the
repository. Logos writes it; it is keyed by the project's canonical root path,
so each checkout (and each worktree) has its own consent, and no repository can
claim another's. To withdraw consent, delete the project's table from the file,
or the file. `logos security status` reports the consent state: the scanners
consented to, and the ones the next scan will ask for.

### Coverage tools — which may run, and as what

Logos has a built-in registry of seven coverage tools. Each is selected by a
marker file at the project root and has a fixed command line:

| Tool | Selected by | Found at | Report |
|---|---|---|---|
| `cargo-llvm-cov` | `Cargo.toml` | `PATH` at 0.6.0 or later, else the copy `logos coverage install` manages; a `PATH` copy below 0.6.0 runs only while there is no managed copy ([Installation](installation.md#coverage-runs-on-a-rust-project)) | Cobertura, `.logos/coverage/rust.xml` |
| `vitest` | `package.json` declaring `vitest` | `node_modules/.bin` only, with a coverage provider (`@vitest/coverage-v8` or `@vitest/coverage-istanbul`) | Cobertura, `.logos/coverage/vitest/` |
| `jest` | `package.json` declaring `jest` | `node_modules/.bin` only | Cobertura, `.logos/coverage/jest/` |
| `pytest-cov` | `pyproject.toml`, `setup.cfg` or `pytest.ini` | the project's `.venv`, else `PATH`; pytest-cov must be installed beside it | Cobertura, `.logos/coverage/python.xml` |
| `maven` | `pom.xml` | `./mvnw`, else `mvn` on `PATH` | JaCoCo XML, each module's `target/site/jacoco/jacoco.xml` |
| `gradle` | `build.gradle` or `build.gradle.kts` | `./gradlew`, else `gradle` on `PATH` | JaCoCo XML, each project's `build/reports/jacoco/test/jacocoTestReport.xml` |
| `go` | `go.mod` | `PATH` | Go coverprofile, `.logos/coverage/go.out` |

Each tool's full command line and tested version are in
[coverage.md](coverage.md#the-registry-seven-tools).

Maven gets the JaCoCo plugin (0.8.13) on its command line. Gradle gets it from an
init script Logos writes to `.logos/coverage/jacoco-init.gradle` and passes with
`--init-script`, and it runs with `--no-daemon`. Neither build file is edited.
A `package.json` declares a tool only in its `dependencies` or `devDependencies`.

**The repository only switches tools on or off.** In `.logos/config.toml`:

```toml
[coverage_ingest]
tools = { gradle = false, jest = false }
```

Every tool is enabled unless it is switched off here. A value other than `true`
or `false`, such as a path, a command or a list of flags, fails the load with
exit 2. So does any other key, or a tool name that is not registered. Nothing
has started by then.

**What a tool runs as is set at user level.** In the [user-level
config](#user-level-config--settings-no-repository-can-choose):

```toml
[coverage_ingest.tools.maven]
# Run this binary instead of the one the registry finds. Must be absolute.
path = "/opt/maven/bin/mvn"
# Extra arguments, one argument per entry (no shell splitting), inserted at a
# fixed place in the tool's command line.
extra_args = ["-Pcoverage"]
# Seconds before the tool's whole process group is killed. Default: 1800.
timeout_secs = 3600
```

A `path` override is the only copy used while it is set. If it is not an
executable file the tool reports *not installed*. A relative `path`, an empty
`extra_args` entry, a `timeout_secs` of 0 and a name that is not registered all
fail with exit 2.

**What the user-level directory also keeps for coverage.** Beside `security-consent.toml`, in
the same directory, these records are written by Logos and never by a repository (the download
consent for the managed `cargo-llvm-cov` is in `security-install-consent.toml`, beside the scanners'):

| File | Records | Keyed by |
|---|---|---|
| `coverage-consent.toml` | The execution consent: the coverage tools you allowed to build the project and run its tests, and the SHA-256 of the `refresh_cmd` text you saw | The project's canonical root path |
| `coverage-dependency-consent.toml` | The exact package-manager commands you allowed Logos to run to add a missing coverage dev dependency | The project's canonical root path |
| `toolchain-consent.toml` | The Rust toolchain component (`llvm-tools-preview`) you allowed Logos to add with your own `rustup` | Your user, for every project |

To withdraw a consent, delete the project's table from its file, or the file (for
`toolchain-consent.toml`, which has no project table, the file);
`logos coverage status` reports the execution consent. What each consent covers, and
what it does not, is in [coverage.md](coverage.md#the-execution-consent).

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
> resolve precisely. Go was measured and stays NULL: re-checked against the
> source, a sample of 30 of its "dead" functions held 30 live — reached through a
> call on a typed receiver, a function value or a build-tag variant the graph
> does not yet bind.


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

### Ratchet constraints — adopt a budget on an existing codebase

An absolute budget fails every existing violation on day one, so a codebase with
debt can never adopt it. A **ratchet** budget tolerates the debt you have
recorded and fails only on **new or worsened** debt
([FR-GV-23](../specs/requirements/FR-GV-23.md)). Every budget whose violations
name a symbol accepts a ratchet table beside its scalar:

```toml
[constraints]
max_cc            = { max = 15, ratchet = true }   # per function
max_fn_lines      = { max = 80, ratchet = true }   # per function
max_nesting_depth = { max = 4,  ratchet = true }   # per function
no_god_files      = { max = 40, ratchet = true }   # per file (keyed by path)
max_fan_in        = { max = 30, ratchet = true }   # per module (keyed by module)
max_fan_out       = { max = 30, ratchet = true }   # per module
no_god_containers = { ratchet = true }             # per class-like container
# Symbol sets: each dead / duplicated / brain-method function is one offender.
max_dead          = { max = 0, ratchet = true }
max_duplicates    = { max = 0, ratchet = true }
max_brain_methods = { max = 0, ratchet = true }
```

`max` keeps the scalar's meaning; `ratchet` defaults to `false`, which is the
scalar spelled as a table. A symbol-set budget under ratchet must say
`max = 0` — every member is named as an offender, so any other value fails at
load (exit 2).

**The blessed set.** `logos check --bless` records every current offender of a
ratchet budget in **`.logos/rules-baseline.json`** — one entry per constraint ×
symbol id, with the value it was blessed at. The file is sorted and
deterministic (re-blessing an unchanged tree is byte-identical), and it is
**checked in**: the generated `.logos/.gitignore` leaves it tracked, so CI sees
it and blessing new debt shows up in code review. Only the bless command writes
it — `check` and `scan` only read it, and `gate` never touches it. The MCP twin is the `bless_rules`
tool.

`logos check` then classifies each offender of a ratchet budget:

| Offender | Severity | Fails `check`? |
|---|---|---|
| In the blessed set, at or below its blessed value | `warning` — blessed debt | No |
| In the blessed set, above its blessed value | `error` — worsened | Yes |
| Not in the blessed set | `error` — new | Yes |
| Blessed, but no longer violating | reported under `ratchet.fixed` | No |

A fixed entry means the set can be tightened: re-run `logos check --bless` and
commit the smaller file. With **no** blessed file, every ratchet offender is an
`error` and the report names the bless command. Identity is the symbol id
**without its repository coordinate**, so the checked-in file holds in a fork,
a renamed repository or a clone with no remote; a rename or move of the symbol
itself reads as one fixed entry plus one new offender — re-bless in the same
change. The JSON report carries a `ratchet` summary (counts, the fixed
list, the bless command) and a `ratchet` mark on each classified violation; a
contract with no ratchet budget reports exactly as before.

**Budgets without symbol identity** — `max_cycles` and `max_clone_ratio` — take
`max_dead`'s count-delta form instead, failing only when the count rises above
the blessed baseline:

```toml
[constraints]
max_cycles      = { baseline = 2, delta = 0 }      # fail at 3 cycles, pass at 2
max_clone_ratio = { baseline = 0.12, delta = 0 }   # ratio units, each in [0.0, 1.0]
```

The CI recipe for adopting ratchets is in
[ci-integration.md](ci-integration.md#adopting-budgets-on-an-existing-codebase-the-ratchet).

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
cohesion_min_classes = 5   # Cohesion: fewest scoreable classes before Cohesion enters the signal
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

`cohesion_min_classes` is the fewest **scoreable classes** the Cohesion
dimension needs before it enters the signal (default `5`). Below it Cohesion
reads `n/a` ("too few classes"), leaves the aggregate and "where to start",
and the Cohesion worst-offender list still lists every class at LCOM4 ≥ 2 — a
mean over one or two classes would let one class move the whole signal (see
[metrics.md](metrics.md#8-cohesion-lcom4--do-classes-hang-together)). `1`
scores any number of classes, as before metric-semantics version 9. It must be
a positive integer (non-positive fails at load with exit 2). Unlike the clone
and duplicate keys it is always in the hashed effective set, its default
included, so the hash changed with it once — on the same upgrade that moved
the metric semantics to version 9 — and editing it re-baselines as any other
key does.

### `[history]` / `[coverage]` — the evidence tiers

Two optional `rules.toml` tables tune the **non-gated** evidence tiers
(`logos hotspots`, `logos coverage`). Every key is optional with a documented
default, and — like `[metric_thresholds]` — the effective set is hashed into
every evidence snapshot, so changing a key is recorded with the data it
produced. Because the gate never reads history or coverage evidence, **editing
these tables can never move the quality signal**; they only shape advisory
output. (Security evidence is the one exception, and only under the opt-in
[`[security]`](#security--the-security-gate) table below.)

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

### `[security]` — the security gate

Without this table, security findings are advisory: `logos security findings`
lists them, and `check` and `gate` print exactly what they printed before,
whether findings are stored, absent or deleted. Adding the table makes them
**gate input**. `check` and `gate` then fail on new findings and on the caps
you set, exactly as they fail on a broken rule.

```toml
[security]
# Scanners that must have a fresh scan at HEAD. A registry name ("semgrep") or
# a report's driver name ("Semgrep OSS"), either case. Default: [] (none).
required_scanners = ["semgrep", "gitleaks"]
# The ratchet: an unsuppressed finding at or above this severity fails unless
# `logos gate --save` recorded it. info | low | medium | high | critical; an
# `unrated` finding is gated as medium, and "unrated" is no value here.
# Default: "high".
ratchet_min_severity = "high"
# Cap: any unsuppressed finding at or above this severity fails, recorded or
# not. Default: unset.
max_severity = "critical"
# Cap: any unsuppressed finding in these OWASP Top 10 categories fails. A bare
# id is the 2025 edition; add ":2021" for the older one. Default: [] (none).
fail_on_owasp = ["A05", "A01:2021"]
```

**The ratchet** lets you adopt the gate on a codebase that already has
findings. Run a scan, then `logos gate --save`. Along with the metric baseline,
this records the fingerprint of every unsuppressed finding of every stored
scan, whatever its severity and whether or not the scan is fresh. After that,
`check` and `gate` fail only on a finding that is new since the save. A
suppressed finding is not recorded. When its suppression expires or is
removed, the finding is new again until you fix it, suppress it again, or save
again. Until a save is recorded, every finding at
or above `ratchet_min_severity` is new, and the report says to run the save.
The recorded set lives in `.logos/history.db` with the findings, not in the
repository. `session_start` records it before the edits when none is saved
and none is checked in, whatever the metric baseline's state, and never moves an
existing one, so only `gate --save` accepts findings introduced since.

**A checked-in baseline.** [`logos security bless`](commands.md#security-bless)
writes the same findings — fingerprint, rule id and path only — to
`.logos/security-baseline.json`. When that file exists the ratchet reads it
**instead of** the set in `history.db`, in every checkout, so CI needs no cache;
`gate --save` and `session-start` then record no set beside it, and only
`security bless` changes it
([ci-integration.md](ci-integration.md#gating-on-security-findings-in-ci-a-checked-in-baseline)).

**The caps** ignore the recorded set. `max_severity = "critical"` fails on any
unsuppressed critical finding, however old it is.

**Suppressions** in [`.logos/security-ignore.toml`](#logossecurity-ignoretoml--suppressions)
exclude a finding from every rule. To accept one finding, add its
fingerprint and a reason there. Each violation message prints the fingerprint.

**Freshness.** Only scans that are `fresh` at HEAD are evaluated. Fresh is the
state `logos security status` reports: anchored to this HEAD, over unchanged
files. Suppose a scanner named in `required_scanners` has no fresh scan: it
never ran, its scan is stale, or it is not installed or is disabled. Then
`check`, `gate` and `session-end` exit **5**, naming the scanner and what
produces the scan ([error-handling.md](error-handling.md#how-it-surfaces-on-the-cli)).
The gate never runs a scanner itself. A scanner the list does not name never
causes exit 5. If its stored scan is stale it is simply not evaluated, and a
note says so. A `gate --save` that exits 5 saves the metric baseline but not
the security one. An invalid `.logos/security-ignore.toml` stops `gate --save`
and `session-start` with exit 2 before either baseline is written.

The table never moves the 0–10000 signal. Security is not a metric dimension,
so adding, changing or removing the table changes no metric and triggers no
re-baseline. A blank scanner name, or an `fail_on_owasp` entry other than
`A01`..`A10` (optionally `:2025` or `:2021`), fails at load with exit 2.

### `[review]` — the branch-delta gate

Without this table, `gate` and `session-end` fail when the 0–10000 signal falls
more than 1 point below the baseline, whatever made it fall. That one number
cannot tell a new 6-deep function from the small Equality drift that any new
above-average function causes. Adding the table changes what fails:

- **A per-symbol regression fails.** `gate` and `session-end` compare the working
  tree with the merge-base of `HEAD` and the default branch, as
  [`logos review`](commands.md#review---base-ref) does. They fail (exit 1) on a
  `new` or `worsened` offender in a dimension `gate_dimensions` names. The
  failure names each one with its value, threshold and trust label.
- **The aggregate becomes a drift budget.** The signal's fall below the baseline
  fails the gate only past `drift_budget` points. The default, `"report"`,
  reports the drift and never fails on it.

```toml
[review]
# The dimensions whose new or worsened offenders fail the gate.
# Default: the five below. "redundancy" (dead or duplicate functions) may be added.
gate_dimensions = ["nesting", "conciseness", "cohesion", "focus", "uniqueness"]
# How many signal points the aggregate may fall below the baseline before the
# gate fails, or "report" to report the fall and never fail on it.
# Default: "report".
drift_budget = "report"
# Who may waive a gating entry: "owner" (only a human) or "agent".
# Default: "owner".
accept = "owner"
# How many declines in a row of one distributional dimension raise a trend
# warning. At least 1. Default: 2.
trend_window = 2
```

The dimension names are the ones `logos review` and `logos offenders` print:

| Name | What a per-symbol entry is |
|---|---|
| `nesting` | a function nested at least `nesting_depth` deep (default 4) |
| `conciseness` | a brain method: complexity, length and nesting all at or over their thresholds |
| `cohesion` | a class whose methods split into unrelated groups (LCOM4 ≥ 2) |
| `focus` | a god container: too many methods, or too long a span |
| `uniqueness` | a member of a near-clone group |
| `redundancy` | a dead or duplicate function (not gated by default: dead code rests on cross-file reachability) |

**Distributional dimensions never gate.** Equality, Modularity, Acyclicity and
Depth score a distribution or the module graph as a whole, so any feature moves
them. Their entries are still listed by `review`, and their movement is bounded
only through `drift_budget`.

**A distributional dimension in decline warns.** When one of the four fell in
each of the last `trend_window` recorded snapshots, `scan`, `gate`,
`session-end` and `review` carry a warning that names it, its values oldest
first (exact, as recorded), and the window:

```text
equality declined at each of the last 2 recorded snapshots that moved it: 0.9123458410127743 → 0.9081204475611208 → 0.9033771926630519 (review.trend_window = 2); a trend warning, never a failure
```

It never changes a verdict or an exit code. The snapshots are the ones
[`logos evolution`](commands.md#evolution) lists: every `scan` and
`gate` records one. A snapshot that did not move the dimension, such as a
`gate` right after a `scan` of the same tree, neither breaks the run nor counts
as a decline. A rise ends the run, and so does a snapshot the dimension did
not apply on, an empty graph, or a change of metric semantics or thresholds,
since values across those do not measure the same thing.

`gate --json` and `session-end` also carry the warning as a structured `trend`
list (`dimension`, `values` oldest first, `window`). The Quality page shows the
same warning beside the Gate verdict, read from the snapshots up to the one it
displays, and records nothing. Without the table, or with a `rules.toml` that does not load, the
page shows no warning and its verdict is unchanged.

**Which entries gate.** An entry gates when it is `new` or `worsened`, its
dimension is in `gate_dimensions`, and its trust label is `resolved`. An entry in
a file that changed since the last sync, or in a language the graph binds within
files only, is listed and never gates. Every listed entry is already at or over
its dimension's threshold: that is what makes it an offender. A `check` violation
does not gate here; a failing rule already fails `check`.

**Who may waive a gating entry.** Under `accept = "owner"`, no agent action can
waive one. Today nothing waives one at all: the entries are judged against the
merge-base, and `gate --save` does not move it. So `"agent"` changes nothing yet.

**When the branch delta cannot be measured** — no default branch resolves, `HEAD`
shares no history with it, or the merge-base tree cannot be indexed — the gate
judges no entry and adds a warning that starts `[review]: the branch delta was
not measured`. It never fails for that reason alone. The drift budget still
applies.

**The cost.** Every gate records a snapshot at `HEAD` over the working tree, so
a snapshot recorded at the merge-base commit may hold uncommitted work made
there. Once that work is committed, reading the snapshot as the before-state
would make a failing entry look `unchanged`. So under this table the before-state
is always the merge-base tree itself: it is indexed once per merge-base and
cached in `.logos/review-cache/`, and a recorded snapshot is never read.

`gate --save` and a first-run `session-start` record the baseline and judge
nothing; a later `session-start` keeps the baseline it finds. A
`gate --save` whose drop exceeds a numeric `drift_budget` must give
`--reason "<why>"`, and exits 2 without it. The reason is stored with the
baseline, together with the attribution of the change being blessed (see
[`gate`](commands.md#gate---save---threshold-n---label-l)). Under the table,
`gate`, `session-end` and the Quality page show that attribution. Without the
table, `gate`, `session-end`, `check` and the Stop hook print exactly what
they printed before; `review` prints the same plus its `attribution`. A `gate_dimensions` entry
that names a distributional dimension or an unknown one, a negative
`drift_budget`, any other value of it than a number or `"report"`, an `accept`
other than `"owner"` or `"agent"`, a `trend_window` that is not a whole number
of at least 1, and an unknown key all fail at load with exit 2. The table never moves the signal or
its baseline.

## Logging and telemetry

- **`RUST_LOG`** controls diagnostic verbosity on **stderr** (e.g.
  `RUST_LOG=debug logos index`). Stdout is never polluted: `--json` output
  and MCP frames stay machine-parseable at any log level.
- **Telemetry is local-only**: events (tool name, duration, ok/failure,
  surface, timestamp — never paths or source content) are appended to
  `.logos/telemetry.db` and feed `logos stats`. Recording activates only
  when `.logos/` already exists, never blocks a command, and degrades
  silently if the database is unwritable.

## The Claude Code plugin — environment variables

The [plugin](plugin.md) has no configuration file. Its hooks read three
environment variables, set in the shell that starts Claude Code (or in the `env`
block of your Claude Code settings):

| Variable | Default | Effect |
|---|---|---|
| `LOGOS_QUALITY_REPORT_DISABLE` | unset | `1` silences the session-start readout, whether the plugin or the hook `logos init -i` writes prints it. |
| `LOGOS_STOP_HOOK_DISABLE` | unset | `1` turns the Stop check off: the hook exits 0 with no output. |
| `LOGOS_STOP_HOOK_BUDGET_MS` | `15000` | The Stop check's budget, in milliseconds, shared by its `check`, `review` and `security` legs. A value that is not a number reads as the default. The plugin declares a 30-second timeout for the hook, so a value above `25000` is held at `25000`, and a leg still running then is reported as not measured. |

The guardrail that asks before an agent edits `.logos/rules.toml`,
`.logos/rules-baseline.json` or `.logos/security-ignore.toml` has no variable;
disabling the plugin is the only way to stop it asking.

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
.logos/plugins/<language>/queries/locals.scm
```

A file present at one of these paths replaces the embedded query for that
capability at startup (a non-compiling query fails fast and names the file).
This is the escape hatch for teaching Logos project-specific conventions
without rebuilding. The embedded queries under `logos-core/plugins/` serve as
reference starting points — each header documents the captures and the known
v1 limitations.

`<language>` is the plugin's name, so each language reads its own directory.
JavaScript (`.js`, `.mjs`, `.cjs`, `.jsx`) reads `.logos/plugins/javascript/queries/`:
an override under `typescript/` or `tsx/` does not reach JavaScript files — copy it
there to keep it. JavaScript runs the TSX plugin's queries, so
`logos-core/plugins/tsx/queries/` is its starting point, except `symbols.scm`:
JavaScript appends its own `logos-core/plugins/javascript/queries/symbols.scm` —
the functions it defines by assignment, `exports` or prototype — to the TSX one,
and a `symbols.scm` override replaces the pair, so start from both files.

`locals.scm` records the names each callable body binds locally — parameters,
local variables, loop, `catch` and pattern variables, local imports — with
their kind, position and scope. Its captures (`@local.parameter`,
`@local.variable`, `@local.import`, the `@local.name` each binds, and the
companions naming its scope) are documented in
`logos-core/src/extract/locals.rs`; an empty file records none. A binding in
force at a call shadows it: a bare call to a local import resolves through that
import, a bare call to a local parameter or variable binds nothing (counted
`local-binding` in `status`), and a rebound Go receiver name is `other`
([Commands](commands.md)). An empty `locals.scm` therefore also turns those
rules off for the language.

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

A call through an imported module or class follows its import. After `from
hc.lib import curl`, `curl.post()` is recorded as `hc::lib::curl::post`. It binds
the one `post` that the module the import names declares: a function, or a class
the call constructs (`Instantiates`). After `from .models import Check`,
`Check.for_project()` binds among `Check`'s own methods, and never reaches a module
function of the same name beside the class. `import a.b` binds the name `a`, as
Python does, and `import a.b as x` binds `x`. A call through an import that binds
nothing stays unbound and never falls back to a workspace module of the same name.
`os.getcwd()` is an example, and so is `os.path.join()`, whose receiver is no
imported name at all. A name that a parameter, a local assignment, a loop or `with`
target, or a module-level assignment rebinds is not the import, so a call through
it keeps its bare form — or the typed form below, where an annotation or a
construction proves the name's type.

A call on a receiver whose type the file proves binds among that type's methods.
`svc.run()` reads the one binding of `svc` in its function: a parameter annotated
`svc: Service`, an annotated local, or `svc = Service(…)`. `self.transport.notify()`
reads the attribute `transport` that the enclosing class declares itself: a
class-level `transport: Transport`, an annotated `self.transport: Transport = …`,
or `self.transport = Transport(…)`. `Optional[T]`, `typing.Optional[T]`,
`T | None` and a string annotation `"T"` read as `T`, and a generic `Repo[User]`
as `Repo`. The call is recorded as `Service::run`, keeping its `other` shape. The
type is read through the file's imports, as a written qualifier is, else in the
file. The call binds the one `run` of that class, else of the nearest level of
its proven `Extends` chain. A type the repository does not declare (`str`, a
`Session` imported from `requests`) binds nothing. So does `x = Factory()` where `Factory` is a
function: a construction is read only from a capitalised name, and a lowercase
`x = make()` proves nothing. A receiver with no proof stays
`no-receiver-evidence`: an untyped one, a name its function binds a second time
(a reassignment, a loop, `with`, `except` or `case` target, a walrus, a nested
`def` or `class`, an import, a comprehension or lambda variable, an inner
function's parameter), a `global` or `nonlocal` name, a module-level name read in
a function, a sibling function's local or a class-body name read in a method,
and a dotted (`models.T`), `Union[…]` or other union, `Annotated`, `ClassVar` or
`Final` annotation. So does an attribute the class inherits or never declares,
one it writes untyped (`self.x = make()`) or with two types, one a `def` or
`@property` of the class body also names, and `cls.x`. A call whose class holds an
attribute of the method's name (`run = other`, `self.run = callback`) binds
nothing: the attribute is what it calls. A type the file both imports and
declares (`class Service(Service)`) binds nothing either: which one an
annotation names is the statements' order.

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
(`A.B`, then `A`, for code in `A.B.C`), before any file-level `using` namespace. One type
decides a level; two at one level bind nothing (`type-ambiguous`) and the walk
never falls through to an outer level. Only the file's own interop family is
read, and the global namespace is never a level. A `using` written inside a
namespace block is read at that block's level, after the namespace's own types
and before the next level out: in `namespace A { using X; namespace B { using
Y; … } }`, code in `A.B` reads `A.B`, then `Y`, then `A`, then `X`. A `using`
written outside every block is read after every level, as are global ones. Two
types the `using`s of one block supply bind nothing, as two of one level do. A
block's `using`s are read at its level only when the block encloses every type
of the file. A block holding only some of the file's types has its `using`s read
after every level, as before; a block holding none of them, such as
`namespace A { using X; }` beside a separate `namespace A.B { … }`, has its
`using`s read nowhere.

**`type_arity` (`namespace` model only).** C# tells `Result` and `Result<T>`
apart by their number of type parameters. A plugin opts in, and the C# plugin
does:

```toml
[module_model]
kind = "namespace"
type_arity = true   # refused under any other kind
```

Each type declaration then records its type-parameter count, and each base type
or `new` the number of type arguments it writes (`Result` 0, `Result<int>` 1).
Types declared in the same file, in the file's own namespace, in each enclosing
namespace and in each `using` namespace are admitted only at the written count, so `Result` written in `A.Sub` passes
over a generic `A.Result<T>` to the `X.Result` that `using X;` supplies, and
`Result<int>` binds `A.Result<T>`. Two types of one name are ambiguous only when
both have the written count. The plugin's `symbols` query must capture each
type's type-parameter list as `@item.type_parameters`, or the plugin fails to
load; a type with no list counts 0. A call through a type name
(`Result.Make()`) records no count and binds as before, and a `using` alias and
a name written qualified (`X.Result<int>`) are still matched by name only.

**`companion_objects`, `top_level_callables` and `wildcards_outrank_package`
(`namespace` model only).** Scala declares a class and its companion `object`
under one name, a Scala 3 file declares `def`s at its top level, and a Scala
wildcard import outranks the package's other files. A plugin opts in to each,
and the Scala plugin opts in to all three:

```toml
[module_model]
kind = "namespace"
companion_objects = true          # refused under any other kind
top_level_callables = true        # refused under any other kind
wildcards_outrank_package = true  # refused under any other kind
```

- With `companion_objects`, a class and the same-named singleton declared beside
  it in one file are one name. A call on the name (`Chain.empty()`) binds among
  the singleton's callables only — the ones the `symbols` query marks
  `@item.receiver.none` — never an instance method of the class; a call on an
  instance the file types `Chain` (`chain.size()`) binds among the class's
  methods alone, never the singleton's. A path or a wildcard through the name
  (`import cats.data.Chain._`) reads both declarations' members. The name itself still names two declarations, so an
  import of it, or a `new Chain()`, binds nothing.
- With `top_level_callables`, a function a file declares at its top level is a
  member of the namespace the file declares, as its types are: `import
  ox.discard` binds it, `import ox.*` brings it into view, and a free call from
  the same namespace reaches it. Only a file of a language that declares the
  key reaches one, and only one its own language declares: Kotlin declares the
  key too, yet a Kotlin file never binds a Scala top-level `def`. A class of
  the same name keeps its import.
- With `wildcards_outrank_package`, a name a wildcard import brings into view
  outranks one the file's namespace declares in another file, as Scala ranks
  them. Java and Kotlin read the namespace first. The file's own declarations
  and its explicit imports come before both either way.

All three default to `false`, so a plugin that declares none binds as before.

**`directory_packages` (`path` model only).** In Go every file of one directory
that declares the same `package` clause shares one scope. A plugin opts in, and
the Go plugin does:

```toml
[module_model]
kind = "path"
directory_packages = true   # refused under any other kind
```

The plugin's `symbols` query captures the clause's name with `@package.name`, and
it is recorded beside the file; the file's module key stays its path's. A bare name
that the file itself, its imports and its dot-imports do not supply then binds to the
one top-level declaration of that name among the package's other files. A method
that only shares the name is never one. A name declared twice in the package, such
as build-tag variants (`term_unix.go`, `term_windows.go`), binds nothing. A file of
the same directory that declares another clause, such as `package foo_test` or a
`//go:build ignore` generator's `package main`, is a package of its own. A file
whose clause is not recorded (a store not re-extracted since the key was declared,
or a damaged parse) is in no package, and no name binds through its directory's
package until it is, since it may declare the name too. The key defaults to `false`,
so a plugin that does not declare it binds as before.

**`link_unit` (`path` model only).** A C program is linked from its translation
units, and a function defined in one is called from another through a prototype in
a header. A plugin opts in, and the C plugin does:

```toml
[module_model]
kind = "path"
link_unit = true   # refused under any other kind
```

The key reads the include scope, so it requires `bare_specifiers =
"include-roots"` (and with it `import_specifier = "path"`); a descriptor that
declares it without them is refused.

A bare call is read through its includes first: the file's own definition, then a
definition a file its `#include`s reach supplies (a `static inline` function or a
macro of a header). A prototype found there only names the function. Failing a
definition, the call binds to the one non-`static` definition of the name among
the language's files (`.c`, and `.h` where the repository's headers are read as
C). A `static` function is its own file's alone. Two non-`static` definitions,
such as platform variants (`src/unix/loop.c` and `src/win/core.c` each defining
`uv_loop_init`), bind nothing. A call through a function pointer — a
parameter, a local or a file-scope variable of the name — binds nothing. No
build file (`Makefile`, CMake,
`compile_commands.json`) is read, so a definition no binary links with the
caller is still a candidate; the exactly-one rule keeps the guess out. The key
defaults to `false`, so a plugin that does not declare it binds as before.

**`extension_functions` and `file_facade` (`namespace` model only).** Kotlin
declares functions that extend a type (`fun String.toSlug()`), and Java reaches
a Kotlin file's top-level functions through a class named after the file. The
Kotlin plugin opts in to both, beside `top_level_callables`:

```toml
[module_model]
kind = "namespace"
top_level_callables = true
extension_functions = true   # refused under any other kind
file_facade = "Kt"           # a bare identifier; refused under any other kind
```

- With `extension_functions`, a top-level function whose `symbols` query
  captures a receiver type (`@symbol.self_type`) is callable on a value. A call
  `title.toSlug()` whose receiver's type is not known binds the one such
  function of its name that the file imports, its namespace declares, or a
  wildcard brings into view, in that order; an import is final. Two such
  functions of one name in one of those scopes bind neither
  (`overload-ambiguous`). A member wins over an extension, and the receiver's
  type is not known, so a callable member of that name on any class of the
  language's family keeps the call unbound (`no-receiver-evidence`). Only the
  call's written arguments decide among overloads: an extension's receiver is
  not one of its parameters. A bare call `f()` reaches an extension only from
  inside an extension's own body, never past a member of the name, and there a
  member of the receiver's own type, when the repository declares it, comes
  before any top-level function.
- With `file_facade`, another language of the family reaches a file's
  top-level functions through a class named after the file: its stem, with
  every character a class name cannot hold replaced by `_` and the first
  letter upper-cased, then the suffix. `mixit/util/Utils.kt` declaring
  `package mixit.util` gives `mixit.util.UtilsKt`, so Java's `import
  mixit.util.UtilsKt;` binds that file and `UtilsKt.enumMatcher(s)` binds its
  `enumMatcher`. An extension takes its receiver as the first argument there.
  The declaring language never names the facade itself. A name set with
  `@file:JvmName` is not read.

Both default to off, so a plugin that declares neither binds as before.

**`companion_name` (`namespace` model only).** A Kotlin `companion object` is a
node of its own, nested in its class (`Svc#Companion#create().`), and its members
are reached through the class. The key names the companion a class has when it is
written without a name of its own:

```toml
[module_model]
kind = "namespace"
companion_name = "Companion"   # a bare identifier; refused under any other kind
```

- A call on a class that has a companion, `Svc.create()`, binds among the
  companion's own callables alone: an instance member of `Svc` is never what it
  calls. Two the call's arguments admit bind neither (`overload-ambiguous`), and
  one holding the name with none it admits is `no-applicable-overload`.
- A bare `create()` inside the class reaches the companion's members after the
  class's own and its supertypes', then the companions of its superclasses,
  nearest first, and only then any top-level function or import. A member of
  the class that the call's arguments admit still wins, and one they do not is
  passed over. A class nested in `Svc` reaches `Svc`'s companion too. A class
  whose base the repository does not hold reaches no companion: that base may
  declare the name.

What the key does not see:

- A companion declared with a name of its own (`companion object Factory`) is an
  ordinary nested object here: `Svc.Factory.create()` binds, `Svc.create()` does
  not. A plain nested `object Companion`, which is no companion, is read as one.
- A companion's own supertypes (`companion object : Factory()`) are not
  recorded, so a member it inherits is not reached, and `Svc.create()` on one
  stays unbound.
- A lambda's receiver is not known. Inside `Builder().apply { create() }` the
  call may be `Builder`'s, yet it binds the companion's `create` as it would a
  member of the class itself.

The key defaults to none, so a plugin that does not declare it binds as before.

**`global_function_fallback` (`namespace` model only).** PHP reads an unqualified
function name in the file's own namespace first, then in the global namespace. The
PHP plugin opts in, beside `top_level_callables`:

```toml
[module_model]
kind = "namespace"
top_level_callables = true
global_function_fallback = true   # needs top_level_callables; refused under any other kind
```

- A file that declares no namespace is in the global one. A bare call it does not
  declare itself binds the one function another such file declares at its top
  level — mantisbt's `gpc_get_int()` from its pages to `core/gpc_api.php` — under
  every binding policy, not only `aggressive`. A function declared inside an `if`
  (a `function_exists` guard) counts as declared.
- A namespaced file's bare call binds its own namespace's function first, then the
  global one. Under `strict` and `balanced` a namespaced function is never reached
  from another namespace, nor from the global one; `aggressive` still tries its
  workspace-unique name after these rules, as for every language. A class name
  never falls back.
- A name two files declare binds neither, whatever their parameters. A method
  that only shares the name is never a candidate.
- A file whose namespace is not known (its top-level declarations sit in two
  namespaces, such as `namespace { … }` beside `namespace Foo { … }`) may
  declare a global function, so a global function of a name it declares too
  binds nothing. Its own bare calls take none of these rules.

The key defaults to `false`, so a plugin that does not declare it binds as before.

A PHP `require`, `require_once`, `include` or `include_once` of a string literal,
or of `__DIR__ . '/…'`, records an `Imports` edge to the file it names, read
against the including file's directory. Any other argument records nothing: a
variable, `dirname(__FILE__) . '/…'`, a string with a variable in it, and a
helper such as mantisbt's `require_api('…')`. PHP itself tries the include path
before the file's own directory, so a literal that a file beside the includer does
not hold stays unbound.

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
  that macro node. A `.h` header C reads — in a repository whose C sources outnumber
  its C++ ones — declares its `#define`s as macros too.

The rule that binds every call still holds: exactly one candidate, or nothing. Two
classes of one name stay unbound. So do a function and a class (or macro) of one
name in one scope, and a name that matches no declaration. Kotlin and Scala read
a bare `Foo()` inside a class as a call on the current instance, so it binds among
that type's own members and never reaches the class — unless, in Scala, the file
shows that no member answers it: then it is the free call it is outside a class. Both keys default to
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
(`base_list`), Kotlin (the supertype list), Scala (`extends A with B`),
TypeScript (`extends`, `implements`) and Java capture them. A supertype
binds only to the one in-repository type its file's scope names — its own
module, its imports, its namespace or package, its wildcards — and never by a
workspace-wide name guess, under any binding policy. A library base (`TestCase`,
`IDisposable`, `\Psr\Log\LoggerInterface`) stays unbound. PHP's leading `\` and
C#'s `global::` make a name fully qualified: `namespace Foo; class Exception
extends \Exception` never names itself. An `Extends` binds a class (an interface,
from an interface); an `Implements` binds an interface or a trait.

C# and Kotlin write the base class and the interfaces in one list (`class A : B,
IC`, `class A : Base(), Iface`), and Scala's `extends A with B with C` may name a
trait first and a class without `with`, so their descriptors declare:

```toml
# C#, Kotlin, Scala: the supertype list does not say which entry is the class.
supertype_kind_follows_target = true
```

Each entry is then captured as `@ref.extends`, and a class's entry binds the one
class, interface or trait it names. Its edge kind follows the target:
**`Extends`** to a class, **`Implements`** to an interface or a Scala trait (the
cake pattern's `with AccountService` mixins). A Scala self-type (`self: A with B
=>`) is no supertype and records nothing. An `object` is never a supertype, so a
mixin named by a trait and its companion `object` (`trait AccountService` beside
`object AccountService`) binds the trait; a name a class shares with its companion
`object` names two classes the graph cannot tell apart, so it stays unbound. The key defaults to
`false`, where each clause binds the kind it spells.

TypeScript's types are structural, so its descriptors (both grammars, and
JavaScript's) declare:

```toml
# TypeScript, TSX, JavaScript: every type relation relates its declaration.
structural_types = true
```

With it, a TypeScript file records every type relation, not only a type's
supertypes: `@ref.instantiates` records the class `new T(…)` constructs
(**`Instantiates`**), and `@ref.type_use` the declared type of a field,
parameter, return value, variable or property signature (**`TypeUses`**) — its
type arguments, and the members of a union, intersection or indexed access
(`Dto | null`, `User['id']`), included. Each binds from whatever declaration it
sits in. An `implements` binds an abstract class or a type alias as readily as an
interface, and an interface's `extends` any type; a class's `extends` and a `new`
still bind a class. A type named through a named or default import is recorded as
`<module>::<Name>` and binds only within what that import binds to — through a
barrel, a `tsconfig` alias or a workspace package, as a call through it does. One
the file declares binds in the file, and one it neither declares nor imports
binds nothing. A type parameter, a primitive, a call (`extends Mixin(Base)`) and
a type named through a namespace import (`ns.Dto`) record nothing that binds. A
type use written in an abstract or interface member signature stays unbound. The
key defaults to `false`.

With `structural_types` on, a call on a receiver whose type the file proves
binds among that type's members. `this.users.findByEmail()` reads the field
`users` of the enclosing class itself: a constructor parameter property
(`constructor(private readonly users: UsersService)`) or an annotated field.
`users.findByEmail()` reads the one binding of `users` in scope at the call: an
annotated parameter or variable, or `const users = new UsersService()`. The call
is recorded as `<module>::UsersService::findByEmail` through the type's named
import, keeping its `other` shape. It binds the one `findByEmail` of
`UsersService`, else of the nearest level of its proven `Extends` chain. On an
abstract class or an interface it binds the declared, bodyless method, never an
implementation; this is the only call that reaches one. A type the repository
does not declare binds nothing. A receiver with no proof stays
`no-receiver-evidence`: an untyped or inferred one, a qualified (`ns.T`), union
or array annotation, and a name two bindings in scope spell — a shadowing local,
an inner function's parameter, a module-level variable beside a parameter. So
does a field the class inherits or does not annotate, and a call on a class name
(`Mapper.from()` without an import). The JavaScript plugin runs the same queries
over `.js` files, where `const x = new T()` is the one proof that can be written.

A proven `Extends` is also what a call on the current instance climbs: a Python
`self.m()`, PHP `$this->m()`, C# or TypeScript `this.M()` or unqualified Kotlin/C# `m()` reaches
an inherited `m`, and Python's `super().m()`, PHP's `parent::m()`, C#'s `base.M()`,
Kotlin's and TypeScript's `super.m()` bind the nearest supertype that declares
exactly one `m`. An `Implements` is not part of that chain, so `base.M()` in a class whose only
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
# Go, Rust, Python, PHP, TypeScript, TSX, JavaScript: a method is reached only through a
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

# JavaScript (`js`, `mjs`, `cjs`, `jsx`): the language enforces no arity, so a
# call in these files is never filtered. The range is still recorded.
arity_unchecked_extensions = ["js", "mjs", "cjs", "jsx"]

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

### Calls never name a value (`separate_call_namespace`)

```toml
# Java: `String name = …; name()` calls the method `name`, never the local.
separate_call_namespace = true
```

A language whose calls and values are separate namespaces declares it: a local
parameter or variable never shadows a bare call there, so the call binds as it
would without the local. Java and Ruby declare it (Ruby's `f()` with
parentheses calls the method even where a local `f` is assigned). A local
import still shadows. It defaults to `false`: a local value in force at a bare
call of its name makes it a call through that value, which binds nothing.

### Member access (`member_access`, `member_access_excluded_extensions`)

```toml
# Java, TypeScript, TSX, JavaScript, PHP, C#, Python, Scala, Kotlin, C++, Ruby: the references query captures
# own-field access (`this.x`, `$this->x`, `self.x`, Ruby's `@x`, and C#'s, Scala's, Kotlin's and C++'s bare `x`) as Accesses
# edges, so a class's methods can share fields.
member_access = true
# Python: its `.pyi` stubs are carved out of the declaration.
member_access_excluded_extensions = ["pyi"]
```

The Cohesion dimension ([metrics.md](metrics.md#8-cohesion-lcom4--do-classes-hang-together))
scores a class only when its language declares `member_access`; a class of any
other language, or in an excluded extension, is not applicable rather than scored
from its intra-class calls alone. It defaults to `false`. Each
`member_access_excluded_extensions` entry must be one of the plugin's
`extensions`, and the key is accepted only beside `member_access = true`. The
capability is not listed by `logos languages`.

### Trait containers (`bodied_traits_are_containers`)

```toml
# Scala: a trait with a bodied method is a container, as a class is.
bodied_traits_are_containers = true
```

Focus and Cohesion ([metrics.md](metrics.md#9-focus--are-containers-god-objects))
read classes and structs as containers. A language whose traits carry state and
behaviour declares this key, and a trait of it with at least one method with a
body then counts as a container too: Focus counts it, and Cohesion scores it when
the language also declares `member_access`. A trait of abstract declarations only
stays no container. It defaults to `false`, so a Rust trait's default methods or a
PHP trait never make a container. Only Scala declares it.

### Shared extensions (`shared_extensions`)

```toml
# C: `.h` is claimed by C++ too; C reads it only in a C repository.
extensions = ["c", "h"]
shared_extensions = ["h"]
```

An extension two plugins claim is read by one of them per repository. The plugin
that declares it shared reads it when the repository's files of its own other
extensions outnumber the other plugin's; the other plugin reads it otherwise, a
tie and a repository of shared files alone included. So C reads `.h` in redis
(475 `.c` files beside 8 C++ ones) and C++ in a C++ project. Only the files the
`languages` list admits are counted. The decision is recorded with the graph: a
`sync` whose files move it re-reads every header, and `status` states it on the
reading row when the repository holds both languages ([Commands](commands.md)).
Each entry must be one of the plugin's `extensions`; it defaults to empty. Only C
declares it.

### Preprocessor regions (`preprocessor`)

```toml
# C#: each `#if` region is read as its first branch.
preprocessor = "c-sharp"
```

Before a file is parsed, each `#if … #elif … #else … #endif` region is reduced to
its first branch: the other branches and the region's `#if`, `#elif`, `#else` and
`#endif` lines are blanked to spaces, so every node keeps the line and column it
has in the file. Other directives (`#region`, `#pragma`, `#nullable`) are left to
the parser. No symbol is
evaluated, since the build's defines are unknown. A type written two ways under
`#if` (Newtonsoft's `class JsonWriter` takes `: IAsyncDisposable` only
`#if HAVE_ASYNC_DISPOSABLE`) is then one class, with its members under it, rather
than a class named after the symbol. A file whose regions do not balance is
parsed as written. A directive is recognised by the first non-blank character
of its line, and strings and comments are not read, so a directive line inside a
multi-line string or comment counts too: a lone `#else` or `#endif` there pairs
with the region around it and can blank the code after it. The
default, `"none"`, parses every file as written. Only C# declares it.

### Bare specifiers (`bare_specifiers`)

```toml
# TypeScript, TSX, JavaScript: a bare specifier (`@/lib/api`, `@acme/ui`) binds through the
# importing file's tsconfig/jsconfig and the workspace packages.
bare_specifiers = "tsconfig"
```

A **bare** specifier is a path specifier that is not relative. With the default,
`"external"`, it names a package and its import stays unbound. With `"tsconfig"`:

- The importing file's nearest `tsconfig.json` (else `jsconfig.json`) applies:
  the one in its own directory or the closest above it. Each package of a
  monorepo is therefore governed by its own config.
- Its `extends` chain is followed inside the repository, up to 8 configs deep. A
  base may be a relative path or a file of a workspace package
  (`@repo/config/base.json`). A package is found through any indexed file in
  its directory, its own `package.json` included while `[config_artifacts]` is
  on. A base outside the repository contributes nothing.
- The specifier is rewritten by the `compilerOptions.paths` pattern it matches
  best (an exact pattern, else the longest prefix), each substitution in turn,
  then by `compilerOptions.baseUrl`. Each rewrite resolves like a relative
  specifier: extensions first, then `index`. The first rewrite that names a
  file decides; one that names two files (`ui.ts` beside `ui.tsx`) leaves the
  import unbound.
- Otherwise, a specifier naming a workspace package (the `name` of a
  `package.json` in a directory of an indexed file or above one) resolves to
  that package's entry. The candidates are tried in this order: `exports` (its
  `import`, `types` and `default` conditions), then `types`, `module`, `main`,
  then the package's `index`. A subpath (`@acme/ui/button`) resolves through the
  `exports` subpaths, or under the package root when `exports` lists none. A
  name two packages declare resolves to neither.

A call through such an import binds exactly as a call through a relative import
does. A call through an external package (`react::useState`) stays unbound, even
beside a workspace function of the same name. Editing any `.json` file (a
config, an `extends` base, a `package.json`) re-binds the imports on the next
sync. The key requires `import_specifier = "path"`.

```toml
# Ruby: a bare `require 'a/b'` binds against the repository's load roots.
bare_specifiers = "load-path"
```

With `"load-path"`, a bare specifier binds against the **load roots**: the
repository root, plus each gem's `require_paths`, read from every `*.gemspec`
in a directory of an indexed file of the language or above one. A gem whose
gemspec assigns no `require_paths` (or `require_path`) has `lib`; a literal
list (`["lib", "ext"]`, `["lib".freeze]`, `%w[lib ext]`, `"lib"`, `<< "ext"`,
`+= ["ext"]`) replaces or extends it, and a computed value (`Dir[…]`, a
constant, an interpolation, `.push(…)`) contributes no root. Only code is read:
a mention in a comment, an `=begin` block or a heredoc assigns nothing. Without
a gemspec, `lib/` is not a load root.

- `require 'rack/lint'` binds the one file `<root>/rack/lint.rb` across every
  root; a trailing `.rb` names the file itself. A specifier that two roots
  answer stays unbound, and so does one that no root answers
  (`require 'json'`, a gem from outside the repository).
- A relative specifier (`require_relative`) binds the one file it names beside
  the requiring file, `..` included, never a directory's `index` file.
- A bare `require './x'` names a file under the working directory, read as
  `x` against the load roots — never beside the requiring file. One headed by
  `../`, or an absolute path, records no import.

Editing a `*.gemspec` re-binds the requires on the next sync. The key requires
`import_specifier = "path"`.

```toml
# C, C++: `#include <a/b.h>` binds under the include roots; `"x.h"` beside the
# including file first.
bare_specifiers = "include-roots"
```

With `"include-roots"`, a header is named whole, extension included, so
`hash.hpp` beside `hash.cpp` is no ambiguity:

- `#include "x.h"` binds the file it names beside the including file, `..`
  included; when there is none, it binds as `<x.h>` does.
- `#include <a/b.h>` binds the one file `<root>/a/b.h` across the **include
  roots**: every directory named `include` or `src`, at any depth, and every
  directory holding an `include` directory (a repository root with `include/`
  is one). Only directories of indexed files of the language count. A header
  two roots answer stays unbound, and so does one no root answers (`<vector>`,
  or a vendored library under a directory that is no root).
- An absolute header (`"/opt/x.h"`) records no import, and nor does a
  macro-named one (`#include HEADER`): no preprocessor runs.

A free or qualified call and, in C++, a base class then bind through the
declarations the file's includes make visible, transitively (see [`logos languages`](usage.md)
for the reach). The root set is read off the indexed files, and any change
re-binds every reference of the language on the next sync. The key requires
`import_specifier = "path"`.

#### Re-exports, default exports and dynamic imports (TypeScript, TSX)

A call through an import binds to what the imported module provides under that
name, not only to a function the module defines itself:

- **Re-exports are followed.** `export * from './spinner'` and
  `export { a as b } from './x'` (`{ default }` and `{ default as X }`
  included) are imports of the module they name. A name imported from a barrel
  is followed through them, barrel after barrel, up to 8 re-exports deep. A
  module's own export of a name hides one an `export *` brings in, and
  `export *` never re-exports `default`. An imported name exported again
  (`import { a } from './x'; export { a };`) is a re-export too.
- **Default imports bind.** `import App from './app'` and
  `import { default as App } from './app'` name the module's default export:
  `export default App`, `export default function App() {}`, or
  `export { App as default }`. For `export default <expression>`, it is the
  `default` symbol of that statement.
- **A literal `import('./routes/landing')` is an import.** It records an import
  edge from the function that makes it. A template or computed specifier
  records none.
- **A call through an imported class binds among its methods.**
  `UserMapper.toDomain(raw)` after `import { UserMapper } from
  './mappers/user.mapper'` — or a default or aliased import of the class —
  binds to that class's one method `toDomain`, static or not, never a function
  of that name beside the class. A class from an external package
  (`Logger.log()` from `@nestjs/common`), an imported function or value
  (`config.get()`), a class declaring the name twice, or a name that reaches two
  classes leaves the call unbound. A call through a namespace import
  (`import * as utils`, `utils.format()`) binds the module's function, as
  before.

The call binds only when exactly one definition is reached. A re-export cycle,
or a name that two modules provide (two `export *` that both bring in
`helper`), leaves the call unbound. One definition reached by two routes is
still one. Only a top-level function or value binding is a call target this
way, never a class itself — a member call on an imported class binds among the
class's methods, as above. A call through a namespace a barrel re-exports with
`export * as ns` (`ns.f()`) is not followed.

#### CommonJS modules (`commonjs`)

```toml
# JavaScript: `require` bindings and `module.exports` are imports and exports.
commonjs = true
```

A `javascript` file's CommonJS imports and exports bind like the ECMAScript
ones above. Only the JavaScript plugin declares the key; it defaults to `false`,
where a `require()` call records its import edge and binds no name.

- **`const m = require('./x')` is a qualifier.** `m.f()` binds what `x`
  provides under `f`, and `m()` calls `x`'s export object (below). `var` and
  `let` declarations count too.
- **`const { f, g: h } = require('./x')` binds named imports.** `f()` and `h()`
  bind what `x` provides under `f` and `g`, a default value (`{ f = d }`)
  included. A rest element or a nested pattern binds nothing.
- **`module.exports = X` and `const X = module.exports` name the export
  object.** Its members are the module's names: `User.exists` of
  `const User = module.exports; User.exists = async function …` is what
  `user.exists()` binds after `const user = require('./user')`. The object is
  the module's default export as well, for `m()` and for
  `import X from './x'`. A module that assigns two different objects to
  `module.exports` exports neither's members.
- **`module.exports = { a, b: c }` exports `c` under `b`.** A shorthand `a`
  exports the declaration found by its own name. A value the module imported
  by name (`const { lookup } = require('./dns')`) is re-exported.

Only a `require` that is the whole initialiser of a declaration at the top of
the file binds a name. A `require` inside a function or a block, a called one
(`require('./x')(User)`), or one read for a property (`require('./x').y`)
records its import edge and binds none, and a function parameter or local
named like the binding shadows it. Only a top-level `module.exports` statement
records an export, and a function assigned to it directly
(`module.exports = function () {}`) names no declaration. A `require` of an external package (`require('express')`) stays
unbound, and so does every call through it (`express.Router()`), even beside a
workspace function of the same name. The specifier resolves as an `import`
specifier does: relative, or through `bare_specifiers` aliases and workspace
packages. A method of a class exported this way (`module.exports = Foo`,
`m.create()` on a static `create`) is not a call target.

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
