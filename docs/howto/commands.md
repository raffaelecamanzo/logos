# Command Reference

All subcommands of the `logos` binary (42 top-level commands in the default
build, including the Sprint-55 `xservice` and `workspace` federation groups).
Every command accepts
the global flags `--project <PATH>`, `--json`, and `--quiet`; see
[usage.md](usage.md#global-flags-and-exit-codes) for those and for the
`0/1/2/3` exit-code contract.

| Command | Status | One-liner |
|---|---|---|
| [`init`](#init--i---hooks---workspace---yes---exclude-glob) | ✅ | Initialise `.logos/`, policy files, MCP host, and git hooks |
| [`index`](#index) | ✅ | Build or rebuild the full code-graph index |
| [`sync`](#sync-paths) | ✅ | Incrementally sync changed files into the index |
| [`status`](#status) | ✅ | Index and sync health |
| [`search`](#search) | ✅ | Full-text search over the code graph |
| [`query`](#query) | ✅ | Façade over search/callers/callees |
| [`context`](#context) | ✅ | Deterministic context bundle for a task |
| [`explore`](#explore) | ✅ | Neighbourhood exploration, grouped by file |
| [`node`](#node) | ✅ | Full info for one symbol |
| [`callers`](#callers--callees) | ✅ | Direct callers of a symbol |
| [`callees`](#callers--callees) | ✅ | Direct callees of a symbol |
| [`impact`](#impact) | ✅ | Transitive impact, both directions |
| [`impact-intersection`](#impact-intersection) | ✅ | Which planned work items collide, and on what |
| [`precedent`](#precedent) | ✅ | Structurally analogous code — the sibling that already does this |
| [`branch-overlap`](#branch-overlap) | ✅ | Which git refs collide, and what a merge did not carry |
| [`affected`](#affected) | ✅ | Files affected by a changed set |
| [`implements`](#implements) | ✅ | Code that implements a doc node or requirement |
| [`referencing-docs`](#referencing-docs) | ✅ | Doc sections that reference a symbol |
| [`stats`](#stats) | ✅ | Usage/performance statistics |
| [`languages`](#languages) | ✅ | Registered language grammars |
| [`serve`](#serve) | ✅ | MCP server over stdio and/or the localhost web UI (`--ui`: the `ui` feature is on by default and absent from the slim build) |
| [`xservice`](#xservice-workspace-federation-queries) | ✅ | Cross-service queries over a workspace: `route-providers` / `callers` / `impact` / `search` / `build-deps` / `type-refs` (`--repo` to scope) |
| [`workspace status`](#workspace-status) | ✅ | Per-member freshness, warm state and open state + the 3-state cross-service coverage summary — exits 1 if a member could not be opened |
| [`workspace reachability`](#workspace-reachability) | ✅ | App-wide cross-service dead-code union view — advisory, never a gate input; exits 1 if a member could not be opened |
| [`workspace check`](#workspace-check) | ✅ | Evaluate workspace governance rules over cross-service bindings — advisory: a violation never moves the exit code (an unopenable member exits 1) |
| [`scan`](#scan) | ✅ | Full architecture-quality scan |
| [`offenders`](#offenders---path-glob---dimension-d) | ✅ | Every offender the last snapshot recorded, uncapped, with its symbol id, value and threshold |
| [`review`](#review---base-ref) | ✅ | The branch delta: every offender and violation classified new, worsened, improved, resolved or unchanged against the merge-base — advisory, never moves `check` or `gate` |
| [`improvements`](#improvements) | ✅ | Where to start: the ranked fix units the last snapshot recorded, each with its estimated recoverable points — advisory, never gated |
| [`check`](#check---rules-file---allow-no-rules) | ✅ | Architecture-rules compliance check |
| [`gate`](#gate---save---threshold-n---label-l) | ✅ | CI quality gate on the signal |
| [`health`](#health) | ✅ | Architecture health — DB integrity, schema, FTS, structural + admission drift |
| [`session-start`](#session-start--session-end) | ✅ | Start the session before edits; saves the quality baseline when none exists (the CLI half of the session gate) |
| [`session-end`](#session-start--session-end) | ✅ | Re-score against the baseline; exit 1 on regression (5 on a missing required security scan) |
| [`doctor`](#doctor) | ✅ | Fast structural-integrity + admission tripwire; exit 1 on drift |
| [`verify`](#verify) | ✅ | Deep reindex-diff consistency check; exit 1 on drift |
| [`coverage refresh`](#coverage-refresh) | ✅ | Run the configured `refresh_cmd` and ingest what it produced |
| [`quality-report`](#quality-report---hook-json) | ✅ | Non-blocking quality readout — writes nothing, always exits 0 (CLI-only) |
| [`hook --emit`](#hook---emit---force) | ✅ | Install the Claude Code session-start quality-report hook (CLI-only) |
| [`evolution`](#evolution) | ✅ | Signal evolution over snapshots |
| [`dsm`](#dsm---granularity-g) | ✅ | Dependency-structure-matrix clusters |
| [`doc-gaps`](#doc-gaps---limit-n) | ✅ | Undocumented exported symbols |
| [`hotspots`](#hotspots) | ✅ | Churn × complexity ranking — the non-gated temporal tier |
| [`coverage ingest`](#coverage-ingest-report---format-fmt) | ✅ | Ingest an LCOV/Cobertura report into the evidence store |
| [`coverage status`](#coverage-status) | ✅ | Per-file coverage freshness + the overall fraction |
| [`security scan`](#security-scan---consent---install---scanner-name---path-path) | ✅ | Run the installed, enabled scanners and ingest their SARIF — the first scan needs `--consent`; `--install` installs the missing ones first; `--scanner`/`--path` re-scan only what a fix touched |
| [`security install`](#security-install-scanner--all---update---from-dir---consent---accept-codeql-licence) | ✅ | Download, verify and install scanners under a per-user consent; your own copy wins (CLI-only) |
| [`security uninstall`](#security-uninstall-scanner--all) | ✅ | Remove Logos-managed scanner copies, never your own (CLI-only) |
| [`security ingest`](#security-ingest-report) | ✅ | Ingest a SARIF 2.1.0 report from any tool into the evidence store |
| [`security status`](#security-status) | ✅ | Each scanner's state — not installed, disabled, never run, stale or fresh — and the consent state |
| [`security findings`](#security-findings) | ✅ | Stored findings with their OWASP categories and suppressions, filtered |
| [`security bless`](#security-bless) | ✅ | Write the stored findings to the checked-in `.logos/security-baseline.json`, the ratchet's baseline in every checkout (CLI-only) |

---

## Answering for one workspace member: `--repo`

Under a [workspace](#init--i---hooks---workspace---yes---exclude-glob) a
single-member question can name the member it is about, exactly as the MCP tools'
`repo` parameter does ([usage.md](usage.md#asking-about-one-workspace-member-repo)):

```bash
logos node beta_only --repo svc-b            # found: svc-b holds it
logos node beta_only                         # at the workspace root: the default member answers — not found there
logos callers helper_beta_only --repo svc-b --json
```

**Which commands take it.** The CLI twins of the read-only MCP tools, plus the `query` façade over three of them, and nothing else:
[`status`](#status), [`search`](#search), [`query`](#query), [`context`](#context),
[`explore`](#explore), [`node`](#node), [`callers` / `callees`](#callers--callees),
[`impact`](#impact), [`impact-intersection`](#impact-intersection),
[`precedent`](#precedent), [`branch-overlap`](#branch-overlap),
[`health`](#health), [`offenders`](#offenders---path-glob---dimension-d), [`review`](#review---base-ref), [`improvements`](#improvements), [`hotspots`](#hotspots), [`dsm`](#dsm---granularity-g),
[`evolution`](#evolution), [`coverage status`](#coverage-status),
[`security status`](#security-status) and [`security findings`](#security-findings). `health` keeps its
exit code, projected from the member it answered for. The `xservice` and
`workspace` groups already took `--repo` with a meaning of their own: it scopes their
fan-out and, for [`xservice callers` and `impact`](#xservice-workspace-federation-queries),
names whose symbol is meant. They do not refuse a name that is no member (exit 0):
`callers`, `impact` and `search` report it as a per-member error inside the answer, with
no cross-service row, and `route-providers` and `reachability` answer empty — check the
spelling before reading an empty answer as "nothing".

**Which commands cannot target another member.** The write and gate commands act on
the repository they run in, and refuse `--repo` as an unknown argument (exit 2):
`index`, `sync`, `scan`, `check`, `gate`, `verify`, `doctor`, `session-start`,
`session-end`, `coverage ingest`, `coverage refresh`, `security scan`, `security ingest`,
`security bless`, `quality-report` and `hook`. Their
MCP twins — `scan`, `rescan`, `check_rules`, `bless_rules`, `verify`, `doctor`, `session_start`,
`session_end`, `coverage_ingest`, `coverage_refresh`, `security_scan`, `security_ingest` — take no `repo` either, and say they act on
the default member. `affected`, `implements`, `referencing-docs`, `doc-gaps`,
`stats` and `languages` have no MCP twin that takes `repo`, so they answer for the
repository they run in.

**The default member.** `[workspace] default` in `logos.workspace.toml`, else the first
member. Run at the **workspace root** (the directory holding the manifest) without
`--repo`, the read-only commands above answer for it, as `logos serve --mcp` does on a
workspace — and say so: the payload carries `member`, first, ahead of the answer's own
fields (an answer that is not an object is wrapped as `{"member": …, "answer": …}`).
Run inside a **member's own directory** without `--repo`, a command answers for that
repository and its payload is exactly what it was before `--repo` existed — no
`member`. (`serve --mcp` started inside a member serves the workspace's default member
instead; ask `--repo` to be explicit.) In a plain repository, or inside a member, a blank
`--repo` is no `--repo`, and nothing changes: no `member` is added. A workspace root whose
manifest cannot answer without a `--repo` — one that does not parse, or names no member —
answers for the repository itself, as before; `--repo` on such a manifest is the error
(exit 2).

A member is read as an unindexed repository is: one that was never indexed is refused
(exit 3, `run \`logos index\``, naming its path) and nothing is written into it. Index it
first: `logos --project <MEMBER-PATH> index`. This is the one place the CLI and the MCP
`repo` differ: `serve --mcp` indexes a never-indexed member on first selection and
answers, as it always has for its default member.

At the workspace root the write and gate commands do not borrow the default member:
there is no index there for them to act on, and they refuse as they always did
(exit 3, `run \`logos index\``).

**Errors** are the MCP parameter's, word for word, from the one core function both
surfaces call:

| Situation | Message | Exit |
|---|---|---|
| `--repo` names no member | `no workspace member "nope"; the members are: svc-a, svc-b, svc-c` | 2 |
| `--repo` with no workspace | `repo "svc-b" selects a workspace member, and no workspace is loaded: this server answers for its one repository` | 2 |
| the member's store will not open | `workspace member "svc-c" is degraded: <the reason>` | 3 |

```bash
logos node x --repo nope       # exit 2, lists the members
logos node x --repo svc-c      # exit 3, names the member and why it would not open
```

The member is found by walking up from `--project` (or the current directory) to the
manifest, so `--repo` works from the workspace root and from inside any member. A member
built for the first time by `--repo` is admitted under the workspace's connection budget
like any other.

---

## Index & freshness

### `init [-i] [--hooks] [--workspace] [--yes] [--exclude <GLOB>]`

```bash
logos init                     # .logos/: store, config.toml, rules.toml, .gitignore (bare)
logos init -i                  # interactive: the bare files + .mcp.json + CLAUDE.md + readout hook
logos init --hooks             # install managed git hooks only
logos init -i --hooks          # interactive setup plus git hooks
logos init --workspace         # federate sibling repos into a workspace (prompts per member)
logos init --workspace --yes   # non-interactive: approve every discovered member
logos init --workspace --exclude 'vendor-*'  # skip members whose name matches the glob
logos init --uninstall         # remove everything Logos wrote; .logos/ stays
```

Bootstraps the project. All forms are **idempotent and non-clobbering** —
re-running on an already-initialised project applies any pending migrations
and leaves existing content untouched.

**Bare (`logos init`)** — writes only inside `.logos/`: `logos.db`, the starter
policy templates `config.toml` and `rules.toml` (both commented, enforcing
nothing until you edit them), and the managed `.logos/.gitignore` block. It is
not the implicit bootstrap that `logos index` performs, which creates `.logos/`
and the store with **no contract** and writes no policy file.

**Project identity.** Every form also writes `[project] id` into
`.logos/config.toml` when the key is absent. The value is the repository's
derived coordinate: the normalised `origin` remote, else the main worktree's
directory name ([`[project] id`](configuration.md#project-id--the-repositorys-identity)).
A present id is never rewritten, and a re-run leaves the file byte-identical. The
step reports on its own row, `.logos/config.toml [project] id`:

- `Updated` with the value it wrote;
- `Unchanged` when the key was already there;
- `Skipped` with the reason when the file is not valid TOML, holds `project` in a
  shape `init` cannot extend, or holds an invalid id.

When `.logos/config.toml` is git-ignored, the row adds that the id will not
travel and names the default other checkouts derive. `init` then records the
coordinate in the store, where `status` reads it.

**Interactive (`-i`)** — additionally writes, outside `.logos/`, a
`logos` entry in the project's `.mcp.json`, a managed usage block in
`CLAUDE.md`, and — default-on —
the **session-start quality-report** hook
([FR-IN-07](../specs/requirements/FR-IN-07.md)), merged into the shared
`.claude/settings.json`, which surfaces a signal/baseline/violations readout
when a session starts, resumes, or is reopened by `/clear`. The readout is a
[`quality-report`](#quality-report---hook-json) — it **writes nothing**, so
firing at every session boundary leaves your [`evolution`](#evolution) series
untouched. The merge is non-clobbering and the binary stays offline (no LLM
call, no outbound connection); it also sweeps the retired SessionEnd
quality-report hook
([CR-095](../requests/CR-095-session-start-quality-readout.md)) if a prior
install left one behind. Each step reports its action (`Created` /
`Updated` / `Unchanged` / `Skipped`). On non-TTY (CI, piped input): safe
defaults, no prompts.

`-i` does **not** install git hooks: on a terminal it offers them and the default
is no, and on a non-TTY it never installs them. Pass `--hooks` for them.

**Hooks (`--hooks`)** — installs managed git hook scripts under
`.logos/hooks/` and sets `core.hooksPath = .logos/hooks` in the project's git
config. Never overwrites unmanaged hook files. Can be combined with `-i`. Two
flavours are installed:

- **Freshness hooks** — `post-commit`, `post-checkout`, and `post-merge` each
  run `logos sync` on the changed file set after their git event, keeping the
  graph fresh. They are best-effort: they bail out silently when `logos` is
  absent and always exit 0, so they never block or fail a git operation.
- **Enforcing gate** — a `pre-push` hook runs [`logos check`](#check---rules-file---allow-no-rules) and
  **propagates its exit code**: a `severity='error'` violation
  (rule / structural / admission / dead-code) makes `git push` fail with exit
  1 and names the offending violation, turning "code about to leave the
  machine" into an enforced non-regression checkpoint. Unlike the freshness
  hooks it is *not* exit-0-swallowed. It still bails **open** (exit 0) when the
  `logos` binary is genuinely absent — never a false block — and
  `git push --no-verify` bypasses it (git skips the hook natively).

After running `logos init -i`, restart your agent — the `logos:*` MCP tools
appear automatically.

The plugin's install steps, the Stop check and its off switch, the `/logos:review`
skill, the guardrail and the trust labels are in
[The Claude Code plugin and the review loop](plugin.md).

**With the Claude Code plugin enabled** — when `enabledPlugins` holds
`"logos@logos": true` in the project's `.claude/settings.local.json`, its
`.claude/settings.json` or your user `settings.json` (under
`$CLAUDE_CONFIG_DIR`, else `~/.claude`), the most specific of those deciding,
`-i` lets the plugin carry the agent side
([FR-IN-12](../specs/requirements/FR-IN-12.md)). It writes the plugin's
enrolment (`extraKnownMarketplaces.logos` and `enabledPlugins`) into the
committed `.claude/settings.json`, so every teammate is prompted to install it,
and writes no `.mcp.json` entry and no readout hook. It removes the ones an
earlier `init -i` wrote, and a skill copy it materialized, which would otherwise
list every tool and print the readout twice. The skill copies removed are those of
the core's own legacy skill (`logos-wiki`) only: a skill under any other name, or
one an extension registers as its own (active or not), stays whatever its
header says. Each removal is its own `Removed` row. With the plugin absent, `-i` is exactly as described above.

**Uninstall (`--uninstall`)** — removes everything Logos wrote into the
project: the `CLAUDE.md` managed block, the `logos` `.mcp.json` entry, the
readout hook entry and its script, the plugin enrolment, skill copies with
their `.claude/skills` pointers, and the managed git hooks (in every linked
worktree too), unsetting `core.hooksPath` only if it still points at
`.logos/hooks`. An entry goes only while it holds exactly what `init` writes,
and a file or directory only when nothing else is left in it, so foreign
content stays. `.logos/` stays too: delete it yourself to drop the index. A
second run reports every row `Unchanged`. Two limits follow from that rule.
`init` keeps no record of what was there before it, so an empty container you
had made yourself (an empty `mcpServers`, an empty `.claude/skills/`) goes with
Logos's last entry in it. And a workspace root's `logos-workspace` `.mcp.json`
entry and root `.gitignore` block, written by `init --workspace`, are not
removed. `-i` and `--hooks` add nothing to it
(`logos init --hooks --uninstall` is the same command); it cannot be combined
with `--workspace`.

Together these hooks realize the local legs of the **freshen / enforce / report
/ bless** loop: the freshness hooks *freshen*, the `pre-push` gate *enforces*,
and the session-start quality-report hook *reports*. The CI leg (and the
release-only *bless* with `logos gate --save`) is documented in
[CI integration](ci-integration.md).

**The parent-of-repos nudge (no flag needed)** — a **plain** `logos init`, with
no `--workspace`, still detects when it is being run one directory too high: a
root that is not itself a git repository but whose immediate children are
([FR-IN-08](../specs/requirements/FR-IN-08.md)). Left alone, that shape used to
succeed silently and create a store that could never admit anything (the
zero-admission defect [`status`/`doctor` also diagnose](#status), below). Now
it explains the shape on **stderr**, naming `logos init --workspace` as the
fix, and — **on a TTY only** — offers to take that path instead; declining
(the default) or answering non-interactively (CI, a piped shell, a dev-pane
spawn) both fall through to the ordinary single-root `init` with **no prompt
and no stdin read**, so an unattended invocation can never wedge on it. stdout
is byte-for-byte the same either way — the nudge and the offer are entirely a
stderr conversation, and `--quiet` does not suppress it (unlike the workspace
footprint notice below): it is the reason the command's own successful-looking
output would otherwise mislead. Accepting on a TTY runs the same
[FR-WS-02](../specs/requirements/FR-WS-02.md) enablement path `--workspace`
takes — which, like an explicit `logos init --workspace -i`, drops `-i`/
`--hooks` rather than applying them per member; the offer names that trade-off
before you answer, so accepting is an informed choice, not a silent downgrade.
If you then decline **every** member, the `logos init` you typed is still
carried out: the command completes the ordinary single-root `init` — with the
`-i`/`--hooks` you asked for, since the workspace those were traded against is
not being created — rather than leaving the root with neither a workspace nor a
`.logos/` ([CR-103](../requests/CR-103-nudge-declined-to-empty-falls-back-to-plain-init.md)).
An explicit `logos init --workspace` is deliberately **not** the same here: there
you asked for a workspace by name, so approving no member reports nothing to do
and writes nothing at all. At an ordinary repository root nothing fires at all.
A re-run at a root that is *already* a workspace still nudges — detection reads
the directory's shape, not whether a `logos.workspace.toml` is sitting there —
so accepting the offer again simply re-runs enablement over the existing
manifest, which is non-clobbering and preserves your hand-written keys.

**Workspace (`--workspace`)** — turns a parent folder of sibling repositories
into a **Logos workspace** ([FR-WS-02](../specs/requirements/FR-WS-02.md)). Run
it from the directory that contains your service repos. It:

1. **Discovers members** — scans immediate child directories for distinct git
   roots (a repo already carrying `.logos/logos.db` also counts).
2. **Gates approval** — prompts per candidate on stderr (y/n). `--yes` approves
   all discovered members non-interactively; `--exclude <GLOB>` drops members
   whose workspace-relative name matches the glob (repeatable). The exclude
   applies only to newly-proposed candidates — members already in an existing
   manifest are never re-prompted or re-excluded. **The globs are persisted**:
   each one is added to the manifest's
   [`[workspace] exclude`](configuration.md#workspace-exclude--keeping-a-directory-out-for-good)
   (sorted, de-duplicated, never removed), so a later `init --workspace --yes`
   that does not repeat the flag neither proposes nor adds the directory — and
   with `autodiscover` on it does not rejoin the workspace either.
3. **Initialises each member** — runs the ordinary per-member `init`
   (write-if-absent, **never** clobbering an existing member config).
4. **Writes the manifest** — a `logos.workspace.toml` at the parent listing the
   approved members, plus your hand-written
   `default`/`autodiscover`/`exclude`/`[workspace.warm]`/`[[links]]`/`[governance]`
   preserved verbatim on a re-run (`exclude` only ever gains the globs of the
   run's `--exclude` flags).
5. **Injects one MCP entry** — a single `logos-workspace` server key in the
   parent `.mcp.json` (distinct from the per-repo `logos` key so a member's own
   entry is never shadowed).

**What it tells you, and what `index` at the root does not cover.** Because
warming is hybrid (below), the output says so rather than sending you to a
command that would not help
([CR-119](../requests/CR-119-workspace-enablement-misdirects-to-an-index-that-builds-only-the-root.md),
[FR-WS-02](../specs/requirements/FR-WS-02.md)):

- Each enrolled member reports `enrolled — queued for background warming (see
  \`logos workspace status\`)`. It does **not** tell you to run `logos index` —
  the supervisor is already doing exactly that, per member.
- One workspace-level note states that background warming started, over how
  many members, and names `logos workspace status` as the surface reporting
  progress and outcome ([FR-WS-15](../specs/requirements/FR-WS-15.md),
  [FR-WS-17](../specs/requirements/FR-WS-17.md)) — in both the human and
  `--json` renderings.
- A plain single-repo `logos init` message is unchanged.
- Running [`logos index`](#index) **at a root carrying `logos.workspace.toml`**
  emits an advisory note that it built the root project only, and that members
  are indexed independently. This matters because the root of a
  parent-of-repos workspace is nearly empty: the nested-`.git` boundary rule
  prunes every member, so a root `index` legitimately reports a handful of
  files while thousands are indexed across the members. The note is advisory —
  `index` still exits `0` and adds no `warnings` entry a CI parser would trip
  on — and `index` never grows a member fan-out
  ([NFR-PE-06](../specs/requirements/NFR-PE-06.md)).
- Before enablement, the same root still gives the
  [`FR-IX-13`](../specs/requirements/FR-IX-13.md) zero-admission warning
  (`files_indexed: 0`). The two states are distinct and both are asserted.
  **After** enablement a re-run of `index` is silent for the plainest of
  reasons — the root now admits its own `logos.workspace.toml`, so the
  zero-admission condition is simply not met. What that leaves is the surface
  that does *not* re-walk: `status` and `doctor` read the file count the last
  completed index stored, so at a root indexed **before** enablement they keep
  seeing `0` until you re-index, and before this was fixed they went on telling
  you to run `logos init --workspace` — which you had already done. The
  diagnostic is now suppressed at any root carrying a `logos.workspace.toml`,
  in the one shared derivation, so every surface falls silent together.

Indexing is **hybrid**: the command returns immediately while a **single
detached supervisor** warms the approved members through a bounded queue — at
most `max(1, cores / 4)` members index concurrently, capped at 4
([FR-WS-14](../specs/requirements/FR-WS-14.md)). It is one supervisor for the
whole workspace, not one process per member: an 86-member workspace warms four
at a time, not 86 at once.

**Tuning the warm bound.** The default is deliberately conservative because it
is derived for a host Logos knows nothing about. To set it yourself, add an
optional table to `logos.workspace.toml`
([FR-WS-01](../specs/requirements/FR-WS-01.md)):

```toml
[workspace]
name    = "pec-services"
members = ["archive-api", "mailbox-api"]

[workspace.warm]
# optional; default = max(1, cores / 4), capped at 4
concurrency = 2
```

Know what you are buying. Each of the K is a full `logos index` child that is
itself parallel over your cores, so K costs roughly **K × cores worker threads**
and up to **K × one member index's peak memory** — which is exactly why the
default divides by four instead of scaling with the host. Legal values are
`1`–`16`. `0` or anything above `16` is rejected when the manifest loads, with a
message naming the key and the legal range; a non-integer is rejected as a type
error naming the key and its line. Either way it is exit code 2 and never a
quiet clamp to something you did not ask for. Note that the manifest is read on
the path of *every* command, so a rejected value fails all of them until you fix
it — the same as any other malformed key. Omit the key
or the whole table for the default. Whatever resolves is a **hard** ceiling: no
member count and no `--yes` puts more indexes in flight than K
([BR-44](../specs/software-spec.md#327-workspace-federation)). The key is
operator-authored — `logos init --workspace` never writes it, and never erases
it on a re-run. A member that has not finished warming still indexes
correctly on first real use (lazy `ensure_indexed`). A member that fails to
*initialise* — a manifest or MCP step — is reported **degraded** without
aborting the others. A member whose **warm** fails after the command has
returned is currently not reported anywhere: it simply stays un-indexed and
shows as `deferred` in [`workspace status`](#workspace-status) until its first
real use indexes it. stdout stays machine-clean (the approval prompt is on
stderr), so `logos init --workspace --yes` is safe to script. Re-running is
incremental — `manifest` and `mcp` report `unchanged`, `root_ignore` reports
`unchanged` on a tracked root and `skipped` on one that is not a repository, and
neither a duplicate MCP entry nor a second managed ignore block is written.
When the workspace root **is** a git working tree, `root_ignore` reports the
managed `.gitignore` block keeping the warm sidecar out of version control; at
a root that is not a repository — the ordinary parent-of-repos shape — it
reports `skipped` with that reason and no file is written
([CR-104](../requests/CR-104-managed-workspace-root-ignore-for-the-warm-sidecar.md),
see [configuration](configuration.md#workspace-federation-files--the-manifest-and-the-warm-sidecar)). When no sibling repos are found, nothing is written and the
command exits 0.

The report also states the **working-tree footprint** the command left behind
([FR-WS-02](../specs/requirements/FR-WS-02.md)) — enabling 84 members
legitimately makes 84 repositories git-dirty, and that should not go unsaid. A
`footprint` object in the report (both `--json` and the default rendering)
counts the members that now carry a `fresh` untracked `.logos/`, those that
already had one (`unchanged`), and any `degraded` ones, and names both halves of
[FR-IN-04](../specs/requirements/FR-IN-04.md): what is meant to be `committed`
inside `.logos/` (`config.toml`, `rules.toml`, `.gitignore` — the policy that
travels with each repository) and what the generated `.logos/.gitignore` already
`ignored` (`logos.db*`, `telemetry.db*`, `secrets.toml`, …). A one-line prose
summary of the same goes to **stderr** whenever at least one member is fresh, so
stdout stays exactly one machine document; `--quiet` suppresses that line and
leaves the payload untouched. Nothing about what `init` writes changed — no
member's tracked files, and no member's own `.gitignore`, are touched.

> **Federation is an in-memory overlay, never a graph union.** Each member keeps
> its own `.logos/logos.db` and its single-root behaviour unchanged; the
> workspace is assembled on demand and never persisted across a database
> boundary ([ADR-52](../specs/architecture/decisions/ADR-52.md)). With **no**
> `logos.workspace.toml` present, every command behaves exactly as a single-repo
> checkout — federation is entirely dormant.

### `index`

```bash
logos index
```

Full pipeline run: discover → extract → resolve → detect frameworks →
annotate. Creates `.logos/` (and `logos.db`) if absent. Idempotent — safe to
re-run any time; the result depends only on the source tree and config.
Reports per-phase counts (files indexed, nodes/edges created, resolution
coverage, routes found, dead/duplicate annotations).

With `--json`, the result also carries a `phases` object with the per-phase
wall-clock duration in milliseconds — `discover`, `load`, `extract`, `persist`,
`resolve`, `framework`, `dispatch`, `annotate` — derived from the same `tracing`
seam as the logs (the durations sum to ≤ `total_ms`, never double-counted). Use
it to see where a cold index spends its time before optimizing.

Each file is persisted in isolation. A file whose facts cannot be persisted is
rolled back alone and left **absent** from the graph; every other file persists.
The file is listed in `files_failed`, its reason in `persist_failures`
(`[{"path", "reason"}]`, omitted when empty), and a warning names it; the run
exits `0`. When files reached persistence and **none** persisted — or the
pipeline itself failed — the result carries `"failed": true` and a
`nothing was persisted` warning, and `index` exits `1`. An index that admits no
file at all still exits `0`.

### `sync [PATHS]...`

```bash
logos sync src/auth.rs src/db.rs   # fold in exactly these paths
```

Incremental fold-in of changes — much faster than a full `index` on large
trees. Deleted files' symbols are captured before removal so inbound
references degrade gracefully rather than dangle.

A sync leaves the store as a fresh `index` of the same tree would, in every
language: every reference of each symbol a changed file affects is re-bound, and
an edge none of those references produces any more is removed in the same
transaction. A captured reference is deleted once it re-binds; one whose target
was renamed away stays unresolved. A store synced by an earlier release that
still holds spent captures loses them on its next `sync`, even one that changes
nothing.

`sync` reconciles **exactly the paths it is given** and never sweeps the rest of
the tree. With no path it re-reads no file, so `logos sync` alone does not pick up
an edit. The paths usually come from somewhere else: the watcher under
`logos serve` passes each debounced batch, and the managed git hooks pass the
files a commit or merge changed. To fold in every change at once, run a
reconcile-then-score command such as `logos scan` (see
[usage.md](usage.md)), or rebuild with `logos index`.

A file whose new facts cannot be persisted keeps its **last good** nodes and
edges and is recorded **stale**; the other paths persist. It appears in
`files_failed` and `persist_failures`, `status` reports it stale, and the next
reconcile retries it — the stale mark clears once it persists or is deleted.
`sync` exits `1` only when every path it reached failed to persist (or the
pipeline failed), with `"failed": true` in the result.

### `status`

```bash
logos status --json
logos status --repo <MEMBER>       # under a workspace: that member's index
```

Index health: file/node/edge counts, store size, unresolved-reference ledger,
resolution coverage (both the single global figure and `resolution_by_language`,
see below), last index/sync timestamps, a persisted monotonic
`graph_revision` (a counter bumped once per `index`/`sync` that actually
changes the graph, and once when opening a store migrates it to the repository
coordinate — a no-op `sync` leaves it untouched; consumers can compare
it across processes to detect a stale cache), and the freshness posture
(navigation serves the latest committed snapshot; it never reconciles per
call).

`project_id` is the repository's identity coordinate
([`[project] id`](configuration.md#project-id--the-repositorys-identity)): the
configured key, else its derived default. It is recorded by `logos init`, by
every full `logos index` and by the migration that moves an existing store to
the coordinate when it is opened
([configuration.md](configuration.md#project-id--the-repositorys-identity)),
and omitted until one of them has run.

While any file's facts could not be persisted, `status` (and `scan`) carry a
`persistence` object — `failed_to_persist`, the count, and `stale_files`, those
whose last good facts the graph still holds (the rest are absent) — plus a
warning saying so. It is omitted once every such file has persisted or been
deleted.

`last_full_index_at` is read from a durable `project_metadata` record, not an
in-process counter — a separate, read-only `status`/`workspace status`
invocation reports the timestamp of the last full index a *different* process
ran, rather than always printing `null`. It dates the **graph**, not the
command: a `sync` that persists no files but leaves an existing graph in place
does not clear it, so it is not confused with `last_sync_at` (unchanged,
already durable). Where no full index has ever completed the field is
reported **absent** — never `0`, never fabricated.

The read-model also carries a source/test **lines-of-code roll-up**:
`total_line_count`, `source_line_count`, and `test_line_count` (with
`total == source + test`). Like `indexed_loc`, the roll-up is computed at
**full-`index`** time and may lag after an incremental `sync` until the next
full index; when it has not been computed the three fields are `null` (never a
fabricated `0`). Since logos 1.4.14 the roll-up distinguishes **three** cases
rather than writing unconditionally, matching the last-full-index stamp beside
it: an index that persists at least one file **writes** the figures; one that
persists nothing over an empty store **clears** them, so the fields read absent
beside `indexed: false`; and one that persists nothing **while a previous graph
survives leaves them untouched**, so `status` keeps reporting the figures that
describe the graph you still have. Before this, an index in which every
candidate failed to load overwrote the roll-up with `0`/`0`, and `status`
reported a populated graph carrying `total_line_count: 0` beside a correct file
count and a correct timestamp. The same three fields ride the MCP `status` tool and the web
`/api/v1/overview` bundle, and surface on the Dashboard Graph card (see
[Dashboard](usage.md)).

If the graph is empty because you ran `logos index` at a **parent folder of
sibling repositories** — every child pruned as a nested `.git` boundary —
`warnings` carries the zero-admission diagnostic naming the prune count, a
sample of the pruned directory names, and the remedy `logos init --workspace`
([FR-IX-13](../specs/requirements/FR-IX-13.md)). It is the same line `index`
emits and `doctor` reports in `zero_admission_warning`, derived from one shared
helper, so you meet the same explanation whichever surface you reach for next.
It is advisory: `status` has no exit code to move, and the diagnostic never
becomes a rule finding or feeds the quality signal.

It is **suppressed** when the root already carries a `logos.workspace.toml` —
you have run the remedy it names, so repeating it would be wrong. This matters
most here: `status` reports the count the last completed index stored, so a root
indexed *before* you enabled the workspace keeps reporting `0` until you
re-index, and that is exactly the state in which the advice would be stale. See
[`init --workspace`](#init--i---hooks---workspace---yes---exclude-glob) for the two states.

#### `resolution_by_language`

One row per indexed language, in name order, each carrying **both** halves of the
figure rather than a bare number:

```jsonc
{ "language": "typescript", "files": 75,
  "calls":   { "references": 1504, "bound": 580, "same_file_edges": 401,
               "cross_file_edges": 179, "cross_file_absence": null },
  "imports": { "references": 166,  "bound": 118, "same_file_edges": 0,
               "cross_file_edges": 118, "cross_file_absence": null } }
```

Exactly one of `cross_file_edges` and `cross_file_absence` is present. A language
that binds nothing across a file boundary is reported as a **named state**, never
as a `0` that would read as a measurement:

| `cross_file_absence.cause` | What it establishes |
|---|---|
| `same-file-only` | references resolved, but every resolved edge stays inside one file |
| `no-resolved-edges` | references were recorded and none bound |
| `no-references-recorded` | the language contributed no reference of that class at all |

A global coverage number cannot express a per-language zero — that is why this
row set exists. Read it before trusting a relational answer on a given language.

**Java and Rust rows also say why a call stays unbound.** The Java row (and
the PHP, C#, Kotlin and Scala rows) and the Rust row carry a `call_residue`
object: `unbound` (the row's `calls.references − calls.bound`), the count per
reason below, and `unclassified` — so `unbound` equals the sum of the reasons
plus `unclassified`. Its `scope` is `"repository"` for a plain `status` and
`"workspace"` for `workspace status`: one repository cannot tell another
member's type from a library's, so only the workspace read has a
`type-in-another-member` count.

```jsonc
"call_residue": { "unbound": 5,
  "reasons": { "external-type": 1, "name-not-in-scope": 1, "no-applicable-overload": 0,
               "no-receiver-evidence": 1, "not-a-callable": 1, "overload-ambiguous": 0,
               "supertype-unreached": 1, "type-ambiguous": 0 },
  "unclassified": 0, "scope": "repository" }
```

| `call_residue` reason | The call stays unbound because |
|---|---|
| `no-receiver-evidence` | the file proves no receiver type (a chained call, an untyped lambda parameter, a generic type variable, a bare call naming no import) |
| `external-type` | no file of this repository declares the receiver's type: the JDK, a library, a generated type, or (in a plain `status`) another member — or, for a Rust path call, the path's head leaves the repository (`Vec::new()`, `serde_json::to_string(…)`) |
| `type-in-another-member` | another workspace member declares the receiver's type (`workspace status` only) |
| `overload-ambiguous` | the type, or the nearest supertype level holding an applicable method, declares two or more methods of that name whose parameter count admits the call, or two static imports each supply one — no argument type is read |
| `no-applicable-overload` | methods of that name were found, and none admits the call's argument count — at no supertype level, and in no free function or import the language goes on to |
| `type-ambiguous` | the type's name reaches two declarations (a `src/main` and a `src/test` class of one name) |
| `supertype-unreached` | neither the type nor any supertype reached in the repository declares the name: the chain leaves the repository, stops at an interface, or cycles |
| `not-a-callable` | the call names a repository declaration that is not a callable: an enum variant (`E::A(1)`) or a tuple-struct constructor (`W(1)`, `Self(1)`) — Rust |
| `name-not-in-scope` | no rung reaches a repository callable of the name: a prelude or std function reached by a bare name (`drop`, `Some`), a closure or a local, an item the file does not import, a module that declares no such item — Rust |

On the Rust row every unbound call carries a reason, so `unclassified` reads `0`
on a fresh index. A call to a type's method — `self.m()`, `Self::m()`, a method
call on a proven receiver ([FR-RS-42](../specs/requirements/FR-RS-42.md)), a
written `T::m()`, a qualified `<T as Tr>::m()` — binds through one
associated-item lookup ([FR-RS-47](../specs/requirements/FR-RS-47.md)) among
the functions of every `impl` block whose self type resolves to `T`, from any
crate and through `pub use` re-exports, and the default bodies of the traits
`T` implements (an empty `impl Tr for T {}` included) that its impl does not
override. A method call that finds nothing on `T` retries on `T`'s `Deref`
target, each type once and at most 8 hops; a path call never does. The reasons:
`external-type` when `T` is no repository type (`String`, `Vec`, an external
crate's, an `impl` for a primitive or a generic parameter, a `Deref` target
outside the repository, or a method the peeled `Arc`/`Rc`/`Box` provides
itself, such as `clone`); `supertype-unreached` when nothing along `T`'s
`Deref` chain declares or inherits such a function the call can reach — a
derive supplies it, method syntax met only an associated function, or the trait
is not in scope at the call;
`overload-ambiguous` for two functions of one rank (two traits in scope each
supplying the method, a default among them), or an inherent `&self` method
beside a trait's by-value `self` one; `no-applicable-overload` when every
such function's parameters exclude the call's argument count (a path call to a
`self`-taking function counts its receiver); `type-ambiguous` when the file, or
the file re-exporting `T`, imports its name twice; `not-a-callable` for an enum
variant. A trait-typed call — `self.m()` / `Self::m()` in a trait's default
body, a receiver typed `&dyn Tr`, `impl Tr` or a generic parameter bounded by
`Tr` (inline or in a `where` clause), a written `Tr::m(x)` — binds every impl
of `Tr::m` plus the trait's default body; two bounds
that each supply the method are `overload-ambiguous`, a method only a bound
outside the repository (`Clone`, `Send`) can supply is `external-type`, and a
trait with neither an impl nor a default of it is `supertype-unreached`. An
unproven receiver is `no-receiver-evidence`. Any other path call is
`external-type` when its head
leaves the repository (`Vec::new()`, `serde_json::to_string(…)`) and
`name-not-in-scope` when it reaches a repository module that declares no such
item; a bare call is `name-not-in-scope` unless it names a type or a variant
(`not-a-callable`). A required trait method — a signature with no body — is
recorded but bound by no call: a call to it reaches the impls that supply the
body. In `workspace status` the Rust row's
`type-in-another-member` stays `0`: a type of another member is not resolved,
and is counted `external-type`.

`call_residue` is never stored. A long-lived engine — `logos serve`, its web
dashboard and MCP server — computes it once per graph revision and `[resolution]`
section and answers every later `status` from memory until an `index`, a `sync`
that changes a file, a config-narrowing purge, a `[resolution]` edit or a schema
migration moves one of them. The computation costs a fraction of a second on a
Java project, about 0.4 s on a Rust project of ~90,000 calls, and nothing on a
project of other languages only. A one-shot `logos status` is a new process, so
it always pays it.

Every figure counted over the reference ledger — `refs_total`/`refs_resolved`,
each language's `references` and `bound`, the per-relation coverage and the
`call_residue` counts — leaves out the capture-before-delete rows a `sync`
writes for edges into a re-read file, because each one duplicates a reference
its source already records. A synced store therefore reads the same figures as
a fresh `index` of the same tree, on the CLI, `--json`, MCP and HTTP alike.

## Navigation

> **Every relational answer carries a `resolution_denominator`.** `callers`,
> `callees`, `impact`, `impact-intersection`, `branch-overlap`, `affected` and
> `precedent` each return a typed field naming the resolved edge set the answer
> was computed over — on **every** answer, empty or not. It is a machine-readable
> field, not prose in `warnings`.
>
> ```jsonc
> "resolution_denominator": {
>   "languages": [ { "language": "typescript", "calls": { … }, "imports": { … } } ],
>   "absence": null
> }
> ```
>
> `languages` holds the row (same shape as [`resolution_by_language`](#resolution_by_language))
> for each anchor's language. When no row could be read, `absence` names why
> instead — `unindexed` (no anchor resolved), `no-language-recorded` (anchors
> resolved into no language-tagged file) or `n/a` (no denominator was read).
> Exactly one side is populated.
>
> This is what separates *nothing depends on this* from *nothing could be
> resolved here*. A `total: 0` beside a `same-file-only` row is the index's reach,
> not evidence of absence — see
> [where the relational claims hold](README.md#where-the-relational-claims-hold).
> The denominator is also present on **non-empty** answers, where it tells you
> whether a count is complete or partial.
>
> **Caveat:** a row counts edges *leaving* its language. An inbound answer whose
> callers live in another language carries only its anchor's row.

> **A symbol written before symbols named their repository still resolves.**
> Every symbol carries its repository's coordinate in the package slot
> (`logos . github.com/acme/payments . src/`lib.rs`/run().`; see
> [project identity](configuration.md#project-id--the-repositorys-identity)). A
> symbol argument whose package slot is empty — `logos . . . src/`lib.rs`/run().`,
> from a script, a bookmark or an older answer — is resolved under the coordinate
> this store's symbols carry, and the answer says so in one line in `warnings`:
>
> ```text
> "logos . . . src/`lib.rs`/run()." carries no repository coordinate; resolved as "logos . github.com/acme/payments . src/`lib.rs`/run().", the form this store holds
> ```
>
> Every tool that takes a symbol does this: `node`, `callers`, `callees`,
> `impact`, `explore`, `precedent`, `impact-intersection`, `implements`,
> `referencing-docs`, the web graph's `?seed=`, and in a workspace `xservice
> callers` and `xservice impact`, where each member resolves it under its own
> coordinate and the cross-service rows follow. A symbol in the canonical
> form answers exactly as before, with no note. A symbol naming another
> repository's coordinate is never rewritten: it resolves as unknown, with
> suggestions. Pass the canonical form to drop the note.


### `search`

```bash
logos search <QUERY> [--kind <KIND>] [--limit <N>] [--repo <MEMBER>]    # default limit 20
```

FTS5 full-text search over symbol names. `--kind` filters by node kind
(`function`, `struct`, `route`, …) — including the documentation kinds
`doc_file`, `doc_section`, `requirement`, `adr`, and `story` once markdown is
indexed (see [Documentation graph](#documentation-graph)), and the config &
artifact kinds once config indexing is on (see [Configuration & artifact
graph](configuration.md#configuration--artifact-graph--indexing-config-and-infra-files)):
`config_file`, `config_section`, `dockerfile_stage`, `make_target`,
`shell_function`, `proto_message`, `proto_service`, `gql_type`, `tf_block`,
`sql_object`, `api_path`, and `api_operation`. Near-miss queries return
suggestions.

### `query`

```bash
logos query <SYMBOL> [--kind <KIND>] [--limit <N>] [--repo <MEMBER>]
logos query <SYMBOL> --callers
logos query <SYMBOL> --callees
```

One entry point for the three most common questions. `--callers` and
`--callees` conflict by design (exit 2 if both given).

### `context`

```bash
logos context <TASK>... [--max-nodes <N>] [--no-code] [--repo <MEMBER>]   # default 25 nodes
```

The token-saving tool: given a free-text task description, assembles a
deterministic bundle of the most relevant symbols with their declarations —
one call replacing many file reads. `--no-code` returns the structural map
only. Anchoring spans code **and** documentation: a multi-word prose task whose
terms match a requirement or doc section anchors there and expands along
doc→code edges to the implementing symbols, so prose phrasing still yields a
non-empty bundle (it is kind-balanced so documentation matches cannot crowd out
code symbols).

### `explore`

```bash
logos explore <QUERY> [--max-files <N>] [--repo <MEMBER>]   # default 10 files
```

Anchors on the best-matching symbol and walks its neighbourhood, returning
source grouped by file — the "show me around this area" tool. When the query is
a bare name that matches several nodes, the anchor is chosen as for `node` (code
before module before doc) and the nodes passed over are listed under
`alternatives` (absent when none). A query that matches no name exactly falls back
to the best full-text match, which passes over nothing.

### `node`

```bash
logos node <SYMBOL> [--code] [--repo <MEMBER>]
```

Everything about one symbol: kind, location, export status, annotations
(dead/duplicate), complexity, immediate edges; `--code` includes the
declaration source. For a code symbol referenced from documentation, the
response also lists the referencing doc sections; for a documentation node, its
doc→code edges.

A bare name that matches several nodes resolves to a code type (class, struct,
interface, trait, enum, type alias), else a callable, else any other code
declaration (a field, constant, variable, macro, route, …), else a module, else a
config-layer node (a YAML key, a shell function, a proto message, …), else a
documentation node; within one class the lowest node id wins. So `logos node
Utils` on a PHP tree where `Utils` is both a class and its file module returns
the class. The nodes it passed over are listed
under `alternatives`, in that order, and each one's `symbol` reaches it when
passed back to `node`. The key is absent when nothing was passed over: a SCIP
symbol is exact, and a name only one node carries has no alternative. A symbol id
without its repository coordinate — the `node_id` an `offenders`, `review` or
single-symbol `improvements` entry carries, such as ``src/`lib.rs`/deep_one().``
— is exact too, and carries no note. `callers`,
`callees`, `impact` and `explore` resolve a bare name by the same rule and name
what they passed over in the same `alternatives` key (see below).

### `callers` / `callees`

```bash
logos callers <SYMBOL> [--limit <N>] [--repo <MEMBER>]   # default 50
logos callees <SYMBOL> [--limit <N>] [--repo <MEMBER>]
```

Direct call-graph neighbours, one hop each way.

A bare name resolves as it does for `node` (code type > callable > other code
declaration > module > config node > doc node, lowest node id within a class), and
the answer lists the nodes it passed over under `alternatives`. So `logos callers
Utils` on a PHP tree where `Utils` is both a class and its file module reports the
callers of the class and names the module. Each alternative's `symbol` reaches it
when passed back; a SCIP symbol or a name only one node carries has no
`alternatives` key.

### `impact`

```bash
logos impact <SYMBOL> [--depth <N>] [--repo <MEMBER>]      # default depth 3
```

Transitive closure in both directions, labeled: *upstream* (what breaks if
this changes) and *downstream* (what this depends on). A bare name resolves as
for `node` and the nodes it passed over are listed under `alternatives`
(absent when none).

### `impact-intersection`

```bash
logos impact-intersection --item <ID>=<SYMBOL>[,<SYMBOL>...] \
                          --item <ID>=<SYMBOL>[,<SYMBOL>...] [--depth <N>] [--repo <MEMBER>]
```

Which planned work items collide, and on what. Give each work item the symbols
it *intends to change*; the command reports every pair whose transitive impact
sets intersect — naming the shared symbols — and every pair that is safely
parallel. Ask it **before** scheduling work in parallel, not after.

```bash
logos impact-intersection --item "S-341=java_http_client_call,http_client_crates" \
                          --item "S-343=typescript_http_client_call" --json
```

`--item` repeats and is required; repeating the same id accumulates its symbols
(`--item A=f --item A=g` is one item naming both), which is also how to pass a
symbol containing a comma. Only the first `=` splits, so a symbol may contain
one. `--depth` bounds each impact set exactly as [`impact`](#impact) does
(default 3); at depth 0 only directly-declared symbols can collide.

An item's impact set is its resolved symbols plus everything upstream and
downstream of them within the depth bound — the same traversal `impact` runs, so
the two answers cannot disagree. Each shared symbol names, in `declared_by`,
which of the two items intended to change it directly; a symbol both items only
*reach* has an empty `declared_by` and is the weaker signal.

The payload states its own coverage limits (`coverage`): impact is computed over
the indexed graph, so an unindexed surface cannot contribute an intersection and
a `safe_parallel` verdict is bounded by what is indexed rather than proof of
independence. `coverage.unresolved` names every declared symbol the graph does
not know (with "did you mean" suggestions), `items_without_resolved_symbols`
names items that are disjoint *by construction*, and a name matching several
symbols is warned about rather than silently disambiguated.

Malformed items and unknown symbols are warnings on an exit-0 payload, never
errors; omitting `--item` entirely is a usage fault (exit 2).

### `precedent`

```bash
logos precedent <SYMBOL|FILE> [--limit <N>] [--repo <MEMBER>]   # default 20, capped at 100
```

Which existing code plays the same structural role as the thing you are about to
write — *"show me the sibling that already does this"*. Ask it **before** writing
new code, when a plan has already settled the scope and what is left is
precedent.

```bash
logos precedent JavaHttpClientCapture --json
logos precedent logos-core/src/lang/java.rs --limit 5
```

**Analogy is three named graph facts, never a score.** Every result says which
of them it matched and through which nodes:

| Facet | What it means |
|---|---|
| `shared_supertype` | The result implements or extends a trait/interface/superclass the target also does. |
| `shared_registration` | Some third node *does something with* both the result and the target the same way — a registry, dispatcher, factory, route table or signature that calls, instantiates, references, routes to, or is typed by both. This is the facet that finds sibling arms of one capability. A module's `use` list is deliberately **not** a registration: admitting it made every pair of co-imported symbols "analogous". |
| `shared_callee` | The result and the target call the same functions — a matching call shape. Counted from **2** shared callees up; one shared helper is coincidence, and the candidates that threshold drops are counted in `coverage.dropped_single_callee_matches`. |

Results are ranked lexicographically by counted facts, all of them printed in
each result's `rank`: number of distinct facets matched, then shared supertypes,
then shared registrations, then shared callees, then canonical symbol ascending.
No weighting and no composite score — the order can be re-derived by hand from
the payload, which is exactly what a similarity score cannot offer. The rule
itself rides on every answer, in `notion` and `ranked_by`.

The target is resolved as a **symbol first, then a project-relative file** (a
`./` prefix normalises). A file target compares every symbol the file defines;
the results are still nodes, so a sibling *file* shows up as a cluster of its
symbols, each naming its file.

**An empty answer always states its reason** rather than relaxing the notion into
a low-confidence guess. `empty_reason.code` is one of a closed set:
`target_unresolved` (with "did you mean" suggestions), `graph_empty`,
`target_absent_from_view` (a documentation or config node, which the code graph
excludes by construction), `no_structural_anchors` (the target implements
nothing, is registered by nothing and calls nothing), `anchors_are_unshared` (it
has structure, but nothing else shares it), and `anchors_are_ubiquitous` (its
only structure is shared with everything — the opposite claim, and it calls for
a different next step) — plus `query_failed` and `results_unavailable` on the
two degraded paths.

`coverage` states the limits: which symbols were compared, how many candidates
were considered, and — under `ubiquitous_anchors` — which shared nodes were
discarded for linking more than 200 nodes, since a helper called from everywhere
is not evidence of analogy. Unknown targets and empty answers are exit-0
payloads, never errors; omitting the target entirely is a usage fault (exit 2).
### `branch-overlap`

```bash
logos branch-overlap --ref <REF> --ref <REF> [--ref <REF>...] \
                     [--base <REF>] [--merge <REF>] [--repo <MEMBER>]
```

Which git refs collide, and what a merge did not carry. Given the refs about to
be merged, reports every symbol more than one of them modifies, naming the refs.
A clean merge is not a complete merge — ask this before integrating parallel
branches, and again after.

```bash
logos branch-overlap --ref sprint-63-I3-S1 --ref sprint-63-I3-S2 \
                     --ref sprint-63-I3-S3 --merge main --json
```

`--ref` repeats and is required; each is passed to git verbatim, so a branch,
tag, or commit all work. `--base` pins the comparison point (default: the
merge-base of the supplied refs). `--merge` states a merge result to check the
refs' work against.

Each contended symbol carries two lists. `modified_by` names the refs whose
changes land inside it. `absent_from` names the refs in the same set that do
**not** touch it — a shared append point some siblings reached and others did
not is what a silent drop looks like from the outside, and it is the signal that
was missing when five Sprint 63 branches contributed to one capability roster
and two of them never joined it.

With `--merge`, the payload also carries `merge.lost_symbols` (symbols a ref
modified that the merge result does not change at all) and `merge.lost_files`
(whole files the merge never took — the only report that can speak for content
the index holds no symbol for).

The payload states its own coverage limits (`coverage`): changed line ranges are
attributed to the symbol spans of the *indexed* snapshot, so a symbol outside
the indexed set cannot be reported. `files_without_indexed_symbols` names
changed files the graph holds nothing for, `files_with_drifted_spans` names
files whose content at a ref differs from the snapshot the spans came from,
`unattributed_hunks` counts the changes that landed in no symbol, and
`unresolved_refs` names refs that are not commits.

Unresolvable refs, a project that is not a git repository, and fewer than two
refs are all warnings on an exit-0 payload, never errors; omitting `--ref`
entirely is a usage fault (exit 2).

### `affected`

```bash
logos affected <FILES>... [--tests-only]
```

Whole reverse-transitive closure at file granularity: given changed files,
which files are affected, ordered nearest-first. Unknown paths are reported
in an `unknown` list — never an error. `--tests-only` narrows the closure to
test-convention paths (the CI use case). Leading `./` is normalised.

## Documentation graph

Markdown documentation is indexed as first-class graph nodes (`DocFile`,
`DocSection`, and — on swe-skills repos — typed `Requirement`/`Adr`/`Story`
nodes) with doc→doc and doc→code edges. Indexing is on by default and
configured by the `[documentation]` table in
[configuration.md](configuration.md#documentation--indexing-markdown). Doc→code
links obey the same **never-fabricate** rule as code: an ambiguous mention
resolves to *no* edge and stays in the unresolved-reference ledger. Documentation
is **metric-neutral** — adding or removing it never moves the quality signal
(see [metrics.md](metrics.md)).

### `implements`

```bash
logos implements <DOC>
```

Lists the code symbols a documentation node points at over doc→code edges —
"which code implements this requirement / heading". `<DOC>` is a documentation
node, requirement, ADR, or heading, given either as a canonical symbol or a
human-facing name (e.g. `FR-DG-06`, or a heading title). The inverse of
[`referencing-docs`](#referencing-docs).

### `referencing-docs`

```bash
logos referencing-docs <SYMBOL>      # alias: referencing_docs
```

Lists the documentation sections that reference a code symbol — the docs a
change to that symbol may oblige updating. The inverse of
[`implements`](#implements).

## Observability

### `stats`

```bash
logos stats [--window <DAYS>]                           # default 7
```

Aggregated local telemetry: calls per tool split by surface, ok-rates, latency
p50/p95/p99, and reads/tokens-saved
estimates. `--json` also carries `activity_by_day` (a per-UTC-day activity series
over the window, oldest-first) and `calls_by_origin` (a per-`origin` usage
breakdown, where `origin` is a worktree's branch name or `"main"`). Reads only
`telemetry.db` — works without an index.

**The surfaces.** Three are **process** surfaces — the kind of process the call ran
in — and two are **override-only**, naming an in-process caller that would
otherwise be indistinguishable from the person using the surface it runs inside:

| Surface | Kind | What it means |
|---------|------|---------------|
| `cli` | process | The `logos` binary |
| `mcp` | process | The `serve --mcp` stdio server |
| `web` | process | The `serve --ui` dashboard — a query a person issued through the SPA |
| `watcher` | override | The debounced filesystem watcher, which runs inside the `serve --mcp` process |
| `shell` | override | The dashboard's own chrome — the app header re-reads the graph-state readout on every client-side navigation, so its events are requests the app made incidentally rather than ones you asked for |

The override-only distinction is the point: without it, `"the dashboard's own
chrome read the graph N times"` and `"a developer did"` sum into one figure that
answers neither question. These are the surfaces a default build emits.

Since logos 1.4.13 every event also carries an opaque **per-process
`session_id`**, recorded independently of `surface` and `origin` and computed
once at init, never on the hot path. It exists because `origin` is a *branch*
name: without it the store can count calls but not sessions — every process in
one worktree collapses into a single bucket and all primary-checkout work
collapses into `main`, which is why a question as basic as "what fraction of
sessions made at least one navigation call?" was previously unanswerable. The
value carries **no user, machine or account identity**. It arrives via a
forward-only v3 schema migration that applies cleanly over an existing v2
`telemetry.db`; rows written before the migration read as **unattributed**
rather than being folded into an arbitrary session (S-307, CR-091). **Self-referential reads are excluded
from every figure** — totals, per-tool, daily series, origin split, latency, and
the estimate — because a request whose subject is Logos's own state (`stats`
reading the telemetry store, the shell's `status` readout) measures the
measurement, not tool value, and a request the application's own chrome makes
(the app header's graph-state readout) is app overhead rather than a question
anyone asked. The exclusion is per *event* and keyed on two axes — the
**tool**, so it applies on every surface (a CLI `logos stats` is no less
self-referential than a dashboard render, while a graph query issued *through*
the dashboard counts normally), and the **surface**, for a read whose caller is
the application's own chrome rather than a person (the app header's `status`
readout, which navigation re-issues and nobody asks for).

**Attribution: which tools, from where, of what kind.** `--json` carries two
further projections:

- `calls_by_tool_origin` — the tool × origin cross-tab: per-tool counts split by
  the same `dev`/`main` buckets as `calls_by_origin`, so *"which navigation came
  from dev panes?"* is answerable from the payload. Neither older breakdown can
  answer it alone (`calls_by_tool` has no origin; `calls_by_origin` has no tool).
- `calls_by_class` — the same cells rolled up to the tool class, which is a
  sprint dogfood table without hand-classifying anything.

Every tool carries a `class` — `navigation`, `quality-gate`, `session-gate`,
`engine-internal`, `read-model`, or `unregistered` for a name written by an older
build that the current registry no longer knows (counted as recorded, with its
class honestly unknown). The `class` on `calls_by_tool` covers raw events *and*
rolled-up days; the two projections above do not (see below).

`attribution_coverage` states those limits in the payload rather than leaving
them to this page: `raw_events_only` (always true — `daily_rollup` is keyed
`(day, surface, tool)` and carries no `origin`; `calls_by_origin` shares this
limit, the totals and `activity_by_day` do not), `requested_window_days` vs
`covered_window_days` with `truncated_by_retention`, and
`legacy_null_origin_folds_into_main`. A row with no `origin` was written by
whichever surface was running before the origin stamp shipped (`cli`, `mcp`,
`web` or `watcher` — the web surface is older than the stamp), so that period
is a distinct population whose dev/main split is unknown, not a CLI+MCP-only
one. `notes` carries the
same limits as display-ready prose, and the Statistics tab renders them beside
the cross-tab/class figures rather than on a separate help page (S-306).

`covered_window_days` is a **guaranteed floor, not a measurement**: raw events
are kept ~90 days, so a 365-day request is guaranteed only the most recent 90 in
these projections — but pruning is flush-triggered rather than time-driven, so a
long-lived `serve` process may still hold older raw events and cover more.
Under-stating is deliberate; never read the figure as the coverage actually
achieved.

**Who invoked the calls.** `calls_by_tool_invoker` splits each tool's calls by
the invoker the calling process declared in `LOGOS_INVOKER`: the plugin's hooks
declare `hook:stop`, `hook:pre-tool-use` and `hook:session-start`, its skill
scripts `skill:logos-review` and `skill:logos-sec-review`. `invoker` is `null`
for a call that declared none — a direct CLI call, an MCP tool call the agent
made itself, a web request — and for a value outside the `hook:<name>` /
`skill:<name>` shape. It is the figure that judges the plugin by what its hooks
and skills caused rather than by call volume. It has the origin cross-tab's
limits (raw events only), and one more, stated as
`null_invoker_includes_pre_stamp_rows`: events written before the stamp existed
are `null` too, so the historical `null` bucket holds hook and skill calls.

**What the calls answered.** Every usage cell — in `calls_by_tool`,
`calls_by_tool_origin`, `calls_by_class`, `calls_by_origin` and
`activity_by_day`, and the same cells of the workspace aggregate — carries
`answered_calls` and `classified_calls` beside `calls` and `ok_calls`. A
classified call is one whose telemetry event recorded an outcome: `answered`,
`empty` (resolved, legitimately nothing), `unresolved` (could not answer) or
`failed`. Today the relational tools `callers`, `impact`, `precedent` and
`affected` are classified; every other tool records none. **No rate is in the
payload** — divide `answered_calls` by `classified_calls`, never by `calls`,
or every unclassified tool drags the figure towards zero. A cell whose
`classified_calls` is `0` carries `outcome_absence: "none recorded"` (otherwise
`null`) — render that, never `0%`. The outcome arrives with a forward-only v4
migration; events and rolled-up days written before it stay unclassified and
are never back-filled (S-445, FR-OB-14).

**Telemetry is repo-global and durable across worktrees.** The store lives at
the **primary** repository's `.logos/telemetry.db`, resolved via
`git --git-common-dir`. A command run inside a linked git worktree writes
*through* to that primary store (it never creates a `telemetry.db` inside the
worktree), so usage recorded during a dev-session worktree survives
`git worktree remove` — and `logos stats` from any worktree reports
repository-wide usage, not an empty per-worktree slice. From the primary
checkout, behavior is unchanged. Independent clones (a separate
`--git-common-dir`) remain independent stores by design. Legacy rows written
before the `origin` column existed read as `"main"`. Note that
`sum(calls_by_origin)` can be *less* than `calls_total` over a window old enough
to reach aged-out daily rollups (rolled-up days carry no `origin`) — an honest
gap, never silently reconciled.

### `languages`

```bash
logos languages --json
```

The registered grammar table: name, extensions (and `filenames` for
basename-claimed formats like `Dockerfile`/`Makefile`), module separator,
capabilities, tree-sitter ABI version, and an `artifact` flag — plus, for each
code language, its declared **reach** (`reach.level`: `resolved`, `partial`,
`same-file` or `symbols`; `reach.cross_file`: the relations it binds across
files — see [usage.md](usage.md#language-support-what-each-language-binds-across-files))
and any grammars skipped for ABI mismatch (`skipped` should be empty). A full `lang-all`
build lists 24 plugins: the twelve code languages (thirteen grammar rows —
TypeScript and TSX/JSX register separately), `markdown`, and the ten
`artifact: true` config/infra grammars (yaml, json, toml, dockerfile, makefile,
shell, protobuf, graphql, terraform, sql).

## Serving

### `serve`

```bash
logos serve --mcp [--project <PATH>]                 # MCP server over stdio (AI agents)
logos serve --ui [--port <N>] [--project <PATH>]     # localhost web dashboard (default port 4983)
logos serve --mcp --ui [--port <N>]                  # both surfaces in one process
```

At least one of `--mcp` / `--ui` is required. `--mcp` starts the MCP server
over stdio (see [usage.md](usage.md#setting-up-the-mcp-server-ai-agents) for
host setup, the 33-tool surface, and the stdout-purity / clean-teardown
guarantees). `--ui` starts the localhost web dashboard (see
[usage.md](usage.md#the-web-ui-dashboard)); it is not offered in the slim build
(see the [build matrix](usage.md#build-matrix--dashboard-or-slim-headless)). The dashboard is a single embedded React SPA served at `/`,
client-side routed over a same-origin `/api/v1/*` JSON read-model API
([ADR-43](../specs/architecture/decisions/ADR-43.md)); the whole app is built at
build time and embedded in the binary, so a page load fetches nothing from the
network, and it binds `127.0.0.1` only with a self-only CSP on every response. It
is read-only except a small, enumerated set of intent-guarded mutating routes —
the config save and apply routes (`/config/save`, `/config/apply`), the workspace
manifest and config saves, the on-demand `verify` (`/api/v1/verify`), and the
Security page's scan start and cancel (`/api/v1/security/scan`,
`/api/v1/security/scan/cancel`), its scanner install start and cancel
(`/api/v1/security/install`, `/api/v1/security/install/cancel`) and its suppression writes
(`/api/v1/security/suppress`, `/api/v1/security/unsuppress`); every other non-GET
request is answered `405`. Combined
`--mcp --ui` runs both on one engine and one watcher: stdout stays JSON-RPC-clean
for the MCP host while the web surface logs to stderr.

**Stopping it.** Ctrl-C (SIGINT) or SIGTERM stops `serve --ui` cleanly, and so
does the MCP host disconnecting under `--mcp --ui`. A Security page scan or
install still running is cancelled first: its `curl`, `uv` or scanner processes
are killed and the partial download is removed. `serve` waits up to 10 seconds
in all for those jobs to end, then exits 0. `serve --mcp` alone stops on Ctrl-C
or SIGTERM too, with exit 0. A scan an MCP client started through the
`security_scan` tool is not one of these jobs, but however `serve` stops, its
scanners are killed as well. A `serve`
killed by SIGKILL, or by any other signal (SIGHUP when its terminal closes), does
none of this, so its download or scanner runs on to its own end; the next
install of any scanner discards what it left in the staging directory.

A **debounced filesystem watcher** runs alongside the engine: file changes
are coalesced over a 300 ms window (configurable via `[watcher] debounce_ms`
in [configuration.md](configuration.md)) and folded into a single `sync`
batch. Navigation and governance responses always reflect the current on-disk
state; the reconcile backstop in every quality command is the correctness
safety net regardless.

**Workspace mode (context-aware `--ui`).** When `serve --ui` starts inside a
[workspace](#xservice-workspace-federation-queries) (a `logos.workspace.toml`
found up-tree, see [configuration.md](configuration.md)), it serves **workspace
mode**: the shared `/api/v1/*` surface runs against the default member (only that
member is warmed eagerly at startup; the rest are built lazily on first touch),
and a `/api/v1/workspace/*` fan-out surface exposes the cross-service
read-models (`status`, `route-providers`, `search`, `callers`, `impact`,
`reachability`, `check`, `statistics`) to the frontend. In a plain repo with no manifest up-tree, `serve --ui` is byte-for-byte
as before — the `/api/v1/workspace/*` routes answer `404` and no member registry
is allocated. Pass **`--standalone`** to force single-repo focus even under a
manifest. Every response (workspace or single-root) still carries the unchanged
self-only CSP and binds `127.0.0.1` only.

```bash
logos serve --ui --standalone                        # force single-repo focus even under a workspace manifest
curl 127.0.0.1:4983/api/v1/workspace/status          # (workspace mode) per-member freshness + coverage
curl 127.0.0.1:4983/api/v1/workspace/reachability    # app-wide reachability, promotions-only and bounded by default
curl '127.0.0.1:4983/api/v1/workspace/reachability?repo=<member>'   # scope it to one member
curl 127.0.0.1:4983/api/v1/workspace/check           # workspace governance findings (advisory)
curl 127.0.0.1:4983/api/v1/workspace/statistics      # telemetry summed over members, engine-free
curl 127.0.0.1:4983/api/v1/workspace/manifest        # the manifest as an editable document (content + fingerprint + parse verdict)
```

The workspace **Config** view writes the manifest through the
intent-guarded `POST /api/v1/workspace/manifest/save` — see [configuration.md](configuration.md#editing-the-manifest-from-the-app).

---

## Workspace federation queries

These commands answer **cross-service** questions over a
[workspace](configuration.md) — a `logos.workspace.toml` manifest at a parent
folder listing sibling member repos (set up with
[`init --workspace`](#init--i---hooks---workspace---yes---exclude-glob)). They compute an **in-memory overlay** over each
member's own graph: nothing is persisted, no graph union is ever written, and no
`NodeId` crosses a member's database boundary ([ADR-52](../specs/architecture/decisions/ADR-52.md)).
Every answer is repo-qualified; a member whose engine fails to start degrades to
a per-member `error` rather than aborting the query. In a plain (non-workspace)
repo these commands report that no workspace was found (exit `3`).

The same thick-core read-models back every surface identically — the `logos
xservice` / `logos workspace` CLI here and the `/api/v1/workspace/*` HTTP
endpoints under [`serve --ui`](#serve) (both live now) — so the
coverage/service-map/impact numbers are always the same whichever way you reach
them. The matching `xservice_*` and `workspace_*` MCP tools are served by
`serve --mcp` started in a workspace (the build with the web UI): the host then
sees 42 tools, the 33 single-root ones plus these nine, and the read-only tools
take `repo` ([usage.md](usage.md#asking-about-one-workspace-member-repo)).
`--standalone`, and the slim headless build, serve the one repository instead.

**One exception, since S-474:** `xservice type-refs` and the
`via_type_reference` section of `callers`/`impact` are on the CLI and its MCP
twins only. `/api/v1/workspace/callers` and `/api/v1/workspace/impact` answer
without the section — exactly the bytes they answered before — and there is no
`type-refs` web route.

### `xservice` (workspace federation queries)

```bash
logos xservice route-providers [--repo <MEMBER>] [--json]   # the service map: cross-service route bindings
logos xservice search <QUERY> [--kind <K>] [--limit <N>] [--repo <MEMBER>] [--json]
logos xservice callers <SYMBOL> [--limit <N>] [--repo <MEMBER>] [--json]
logos xservice impact <SYMBOL> [--depth <N>] [--repo <MEMBER>] [--json]
logos xservice build-deps [--repo <MEMBER>] [--json]          # what each member builds against — never a runtime coupling
logos xservice type-refs [--repo <MEMBER>] [--json]           # which members import each member's types — advisory, never a coupling
```

**Method wildcards.** A provider declared without an explicit verb — Spring's
`@RequestMapping` with no `method =`, Express's `app.all(...)`/`router.use(...)` —
registers the method token `ANY` and matches **every** verb. When both a wildcard
and an exact-method provider exist for the same template, the **exact-method
provider wins**; a consumer only becomes ambiguous when two providers are equally
specific. Two members owning the same template both via `ANY` leaves the consumer
`ambiguous` with **no edge** — Logos refuses rather than picking one. Expect the
`ambiguous` count to be non-trivial on a real workspace: that is the refusal
working, not a defect.

**Provider paths built from string constants (Java).** A Spring mapping whose
path is a concatenation — `@GetMapping("/users/{" + USER_ID + "}")` — is folded to
one literal and becomes a `route` like a written path. The constant may be a
`static final String` of the same class, a field of the same interface, or a
constant of another type **in the same member**, reached through a single-type
`import static a.b.Type.X;` or a qualified `Type.X`. A path that cannot be folded
(a method call, a non-final field, a wildcard static import, a constant from a
library or another member, two visible declarations of one name) produces **no**
route and is counted in the run's `routes_not_composed`, so it is never dropped
silently. Editing only the file that declares the constant re-folds the routes
that use it on the next `logos sync`. Kotlin concatenated paths are neither folded
nor counted yet.


- **`route-providers`** — the workspace service map: every cross-service binding
  where one member's declared route provides for a reference in another
  (`BridgeEdge`s matched exactly-one on a portable `route_key`; two providers ⇒
  ambiguous, no edge). As of Sprint 56 the consumer side includes not just
  declared cross-service contracts but **static HTTP client calls** — an outbound
  `"METHOD /template"` call in one member binds a matching `Route` in another
  through the same `route_key`. Since logos 1.4.13 a path **composed from a
  committed configuration key** binds through that key's committed value too
  (S-420, CR-133) — the same admission the coverage tier already made, so the
  two tiers no longer classify one call site two ways; a path composed from a
  value resolved only at runtime (`base-url-runtime`), and a genuinely
  non-static path, still stay unbound with a reason, never approximately
  matched. An edge admitted this way carries `from_value: config-bound` and is
  never reported as though it had been observed at the call site (ADR-64); see
  the config-bound section below for the estate reading. `--repo X` scopes to
  routes *provided by* member `X`.

  **Declared contracts beside the bindings (since S-461).** When a member holds
  a vendored spec, the answer gains two keys **beside** `providers`, never among
  them: `declared_contracts` — each spec document a member holds and does not
  implement, naming the member whose own spec it is (document identity) or a
  named external — and `bound_external` — each `no-provider-in-workspace` REST
  call judged against the externals its own member declares, under a committed
  base path (see
  [Vendored specs](configuration.md#vendored-specs--declared-contracts-and-named-externals)).
  Both are **declared by vendored specs, not observed calls**: no row is a
  bridge edge, and `providers` is byte-identical with or without them. They are
  the same bytes `workspace status --json` carries under `coverage`, each with
  its `headline` beside its denominator. They are **workspace-wide under
  `--repo`** — a declared contract is not a provided route — and a scoped answer
  says so in `declared_scope_note`. Each key is absent when there is nothing to
  report (no vendored or `mock`-held spec; no named external a member
  declares), so such a workspace answers exactly the keys it did before
  (`providers`, or `scope` and `providers`). The MCP twin
  `xservice_route_providers` returns the same payload. The web
  `/api/v1/workspace/route-providers` route stays edges-only; the service map
  reads the relations from `workspace/status`.

  ```jsonc
  // logos xservice route-providers --repo webmail --json (abridged; 83-member estate)
  { "scope": "webmail", "providers": [],
    "declared_contracts": {
      "headline": { "declared_contract_pairs": 6, "to_member": 1, "to_external": 5,
                    "documents": { "documents": 41, "own": 11, "vendored": 7, "partial": 0, "unjudged": 0, "mock": 3, "documentation": 20 },
                    "named_externals": 5, "identity_collisions": 0, "resolved_ties": 15,
                    "summary": "6 declared contract pairs (1 by document identity, 5 to named externals) from 7 vendored of 41 spec documents; 5 named externals; 15 contract-surface ties resolved by document identity; declared by vendored specs, never observed calls" },
      "contracts": [
        { "holder": "webmail", "document": "mailbox-aggregator.yaml", "provenance": "vendored-spec",
          "target": { "kind": "member", "member": "mailbox-aggregator-api", "document": "src/main/resources/openapi/v1.yaml", "shared": 31, "total": 31 } },
        { "holder": "pecserver-facade", "document": "src/main/resources/pec-server/pec-server-api_v1.yaml", "provenance": "vendored-spec",
          "target": { "kind": "external", "external": "pecserver-facade:src/main/resources/pec-server/pec-server-api_v1.yaml", "name": "PSS" } }, … ],
      "externals": [ { "id": "pecserver-facade:src/main/resources/pec-server/pec-server-api_v1.yaml", "name": "PSS",
                       "copies": [ … ], "declared_by": ["pecserver-facade", "webmail"], "stand_ins": ["pecserver-mock"] }, … ],
      "collisions": [], "resolved_ties": [ … ] },
    "bound_external": {
      "headline": { "bound_external": 21, "no_provider_rows": 32,
                    "accounting": { "bound_external": 21, "no_declared_external": 10, "external_not_declared_by_member": 0, "no_base_key": 0,
                                    "base_path_uncommitted": 0, "base_paths_disagree": 0, "suffix_only": 0, "no_match": 1, "several_matches": 0 },
                    "summary": "21 of 32 invocation no-provider-in-workspace REST rows bound to a named external their own member declares (refused: 10 no declared external, 1 no match); declared by vendored specs, never a cross-service edge, and outside egress_resolution" },
      "rows": [
        { "from": { "member": "pecserver-facade", "symbol": "…PecServerApiRestClient#activateMailbox()." },
          "target": "PUT ${pecserver.uriactivatemailboxpath}", "state": "bound-external",
          "external": "pecserver-facade:src/main/resources/pec-server/pec-server-api_v1.yaml", "name": "PSS",
          "document": "src/main/resources/pec-server/pec-server-api_v1.yaml", "operation": "PUT /prov/domain/{}/user/{}",
          "base": { "path": "/prov", "origin": "deploy-overlay",
                    "sources": [ { "file": "deploy-coll/values.yaml", "key": "envfrom.pecserverbaseurl" }, … ] } },
        { "from": { "member": "pecserver-facade", "symbol": "…PecServerApiRestClient#getUnreadMails()." },
          "target": "GET ${pecserver.urigetunreadmails}", "state": "refused", "reason": "no-match" }, … ] },
    "declared_scope_note": "`--repo webmail` scopes `providers` to routes webmail provides; `declared_contracts` and `bound_external` are workspace-wide — a declared contract is not a provided route" }
  ```

  A bound row's `base.origin` is `deploy-overlay` or `application-config`, and
  `sources` names every file and key that commits the base path; a `path` of
  `""` is a base URL with no path. A refused row carries a kebab-case `reason`,
  with detail where there is some (`paths` for `base-paths-disagree`, `sources`
  for `base-path-uncommitted`, `operation` and `base_path` for `suffix-only`,
  `matches` for `several-matches`). A bound row stays
  `no-provider-in-workspace` in `workspace status`'s coverage. `route-providers`
  pays one extra coverage walk to compute the two relations (on the 83-member
  estate the `--json` payload grows from 104,984 to 135,708 bytes).
- **`search`** — full-text search fanned across every member, each hit tagged
  with its member. `--repo X` scopes the fan to member `X`.
- **`callers`** — direct callers of a symbol per member, plus the cross-service
  callers stitched across bridge edges (a consumer in another member that binds
  the symbol's route). For a type another member imports (see `type-refs`
  below), a `via_type_reference` section lists each importer — member, file and
  line — as a class-grain caller, each entry tagged `"reached": "via type
  reference"` with the reference under `via`.
- **`impact`** — transitive impact per member, extended across bridge edges: a
  handler reachable only via a matched cross-service call is included, tagged
  with the bridge edge it was reached through. For a type another member
  imports, a `via_type_reference` section carries, per bound reference (one
  per import row, so a file importing the type and a static member of it is
  listed twice, with one closure), the importing
  member's [`affected`](#affected) closure of the importing file — `changed` is
  that file, `affected` every file depending on it there, within `--depth`
  hops like the rest of the answer — or that member's
  `error` when its store will not open.

  Both sections sit **apart from** `cross_service` and are never merged with
  it: a type reference is not a bridge edge, and `unresolved_egress` counts
  neither. A third, `via_asserted_link`, lists the edges a valid `[[links]]`
  entry asserts, marked `"reached": "(asserted)"` — also apart, also uncounted
  (see [where a valid link shows up](#links--what-each-declared-links-entry-became)). Name the type by its node (``…/`Dto.java`/Dto#``) or by its dotted
  name (`com.acme.lib.Dto`); an Avro-declared type has no node, so its dotted
  name is the only way to reach it. Like the bridge tier, the match keys on
  the declaring member as well as the symbol: under `--repo` only the types
  that member declares are reached, on both the node and the dotted-name
  paths, whichever member imports them; unscoped, the entries are grouped by
  declaring member. A reference whose declaring member is not in the
  workspace is never dropped — it is listed apart, in
  `type_reference_unattributed`, under any scope. A member whose declared types could
  not be read (not yet extracted after an upgrade, or a store that will not
  open) holds references nobody can reach, so both answers then carry
  `type_reference_unread`, naming each such member with its reason — an absent
  `via_type_reference` beside it is not "nothing imports this". A symbol no
  type reference names, in a workspace whose members were all read, answers
  exactly the bytes it did before. The reach is **file grain**: a
  Java/Kotlin import is held by the importing file, so every importer of the
  type is reached, whichever of its methods the importer calls.
- **`build-deps`** (since S-464) — what each member **builds against** and what
  is **built against it**, joined from the members' Maven/Gradle manifests
  ([FR-WS-33](../specs/requirements/FR-WS-33.md)). Every member read gets a
  `builds_against` and a `built_against_by` list (a member with no edge is
  listed with both empty); each row names `kind` (`parent`, `dependency`,
  `managed` — a version pin — or `bom-import`), `scope` as declared (`null`
  when the manifest declares none; never defaulted to `compile`), `artifact`
  (`groupId:artifactId`), `references`, and `platform: true` when its target is
  a member declared [`kind = "platform"`](configuration.md#kind--platform--build-hubs).
  The workspace `headline` rides beside the rows — `build_dependency_pairs` by
  kind with its denominator, the same section `workspace status` carries — and
  `cross_context` lists the members depending on the model libraries of **two
  or more** bounded contexts, each library named with its producer. A context is
  read off its model library's coordinate: an artifactId `kafka-models` names the
  groupId's last segment (`com.acme.archive:kafka-models` is the `archive`
  context — the only shape on the reference estate, whose repositories are named
  `archive-kafka-models` and so on), and an artifactId `<context>-kafka-models`
  names its prefix. Only `dependency` rows count: a parent POM that pins
  every context's models under `managed` is not a hint. `--repo X` scopes the
  rows and the hint to member `X` while the headline stays workspace-wide; a
  name that is not a member read (unknown, or its build facts could not be
  read or are not yet extracted) answers an empty `members` list **with** a
  `scope_note` stating which,
  never a silent "no edges". The MCP twin is `xservice_build_deps`, and the web
  service map draws the same relation behind a legend toggle that is off by
  default.

  **A build dependency is never a runtime coupling**
  ([BR-58](../specs/software-spec.md#327-workspace-federation)): no row is a
  bridge edge, none enters `route-providers`, `callers`, `impact` or any
  coverage figure, and a member that builds against another is not thereby
  coupled to it at runtime.

  ```jsonc
  // logos xservice build-deps --repo archive-kafka-models --json (abridged)
  { "scope": "archive-kafka-models",
    "headline": { "build_dependency_pairs": { "pairs": 148, "parent": 51, "dependency": 84, "managed": 15, "bom-import": 0 },
                  "summary": "148 pairs (…) built against another member, from 184 of 1728 referenced artifacts (…); a build dependency, never a runtime coupling", … },
    "members": [ { "member": "archive-kafka-models", "builds_against": [],
                   "built_against_by": [ { "from": "archive-feeder", "to": "archive-kafka-models", "kind": "dependency",
                                           "scope": null, "artifact": "com.sourcesense.poste.pec.archive:kafka-models", "references": 1 }, … ] } ],
    "cross_context": [] }
  ```
- **`type-refs`** (since S-474) — the cross-member type references
  ([FR-WS-35](../specs/requirements/FR-WS-35.md)): per **provider** member, the
  types other members import from it, each with its `owner` (the declaring
  source file and node, or the Avro schema, which has no node) and every
  importer under `importers` — `member`, `file`, `line`, the importing
  declaration's `symbol`, `naming` (`exact`, or `enclosing` for a static-member
  or nested-type import), `form` (`import` or `type-use`) and `evidence` (`via:
  build`, or `via: collision` with the `artifacts` named). The `headline` is
  the `type_reference` section of `workspace status` — `type_reference_pairs`
  beside every row considered and the members read — and stays workspace-wide
  under `--repo`. Unscoped, only members whose types another member imports are
  listed. `--repo X` lists provider `X` alone, with an empty `types` list when
  nothing imports its types; a name that is not a member read (unknown, or its
  declared types could not be read or are not yet extracted) answers an empty
  `providers` list **with** a `scope_note` stating which, and exits 0. A
  `type_only` match is never an importer here: it stays in the headline's
  `type_only` list. The MCP twin is `xservice_type_refs`; there is no web
  route yet.

  **A type reference is advisory, never a coupling**
  ([BR-60](../specs/software-spec.md#327-workspace-federation)): no row is a
  bridge edge, a resolved call or a build dependency, and nothing in
  `coverage` or `build_dependency` counts it.

  ```jsonc
  // logos xservice type-refs --repo lib --json (the fixture in cli/tests/xservice_type_refs.rs)
  { "scope": "lib",
    "headline": { "type_reference_pairs": 1, "build_pairs": 1, "collision_backed_pairs": 0, "triples": 1,
                  "rows": { "considered": 5, "imports": 2, "type_uses": 3, "bound": 1, "type_only": 1, "unqualified": 3, … },
                  "members": { "members": 5, "read": 5, … },
                  "type_only": [ { "from": "stray", "to": "lib", "types": ["com.acme.lib.Dto"], "references": 1 } ],
                  "summary": "1 member pairs (1 build · 0 collision-backed) bind 1 of 5 unresolved …" },
    "providers": [ { "member": "lib", "types": [ {
        "fqn": "com.acme.lib.Dto",
        "owner": { "member": "lib", "origin": "source", "declared_in": "src/main/java/com/acme/lib/Dto.java",
                   "symbol": "logos . example.com/lib . src/main/java/com/acme/lib/`Dto.java`/Dto#", "kind": "class" },
        "importers": [ { "member": "app", "file": "src/main/java/com/acme/app/App.java", "line": 3,
                         "symbol": "logos . example.com/app . src/main/java/com/acme/app/`App.java`/",
                         "naming": "exact", "form": "import", "evidence": { "via": "build" } } ] } ] } ] }
  ```

`--repo` constructs only the member engines the answer needs (a one-shot never
builds all N, [NFR-PE-10](../specs/requirements/NFR-PE-10.md)). All `--json`
output is a single machine-clean line.

**Each bridge answer names the members it read.** `route-providers`, `callers`
and `impact` — on the CLI, the `xservice_*` MCP tools and
`/api/v1/workspace/*` alike — carry `member_reads`: `read`, the members this
answer read, and `unread`, every member it needed and could not read, each with
its reason (absent when none). A member is never left out silently, with the one
exception stated below.

```json
"member_reads": { "read": ["api", "web"], "unread": { "audit": "starting the engine for workspace member \"audit\": …" } }
```

What gets read depends on whether the bridge has already answered once. Its
**first** answer reads every member — an edge binds the *sole* provider of a
key, and only every member's surface can say a provider is the sole one — so a
CLI one-shot, whose bridge starts empty, always lists every member. A
long-running surface (`serve`, the MCP server) keeps the bridge's
answer, and each later answer only checks the members whose index can have
changed since — those with a live engine — and retries any it could not open
before or whose store file has gone; the check starts no other member. When a
stamp it checks has moved, the answer recomputes, and a recompute reads every
member again. `callers` also reads the members of its per-member fan-out and
`impact` those of its seed — the one member `--repo` names, or every member
without it — and `impact` the far member of each edge it crosses. The type-reference section and the declared relations
of `route-providers` state their own coverage (`type_reference_unread`, the
headlines' denominators) and are not counted in `member_reads`.

The exception: a member the bridge has opened before and that has since been
evicted is not reopened by the check while its store file is still there. If
that file no longer opens (corrupt contents, a schema newer than this binary, an
`open` refused), the answer serves the edges last read from it and names it
unread only through a tier that opens it — `callers`' fan-out, `impact`'s seed
and far side. `route-providers` has no such tier, so there that member appears
in neither `read` nor `unread` until the bridge recomputes.

**A reachability answer carries its unresolved residue** ([CR-125], [BR-53]).
`callers` and `impact` answer "what reaches this / what does this reach" across a
boundary, and an **empty** answer there is read as safety. So both — on the CLI,
the `xservice_*` MCP tools and `/api/v1/workspace/*` alike — carry an
`unresolved_egress` block reporting the captured outbound call sites *in scope*
that did not resolve:

| Field | What it says |
|---|---|
| `unresolved_sites` | how many captured outbound sites in scope produced no edge |
| `measured_sites` | the denominator — the same `bound + ambiguous + unbound` egress population `workspace status`'s `egress_resolution` is computed over, so `unresolved_sites = measured_sites − bound` |
| `by_reason` | the per-reason breakdown, most sites first; sums exactly to `unresolved_sites` |
| `no_provider_in_workspace` | sites whose provider is outside this workspace — bucketed **apart**, never counted inside the residue |
| `scope` / `members_in_scope` | which members' egress this covers — `members_in_scope` is how many members the unresolved sites are **spread across**, not the workspace's roster size |
| `covers_all_members` | `false` marks a residue computed over fewer than all members |
| `summary` | the one composed line carrying all of it |

**The block is absent exactly when the residue is zero**, and only then — so an
answer over a fully-resolved scope reads exactly as it always has. Read the two
together: an empty `cross_service` list *beside* an `unresolved_egress` block does
not say "nothing reaches this symbol", it says the question was answered over a
graph missing that many outbound calls. `--repo` scopes the residue to that
member's egress, the same narrowing it applies to the answer's own fan-out. The
residue is advisory — it is never a gate input and moves no verdict or baseline.

**`--repo` names which member's symbol you mean.** A symbol is unique within one
member only: two members that declare the same file and declaration — services
generated from one template, a fork and its original — hold the same symbol
string. So the cross-service tier matches the member *and* the symbol: under
`--repo` it keeps only the edges whose queried end (the provider for `callers`,
the near end for `impact`) is that member's, while the callers and the far side
stay in whatever member they lie. Under a scope the `summary` line names the
population of each half — the resolved count as *that member's symbol, from any
member*, the residue as the member's own egress — so the two are never read as
one figure.

Without `--repo`, a symbol that two or more members hold is answered **grouped
per member, never pooled**: every `cross_service` row already names its
members, `resolved_by_member` gives each holding member's share of the resolved
count (a member that holds the symbol and provides nothing reads `0`), the
`summary` line names the same breakdown, and `ambiguity_warning` names the
members and tells you to pass `--repo` to choose one. A symbol one member holds
answers exactly as before — neither field appears.

The `via_type_reference` section follows the same rule: with `--repo X` it lists
only the references whose owning member is `X`; without it, every entry is listed, its
owner named under `via`. A type reference whose owner names no member of the workspace
cannot be claimed or excluded by any scope, so it is listed under
`type_reference_unattributed` whatever `--repo` says — reported, never dropped.
The section is absent when every reference reaching the symbol has an owner.

[BR-53]: ../specs/software-spec.md#327-workspace-federation
[CR-125]: ../requests/CR-125-an-unresolved-egress-must-not-read-as-an-absence.md

**Cross-service invocation arms.** The bridge binds real runtime invocations, not
just declared contracts, through a pluggable arm contract (each arm normalizes a
call to a portable key and refuses anything non-static — zero approximate binds).
The HTTP client-call arm is live in `route-providers` (above). The gRPC
stub-call arm **ships no capture**: its key normalizer and bridge namespace
exist and would bind `package.Service/Method` FQNs at the ledger tier, but no
language's `invocations.scm` captures a stub call, so nothing reaches them and
the arm binds nothing. It is reported as **honestly absent** rather than as an
empty result ([FR-WS-09](../specs/requirements/FR-WS-09.md)) — do not read a
zero gRPC figure as "no gRPC coupling here". The
message-broker publish/subscribe arm is now **first-class**: a forward-only
migration (schema version 17) admits `Topic` / `Producer` / `Consumer` nodes and
`Publishes` / `Subscribes` edges, so a publish in one member binds every
subscribe on the same topic across members, and the coupling renders as an
explicit topic hop (`A → topic → B`) in the workspace service map rather than an
opaque line. A per-repo topic is visible before any cross-repo match; a graph
with no broker topics is byte-for-byte unaffected by the migration.

The broker arm also recognises a **Kafka Streams topology** — a
`builder.stream(<topic>)…to(<topic>)` chain against a `StreamsBuilder`/`KStream`
receiver (Java, Rust and Go) — as a subscribe/publish pair, alongside the
existing annotation and header-based forms. A topic operand that is a
`@ConfigurationProperties` accessor (bare or qualified, e.g.
`kafkaTopics.getArchiveEvents()`) resolves through the same accessor hop the
HTTP client-call arm uses, and the resolved topic is keyed by its **committed
configured value** rather than the placeholder as written — so a subscribe
declared `${spring.kafka.topics.x}` and a Streams publish resolving to the same
committed value bind across members even though their source text differs.
Since logos 1.4.13 a topic operand that is a **parameter of its enclosing
method** also resolves, through that method's callers — name-, arity- and
receiver-aware, positionally, within the build module, over `src/main` call
sites, up to **two frames**, carrying provenance that names the hop chain. It
**refuses** rather than guesses on a third frame, an out-of-module caller, two
callers passing different keys, a bare variable, or a varargs/explicit-receiver
signature; a test-tree call site (a Mockito `any()`) neither admits nor vetoes
(S-417, FR-WS-26). Where no committed source defines the key, or the operand
does not resolve, the site carries the existing refusal vocabulary
(`topic-not-literal`, `config-key-missing`, `config-placeholder-value`) rather
than a fabricated topic. **Migration
17 is forward-only and irreversible** — a store opened by a Sprint-57-or-later
binary advances `PRAGMA user_version` 16 → 17 and cannot be reopened by an older
binary.

### `workspace status`

```bash
logos workspace status [--json]
```

Per-member freshness (each member's index/sync state, including the durable
`last_full_index_at` described under [`status`](#status) above) plus the
**3-state cross-service coverage summary** — every cross-boundary reference classified
`bound` / `ambiguous` / `unbound`, each unbound one carrying a reason
(`no-provider-in-workspace`, `path-not-composed`, `base-url-runtime`,
`ambiguous`, `topic-not-literal`, `config-key-missing`,
`config-placeholder-value`). The coverage tier is **advisory only** — it
is bucketed separately (`no-provider-in-workspace` never depresses
`spec_conformance_ratio`) and never feeds any member's quality gate
([ADR-53](../specs/architecture/decisions/ADR-53.md)).

##### Each member's coordinate, and the duplicate it refuses

Every member row carries `coordinate`: the repository coordinate the member
resolves to, which is the package slot of every symbol its graph holds (its
[`[project] id`](configuration.md#project-id--the-repositorys-identity), else
the identity derived from its `origin` remote). It is present on every row, a
degraded one included, in human and `--json` output and in
`GET /api/v1/workspace/status`; all three print the one read-model, so they
cannot disagree.

```jsonc
// logos workspace status --json
"members": [
  { "member": "orders-a", "coordinate": "github.com/acme/orders", "open_state": "opened", … },
  { "member": "billing",  "coordinate": "github.com/acme/billing", "open_state": "opened", … }
]
```

Two members that resolve to **one** coordinate — two clones of one remote, or a
copied checkout — would hold the same symbols byte for byte, so the workspace
never folds them. The **later** member in manifest order is reported
`open_state: "degraded"` with `degraded_cause: "duplicate-coordinate"`; its
`degraded_reason` names both checkouts and the override:

```text
workspace member "orders-b" (/ws/orders-b) resolves to repository coordinate
"github.com/acme/orders", which member "orders-a" (/ws/orders-a) already holds, so
it is excluded rather than folded into it. Set a distinct `[project] id` in
/ws/orders-b/.logos/config.toml, then re-open the workspace
```

The first member keeps answering. The duplicate is excluded from every
fan-out, from the bridge and from the coverage figures exactly as a
member that cannot be opened is (`workspace telemetry` reads each member's
`telemetry.db` without opening an engine and still sums its usage): it is named in `degraded_rollup`, the figures
carry `covers_all_members: false`, and the command exits 1
(see the exit code below). Three clones degrade the second and the
third, each naming the first. Setting a distinct `[project] id` in the later
member's `.logos/config.toml` and re-running the command clears the
degradation: opening that member migrates its store to the new coordinate in
place (see *Opening a store moves it to the coordinate in force* in
`configuration.md`), so no re-index is needed. A
workspace whose members all resolve distinct coordinates is otherwise
unchanged: the only difference in its output is the added `coordinate` field.

A member with no `origin` remote takes its directory name as its coordinate, so
two remote-less repositories that share a directory name (`a/api` and `b/api`)
resolve one coordinate as well, and the later is degraded the same way: the
remedy is the same `[project] id`. A malformed `[project] id` fails the member's
config load (exit 2 naming `project.id`) and the member then takes the derived
identity in this roster, so fix the id before relying on the override.

##### `exclusion` — what `[workspace] exclude` left out

When the manifest declares
[`[workspace] exclude`](configuration.md#workspace-exclude--keeping-a-directory-out-for-good),
`workspace status` (human and `--json`) gains an `exclusion` block, so an
exclusion is visible instead of silent:

```jsonc
// logos workspace status --json
"exclusion": {
  "globs": ["*-fork"],              // the declared list, as written
  "excluded": ["svc-fork"],         // autodiscovered directories the globs kept out (absent from every row below)
  "conflicts": [                    // explicit `members` entries that match a glob and were KEPT
    { "entry": "svc-fork", "glob": "*-fork", "warning": "[workspace] members entry \"svc-fork\" matches …" }
  ]
}
```

The block is absent when the manifest has no `exclude`, so such a workspace's
output is byte-identical to before the key existed.

##### `links` — what each declared `[[links]]` entry became

A `[[links]]` entry asserts a cross-service coupling the static bridge cannot
see — a runtime-composed URL, a computed topic. `relation` is `http_call`,
`grpc_call` or `publishes`; anything else is a parse error (exit 2). `from` and
`to` are each `<member>::<target>`, where `<target>` is a canonical symbol or a
name, resolved by **that member's own index**:

```toml
[[links]]
relation = "http_call"
from = "web::fetch_cart"
to = "api::get_cart"
```

`workspace status` (human and `--json`) reports every declared link, in
manifest order, with what became of it:

```jsonc
// logos workspace status --json
"links": [
  { "relation": "http_call", "from": "web::fetch_cart", "to": "api::get_cart",
    "status": "valid",
    "edge": { "relation": "route", "from": {…}, "to": {…}, "intake": "asserted", … } },
  { "relation": "http_call", "from": "web::fetch_cart", "to": "other::get_cart",
    "status": "invalid", "reason": "ambiguous", "endpoint": "to",
    "candidates": { "symbols": ["…", "…"], "total": 2, "omitted": 0 },
    "detail": "`to` endpoint's target `get_cart` matches 2 nodes in member `other` (2 listed); name one by its canonical symbol" }
]
```

- `valid` — both endpoints resolved to exactly one node, in two different
  members: the link is one bridge edge with intake `asserted`, filed under the
  relation class a measured coupling of that kind uses (`route`, `grpc-call`,
  `broker-topic`).
- `invalid` — no edge, with a `reason`: `unknown-member` (no such member,
  including one `[workspace] exclude` removed), `member-degraded` (the member
  could not be opened or read, so the answer is unknown), `no-match`,
  `ambiguous` (at most 8 candidates listed, the rest counted in `omitted`), or
  `same-member` (both ends in one member — that edge is the member's own graph's).
  `endpoint` says which end the reason is about.
- `redundant` — valid, but an edge already joins the same two endpoints; it
  names that edge under `duplicates` and adds no second one. **A route and the
  handler it serves are one endpoint**: a measured HTTP edge ends at the route
  node (`GET /users/{user_id}`), so a link naming the handler behind an
  already-bound route is `redundant`, naming that measured edge, and is never
  listed beside it. A handler no bound route serves is a different endpoint.

A link is a **claim, never evidence**: no asserted edge enters the bridge's
matcher, the coverage figures, `resolved_cross_service_edges`,
`egress_resolution`, `spec_conformance_ratio` or workspace governance. The block
is absent when the manifest declares no `[[links]]`, so such a workspace's
output is byte-identical to before links were interpreted.

**Where a valid link shows up — always labelled `asserted`:**

- **`xservice callers` / `xservice impact`** (CLI, MCP, web) list the asserted
  edges whose queried end is the `(member, symbol)` pair, under their own
  `via_asserted_link` key — **apart from** `cross_service`, and in neither its
  resolved count, `resolved_by_member`, nor the residue line. Each row carries
  `intake: "asserted"` (a callers row at its top level, an impact row on its
  `via` edge) and `"reached": "(asserted)"`, which is what the human rendering
  shows beside it. `--repo` narrows them exactly as it narrows `cross_service`.
- **`workspace reachability`**: a link declaring a **call** (`http_call`,
  `grpc_call`) roots its target, and what only it reaches is listed under
  `live_via_asserted_link` with verdict `live-via-asserted-link` — never
  `live-via-cross-service`, which stays the measured verdict and wins when both
  reach a callable. A `publishes` link reaches a topic, not a callable, and roots
  nothing. The rider's `asserted_call_edges` and the per-member
  `asserted_roots` / `live_via_asserted_link` counts are absent at zero.
- **`workspace status`**: `coverage.asserted_edges` counts the links **beside**
  the headline — `valid`, `invalid` (and `invalid_by_reason`), `redundant`, and
  a `summary` line. A residue row (an invocation reference that did not bind)
  that a valid link starts from keeps its state and reason and gains an
  `asserted_by` pointer naming the link as written.
- **The web UI** draws each valid link as a dashed line in its own colour on the
  service map, with an *Asserted links* legend entry and widget, and lists the
  callables only a link keeps live apart on the Workspace Dashboard.

A link is re-judged on every answer: rename its target and re-sync the member,
and the link turns `invalid` (`no-match`) and its `live-via-asserted-link`
roots disappear on the next query.

##### The headline: `resolved_cross_service_edges` and `egress_resolution`

The workspace headline is a **count of cross-service edges that actually
resolved**, plus the **rate at which captured outbound call sites resolve at
all**:

```bash
logos workspace status            # human
#   61 resolved cross-service edges; egress resolution 0.444 (52 of 117 egress sites resolved)
```

```jsonc
// logos workspace status --json
"coverage": {
  "resolved_cross_service_edges": 61,   // edges resolved from a captured invocation
  "egress_resolution": 0.444,           // absent (null) when no egress site was captured
  "egress_resolution_measured": 117,    // the rate's denominator, explicit
  "resolved_edges_summary": "61 resolved cross-service edges; egress resolution 0.444 (52 of 117 egress sites resolved)"
}
```

- **`resolved_cross_service_edges`** counts edges resolved from an
  **invocation** — a caller→callee HTTP client call, or a producer→consumer
  broker publish or subscribe. (A gRPC stub call would qualify; none is captured
  today, so none can contribute.) It counts *edges*, not sites: under the broker
  fan-out one publish binds every cross-member subscriber and the bridge emits one
  edge per subscriber, so one row can contribute several. You can reconcile it
  against `coverage.references` yourself — sum `1` for a bound invocation row with
  a `to`, and `candidates.total` for one with a bound-to set.
- **Both halves of the line are counted over one population by one walk.** Until
  logos 1.4.11 they were not: the edge count carried an extra `config-bound`
  exclusion that the rate's numerator did not, and this estate published
  *"0 resolved cross-service edges; egress resolution 0.032 (5 of 155 egress sites
  resolved)"* — one sentence stating two different populations. Read
  `resolved_edges_summary`; never rebuild it from the two numbers.
- **`egress_resolution`** is `invocation.bound / (invocation.bound +
  invocation.ambiguous + invocation.unbound)` — over *sites*, so it is **not**
  the count above divided by anything. It is **absent** (`null` under `--json`)
  when that denominator is zero, never a perfect score: no captured egress site is
  *no measurement*, not full coverage.
- **`resolved_edges_summary`** carries both in one line, which is the point —
  a headline coverage figure is never published without the rate at which the
  underlying sites resolved at all
  ([BR-51](../specs/software-spec.md#327-workspace-federation)). Every rendering —
  human, `--json`, MCP and the web coverage view — shows the pair.

> **`bound_ratio` is retired.** Re-measured on an 84-member Spring estate it read
> `0.287 (81 of 282 measured)` over a workspace whose caller→callee edge count was
> **zero** — every one of those 81 bound rows was a declared OpenAPI operation
> matched to a controller, not a resolved call. It also moved *downward*
> (0.355 → 0.287) as broker instrumentation improved, which is the wrong direction
> for a headline. The formula survives as `spec_conformance_ratio` below; the key
> `bound_ratio` is **no longer emitted**, so a reader of it fails loudly rather
> than silently reading a figure that no longer means what it did. It is accepted
> as a deserialization **alias** for one release, so a stored pre-change capture
> still parses.

##### `spec_conformance_ratio`, and why it never appears without its denominator

`spec_conformance_ratio` is `bound / (bound + ambiguous + unbound)` — the retired
bound-ratio's formula, unchanged, under the name of what it actually measures:
how far this workspace's **declarations** line up with its controllers. It is
dominated by `contract-surface` intake and is **never** a measure of cross-service
coupling.

A ratio alone can be read as far more than it is. `no-provider-in-workspace`
references are excluded from the denominator by design (they are the *correct*
exclusion — nothing in this workspace claims to serve them), but a healthy-looking
ratio computed over a handful of references, while hundreds sit excluded, misleads.
So every rendering carries the **denominator and the excluded count** beside it:

```bash
logos workspace status            # human
#   0.365 (126 of 345 measured; 691 excluded as no-provider-in-workspace)
```

```jsonc
// logos workspace status --json
"coverage": {
  "bound": 126,
  "ambiguous": 175,
  "unbound": 44,
  "no_provider_in_workspace": 691,          // the excluded bucket
  "spec_conformance_ratio": 0.365,
  "spec_conformance_measured": 345,         // the denominator, explicit
  "spec_conformance_summary": "0.365 (126 of 345 measured; 691 excluded as no-provider-in-workspace)"
}
```

Those are real figures from an 84-member Spring estate, measured 2026-09-18.

`spec_conformance_measured` and `no_provider_in_workspace` are explicit fields, so
a machine consumer never re-implements the denominator rule. The same figures ride
the [`workspace reachability`](#workspace-reachability) coverage rider, and the web
dashboard renders the score bar **muted** rather than as a confident fill when the
excluded bucket dominates the denominator. When `spec_conformance_ratio` is absent
on a zero denominator, the excluded count is **still** reported — "0 measured,
N excluded" is the informative statement. The same absent-not-zero rule governs
`egress_resolution`.

Some members are not services: a documentation repository, or a mock. If the
manifest declares one of them `kind = "documentation"` or `"mock"`, its
contract-surface rows leave these figures. They are reported under
`coverage.declared_apart`, with a `summary` stating the count over its
denominator. Undeclared members that hold API documents and no runnable source
are listed under `kind_candidates` as a hint, and nothing moves until you
declare them. Both keys are absent when there is nothing to report. See
[`[workspace.member.<name>] kind`](configuration.md#workspacemembername-kind--documentation-and-mock-members).

Some members hold a copy of an API spec they do not implement — a **vendored
spec**. Both renderings (human and
`--json`) then carry two more keys at the end of `coverage`, each a headline
beside its own denominator and a composed `summary`:

- **`coverage.declared_contracts`** — the declared-contract relation:
  `declared_contract_pairs` (split `to_member` by document identity and
  `to_external`) over `documents`, every spec document read by bucket (`own`,
  `vendored`, `partial`, `unjudged`, `mock`, `documentation`); plus the
  `contracts`, the named-external registry (`externals`, each with `id`,
  `name`, `copies`, `declared_by`, `stand_ins`), identity `collisions` and
  `resolved_ties`. On the 83-member reference estate:
  `"6 declared contract pairs (1 by document identity, 5 to named externals) from 7 vendored of 41 spec documents; 5 named externals; 15 contract-surface ties resolved by document identity; declared by vendored specs, never observed calls"`.
- **`coverage.bound_external`** — the external join: `bound_external` over
  `no_provider_rows` (the invocation `no-provider-in-workspace` REST rows),
  with an `accounting` of every refusal that sums to the denominator, and one
  row per judged call. On the same estate:
  `"21 of 32 invocation no-provider-in-workspace REST rows bound to a named external their own member declares (refused: 10 no declared external, 1 no match); declared by vendored specs, never a cross-service edge, and outside egress_resolution"`.

Both are **declared, not observed**. A bound call **stays**
`no-provider-in-workspace` in `references`, `no_provider_in_workspace`,
`by_intake` and `spec_conformance_summary`; `resolved_cross_service_edges`,
`egress_resolution` and the bridge edge set never move. `declared_contracts` is
absent when no member holds a vendored or `mock`-held spec, and `bound_external`
when no member declares a named external — so a workspace with no vendored spec
and no `kind` prints exactly what it printed before. The same keys ride
[`xservice route-providers`](#xservice-workspace-federation-queries), and the web
coverage tab renders them as their own card. See
[Vendored specs](configuration.md#vendored-specs--declared-contracts-and-named-externals)
for what counts as vendored and how a base path is proven.

Members also build against each other. When any member holds a `pom.xml` or
`build.gradle(.kts)` — or a member's facts could not be read, which is never
reported as "none" — both renderings (human and `--json`) carry a
`build_dependency` section, its own top-level key after every runtime one:
`build_dependency_pairs` by kind (`parent`, `dependency`, `managed`,
`bom-import`) beside the `references` it was joined from and the `members`
read, the `collisions` (a coordinate two members produce, resolved to
neither), the `platform_candidates` by in-degree, and — when a member is
declared `kind = "platform"` — its inbound pairs under `platform_apart`. It is
a build dependency, never a runtime coupling: no build edge enters the figures
above. Its `summary` states the pairs by kind beside the denominator in one
line — read that rather than recomposing the two. **After upgrading from a release before
the build relation**, a member's facts exist only once that member has been
fully re-read: run `logos index` or `logos health` in each member (`logos sync`
reads only the paths it is given, so it does not count). Until then the member is
listed under `members.unread` with `members.unread_reasons` giving
`"build facts not yet extracted"`, and the section stays present. It is never
reported as a member with no manifest; its pairs are simply not counted yet. A member
that has **never been indexed** reads the same way, for the same reason. Re-read
before starting `logos serve`, or restart it afterwards. A running serve keeps
its cached build relation, which its `xservice build-deps`, the MCP twin and the
map's build layer read, until that member re-syncs inside the serve. A re-read
run from another shell does not refresh it, though `workspace status` reads fresh. The rows behind it are
[`xservice build-deps`](#xservice-workspace-federation-queries); the web
coverage tab renders the same section as its own card, after every runtime
board. A workspace with no build manifest, every member indexed, prints exactly
what it printed before. See [`kind = "platform"`](configuration.md#kind--platform--build-hubs).

Members also import each other's types. When any member is Java, Kotlin or Avro
— or a member's declared types could not be read — both renderings carry a
`type_reference` section after `build_dependency`. A still-unresolved
import (or qualified type use) in a `.java`/`.kt` file that names a type
**exactly one other** member declares, in its main source tree or in an `.avsc`
schema, is a type reference, with the importing file and line and the declaring
file or schema. It counts only between members the build relation relates, or
where the importer references a colliding artifact the owner produces. Those
pairs are listed under `collision_backed`, with the artifact named. Any other
match is listed under `type_only` and counted there, but never bound and never
part of `type_reference_pairs`. A type several members
declare stays unbound and is listed under `ambiguous_owner`, with the owners
named. The headline `type_reference_pairs` (split `build_pairs` /
`collision_backed_pairs`) sits beside `rows`, every row considered filed into
exactly one of `bound`, `type_only`, `pair_unread`, `ambiguous_owner`,
`self_owned`, `unqualified` (a bare type use such as `Dto`, which names no
package and so is never looked up) and `no_owner`, and the `members` read. Read `summary` for the
one-line form. A type reference is advisory and never a coupling: it is not a
bridge edge, and nothing in `coverage` or `build_dependency` moves with it. The
references behind it are
[`xservice type-refs`](#xservice-workspace-federation-queries).
**After upgrading from 1.7.0**, a member's declared types exist only once it has
been fully re-read (`logos index` or `logos health` in the member); until then
it is listed under `members.unread` with `"declared types not yet extracted"`.
The web coverage tab does not render this section yet. A workspace with no
Java/Kotlin/Avro member, every member indexed, prints exactly what it printed
before.

##### Each reference names the other end

`bound: 96` and `ambiguous: 169` are not actionable on their own — the obvious
next question is *bound to what?*, and *ambiguous between what?*. Every row in
`coverage.references` answers it:

```jsonc
// a BOUND row: the provider it bound to, and how the binding was captured
{ "relation": "route", "from": { "member": "orders", "symbol": "…" },
  "bucket": "bound", "state": "bound",
  "to": { "member": "mailbox-api", "symbol": "…" },
  "intake": "contract-surface" }

// an AMBIGUOUS row: the providers it tied between — none of them bound
{ "relation": "route", "from": { "member": "orders", "symbol": "…" },
  "bucket": "ambiguous", "state": "unbound", "reason": "ambiguous",
  "intake": "contract-surface",
  "candidates": {
    "disposition": "tied-between",
    "providers": [ { "member": "deprecated-mailbox-core", "symbol": "…" },
                   { "member": "funnel-aggregator-api",   "symbol": "…" },
                   { "member": "mailbox-aggregator-api",  "symbol": "…" } ],
    "total": 3, "omitted": 0,
    "summary": "3 tied providers, all listed; none bound" } }
```

A bound row's `to` is the **same** pair
[`xservice route-providers`](#xservice-workspace-federation-queries) reports for that
reference — the two surfaces are computed from one pass and cannot disagree.

Five things worth knowing about these fields:

- **`to` and `candidates` are optional.** A row with no provider to name
  (`no-provider-in-workspace`, `path-not-composed`, `topic-not-literal`,
  `config-key-missing`, `config-placeholder-value`) carries
  neither — absent, never `null` or an empty list. A store indexed before they
  existed has them nowhere; every consumer must read a row without them.
- **`intake` is not optional.** It is on **every** row, in every state (see
  [the intake split](#the-counts-are-two-populations-read-the-split) below). Do
  not read its presence as "this row bound something" — that is `bucket` /
  `state`.
- **`candidates` is bounded and never silently trimmed.** At most 8 providers are
  listed; `total` is the count *before* truncation and `omitted` is the remainder,
  stated in a field and again in `summary`.
- **Naming a candidate is not binding to it.** An ambiguous row's `state` stays
  `unbound` and no edge exists — `disposition` says which of the two a listed set
  is (`tied-between` = none bound; `bound-to` = all bound, the broker fan-out
  shape, where one publish reaches every cross-member subscriber).
- **`provenance` is not optional either, and it says where the target came
  from.** Every row carries it, in every state. `"literal"` means the target is
  written at the call site; `"config-bound"` means it was read from **committed
  configuration**, and the row then also carries `bound` — one entry per
  configuration key the target names, each with its defining files and the
  profiles that prove it; `"config-unresolved"` means the target names keys the
  committed sources do not admit, and carries `keys` and `refusal`. An admitted
  value must never be indistinguishable from an observed one, so a consumer that
  renders a target without reading this field is presenting configuration as
  source text.

```jsonc
// A configuration-bound row: the target was read, not written.
{ "relation": "route", "from": { "member": "web", "symbol": "…" },
  "bucket": "bound", "state": "bound", "intake": "invocation",
  "to": { "member": "orders", "symbol": "…" },
  "provenance": "config-bound",
  "bound": [ { "key": "orders.base", "source": "placeholder",
               "values": [ { "value": "/orders",
                             "profiles": [ "docker" ], "unprofiled": false,
                             "sources": [ "src/main/resources/application-docker.yml" ] } ] } ] }
```

  Two or more entries in a `values` list is an **overlay divergence**: the
  overlays commit different values and every one is retained with the profiles
  that prove it, never averaged and never refused.

- **What this emits on a real estate today, measured rather than estimated
  (2026-09-18).** The accessor capture hop is wired and reaches a *qualified*
  receiver: an `@ConfigurationProperties` accessor expression — whether written
  bare or as `this.mailboxApiProperties.getUriGetMailbox()` — resolves to its
  canonical key at index time and reaches the same resolution a `${...}`
  placeholder already took. On the 84-member reference estate re-enrolled at
  1.4.12, `logos workspace status --json` emits **119** rows carrying
  `config-bound` provenance, and they are now **two populations, not one**: **90**
  on the HTTP arm (`relation: route` — 18 `bound`, 29 `ambiguous`, 42
  `no-provider-in-workspace`, 1 `path-not-composed`) and **29** on the broker arm
  (`relation: broker-topic` — 27 `bound`, 2 `no-provider-in-workspace`), the second
  admitted by the committed-topic-value work of logos 1.4.12. The HTTP figure was
  81 on the 2026-09-13 index, **44** before the qualified receiver was admitted,
  and **0** before the hop existed at all. Read it with its denominators, because
  it is **not** full coverage:

  | | |
  |---|---|
  | rows carrying `config-bound` provenance, HTTP arm | **90** (was 81 on 2026-09-13, 44 before that) |
  | rows carrying `config-bound` provenance, broker arm | **29** (was 0 — the arm admitted none before 1.4.12) |
  | the accessor denominator (production client-call sites the HTTP arm refuses without configuration) | 96 (was 108) |
  | what the measurement harness proves resolvable on that denominator | 81 on 2026-09-13; 84 on merged `main` 2026-09-14; **90 on merged `main` 2026-09-15**, which the 2026-09-18 product reading now matches — see the notes below |
  | rows carrying `config-unresolved` provenance | 0 |

  (The 96 and the 81 are measured and pinned by
  `logos-core/tests/operand_resolvability/configuration_agreement.rs`, which names
  this table among the places to re-record if they move; the 81 admitted is pinned
  by `logos-core/tests/config_bound_admission.rs`, and the full dated record with
  both figures and their denominators is the artifact beside it.)

  **The 81-of-81 agreement is suspended, and the reason is known (2026-09-14).**
  [S-399](../planning/journal.md#s-399-the-accessor-hop-reaches-through-a-uribuilder-lambda)
  landed later in the same sprint and reaches an accessor composed inside a
  `UriBuilder` lambda, which admits **three** further sites on this estate. The
  harness half therefore reads **84** on merged `main` while the product half
  still reads 81, because the product figure is read from each member's *indexed*
  store and this estate's index predates that story. A re-index is expected to
  bring the product to ~84 and restore the agreement at 84-of-84; record whatever
  it reads rather than bending anything to reproduce 81.

  **Re-measured 2026-09-15: the harness half now reads 90, and the expectation is
  90-of-90.**
  [S-405](../planning/journal.md#s-405-a-path-neutral-composer-link-resolves-on-its-path-operand)
  ([CR-129](../requests/CR-129-path-neutral-composer-link-in-a-uribuilder-lambda.md))
  widened that same `UriBuilder` rule from "`path(…)` and an optional `build(…)`"
  to "`path(…)` and any number of links that provably cannot alter the path
  template", which admits the **six** remaining lambda sites on this estate — the
  ones that chain a `queryParam`-family link. The paragraph above is the
  2026-09-14 reading and is left standing as the dated record it is. The product
  half still reads 81 for the unchanged reason, and the denominator is still 96.

  **The re-index arrived, and the agreement returned at 90-of-90 (2026-09-18).**
  The estate was re-enrolled at logos 1.4.12 on 2026-09-17, so the product half
  now reads what the harness half has read since 2026-09-15: **90** of the same
  denominator of **96**. The two paragraphs above are the dated records they were
  and are not restated. The same re-enrolment is what brought the broker arm's 29
  `config-bound` rows into the payload, which is why the table above reports two
  populations where it previously reported one — and it is a *different* arm, not
  a movement in this one.

  **No floor is asserted on either figure, and none should be read into them.**
  The 81-of-81 agreement is what one estate produced on one date, not a property
  the product holds: a harness figure measures what is *derivable*, and turning
  one into a prediction about the product is exactly the error the predecessor
  story was bitten by.

  **Two independent things moved between the 44 and the 81, and they are not the
  same kind of change.** The `config-bound` count rose by exactly **37** — the
  qualified-receiver sites — and the `base-url-runtime` refusals fell by exactly
  37 in the same four members, row for row. Separately, the captured
  invocation-intake population itself fell **186 → 160** because the Go
  client-call candidacy gate became receiver-grained and stopped capturing 26
  non-call sites; those 26 were keyless refusals that produced no reference and no
  edge, so nothing that bound stopped binding. Both show up as "fewer refusals"
  — the residue falls 87 → 24 — but only the first 37 are a coverage gain. A
  lower refusal count is not a coverage gain on its own.

  What still resolves to nothing, stated so the 81 is not read as a ceiling
  reached: a **Kotlin** use site (the grammar field-names neither the receiver nor
  the callee of a member call), and a chained accessor
  (`config.getMail().getHost()`). Each is a refusal, not a wrong answer.

  Two further gaps are open: production ingestion reads **no** `.properties`
  source at all (no plugin descriptor claims the extension), which costs this
  particular estate nothing — a measured residue of **0 of 96** accessor sites,
  because it commits its keys in yaml — but would cost an estate that used them;
  and an accessor-resolved key reports `"source": "placeholder"` rather than a
  distinct `"properties"` label, so the `--json`, MCP and dashboard surfaces
  cannot yet tell the two spellings apart. Nothing is mislabelled as *admitted*
  by that: the provenance, the key, the defining sources and the profile set are
  all correct.

  Note finally what a `config-bound` row reaches, and that this changed in logos
  1.4.13. It is counted in `resolved_cross_service_edges`, because it resolved:
  the coverage tier composed its target from committed configuration and found
  exactly one provider in another member. It now **also draws a bridge edge** —
  the bridge keys the same consumer through the same committed value, so the row
  appears in `xservice route-providers` and seeds a cross-service reachability
  root. Before logos 1.4.13 it did not: the bridge keyed a consumer on its *raw*
  ledger target and a `${…}` placeholder reduced to no portable key there, so the
  two tiers classified one fact two ways.

  The same 84-member estate, read twice on **2026-09-18** — once with the 1.4.12
  binary and once with the arm merged, over the same stores:

  | | before | after |
  |---|---|---|
  | `resolved_cross_service_edges` | 51 | 51 |
  | `bridge_invocation_edges` | 33 | **51** |
  | invocation edges `xservice route-providers` returns, `relation: route` | **0** | **18**, over 8 member pairs |
  | …`relation: broker-topic` | 33 | 33 |
  | `contract-surface` edges | 81 | 81 |

  The coverage tier is byte-identical across the pair; only the bridge moved.
  **No floor is asserted on any of these figures** — the single home for them is
  `logos-core/tests/config_bound_admission.rs`. `bridge_invocation_edges` is still
  published in its own right on `logos workspace reachability`, the surface whose
  `live-via-cross-service` promotions rest on it, and it is still the figure to
  read when the question is what the union view was seeded from: it and the
  headline agree on this estate today, but they answer different questions and a
  fan-out publish or an ambiguous composition separates them again.

  Both terms of `egress_resolution` moved between generations, so it is quoted here
  with its denominator and should never be quoted without one: `0.032 (5 of 155)`
  became `0.128 (15 of 117)`, `0.385 (45 of 117)` on the 2026-09-18 re-enrolled
  index once the broker arm admitted committed values, and `0.444 (52 of 117)` on
  2026-09-21 once S-424 gave the promotion pass and the bridge one identify
  function instead of two that disagreed. The denominator has not moved since. The `-38` in the first
  denominator move has two causes and they are never summed — `-26` sites left the
  captured population entirely when the Go client-call gate became receiver-grained
  (they were never outbound calls), and `-12` moved into
  `no_provider_in_workspace`, which sits outside the denominator, because the newly
  resolved templates name services this workspace does not serve. Only the first is
  a coverage change.

##### The counts are two populations: read the split

`bound: 96` adds two different claims together. A **`contract-surface`** reference
is a *declared* endpoint (an OpenAPI operation) matched to a controller; an
**`invocation`** reference is a *captured call site* (an HTTP client call, or a
broker publish or subscribe — a gRPC stub call would qualify but no arm captures
one). Both are real coupling, but "our specs
line up with our controllers" and "our outbound calls resolve to a service in this
workspace" are not the same statement, and summing them hides the weaker one.

Every row carries its `intake`, and the four counters are reported split by it:

```jsonc
// logos workspace status --json
"coverage": {
  "bound": 133, "ambiguous": 175, "unbound": 37, "no_provider_in_workspace": 691,
  "by_intake": {
    "contract_surface": { "bound": 81, "ambiguous": 146,
                          "unbound": 1, "no_provider_in_workspace": 646 },
    "invocation":       { "bound": 52, "ambiguous":  29,
                          "unbound": 36, "no_provider_in_workspace": 45 }
  }
}
```

Those are the real figures from an 84-member Spring estate, measured 2026-09-21.
`bound: 133` looks like a workspace that binds; `by_intake.invocation.bound: 52`
says that of its 162 captured outbound call sites, fifty-two resolve — 18 HTTP
client calls and 34 broker publishes (it read 45 on 2026-09-18, as 18 and 27; the
+7 is broker alone, attributed to S-424, and the HTTP arm has not moved since). That is what the split is for — and on the
generation of this estate indexed before the accessor capture hop existed, the same
field read **0** beside a `bound` of 81, which is the starker form of the same
point. The `contract_surface` row has not moved across any generation of this
estate; every cell that has ever moved is in the `invocation` row. No floor is
asserted on any of these figures.

The two populations always **sum** to the four counters above them — the headline
is computed from the split, so the two cannot disagree — and because every row
publishes its own `intake`, you can reproduce the split from `coverage.references`
rather than take it on trust.

Two readings to keep apart. `invocation.bound: 0` with invocation references
present is a **finding**: call sites exist and none of them binds. The same zero
with *no* invocation references at all is **honest absence**: nothing was
captured, and the number says nothing about your outbound calls either way. Sum
an `invocation` row's four counts to tell which you are looking at.

The same split rides the `workspace_status` MCP tool and the web coverage view,
which shows it as its own board — a per-arm count cannot separate the two,
because an OpenAPI operation and an HTTP client call are both the `route` arm.

##### Reading a large `ambiguous` count

A high `ambiguous` figure is usually **not** a matching defect, and reaching for
the matcher is the wrong move. Where two or more members legitimately serve the
same normalized template — the aggregator pattern, in which a façade re-exposes
the paths of the services it proxies — the exactly-one rule is refusing
*correctly*, and no refinement of path normalisation or method precedence can
resolve it. The evidence that decides which provider a consumer meant lives in
the consumer's own call site. That ambiguity is **call-site-gated, not
match-gated** ([FR-CG-09](../specs/requirements/FR-CG-09.md) Notes). The
`candidates` list is what lets you see this at a glance: four aggregator members
on one template is an architecture, not a bug.

One shape that surprises people: a tie whose candidates are **all in the
consumer's own member**. Only a *sole* same-member provider is excluded as an
intra-repo fact; a two-or-more tie applies no member filter, so it is reported
here with every participant named and none of them cross-boundary. That is
long-standing behaviour of the exactly-one rule — the bridge agrees, emitting no
edge either way — and `candidates` is simply the first thing to make it visible.

#### Two per-member axes: `warm_state` and `open_state`

Each member row carries **two independent labels**, and conflating them is the
mistake to avoid:

| Field | Question | Values |
|---|---|---|
| `warm_state` | does this member's graph hold an index? | `warm`, `warming`, `deferred`, `degraded` |
| `open_state` | could this member's store be opened? | `opened`, `not-attempted`, `degraded` |

A member with no index yet is `deferred` / `opened` — honest and
**non-alarming**, it indexes lazily on its first query. A member whose store
cannot be opened is `degraded` on both, with a `degraded_reason` and, where the
diagnostic identifies one, a `degraded_cause`:

> **A failed member is attempted and announced once per answer, on both
> surfaces.** `workspace status` walks every member four times to build one
> answer. Before [CR-105](../requests/CR-105-report-a-failed-member-open-once-per-answer.md)
> the report-once guarantee was gated on the CLI's one-shot registry, so the
> one-shot obeyed it and the **served** surface did not: a broken member cost
> four open attempts and four near-identical diagnostics on *every* `GET
> /api/v1/workspace/status`. On an 84-member workspace with 63 degraded members
> that is 252 attempts and roughly 100 KB of repeated warning text per request,
> under exactly the file-descriptor pressure that caused the failures in the
> first place. The guarantee is now scoped to the **answer** rather than to the
> registry's mode, so it holds identically for `logos workspace status` and for
> the served endpoint.
>
> This is deliberately *not* caching across requests. A **later** request
> re-attempts the member and re-announces it, so a member that recovers between
> two requests stops being reported degraded by the second one — the
> transient-recovery property the old mode gate existed to protect. The payload
> is unchanged: same `degraded_rollup`, same per-member rows, same
> `covers_all_members` marker. Only the number of attempts and the volume of
> repeated text change.


Three similarly-named fields can appear on a degraded row, and each answers a
different question: `error` is the row's canonical single fact (its verbatim
engine diagnostic) when a consumer wants just one field; `degraded_reason` /
`degraded_cause` / `degraded_diagnostic` appear only when `open_state` is
`degraded` (below); and `reason` appears only when `warm_state` is `degraded`,
carrying why the warm attempt failed. Because the two axes are derived
independently, a member can be `degraded` on `warm_state` alone (its store
opens fine, but a durable record beside the manifest says its last warm
failed) — that row carries `reason` with no `degraded_reason` at all. A member
whose store cannot be opened is `degraded` on both, and there `reason` prefers
the durable record over this run's own open failure when one exists, so
`reason` and `degraded_reason` can legitimately name two *different*
failures — a recorded one and a live one — on the same row.

- `host-resource-limit` — the process ran out of file descriptors
  (`RLIMIT_NOFILE`). The member's store is present and its graph intact, so
  **a re-index is not the remedy**; raise `ulimit -n`, or query fewer members.
- `store-obstructed` — something that is not a regular file occupies the
  member's `.logos/logos.db` path (a directory, a socket, a dangling symlink).
  Clear that path, *then* run `logos index` in that member.
- `duplicate-coordinate` — the member resolves to the
  [repository coordinate](#each-members-coordinate-and-the-duplicate-it-refuses)
  an earlier member already holds. Its store is intact, so **a re-index is not
  the remedy**; give the later checkout a distinct `[project] id`.

`degraded_cause` is **absent** when the diagnostic identifies no cause, and
`degraded_reason` is then the verbatim engine diagnostic. Notably, a failure whose
store file is simply *missing* claims **no** cause: the store is created on open,
so a never-indexed member opens perfectly well (it reads `deferred` / `opened`) —
which means an absent file at failure time is equally consistent with descriptor
exhaustion partway through creating it. Guessing "no store, go re-index" there
would send an operator whose real problem is `ulimit -n` to a command that cannot
help. `degraded_diagnostic` always carries the verbatim engine error, whether or
not a cause was identified.

`warming` is in the vocabulary but is never reported without a live signal from
the warm supervisor, and the roll-up then **omits** the `warming` key entirely
rather than sending `0` — an absent `warming` means *not knowable*, never *none*.

A member the command never needed to open (a `--repo`-scoped query, say) is
`not-attempted`, and a member whose engine was **evicted** to stay inside the
workspace connection budget still reads `opened` — eviction reclaims a success
and is not a failure.

#### Exit code (⚠️ changed)

`workspace status`, `workspace reachability` and `workspace check` **exit 1 when
one or more members could not be opened**, and 0 otherwise.

Where the members are named differs by subcommand, because the three payloads are
different read-models:

| Subcommand | `--json` naming |
|---|---|
| `workspace status` | `degraded_rollup.degraded_members`, plus `open_state` / `degraded_cause` / `degraded_reason` / `degraded_diagnostic` on each member row |
| `workspace reachability` | `skipped_members`, plus `coverage.members_read` vs `coverage.members_total` (this also fires when a member opened and its surface read failed) |
| `workspace check` | nothing structured — its payload is a bare governance `Option` that must keep serialising as `null`, so stderr is its only channel |

**All three** additionally print a human-readable warning to **stderr** naming
each degraded member *and its cause*, so `--json` stdout stays machine-clean and
`check` is not left exiting 1 with no diagnosis. The warning is **grouped by
cause**: each distinct cause is printed once, as a heading, with an
`affected (N): …` line naming every member it covers. A workspace where most
members fail the same way therefore reads the remedy once rather than once per
member, and no member loses its name or its diagnosis.

On `workspace status`, `degraded_rollup.covers_all_members: false` marks the
member rows and the warm roll-up folded from them as covering fewer than all
members. It does **not** govern `coverage`, which carries its own
`covers_all_members` from a separate walk — a member can open fine and still fail
its contract-surface read, which reduces the coverage figures while leaving
nothing degraded. Read the marker that belongs to the figure you are rendering.

**This is a breaking change.** These commands previously returned 0 no matter how
many members failed, so a workspace where 63 of 72 members could not be opened
was a *successful* command that passed in CI over a payload three-quarters
missing. A script that relies on the old behaviour needs updating; the states
that do **not** move the exit code are `deferred` (nothing indexed yet),
`not-attempted` (nothing needed that member) and an evicted member.

### `workspace reachability`

```bash
logos workspace reachability [--json]
logos workspace reachability --repo <member> [--json]   # scope to one member
logos workspace reachability --all [--json]              # include the full per-repo-dead set
```

The **app-wide cross-service reachability union view** — a separate, explicitly
labeled union of every member's `Calls` / `RoutesTo` adjacency plus the bridge's
cross-service invocation edges folded in as extra live roots. It answers the one
question a per-repo dead-code verdict structurally cannot: *is this callable dead
only because the thing that calls it lives in another repo?* A handler reachable
only via a matched cross-service call is reported live in this view. A valid
`[[links]]` call also roots its target, apart and under its own verdict
(`live_via_asserted_link`, `live-via-asserted-link`) — see
[where a valid link shows up](#links--what-each-declared-links-entry-became).

**Scope and default.** By default the response carries only the **promotions**
(nodes the union view flips from per-repo-dead to app-wide-live: the measured
ones, and the asserted ones listed apart) — the payload a
consumer almost always wants, kept well within the surface budget. The full
per-repo-dead set is large and is suppressed unless you pass `--all`; `--repo
<member>` narrows either view to a single workspace member. The response states
**every** applied bound explicitly under `scope` (`scope.repo`,
`scope.promotions_only`) so a filtered reply can never be mistaken for the
complete dead-set. The projection is a filter, never a silent cap: a suppressed
dead set serialises as `null`, never as an empty `[]` that would read as "nothing
is dead".

The composition is **additive and monotone toward live** — a missing invocation
edge never marks anything dead, and a node whose per-repo verdict is `NULL`
(not-computed) stays `NULL`. The view is **advisory**: it is never a gate input
and never alters a member's own dead-code verdict, and every claim carries a
**coverage rider** stating how much of the invocation graph bound. The rider
carries the same headline `workspace status` reports —
`resolved_cross_service_edges` with `egress_resolution` and
`egress_resolution_measured` beside it, plus the pooled four counts and
`spec_conformance_ratio`.

It also carries one figure `workspace status` does not:
**`bridge_invocation_edges`**, the number of invocation edges the bridge actually
drew — which is what a `live-via-cross-service` promotion rests on, and it is
**not** the headline. The two count different things: a fan-out publish resolves
once and draws one edge per subscriber, and an ambiguous composition resolves
nothing drawable. Until logos 1.4.13 the HTTP arm added a third difference — the
coverage tier composed a call target from committed configuration and the bridge
did not, so a `config-bound` row resolved in the headline and seeded no root — and
on the reference estate the rider read `resolved_cross_service_edges: 15` beside
`bridge_invocation_edges: 0` (2026-09-13): fifteen outbound call sites resolved,
and the union view was seeded from none of them. Both arms now key on the
committed value, and on the same estate re-measured 2026-09-18 the rider reads
**51** beside **51**. Read the second anyway when the question is what the view
could reach — the reasons they can diverge are unchanged.

The rider deliberately does **not** carry `by_intake`: the
split is a decomposition a reader consults once, beside the summary, not eight
counters repeated on every claim. On a real
workspace the promotion set may be legitimately empty (a language that captures a
broker subscribe and one that computes dead-code reachability are, today, disjoint
sets) — an honest empty is reported with its rider, never a fabricated claim.

### `workspace check`

```bash
logos workspace check [--json]
```

Evaluate the **workspace governance rule family** declared under `[governance]`
in `logos.workspace.toml` over the cross-service bridge bindings — for example
"no `edge`-layer service may call a `core`-layer service", or "this deprecated
endpoint has no cross-service callers". Rules quantify over real bridge matches,
never a fabricated edge set.

Governance is reported at the **workspace level** and is **advisory by design**:
it is a separate family from the per-repo rules ([`check`](#check---rules-file---allow-no-rules)), it never
alters any member's per-repo quality gate, and a governance violation **never
moves the exit code** — it is *reported*, not *gated*. With no `[governance]`
rules declared, there is no output at all (`null` under `--json`) — an honest
empty, never a fabricated passing report.

A `[[governance.no_cross_service_callers]]` rule's `symbol` glob matches the
provider symbol's **descriptor tail** — its path and scope, without the
repository coordinate — and the optional `member` scope names the repository:

```toml
[[governance.no_cross_service_callers]]
member = "orders"
symbol = "src/`routes.ts`/legacy*"
reason = "deprecated in v3"
```

A glob written against the whole symbol before symbols carried a coordinate
(`logos . . . src/`routes.ts`/legacy*`) keeps matching the same bindings: it is
also matched against the symbol's coordinate-less form. A glob written as a
path, which matched nothing before because the whole symbol began with `logos`,
now matches by its tail and can report violations. A glob that spells a
coordinate (`logos . github.com/acme/orders . …`) matches neither; use `member`.

A glob that matches **no provider in its scope** — no endpoint any member in the
rule's `member` scope (every member when unscoped) exposes to a cross-service
caller, called or not — can never fire, so it is never read as a pass. It is
listed under `unmatched_rules` (absent when every rule matched), and stderr
names it:

```text
1 workspace rule(s) matched no provider in scope and were not counted as passing: no-cross-service-callers:*retired*
```

A glob that matches a provider nobody calls is the rule holding, and is not
listed. Like a violation, an unmatched rule does not move the exit code.

An **unopenable member** does exit 1, here as for every `workspace` subcommand
(see [`workspace status`](#workspace-status)): that is the answer being
incomplete, not a governance verdict.

---

## Quality & Governance

Every quality command follows the **reconcile-then-score** contract: changed
files are synced first, then the analysis runs against the freshened graph.
The `freshness` field in every response confirms what was reconciled (e.g.
`"reconciled 3 files · HEAD abc1234 · 0 unresolved refs"`). Pass
`--no-reconcile` to skip the sync and score the last committed state — useful
in CI after a pre-built index step.

### `scan`

```bash
logos scan [--no-reconcile]
```

Full code-quality scan: reconcile the index, compute the ten quality
metrics (see [metrics.md](metrics.md)), persist a timestamped snapshot into
`metric_snapshots`, and report the 0–10000 signal with a per-metric breakdown.
The `--json` output also carries a `worst_offenders` field — a per-dimension,
deterministically ordered, top-10 list of the specific functions/containers
dragging each score (report-only; it never gates). Constraints declared in
`rules.toml` are not evaluated here — use `check` for that.

On a graph with fewer than 5 cross-module edges, Modularity, Acyclicity and
Depth are not applicable: `--json` gives each one's reason in `metrics`, under
`modularity_not_applicable`, `acyclicity_not_applicable` and
`depth_not_applicable`, and the count as `cross_module_edge_count`. A language
that is at least 10% of the production callables and binds no call or import
across files earns a `coverage: …` entry in `warnings`, as it does in `gate`
and `session-start`; it never changes the exit code. Both are explained in
[metrics.md](metrics.md#applicability-and-the-na-drop-out).

Under a `rules.toml [review]` table, `warnings` also carries a [trend warning](configuration.md#review--the-branch-delta-gate)
when Equality, Modularity, Acyclicity or Depth fell in each of the last
`trend_window` recorded snapshots (default 2). It never changes the exit code.

Since logos 1.9.0 the lists are **persisted with the snapshot** — by `scan`,
`gate`, `session_start` and `session_end` alike — so the read-only Health page
(`GET /api/v1/health`) shows exactly what the snapshot computed. `worst_offenders`
carries a `recorded` boolean, always present:

```json
"worst_offenders": {
  "recorded": true,
  "nesting": [{"name": "beta_depth_six", "file": "src/lib.rs", "line": 14, "detail": "nesting depth 6"}],
  "conciseness": [], "cohesion": [], "focus": [], "uniqueness": [],
  "acyclicity": [], "depth": [],
  "equality": [{"name": "beta_depth_six", "file": "src/lib.rs", "line": 14, "detail": "complexity 7"}],
  "redundancy": []
}
```

`recorded: true` with empty lists is a recorded-empty result (nothing flagged).
`recorded: false` means the snapshot predates 1.9.0 or the store was never
scanned: its empty lists carry no meaning. A Uniqueness entry's `detail` reads
`clone group #G · N members × L lines`, heaviest group (members × mean lines) first.

Since CR-209 nine dimensions carry a list: Acyclicity, Depth, Equality and
Redundancy join the five above (what each lists, and its tie-break, is in
[metrics.md](metrics.md#worst-offenders--naming-the-cause)). A snapshot recorded
before CR-209 also carries `"unrecorded": [...]`, naming each of those four lists
it holds no rows for although its score says the list is not empty. Treat each
named list as not recorded. The key is absent on every snapshot written since.

Since logos 1.20.0 ([FR-QM-19](../specs/requirements/FR-QM-19.md)) every entry
also carries four fields, so a review can say which symbol got better or worse:

- `node_id` is the symbol id without its repository coordinate. It is the same
  identity a `check --bless` entry uses, and it does not change on re-index.
  `node`, `impact`, `callers` and `callees` resolve it.
  It is `null` on a Depth row, which names a chain of directories, not a symbol.
  An Acyclicity row names its cycle by one member, and a re-index can pick a
  different member for the same cycle. So compare cycles by their members,
  not by `node_id`.
- `value` is the number the entry was judged on, and `measure` says what that
  number counts: `nesting_depth`, `cyclomatic_complexity`, `lcom4`, `methods`
  or `span` (Focus names the one its container crossed), `clone_group_size`,
  `cycle_size`, `chain_length` or `line_count`.
- `threshold` is the effective detection threshold the value met, so
  `value ≥ threshold` always holds. It is `null` for Uniqueness, Acyclicity,
  Depth, Equality and Redundancy, which have no per-unit threshold.

`worst_offenders.values_recorded` is `false` on a snapshot written before
1.20.0. The four fields are then `null` and mean "not recorded", never `0`. The
top-10 cap, the signal and the gate are unchanged. Use `offenders` (below) for
the full lists.

### `offenders [--path <GLOB>] [--dimension <D>]`

```bash
logos offenders --json [--repo <MEMBER>]               # every offender, every dimension
logos offenders --path 'src/a/**' --dimension nesting --json
```

Lists **every** offender the last snapshot recorded, uncapped. These are the
full lists behind `scan`'s top-10 `worst_offenders`, in the same order:
dimension, then the list's own order. Each entry has its `dimension`, `node_id`,
`name`, `file`, `line`, `detail`, `measure`, `value` and `threshold`, as
described under `scan` above. The MCP twin is the `offenders` tool. Both return
the same bytes, and a rerun on an unchanged store returns identical output.

- `--path` keeps the entries whose file matches a project-relative glob. `*`
  stays inside one directory and `**` crosses directories, so `src/a/**`
  matches everything under `src/a` and nothing under `src/ab`. A path without
  glob characters names a file or a directory, so `src/a` and `src/a/` mean
  `src/a/**`; a leading `./` is dropped, `\` reads as `/`, and an empty
  `--path` is no filter. A Depth row has no file, so a path filter never lists
  one.
- `--dimension` keeps one of `nesting`, `conciseness`, `cohesion`, `focus`,
  `uniqueness`, `acyclicity`, `depth`, `equality` or `redundancy`. Modularity
  has no list.
- Both filters together must both match. An unknown dimension, or a glob that
  does not compile or escapes the project root, exits **2**.

`offenders` reads the last snapshot and writes nothing, so run `scan` (or
`gate`) first to measure the current tree. When no snapshot exists, or the last
one was written before 1.20.0, the answer is `recorded: false` with
`not_recorded_because` set to `never_scanned` or
`snapshot_predates_uncapped_offenders`. Its empty list means nothing was
recorded, not that the code is clean.

```json
{
  "recorded": true,
  "not_recorded_because": null,
  "path": "src/a/**",
  "dimension": "nesting",
  "offenders": [
    {"dimension": "nesting", "name": "deep_one", "file": "src/a/one.rs", "line": 3,
     "detail": "nesting depth 6", "node_id": "src/a/`one.rs`/deep_one().",
     "measure": "nesting_depth", "value": 6, "threshold": 4}
  ]
}
```

### `review [--base <REF>]`

```bash
logos review --json                                    # against the default branch
logos review --base main --json [--repo <MEMBER>]
```

Answers "what did this branch change?" per symbol. It compares the working
tree with the **merge-base** of `HEAD` and `--base` (default: the repository's
default branch, `origin/HEAD`, else `main`, else `master`) and classifies every
offender of all nine `offenders` dimensions, and every `check` violation, as:

- `new`: absent at the merge-base;
- `worsened`: present before, with a higher value now;
- `improved`: present before, with a lower value now, still at or over its threshold;
- `resolved`: present before and gone now;
- `unchanged`.

`offenders` and `violations` list every entry the branch changed. An
`unchanged` entry is counted in `offender_counts` or `violation_counts` and not
listed: on a large codebase almost every offender is unchanged, and listing them
would make the answer too large for an agent to read.

When the module graph has fewer than 5 cross-module edges on the merge-base or
on the branch, Acyclicity and Depth are outside the signal there
([metrics.md](metrics.md#applicability-and-the-na-drop-out)), so their
offenders are dropped from both sides and not classified, and `notes` says so.

Each offender carries `before` and `after` values, the `measure` they count and
the `threshold`. Offenders are matched by dimension, measure and symbol id: a
Depth chain by its head, and an Acyclicity cycle by the directories it spans.
A Focus container that now crosses on its span instead of its method count
reads as one entry `resolved` and another `new`. A violation is matched by
rule and subject (the symbol id, a file, a module key, or empty for a
project-wide count). Its `before` and `after` are the per-subject value of
`max_cc`, `max_fn_lines`, `no_god_files`, `max_fan_in`, `max_fan_out`,
`max_nesting_depth` and `no_god_containers`. A violation with no value, such as
a layer or boundary edge or a project-wide count, is also matched by its message.
`introduced_clone_groups` lists the near-clone groups whose members no group at
the merge-base has, and marks each member that was in no group before.

`attribution` explains how far the 0–10000 signal moved from the merge-base to
the branch. The signal is a geometric mean of the dimensions, so the change
splits into one share per dimension when you work in log space:

- `change` is the difference between the two integer signals.
- `exact_change` is the difference between the unrounded signals. The
  `contributions` sum to it.
- `rounding` is what rounding each signal to an integer adds (under one point).
- `residual` is what the contributions, each rounded to two decimals, leave
  unexplained.

Each contribution gives a dimension's `before` and `after` normalized values
(`null` where the dimension is not applicable) and its `points`, largest share
first. It also lists the `drivers`, the offenders that pushed the dimension the
way it moved: `new` and `worsened` entries for a fall, `resolved` and `improved`
entries for a rise, largest change first. At most five are listed;
`driver_count` gives the total. For Uniqueness the drivers are the near-clone
groups that appeared or went, each with its `members`. Modularity has no
offender list, so its share names no drivers. `not_attributable_because` says
why a change has no shares: a signal is `n/a`, or an applicable original
dimension is 0.

Every entry has a `trust` label:

- `resolved`: the file is indexed as it stands, and its language binds relations
  across files;
- `stale`: the file changed on disk since the last sync. `review` reads the graph
  as it stands and never reconciles, so run `logos sync` first. `stale_files`
  lists every such file. An entry with no file reads `stale` while any file is;
- otherwise the `cause` tag of a language the graph binds within files only, as
  [`status`](#status) reports it (Python's, for one).

**The before-state** (`before_state`) is the merge-base commit and nothing else:

- `recorded-snapshot`: the snapshot `scan`, `gate` or `session-start` recorded
  at that commit. It is used only when it was scored under the current
  thresholds and metric semantics, and when the contract evaluates no rule,
  because a snapshot keeps no violation set.
- `shadow-index`: otherwise. The merge-base tree is written from the object
  database to a temporary directory, indexed into a throwaway store, measured
  under the current contract, and cached as
  `.logos/review-cache/<commit>.json`, which the generated `.logos/.gitignore`
  ignores. The first run costs one index of that tree and says so on stderr.
  Later runs reuse the entry while the contract, thresholds, admission, metric
  semantics and logos version are unchanged. The newest 16 entries are kept.

A snapshot at any other commit is never used. When the base does not resolve,
shares no history with `HEAD`, or its merge-base tree cannot be measured (an
object missing from the repository, say), the answer is `measured: false` with
`not_measured_because`, nothing is classified, and the command exits **1**.
Otherwise it exits 0. Like `check`, it re-materialises the graph's derived
annotations under the current `rules.toml` first. It is advisory: it writes no snapshot or violation set
and never changes a `check` or `gate` exit code by itself.

Under a [`rules.toml [review]`](configuration.md#review--the-branch-delta-gate)
table, `gate` and `session-end` judge this same delta, and the answer says how.
`policy` carries the declared `gate_dimensions` and how many listed offenders
gate (`gating`), and each such offender carries `"gating": true`: `new` or
`worsened`, in a gating dimension, with trust `resolved`. A recorded snapshot is
then never the before-state (`before_state` is `shadow-index`), since a gate may
have recorded it over the very work under review. Without the table neither
field appears. Under the table `warnings` also carries the [trend warning](configuration.md#review--the-branch-delta-gate)
of a distributional dimension in decline, read from the recorded snapshots;
without it there is no `warnings` key. The MCP twin is the `review`
tool. Both return the same bytes, and a rerun returns identical output.
`sections` holds one entry per further delta section: today, `security`.

#### `sections.security`

The security findings the branch introduced, touched or resolved, read from the
reports `logos security scan` and `security ingest` stored
([the guide](security.md#the-security-section-of-review---base) explains how to
read it). Every unsuppressed finding of each scanner's latest report is classified,
and each entry names its `method`:

- `introduced`: its fingerprint is absent from that scanner's report recorded at the
  merge-base (`method: "fingerprint"`), or, when none was recorded, it lies inside a
  hunk the branch changed (`method: "hunk"`; uncommitted and untracked files count);
- `touched`: in the merge-base report and inside a changed hunk;
- `resolved`: in the merge-base report, absent from the latest report, on a file that
  report saw as it stands now;
- `stale`: as `resolved`, but the file was edited after the latest report, so nothing
  is claimed;
- `pre-existing`: anything else.

`scanners` lists each scanner's `method` and the commit of its latest report
(`latest_head`);
`counts` has `introduced`, `touched`, `resolved`, `stale`, `pre_existing` and
`suppressed`; `findings` lists the
classified entries, ordered by class, path and line. An entry carries `status`,
`method`, `scanner`, `fingerprint`, `rule_id`, `severity` (an `unrated` one
also carries `severity_reason`), `owasp`, `message`
(the redacted text for a secret-class finding, which `redacted` flags),
`path`, `start_line`, `end_line`, `freshness`, `reachability` and `confidence`.
`suppressed` lists the findings an unexpired ignore entry covers, apart, with each
one's `reason` and `expires` and the class it would otherwise have. The merge-base
tree is never scanned. With no stored report, or when the findings or the diff
cannot be read, the section's `measured` is false, `not_measured_because` says
why, and its lists are empty and mean nothing; the quality half of the answer, and the exit
code, are unaffected. `notes` records how the answer was produced, for instance
that a scanner has not re-scanned since the merge-base, that `HEAD` is the
merge-base, where a scan run now replaces the report recorded there, or that a
`--path` re-scan rewrote the merge-base report, so its scanner is compared by
hunks.

```json
{
  "base": "main", "base_origin": "stated",
  "head": "9f2c…", "merge_base": "41d0…",
  "measured": true, "not_measured_because": null,
  "before_state": "shadow-index",
  "offenders": [
    {"dimension": "nesting", "status": "new", "node_id": "src/`deep.rs`/beta().",
     "name": "beta", "file": "src/deep.rs", "line": 9, "measure": "nesting_depth",
     "before": null, "after": 6, "threshold": 4, "detail": "nesting depth 6",
     "trust": "resolved"}
  ],
  "offender_counts": {"new": 1, "worsened": 0, "improved": 0, "resolved": 0, "unchanged": 1},
  "violations": [],
  "violation_counts": {"new": 0, "worsened": 0, "improved": 0, "resolved": 0, "unchanged": 0},
  "introduced_clone_groups": [],
  "stale_files": [],
  "attribution": {"before": 8300, "after": 8297, "change": -3, "exact_change": -2.6,
    "rounding": -0.4, "residual": 0.0,
    "contributions": [
      {"dimension": "nesting", "before": 0.9812, "after": 0.9794, "points": -1.64,
       "drivers": [{"status": "new", "name": "beta", "node_id": "src/`deep.rs`/beta().",
         "file": "src/deep.rs", "line": 9, "measure": "nesting_depth",
         "before": null, "after": 6}],
       "driver_count": 1},
      …
    ]},
  "sections": {"security": {"measured": true, "not_measured_because": null,
    "scanners": [{"scanner": "semgrep", "method": "hunk", "latest_head": "9f2c…"}],
    "findings": [{"status": "introduced", "method": "hunk", "scanner": "semgrep",
      "fingerprint": "ab12…", "rule_id": "example.shell-injection",
      "severity": "high", "owasp": ["A05:2025"], "message": "a tainted value reaches a shell command",
      "redacted": false, "path": "src/deep.rs", "start_line": 9, "end_line": 9,
      "freshness": "fresh", "reachability": "reachable", "confidence": "resolved"}],
    "counts": {"introduced": 1, "touched": 0, "resolved": 0, "stale": 0,
               "pre_existing": 0, "suppressed": 0},
    "suppressed": [], "notes": []}},
  "notes": ["after is the graph as last synced, …",
            "an unchanged entry is counted in offender_counts or violation_counts and not listed"]
}
```

#### `review --branches <B1> <B2> …`: shared budgets before a merge

```bash
logos review --base main --branches b1 b2 b3 --json [--repo <MEMBER>]
```

Answers "would merging these branches together break a shared limit that
each one fits on its own?". Four sessions that each add one engine edge pass
`check` one by one, and only the merge exceeds `max_fan_out`. This command
catches that case before the merge.

For each branch it measures the tip and its merge-base with `--base`, and
takes the difference as that branch's **share**. It then adds every share to
the value at the base commit and lists each limit the sum takes past its
ceiling. The limits are:

- every declared `[constraints]` budget: per module (`max_fan_in`,
  `max_fan_out`), per function (`max_cc`, `max_fn_lines`,
  `max_nesting_depth`), per file (`no_god_files`), and the project-wide counts
  (`max_cycles`, `max_dead`, `max_duplicates`, `max_brain_methods`);
- a numeric `[review] drift_budget`. The merge is projected to score the base
  commit's signal less every branch's drop, and its drift is measured from
  the gate's blessed baseline, as the gate after the merge would judge it.
  This holds whenever this checkout has a baseline saved under the current
  metric semantics and thresholds. Without one, the drift is measured from
  the base commit's signal. **The answer therefore depends on this checkout's
  baseline**: another checkout can answer differently, and a note says which
  reference was used.

`max_clone_ratio` and an enforced `no_god_containers` are not projected; they
are listed under `not_projected` with the reason. So is `drift_budget =
"report"`.

It is **report only**: it exits 0 whatever it finds, so a merge step or a
reviewer decides what to do. The answer also lists:

- `branches`: every branch you named. One that does not resolve, shares no
  history with the base, or cannot be indexed reads `measured: false` with
  the reason, and is left out of the sum. So does one whose commits a branch
  named before it already holds. A branch built on a branch named before it
  is measured from that branch's tip and names it in `stacked_on`, so their
  shared commits count once.
- `subjects`: the denominator, the (limit, subject) pairs evaluated. That
  is each module, function or file with a value above 0 on some side, plus
  every project-wide count, even at 0.
- `exceeded`: each limit, with its subject (a module key, a function's symbol
  id, a file, or `""` for a project-wide count), its file and trust label,
  `limit`, `base`, `projected` and each branch's `share`. `exceeds_alone`
  marks a share that breaks the limit by itself.

A limit the base already exceeds is listed only when a branch raises it;
`check` reports the rest.

Every side is a committed tree: uncommitted work is not counted, and neither
`HEAD` nor the live graph is measured. Each tree is shadow-indexed once and
cached by commit under `.logos/review-cache/`, so the first run can take as
long as an index per branch, and a rerun prints the same bytes. A share is a
difference, so a change two branches both make independently is counted
twice, where the merge would count it once. The same holds for a branch whose
work already reached the base by a squash merge. A ratchet budget is projected against its `max`;
its blessed set is not read.

The MCP twin is the `review` tool with `branches`.

```json
{
  "base": "main", "base_origin": "stated", "base_commit": "0b18…",
  "measured": true, "not_measured_because": null,
  "branches": [
    {"name": "b1", "commit": "5703…", "merge_base": "0b18…", "measured": true, "not_measured_because": null},
    {"name": "b2", "commit": "654b…", "merge_base": "0b18…", "measured": true, "not_measured_because": null},
    {"name": "gone", "commit": null, "merge_base": null, "measured": false,
     "not_measured_because": "gone does not resolve to a commit"}
  ],
  "limits": ["max_fan_out"],
  "not_projected": [],
  "subjects": 1,
  "exceeded": [
    {"rule": "max_fan_out", "subject": "module:src/`engine.rs`/", "file": "src/engine.rs",
     "trust": "resolved", "limit": 3, "base": 2, "projected": 4,
     "shares": [{"branch": "b1", "share": 1, "exceeds_alone": false},
                {"branch": "b2", "share": 1, "exceeds_alone": false}]}
  ],
  "notes": ["each side is a full index of a committed tree — …", …]
}
```

A `drift` key appears only when the summed drops exceed a numeric
`drift_budget`. It carries:

- `baseline`: the reference signal;
- `current`: the projected signal;
- `drop`, `budget` and `exceeded`;
- each branch's `shares`. `exceeds_alone` counts the base's own distance
  from the reference;
- `reference`: `{"kind": "baseline", "snapshot_id": N}`, or `{"kind": "base",
  "reason": "…"}` when no comparable baseline exists (none saved, or saved
  under other metric semantics or thresholds).

A base already past the budget is listed only when the branches lower the
signal further.

### `improvements`

```bash
logos improvements --json [--repo <MEMBER>]            # where to start, from the last snapshot
```

Answers "what should I fix first?" from the last snapshot. It prints the
**where to start** list: at most ten fix units, each the one thing a fix would
change, ranked by the signal points fixing it would recover. The MCP twin is the
`improvements` tool, and `GET /api/v1/health` carries the same value as
`scan.where_to_start`. All three return the same list for one snapshot, and a
rerun on an unchanged store returns identical output.

- A **unit** is a class (Cohesion, Focus), a function (Nesting, Conciseness), an
  exact-duplicate group (Redundancy) or a near-clone group (Uniqueness). Each
  carries its `kind`, `node_id`, `name`, `file`, `line`, `dimensions`,
  `points`, a `detail` describing the offending magnitude, and `outliers`: the
  measures it is labelled an outlier on.
- **`node_id` survives a re-index.** For a class or a function it is the symbol
  id without its repository coordinate, the same identity an
  [`offenders`](#offenders---path-glob---dimension-d) entry carries; pass it to
  `node` or `impact` and it resolves to that symbol. For a duplicate or
  near-clone group it is `duplicate_group:` or `clone_group:` followed by a
  digest of its members' identities, sorted. It stays the same while the member
  set does, and a member more or fewer makes it another group. A group is named
  (`name`, `file`, `line`) by its member first in that order. A snapshot
  recorded before units carried an identity reads `node_id: null`, never the
  old numeric id. A near-clone group's `detail` still names its cluster number
  (`clone group #N`), which a re-index can change.
- **`points` is an estimate.** It is the quality signal recomputed with that one
  unit's contribution removed and nothing else changed, minus the current
  signal. It is not a promise: fixing two units recovers about the sum of their
  points, not exactly that. The payload's `basis` field says so in words.
- Units are ordered by points descending, then dimension (Modularity, Acyclicity,
  Depth, Equality, Redundancy, Nesting, Conciseness, Cohesion, Focus,
  Uniqueness), then `node_id`. A unit that recovers less than one whole point is
  not listed. Equality, Depth, Acyclicity and Modularity never appear in `units`,
  because splitting a function or a module has no single result; their lists are
  `offenders`.
- `outliers` lists, for each of seven per-unit measures (`lcom4`, `methods`,
  `span`, `cc`, `nesting`, `duplicate_mass`, `clone_mass`), how many units the
  member's own distribution holds (`units`), the upper fence `Q3 + 1.5 × IQR`
  (`fence`), the detection `threshold`, how many units it labels (`labelled`)
  and the labelled units (`labels`, capped like the other lists). A unit is
  labelled only when it is above the fence **and** at or above the threshold.
  With fewer than 8 units there is no fence and no label (`fence: null`).

The list is advisory. It never enters the signal, the gate verdict or any
`check` exit code. `improvements` reads the last snapshot and writes nothing, so
run `scan` (or `gate`) first. When no snapshot exists, or the last one was
written before 1.20.0, the answer is `recorded: false` with empty lists, which
mean nothing was recorded. When `recorded` is `true` and `units` is empty, no
single unit recovers a whole point. That is not the same as clean.

```json
{
  "recorded": true,
  "basis": "estimate: the signal recomputed with this one unit's contribution removed and nothing else changed, minus the current signal; advisory, never gated",
  "units": [
    {"kind": "function", "node_id": "src/`lib.rs`/deep_one().", "name": "deep_one", "file": "src/lib.rs", "line": 49,
     "dimensions": ["nesting"], "points": 159, "detail": "nesting depth 6", "outliers": ["nesting"]}
  ],
  "outliers": [
    {"measure": "nesting", "units": 9, "fence": 1.0, "threshold": 4, "labelled": 1,
     "labels": [{"node_id": "src/`lib.rs`/deep_one().", "name": "deep_one", "file": "src/lib.rs", "line": 49, "value": 6}]}
  ]
}
```

### `check [--rules <FILE>] [--allow-no-rules]`

```bash
logos check                              # use .logos/rules.toml
logos check --rules path/to/rules.toml
logos check --allow-no-rules             # exit 0 even with no contract loaded
logos check --bless                      # bless ratchet debt into .logos/rules-baseline.json, then check
```

Architecture-rules compliance check: reconciles the index, then evaluates
every `[constraints]`, `[[layers]]`, and `[[boundaries]]` declaration in
`rules.toml` against the live graph. This includes the four structural budgets
(`max_nesting_depth`, `max_brain_methods`, `max_clone_ratio`,
`no_god_containers` — see
[configuration.md](configuration.md#metric_thresholds--tuning-the-structural-dimensions)),
the hard-gate counterparts of the structural metric dimensions. Violations with
severity `error` cause exit 1; warnings are reported but do not fail. The
structured report names each violated rule, the offending node or pair, and the
contract it breaks, in a deterministic order.

`check` also always folds in the same `doctor` verdict (below) as two
additional error-severity findings, independent of any `rules.toml` contract —
a `graph-structural-integrity` rule id for the one-node-per-`symbol_id` and
orphan-row invariant, and a distinct `graph-admission-drift` rule id for the
admission tripwire. Neither is persisted to the `violations` table (they are
live invariant checks, not authored rules), but both fail `check` (exit 1) the
same way an authored `error`-severity rule would.

**No contract loaded exits 4** ([FR-GV-22](../specs/requirements/FR-GV-22.md)).
A verdict over an empty evaluated set is not a verdict, so when no `rules.toml`
was loaded *and* nothing fired, `check` reports `passed: null` with
`rules_present: false` and exits **4** rather than a vacuous `passed: true`.
Two ways in are a git worktree that was seeded without the contract, and an
`index` run in a directory that was never `logos init`-ed.

Read `rules_present`, never `checked_rules`, to tell the states apart — a fresh
`init` legitimately has `rules_present: true` with `checked_rules: 0`. Note the
always-on fold-ins above are independent of the contract: if one of them raises
a real violation with no `rules.toml` present, the verdict is `passed: false`
and the exit is **1**, not 4. Only the genuinely empty case reports 4.

Pass `--allow-no-rules` to restore exit 0 for callers that have deliberately
authored no contract yet.

**Ratchet budgets and `--bless`** ([FR-GV-23](../specs/requirements/FR-GV-23.md)).
A budget declared `{ max = N, ratchet = true }` is classified against the
checked-in blessed set `.logos/rules-baseline.json`: an offender blessed at or
below its blessed value is a `warning` (no failure), a new or worsened one is an
`error` (exit 1), and a blessed offender that no longer violates is listed under
`ratchet.fixed`. With no blessed file every ratchet offender is an `error` and the
report names the bless command. `logos check --bless` is the **only** writer of
that file: it records every current offender of a ratchet budget — sorted and
deterministic, byte-identical when nothing changed — replacing only the entries
of the rules the contract it evaluated ratchets (so `--bless --rules FILE` keeps
the default contract's blessed debt), then reports exactly as
`check` does against the set it just wrote. `check` and `scan` only read it, and
`gate` never touches it. The MCP twin is `bless_rules`. The JSON report gains a `ratchet` summary
(counts, the fixed list, the bless command) and each classified violation a
`ratchet` mark (`symbol`, `value`, `blessed`, `status`); both are absent when no
budget is under ratchet, so such a report is unchanged. See
[configuration.md](configuration.md#ratchet-constraints--adopt-a-budget-on-an-existing-codebase)
and the adoption recipe in
[ci-integration.md](ci-integration.md#adopting-budgets-on-an-existing-codebase-the-ratchet).

**A `[security]` table gates on findings, and a missing scan exits 5**
([FR-SC-10](../specs/requirements/FR-SC-10.md)). With
[`rules.toml [security]`](configuration.md#security--the-security-gate)
present, `check` also fails (exit 1) on an unsuppressed finding that is new
since `gate --save` at or above `ratchet_min_severity` (rule
`security:ratchet`), or that a cap names (`security:max_severity`,
`security:fail_on_owasp:<id>`). A scanner in `required_scanners` with no fresh
scan at HEAD exits **5** instead, and `missing_scans` names it; exit 4 takes
precedence. Exit 5 outranks a failing rule's exit 1, so read the rules' verdict
from the `violations` (and `security_violations`): `passed` is false whenever a
required scan is missing, whatever the rules say. Without the table nothing here runs, and the report is unchanged
whatever security evidence exists.

**Since logos 1.4.13 a `check` run leaves a record that it happened.** Before
that the only trace a run left was its violation rows, which are replaced
wholesale each run — so a run that found **nothing** left nothing, and a clean
project was byte-for-byte indistinguishable from one that had never been
checked. `check` now records a **singleton marker** (`ran_at`, `commit_sha`
nullable, `violation_count`) in the *same transaction* that replaces the
violations, so the two can never disagree about which run they describe: a run
that is interrupted leaves neither a marker without its rows nor rows without a
marker. A store that has never been checked has **no** marker at all, and N runs
leave exactly one row — the singleton is enforced by the schema, not by
convention.

`commit_sha` records `HEAD` **at write time** and means nothing more — it
answers "has the tree moved since this ran", **not** "which commit introduced
these findings"; a tree with no resolvable `HEAD` stores it NULL rather than a
placeholder. Alongside the marker, `check --json` now carries `created_at` on
each violation row — the per-row timestamp that has been stored since the
governance engine was built but was never read back — added additively, with no
existing field renamed or removed, and exit codes unchanged (S-313, CR-096).

### `gate [--save] [--threshold <N>] [--label <L>]`

```bash
logos gate                      # compare to last saved baseline; exit 1 on regression
logos gate --save               # score and persist a new baseline
logos gate --threshold 8000     # also fail if signal drops below 8000
logos gate --save --label "v1.2.0"
logos gate --save --reason "the new dispatcher is intrinsic to the feature"
```

The CI quality gate. Without `--save`, compares the current signal to the
last saved baseline plus an epsilon tolerance (1 point on the 0–10000 scale).
Exit 1 if the signal regressed past epsilon or below `--threshold`. `--save`
persists the current scored snapshot as the new baseline — use on release
branches. If neither side has a baseline yet (n/a graph), the gate is
informational (exit 0) unless `--threshold` is set.

If the baseline was scored under a different `metric_version` or a different
structural-threshold set (`thresholds_hash`), the two signals aren't comparable;
the gate **auto-re-baselines once**, reports `baseline reset: metric semantics
changed (v7 8292 → v8 8295)` or `baseline reset: metric thresholds changed (v8
8292 → v8 8295)` — the replaced baseline's metric version and signal, then the
run's, also in `--json` as `baseline_reset` — and passes informationally. The
next gate compares normally. This is what lets you re-tune
a `[metric_thresholds]` value without a spurious CI failure — see
[metrics.md](metrics.md#versioned-baseline--automatic-re-baseline-on-semantics-or-threshold-change).

Under a [`rules.toml [security]`](configuration.md#security--the-security-gate)
table the gate also judges security findings. `--save` records the fingerprint
of every unsuppressed stored finding as the ratchet's baseline, beside the
metric baseline. A compare then fails (exit 1) on a new finding past the ratchet
or on a cap, listing each under `security_violations`. A required scanner with
no fresh scan at HEAD exits **5**, listed under `missing_scans`, and a `--save`
that exits 5 leaves the security baseline unrecorded. The signal never moves.
With a checked-in `.logos/security-baseline.json` ([`security bless`](#security-bless))
that file is the ratchet's baseline instead, and `--save` records none beside it.

Under a [`rules.toml [review]`](configuration.md#review--the-branch-delta-gate)
table a compare judges the branch, not just the number. It measures the
[`review`](#review---base-ref) delta against the merge-base of `HEAD` and the
default branch, and fails (exit 1) on each `new` or `worsened` offender in a
gating dimension, listed under `gating_entries` with its value, threshold and
trust label and named in `message` (one line, wrapped here):

```text
signal 8297 drifted 3 below the baseline 8300 (drift_budget "report": reported, never failing);
1 gating branch-delta entry against the merge-base 5f0c2e91a7b3: nesting `parse_block`
(src/parser.rs:212) nesting_depth 6, threshold 4 [new, trust resolved]
```

The aggregate is then a drift budget in place of `baseline − ε`: `drift`
carries `baseline`, `current`, `drop` (how far the signal fell; negative when it
rose), `budget` (`null` for `"report"`) and `exceeded`, and the gate fails on
the aggregate only when `exceeded` is true. `--threshold` still applies. A delta
that cannot be measured adds a `[review]:` warning and judges no entry.
A distributional dimension that fell in each of the last `trend_window`
recorded snapshots adds a [trend warning](configuration.md#review--the-branch-delta-gate) to `warnings`; it never changes the verdict
or the exit code.
Without the table neither field is printed and the gate is exactly as above.

Under the table the result also carries `attribution`: the signal's change from
the baseline, split per dimension with its driving symbols, in the same shape
[`review`](#review---base-ref) prints it. `baseline_reason` is the reason the
baseline was saved with, when it has one.

**A re-bless says why.** `--save` stores two things with the new baseline. One
is the attribution of the change it blesses, measured from the baseline it
replaces. The other is the `--reason` text, if you give one. Under the table, a
save whose drop exceeds a numeric `drift_budget` requires `--reason`. Without
one it exits **2**, names the drop and the flag, and leaves the baseline as it
was:

```text
logos gate --save --reason "Equality drift from the new scan dispatcher; split tracked in CR-240"
```

`--reason` without `--save` is a usage error. A save within the budget, or one
made without the table, needs no reason. The attribution is stored either way,
but the output carries it only under the table. Any save that gives no reason
clears the reason the previous baseline had; `session-start` never moves a comparable
baseline, so it keeps the reason too. An agent's
`gate --save` still asks the human first, whatever reason it gives (see the
[plugin guardrail](plugin.md#the-guardrail)).

### `quality-report [--hook-json]`

```bash
logos quality-report              # the non-blocking readout; always exit 0
logos quality-report --json
logos quality-report --hook-json  # the agent-host session-start payload
```

The **report tier**: the current signal, the blessed baseline, their delta, and
the recorded rule violations. Always exits 0 — it reports, it never gates. Use
[`gate`](#gate---save---threshold-n---label-l) when you want a verdict and an exit code.

It **writes nothing**, which is the whole reason it exists as its own command.
Both [`scan`](#scan) and [`gate`](#gate---save---threshold-n---label-l) persist a metric snapshot on every run,
so a readout on an automatic, frequent trigger — the
[session-start hook](#hook---emit---force) fires at startup, at every
resume and at every `/clear` — would fill your
[`evolution`](#evolution) series with "someone opened an editor" entries and
contend for the graph write lock with a running `serve`. `quality-report`
recomputes the signal through the same deterministic computation `gate` uses and
simply does not persist it, so the number agrees with the gate's while the series
stays a record of deliberate movements.

Two consequences worth knowing:

- **Nothing absent is defaulted.** An absent signal reads `signal n/a` with the
  cause that produced it (see [the signal line](#the-signal-line-in-full) below),
  not `0`. No blessed baseline reads `no baseline saved`, not a delta against
  zero. A baseline scored under a different metric version or threshold set reads
  `not comparable` with no delta invented.
- **The violations are as of the last recorded rule check**, and say so — with a
  date. (The last *recorded* one: [`scan`](#scan) replaces the violation set too,
  so the run behind the line is not necessarily a
  [`check`](#check---rules-file---allow-no-rules) you invoked, and the line
  deliberately names no command — see [the violations line](#the-violations-line-in-full)
  below.) Re-evaluating the rules re-materialises the derived policy graph, which
  is a write, so the readout reports what was last recorded rather than paying a
  write to look current. What it adds is *how stale* that is, so you can tell a
  live finding from an archaeological one. Run
  [`check`](#check---rules-file---allow-no-rules) when you want current findings.

#### The signal line, in full

There are **two** reasons the signal can be absent, and only one of them is an
empty graph ([FR-EH-04](../specs/requirements/FR-EH-04.md),
[CR-138](../requests/CR-138-a-readout-names-the-cause-its-gating-condition-establishes.md)).
The readout names the one its own reading established, and the machine-readable
`signal_absence` object carries the same verdict as the rendered line.

**`empty-graph` — no source was ingested, or none of it yielded a node.** There
is nothing to score because no graph was built:

```text
$ logos quality-report --hook-json    # rendered
logos quality report: signal n/a (empty graph) · no baseline saved · violations none recorded (no rule check has run)

$ logos quality-report                # the read-model
  "signal": null,
  "signal_absence": {
    "cause": "empty-graph"
  },
```

**`no-production-scope` — the store holds a graph, but none of it is production
code.** The metric signal is computed over the **production scope**
([FR-QM-08](../specs/requirements/FR-QM-08.md)): `is_test` vertices, derived
policy vertices and promoted broker markers are dropped before scoring. A crate
whose only source is a test module therefore scores an empty graph while being
plainly indexed — and says so, carrying the two figures that establish it.

The figures below are a real reading of this exact tree, and they move if the
tree does — a `Cargo.toml` one key longer indexes one node more — so it is given
in full rather than described:

```text
onlytests/
├── Cargo.toml          [package] name = "onlytests", version = "0.1.0"
└── tests/
    └── only_tests.rs   #[test] alpha(), #[test] beta(), and their helper()
```

```text
$ logos status                        # the same store, in the same breath
  "indexed": true,
  "node_count": 8,
  "edge_count": 8,

$ logos quality-report --hook-json    # rendered
logos quality report: signal n/a (no production code — 8 node(s) indexed, 3 test function(s) excluded) · no baseline saved · violations none recorded (no rule check has run)

$ logos quality-report                # the read-model
  "signal": null,
  "signal_absence": {
    "cause": "no-production-scope",
    "indexed_nodes": 8,
    "test_functions": 3
  },
```

`indexed_nodes` is the store's own node count — the very figure
[`status`](#status) prints, read from the same query, so the two commands cannot
contradict each other about that number. `test_functions` is what the production
filter excluded, and is reported as the count it is: `0` there means the scope
was emptied by something other than tests, not that no test exists.

The two commands do answer different *questions* about "indexed", deliberately.
[`status`](#status) reports `indexed: true` when the store holds **any** row
(`files > 0 || nodes > 0`); this arm requires **both** (`files > 0 && nodes >
0`), because a node is not by itself evidence that code was indexed. Declaring
`[[layers]]` in `rules.toml` materialises one derived vertex per declaration
whether or not a file matches, so a project that has never been indexed can show
`file_count: 0, node_count: 3`. That reads `empty graph` here — and the step
really is [`index`](#index), which the other arm would have told you does not
exist.

Neither line names a next command. For `empty-graph` the step is the obvious
[`index`](#index); for `no-production-scope` **no command changes the
state** — writing production code does — and naming one that cannot would be
exactly the misdirection [FR-EH-04](../specs/requirements/FR-EH-04.md) forbids.

#### The violations line, in full

[FR-IN-07](../specs/requirements/FR-IN-07.md),
[CR-096](../requests/CR-096-recorded-check-marker.md) and
[CR-140](../requests/CR-140-the-recorded-check-marker-carries-what-it-evaluated.md)
govern this line. Every run that replaces the violation set records a marker —
its time, the `HEAD` it saw, how many violations it found, and **what it
evaluated** — and the readout renders one of these lines from it:

```text
rule violations: 0 of 12 rule(s) evaluated — clean, at HEAD e5eddac, 4 minutes ago
rule violations: 3 of 12 rule(s) evaluated, at HEAD ff657f5, 6 days ago
rule violations: 3 of 12 rule(s) evaluated, at HEAD ff657f5, 6 days ago; measured against a different tree — HEAD is now a1b2c3d
rule violations: no rules contract authored — no pass is stated, at HEAD e5eddac, just now
rule violations: a rules contract present, authoring no rules — no pass is stated, at HEAD e5eddac, just now
rule violations: evaluated set unknown (this marker predates its recording) — no pass is stated, at HEAD e5eddac, just now
rule violations: none recorded (no rule check has run)
```

Rules govern which one you get, and each exists to stop the readout asserting
something it cannot establish:

- **A clean check may be stated as clean — but only from the marker, and only
  with its denominator.** A run that found nothing records
  `violation_count = 0`, which is precisely what an empty findings table cannot
  express: [`check`](#check---rules-file---allow-no-rules) clears and rewrites
  that table, so a clean run and a project nobody ever checked leave it
  identically empty. With the marker the distinction is a recorded fact, so the
  readout states it. Without one it says **no rule check has run** — never
  `0 violations`, and never a clean result.
- **A count without its denominator is not a result.** `0` is only a pass over
  a contract that authored rules to evaluate
  ([FR-GV-03](../specs/requirements/FR-GV-03.md): *"clean means a contract was
  evaluated and held; it never means nothing was evaluated"*), so the line
  carries the rule count it was measured over. Three states have no denominator
  to carry, and each is named rather than rendered as a zero:
  - **no rules contract authored** — the state
    [`check`](#check---rules-file---allow-no-rules) exits `4` on, printing
    *"nothing was evaluated"*. The readout now agrees with it instead of calling
    the same run clean.
  - **a rules contract present, authoring no rules** — what [`init`](#init--i---hooks---workspace---yes---exclude-glob)
    writes by default, so the ordinary state of a freshly initialised project.
    It is a *configured* project that enforces nothing, which is not the same
    situation as an unconfigured one, and the two are never collapsed.
  - **evaluated set unknown** — a marker written before the evaluated set was
    recorded, which is what every store upgraded in place carries until its next
    run. Unknown is rendered as unknown: never clean, and never zero.
- **The line names no command.** The marker is written by whichever run last
  replaced the violation set, and [`scan`](#scan) does that as well as
  [`check`](#check---rules-file---allow-no-rules) — so a command name in the
  sentence would attribute the run to something you may never have invoked
  (`init` → `index` → `scan`, with no `check` at any point, produced exactly
  that). The `HEAD`, the age and the denominator are what you can act on, and
  they are what the line carries.
- **Staleness is a property of the tree, not just the clock.** When the marker's
  `HEAD` differs from your current `HEAD`, the line says the finding was
  *measured against a different tree*. With only an age, a check against the
  code you are looking at and one against code that has since changed render
  identically. The recorded `HEAD` means "what `HEAD` was when this ran" — it
  does **not** claim the findings were introduced by that commit.
- **Nothing absent is defaulted.** A run on a tree with no resolvable `HEAD`
  (no git, no commits) omits the `at HEAD …` clause and the tree comparison
  rather than printing a placeholder, and an unresolvable *current* `HEAD` is
  never treated as a moved tree.
- **Uncommitted edits are deliberately not consulted.** The readout fires at
  every session start, resume and `/clear`; shelling out to `git status` on each
  one is the per-firing cost this command exists to avoid. So a clean check can
  be current by `HEAD` and yet invalidated by unstaged work — which is why the
  assertion is never printed bare, always with its age and `HEAD`.
- **An age it cannot compute is said, not smoothed.** Two clock readings are
  possible and neither is rendered as a number. A marker stamped **ahead of
  now** — a copied tree, a restored backup, a skewed clock — reads `at an
  unknown age (recorded ahead of now — check the clock)`, and an implausibly old
  one reads its own caveat rather than an age. The rejected alternative was
  clamping a negative age to zero and printing `just now`, which invents the
  most reassuring reading of a fact the readout cannot establish.

A store written before the marker was introduced has findings but no marker. It
needs no re-index: those rows carry their own timestamp, so they are dated
straight away — but with no recorded `HEAD` they are attributed to no tree, no
evaluated set is known for them, and no clean check can be claimed from them
until the next [`check`](#check---rules-file---allow-no-rules). The same holds,
one migration later, for a store whose marker predates the evaluated set: it
upgrades in place, reads *evaluated set unknown*, and starts carrying a
denominator from its next run onwards.

Reading all of this **writes nothing**: the marker's row count and content are
as unchanged by a `quality-report` as the snapshot series is. Reading a run is
not running one.

`--hook-json` renders the same read-model as the agent-host session-start payload
(`systemMessage` + `hookSpecificOutput.additionalContext`). It exists for the
installed hook script to exec; you would not normally run it by hand.

### `health`

```bash
logos health [--no-reconcile] [--repo <MEMBER>]     # unhealthy graph exits 1
logos health --json
```

**ARCHITECTURE** health — the counterpart to [`status`](#status), which reports
**INDEX** freshness. Checks database presence and size, the schema version, FTS
coherence, structural integrity ([FR-GV-18](../specs/requirements/FR-GV-18.md)),
the [FR-GV-20](../specs/requirements/FR-GV-20.md) admission tripwire, and the
graph node/edge counts.

**It projects a verdict.** `ok` is `fts_ok && structural_ok`, and the admission
tripwire folds into `structural_ok`, so `logos health` exits **1** on an
unhealthy graph — which is what
[FR-GV-20](../specs/requirements/FR-GV-20.md) requires of it alongside
`session-end` and `check`, independent of the metric signal. An FTS desync or
admission drift is still *reported* in full rather than raised as an error —
diagnosing it is what `health` is for — but the process exit says so too, so a
CI step or a shell script notices. Payload-identical with the `health` MCP tool
(MCP has no exit codes; only the projection is CLI-side).

### `hook --emit [--force]`

```bash
logos hook --emit          # install the Claude Code quality-report hook (no-op if present)
logos hook --emit --force  # re-emit, replacing the managed entry
```

The hook is a core command: it installs the same in every build. `--emit` is
required — `logos hook` alone is a usage error (exit `2`) — and the command
prints one JSON object describing what it did (`script`, `settings`, `action`,
`notice`, `retired_removed`, `retired_skipped`; compact under `--json`).

Installs the **session-start quality-report** hook
([FR-IN-07](../specs/requirements/FR-IN-07.md)), merged into the shared
`.claude/settings.json` under `hooks.SessionStart` with the source matcher
`startup|resume|clear` — when a session starts, resumes, or is reopened by
`/clear` it surfaces the current signal, the blessed baseline and their delta,
and the recorded rule violations as a non-blocking readout; always exits 0. The
readout is emitted as one JSON object on stdout: `systemMessage` for you,
`hookSpecificOutput.additionalContext` for the agent. Set
`LOGOS_QUALITY_REPORT_DISABLE=1` in the environment to silence it without
uninstalling the hook.

The installed script is a **launcher**: it runs
[`logos quality-report --hook-json`](#quality-report---hook-json) and passes the
output through. It builds nothing itself, so there is no shell JSON assembly to
get wrong, and it swallows the command's failure — a `logos` on `PATH` older
than the emitted script degrades to silence rather than to a visible failed
hook.

**Why session start and not session end.** The readout used to ride a
SessionEnd hook and never worked, for two reasons in the agent host's contract
(observed against Claude Code 2.1.220 — undocumented internals a future release
may change; see [CR-095](../requests/CR-095-session-start-quality-readout.md)):
a SessionEnd hook's exit-0 output is **discarded** — the host renders it only on
a *failing* hook, and hook stdio is piped, never inherited — and SessionEnd
hooks are capped at **1500 ms** against 600 s for every other event, so a
readout costing seconds was cancelled on every firing and surfaced as
`SessionEnd hook [...] failed: Hook cancelled`, including on `/clear`. Every
emit therefore also **sweeps** the retired SessionEnd entry and its orphaned
`logos-quality-report.sh`, so upgrading stops the error with no hand-editing;
the sweep is bounded by Logos' own ownership marker, and a foreign SessionEnd
entry sharing that array is left untouched.

The merge is **non-clobbering**: an existing managed entry that already matches
is left byte-identical, a foreign/unparseable config is never overwritten, and
the merge is idempotent. `logos init -i` installs it **default-on**. Installing or running the hook performs no LLM call and
opens no outbound connection inside the binary — the offline boundary holds.
CLI-only.

The `action` field distinguishes the two ways an existing entry can be
rewritten:

| `action` | Meaning |
|---|---|
| `created` | Nothing of ours was there; the script and the entry were written. |
| `reconciled` | Our entry was there and Logos rewrote it **without** `--force` — its shape had drifted from the current spec (an older emit's `timeout`, a hand edit), or a retirement needed sweeping. Nothing of yours was discarded. |
| `forced` | `--force` re-materialized an existing artifact, **overwriting local edits**. Only this value implies a destructive write. |
| `skipped` | Nothing was written. `notice` disambiguates: absent means "already current"; present means a foreign config was left alone, with the reason. |

`reconciled` exists because `forced` was previously reported for both of the
middle two cases, telling a consumer local edits had been overwritten when the
caller had passed no flag and nothing of theirs was touched
([CR-095](../requests/CR-095-session-start-quality-readout.md) §3.5).

### `session-start` / `session-end`

```bash
logos session-start            # start the session BEFORE edits
# ... make your changes ...
logos session-end              # re-score and compare; exit 1 on regression
```

The session gate ([FR-GV-04](../specs/requirements/FR-GV-04.md),
[FR-GV-05](../specs/requirements/FR-GV-05.md)) without an MCP host, so an agent
directed at the CLI can run the same mandatory bracket the MCP
`server-instructions` describe. `session-start` computes a fresh snapshot and
reports the `session_id` (the snapshot row id), its signal and the freshness
line; it always exits 0. When no baseline is saved, it saves that snapshot as
the project baseline, whatever the security ratchet's state. Once a baseline exists, `session-start` never
moves it: the message says `baseline kept`, and only
[`gate --save`](#gate---save---threshold-n---label-l) re-blesses — the command
an agent's [guardrail](plugin.md#the-guardrail) asks the human about. So an
agent's session cannot bless its own regression. The one exception is a
baseline that no longer compares, because the metric semantics or thresholds
changed since it was saved: `session-start` resets it onto the pre-edit
snapshot with the notice `gate` gives (`baseline reset: …`, and `baseline_reset`
in `--json`), so `session-end`
still judges the session's edits.
`session-end` re-scores and compares against the baseline, exiting **1** on an
aggregate regression beyond epsilon — the same verdict projection
[`gate`](#gate---save---threshold-n---label-l) makes, and what lets a shell
script or CI step notice a failing gate.

Under a [`rules.toml [security]`](configuration.md#security--the-security-gate)
table `session-end` behaves as `gate` does. `session-start` decides each
baseline on its own, before the edits: it saves a missing gate baseline
whatever the ratchet's source, and records the security ratchet's baseline when
neither the checked-in `.logos/security-baseline.json` nor `history.db` holds
one, and its message says so. If a required scan is missing it says the security
baseline was not saved, and it still exits 0. It never overwrites a baseline
that exists (`baseline kept`, `security baseline kept`), so a finding an agent
introduces during the session stays new. `session-end` also fails (exit 1) on a security violation, and exits
**5** when a required scanner has no fresh scan at HEAD. Every commit makes such
a scan stale, so re-run the scan before `session-end`.

Under a [`rules.toml [review]`](configuration.md#review--the-branch-delta-gate)
table `session-end` judges the branch delta as `gate` does: it fails on a gating
entry, and on the aggregate only past `drift_budget`. `session-start` is
unchanged. The verdict holds on a rerun over the same work, committed or not:
the before-state is the merge-base tree, never a snapshot an earlier
`session-end` recorded over that work.

The baseline lives in the store, not in process memory, so the two commands work
across separate invocations. Both are payload-identical with their
`session_start` / `session_end` MCP twins; only the exit-code projection is
CLI-side.

### `doctor`

```bash
logos doctor            # fast structural-integrity + admission check; exit 1 on drift
logos doctor --json
```

The fast, always-on graph-integrity guard, with two dimensions. In a handful of
indexed queries it asserts the core structural invariant — **one node per
`symbol_id`** — plus zero orphan rows (dangling `file_id`, dangling edge
endpoints, orphan shingles). It also runs the **admission tripwire**
([FR-GV-20](../specs/requirements/FR-GV-20.md),
[CR-054](../requests/CR-054-graph-update-admission-unification.md)): every
indexed file the *current* `AdmissionAuthority` would reject — gitignored,
under a nested `.git` boundary, in `ignored_dirs`, or glob-excluded — is
flagged, closing the blind spot where `doctor` reported a graph "sound" while
it silently held scratch (e.g. a dev worktree, or a `.playwright-mcp/`
browser-test directory) the whole-graph quality signal then reflected. It
reports `ok` or names each fault. It is a pure read (no reconcile, no
filesystem walk — O(files) matcher checks against the already-indexed paths),
cheap enough to run after every `index`/`sync` as a debug-build assertion.
Drift exits 1.

`--json` adds four additive fields alongside the pre-existing structural ones
(`node_count`, `distinct_symbol_ids`, `duplicate_symbol_nodes`,
`dangling_file_refs`, `dangling_edge_endpoints`, `orphan_shingles`): the exact,
never-truncated `unadmitted_files` count and a capped, lexically-ordered
`unadmitted_sample` of the offending paths, plus two **diagnostic-only**
advisories — a `doc_symlink_warnings` array and a `zero_admission_warning`
string. Each `doc_symlink_warnings` entry names a documentation
directory-symlink that exists under your doc-include set but ended up
**unindexed** — either because no sanctioned docs root (`.swe-skills`) is
configured, or because the symlink target escapes the sanctioned containment (see
[configuration.md § Documentation](configuration.md#documentation--indexing-markdown)).
It is advisory: a populated `doc_symlink_warnings` **never** flips `ok` to `false`
or changes the exit status — it flags docs you likely meant to index but aren't.
`zero_admission_warning` is the other advisory, and answers the opposite
question — why the graph is *empty* rather than why it holds too much. It is
populated (otherwise `null`) when the root admitted **no** files because every
immediate child was pruned as a nested `.git` boundary — a parent folder of
sibling repositories — **and** the root does not already carry a
`logos.workspace.toml`. That last clause is the "don't repeat advice already
taken" rule the shared helper applies: see [`status`](#status), which reaches
the same state by the same route. It names the prune count, a bounded sample of
the pruned directory names, and the remedy — `logos init --workspace`
([FR-IX-13](../specs/requirements/FR-IX-13.md),
[CR-098](../requests/CR-098-nested-git-prune-diagnostic.md)). It is the **same
line**, derived from the same helper, that `index` emits in its `warnings` and
that `status` carries in its own `warnings` array, so the three surfaces cannot
disagree about a root. Like `doc_symlink_warnings` it is advisory: it never
flips `ok`, never changes the exit status, never becomes a rule finding, and
never moves the quality signal — `doctor` still exits on structural or
admission drift alone.

`plugin_warnings` is one more diagnostic-only advisory array, empty unless the Claude Code plugin
is enabled for the project ([FR-IN-12](../specs/requirements/FR-IN-12.md)). It
names a **duplicate registration** — a `logos` server declared both by
`.mcp.json` and by the plugin, so every tool is listed twice; `logos init -i`
removes the legacy entry — and a **version skew**: this binary is older than
the `minBinaryVersion` in a plugin manifest that applies here (an installed copy
Claude Code recorded for you or this project, or the repository's own
`plugins/logos/.claude-plugin/plugin.json`). Like the others it never flips
`ok` or changes the exit status.

A full `logos index` purges every unadmitted file (its inbound edges return to
`unresolved_refs`), healing the admission drift.

The same verdict is folded into `health` and — critically — **hard-fails the
quality gate**: `check` (as `check_rules`, under a distinct
`graph-admission-drift` rule id, separate from the structural
`graph-structural-integrity` one) and `session_end` exit 1 on structural **or
admission** drift **independent of the metric signal**, so a corrupted or
admission-drifted graph whose score happens not to move is still caught. This
is a correctness gate, not an evidence tier.

One nuance: `check`/`gate`/`session_end`/`health` all reconcile first (the
**reconcile-then-score** contract, below), and that reconcile's FullWalk sweep
now purges any still-on-disk file the current admission rejects too — so a
`.gitignore` edit or a newly-added nested-`.git` boundary is usually healed by
the very call that would otherwise hard-fail on it. `doctor` itself never
reconciles, so it is the most direct way to see admission drift as it exists
right now; `--no-reconcile` on `check`/`gate` surfaces the same un-healed
verdict (and still exits 1).

### `verify`

```bash
logos verify            # deep shadow-reindex consistency check; exit 1 on drift
logos verify --json
```

The deep, on-demand consistency check. It reindexes the project into a
throwaway **shadow store**, censuses both stores, and diffs node/edge/file
counts and symbol sets against the live graph — surfacing **leaked** symbols
(present live, absent from a fresh index) and **orphaned** symbols (the reverse),
with a capped sample of each, and the **lost** edges (in a fresh index, missing
live) and **extra** edges (live-only) — `lost_edges` / `extra_edges` in the JSON,
each naming its `kind`, `source_symbol`, `target_symbol`, `source_file` and
`target_file`, capped at 50 with the exact `lost_edges_total` /
`extra_edges_total` beside each (a count such as `edges -32` says the graph
drifted, the lists say where). Any lost or extra edge fails the check, even when
the edge counts match. The report also embeds the `doctor` report — which carries
the same structural fields *and* the admission tripwire's `unadmitted_files`/
`unadmitted_sample`, read inside `verify`'s single read-only checkout so every
count in the payload reflects one consistent snapshot. It catches drift `doctor`
cannot: a file the live store retains but a fresh index would drop. A full
reindex is **seconds-to-minutes**, so run it deliberately when `doctor` is clean
but a count still looks wrong — not on every check. The live store is opened
read-only for the census; the shadow store (and its `-wal`/`-shm` sidecars) is
torn down on completion. Drift exits 1.

### `evolution`

```bash
logos evolution [--limit <N>] [--repo <MEMBER>]    # default: all snapshots
```

Signal trend over stored snapshots — the architecture-drift detector. Reports
each snapshot's date, label, signal, and per-metric delta from the previous
snapshot. The first snapshot shows null deltas. Reads the append-only
`metric_snapshots` table; history cannot be quietly rewritten.

### `dsm [--granularity <G>]`

```bash
logos dsm [--repo <MEMBER>]           # module-level coupling (default)
logos dsm --granularity file       # file-level coupling
```

Dependency-structure-matrix view: a square coupling matrix between directories
(module granularity) or individual files. Rows and columns are sorted by layer
order (from `rules.toml`); unassigned files appear last. High off-diagonal
values identify coupling hotspots and layering violations.

At module granularity, Go files are grouped into **one row per package**, named
`go:<directory>:<package>`. The row is keyed by the file's directory plus its
`package` clause, so `package store` and `package store_test` in one directory
are two rows. A Go file whose clause cannot be read becomes a `go:<directory>`
row, and the JSON `notes` say so. Edges inside a package are not counted, just as
a module's edges to itself are not. File granularity is unchanged: every Go file
is its own row.

### `doc-gaps [--limit <N>]`

```bash
logos doc-gaps                     # all gaps        (alias: doc_gaps)
logos doc-gaps --limit 50          # top 50
logos doc-gaps --no-reconcile      # score the last committed state
```

Exported symbols with no referencing `DocSection` — a prioritised list of the
public surface that documentation does not yet cover. Only doc-less **exported** symbols are
reported; doc nodes themselves never appear in the list. Use the
`[[require_documented]]` contract in
[configuration.md](configuration.md#rulestoml--the-architecture-contract) to
turn a chosen `paths` glob into an enforceable `logos check` gate.

---

## Evidence tiers (history & coverage)

Two **non-gated, advisory** tiers built from git history and external coverage
reports. They live in a separate store, `.logos/history.db`, on its own
forward-only migration track — created on demand by the first `hotspots` or
`coverage ingest` call. The quality `gate` never opens `history.db`, so nothing
in this section can move the 0–10000 signal (see
[metrics.md](metrics.md#the-non-gated-evidence-tiers)).

### `hotspots`

```bash
logos hotspots [--repo <MEMBER>]      # all ranked files
logos hotspots --limit 20          # top 20 by score
logos hotspots --untested          # only files with no fresh positive coverage
logos hotspots --untested --production-scope   # exclude whole test files from the board
logos hotspots --with-findings     # rank files with a fresh security finding first
```

Ranks files high in **both** git churn (change frequency over a HEAD-anchored
window) **and** structural complexity — the "where is risk concentrating over
time" report. The first call lazily mines git history into `history.db`;
subsequent calls mine only commits since the last mined SHA. The window, the
co-change mega-commit cap, and the defect-message patterns are tunable via the
`[history]` table in
[configuration.md](configuration.md#history--coverage--the-evidence-tiers).

Each ranked row carries a `coverage` cell — a `state` (`"fresh"`, `"stale"`, or
`"n/a"`) plus a `coverage_bp` percentage in basis points — joining the temporal
tier with the coverage tier. `--untested` keeps only files with no fresh
positive coverage (never-covered, stale, or fresh-0%); a file with *any* fresh
positive coverage is treated as tested and excluded. The kept files are ranked
by score. When **no** coverage has been ingested, `--untested` falls back to a
labeled static-reachability signal: the report carries
`coverage_basis = "static-reachability"` and an explicit `coverage_label`
caveat, so the fallback is never silently conflated with execution coverage.

`--production-scope` (optional, off by default) narrows the board to production
files: a file is dropped from the candidate set **before** ranking when *every*
one of its complexity-contributing functions is `is_test` (a whole test file —
`tests.rs`, `*_tests.rs`, `tests/`), so the `--untested` view surfaces the
production code the surface exists to highlight instead of test files that have
high churn and no coverage of themselves. A production file with an in-file
`#[cfg(test)] mod tests` keeps its production functions and stays on the board.
The flag composes with `--limit`/`--untested`, is opt-in and **gate-immune** (the
hotspot tier is non-gated; toggling it never moves a gated signal), and returns
identical rankings across the CLI, the MCP `hotspots` tool (`production_scope`),
and the web Files & Risk view.

Once a scan is stored (`security scan` or `security ingest`), each row also
carries a `findings` cell — `count` of the file's unsuppressed findings and the
`highest` severity among them — so a churning, untested file with a fresh high
finding reads as such. A finding whose file changed since the scan is **stale**:
it is tallied apart as `stale`, never counted, and never lifts `highest`; a
finding an unexpired entry in `.logos/security-ignore.toml` suppresses is not
counted either, while an expired entry no longer suppresses. These are the counts
of [the one definition](security.md#one-definition-of-a-counted-finding), so a
project whose only stored scans are `--path` re-scans has the cell too, and a file
those scans never read reads `0` — absence of a finding, not proof of a scan
(`security status` reads `scoped_only` for that scanner). A finding bound to no file
has no row here. A project with no stored scan has no `findings`
cell on any row — no column, rather than a column of zeros for files nothing
scanned — and `--with-findings` then changes nothing. `--with-findings` ranks the
files with a fresh finding ahead of those without — highest severity first, then
the count, then the usual score and path — and drops no file. The cell is the same
on the CLI, the MCP `hotspots` tool (`with_findings`) and the web Files & Risk
view, whose Findings column links each file to `/security?path=<file>`.

Determinism: the window cutoff is computed from the **HEAD committer
timestamp**, never the wall clock — same HEAD in, byte-identical ranking out.
Files with no in-window history or no parsed functions are **excluded**, never
zero-scored.

### `coverage ingest <REPORT> [--format <FMT>]`

```bash
logos coverage ingest target/coverage/lcov.info        # auto-detect format
logos coverage ingest coverage.xml --format cobertura   # force the parser
```

Parses an external **LCOV** or **Cobertura** coverage report and folds it into
the evidence store as a new snapshot. Format is auto-detected from the content;
`--format` (`lcov` | `cobertura`) forces it. Report-file paths are matched to
indexed files by longest-unique-suffix; absolute build-dir prefixes can be
stripped first via `[coverage] path_strip_prefixes` in
[configuration.md](configuration.md#history--coverage--the-evidence-tiers).
Parsing is **all-or-nothing**: a malformed report is rejected loudly and never
writes a partial store. An ambiguous report path that matches no single indexed
file is reported `unmatched`, never guessed. Exit 3 on an unreadable report,
unknown format, or absent HEAD.

### `coverage refresh`

```bash
logos coverage refresh             # run [coverage_ingest].refresh_cmd, then ingest the artifact it produces
```

Runs the author-configured `[coverage_ingest].refresh_cmd` (see
[configuration.md](configuration.md#history--coverage--the-evidence-tiers)) as a
subprocess via `sh -c`, then discovers and ingests the coverage artifact it
produced. This is the **only** place Logos ever *runs* a coverage command — never
on the `serve`/watcher path ([ADR-38](../specs/architecture/decisions/ADR-38.md)),
only on this explicit invocation. Errors loudly (exit 3) if no `refresh_cmd` is
configured, the command fails, or it produces no recognizable artifact. Artifact
discovery resolves the built-in conventions plus *literal* `artifact_glob`
entries (newest by mtime); a wildcard-only glob that matches no convention or
literal yields a loud error. With a `[coverage_ingest]` table configured, a
running `serve` watcher **auto-ingests** a matching artifact whenever it appears
or changes (a local read+parse, degraded to a warning on any failure — never a
subprocess); `coverage refresh` is the manual counterpart that also produces the
artifact first.

### `coverage status`

```bash
logos coverage status [--repo <MEMBER>]   # human summary
logos coverage status --json       # per-file freshness + overall fraction
```

Reports per-file coverage **freshness** and the overall fresh-coverage
fraction. Freshness is content-hash based: a file whose content changed since
the report was ingested flips to `stale`, and **stale coverage carries no line
data** (it reads as absent, not as the old number). With nothing ingested, the
command returns `n/a` plus a notice (`no coverage ingested — run 'logos coverage
ingest <report>' …`) and exits 0 — absent evidence is never fabricated into a
zero.

## Security findings

This section is the command reference. The guide to the whole section — the tested
version of each scanner, consent and the network, reachability confidence, the
`[security]` gate and the Security page — is [security.md](security.md).

Security scanners report *that* a finding exists; Logos stores each one as
evidence in `.logos/history.db`, anchored to HEAD, the scanner and the file it
names, beside coverage. Like the other evidence tiers it is advisory: nothing
in this section moves the 0–10000 signal, and it gates a merge only under a
[`[security]` table](configuration.md#security--the-security-gate) in `rules.toml`. No snippet of source is ever
stored. A secret-class finding's message is replaced before it is written, and
every other message is stored with anything shaped like a secret masked.
Each command below except `security install`, `security uninstall` and
`security bless` has an MCP twin returning the same payload (`security_scan`, `security_ingest`,
`security_status`, `security_findings`).

### `security scan [--consent] [--install] [--scanner <NAME>]… [--path <PATH>]…`

```bash
logos security scan --consent      # the first scan in a project: record consent, then scan
logos security scan                # later scans
logos security scan --json         # one outcome per registered scanner
logos security scan --install --consent   # install the missing scanners, then scan
logos security scan --scanner semgrep --path src/a.py   # re-scan one file with one scanner
```

Runs every installed, enabled scanner — Semgrep, Trivy, Gitleaks, OSV-Scanner,
Bandit, gosec, Checkov, CodeQL and Dependency-Check — with the fixed commands
listed in [configuration.md](configuration.md#user-level-config--settings-no-repository-can-choose),
writes each SARIF report to `.logos/security/` and ingests it as it finishes.
Without `--install` it runs only what is already there; `[security_scan]` in
`.logos/config.toml` switches a scanner off.

`--install` first installs the scanners Logos can install that are missing and
not switched off in this project — never the opt-in Dependency-Check and CodeQL —
exactly as
[`security install`](#security-install-scanner--all---update---from-dir---consent---accept-codeql-licence)
does, then scans; its exit code is the scan's, not the install's. With `--consent` it records both consents — the per-user
install consent for what it installs, and this project's scan consent — and
without it, exit 2 names what is awaiting consent and nothing is downloaded. The
payload is `{install, scan}`: the install payload, then the scan payload below.

One scan or install of a project runs at a time, whichever surface or process
started it: while one runs, `security scan` exits 3 naming it (`a security scan
is already running on this project: job 4, started from the web UI by Logos
process 81234 12 s ago`) and starts nothing. The same holds for `security
install`, `scan --install` and the MCP `security_scan` tool
([One scan or install at a time](security.md#running-scans-and-reading-the-results)).

The first scan needs `--consent`, because a scanner may reach the network to
refresh its rules or database; without it the command exits 2 naming the flag and
the scanners. Later scans do not ask, and a newly installed or enabled scanner is
asked for alone ([Scan consent](configuration.md#scan-consent)). Nothing starts
before that check. Consent is yours to give: the MCP `security_scan` tool never
records it and refuses a `consent` argument, so an agent that meets the refusal
must ask you to run `logos security scan --consent`.

The payload has one entry per registered scanner, with its `outcome`:

| `outcome` | Meaning |
|---|---|
| `ran` | It ran; `sarif` lists its reports, `findings` counts them, `ingests` holds each ingest summary |
| `not_installed` | No binary was found |
| `disabled` | `[security_scan.<scanner>] enabled = false` |
| `not_selected` | Installed, but `--scanner` did not name it, or a `--path` scan left out a scanner it cannot narrow (below) |
| `requires_build` | CodeQL only: every language present needs a build; ingest a CI report instead |
| `not_applicable` | Nothing it reads is in the project, so nothing started: gosec with no `.go` file, Bandit with no `.py` or `.pyw` file, CodeQL with no file in a language it analyses. Ignored files do not count. `reason` says which files it looked for |
| `failed` | It produced no report, or one that could not be ingested |

One scanner failing never stops the others, and a failed scanner's previous
snapshot is kept. A `failed` entry carries the scanner's `exit_code`, the `log`
holding its whole captured output (`.logos/security/<scanner>.failed.log`;
`null` if Logos could not write it, which never stops the other scanners), and
in `stderr` the output's last lines with anything that looks like a secret
replaced by `[masked]`: a password or token value, a URL's credentials, a private
key block, or a long generated token. It also carries `cause`: one masked line of
that output, the first line that reports an error (the word `error`, `fatal` or
`panic`) or else the last line, cut to 240 characters. The full text stays in the
log, which is under the gitignored `.logos/security/`. The log is removed the next
time that scanner runs cleanly or is `not_applicable`, and `security status` reports it as `last_failure`,
with the same `last_failure_cause`, until then.

Exit 2 also for an invalid `[security_scan]` table or user-level config; the
scan then starts nothing.

#### A re-scan of one scanner and some paths

`--scanner <NAME>` (repeatable) runs only the named scanners, each by registry
name (`semgrep`) or by the report's tool name (`Semgrep OSS`, the name the
[branch review](#review---base-ref) gives it); every other installed one reads
`not_selected`. `--path <PATH>` (repeatable) scans only those files or
directories, so the proof of a security fix takes seconds rather than a full
scan:

- **Paths** are relative to the project root, or absolute inside it, and
  literal: a glob is not expanded, so name each file or directory. One that
  resolves outside the root — `../x`, an absolute path elsewhere, or a symbolic
  link pointing out — is refused with exit 2 before anything starts, as is one
  that does not exist, the root itself, or one beginning with `-`. A path inside
  another given directory is dropped, so nothing is scanned twice. Each path
  reaches the scanner as one argument, never through a shell.
- **Semgrep, Gitleaks, Bandit, gosec and Checkov** run on the paths only. Their
  command is the full scan's with the paths in place of the project: gosec is
  given each file's package directory, and Gitleaks and Checkov run once per path
  (Gitleaks with `--no-git`, reading the file as it is now rather than the git
  history a full scan reads). The report goes to `.logos/security/scoped/`, and
  its ingest replaces that scanner's findings **only** in those paths: every
  other file's findings and freshness stay exactly as they were. The same scoped
  report over the same paths again is a no-op.
- **Gitleaks** is the exception to "replaces". Its full scan reads git history,
  so a secret removed from a file is still there, and its findings in the
  scanned paths are kept. A Gitleaks finding is keyed by its rule, its file and
  a keyed digest of the secret, never by its position or commit, so a secret the
  scoped run finds that is one of them has that finding's very fingerprint, and
  `check`, `gate` and `.logos/security-ignore.toml` still recognise it. Only a
  new secret is listed as new.
- **Trivy, OSV-Scanner and Dependency-Check** read whole manifests, and **CodeQL**
  a database of every file, so none can be narrowed. Each entry says so in
  `scope` (`"whole-manifest only"`, or `"whole-project only"` for CodeQL). Named
  with `--scanner`, it runs on the whole project; otherwise a `--path` scan leaves
  it out.

Consent is asked exactly as for a full scan, for the scanners this scan starts.
With `--path`, each entry carries `scope`: `{"kind": "paths", "paths": […]}` for
a narrowed run, `{"kind": "unscoped", "reason": …}` for one that cannot be. A
scanner whose only stored report comes from `--path` scans — no full scan behind
it — reads `stale` (`scoped_only`) in [`security status`](#security-status), and
one whose full scan is from an earlier commit reads `stale` (`head_moved`), so a
re-scan of a few files never makes a scanner read fresh for the rest. `--install`
takes neither flag. The MCP `security_scan` tool takes `scanners` and `paths`.

### `security ingest <REPORT>`

```bash
logos security ingest ci/codeql.sarif          # a report from CI, any SARIF 2.1.0 tool
```

Ingests one SARIF 2.1.0 report. The whole report is validated before anything is
written; a malformed one is rejected naming its defect, and the store is left as
it was. Report paths bind to indexed files by longest unique suffix; an ambiguous
or unknown path is listed `unmatched`, never guessed. Ingesting the same bytes at
the same HEAD again writes nothing and says `already_ingested`. A report from a
tool that is not a registered scanner is stored under its own name and listed by
`security status` under `other_reports`.

A running `serve` watcher ingests a `.sarif` file written directly into
`.logos/security/` by itself — a read and a parse, never a scanner run.

### `security status`

```bash
logos security status [--repo <MEMBER>]
logos security status --json
```

One state per registered scanner:

| `state` | Meaning |
|---|---|
| `not_installed` | No binary was found and no report is stored |
| `disabled` | Switched off in `[security_scan]` |
| `never_run` | Installed, and no report is stored |
| `not_applicable` | gosec or Bandit only: installed, no fresh report is stored (a stale one cannot be refreshed here), and nothing it reads is in the project, the scan's `not_applicable` above with the same `reason`. Never a missing scan for `check`, and not in the workspace roll-up's `scanners_enabled`. CodeQL with no language to analyse still reads `never_run` here |
| `stale` | A report is stored, but HEAD moved since the full scan behind it (`because: head_moved`), a file it anchors changed (`files_changed`), or only `--path` re-scans stand behind it (`scoped_only`) |
| `fresh` | A report is stored at this HEAD and no file holding one of its findings has changed. A report with no findings anchors no file, so only a new HEAD makes it stale |

`never_run` carries no count; `fresh` carries `findings` (unsuppressed) and
`suppressed`, so a clean scan reads `fresh` with `findings: 0` and is never
confused with a scanner that has not run. A stored report counts whether or not
the scanner is installed here, so a report ingested from CI reads `fresh` or
`stale`. `consent` lists the scanners consented to and the `pending` ones the
next scan will ask for. `suppressions` lists every entry of
`.logos/security-ignore.toml` as [`security findings`](#security-findings) does,
each with `suppressed`, the findings it suppresses now, and flagged `expired` or
`orphaned` (a path glob matching nothing is orphaned).

An installed scanner also carries `installed`: the `copy` that runs —
`override` (your user-level `path`), `path` (found on `PATH`) or `managed`
(installed by Logos) — its `binary`, its `version`, and `update_available`, the
newer version this release pins when the managed copy is older (run
`logos security install <scanner> --update`). A scanner that is not installed
and that Logos can install carries `install`, the command that installs it —
for CodeQL with `--accept-codeql-licence` until its licence is accepted; the
MCP `security_status` names it the same way, since no MCP tool installs.
CodeQL's row also carries `licence`, the recorded acknowledgement. Reading
the versions runs each installed scanner's version command; no scan starts.

### `security install [<SCANNER>…|--all] [--update] [--from <DIR>] [--consent] [--accept-codeql-licence]`

```bash
logos security install --all                 # exit 2: lists what would be installed, downloads nothing
logos security install --all --consent       # record the consent, then install
logos security install trivy gosec --consent # only these
logos security install trivy --update --consent   # replace an older managed copy
logos security install --all --from ~/Downloads/logos-tools --consent   # offline
logos security install dependency-check --consent                  # opt-in, with its Java runtime
logos security install codeql --accept-codeql-licence --consent    # opt-in, after its licence
```

Downloads each scanner's release for this platform, checks its SHA-256 against
the version this Logos release pins, and moves it into the user-level tools
directory (`$LOGOS_DATA_HOME`, else `$XDG_DATA_HOME/logos/tools`, else
`~/.local/share/logos/tools`) — never inside a repository. Trivy, Gitleaks,
OSV-Scanner and gosec install this way; Semgrep, Bandit and Checkov install
through a managed `uv` (below). Name the scanners, or `--all` for those seven.
Dependency-Check and CodeQL are opt-in (below): `--all` never installs them, nor
does `security scan --install`; name them.

**Your own copy wins.** A scanner is installed only when none is usable: a
user-level `path` override, or a copy on `PATH` at or above the minimum version
Logos was tested with, is kept and listed under `kept` — never downloaded. A
`PATH` copy below that minimum is installed over, and the managed copy then
runs in its place; your copy is never removed.

**Consent.** Installing needs your consent for each tool's pinned version. It
is recorded per user (`security-install-consent.toml` beside the
[scan consent](configuration.md#scan-consent)), covers that version in every
project, and is asked for again when a release pins a new version. Without it
the command exits 2 naming each tool, version, source host and download size,
and the command to run, and downloads nothing. `--consent` records it and
installs. An MCP client cannot give it: no MCP tool installs.

**Update.** When this release pins a newer version than your managed copy,
`security status` shows `update_available` and the older copy keeps running —
nothing updates by itself, and a plain `install` leaves it. `--update` asks
consent for the new version, installs it, and removes the old copy.

**Interrupted installs.** An install stopped part-way leaves no tool, only its
download or partial unpack in the `.staging` directory beside the tools. The
next install, of any scanner, discards it, unless a Logos process that is still
running is writing it. Installing the same scanner at the same version from two
processes at once fails it in the second (`another Logos process is using the
staging of <tool> <version>`); install it again once the first has ended.

**Offline.** `--from <DIR>` reads each artifact from a directory instead of the
network, under the file name its release URL ends in, with the same checksum
check. Nothing is downloaded, so `curl` is not needed. An offline
Dependency-Check also needs the Java runtime archive its release URL names.

**Python scanners.** Semgrep, Bandit and Checkov need no Python on the machine.
Logos installs its pinned `uv` (a release binary, checked like the others),
which installs a managed Python and then each scanner into an environment of
its own, from requirements this release pins: every package at one version,
with its hashes checked, from wheels only. A package whose hash does not match
fails that scanner at `checksum` and leaves no environment. The consent names
`uv` and `python` as lines of their own while they are not installed, and each
scanner's download from `pypi.org`. Everything stays in the tools directory —
`uv/`, `python/`, `uv-cache/` and the environments — and nothing is written to
your own Python, your `PATH` or your home directory. For `--from`, the
directory also needs `uv-cache/`: copy `<tools dir>/uv-cache` from a machine
with the same operating system and processor that installed the three — the
cache holds only that platform's packages and Python — and put the `uv` archive
its release URL names beside it. Every `uv` step then runs offline. The `uv`
archive and the Python runtime are checked as online; the packages in
`uv-cache/` are trusted the way `uv` trusts its own cache, so their files are
not re-hashed: use a cache you produced yourself.

**Dependency-Check (opt-in).** Installed only when named. It needs no Java on
the machine: Logos installs a pinned Java runtime (Eclipse Temurin 21, about
150 MB unpacked) with it, checked like every release, into `java/` in the tools
directory, and runs the managed Dependency-Check on that runtime whatever your
own `JAVA_HOME` says. The consent names `java` as a line of its own, with its
download size, while it is not installed. A Dependency-Check you installed
yourself still runs on your own Java. Its first scan downloads the NVD data,
which takes a long time without an NVD API key, and Dependency-Check 13 fails
outright without one: the scan reports it `failed` with the cause `the NVD API
key is missing or rejected`, naming the remedy. Get a free key at
https://nvd.nist.gov/developers/request-an-api-key and pass it through your
user-level `config.toml` (never the repository's, which may not set
`extra_args`):

```toml
[security_scan.dependency-check]
extra_args = ["--nvdApiKey", "<key>"]
```

**CodeQL (opt-in, licensed).** Installed only when named, and only once you
accept its licence: the CodeQL CLI may be used to analyse open-source codebases
and for academic research; any other use, such as on a closed-source codebase,
needs a licence from GitHub ([terms](https://github.com/github/codeql-cli-binaries/blob/main/LICENSE.md)).
Without `--accept-codeql-licence` the command exits 2 showing these terms and
downloads nothing. With it, the acknowledgement is recorded for that version in
`security-install-consent.toml` (a `[licence]` table), together with the
install consent, once the install goes ahead; a release that pins a new
version asks again. `security status` shows it on CodeQL's row as `licence`:
the `accepted` versions and whether the pinned one is among them
(`pinned_accepted`). CodeQL installs as the whole bundle (the CLI with its
query packs: a 1.3 GB download on macOS), and still never builds your project.

The payload lists `consent_recorded` (the `<tool> <version>` lines this run
added), `licence_recorded` (the `<tool> <version>` licences it recorded as
accepted), `kept` (each tool left as it was, with its `copy` and `version`) and
`tools`, one entry per tool installed or attempted, with its `outcome`:
`installed`, `already_present`, `unavailable` (no release for this platform) or
`failed` with the `stage` — `download`, `checksum`, `unpack` or `install` — and
the `reason`. One tool failing never stops the others, and a failed tool leaves
nothing behind.

Exit codes: **0** when at least one tool you asked for is in place — installed,
`already_present`, or kept (your own copy) — however many of the others failed;
the per-tool `outcome`s say which. **1** when you asked for tools and none ended
in place: every one `failed` or was `unavailable`, so a script can tell that
nothing was installed. **2** for a missing consent or licence
acknowledgement, a name Logos does not install, a platform without managed
installs (Windows today), or no tools directory.

### `security uninstall [<SCANNER>…|--all]`

```bash
logos security uninstall --all
logos security uninstall gosec
```

Removes the managed copies of the named scanners, every version, from the tools
directory. A copy you installed yourself — on `PATH` or named by a user-level
`path` — is never touched; `kept` lists the copies still in use afterwards.
Removing the last of Semgrep, Bandit and Checkov also removes `uv`, the managed
Python and `uv-cache/`; removing Dependency-Check also removes the Java runtime,
which no other scanner needs. `--all` covers Dependency-Check and CodeQL too.
Each entry in `tools` is `removed`, `not_installed` or `failed` with a `reason`.

### `security findings`

```bash
logos security findings                                  # everything stored
logos security findings --severity high                  # high and critical
logos security findings --scanner semgrep --path src/api # filters intersect
logos security findings --category A05 --edition 2021    # OWASP Top 10:2021 numbering
logos security findings --suppression active             # what is not suppressed
```

Lists the stored findings, each with its OWASP Top 10 categories (2025 by
default; `--edition 2021` reclassifies without re-ingesting) and its suppression
from `.logos/security-ignore.toml`. The filters are `--severity` (the least
severity listed: `critical`, `high`, `medium`, `low`, `info`, an `unrated`
finding counting as `medium`; or `unrated`, which lists the unrated findings
alone), `--scanner` (a
registry name or the report's tool name), `--category` (an id such as
`A05:2025`, or its number `A05`), `--path` (a project-relative file, or a
directory holding it), and `--suppression active|suppressed`; together they
intersect. An unknown severity, edition or suppression token exits 2. A finding
whose report gave no rating reads `unrated` and carries `severity_reason`
(`"the scanner gave none"`); a rated finding has no `severity_reason`
([security.md](security.md#severities-and-unrated)).

The payload also says what the list is drawn from, so an empty list is never
read as clean: `scanned` lists every stored report with its finding count
(suppressed findings included, unlike `status`'s `findings`),
`never_run` the installed, enabled scanners with none and something to scan (as
`security status` means it, so never a `not_applicable` one), and `notice` says when nothing is stored at all. `categories` gives every category of the edition: `covered`
with the count of every stored finding under it (suppressed ones included, and
whatever the filters), or `not_covered` when no enabled scanner whose report is stored can
detect it. An installed scanner that has never run covers nothing: it has not
looked. A `not_covered` category never reads as 0, though a
finding can still fall under it and is listed. `suppressions` lists every
ignore entry — a `fingerprint`, or a `path` glob with its `rule` and `scanner` —
with `suppressed`, the findings it suppresses now, flagged `expired` and
`orphaned`; a finding's `suppression` names the entry under `by`. An orphaned entry that names a
Gitleaks finding's earlier, per-commit fingerprint also carries `should_name`,
the fingerprint that finding has now (the file is never rewritten). A Gitleaks
finding lists in `commits` every commit its secret was found in, newest first,
and a history-mode one carries `anchor` (`commit`, `short_commit`, `path`): the
commit it is anchored to. Its `binding` is `historical`, with no `path` and an
unbound `context`, when its secret is no longer on that line in today's tree.

### `security bless`

```bash
logos security bless          # writes .logos/security-baseline.json
git add .logos/security-baseline.json
```

Writes the security ratchet's baseline to the checked-in
`.logos/security-baseline.json`: one entry per unsuppressed finding of every
stored scan — the findings `gate --save` records — holding its `fingerprint`,
`rule_id` and `path` relative to the project (`null` when the finding has no
location or was reported outside the project), and nothing else: no message,
snippet, line or secret value. Entries are sorted by path, rule id and
fingerprint, so blessing an unchanged state writes the same bytes and leaves the
file untouched. Runs no scanner.

When the file exists, the [`[security]`](configuration.md#security--the-security-gate)
ratchet reads it **instead of** the set `gate --save` records in the gitignored
`.logos/history.db`: commit it and a fresh CI clone reads only new findings as
new ([ci-integration.md](ci-integration.md#gating-on-security-findings-in-ci-a-checked-in-baseline)).
A new finding's message then names the file it is missing from; `gate --save`
and `session-start` record no ratchet baseline beside it. A malformed file, or
one with a field it does not define, makes `check` and `gate` exit 2 naming it;
the next `security bless` rewrites it.

The payload: `file`, `recorded` (whether the file now holds the current
findings), `changed` (whether its bytes changed), `findings` (how many it
records), `missing_scans` and `message`. Under a `[security]` table whose
required scanner has no fresh scan at HEAD it writes nothing and exits **5**,
listing the scanner under `missing_scans`, as `gate --save` records nothing then.
Without a table it writes the file, which the ratchet reads once one is added.
The plugin guard asks before an agent runs it or writes the file
([plugin.md](plugin.md)).
