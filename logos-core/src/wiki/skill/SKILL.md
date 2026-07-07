---
name: logos-wiki
description: >-
  Generate and maintain the Logos source wiki — durable, anchored, agent-written
  pages stored in `.logos/wiki.db`. Use when asked to write, refresh, or audit
  wiki documentation about this codebase, or when `logos wiki status` reports
  stale, missing-anchor, or absent/revision-stale structured sections to work
  through.
---

<!-- logos-wiki skill — emitted from Logos v{{LOGOS_VERSION}}. After a binary
     upgrade, refresh with `logos wiki skill --emit --force`. -->

# Logos source wiki — generation skill

Logos **stores, anchors, and serves** wiki pages; **you (the coding agent)
generate them**. Generated prose is the one artifact Logos cannot reproduce
deterministically or offline, so it lives quarantined in a fourth store
(`.logos/wiki.db`) that survives a full `index`, is never seen by the quality
gate, and carries mandatory provenance on every read. Your job is to write
accurate pages, anchor them to the right graph entities, and keep them fresh as
the code moves.

Everything below uses the `logos wiki` CLI. Five commands have a
payload-identical MCP twin (`wiki_write`, `wiki_read`, `wiki_search`,
`wiki_status`, `wiki_materialize`) — use whichever your host exposes, the
contract is the same. The local-only commands (`wiki generate`, `wiki hook
--emit`, `wiki skill --emit`, `wiki delete`) are CLI-only.

`wiki materialize` (CR-062, [FR-WK-20]) is the binary's own deterministic
writer for the presented tier (below) — it never involves you and you never
need to run it: the augmentation hook and the UI-gated generation trigger both
run it automatically, ahead of the queue, before you ever see one. It exists on
both surfaces mainly so a host/CI can invoke it directly; running it yourself
is harmless (idempotent, offline) but is not part of your normal loop.

## When to use this skill

- The user asks for a wiki page, an overview, an architecture note, a "how does
  X work" write-up, or onboarding docs about *this* codebase.
- `logos wiki status` lists **stale** pages (an anchored file/symbol changed),
  **missing-anchor** pages (an anchor's entity is gone but others survive), or
  **page-worthy entities without pages** (regenerate candidates).
- A page was auto-pruned (every anchor disappeared) and the concept still
  deserves a page.

Do **not** invent facts. Read the graph first (`logos context`, `logos node`,
`logos search`, `logos explore`, `logos callers/callees/impact`) and write only
what the code supports. The wiki is generated content, not verified fact — Logos
labels it as such on every read.

## Overview page template — write for the reader

The five Guided-Tour Overview pages (`overview/project-overview`,
`overview/getting-started`, `overview/key-concepts`, `overview/how-it-works`,
`overview/known-issues`) are the first thing a newcomer opens. Write them from
the **reader's** perspective, not the code's — this is not a symbol tour, and
it is not a terse capability list either.

**Open with a concrete reader outcome** — one or two plain-language sentences,
before any structural detail, naming what the reader will be able to *do*
after reading this page and why that matters to them. Everything below fills
in after that opening, never instead of it:

1. **Title + concrete opening outcome** — the one or two sentences above:
   what this page orients the reader to and what it gets them, first.
2. **What this gets you** — the reader's goal in more depth: what the project
   lets them do and why it matters, still in plain language, still no
   structural or internal detail.
3. **Behavior and workflows** — how the reader actually drives the project,
   framed around the **real** commands, subcommands, MCP tools, or workflows
   the grounding doc(s) below name.
   Quote the actual command, subcommand, or workflow name exactly as the doc
   gives it (its literal CLI invocation or tool name) rather than
   paraphrasing it into vague capability language — never internal call
   graphs, function signatures, or module layout.
4. **Key concepts** — the handful of ideas a newcomer needs, defined in plain
   language, not as a symbol glossary.
5. **Where to go next** — point to the Getting Started page, the User Guide, or
   the relevant consolidated documentation page.

Ground these pages in the doc-grounding directive the work-list hands you (see
below) — it names exactly which `README.md`/`docs/howto/*` files to read and
summarize. Pull the concrete commands and workflows those docs actually
document into the page rather than staying abstract; a first-time user should
recognize something they can go run. When a mapped doc is absent, the
directive falls back to the code graph, but the page still reads user-facing
and still opens with a concrete outcome: describe what the reader can do and
how it behaves, not the internal mechanism. Reserve symbol names, file paths,
and code-level invariants for genuinely code-level pages (below).

## Page structure — code-level pages

For a page about a specific symbol, module, or component — **not** an Overview
page (above) or a consolidated documentation page (grounded in its own `docs/`
category) — write the body as Markdown. A good page is self-contained and
skimmable:

1. **Title + one-sentence purpose** — what this page documents and why it matters.
2. **Overview** — the concept in 2–5 sentences, in plain language.
3. **How it works** — the mechanism, grounded in named symbols/files. Link claims
   to the entities you anchored (see below) so a reader can jump to the source.
4. **Key types / entry points** — the handful of symbols a newcomer needs.
5. **Gotchas / invariants** — non-obvious constraints the code enforces.
6. **Related pages** — slugs of adjacent wiki pages.

Keep bodies focused and under the **1 MiB** hard cap. Prefer several tightly
anchored pages over one sprawling page — staleness is per-anchor, so a tight
page goes stale only when *its* subject changes.

## Write contract

```bash
logos wiki write \
  --slug architecture/wiki-engine \
  --title "Wiki engine" \
  --generator "claude-opus / logos-wiki skill" \
  --anchor symbol:'<canonical LogosSymbol>' \
  --anchor file:logos-core/src/wiki/mod.rs \
  --body-file page.md
```

Rules the store enforces (a violation exits non-zero and leaves the store
**byte-identical** — nothing is partially written):

- **Slug** — one or more `/`-separated segments; each segment is lowercase
  ASCII letters, digits, `-`, or `_`. No leading/trailing `/`, no `.`/`..`, no
  spaces. The slug is the upsert key: writing the same slug **replaces**
  (last-write-wins), so reuse a slug to refresh a page.
- **Title** — short human label.
- **Generator** — **mandatory, non-empty**. Identify what produced the page
  (your model + this skill). Empty generator is rejected.
- **Body** — stored **byte-verbatim**, ≤ 1 MiB. A read returns it unchanged.
- **Anchors** — zero or more `"<kind>:<key>"` bindings (see below). An anchor
  whose entity does not exist in the current graph/tree **rejects the whole
  write** — anchors are never guessed.

## Anchoring (and multi-anchoring)

An anchor binds the page to a stable graph entity and captures that entity's
content hash at write time. At read time Logos re-hashes and reports each
anchor as **fresh** / **stale** / **missing** — that is how the wiki tracks
drift without any LLM and without touching `sync`/`index`.

Two anchor kinds:

- `file:<repo-relative-path>` — e.g. `file:src/engine.rs`. Freshness is the
  file's bytes on disk; an edit flips it **stale** at the next read with no
  `sync`; a rename/delete reads **missing**. Paths must be repo-relative and
  non-traversing (no leading `/`, no `..`).
- `symbol:<canonical LogosSymbol>` — a code symbol, `DocSection`, or doc-graph
  Requirement/Adr/Story node. Get the exact symbol string from
  `logos node <name>` or `logos search <name> --json`. Existence is resolved in
  the graph; content is the symbol's defining file on disk. (A `symbol:` key may
  itself contain `:` — only the first `:` splits the kind from the key.)

**Multi-anchoring guidance** — anchor a page to **every** entity whose change
should mark the page stale, and no more:

- A page about one function → anchor that **symbol**. Editing the function flips
  the page stale; you'll see it in `wiki status` and regenerate.
- A page spanning a module → anchor the **handful of key symbols / files** it
  describes, not every file (over-anchoring makes pages perpetually stale on
  unrelated edits).
- A page tied to a requirement/decision → anchor the doc-graph node
  (`symbol:` for the Requirement/Adr/Story) **and** the primary implementing
  symbol, so it goes stale when either the spec or the code moves.
- **Lifecycle:** a page survives as long as **one** anchor exists (missing ones
  are flagged). It is auto-deleted and logged only when **every** anchor is
  gone. A **zero-anchor** page (a hand-curated overview/index) is never stale
  and never auto-pruned — use it sparingly, only for content not tied to any
  single entity.

## Reading and provenance

```bash
logos wiki read --slug architecture/wiki-engine          # human
logos wiki read --slug architecture/wiki-engine --json   # machine
```

Every read carries four mandatory provenance fields — the **generator**, the
**written-at HEAD** commit, **per-anchor freshness**, and the fixed
generated-content marker (`generated content — not extracted by Logos`). A
page presents as stale if any anchor is stale. Trust the freshness flags:
"fresh" means *the anchors are unchanged since write*, never that the prose was
verified.

## Search

```bash
logos wiki search "near-clone threshold"        # bm25-ranked, staleness-flagged
logos wiki search --list                         # list every page
```

FTS5 search runs offline inside `wiki.db` over titles and bodies. Hits carry
their staleness flag — prefer fresh hits, and treat a stale top hit as a
regeneration cue.

## The structured wiki: three tiers

The wiki is served under a fixed information architecture with three tiers of
different provenance (CR-062). Knowing the split tells you exactly what to
write — and what **not** to.

- **Native (extracted) tier — Logos owns it, you never write it.** Logos
  live-renders the deterministic, structural sections straight from the graph,
  always fresh, labelled *"extracted — live from graph @revision N"* — **three**
  sections: the Codebase structure tree, the crate→module→file Files tree, and
  the dependency Mermaid diagram (rendered as a visual diagram in the web UI).
  These are **never** in the `wiki status` work-list and must never be authored
  as pages — doing so would duplicate extracted fact as stale prose. (There is
  no longer a native Configuration section — config artifacts are not part of the
  wiki read-model.)
- **Presented tier — Logos owns it, you never write it or touch it.** In SRS
  mode (Case 1, below) the binary's own `wiki materialize` operation ([FR-WK-20])
  deterministically copies the project's authored `docs/specs/**` — the
  Architecture page and every present Design/Specs category — into `wiki.db`
  verbatim, one in-page section per source document, labelled *"presented from
  `docs/specs/…`"*. It is `generator = "logos:doc-present"`, byte-identical on
  re-run, and makes no LLM/network call. It runs automatically, before you ever
  see the queue — **never** hand-author, "helpfully" refresh, or `wiki write` any
  slug this tier owns, even if asked; that would fight the binary's next
  materialize and get silently overwritten anyway.
- **Agent (advisory) tier — you write it.** The synthesized prose that only an
  LLM can produce. The web wiki presents the Overview pages as a **Guided Tour**
  (the Start-here landing leads straight into it) — codebase-wide pages, each at
  a canonical `overview/<slug>`, all **zero-anchor**:

  | Guided-Tour page | Canonical slug | Anchors |
  |------------------|----------------|---------|
  | Project Overview | `overview/project-overview` | none (zero-anchor) |
  | Getting Started | `overview/getting-started` | none (zero-anchor) |
  | Key Concepts | `overview/key-concepts` | none (zero-anchor) |
  | How It Works | `overview/how-it-works` | none (zero-anchor) |
  | Known Issues | `overview/known-issues` | none (zero-anchor) |

  The **Architecture** page (`overview/architecture`) is **no longer an
  Overview page you author** (CR-062): it is now the **presented**
  `docs/specs/architecture.md`, assembled deterministically under the Design
  tier — never queued to you.

  Plus the **consolidated documentation pages** — one readable document per
  documentation category, each summarizing the project's own `docs/` files (all
  **zero-anchor**, at canonical slugs):

  | Consolidated page | Canonical slug | Source glob (read & summarize) |
  |-------------------|----------------|--------------------------------|
  | ADRs | `architecture/adrs` | `docs/specs/architecture/decisions/*.md` |
  | Components | `architecture/components` | `docs/specs/architecture/components/*.md` |
  | Integrations | `architecture/integrations` | `docs/specs/architecture/integrations/*.md` |
  | Functional Requirements | `specs/functional-requirements` | `docs/specs/requirements/FR-*.md` |
  | Non-Functional Requirements | `specs/non-functional-requirements` | `docs/specs/requirements/NFR-*.md` |
  | User Acceptance Tests | `specs/user-acceptance-tests` | `docs/specs/requirements/UAT-*.md` |
  | Frontend Design | `specs/frontend-design` | `docs/specs/frontend-design.md` |

  Write the Guided-Tour pages and consolidated pages at exactly their canonical
  slugs (the view looks them up there) as **zero-anchor** pages — they are
  codebase-/docs-wide synthesis tied to no single entity, so they carry no
  `file:`/`symbol:` anchor and are kept fresh by the revision signal below, not by
  per-anchor staleness.

  **Per-file/module pages are no longer part of this tier.** `wiki status` /
  `wiki generate` never seed a per-file "objectives" page or an unanchored
  File/Module entity — a generic per-entity page has no navigational entry
  point in the wiki menu or the native Files/Codebase-structure tier, which
  already lists every file and module live. Do not author one on your own
  initiative just because a file lacks a page; let the work-list drive (below).
  A per-file page written before this change remains valid and searchable —
  refresh it like any other anchored page if `wiki status` flags it stale — but
  never author a new one.

  **Reuse the project's own docs — don't synthesize from scratch.** The wiki
  consolidates the project's documentation: rather than one fragmented page per
  ADR / requirement / component, each consolidated page is **one document
  summarizing all files in its `docs/` category**, and four of the five
  Guided-Tour Overview pages are grounded in the project's own **user-facing**
  docs rather than free synthesis ([FR-WK-24] — write them with the Overview
  page template above, not the code-level structure). When the work-list hands
  you a consolidated page or a grounded Overview page, it carries a
  **doc-grounding directive** naming the source file(s)/glob to read. Read those
  docs and summarize them — do not invent content the docs do not state.
  Organize a long category page (e.g. 100+ requirements) by functional
  area/sub-group with an on-page table of contents, and give **each source
  document its own in-page sub-section** — a heading matching the source
  document's ID or title (e.g. `## ADR-01 — …` for a decision,
  `## FR-WK-06 — …` for a requirement, `## wiki-engine — …` for a component) so
  deep links into the category survive. The authoritative `docs/` files remain
  the source of record; the wiki is the readable summary.

  - **Project Overview** (`overview/project-overview`) ← `README.md` +
    `docs/specs/software-spec.md`.
  - **Getting Started** (`overview/getting-started`) ← `docs/howto/README.md` +
    `docs/howto/installation.md` + `docs/howto/usage.md`.
  - **How It Works** (`overview/how-it-works`) ← `docs/howto/usage.md` +
    `docs/specs/software-spec.md`.
  - **Key Concepts** (`overview/key-concepts`) ← `README.md` +
    `docs/specs/software-spec.md`.
  - **Known Issues** stays free synthesis — no grounding directive.
  - **Consolidated category pages** ← their `docs/` glob (table above).

  This mapping is the same in **both** modes (SRS and inference) — it is not
  restricted to Case 1 or Case 2.

  If a mapped doc is **absent**, the directive says so and you fall back to
  reading the code graph (`logos context`/`node`/`explore`) and summarizing
  from the code — but keep writing the page from the reader's perspective (the
  Overview page template above), never the symbol-centric structure.
  Consolidated category pages are only queued when their source files exist, so
  they never need a fallback. Story (journal/sprint) and Change-Request nodes
  are **not** part of the wiki — never author pages for them.

  **When an SRS is present (Case 1), the Design/Specs pages are not yours.**
  If the project carries `docs/specs/architecture.md` and requirement files, the
  binary **presents** the Design/Specs pages deterministically (CR-062) and
  `wiki generate` restricts your queue to the **Summary/Overview tier only** —
  you never author the consolidated ADRs/Components/Requirements/UAT pages or the
  Architecture page in that mode. When no SRS is present (Case 2) the queue is
  unchanged (Overview + present consolidated categories).

  **Let the work-list drive — don't author from this table by hand.**
  `wiki status` / `wiki generate` (below) are authoritative: they surface the
  Overview sections and (Case 2) consolidated documentation pages Logos tracks,
  and hand you the exact slug + anchor for each page that is absent or stale.
  Write whatever they report. After the first `index` the work-list seeds **exactly
  these five** Overview pages plus (Case 2 only) the present consolidated
  categories — each `absent` until you write it, then `revision-stale` once the
  graph advances past it — so `wiki generate` and this table describe the same
  Guided-Tour set. A page you have not written yet simply renders the honest "not
  yet generated" placeholder until you do.

**Derived "stale — regeneration pending" freshness.** Beyond per-anchor
staleness, every agent page records the **graph revision it was built at**. When
the graph advances (a completed `index`/`sync`), any page built at an older
revision is flagged **revision-stale** — "regeneration pending" — even a
zero-anchor page that per-anchor freshness can't see. This verdict is derived
purely by comparing revisions; the binary never regenerates on a page view.
Regenerating is **your** job, off the request path: refresh the page (same slug)
so its built-at revision catches up.

**First availability is the first `index`.** Before any `logos index` the wiki
has no graph to describe and the structured work-list is empty — `init` is never
blocked waiting on wiki generation. Once the first `index` completes, the native
tier renders immediately and `wiki status` begins prompting you for the agent
sections above.

## The `wiki status`-driven regeneration loop

`logos wiki status` is the work-list. Run it, then work it top to bottom:

```bash
logos wiki status            # store summary + regeneration work-list
```

It surfaces:

- **Stale pages** — an anchored entity changed. Re-read the current code for
  that entity, update the body, and `wiki write` the **same slug** (last-write-
  wins refreshes the page and re-captures fresh anchor hashes).
- **Missing-anchor pages** — one anchor's entity is gone but the page survives.
  Decide: drop the dead anchor (rewrite with the surviving anchors) or repoint
  it to the renamed entity.
- **Pruned log** — pages auto-deleted because every anchor vanished. If the
  concept still matters, regenerate it with current anchors.
- **Unanchored page-worthy entities** — currently always empty: files, modules,
  and (when present) doc-graph Requirement/Adr/Story nodes are all excluded
  from this list, since a generic per-entity page has no navigational entry
  point in the wiki menu or the native Files/Codebase-structure tier. Never
  author one on your own initiative even if you notice a file or symbol with no
  page.
- **Structured sections** (`work_list.structured_sections`) — the agent-tier
  sections of the structured wiki above, each tagged `state: "absent"` (never
  written) or `state: "revision-stale"` (built at an older graph revision). Each
  row carries the `section` label, the `slug` to write, the page `title`, the
  `anchor` to bind (always empty — every current section is zero-anchor), and —
  for four of the five Overview pages and the consolidated category pages —
  a `grounding` directive naming the doc source(s) to read and summarize.
  Work them like any other item — write the page at the given slug; an `absent`
  row becomes a first authoring, a `revision-stale` row a refresh. The native
  (extracted) sections never appear here, and a category whose source files are
  absent is never seeded.

The loop: `wiki status` → pick an item → read the graph → write/refresh the
page at the given slug with correct anchors → repeat until the work-list is
empty. Each refresh is an upsert on the existing slug (and re-captures the
current built-at revision), so the loop converges without manual cleanup.

## Driving the loop: `wiki generate`

`logos wiki generate` turns the `wiki status` work-list into a ready-to-run
**generation queue** so you don't assemble `wiki write` invocations by hand:

```bash
logos wiki generate          # human prompt block: one ready-to-run item per page
logos wiki generate --json   # the same queue as machine JSON
```

- The default output is a **prompt block** — a deterministically ordered list of
  the absent/stale agent-tier sections the work-list reports (the Guided-Tour
  pages, then the present consolidated documentation pages), each carrying its
  target slug, a `grounding:` directive where the page is
  doc-grounded, and a runnable `logos wiki write …` skeleton (slug positional,
  body on stdin, anchors and free-text fields prefilled). Take an item, read the
  named docs (or the graph, per the directive), summarize them into the body, and
  run it.
- `--json` emits the same queue as one compact object (`{"items": […]}`), each
  item carrying `category` (including `consolidated-doc`), `slug`, `title`,
  `anchor`, `reason`, the runnable `command`, and — where doc-grounded — a
  `grounding` object (`sources`, `fallback_to_code`, `directive`). It is
  **byte-identical** for a fixed `wiki.db` + graph revision.
- It is a **pure read** — never native-tier content, no `wiki.db` write, no LLM,
  no network. An empty work-list prints `Nothing to generate — the wiki
  work-list is empty.` and exits 0 (never a fabricated queue).

You still write the prose — `wiki generate` only formats *what* to write and the
exact command to store it. The loop becomes: `wiki generate` → for each item,
read the graph → fill in the body → run the skeleton → repeat until the queue is
empty.

## Automated trigger: the Claude Code augmentation hook

So the loop runs off the request path without you having to remember it, Logos
can install a **Claude Code augmentation hook** that surfaces the queue to you
automatically — while keeping the binary fully offline:

```bash
logos wiki hook --emit          # install (or no-op if already present)
logos wiki hook --emit --force  # re-install after a binary upgrade
```

`logos init -i` installs it **default-on** alongside the embedded skill. It
writes a marker-tagged PostToolUse hook script and merges a hook entry into
`.claude/settings.json` **without clobbering** existing settings (a foreign
config is never overwritten; a re-run is idempotent; `--force` re-emits). After
an `index`/`sync`, the hook runs `logos wiki generate` and hands the resulting
queue to *you* (the agent) as PostToolUse `additionalContext` — the prompt to
work the loop arrives on its own. The hook is **non-blocking** (it always exits
0) and emits nothing when the work-list is empty.

The division of labour holds: the **binary** only emits a shell script and runs
the offline `wiki generate` read — it makes no LLM call and opens no outbound
connection. **You**, the connected agent, generate the prose. The trigger is
automated; the generation is not.

## Offline guarantee

Generating wiki content with this skill involves an LLM (you), running in the
agent. The Logos binary itself stays fully local: `wiki
write/read/search/status/generate` make **zero outbound connections**, and
installing the augmentation hook (`wiki hook --emit`, `init -i`) is pure local
filesystem I/O. Never add a step that fetches remote content into a page without
the user's explicit instruction.
