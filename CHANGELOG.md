# Changelog

All notable changes to Logos are recorded here. This project adheres to
[Semantic Versioning](https://semver.org/).

The dogfood point releases between 0.2.0 and 0.7.6 advanced the self-dogfood pin
without a capability change and were recorded only in the commit history;
0.8.0 is the next notable, capability-bearing release.

Releases 1.1.0 through 1.4.13 shipped without changelog entries. The entries for
them below are one-line summaries reconstructed afterwards from the release commits
and sprint records. 1.4.2 and 1.4.4 were never released.

## [Unreleased]

## [1.9.1] — 2026-10-04

### Fixed

- **An index never silently empties.** Same-named declarations of different kinds —
  a Go function and method `F`, a TypeScript `interface X` and `class X`, a Scala trait
  and its companion `object`, a C `typedef struct S S` — collided on one symbol and
  aborted the whole index while `index` exited `0` with 0 files. They now get distinct
  symbols (siblings are numbered per descriptor family), and a declaration whose name
  the parser could not recover emits nothing. zap, ollama, preact, gitbucket, ox,
  ccache, nlohmann/json, redis and libuv now index every admitted file. Symbols of
  Rust and Java code are unchanged.
- **One file that cannot be persisted fails alone.** Each file persists under its own
  savepoint: a failing file is rolled back alone, listed in `files_failed` with its
  reason (`persist_failures`), and named in a warning, while every other file is
  stored. On `sync` and in the `serve` watcher a failing file keeps its last good facts
  and is marked stale until it persists. `status` and `scan` report
  `persistence: {failed_to_persist, stale_files}` when anything remains. An
  `index`/`sync` that reached files and persisted none exits `1`; an index that
  admits no file still exits `0`.
- **No declaration takes a parse-error region's span.** Error recovery no longer lifts
  a declaration into an enclosing ERROR node (ccache's `parse_umask` spanned a whole
  627-line file at complexity 117; it now spans its own declarator), and a C++ class
  head stranded in an ERROR is recovered (nlohmann/json's `class exception`). The
  file's partial-extraction warning counts what was affected:
  `N declaration(s) truncated and M skipped at a parse error`.
- **Test code is classified by where it lives.** A `test`/`tests`/`spec` directory
  under a production source root (`src/main/…`, a Gradle `*Main` source set) is
  production; `src/it/` and Gradle `src/*Test/` source sets are test code; a
  `*.test.*` tag needs a three-part file name (a bare `test.py` is not a test); a PHP
  `test*` method is a test only in a `*TestCase` subclass or a test file; and a
  module node is no longer re-judged by the test-name markers.

### Changed

- **Every language states its cross-file reach.** `logos languages` (and `--json`)
  carries `reach.level` and `reach.cross_file` for each code language: `resolved`
  (Rust, Java), `partial` (Go, TypeScript/TSX, Kotlin — imports only), `same-file`
  (Python, PHP, C#, Ruby, Scala) and `symbols` (C, C++). The manual's table and the
  README are generated from the same declarations, and a fixture per language fails
  the build if a declaration over- or under-claims.
- **A bare-name lookup prefers code — in `node`, `callers`, `callees`, `impact` and
  `explore`.** When a bare name matches several nodes, each of the five resolves to a
  code type, else a callable, else another code declaration, else a module, else a
  configuration artifact, else a documentation node, and lists what it passed over as
  `alternatives` (CLI and MCP). Qualified and SCIP lookups are unchanged, and an answer
  that passed over nothing carries no `alternatives` key. `callers Utils` on a PSR-4 PHP
  tree now reports the callers of the `Utils` class rather than of its `Utils.md` doc
  section or file module. `impact-intersection` keeps its lowest-id resolution and its
  ambiguity warning.

### Upgrade notes

- **Store migration 27** (forward-only) adds the `persist_failures` table. The first
  run of this release on an existing project migrates `.logos/logos.db`; Logos 1.9.0
  cannot open the migrated store afterwards (exit 3). Back up `.logos/` first if you
  may need to downgrade.

## [1.9.0] — 2026-10-03

### Added

- **Health shows the worst offenders its snapshot computed.** Every `scan`, `gate`,
  `session_start` and `session_end` now persists its per-dimension worst-offender lists
  with the snapshot (migration 26), and the Health page reads them from the same single
  snapshot read as the signal. Each drill-down renders one of three states: the offender
  table, *No offenders flagged within thresholds* (only for a recorded-empty result), or
  *Offenders were not recorded for this snapshot — run `logos scan`* for a snapshot
  written before this release. `scan --json`'s `worst_offenders` carries a `recorded` flag.
- **The Workspace Chat has its own view.** In a workspace serve the Workspace section
  lists Chat at `/workspace-chat` and the Service section lists none; `/chat` redirects
  there with its query. Chat state is kept per scope, so a thread from one chat is never
  reopened in another. `GET /api/v1/workspace/config/read-roots` lists each member's
  effective read roots without starting an engine.
- **`[metric_thresholds] duplicate_min_tokens`** (default 50): a function counts as an
  exact duplicate only when it has a body and at least that many normalized tokens.

### Changed

- **Metric semantics v7: declarative code stops counting.** Extraction records whether a
  callable has a body (migration 25). Cohesion (LCOM4) and Focus count only bodied
  methods; bodyless declarations never count as duplicates; the Uniqueness list ranks
  clone groups by mass (members × mean lines). The first `gate` after upgrading re-baselines
  once (`baseline reset: metric semantics changed`). Signals rise on declarative code: on a
  real 84-member Spring estate, no member's signal fell.
- **Upgrade:** opening a store applies migrations 25 and 26, and the next `logos scan` (or
  `logos index`) re-extracts every file once. Until then callables count as bodied. A bare
  `logos sync` re-reads no file. Earlier `logos` versions refuse an upgraded store.

## [1.8.3] — 2026-10-02

### Changed

- **The workspace chat checks its read roots before a turn starts.** The workspace
  tier's `[chat] read_roots` and every member's effective ones are checked when a
  `POST /workspace/chat` turn starts — config reads only, no member engine is started.
  A bad read-root entry fails the turn up front, by name (the declaring root, the entry
  and why), before anything is recorded, as the member chat already does. A member whose
  `[chat]` cannot be read, or whose root is gone, is skipped by that check and keeps its
  fault on its own addressed source calls.

## [1.8.2] — 2026-10-02

### Added

- **The workspace chat is its own service, route and store** (`agents` builds, workspace
  serve only). `POST /workspace/chat` runs a turn with the same intent guard, host guard,
  consent disclosure and SSE/buffered contract as `POST /chat`; its conversations are at
  `GET /api/v1/workspace/chat/threads[/{id}]` and `POST …/{id}/delete`. It ignores `?repo=`
  and answers `404` in a single-root or `--standalone` serve. Its policy and credential come
  from the workspace tier alone — a member's own `[chat]` does not configure it, and the
  configure-first state names the workspace root and the missing half. Turns are stored in
  `<workspace root>/.logos/chat.db`, which the generated workspace-root ignore entry now
  covers; no member's `chat.db` is created or changed. There is no in-app view yet.
- **A workspace-centred chat roster.** The workspace chat's planner and Synthesizer are told
  the workspace's members and their kinds; a Workspace-Analyst holds the workspace
  read-model tools (`workspace_status`, `workspace_reachability`, `workspace_check`,
  `xservice_build_deps`, `workspace_roster`) and the four `xservice_*` tools under its own
  cap; Graph-Navigator, Governance-Analyst and Source-Reader take a required `repo` and open
  only the member they address, reading through that member's own sandbox and its
  *effective* `[chat] read_roots`. The Synthesizer ranks members by their own named signals
  and never states a composite workspace score.
- **Follow-up chat turns see prior turns.** The planner and the Synthesizer of both chats
  receive a bounded window of the thread's earlier turns, oldest first, bounded by two new
  `[chat]` keys: `history_max_turns` (default 6, in `[1, 50]`) and `history_max_chars`
  (default 16000, in `[1, 200000]`). When turns are dropped the prompt says how many. The
  first turn of a thread renders exactly as before.
- **Cross-service answers name the members they read.** `xservice callers`, `impact` and
  `route-providers` — CLI `--json`, MCP and `/api/v1/workspace/*` — carry a `member_reads`
  field: `read`, and `unread` with each member's reason.

### Changed

- **The member chat in a workspace serve is single-backing.** `/chat?repo=<member>` no
  longer carries the `xservice_*` tools or the federated addenda; cross-service questions
  belong to the workspace chat.
- **A warm cross-service bridge reads only the members a query needs** instead of every
  member's sync stamp on each query; results are unchanged.

## [1.8.1] — 2026-10-01

### Changed

- **The minified-JavaScript exclusion notice is an advisory note, not a warning.** The
  "`<N>` minified JavaScript file(s) excluded from indexing by the `**/*.min.js` exclude
  glob" line moved from `warnings` to the advisory `notes` channel that `index` already
  uses. It is a permanent, benign statement about a default working as intended, yet it
  repeated on every `index` and every reconcile-backed readout and could trip a CI parser
  that scans `warnings`. It now appears under `notes` on `index` (human and `--json`) and
  on `scan`, `check`, `gate`, `dsm`, `doc_gaps` and the config-apply result, and no longer
  under `warnings`; each read-model's `notes` key is omitted when empty, so output with
  nothing to note is byte-identical to before. The oversize-file notice stays a warning.

### Fixed

- **`logos sync --help` no longer claims `sync` defaults to all changed files.** With no
  path `sync` re-reads no file, as `docs/howto/commands.md` says; the help now says the
  same and points at `logos scan` / `logos index` for folding in every change. Help text
  only — `sync`'s behaviour is unchanged.

## [1.8.0] — 2026-10-01

### Added

- **Members record the types they declare.** An index records every top-level Java/Kotlin
  class, interface, enum and record under its package-aware fully-qualified name, with its
  node's symbol and whether it sits in the main or the test tree, and every Avro `.avsc`
  record and enum (nested named types included) under its namespace, with the schema's path.
  A source file whose `package` statement disagrees with its directory is recorded refused
  with both named; a malformed schema is recorded with its reason and yields no name. Store
  migration 24 (two new tables); a store upgraded from 1.7.0 fills them on its first full
  reconcile, which re-extracts the member's Java/Kotlin files once. A member with no
  Java/Kotlin/Avro file gains only the empty tables and one marker row.
- **An import of another member's type binds, as an advisory type reference.** In a
  workspace, a still-unresolved Java/Kotlin import naming a type exactly one other member
  declares (main-tree source or an Avro schema) is matched to it, with the importing file and
  line and the declaring file or schema. It is admitted only where the build relation relates
  the two members, or the importer references a colliding artifact the owner produces (the
  artifact named);
  every other match is listed `type-only`, and a type several members declare stays unbound
  as `ambiguous-owner`, owners named. `workspace status` gains a `type_reference` section:
  `type_reference_pairs` beside every row considered, by bucket, and the members read. It is
  never a bridge edge, a coupling or a gate input: the coverage figures, the build headline
  and every member's `scan`/`gate` are unchanged. A workspace with no Java/Kotlin/Avro member
  shows no new section.
- **`logos xservice type-refs [--repo]` lists the type references, and `callers`/`impact`
  follow them.** Per provider member, `type-refs` lists the types other members import, each
  with its declaration and every importer's file and line, under the `type_reference`
  headline beside its denominators; `--repo` scopes the listing to one provider, and a name
  that is not a member read answers a `scope_note` instead of an error. The MCP twin is
  `xservice_type_refs`. `xservice callers` and `xservice impact` on a type — its node, or its
  dotted name, the only handle an Avro type has — add a `via_type_reference` section, apart
  from the bridge-reached results and never merged with them: each entry is tagged
  `via type reference` with the reference it was reached through, and for `impact` carries the
  files depending on the importing file in its member, or that member's error when it will not
  open. A member whose declared types could not be read is named, with its reason, in
  `type_reference_unread`. A symbol no type reference names answers exactly as before. Advisory, never a
  coupling. As with every `xservice_*` tool, `logos serve --mcp` does not yet route the twin:
  it serves the single-root engine even inside a workspace.

### Changed

- **Kotlin files are keyed by their package, as Java files are.** The Kotlin plugin now
  declares `[package_modules]` under `src/main/kotlin` and `src/test/kotlin`, so a Kotlin
  file there is `com.x.Svc` rather than `main::kotlin::com::x::Svc`: Kotlin imports of
  in-repository types can bind, and a type declared under one name in both trees stays
  unbound. Kotlin projects will see more resolved imports after their first full reconcile or
  re-index, and `status` reports the Kotlin row's `call_residue` as it does Java's. Kotlin type
  relations (`Extends`/`Implements`/`TypeUses`) are still not captured.
- **TypeScript and TSX class fields are `Field` nodes, so `this.x` binds.** A declared field, a
  `#private` field and a constructor parameter property (`private readonly http`) of a
  `class` declaration are now captured as `Field`, and a `this.#x` read is an own-field access like
  `this.x`. The exactly-one `Accesses` binding finds them, so LCOM4 sees the fields methods share.
  A parameter property is owned by the class, not by its constructor. A getter, a method, an
  inherited member and an absent name still stay unresolved, and so does a field of an abstract
  class or a class expression (neither is a captured class, so such a field is not created at
  all). A field initialised with an arrow function is a field, and refs inside an initialiser are
  attributed to it. On the reference estate's TypeScript, first-party own-field accesses bound
  0 → 334 of 507; vendored JavaScript stays 0 of 6,805 (no class fields there). A new field can
  make a bare name ambiguous where it used to bind by coincidence: it then stays unresolved.
- **Minified JavaScript is no longer indexed by default.** The default code `exclude` gains
  `**/*.min.js` (any depth), so minified third-party bundles stop contributing nodes and
  unresolved references; `index` reports how many such files it skipped. An existing graph
  purges them on the next `index` or reconcile, because the admission fingerprint changes. A
  `config.toml` that sets its own `exclude` replaces the default and re-admits them.

## [1.7.0] — 2026-09-30

### Added

- **A vendored spec is a declared contract.** An OpenAPI document a member holds but does not
  implement declares `declares-contract(A → C)`: to the member whose own spec holds ≥ 90 % of its
  operations (document identity, score and matched document named), or else to a **named
  external** grouped across copies. Names are not unique, so an external is identified by the
  `member:path` of its first copy. A `kind = "mock"` member is a stand-in provider; a
  `documentation` member's copies stay out. Built in memory on the first cross-service query; no
  migration. Published as `coverage.declared_contracts`, headline `declared_contract_pairs` over
  every spec document read. On the reference estate: 6 pairs (1 by identity, 5 to named
  externals) from 7 vendored of 41 spec documents.
- **A no-provider call binds to the external its member vendors.** A `no-provider-in-workspace`
  REST call whose path, under a base path its member's committed sources prove (application
  configuration, or agreeing committed deploy overlays), equals exactly one operation of an
  external that member declares is reported in `coverage.bound_external`, with the file, key and
  matched operation. Every other case is refused with its reason. On the reference estate: 21 of
  32 rows bound.
- **Declared contracts and named externals on every surface.** `workspace status`,
  `xservice route-providers` (CLI `--json` and MCP; `--repo` adds `declared_scope_note`) and the
  web service map — a dotted declared-contract edge class with its legend section, named-external
  nodes, and a declared-relations card on the Coverage tab. A workspace with no vendored spec and
  no `kind` renders every surface unchanged.

### Changed

- **Modularity drops out of a graph too small to have community structure.** Below 5
  module-rollup edges Modularity is *not applicable*: its computed pair is kept, it leaves both
  the geometric mean and the zero short-circuit, and `quality-report --json`, `scan --json` and
  the dashboard say why (`n of 5`). Metric semantics 5 → 6, so every project re-baselines once
  (`baseline reset: metric semantics changed`). Graphs with 5 or more edges score exactly as
  before; on the reference estate 7 small members rise (e.g. `notification-kafka-models`
  0 → 9725) and all 63 others are byte-identical.
- **Forward-only store migration 23** (`metric_snapshots.modularity_applicable`). Once 1.7.0 opens
  a store, 1.6.x can no longer open it.
- **Declared figures stand beside the runtime ones, never inside.** `resolved_cross_service_edges`,
  `egress_resolution`, `no_provider_in_workspace` and the bridge edge set are unchanged by either
  new relation. `workspace status --json` grows about 3 % on the reference estate and
  `route-providers --json` about 29 %.

## [1.6.0] — 2026-09-29

### Added

- **A plugin can declare a package-shaped module path.** A `[package_modules]` table in
  `plugin.toml` lists `source_roots` (for example `src/main/java`). A file under one is keyed by
  the path after the root, so `src/main/java/com/x/Svc.java` is `com.x.Svc`, the name its
  imports spell. The table is off by default; Java opts in, and every other language keeps its
  module keys.
- **Java type relations are edges.** A class's superclass and an interface's super-interfaces
  bind as `Extends`, a class's interfaces as `Implements`, `new T(…)` as `Instantiates`, and a
  declared field, parameter, local or return type (type arguments included) as `TypeUses`. Each
  binds only to the one in-repository type of the right kind; JDK, library, generated and
  ambiguous types stay in `unresolved_refs`. The four kinds are kept in the full symbol graph
  and fenced out of the dependency views the metrics run on, so they do not change the signal
  by themselves.

### Changed

- **Java imports bind to the type they name.** A single-type import binds to the class and a
  static import to the member; a wildcard import brings its package's (or type's) members into
  scope; and a reference to a type in the file's own package binds without an import. A type declared under one name in both `src/main` and
  `src/test` stays unbound. On the reference estate Java imports went from 18 to 4,628 bound
  of 25,132; the rest are external, generated or in another member.
- **Spring routes built from string constants are promoted, or counted.** A mapping path
  written as `"/users/{" + USER_ID + "}"`, with `USER_ID` a `static final String` of the same
  type, of an interface, or of a type in the same member reached through `import static` or
  `Type.NAME`, folds to one path and becomes a `route` node like a written one. A path that
  cannot be folded (a method call, a non-final field, a wildcard static import, a constant in
  another member or a library) promotes no route and is counted in `routes_not_composed`
  instead of being dropped silently. Kotlin non-literal paths are not counted yet. On the
  reference estate all 16 concatenated sites are promoted, and `resolved_cross_service_edges`
  rose from 65 to 70.
- **A typed Java call binds through its receiver's class and that class's superclasses.**
  `service.send()` on a receiver the file declares binds to that class's `send()`. If the
  class does not declare it, the call binds to the one `send()` of its nearest superclass in
  the repository. `super.m()` and an inherited `m()` bind the same way. None of these binds:
  a method overloaded on one class, a JDK or library type, a type another member declares,
  or a superclass outside the repository. The nearest class that declares the name decides,
  without reading arity or visibility. An overload split across a class and its superclass
  therefore binds the nearer one. A nested type a class inherits does not yet hide a
  same-package type of the same name, so a receiver typed with that name binds the
  same-package type's method.
- **`status` states why the rest stays unbound.** The Java row of `resolution_by_language`
  carries `call_residue`: the unbound calls, and how many stay unbound for each reason
  (`no-receiver-evidence`, `external-type`, `type-in-another-member`, `overload-ambiguous`,
  `type-ambiguous`, `supertype-unreached`). Outside a workspace, another member's type counts
  as `external-type`.
- **Java figures move on the first re-index, and the move is a correction.** A Java method
  that was called only through a typed or inherited call gains its first inbound `Calls` edge,
  so:
  - fan-in and fan-out rise;
  - `callers`, `callees` and `impact` answers grow;
  - class coupling rises;
  - methods reported dead only because their callers never bound stop being dead;
  - the signal score of a Java repository moves with all of these.

  Some figures fall instead. A typed call used to fall through to a same-named method in lexical
  scope, often the calling method itself: `delegate.write()` inside `write()` bound to that
  `write()`. Those edges were fabrications and are gone (355 on the reference estate: 324
  self-edges, 31 to a sibling method). A method "called" only through one loses fan-in, drops
  out of `callers` and `impact` answers, and can newly be reported dead.

  Rust, and every other language, is unchanged: this repository's own index is byte-identical.
- **Known limitation: a sync can leave a stale Java call edge until the next full `index`.**
  `logos sync` re-binds the calls a hierarchy edit moves, but it does not remove an edge the
  call bound before. This happens when a class in the middle of a hierarchy gains an override
  (the call then has an edge to both methods), drops its `extends`, or is deleted. A full
  `logos index` has no stale edge. The same holds for a method that gains an overload.

## [1.5.0] — 2026-09-28

### Added

- **Members that build against each other.** Indexing reads each member's Maven `pom.xml` and
  Gradle `build.gradle` / `build.gradle.kts` into member-local facts: the artifacts it produces
  and the ones it references, by kind (`parent`, `dependency`, `managed`, `bom-import`) and
  scope. An unresolved `${…}` coordinate is refused with a reason, never guessed; Gradle
  support is read but reported unexercised.
- **A `builds-against` relation across workspace members**, with its own headline
  `build_dependency_pairs` split by kind beside its denominator. An artifact produced by two
  members resolves to neither, and the collision is reported. A build dependency is never a
  runtime coupling: it never enters `resolved_cross_service_edges`, egress resolution, a
  provider bucket or a bridge edge.
- **`logos xservice build-deps`** (`--json`, `--repo <member>`) and its MCP twin list what each
  member builds against and what builds against it. `workspace status` gains a
  `build_dependency` section. The web service map gains a `build` edge layer behind a legend
  toggle that is off by default, with platform members collapsed, and a cross-context model
  hint that is never drawn as an edge.
- **A member can declare its kind** in `logos.workspace.toml`: `kind = "documentation"`,
  `"mock"` or `"platform"`. Documentation and mock members leave the contract-surface headline
  and `spec_conformance_ratio`, and their rows are reported apart with their count. A platform
  member's inbound build edges are counted apart. `workspace status` lists candidates for each
  and never classifies a member itself.

### Changed

- **Graph store schema version 22** (forward-only) adds the build-manifest facts. After
  upgrading, each member with build manifests reads *unread — build facts not yet extracted*
  until `logos index` or `logos health` runs in it; it is never reported as having no
  manifests. An older binary cannot open a store at version 22.

## [1.4.27] — 2026-09-27

### Added

- **A project README** with install, quick-start and configuration guides.
- **Homebrew install:** `brew install raffaelecamanzo/tap/logos`. The release pipeline now
  publishes the formula to the `raffaelecamanzo/homebrew-tap` tap on every tagged release.

### Changed

- **The Logos mark and wordmark in the web header are 1.4 times larger** (mark 28px →
  39px, wordmark `--text-lg` × 1.4).

## [1.4.26] — 2026-09-27

Sprint 79, hotfix round 2.

### Changed

- **The web header shows the Logos mark and wordmark only.** The "code intelligence"
  subtitle after the wordmark is removed.

## [1.4.25] — 2026-09-27

Sprint 79, hotfix round 1.

### Added

- **The chat can read documentation kept outside the project.** A new optional
  `[chat] read_roots` key (for example `read_roots = ["../logos-docs"]`) lets the
  Source-Reader read files reached through symlinks inside the project that point
  into the listed folders. `grep` and `glob` follow only those links. It is empty by
  default, so nothing changes unless a project opts in. The consent banner and the
  CHAT band name the extra folders, and changing them asks for consent again. A
  listed folder that does not exist fails the turn with an error naming it
  (NFR-SE-04 amended).

### Fixed

- **Mermaid shapes are no longer drawn solid black.** The strict content-security
  policy blocked Mermaid's own styles, so cylinders, class boxes and most
  sequence-diagram parts fell back to black, and self-calls drew as filled blobs.
  Those styles are now applied in a way the policy allows, without loosening it, in
  both the Chat and Wiki tabs.
- **Sequence diagrams with a `;` in a message or note render.** Mermaid treats `;` as
  the end of a statement and rejected the whole diagram, so the chat showed its
  source. The text is repaired to Mermaid's `#59;` escape before rendering; the
  Source view still shows exactly what the model wrote, and the chat's answer writer
  is told to avoid a bare `;` in diagram text.

## [1.4.24] — 2026-09-27

Sprint 79.

### Changed

- **The Chat tab uses the application's two-pane layout.** The conversation now sits
  inside the shared card with its red top edge, beside a history rail that runs the full
  height of the view, on the same rail track and column gap as the Wiki tab. The centred
  reading column that left a 172px gap beside the rail at 1600px (652px at 2560px) is gone.
  A full-size, iconed **+ New chat** stays pinned above the scrolling conversation list
  (CR-092).

### Added

- **A persistent CHAT status band.** After you acknowledge the consent banner, the band
  takes its place at the top of the view. It names the provider, endpoint host and model,
  and the turn's budget tree (tool calls, per-subagent cap, replans), and stays visible
  after the first message. A value you have not configured is shown as not configured,
  and the key never appears (CR-092, FR-UI-33).
- **Chat answer tables render as bordered blocks.** A table in an answer or an Activity
  step result uses the code block's border and radius, with an uppercase header over a
  stronger underline, hairline row rules and a row hover tint. A table wider than the
  column scrolls sideways inside its block instead of breaking cells mid-word (CR-094).

### Fixed

- **The Statistics "By surface" caption no longer claims dashboard activity is
  excluded.** It states the rule the figures apply: the tab's own `stats` request and the
  shell's `status` readout are excluded per event, and a graph query issued through the
  dashboard counts (CR-146).

## [1.4.23] — 2026-09-26

Sprint 78.

### Added

- **Telemetry records what a call answered, not only that it ran.** `callers`, `impact`,
  `precedent` and `affected` now record an outcome (`answered`, `empty`, `unresolved` or
  `failed`), and every usage cell in `logos stats --json` carries `answered_calls` and
  `classified_calls` beside `calls`. The payload computes no rate: divide `answered_calls` by
  `classified_calls`, never by `calls`. A cell with no classified call carries
  `outcome_absence: "none recorded"`. The workspace aggregate carries the same fields. The
  store migrates to schema v4 on first use; older events stay unclassified and are never
  back-filled, and 1.4.22 still reads a v4 store (CR-144, FR-OB-14).
- **Statistics tab: tool attribution by class.** A new card shows per-tool usage split by
  dev/`main` origin, grouped by tool class, with each cell's `N of M answered` or
  `none recorded`. The coverage limits (raw events only, retention, pre-origin-stamp data)
  are printed beside the figures (CR-091).
- **A census fails the build if a traced tool is neither classified nor excluded with a
  reason**, so a new tool cannot silently shrink the answered denominator.

### Fixed

- **The Wiki progress banner never overshoots.** When an auto-continued run surfaces work it
  did not start with, the denominator grows to the real scope, never decreases, and the
  banner says so (`scope grew from 5 as new work surfaced during the run`), instead of
  reporting `7/5` (CR-093).
- **The chat Activity fold is correct on replanned turns.** A later round's observation no
  longer marks an earlier round's step done, the fold groups each round's plan with its own
  steps, and a replan to zero steps keeps the earlier round visible (CR-090).
- **The Statistics notes no longer claim pre-origin-stamp data is CLI+MCP-only.** It came
  from every surface running at the time, including web and the watcher.

## [1.4.22] — 2026-09-25

Sprint 77, second hotfix round.

### Performance

- **A workspace's first load is about 5× faster.** Compiled tree-sitter queries are now shared
  across member engines in one process instead of being recompiled at every member start. On
  the 84-member reference workspace, the Workspace Dashboard's first load after `logos serve`
  starts dropped from ~46 s to ~9.4 s, with identical answers. A per-root
  `.logos/plugins/<language>/` override still compiles its own copy (ADR-04 amended).

## [1.4.21] — 2026-09-25

Sprint 77, hotfix round.

### Changed

- **A workspace `[wiki].model` is inherited.** Wiki generation resolves its model in this
  order: the member's own `[wiki].model`; else the workspace root's `[wiki].model`, but only
  while the member inherits the workspace `[chat]` policy; else the effective chat model. A
  member that declares its own `[chat] model` owns its endpoint and never receives the
  workspace wiki model. The Wiki tab and the workspace Config view state the same rule, and
  the config read-model carries it as `effective_wiki` (CR-145, ADR-67, FR-CF-07).
- **The manifest routes emit telemetry.** `GET /api/v1/workspace/manifest` and
  `POST /api/v1/workspace/manifest/save` book `config_read` / `config_write` events into the
  serve's telemetry store, like the workspace config routes.
- **A broken workspace config file is one click from its repair.** In workspace mode the
  member Chat and Config tabs' error state links the workspace Config view and names both
  roots the failure can come from.

Sprint 77, hotfix round.

### Changed

- **A workspace `[wiki].model` is inherited — by a member that inherits the workspace
  `[chat]` table.** Wiki generation now takes the member's own `[wiki].model`, else the
  workspace root's when the member declares no `[chat] model` and the workspace root
  declares one, else the effective chat model. A member that declares its own
  `[chat] model` owns its endpoint, where a model named for the workspace endpoint may not
  exist, so it never receives the workspace wiki model. Provider, endpoint and key are
  still the effective chat resolution's. This replaces 1.4.20's "documented as not read":
  the workspace Config view now states the condition instead of *Not inherited*, and the
  config read-model (`GET /api/v1/config`) carries an `effective_wiki` slice beside
  `effective_chat`, which the Wiki tab's readiness and consent disclosure read, so the tab
  names the model the run uses (CR-145, FR-CF-07, ADR-67).
- **The workspace manifest routes are counted in `logos stats`.** `GET` and
  `POST /api/v1/workspace/manifest[/save]` each book one `config_read` / `config_write`
  event, as the workspace config routes already did (FR-UI-38).

## [1.4.20] — 2026-09-25

Sprint 77: the workspace is configured from the app, and the chat answers cross-service
questions.

### Added

- **The workspace Config view.** In workspace mode the sidebar's Workspace section
  gains **Config** (`/workspace-config`), which edits `logos.workspace.toml` in the
  same typed-fields + raw-TOML grammar as the member Config tab: `[workspace]`,
  `[workspace.warm]` and the full `[governance]` family, shown beside the advisory
  findings. It says at the point of editing that workspace governance never moves a
  member's gated signal. A save is validated before anything is written. A document
  identical to disk writes nothing, and fields the manifest does not declare are never
  added. A manifest changed on disk since the view loaded it is **not overwritten
  silently**: the view shows the disk copy and lets you load it or overwrite it, and
  says which happened. New routes: `GET /api/v1/workspace/manifest` and
  `POST /api/v1/workspace/manifest/save` (CR-137, FR-UI-38).
- **Workspace chat settings in the same view.** A second group edits the workspace
  `[chat]` policy and the masked, write-only chat key at the workspace root, names both
  files, and states how members inherit each half (CR-145).
- **Cross-service tools in the chat.** In a workspace the chat's Graph-Navigator can call
  `xservice_route_providers`, `xservice_callers`, `xservice_impact` and
  `xservice_search`. Answers are qualified by member, and an empty answer over an
  unresolved residue is reported as unresolved, never as "none". A plain single repo's
  tool list is unchanged (CR-137, FR-WS-29).

### Changed

- **The workspace chat tier's read is the repair path, and its save is clobber-safe.**
  `GET /api/v1/workspace/config` now returns the literal `config.toml`, a load
  fingerprint and `parsed: null` when the file is broken (a `200`, not a `500`). A fault
  in either file is named by file and line/column only, and nothing from
  `secrets.toml` is ever echoed. `POST /api/v1/workspace/config/save` requires that
  fingerprint (`400` without it) and answers `409` with the disk copy when the file
  changed since the read.
- **The Chat tab's configure-first state links the workspace Config view** for the
  workspace half, instead of naming the file as text.
- **A `[wiki].model` at the workspace root is documented as not read** by any member;
  the workspace Config view labels it *Not inherited*.

## [1.4.19] — 2026-09-24

Sprint 76, hotfix round.

### Changed

- **A member's own API key is never sent to a workspace endpoint.** When a
  member inherits the workspace `[chat]` policy (it declares no `model`), the key
  now comes from the workspace root or not at all; the member's own key is
  withheld (`effective_chat.member_key_withheld`). The Chat tab, the member Config
  tab and a refused chat request say so, and setting a `[chat] model` on the
  member makes it use its own key. A workspace key still reaches an endpoint a
  member declares itself (CR-145, FR-WS-30, ADR-67).


## [1.4.18] — 2026-09-24

Sprint 76 — the chat works in a workspace, and its not-ready state says why.

### Added

- **Workspace-level chat configuration.** Declare `[chat]` (and `[wiki].model`)
  in `<workspace-root>/.logos/config.toml` and the chat API key in
  `<workspace-root>/.logos/secrets.toml` once, and every member that declares
  none inherits them. Inheritance is **per half** — the policy table and the key
  resolve independently — and the member wins wherever it declares; the policy
  half is atomic on `model`, so a member that sets its own model uses its own
  table entire. The Chat tab, the chat request path and wiki generation all read
  one resolution, so they cannot disagree (CR-145, FR-WS-30).
- **`GET /api/v1/workspace/config`, `POST /api/v1/workspace/config/save`,
  `POST /api/v1/workspace/config/secret`** — read and write the workspace tier
  with the existing validated, atomic, `0600` writers. Intent-guarded, `404`
  under a single-root server, no apply/reindex action, and no engine is started
  at the workspace root.
- The config read-model (`GET /api/v1/config`) carries an `effective_chat` slice
  — the resolved policy, the masked key and the origin of each half (`member`,
  `workspace`, `unset`) — **beside** the member's literal document, which is
  unchanged byte for byte. The member Config tab shows inherited values as a
  read-only note and never writes them back into the member's `config.toml`.

### Fixed

- **The Chat tab's not-ready state now says what it checked**: the root it
  inspected (the member by name in a workspace, "this repository" otherwise),
  which half is missing (model, key or both), and where any present half came
  from. Previously it said only that a provider was needed — on the reference
  workspace it was reporting on a member other than the one being configured. A
  refused chat request names the same facts.
- The Wiki tab's readiness and consent disclosure read the effective resolution,
  so a member inheriting the workspace key is offered generation instead of a
  configure-first state.
- The sidebar Service header is now a column, so its label no longer truncates
  to `Se…` beside a long member name; the Workspace section renders as one list.


## [1.4.17] — 2026-09-23

### Fixed

- **The cross-file call relation is no longer Rust-only.** A module specifier is
  now canonicalised as a *path* rather than through the member-path separator, so
  TypeScript, TSX, JavaScript and Go stop producing zero `Imports` edges, and the
  **imported** rung of the scope hierarchy now binds a call through an imported
  binding. `logos callers navItemsFor` returned a confident `total: 0` for a
  function called at `web/ui/src/shell/Sidebar.tsx:172`; it now returns its real
  callers. Capitalised JSX elements (`<RuleFindingsCard />`) are captured as
  calls, without admitting `<div>` or `<Nav.Item>`. Rust resolution is unchanged,
  asserted by hashing the sorted edge set rather than by comparing counts
  (CR-142).

### Added

- **`status` reports resolution coverage per language, with its denominator**
  (`resolution_by_language`). A language that binds nothing across a file
  boundary renders as a named state — `same-file-only`, `no-resolved-edges` or
  `no-references-recorded` — never as a bare `0` that would read as a
  measurement. A single global coverage figure cannot express a per-language
  zero, which is how the defect above survived (FR-RS-09).
- **Every relational answer states the resolution denominator it was computed
  over** (`resolution_denominator`), on `callers`, `callees`, `impact`,
  `impact-intersection`, `branch-overlap`, `affected` and `precedent` — present
  on empty *and* non-empty answers, as a typed field rather than prose. This
  separates *nothing depends on this* from *nothing could be resolved here*
  (FR-NV-14, CR-143).
- **A structural arm to the absence audit**: the relational result types are
  enumerated from their definitions, so a new answer type that omits the
  denominator fails the suite rather than shipping silently.
- The shipped guidance in the managed `CLAUDE.md` block, the MCP server
  instructions and `docs/howto/README.md` now state the language scope on which
  their relational claims hold, keyed on the denominator rather than on a
  hardcoded language list.

### Upgrade note

Existing stores need a full `logos index`, not a `sync` — a sync does not
re-extract unchanged files, so the new `Imports`/`Calls` edges would not appear.

## [1.4.16] — 2026-09-21

Sprint 74 — the workspace stops being one tab and becomes a scope the
application declares.

### Added

- **Every navigation entry declares the scope it answers for.** `scope`
  (`app` | `member`) is now a required field, replacing a path-prefix test over a
  hard-coded one-item list that let the member selector silently govern eleven
  views and not the twelfth. The sidebar renders a **Workspace** section and a
  **Service** section whose header carries the member selector, so the control
  sits inside the boundary it governs. Single-root rendering is unchanged.
- **Two shipped capabilities gain the HTTP surface they never had.**
  `GET /api/v1/workspace/reachability` and `GET /api/v1/workspace/check`
  serialise the existing `federation::reach` and `federation::governance`
  read-models — reachable from the CLI alone since they shipped. A CLI/HTTP
  field-for-field agreement test pins them to one read-model rather than to two
  fixtures that could drift.
- **The selected member is in the URL.** `?repo=<member>` scopes the SPA on first
  paint with no unscoped pre-pass, switching writes through history, and a
  `?repo=` naming a member the workspace does not have renders a state naming the
  members it does have — while **no** view renders any member's figures. The
  normalisation is one table compiled into both the server and the client, so the
  two spellings cannot drift.
- **Workspace Dashboard and Workspace Health**, two `app`-scoped views. Every
  ratio carries its denominator and its exclusion; a zero denominator renders the
  ratio absent rather than as a `0%` that reads like a measurement. No
  workspace-level score is invented and no per-member signal is aggregated.
- **Workspace Statistics**, summed over members and read **engine-free** — the
  view load leaves the resident-engine count unchanged. It states its member
  denominator and names every member whose telemetry could not be read, reserving
  "a lower bound" for members that *failed* a read and never for one that simply
  has no store yet. It omits `artifact_bindings` and latency percentiles by
  construction: both are `Engine`-bound, and the ceiling is the point.

### Fixed

- **The Dashboard Rule findings card no longer passes a vacuous check.** A
  contract authoring zero rules now renders the onboarding empty state instead of
  a green `PASS` badge over a check that evaluated nothing — the fourth and last
  surface still doing so. Violations still win: a finding raised by an always-on
  fold-in renders red even on a zero-rule contract.

## [1.4.15] — 2026-09-21

Sprint 73 — every readout carries the denominator that makes it checkable, and
the absence vocabulary is stated once.

### Fixed

- **The session-start readout no longer calls a vacuous run a clean check.** The
  `check_run` marker recorded a violation count and nothing about what produced
  it, so a run that evaluated **zero rules** — no contract (exit `4`), or `logos
  init`'s default contract authoring none (exit `0`, the ordinary state of a
  fresh project) — rendered as ``clean `logos check` ``. Migration 21 widens the
  marker with the evaluated set (`checked_rules`, `rules_present`) and the
  operation that wrote it; the readout now names four distinct states, states a
  clean result only beside its denominator (`0 of 12 rule(s) evaluated — clean`),
  renders a pre-migration marker as *evaluated set unknown*, and names no
  command. Additive, in-place, no re-index (S-437, CR-140).
- **`quality-report` no longer reports a populated graph as empty.** A store
  whose every function is a test computes a legitimately empty *production*
  scope; it was reported as `signal n/a (empty graph)` while `logos status` said
  `indexed: true` in the same breath. The readout now distinguishes the two and
  carries the figure that establishes it (S-432, CR-138).
- **The dashboard no longer renders an age it cannot establish.** A timestamp
  ahead of now was clamped and shown as `just now`; since `status.last_sync_at`
  is a file mtime, that is routine on copied trees and restored backups. Both
  degradations now use the CLI readout's own wording (S-433, CR-138).
- **The Health page no longer labels a superseded snapshot `current`.** A
  snapshot persisted before the graph was last re-indexed now drops the word and
  carries its own date. Detects disagreement, not currency — it over-reports by
  design, and an indeterminate ordering renders neither (S-436, CR-135 §3.2).
- **Logos's own wiki generation is no longer counted as a developer browsing the
  dashboard.** `POST /wiki/generate` inherited the `serve --ui` process surface;
  it now has a distinct override-only `wikigen` telemetry surface, separable from
  `web`, `mcp` and `chat` (S-435, CR-139, FR-OB-13).

### Changed

- **One absence taxonomy across the CLI, governance and SPA surfaces**, stated
  once in code and referenced rather than restated. Audited from source: 3
  non-conformant occurrences of 68, in 1 file, at 2026-09-20 — the other 65 were
  already conformant (S-434, CR-138).
- `logos stats`'s surface vocabulary is documented in full: four process
  surfaces (`cli`, `mcp`, `web`, `chat`) and three override-only ones
  (`watcher`, `shell`, `wikigen`).

## [1.4.14] — 2026-09-20

### Added
- **The governance readout dates its violations and can state a recorded clean check**
  (S-314, CR-096, FR-IN-07). `quality-report` reports the check run's age and `HEAD`, says
  when a finding was measured against a *different* tree, and states a clean check only
  from the recorded marker — absent a marker it says **no check has run**, never
  "0 violations". Two unusable clock readings degrade explicitly rather than being
  smoothed into "just now".
- **The Health page states staleness instead of implying currency** (S-422, CR-135,
  FR-UI-04). Its gate verdict and quality grid are built from **one** read of the last
  persisted snapshot, so a concurrent `scan` can no longer render two cards describing
  different generations. With a signal present over a graph that is no longer indexed,
  both cards render their figures **labelled**, dated and naming `logos index`.
- **The app header survives a narrow viewport** (S-317, CR-097, FR-UI-34). `Header.module.css`
  gains its first media queries and a progressive-disclosure order: the graph-state readout
  is dropped **whole** before the brand subtitle, never truncated, and the brand lockup,
  member selector and theme toggle survive to the narrowest supported width.

### Changed
- **The LOC roll-up distinguishes three cases instead of writing unconditionally**
  (S-421, CR-134, FR-IX-12). Write on persist, clear over an empty store, and **leave
  untouched while a previous graph survives** — so an index in which every candidate fails
  to load no longer reports `total_line_count: 0` beside a correct file count.
- **The shell's own status read is classified as non-usage telemetry** (S-316, CR-097,
  FR-OB-09). The app header's `status` request is registered as a self-referential
  read-model request at the adapter boundary, so rendering chrome stops counting as
  someone using the tool. `surface` is now required on both `spawn_blocking` hops, making
  an unclassified handler a build failure.
- **The promoted topic inventory keys on the committed configured value** (S-424, CR-136,
  FR-WS-27), closing the last tier of the ADR-52 one-classifier drift: one identify
  function is now called from both the intra-repo promotion pass and the federation
  bridge, so a publisher and a subscriber that spell one property differently meet on one
  topic and the coupling draws as a `publisher → topic → subscriber` hop.
- **The rust client-call candidacy gate is receiver-grained** (S-423, CR-128, FR-WS-08),
  following Java's and Go's rule shape: a verb-named call on a non-client receiver no
  longer promotes a reference merely because its file imports a client crate. Six of the
  ten arms declaring `http_client_detectors` now scope their receivers.

### Fixed
- **Reading the Health page could have destroyed the last recorded `check` result.** The
  write-free contract was guarded per-path — S-314's path on the violations table and
  marker, S-422's on `metric_snapshots` — and neither covered the union, which is the
  actual contract. Found at sprint review by restoring the defect: an inserted write left
  the suite green while every page load would have truncated `violations` and stamped a
  fabricated recorded-clean marker over it.


### Changed — BREAKING (exit code)
- **`logos workspace status` / `reachability` / `check` now exit 1 when a member
  could not be opened (CR-100, S-326, FR-WS-16, FR-CL-01).** These commands
  previously returned **0** no matter how many members failed to open: on a
  72-member workspace under the stock 256-descriptor macOS limit, 63 members
  failed and the command still succeeded, so the run passed in CI over a payload
  that was three-quarters missing. An unopenable member now names itself and
  moves the exit code.

  **Scripts that tolerated the old exit 0 will begin failing** — which is the
  point, but it is a behaviour change to plan for. What does *not* move the exit
  code: a member with no index yet (`warm_state: deferred` — it indexes lazily on
  first query), a member the command never needed to open (`open_state:
  not-attempted`), and a member whose engine was **evicted** to stay inside the
  workspace connection budget (`open_state: opened` — eviction reclaims a
  success, not a failure). A workspace-governance violation still never moves the
  exit code either; `workspace check` remains advisory.

### Changed — BREAKING (payload)
- **`bound_ratio` is retired from the coverage payload; the headline is
  `resolved_cross_service_edges` with `egress_resolution` beside it (CR-120,
  S-376, FR-WS-05, ADR-53, BR-51).** Re-measured on the 84-member reference
  workspace the bound-ratio read `0.287 (81 of 282 measured)` over a workspace
  whose caller→callee edge count was **zero**: every one of those 81 bound rows
  was an OpenAPI operation matched to a controller route, not a resolved call. It
  also moved *downward* (0.355 → 0.287) as broker instrumentation improved, which
  is the wrong direction for a headline. CR-111 had already made the figure
  legible; legibility was not the problem.

  **`bound_ratio`, `bound_ratio_measured` and `bound_ratio_summary` are no longer
  emitted** on any surface — `logos workspace status` human output and `--json`,
  the `workspace_status` and `workspace_reachability` MCP tools, the
  `/api/v1/workspace/status` endpoint and the web coverage view. A reader of the
  old key now gets a missing field and fails loudly rather than silently reading a
  figure that no longer means what it did. The three keys are accepted for one
  release as **deserialization aliases** (`logos_core::federation::SpecConformanceReading`),
  so a stored pre-change capture still parses. **That window closes with the
  release after the one carrying this entry** — the aliases, and the type holding
  them, are removed then. Anchored here rather than left as "one release", because
  an unanchored deprecation becomes permanent vocabulary.

  What replaces them:
  - `resolved_cross_service_edges` — cross-service edges resolved from a captured
    **invocation** (a caller→callee HTTP client call, a producer→consumer broker
    publish, a gRPC stub call). It counts *edges*, not sites: under the broker
    fan-out one publish binds every cross-member subscriber and the bridge emits
    one edge per subscriber.
  - `egress_resolution` — the rate at which captured egress *sites* resolve at
    all, over the invocation population's own denominator, with
    `egress_resolution_measured` beside it. **Absent** (`null`) when no egress
    site was captured, never a fabricated `1.0`.
  - `resolved_edges_summary` — both figures as one composed line, so no rendering
    can show the count without the rate (BR-51).
  - `spec_conformance_ratio` / `_measured` / `_summary` — the retired ratio's
    formula, unchanged, under the name of what it always measured: how far this
    workspace's *declarations* line up with its controllers. The composed line's
    wording is byte-identical to the old one, so a pre-change capture and a
    post-change one compare directly. CR-100's absent-on-zero-denominator
    guarantee and CR-111's denominator-disclosure duty carry over intact.

  Measured on the reference workspace 2026-09-09 and recorded as a durable
  baseline in `logos-core/tests/coverage_headline_baseline/`: **0 resolved
  cross-service edges, egress resolution 0.000 over 54 egress sites**, beside a
  pooled `bound: 81`. That baseline is labelled with the index generation that
  produced it (logos 1.4.7, pre-S-374) and carries the refresh procedure for the
  re-index that will move it.

### Changed
- **Cross-service coverage reports its `intake` on every row, and splits the
  classification counts by it (CR-120, S-377, FR-WS-05).** The headline
  `bound`/`ambiguous`/`unbound`/`no-provider` counters add two different claims
  together: a `contract-surface` reference is a *declared* endpoint matched to a
  controller, an `invocation` reference is a captured *call site*. On the
  84-member reference workspace the split is **81 contract-surface and 0
  invocation** bound rows — no outbound call site in the whole estate resolves —
  and a bare `bound: 81` reads as a healthy workspace.

  `intake` was previously serialized **only on a bound row**, so the invocation
  population's non-bound rows carried no population marker at all and the split
  was not recoverable from the payload. It is now present on **every** row, in
  every state, and is no longer optional. Read `bucket`/`state` for "did this row
  bind" — the presence of `intake` never meant that and now cannot be mistaken
  for it.

  New: `coverage.by_intake.contract_surface` and `coverage.by_intake.invocation`,
  each carrying the same four counts. The two **sum to** the four top-level
  counters, which the server now derives from the split — so a figure and its
  decomposition cannot disagree. Present on all four surfaces:
  `workspace status` human output and `--json`, the `workspace_status` MCP tool,
  and the web coverage view, which gains a *Coverage by intake* board beside its
  per-arm one (the arm axis cannot separate the two — an OpenAPI operation and an
  HTTP client call are both the `route` arm).

  No existing field changed meaning or value, and the unbound-reason taxonomy is
  untouched. Payload cost, measured on the reference workspace's shape: **+7.2%**
  — 311 202 → 333 619 bytes, `intake` on the 794 non-bound rows plus the summary
  block.

- **The HTTP client-call arm records the call sites it declines, so coverage
  reports them instead of losing them (CR-120, S-374, FR-WS-08 AC2).** The arm's
  normalizer returned a refusal reason and the caller discarded it with `.ok()`,
  so a declined call site left **no reference, no ledger row and no coverage
  entry** — an estate whose client paths are all composed at runtime read exactly
  like one with no outbound calls at all. FR-WS-08's second acceptance criterion
  already required such a path to "appear under a runtime-composition coverage
  reason", so this was a conformance failure, not a gap.

  A call whose path is not a static absolute literal now leaves one **keyless**
  `unresolved_refs` row — empty target, so no template is fabricated — which
  `workspace status` reports under `base-url-runtime`. That reason previously had
  **no production producer** anywhere in the tree; it now has one, and
  `impl From<ClientCallRefusal> for UnboundReason` is no longer dead. The row is
  inert to binding by construction: it promotes no node, keys no topic, and
  `route_key` refuses an empty `"METHOD /template"`, so it can never become an
  edge. One row per **declaration** (the ledger's own identity ignores `line`),
  stable across re-syncs. The recorder is the one the broker arm has used since
  S-370, generalized rather than copied — the two arms now share
  `extract::config::refs::record_refusals`.

  **This makes the reported numbers worse, and that is the correction working.**
  Measured read-only over the 84-member reference workspace: **115 production
  rows** (94 Java, 20 Go, 1 Python) and 16 test-tree rows, **131 in total**,
  against a prior count of zero. All 94 Java sites refuse; the workspace-wide
  reference count stays 1. Full reconciliation, including why 115 agreeing with
  CR-120's ~111 is two different measurements rather than one confirmed twice, in
  `logos-core/tests/operand_resolvability/client_call_refusal_finding.txt`.

  Two populations stay invisible and are stated rather than implied: a call the
  language's `invocations` query never matched (every stated capture ceiling — a
  verb-suffixed `RestTemplate` method, OpenFeign, a receiver the S-375 rule
  declines) is refused before a call site exists, so it carries no reason and
  only a query change can reach it; and `path-not-composed`, which needs a
  non-keyless row and is out of this increment's scope — zero on Java by S-355's
  recorded evidence, and bounded rather than measured workspace-wide.

- **The NFR-PE-05 cold-start budget is re-derived 500 → 600 ms and now enumerates
  all six phases (CR-116, S-368, S-369).** The requirement enumerated three
  phases — embedded `plugin.toml` parse, `LanguageRegistry` construction, query
  compilation — while both tests citing it measured something else, and neither
  measured its enumeration. Per-phase instrumentation over eight fresh-process
  cold starts settled it: the three enumerated phases came in at mean 440.1 /
  **max 457.5 ms**, inside the old 500 ms budget in every sample, so **this was
  never a performance breach**. The ~67 ms excess was store open, schema
  migration, pool startup and incidental cold-path work the requirement never
  claimed to bound.

  A user waiting for a ready engine waits for the store too, so the requirement
  now enumerates all six phases and bounds their **total** wall time — the whole
  path to a ready engine — at **≤ 600 ms**, re-derived from the measured full
  total: p90 528.4 / max 539.8 ms, leaving 71.6 ms and 60.2 ms of margin (11.9%
  and 10.0% of the budget), rather than from the old target plus a margin. `LOGOS_PERF_TOLERANCE`'s default stays **1.0**; no cost
  regressed, and no per-phase sub-budgets exist, so no subset of the six may be
  gated as NFR-PE-05 conformance. The 200 ms `Runtime::open` guard, which
  asserted over exactly the phases the old requirement *excluded*, keeps its band
  but drops the citation and is renamed
  `runtime_open_stays_within_its_store_and_pool_regression_band`.

- **`Engine::start` runs one `git rev-parse --git-common-dir` on the DB-less
  worktree path instead of two (CR-116 §9.5, S-369).** The graph-store seed and
  the governance-contract seed each resolved the primary checkout with their own
  identical subprocess. They now share one resolution, via the new
  `workspace::seed_source_from_primary`. Waste removal on a budgeted cold path,
  not budget-chasing: the saving is headroom, and the 600 ms figure above is
  derived from the measurement *before* it. Measured effect on the phase that
  absorbs it (`ColdStartPhases::other`), medians over 10 fresh-process samples
  per run — medians because the documented cross-process ramp-up effect puts
  the occasional 1 s outlier in the mean: **32.1 / 32.1 ms before** (2 runs) →
  **24.2 / 24.2 / 22.5 / 24.4 / 23.5 ms after** (5 runs), a saving of ~8 ms,
  ~25% of the phase. The **full** cold start is *not* measurably faster: query
  compilation dominates it at ~440 ms and drifts ±25 ms between runs on the same
  host, which swamps an 8 ms saving. That is the expected outcome — the point was
  to stop paying twice for one answer.

### Added
- **Named, caused, degraded-member reporting in `workspace status` (S-326,
  FR-WS-16).** Each member row gains an `open_state` — `opened` /
  `not-attempted` / `degraded` — on an axis **separate** from the existing
  `warm_state`: the first is about whether the store could be opened, the second
  about whether it holds an index, and an un-indexed member that opens perfectly
  well is `deferred` / `opened`. A degraded row carries `degraded_reason` and,
  where the diagnostic identifies one, `degraded_cause`:
  `host-resource-limit` (the store is present and intact; the process ran out of
  file descriptors — raise `ulimit -n`, do **not** re-index) or
  `store-obstructed` (something that is not a regular file occupies
  `.logos/logos.db` — clear that path, then re-index). The previous message named
  only SQLite's "unable to open database file", which reads as a corrupt store
  and sent readers to the wrong remedy. A failure whose store file is merely
  *missing* deliberately claims **no** cause: the store is created on open, so an
  absent file at failure time is equally consistent with descriptor exhaustion
  during creation, and a guessed "go re-index" would relocate the very
  misdiagnosis this removes. `degraded_diagnostic` always carries the verbatim
  engine error beside the classified sentence, so classifying never destroys the
  evidence it read.
  The payload gains a `degraded_rollup` naming every unopenable member, folded
  into the same member table as the warm roll-up rather than a competing one, and
  a human-readable warning naming them goes to stderr so `--json` stdout stays
  machine-clean. The existing per-member `error` field is unchanged.

### Fixed — BREAKING (payload shape)
- **`coverage.bound_ratio` is now reported ABSENT instead of a fabricated `1.0`
  when nothing was measured (CR-100, S-326, FR-WS-05, NFR-CC-04).** A zero
  `bound + ambiguous + unbound` denominator used to serialise as a perfect
  score, so the observed partial workspace reported `bound: 0` beside
  `bound_ratio: 1.0` — a confident measurement invented from no evidence. The
  field is now omitted (`null` in `--json`) in that case, in `workspace status`,
  `workspace reachability`'s coverage rider, and the `/api/v1/workspace/*` web
  payloads alike. A consumer reading it as a number must handle absence; the web
  UI renders "bound ratio not measured" rather than an empty or full bar.
- **The coverage summary states how much of the workspace it covers (S-326,
  FR-WS-16, NFR-CC-04).** `coverage.members_read` / `members_total` /
  `covers_all_members` are new: a summary computed over a partially-opened
  workspace is now marked as covering fewer than all members instead of reading
  as a complete picture.

## [1.4.13] — 2026-09-18

Sprint 71: the last cross-service arm agrees with itself, and three readouts start reporting state they can establish (CR-132, CR-133).

## [1.4.12] — 2026-09-17

Sprint 70: Kafka Streams coupling becomes visible, and the two gates that decide the config-declared intake are measured (CR-131).

## [1.4.11] — 2026-09-14

Sprint 69: the capture reaches the shapes real estates write, and every cross-service figure agrees with its own payload (CR-123..CR-127).

## [1.4.10] — 2026-09-13

Sprint 68: committed-configuration evidence is wired into the production extract pass, and the workspace surface stops repeating itself.

## [1.4.9] — 2026-09-12

Sprint 67: committed configuration becomes cross-service evidence, and both blocking gates return a verdict (CR-121).

## [1.4.8] — 2026-09-09

Sprint 66: the invocation arms report their own refusals, and the headline stops flattering them (CR-120).

## [1.4.7] — 2026-09-07

Sprint 65: the gating measurements for the blocked change requests, plus the capture work not waiting on them.

## [1.4.6] — 2026-09-06

Sprint 64: `logos check` over an absent rules contract exits 4 instead of passing vacuously, seeded worktrees carry the contract, and the graph is offered at scoping time (CR-112..CR-114).

## [1.4.5] — 2026-09-05

Sprints 61–63: bounded workspace index/warm resources, JVM Spring route binding, a truthful multi-repo workspace from first contact to steady state, and visible cross-service REST coupling (CR-098..CR-102, CR-108..CR-111). 1.4.4 was a dev-only build.

## [1.4.3] — 2026-08-06

The session-start quality readout and the write-free `logos quality-report` (CR-095); four RustSec advisories cleared. 1.4.2 was never released.

## [1.4.1] — 2026-07-26

Sprint 60: chat UI refresh — assistant-ui column, foldable activity, Mermaid viewer (CR-089).

## [1.4.0] — 2026-07-25

Sprint 59: multi-thread chat history, streaming answers, and metric-semantics corrections (CR-053).

## [1.3.0] — 2026-07-23

Sprint 58: hardened CR-061 app-wide reachability and broker coupling, and dashboard LOC figures (CR-084).

## [1.2.3] — 2026-07-12

Sprint 57: CR-061 completion — broker topics, reachability, governance, workspace UI.

## [1.2.2] — 2026-07-11

Sprint 56: CR-061 cross-service invocation arms.

## [1.2.1] — 2026-07-11

Sprint 55: CR-061 federation surface.

## [1.2.0] — 2026-07-11

Sprint 54: CR-061 multi-repo workspace federation foundation.

## [1.1.1] — 2026-07-10

Sprint 53: the Quadrant view and static test-gaps tool are dropped, and Rule findings are promoted (CR-079).

## [1.1.0] — 2026-07-10

Sprint 52: the web dashboard joins the default build, with the agents egress carve-out (CR-078).

## [1.0.7] — 2026-07-09

**Sub-second `serve` cold start (CR-077).** The `serve --mcp` filesystem watcher
no longer pre-walks build-output trees at registration time, restoring the
NFR-PE-05 cold-start budget. No schema, reconcile-contract, or public-interface
change; the quality signal is unchanged (delta 0).

### Fixed
- **Watcher registration prunes ignored directories (CR-077, S-285, NFR-PE-05,
  FR-SY-04, FR-SY-11, ADR-48).** `notify-debouncer-full`'s file-ID cache used to
  seed itself by walking the *entire physical tree* on `.watch()` — `target/`
  included (measured ~1.2M entries) — before the server could answer its first MCP
  request, a ~58–93 s stall on every `serve` boot. A custom `PrunedFileIdCache` now
  routes that registration-time seed walk through the same `AdmissionAuthority`
  predicate the full `index` walk and event-time `classify` already share, so it
  visits only admitted source directories. Cold start returns to sub-second
  (measured ~1.7 s cold / ~0.5 s warm on the ~221k-LOC dogfood repo, was ~58–93 s).
  Directory descent is name-pruned (`target`, `node_modules`, `dist`, `build`,
  `vendor`, `.git`, the feedback-loop set, and `[semantics].ignored_dirs`); leaf
  files are gated through the admission authority for full-walk parity; the walk
  degrades to a name-only prune when no authority is available. Rename tracking for
  admitted source paths and event-time `classify` are unchanged — the watcher stays
  best-effort and non-load-bearing for correctness (FR-SY-06, ADR-11).

## [1.0.6] — 2026-07-09

**Reachability & metric-scope precision (CR-073 + CR-074 + CR-075 + CR-076).**
Four independent graph-precision fixes closing standing CRs, all monotonic and
never-fabricate. No schema or reconcile-contract change; the signal-vs-baseline
gate shifts by design (recovered coupling + production-scoped metric values) and is
re-blessed at release via `logos gate --save`.

### Added
- **Optional production-scope filter for the hotspot surface (CR-076, FR-GH-06,
  FR-CV-07).** `logos hotspots --production-scope` (and the MCP `hotspots` tool's
  `production_scope` arg + the web Files & Risk view's "Production files only"
  toggle) drops whole test files (`tests.rs`/`*_tests.rs`/`tests/`) from the
  candidate set *before* ranking, so the `--untested` board surfaces production
  files instead of test files. Opt-in and gate-immune (BR-26): the default board is
  byte-identical and toggling never moves a gated signal. CLI/MCP/web return
  identical rankings for the same state (NFR-CC-01).

### Fixed
- **Trait-object dynamic-dispatch reachability (CR-073 / CR-068 Part C, FR-RS-08,
  ADR-39).** A `&dyn Trait` receiver-method call now fans out to the trait's default
  body ∪ its concrete workspace impls via real `Calls` edges when the receiver is a
  *provable* trait object (explicit `&dyn T`/`Box<dyn T>`/`Arc<dyn T + …>` binding),
  and the dispatch live-rooting pass now roots trait-default bodies. The six
  `LanguagePlugin` trait methods reached only through `dyn` dispatch stop reading as
  dead (dead-callable census down); the CR-066 receiver-method guard (FR-RS-06) is
  not loosened and never-fabricate (NFR-RA-05) is preserved — a non-provable receiver
  is an honest miss, not a guess. A new (previously-unemitted) `Implements` edge is
  fenced out of every metric subgraph, so only the intended dead-code recovery moves.
- **Redundancy budgets production-scoped (CR-074, FR-QM-08).** `check_redundancy`'s
  `max_dead`/`max_duplicates` budgets now exclude `is_test` functions via the shared
  `test_node_ids` set, matching the Redundancy metric (FR-QM-05) and the sibling
  fan/structural budgets. `max_dead` is behaviour-preserved; `max_duplicates` drops
  by the excluded test-duplicate count.
- **Plural Rust test-file conventions recognized in `is_test` detection (CR-075,
  FR-AN-05).** The shared `is_test_path` helper now matches bare `tests.rs` and the
  snake_case `*_tests.rs` suffix (it already matched CamelCase `*Tests`). Non-`#[test]`
  helpers in those files are now correctly `is_test=true`, removing test-code
  contamination from every production-scoped metric via the single shared column; all
  consumers correct in lock-step with no per-consumer edits.

## [1.0.5] — 2026-07-08

**Graph-precision & config-surfacing cleanup (CR-071 + CR-068 Part B + CR-067).**
Three independent precision/UX fixes closing standing CRs, all additive/read-model.
No schema, reconcile-contract, or gate-baseline change.

### Added
- **Config parameter defaults surfaced in the web Config editor (CR-067, FR-UI-12).**
  `GET /api/v1/config` gained a read-model `defaults` projection — code-sourced and
  computed independently of the live documents. The Config editor now renders a
  `Default: …` hint beside every `config.toml`/`[metric_thresholds]` field and
  `unset → not enforced · Recommended: …` beside every `[constraints]` field. The
  save/apply/validate path is byte-identical, and the chat API key is never present
  in the projection (NFR-SE-07).

### Fixed
- **Sanctioned external-docs symlink now followed on git-ignoring checkouts (CR-071,
  FR-IX-10, ADR-59).** Resolves the CR-069/1.0.4 known limitation: the discovery walk
  built its `WalkBuilder` with git-ignore filtering upstream of the `.swe-skills`
  follow-branch, so a repo whose `docs/{specs,planning,requests}` symlinks are
  git-ignored indexed **zero** doc nodes. A dedicated git-ignore-bypassing detection
  pass — confined to the documentation subtree, one-hop, containment-gated, sanctioned
  root only — now follows the symlink and indexes the docs behind it. Admission parity
  (`admits_path`, ADR-48) mirrors the carve-out so `doctor`'s admission tripwire does
  not flag the freshly-indexed docs as drift. Source-code symlinks are still skipped
  wholesale; an escaping/unsanctioned target is refused, not followed.
- **Associated-function `Method` kinding and bare-path binder tie-break (CR-068 Part B,
  FR-EX-05, FR-RS-07, ADR-39).** Rust `impl`-nested associated functions are now kinded
  `NodeKind::Method` (distinct from free `NodeKind::Function`) at emission — symbol IDs
  and ordinals byte-identical (NFR-RA-06). In the binder, a single-segment bare-path
  call now prefers a free function over same-named associated methods (a monotonic
  tie-break, never a drop; the full callable set stands when no free function exists).
  On the Rust dogfood this recovers the `graph_store/mod.rs` `insert_node`/`insert_edge`/
  `upsert_symbol` cluster: resolved `Calls` edges **3694 → 3716 (+22)** with **0 lost**
  and **0** live callable turned dead (BR-38 monotonic). Receiver-method calls and the
  CR-066 unique-name fallback are untouched.

### Added (diagnostics)
- **`doctor --json` `doc_symlink_warnings` (CR-071, FR-IX-11).** A new advisory array
  naming any documentation directory-symlink that exists under the doc-include set but
  ended up unindexed (no sanctioned root, or target escapes containment). Purely
  diagnostic — it never flips `ok` or changes the exit status. `index`/`sync` fold the
  same drops into their warnings.

## [1.0.4] — 2026-07-08

**Binding precision & discovery fidelity (CR-068 + CR-069 + CR-070).** Continues the
measurement-precision arc into *binding & discovery* precision, with the same
never-fabricate, monotonic, honest discipline. No schema, reconcile-contract, or
gate-baseline change.

### Added
- **Function-pointer handoffs recognized as live roots (CR-068 Part A, FR-AN-01,
  ADR-39).** The framework-dispatch pass now recognizes axum function-pointer
  handoffs — `.fallback(fn)`, `middleware::from_fn(fn)` / `from_fn_with_state(_, fn)`,
  and every method-router handler inside `route(path, get(fn)|post(fn)|…)` including
  chained setters (`get(a).post(b)`) — as live roots via the existing `RoutesTo`
  self-marker. A handler name is bound only to the one same-file callable of that
  name (exactly-one-or-nothing), so nothing is fabricated (NFR-RA-05). On the Rust
  dogfood this drops false-positive dead functions **61 → 49** (12 `web/src/lib.rs`
  handlers/guards/fallbacks recovered) with **zero** previously-resolved `Calls`
  edges lost and **no** live function turned dead (BR-38 monotonic); a full re-index
  is required to observe the new liveness (an incremental `sync` over unchanged files
  keeps the prior markers). The `name_matcher` is untouched (the CR-066 fallback is
  not loosened).

### Removed
- **PostToolUse wiki-augmentation hook retired (CR-070, FR-WK-14).** The advisory
  augmentation hook — which surfaced the `wiki generate` work-list to the connected
  agent on every tool call — is deleted from the binary (`AUGMENT_SPEC`, the augment
  script/constants, and the augment `materialize()` entry point are gone). `logos
  init -i` and `logos wiki hook --emit [--force]` now install/emit only the SessionEnd
  quality-report hook (FR-IN-07); `wiki hook --emit --json` consequently returns a
  single summary object rather than a two-element array. The deterministic `wiki
  generate` queue and the `ui`-gated in-process generator (FR-WK-18) are unchanged.

### Known limitations
- **External docs-root symlink following (CR-069, FR-IX-10) landed but is inert on
  checkouts that git-ignore the doc symlinks.** The discovery carve-out that follows
  a `.swe-skills`-sanctioned docs symlink is correct in isolation, but the walk's
  git-ignore/exclude filtering runs upstream of it, so on a repo whose `docs/{specs,
  requests}` symlinks are git-ignored the doc nodes are not indexed. Resolving whether
  a sanctioned docs root should override its own git-ignore is a discovery-contract
  decision deferred to a follow-up CR (amending FR-IX-10 / ADR-59 / NFR-SE-04).

## [1.0.3] — 2026-07-07

**Graph measurement precision (CR-065 + CR-066).** Two core graph measurements
are re-based to reflect genuine structure rather than measurement artifacts, with
no change to the tool surface.

### Changed
- **Module-grain, production-scoped coupling budgets (CR-065, FR-GV-11, FR-QM-08).**
  `check_coupling`'s `max_fan_in` / `max_fan_out` now count each *module's* distinct
  neighbouring modules over the canonical dependency view (reusing the module rollup
  that backs `logos dsm`), excluding `is_test` nodes before the rollup. A shared
  helper called from many symbols in one module counts that module once, not once
  per call site — so the budget flags genuine cross-module coupling, not the name
  popularity of standard-library method names. Deterministic, module-key-ordered
  output (NFR-RA-06); the coupling budget stays a `check_rules`-only budget,
  orthogonal to the quality-metric gate signal (ADR-21).
- **Receiver-unqualified method binding no longer fabricates `Calls` edges (CR-066,
  FR-RS-06, FR-RS-03, NFR-RA-05).** A bare `x.f()` method call now resolves only on
  genuine lexical/module scope evidence; the workspace unique-name and suffix
  fallbacks are gated off for the receiver-method form, so a `.map()`/`.collect()`/
  `.join()`/`.path()` with no in-scope target stays in `unresolved_refs` and retries
  on sync instead of binding to a same-named callable in an unrelated module. Removes
  the ~29.5% of `Calls` edges that funnelled into ~15 std-method-named targets
  (~1065 edges / −21.3% on the dogfood self-graph). Path-qualified and typed calls,
  and free-call resolution across C++/Java/Kotlin/Scala/C#, are unchanged. The
  downstream dead-code/cycles signal shift is re-blessed deliberately at release per
  FR-GV-16, with CR-066 as the accepting authority.

## [1.0.1] — 2026-07-06

**Trustworthy wiki reframe (CR-062).** The source wiki gains a third tier: the
binary now *presents* the project's authored `docs/specs/**` and `docs/howto/**`
verbatim instead of paraphrasing them with a model, reserving LLM inference for
the Summary/Overview tier alone. This retires the per-file wiki pages that added
volume without traceable provenance, so the corpus converges on three canonical
tiers (extracted / presented / generated) — see [ADR-57].

### Added
- **Deterministic presented tier — `logos wiki materialize` (FR-WK-20, ADR-57).**
  A pure, offline, idempotent command assembles one wiki page per Design/Specs
  category (section-per-source-file) directly from the authored SRS sources,
  labelled `generator = logos:doc-present` ("Presented verbatim … Not
  model-generated") — copied verbatim, never paraphrased. No LLM, no network.
  `materialize` has a payload-identical MCP twin (`wiki_materialize`), bringing
  the wiki tool set to five (`write`/`read`/`search`/`status`/`materialize`).
- **User Guide tier (FR-WK-23).** `wiki materialize` also presents one
  `guide/<name>` page per `docs/howto/*.md` (e.g. `guide/overview` ← `README.md`),
  so the rendered manual is copied verbatim from source rather than regenerated.
- **SRS-mode bimodal generation gate (FR-WK-21).** When the project ships an SRS
  (`docs/specs/architecture.md` + a requirement), the Design/Specs and User
  Guide pages are presented and the connected agent generates only the
  Summary/Overview tier; otherwise it infers the full set from the code graph.

### Changed
- **User-needs-aware Overview generation (FR-WK-24).** The generated
  Summary/Overview pages are grounded in `README.md` + `docs/howto/**` and
  prompted to read as user-facing (goals/workflows), not a symbol tour.

### Removed
- **Per-file wiki pages retired (FR-WK-22).** The `files/*` pages are excluded
  from the generation work-list and from the status page count, and a
  reconciliation sweep purges unreachable orphan pages so a previously bloated
  corpus self-prunes to the three canonical tiers.

## [1.0.0] — 2026-07-05

**First stable release.** Logos reaches 1.0.0 with a trustworthy source-wiki
generation pipeline (Sprint 44 — CR-059) on top of the structural code-graph,
architecture-quality gate, agentic chat, and single-binary web UI shipped across
the 0.x line. The command surface, `.logos/` store layout, MCP tool set, and CLI
JSON contracts are now considered stable under [Semantic
Versioning](https://semver.org/).

### Added
- **Grounded wiki generation (FR-WK-18, ADR-51).** Each queue item's grounding
  content — the referenced `docs/` source read, or a token-bounded code-graph
  digest as fallback — is resolved in-binary and injected into the synthesis
  prompt, so the tool-less generator writes page prose only from supplied
  context instead of hallucinating or emitting planning noise. Generation stays
  offline; no network egress on the grounding path (NFR-SE-01).
- **Write-path content-validity guard (FR-WK-19, NFR-CC-04).** The shared
  `wiki write` façade — CLI stdin/`--body-file` and the in-process generator
  alike — rejects a body that is agent-noise rather than page prose (a
  `<tool_call>` token, an `Error:`/`cmd:` transcript, a first-person
  planning/refusal preamble, no Markdown heading, or below a minimum length). A
  rejected write leaves the store byte-identical and is reported as an honest
  per-page failure, never a fabricated page. The two noise-content signatures
  are scanned with fenced code blocks stripped, so a page that legitimately
  *quotes* one of those patterns inside a ` ``` `/`~~~` fence is accepted.
- **Wiki run-state legibility (S-239).** The web Wiki tab shows cumulative
  "N of M" progress and a visible per-page synthesis-timeout hint; a halted run
  reads "Generation halted" rather than a stale "Generating…" or a false
  "complete", and a drained work-list launches no redundant run.

### Changed
- **Corpus regeneration under the fixed pipeline (S-238).** Regeneration drives
  grounding + guard together and purges orphaned pages, so a previously
  corrupted corpus is replaced with grounded, guard-checked prose.
- **Statistics "Dev vs main" card aggregates worktree branches (FR-OB-08).** The
  origin split (`calls_by_origin`, surfaced in the web Statistics tab and `logos stats`)
  now collapses every non-`main` origin into a single cumulative `"dev"` bucket, so the
  card is a two-way comparison of all development-increment work versus `main` — instead
  of one bar per (often stale) worktree branch, which added noise over a wide window. The
  stored per-event `origin` is unchanged; only the read-model aggregation changed.

## [0.14.0] — 2026-07-04

**Recoverable-fault degradation across the agent substrate (Sprint 43 — CR-060).**
A single recoverable subagent fault (a transient provider hiccup, a missing-path tool
error) no longer kills the whole chat turn. The runtime now recovers at three layers —
transparent provider retry, tool-errors-as-observations, and cross-step degradation —
so the turn continues and returns a best-effort grounded answer.

### Added
- **Provider-call retry with backoff (S-240).** A transparent `RetryingModel` decorator
  over rig's `CompletionModel`, wired into both provider constructors, retries retryable
  faults (transport, 429, 5xx, deserialization hiccups) with exponential backoff + jitter;
  `Auth` and other terminal errors are never retried, and exhaustion returns the original
  error. Two new `[chat]` keys — `max_provider_retries` (default `2`, `0` disables) and
  `provider_retry_base_ms` (default `200`, `0` rejected at load) — are inherited by the
  wiki-agent.
- **Tool errors as self-correcting observations (S-241).** In `run_tool_subagent`, tool
  errors and out-of-domain requests become model-visible `tool_result` observations so a
  subagent adapts instead of dying, bounded by a consecutive-error soft-close cap
  (`CloseReason::ToolErrors`) that closes well-formed with a `[bounded — …consecutive tool
  errors…]` marker; a success resets the streak.
- **Cross-step fault degradation (S-242).** A new `StepError::Unavailable` reclassifies the
  three recoverable roster fault sites; a step that stays unavailable after retries degrades
  to a `[unavailable — …]` observation and the turn continues, answering best-effort when any
  usable observation exists (and halting honestly when none do). Synthesizer/structural faults
  stay turn-fatal; sustained outage terminates via `max_replans`.

### Changed
- Recoverable subagent faults are now degradation events, not turn-fatal errors — extending the
  bounded-degradation posture established in 0.13.0 (CR-048) from budget exhaustion to provider
  and tool faults.

## [0.13.0] — 2026-07-04

**Bounded, graceful degradation of generative work (Sprint 42 — CR-048 + CR-044).**
The agent budget tree stops being a hard tripwire and becomes a soft, self-summarizing
bound; subagent preambles are budget-aware; wiki regeneration cadence is dampened.

### Changed
- **Soft per-subagent budget cap (S-181).** A subagent that reaches its per-subagent
  tool-call cap no longer hard-halts the turn — it closes well-formed, summarizes its
  findings tool-free, and returns a marked `[bounded — …cap…]` observation. Only the
  global `max_tool_calls` ceiling and `max_replans` hard-halt; on a hard halt the
  orchestrator returns a best-effort grounded answer over the scratchpad (or an honest
  bare halt when nothing was gathered) instead of an error.
- **Budget-aware subagent preambles (S-182).** Each tool-bearing subagent's preamble
  names its cap and running "calls remaining", steering it to prefer the
  breadth-efficient `context` tool; the preamble is rebuilt from the live budget at
  every model round, including the soft-close step.

### Added
- **`[wiki].revision_stale_threshold` (S-164).** A new config key (default `5`, min `1`,
  `0` rejected at load) dampens the re-queue cadence of anchorless prose wiki pages.
  `revision_stale_count` stays truthful; the regeneration queue stays a pure offline read.

## [0.12.1] — 2026-07-04

**Web-UI polish (dogfood fixes).** Three rendering/aesthetic fixes surfaced while
dogfooding 0.12.0; no behavior or API change.

### Fixed
- **Files & Risk section spacing.** `FilesView` returned bare fragments, so the
  hotspot Callout and the risk/ownership Cards stacked flush; it now uses the shared
  `.view` section-stack (`gap: var(--space-5)`) like the Coverage/Quadrant tabs.
- **Wiki search form.** The hand-rolled raw `<input>` (TOC-heading label voice, no
  label↔control spacing, browser-default ~20-char width) is replaced by the shared
  `TextField`: a real form label, `var(--space-2)` spacing, and a full-width input.
- **Mermaid diagram legibility.** Under the self-only CSP, Mermaid's injected style
  is stripped, leaving arrowhead markers on the SVG default black fill and edges at
  an uncontrolled weight. The external CSS now sets a light edge `stroke-width` and
  reliably re-colors the arrowhead markers to `--text-2`, so diagrams read cleanly.

## [0.12.0] — 2026-07-03

**Wiki generation, made usable end-to-end (CR-056).** The in-process wiki
generation run no longer floods its work-list with per-file pages, survives a
dropped SPA connection, and reports honest cumulative progress on re-attach.

### Changed
- **Pruned generation work-list (S-221).** `status()` / `structured_sections()` /
  `generation_queue()` no longer seed per-file `objectives` pages or unanchored
  File/Module entities — the cold-start queue collapses from ~1600 to dozens on
  this repo. Existing `wiki.db` pages are still served and refreshed on drift;
  determinism (NFR-RA-06) preserved.
- **Connection-resilient, auto-continuing run (S-222).** The run's lifetime is
  owned in app state rather than the SSE response body, so dropping the stream no
  longer aborts generation — it auto-continues across budget chunks until the
  work-list drains, bounded by a hard safety ceiling. A per-page synthesis timeout
  prevents a hung provider from orphaning the single-run lock.
- **Wiki-tab re-attach + cumulative progress (S-223).** Reopening the tab mid-run
  subscribes to the live run (exactly-once delivery) and shows a whole-run
  cumulative "N of M", not a per-chunk reset; reopening after completion reads
  "up to date" and starts no second run.

### Added
- **Typed `[wiki].model` field in the Config tab (S-224).** The wiki synthesis
  model is editable in the UI; a valid save round-trips through an atomic
  write-back, an invalid document is rejected inline with no partial write, and an
  absent `[wiki]` section renders blank rather than crashing.

## [0.11.0] — 2026-07-03

**Durable telemetry + Statistics tab (CR-058).** Logos tool usage is recorded in a
store that survives worktree teardown, and the dashboard surfaces it.

### Added
- **Durable, shared-primary telemetry store.** Tool invocations are persisted to a
  telemetry store rooted in the git common directory (surviving worktree teardown),
  stamped with an `origin`, and queryable via `logos stats`.
- **Statistics dashboard tab.** The web UI exposes recorded tool usage.

### Fixed
- **HF-1:** web-UI-originated activity is excluded from the Statistics read-model so
  the dashboard reflects agent/CLI usage rather than its own rendering.

## [0.10.0] — 2026-07-03

**Cold-index performance — measure-first de-serialization of the write path (CR-057).**
A full cold index is materially faster with byte-identical graph output. Nothing
about the produced graph changes; only how fast it is built.

### Added
- **Per-phase index instrumentation (S-225).** `logos --json index` now carries a
  `phases` object with per-phase wall-clock durations (`discover`, `load`, `extract`,
  `persist`, `resolve`, `framework`, `dispatch`, `annotate`), derived from the same
  `tracing` seam as the logs and reconciling to `total_ms`. A repeatable cold-index
  benchmark (`cold_index_phase_baseline`) records total + per-phase + peak RSS.

### Changed
- **Parallel annotation compute (S-229).** Near-clone clustering (the measured ~83%
  of the annotate phase) and the per-node verdict loop now run on the shared worker
  pool via keyspace-sharded pair counting — **annotate −54% (2.2×)**, byte-identical
  across every worker count, peak RSS held under the 1 GB ceiling.
- **Chunked Pass-1 persistence (S-226).** The per-file commit storm collapses into
  bounded write batches (≈958 → ≈4 transactions on this repo), single-writer
  invariant preserved.
- **Parallel file-load and discovery walk (S-228).** Read+hash and the directory
  walk fan out on the shared pool; order-deterministic, byte-identical.
- **Writer bulk-load pragmas (S-227).** The writer connection sets `cache_size` /
  `mmap_size` / `temp_store` for the index write window; the reader pool is untouched.

Overall cold-index total is down ~25% on a real repo, driven by S-229 and S-226.
Full analysis: `docs/perf/cold-index-0.10.0.md`.

## [0.9.8] — 2026-07-02

**Web-UI packaging hardening — no more silent white page (CR-049 follow-up).** A
`--features ui` binary built without a matching `npm run build` no longer serves a
blank page. This is a build/packaging robustness fix; the offline default binary is
unaffected.

### Fixed
- **Hash-free committed placeholder (`web/ui/dist/index.html`).** An earlier revision
  committed a real Vite build's `index.html` as the "placeholder", so its frozen
  `/assets/index-<hash>.js` reference never matched any fresh build — embedding it
  produced a `200` shell whose JS bundle `404`'d (a blank page). The placeholder is
  now a genuine hash-free stub that references no build artifacts and shows a visible
  "UI bundle not built — run `npm run build`" message, so it can never create the
  hash mismatch. This also fixes `web/tests/spa_shell.rs`'s
  `every_shell_referenced_asset_resolves_through_the_router`, which was red at HEAD.

### Added
- **Serve-time SPA consistency guard (`web`).** When the embedded shell references an
  `/assets/*.{js,css}` the binary does not embed, `/` (and the history fallback) now
  return a `503` self-describing diagnostic naming the missing assets and the rebuild
  recipe, instead of the silent white page. Detection is a pure ref-extractor over
  the embedded `index.html`; the happy path (consistent bundle) is unchanged.

## [0.9.7] — 2026-07-02

**Internalized wiki generation (CR-047).** Source-wiki prose generation moves
**in-process** onto the shared `rig` agent substrate the Chat tab already uses,
replacing the out-of-process headless `claude -p` SessionEnd autogen hook. The
default (offline) binary is unchanged and remains provably offline — the wiki
generator, like chat, compiles only under `--features ui`.

### Added
- **Dedicated `[wiki].model` config (S-176, FR-CF-07).** An optional `[wiki]`
  section selects a wiki-generation model distinct from `[chat].model`, inheriting
  `provider` / `base_url` / the `secrets.toml` key from `[chat]` (no separate wiki
  provider, endpoint, or secret). Omit it to fall back to the chat model.
- **In-process wiki-agent (S-177, FR-WK-18).** A new `ui`-gated `wiki-agent` crate:
  a single-purpose `rig` agent on the shared `agent-core` substrate that loops the
  deterministic `wiki generate` queue, uses the embedded `logos-wiki` skill body as
  its system prompt, and persists pages via the unchanged `wiki write` contract. No
  planner/subagent roster; a per-run budget bounds the pass.
- **Wiki-tab generation trigger (S-178, FR-WK-18, NFR-SE-07).** Opening the Wiki tab
  launches a background generation run under a single-run lock, renders existing
  pages immediately, and streams per-page refreshes over a same-origin SSE endpoint
  (`POST /wiki/generate`). The first outbound call in a session is gated by a
  first-use consent disclosure naming the configured endpoint; an unconfigured
  provider shows a configure-first state, not an error.

### Changed
- **Retired the headless `claude -p` wiki-autogen hook (S-179, FR-WK-16 → CR-047).**
  `logos init -i` no longer installs the SessionEnd autogen hook or its
  `.claude/settings.local.json` materialization. The embedded `logos-wiki` skill is
  now the wiki-agent's system prompt (single source of guidance) and is still
  materialized via `logos wiki skill --emit` for manual regeneration. The advisory
  PostToolUse augmentation hook is unchanged.

### Verified
- **Offline carve-out regression + UAT (S-180, UAT-WK-06, NFR-SE-01).** The default
  build links no HTTP client (no-networking-crate fitness test byte-identical); the
  full configured→open→consent→regenerate→stream→dual-axis-fresh flow is exercised
  end-to-end via the mock `CompletionModel` with zero real egress and a loopback bind.

## [0.9.6] — 2026-07-02

**Standalone quality integration (CR-055).** The full quality loop —
**freshen / enforce / report / bless** — is now a first-class capability any
adopter gets from `logos init`, decoupled from the swe-skills harness (ADR-49).
The existing `check` / `scan` / `gate` commands are unchanged; two new
`init`-installed triggers plus a documented CI recipe wire them into everyday
workflow.

### Added
- **Enforcing `pre-push` git gate (S-218, FR-IN-06).** `logos init --hooks` now
  installs a fourth, **blocking** git hook alongside the exit-0 freshness hooks:
  a `pre-push` gate that runs `logos check` and **propagates its non-zero exit**,
  so a rule / structural / admission / dead-code regression makes `git push` fail
  (exit 1) and names the offending contract. It bails **open** (exit 0) when the
  `logos` binary is absent — never a false block — carries the managed marker, and
  is bypassed by `git push --no-verify`.
- **Harness-agnostic Claude Code SessionEnd quality-report hook (S-219,
  FR-IN-07).** `logos init -i` now registers a non-blocking SessionEnd hook that
  runs `logos check` + `logos scan`, prints the current signal, the baseline
  signal, and any violations to the terminal, and **always exits 0** (never blocks
  session teardown). Disable it without uninstalling via the
  `LOGOS_QUALITY_REPORT_DISABLE=1` off-switch (mirroring the wiki-autogen hook).
- **CI recipe and workflow-integration docs (S-220).** New
  [`docs/howto/ci-integration.md`](docs/howto/ci-integration.md) documents the
  freshen / enforce / report / bless model and ships a copy-pasteable CI recipe —
  `logos check` as the enforcing build step, `logos scan --json` as the
  non-blocking signal report, and `logos gate --save` blessing a new baseline **at
  release only, never in the PR path**. Cross-linked from the how-to README,
  usage, and the `init` command reference; `error-handling.md` documents the
  `pre-push` exit-1 contract.

## [0.8.0] — 2026-06-27

**Agentic Chat.** A new `ui`-gated Chat tab brings an orchestrated LLM agent over
the code graph to the localhost dashboard (Sprint 30, CR-045 + CR-046).

### Added
- **Agentic chat orchestrator** — an LLM planner runs a plan→act→observe→replan
  loop over a fixed roster of four specialized subagents (Graph-Navigator,
  Governance-Analyst, Source-Reader, and a tool-less Synthesizer), each driven at
  the completion-model level through a least-privilege bounded tool dispatcher, all
  bounded by a budget tree (global tool-call ceiling / per-subagent cap / max
  replans) that halts honestly rather than fabricating an answer.
- **Chat tab UI** — a consent-gated composer that streams the plan, live
  subagent-activity chips, and the final answer over Server-Sent Events; the SSE
  rides the intent-guarded `POST /chat` (a `GET` `EventSource` cannot carry the
  CSRF intent header) under the unchanged self-only CSP, with a no-JS buffered
  fallback and Clear-history.
- **Token-by-token answer streaming** — the Synthesizer's answer now types out live,
  token by token (`answer_delta` SSE events), reconciling to the authoritative final
  answer when the turn completes.
- **`[chat]` configuration** — provider (Anthropic native / OpenAI-compatible,
  default OpenRouter), model, budget-tree params, and sampling, with the API key in
  a `0600` `secrets.toml` that is masked and never echoed. The Config tab gives the
  provider, model, and base_url their own typed controls (a provider select + a
  model input + a base_url input, patched into the validated raw-TOML candidate
  like every other typed field, in a full-width `[chat]` fieldset), so the settings
  that gate whether Chat is usable — and the endpoint it talks to — are discoverable
  rather than hidden in the raw pane; the remaining `[chat]` keys stay in the raw pane.
- **Multi-step agent memory** — per-thread scratchpad + working memory in
  `.logos/chat.db`, with the Synthesizer grounded on the persisted scratchpad.

### Security / carve-out
- The entire chat stack is gated behind the non-default `ui` feature: the default
  binary links **no** networking or LLM crate and stays byte-identical to 0.7.6
  (the `no_network_deps` fitness function and three further carve-out guards hold).
  The first and only outbound egress is the explicit, consent-gated chat turn.

## [0.2.0] — 2026-06-14

Language-breadth release — the first 0.x increment to carry new capability
rather than a pure dogfood re-pin. The default binary now indexes **twelve**
programming languages out of the box (up from five), and three resolution/config
correctness fixes land. The self-dogfood pin advances to this version.

- **Feature — seven more languages in the default build (CR-009).** Kotlin, C,
  C#, C++, Ruby, PHP, and Scala join the out-of-the-box code-language set, taking
  it from five to twelve (`logos languages` now lists 24 grammar rows including
  artifacts). Each ships as pure plugin data — one grammar crate, one feature
  line, one `plugins/<lang>/` descriptor — riding the existing plugin substrate
  and the load-time ABI assertion (ADR-09) without touching core extraction logic
  (NFR-MA-01). The stripped default binary stays within the NFR-PC-04 ≤ 50 MB
  budget. The NFR-PE-05 cold-start budget is revised 200 → 500 ms to reflect the
  larger grammar set compiled at registry load.

- **Fix — reconcile-purge demotes inbound references (CR-017 Defect A).** When a
  config change narrows the indexed set and purges a file, references from
  still-indexed code that resolved to the purged file are now correctly returned
  to unresolved instead of keeping a stale resolved row (NFR-RA-05 honesty).
  Surfaced by 0.1.2's incremental resolution, whose change-delta did not see the
  out-of-band purge.

- **Fix — OpenAPI operations bind to their framework routes (CR-017 Defect B).**
  An `ApiOperation` reference now resolves to the route node the framework pass
  promotes, producing the operation → route edges that previously came out empty
  on a no-op sync. Same incremental-resolution interaction as Defect A: the route
  is promoted after the resolve pass, so a focused re-resolve over the newly
  promoted names is now run.

- **Fix — the `languages` config field gates indexing (CR-017 Defect C).**
  Previously inert (it fed only the admission fingerprint), `languages` now
  restricts which code grammars are admitted: omitted or empty means all
  compiled-in languages (preserving the twelve-language default), a non-empty
  list narrows, and narrowing purges the dropped languages' files.

## [0.1.2] — 2026-06-13

Dogfooding bugfix release. Re-pins the self-dogfood binary. Two resolution-engine
performance fixes that, together with 0.1.1's watcher-exclusion fix, eliminate the
CPU melt that had forced dev-pane Logos off.

- **Fix — `serve --mcp` whole-ledger re-resolution under churn (CR-015).** The
  watcher-fired `Engine::sync` re-bound the *entire* reference ledger (~40k rows)
  on every sync via an all-core parallel pass, even when nothing relevant changed
  — N concurrent panes saturated the machine. Resolution is now **incremental**:
  a sync re-binds only the change-affected rows (the changed files' rows plus the
  untouched rows whose target token a change moved), and the watcher coalesces
  bursts via a settle window + rate-limit floor + staleness cap. A no-op sync on
  the self-graph drops ~152 s → ~1.6 s. Guarded by a sync≡reindex equivalence net.

- **Fix — cold `logos index` exponential glob resolution (CR-016).** The binder's
  `through_globs` resolved each glob import's own module path by re-entering
  `through_globs`, so a file with `G` glob imports did `O(G^8)` work per reference;
  a file with 14 `use super::*` imports drove a single bind to ~1.5e9 operations
  (~150 s) and, in parallel, pegged every core. A re-entrancy guard collapses this
  to `O(G)`. Cold self-index ~7 min → **3.8 s**, restoring the NFR-PE-02 budget.
  Edge output is byte-identical (verified by the equivalence net + exhaustive glob
  classification).

## [0.1.1] — 2026-06-12

Dogfooding bugfix release. Re-pins the self-dogfood binary.

- **Fix — `serve --mcp` watcher CPU storm.** The hosted filesystem watcher
  excluded only `.logos`/`.git`, so build-output churn under indexer-ignored
  directories (`target/`, `node_modules/`, `dist/`, `build/`, `vendor/`) flooded
  the debounced sync worker — a single `cargo build` drove `Engine::sync` across
  every core, and N concurrent dev panes saturated the machine. The watcher now
  drops events under the same `ignored_dirs` set the indexer prunes (unioned with
  the always-excluded internal dirs, so feedback-loop containment is not
  configurable away). The watched set now matches what indexing admits; real
  source edits still sync. Regression-tested in `logos-core/src/watch`.

## [0.1.0] — 2026-06-11

First tagged release and the baseline pin for self-dogfooding.

Logos at 0.1.0 is a single static binary providing structural code intelligence
for AI-assisted development: deterministic, offline, never-fabricating. Headless
CLI plus a stdio MCP server (`logos serve --mcp`).

Capabilities in this release:

- **Code graph** — multi-language extraction (Rust, Python, TypeScript, Go,
  Java, Kotlin, Swift, Ruby, PHP, C#, C/C++, Scala, …), SCIP-conformant data
  model, resolution, annotation, and the eight navigation tools
  (`search`/`query`/`context`/`callers`/`callees`/`impact`/`affected`/`explore`).
- **Quality metrics & governance** — full architecture-quality `scan`, the
  versioned-baseline `gate` (CI regression check), `check` against `rules.toml`,
  test-aware and production-scope metrics.
- **Extended analytics** — structural metrics and near-clone detection,
  git-history temporal metrics and `hotspots`, external `coverage` ingestion,
  `dsm`, `evolution`, `test-gaps`, `doc-gaps`.
- **Documentation graph** — markdown doc nodes, doc↔code link resolution, and
  doc-aware traceability (`implements`, `referencing-docs`).
- **Config & artifact graph layer** — an `artifact = true` plugin class over 10
  artifact grammars (YAML/JSON/TOML, Dockerfile/Makefile/Shell,
  Protobuf/GraphQL, Terraform/SQL) with content-sniffed OpenAPI promotion;
  metric-neutral by construction.
- **Setup & integration** — `logos init` (self-contained: `.logos/` config +
  rules, optional git hooks for automatic freshness, `.mcp.json` for any MCP
  host).

[0.2.0]: https://github.com/ — local tag `v0.2.0`
[0.1.2]: https://github.com/ — local tag `v0.1.2`
[0.1.1]: https://github.com/ — local tag `v0.1.1`
[0.1.0]: https://github.com/ — local tag `v0.1.0`
