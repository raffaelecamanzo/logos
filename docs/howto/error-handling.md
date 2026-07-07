# Error Handling

Logos has **one error contract**, applied identically on both surfaces:

> **Fail soft where safe; fail loud on correctness.**

Every failure is one of two kinds, and the *kind* — not the surface — decides
what happens:

- **Degraded** — a local, recoverable fault (one unparseable file, an
  incompatible grammar, an oversized file, a tail of unresolved references).
  Logos **warns and continues**. The result is *partial but honest*: the command
  still succeeds (exit `0`), and the advisory rides inside the read-model so you
  can see exactly what was skipped.
- **Correctness** — a fault that would make the answer *wrong* if it were
  swallowed (no index, a corrupt store, an invalid config). Logos **aborts
  loud**: a non-zero exit on the CLI, or a structured protocol error on MCP —
  never a silent empty result, and never a raw panic.

The classification lives in one place in the engine (the `Severity` carried by
each typed `CoreError`), so the CLI and the MCP server never re-decide it — they
only project it onto an exit code or an MCP error tag. This is the design in
[ADR-14](../specs/architecture/decisions/ADR-14.md); the requirement is
[FR-EH-01](../specs/requirements/FR-EH-01.md).

## The contract at a glance

| Condition | Severity | What Logos does | Surfaces as | Remedy |
|---|---|---|---|---|
| Unsupported file extension | Degraded | Silently skips the file | (nothing) | — none needed; only known languages are indexed |
| **Unparseable file** | Degraded | Skips the file, indexes the rest | `warnings` in the read-model; exit `0` | Fix the syntax, then `logos sync <file>` |
| **Grammar ABI mismatch** | Degraded | Skips that one grammar, keeps every other language | `skipped` in `logos languages`; a startup warning on stderr | Rebuild Logos against a matching tree-sitter runtime |
| **Oversized file** | Degraded | Skips the file with a notice | `warnings` in the read-model; exit `0` | Raise `max_file_bytes` in `.logos/config.toml` to include it |
| **Partial resolution** | Degraded | Returns the bound results + a coverage number | `resolution_coverage` in `logos status`; exit `0` | `logos sync` after the missing targets land (often nothing to fix — see below) |
| Unknown-symbol query | Degraded | Returns an empty result (+ suggestions where available) | empty read-model; exit `0` | Check the name; re-index if the symbol is new |
| **No index present** | Correctness | Aborts before doing anything | `error:` on stderr; exit `3` | Run `logos index` |
| **Corrupt / unreadable index** | Correctness | Aborts | `error:` on stderr; exit `3` | Delete `.logos/logos.db` and run `logos index` |
| **Invalid `config.toml` / `rules.toml`** | Correctness | Aborts at load | `error:` on stderr; exit `2` | Fix the named key/pattern in the file |
| Engine / internal failure | Correctness | Aborts | `error:` on stderr; exit `3` | See the message; re-index; file an issue if it persists |

Degraded conditions are **never** fatal — a single bad file can never abort a
whole index. Correctness conditions are **never** swallowed — Logos would rather
tell you the answer is unavailable than hand you a wrong one.

## How it surfaces on the CLI

Correctness faults print a one-line, actionable message to **stderr** (stdout
stays clean for `--json`) and set the exit code. The exit codes are a stable
scripting contract (full table in
[usage.md](usage.md#global-flags-and-exit-codes)):

| Code | Meaning |
|---|---|
| `0` | Success — including a *degraded* run that skipped something and warned |
| `1` | Completed, but violations/threshold failures found (`check`, `gate`), or structural/admission drift detected (`doctor`, `verify`) |
| `2` | Usage error: bad flags, or an invalid `config.toml`/`rules.toml` |
| `3` | Internal/environment error — no index, a corrupt store, an engine fault |

Try the two most common ones yourself:

```bash
# Correctness: a command before `logos index` aborts loud, exit 3,
# with a message that names the remedy.
cd /tmp && mkdir -p logos-demo && cd logos-demo
logos search anything ; echo "exit=$?"
# → error: no Logos index found under /tmp/logos-demo: run `logos index` to build one
# → exit=3

# Degraded: an oversized or unparseable file is skipped, the run still
# succeeds (exit 0), and the skip rides inside the read-model.
logos index --json | jq '.warnings'   # any skips are listed here
echo "exit=$?"                          # → exit=0
```

Degraded skips show up in the command's own read-model, so `--json` consumers
can act on them programmatically — e.g. `logos index --json | jq '.warnings'`.

## The `pre-push` enforcing gate

The `pre-push` git hook installed by [`logos init --hooks`](commands.md#init--i---hooks)
turns exit `1` into an **enforcement point**: it runs `logos check` and
**propagates its exit code** as the hook's own. So an error-severity violation
(rule / structural / admission / dead-code) makes `git push` **fail with exit 1**
and names the offending contract — code with a known regression never leaves the
machine. Unlike the best-effort freshness hooks (`post-commit` /
`post-checkout` / `post-merge`, which always exit `0`), this gate is *not*
exit-0-swallowed. It still bails **open** (exit `0`) when the `logos` binary is
genuinely absent — never a false block — and `git push --no-verify` bypasses it
(git skips the hook natively). It is a local pre-flight, not a replacement for
the CI enforce step; see [CI integration](ci-integration.md) for the full
freshen / enforce / report / bless loop.

## How it surfaces on MCP

The same split applies to the `logos:*` tools, mapped to MCP semantics:

- **Degraded** conditions never become protocol errors. They ride inside the
  tool's JSON result — as `warnings`, an `INCOMPLETE` freshness line, or an
  `n/a` + `notice` field — so the agent gets an honest partial answer and keeps
  working.
- **Correctness** conditions return a **structured MCP error**, never a crash.
  The error payload carries a `severity: "correctness"` tag (the same
  classification the CLI projects onto exit `3`) and a human-readable message.
  The server **stays alive** and keeps answering subsequent calls — a tool-level
  fault never takes the session down ([NFR-RA-12](../specs/requirements/NFR-RA-12.md)).
- A malformed request (unknown tool, bad argument) is an `invalid_params` /
  parse error naming the offending input — again, the server keeps running.

This is why an agent can call `logos:scan` on a repo with a few unparseable
files and still get a usable signal: the bad files are degraded skips inside the
result, not an aborted call.

## Reading a degraded result honestly

A degraded result is *correct about what it could do*, not a defect:

- **`resolution_coverage` around ~50% is normal**, not a broken index — it
  counts every syntactic reference including stdlib/external calls that can
  never bind to a workspace symbol. See the detailed explanation in
  [usage.md](usage.md#navigating-the-graph). Look at the `refs_resolved` count
  and the `artifact_bindings` breakdown to gauge *workspace-internal* resolution.
- **Skipped files** are listed in the producing command's `warnings`. If a skip
  surprises you, the message names the cause (a syntax error, a size over the
  `max_file_bytes` cap) and the fix.
- **Evidence-tier degrades** (e.g. `logos hotspots` in a non-git or shallow
  checkout) return `n/a` + a one-line `notice` and exit `0` — they never
  fabricate a number.
- **Performance-envelope advisory.** Logos is tuned for a ~100k-LOC
  small/medium target. When the indexed repository materially exceeds that
  envelope, `logos index` and `logos status` add a one-line advisory to their
  `warnings` (exit `0`) — navigation stays correct, you are just past the
  latency budgets Logos guarantees. It is an advisory, not a failure.

## Troubleshooting common errors

| You see | It means | Do this |
|---|---|---|
| `no Logos index found under <path>: run `logos index`` | No `.logos/logos.db` yet (exit 3) | `logos index` |
| An `error:` about a corrupt/unreadable store (exit 3) | The store failed an integrity check | `rm .logos/logos.db && logos index` |
| `invalid TOML in .logos/config.toml: …` | A syntax error or unknown key (exit 2) | Fix the key/line the message names |
| `glob pattern … escapes the project root` | An `exclude`/path glob with `..` or an absolute path (exit 2) | Make the pattern project-relative |
| `unknown node kind "<x>"` | A bad `--kind`/`kind:` filter (exit 2 / `invalid_params`) | Use one of the listed kinds |
| A query returns nothing | The symbol isn't in the graph, or the index is stale | Check the name; `logos sync` after edits |
| Node counts look wrong / keep growing across incremental `sync`s | Graph drift — duplicate or orphan rows the live store accumulated (exit 1 from `doctor`/`verify`) | `logos doctor` for the fast structural verdict, `logos verify` for the deep shadow-reindex diff; a full `logos index` heals a leak. Migration 16 auto-dedups on first open. |
| `doctor`/`verify` reports `unadmitted_files > 0` (or `!ok`), naming the offending paths in `unadmitted_sample`; `gate --no-reconcile`, `check_rules`, or `health` fail alongside it | Admission drift — the store still holds a file the *current* `AdmissionAuthority` would reject (gitignored, under a nested `.git` boundary, in `ignored_dirs`, or glob-excluded), most often a dev worktree (`.worktrees/**`) or browser-test scratch (`.playwright-mcp/**`) that slipped in before [CR-054](../requests/CR-054-graph-update-admission-unification.md) or via a `.gitignore` edit that narrowed scope | `logos index` — a full reindex purges every unadmitted file (edges return to `unresolved_refs`); a plain reconciling `gate`/`check`/`session_end` usually self-heals it too. See [commands.md](commands.md#doctor). |

## Where this is defined

- Requirements: [FR-EH-01](../specs/requirements/FR-EH-01.md) (the policy),
  [FR-EH-02](../specs/requirements/FR-EH-02.md) (actionable messages),
  [NFR-UX-02](../specs/requirements/NFR-UX-02.md) (each path names its remedy),
  [NFR-RA-11](../specs/requirements/NFR-RA-11.md) (graceful partial resolution).
- Design: [ADR-14](../specs/architecture/decisions/ADR-14.md) — typed core
  errors with the Correctness/Degraded `Severity` split, translated at the
  [api-facade](../specs/architecture/components/api-facade.md) boundary.
