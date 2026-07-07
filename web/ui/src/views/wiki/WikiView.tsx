/*
 * WikiView (S-189, FR-UI-06, FR-UI-21) — the Wiki tab migrated to React over
 * `/api/v1`. It owns its own client sub-routes inside one mounted view: the landing
 * (`/wiki`), the full-text search (`/wiki/search`), and the agent-page reader
 * (`/wiki/page/*`), in the shared three-column layout (the four-tier menu / content /
 * "On this page" rail). It preserves the wiki's information architecture: the
 * four-tier menu (Summary, Design, Specs + Search), the single page `h1` title, GFM
 * tables, and client-side Mermaid — by MOUNTING the server-rendered, already-safe
 * comrak HTML the `/api/v1/wiki/page` bundle returns (the XSS boundary stays on the
 * server) and rendering its `.mermaid` blocks with the vendored bundle as today.
 * Consumes the shared `/api/v1` data-access layer and renders exclusively through the
 * S-193 design system. Every read is GET-only — loading the view mutates no store
 * (ADR-28).
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { useTheme } from "../../theme/theme.ts";

import {
  fetchWikiNav,
  fetchWikiPage,
  fetchWikiStatus,
  searchWiki,
} from "../../api/client.ts";
import { fetchWikiConfig } from "../../api/wikiGenClient.ts";
import { AsyncResource, useApiResource } from "../../api/hooks.tsx";
import type { ConfigReadModel, WikiHit, WikiNav, WikiPageView } from "../../api/types.ts";
import { Badge, Button, Callout, Card, DataTable, DEFAULT_TABLE_PAGE_SIZE, EmptyState, TextField } from "../../components/index.ts";
import type { BadgeTone, Column } from "../../components/index.ts";
import { navigate, useNavigationState, usePathname } from "../../router.tsx";
import { renderMermaidIn } from "./mermaid.ts";
import { clearHighlight, highlightFirstMatch } from "./searchHighlight.ts";
import {
  extractToc,
  freshnessSummary,
  pageHref,
  wikiRoute,
  type TocEntry,
} from "./wikiModel.ts";
import {
  hasWikiConsent,
  isWikiConfigured,
  rememberWikiConsent,
  wikiDisclosure,
  type WikiDisclosure,
  type WikiGenState,
} from "./wikiGenModel.ts";
import { useWikiGeneration } from "./wikiRuntime.tsx";
import styles from "./WikiView.module.css";

export function WikiView() {
  const pathname = usePathname();
  const route = wikiRoute(pathname);

  // The config read-model drives the configure-first gate + the consent disclosure
  // (NFR-SE-07). It is additive to the wiki content: an unresolved/errored config
  // never blocks browsing — it just leaves generation unavailable.
  const config = useApiResource<ConfigReadModel>(() => fetchWikiConfig(), []);
  const configModel = config.status === "ready" ? config.data : undefined;
  const configured = configModel ? isWikiConfigured(configModel) : false;

  // First-use consent (NFR-SE-07): remembered across loads; the trigger fires only
  // once it is accepted.
  const [consented, setConsented] = useState<boolean>(() => hasWikiConsent());
  const acceptConsent = useCallback(() => {
    rememberWikiConsent();
    setConsented(true);
  }, []);

  // The trigger activates only with a configured provider AND accepted consent; the
  // work-list check + configure-first are decided server-side by the runner.
  const { state: gen, refreshKey } = useWikiGeneration(configured && consented);

  const withToc = route.kind === "page";
  // A page write bumps `refreshKey`, so the menu (and each read-model below) reloads
  // as pages stream in — existing pages stay browsable, refreshes stream in.
  const nav = useApiResource<WikiNav>(() => fetchWikiNav(), [refreshKey]);

  return (
    <div className={styles.view}>
      {configModel && !configured && <WikiConfigureFirst />}
      {configModel && configured && !consented && (
        <WikiConsentBanner disclosure={wikiDisclosure(configModel)} onAccept={acceptConsent} />
      )}
      {configured && consented && <WikiGenerationStatus state={gen} />}
      <AsyncResource resource={nav} loadingLabel="Loading the wiki…">
        {(menu) => (
          <div className={withToc ? `${styles.layout} ${styles.layoutWithToc}` : styles.layout}>
            <WikiMenu nav={menu} pathname={pathname} />
            {route.kind === "landing" && <WikiLanding nav={menu} refreshKey={refreshKey} />}
            {route.kind === "search" && <WikiSearch />}
            {route.kind === "page" && <WikiPageReader slug={route.slug} refreshKey={refreshKey} />}
          </div>
        )}
      </AsyncResource>
    </div>
  );
}

/** The honest configure-first state ([FR-UI-18], [NFR-CC-04]): a muted callout into
 *  the Config tab — NOT an error. The wiki stays browsable; only generation is
 *  unavailable until a provider (and key) is set. */
function WikiConfigureFirst() {
  return (
    <Callout label="CONFIGURE" tone="muted">
      <p>
        Wiki generation needs an LLM provider before it can refresh pages. Set the
        provider, model, and API key in the <a href="/config">Config</a> tab (or a
        dedicated <code>[wiki].model</code>), then return here. Until then no outbound
        call is possible and the pages below stay read-only.
      </p>
    </Callout>
  );
}

/** The first-use consent disclosure (NFR-SE-07): names the endpoint and what is sent
 *  before any outbound call; generation does not start until it is accepted. */
function WikiConsentBanner({
  disclosure,
  onAccept,
}: {
  disclosure: WikiDisclosure;
  onAccept: () => void;
}) {
  return (
    <Callout label="BEFORE GENERATING" tone="warm" className={styles.consent}>
      <p>
        Opening the Wiki refreshes stale pages by sending <strong>source and graph
        excerpts</strong> from this project to <strong>{disclosure.endpointHost}</strong>{" "}
        (the configured <code>{disclosure.provider}</code> endpoint). Nothing is sent
        until you allow it.
      </p>
      <p className={styles.providerLine}>
        {disclosure.provider} · {disclosure.endpointHost} · {disclosure.model}
      </p>
      <Button variant="primary" onClick={onAccept}>
        Allow generation
      </Button>
    </Callout>
  );
}

/** The live generation banner: what the background run is doing, streamed from the
 *  wiki-agent's per-page progress. Honest by construction ([NFR-CC-04]) — a halt,
 *  fault, or busy state is named, never hidden. Renders nothing until a run reports. */
function WikiGenerationStatus({ state }: { state: WikiGenState }) {
  if (state.phase === "idle") return null;
  if (state.phase === "busy") {
    return (
      <Callout label="WIKI" tone="muted">
        <span>A wiki generation run is already in progress — its refreshes will appear here.</span>
      </Callout>
    );
  }
  if (state.phase === "error") {
    return (
      <Callout label="WIKI" tone="signal">
        <span>{state.message ?? "Wiki generation could not complete."}</span>
      </Callout>
    );
  }
  if (state.phase === "configure-first") {
    // The runner reported configure-first (e.g. the key was cleared) — mirror the
    // static notice honestly rather than showing an error.
    return (
      <Callout label="WIKI" tone="muted">
        <span>{state.message ?? "Wiki generation is not configured."}</span>
      </Callout>
    );
  }
  // A `halted` frame can land before the terminal `completed` one (each SSE frame
  // triggers its own render), so `state.phase` may still read `"running"` for a beat
  // after `state.halted` is already known. Deciding `halted` first — and folding it
  // into `running` — keeps that window from rendering the self-contradictory
  // "Generating… · … · halted: …" (with a stale "writing X" / timeout hint still
  // attached to a page that already stopped) ([FR-UI-24], [S-239]).
  const halted = state.halted !== null;
  const running = state.phase === "running" && !halted;
  const headline = halted ? "Generation halted" : running ? "Generating…" : "Generation complete";
  const tone = halted ? "warm" : running ? "signal" : "pass";
  const detailParts: string[] = [];
  if (state.total > 0) {
    detailParts.push(`${state.written.length}/${state.total} page(s) refreshed`);
  } else {
    detailParts.push(`${state.written.length} page(s) refreshed`);
  }
  if (state.failed.length > 0) detailParts.push(`${state.failed.length} failed`);
  if (halted) detailParts.push(`halted: ${state.halted}`);
  return (
    <Callout label="WIKI" tone={tone}>
      <span>
        <strong>{headline}</strong> · {detailParts.join(" · ")}
        {running && state.current ? ` · writing ${state.current}` : ""}
        {running && state.current && state.synthesisTimeoutSecs
          ? ` (a page may take up to ${state.synthesisTimeoutSecs}s to synthesize — a liveness guard, not a hang)`
          : ""}
      </span>
    </Callout>
  );
}

/** A same-origin in-SPA wiki link (a real `<a href>` for accessibility + middle-click,
 *  intercepted to client-navigate). The link matching the current path is current. */
function WikiLink({
  href,
  pathname,
  className,
  children,
  state,
}: {
  href: string;
  pathname: string;
  className: string;
  children: React.ReactNode;
  /** Optional navigation-state payload (FR-WK-28) — e.g. the search term a hit
   *  link carries into the reader for the jump-to-match highlight. */
  state?: unknown;
}) {
  const active = pathname === href;
  return (
    <a
      className={className}
      href={href}
      aria-current={active ? "page" : undefined}
      onClick={(e) => {
        e.preventDefault();
        navigate(href, state);
      }}
    >
      {children}
    </a>
  );
}

/** The four-tier doc-tree menu (the wiki IA): the Summary / Design / Specs tiers of
 *  discrete page links plus the top-level Search link. */
function WikiMenu({ nav, pathname }: { nav: WikiNav; pathname: string }) {
  return (
    <nav className={styles.menu} aria-label="Wiki navigation">
      {nav.tiers.map((tier) => (
        <details className={styles.tier} open key={tier.title}>
          <summary>{tier.title}</summary>
          <ul className={styles.menuList}>
            {tier.items.map((item) => (
              <li key={item.slug}>
                <WikiLink href={pageHref(item.slug)} pathname={pathname} className={styles.menuLink}>
                  {item.label}
                </WikiLink>
              </li>
            ))}
          </ul>
        </details>
      ))}
      <WikiLink href="/wiki/search" pathname={pathname} className={styles.topLink}>
        {nav.search_label}
      </WikiLink>
    </nav>
  );
}

/** The Start-here landing: the verdict-first freshness banner, the Start-here callout
 *  into the Summary tier, and the IA intro. `refreshKey` bumps as generation writes
 *  pages, so the freshness banner re-reads live during a run ([FR-WK-18]). */
function WikiLanding({ nav, refreshKey }: { nav: WikiNav; refreshKey: number }) {
  const status = useApiResource(() => fetchWikiStatus(), [refreshKey]);
  const firstSummary = nav.tiers[0]?.items[0];
  return (
    <div className={styles.content}>
      <AsyncResource resource={status} loadingLabel="Loading the wiki…">
        {(s) => {
          const summary = freshnessSummary(s);
          return (
            <>
              <Callout label="WIKI" tone={summary.tone}>
                <span>
                  <strong>{summary.state}</strong> · {summary.detail}
                </span>
              </Callout>
              {firstSummary && (
                <Card title="Start here">
                  <p className={styles.landingIntro}>
                    New to this codebase? Start with the <strong>Summary</strong> —
                    synthesized prose that orients you before you dive into the design
                    and specs.{" "}
                    <a
                      href={pageHref(firstSummary.slug)}
                      onClick={(e) => {
                        e.preventDefault();
                        navigate(pageHref(firstSummary.slug));
                      }}
                    >
                      Begin with {firstSummary.label} →
                    </a>
                  </p>
                </Card>
              )}
              <Card title="Welcome to the wiki">
                <p className={styles.landingIntro}>
                  The wiki is a per-page documentation site. Browse it from the menu:
                  the <strong>Summary</strong> (synthesized prose),{" "}
                  <strong>Design</strong> (the architecture narrative and the
                  consolidated ADRs, Components, Integrations, and Frontend Design
                  documents), <strong>Specs</strong> (the consolidated requirements and
                  acceptance tests), and a top-level <strong>Search</strong> link. Agent
                  prose carries its generated-content marker.
                </p>
                <p className={styles.landingMeta}>{s.page_count} agent page(s) stored.</p>
              </Card>
            </>
          );
        }}
      </AsyncResource>
    </div>
  );
}

/** Debounce `value` by `ms` — keeps the live search from issuing a read per keystroke
 *  (the legacy htmx 200ms `keyup changed delay`). */
function useDebounced<T>(value: T, ms: number): T {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const id = setTimeout(() => setDebounced(value), ms);
    return () => clearTimeout(id);
  }, [value, ms]);
  return debounced;
}

/** The self-sufficient FTS search page: a pre-filled input that live-searches over
 *  `/api/v1/wiki/search`, plus the staleness-flagged results table. */
function WikiSearch() {
  const initial = new URLSearchParams(window.location.search).get("q") ?? "";
  const [query, setQuery] = useState(initial);
  const term = useDebounced(query.trim(), 200);
  const results = useApiResource<WikiHit[]>(() => searchWiki(term), [term]);
  return (
    <div className={styles.content}>
      <Card title="Search">
        <TextField
          label="Search the wiki"
          type="search"
          value={query}
          placeholder="Search titles and bodies…"
          autoComplete="off"
          onChange={(e) => setQuery(e.target.value)}
        />
        <div aria-live="polite">
          {term === "" ? (
            <p className={styles.tocEmpty}>Type to search the wiki.</p>
          ) : (
            <AsyncResource
              resource={results}
              loadingLabel="Searching…"
              isEmpty={(hits) => hits.length === 0}
              empty={<EmptyState message={`No wiki pages match “${term}”.`} />}
            >
              {(hits) => <SearchResults hits={hits} term={term} />}
            </AsyncResource>
          )}
        </div>
      </Card>
    </div>
  );
}

/** The state badge(s) for a hit — the same vocabulary the page reader shows: REGEN
 *  PENDING (orange), STALE / MISSING ANCHOR (red), else FRESH (green). */
function HitBadges({ hit }: { hit: WikiHit }) {
  const badges: { tone: BadgeTone; label: string }[] = [];
  if (hit.revision_pending) badges.push({ tone: "orange", label: "REGEN PENDING" });
  if (hit.stale) badges.push({ tone: "red", label: "STALE" });
  if (hit.has_missing) badges.push({ tone: "red", label: "MISSING ANCHOR" });
  if (badges.length === 0) badges.push({ tone: "green", label: "FRESH" });
  return (
    <span className={styles.badges}>
      {badges.map((b) => (
        <Badge key={b.label} tone={b.tone}>
          {b.label}
        </Badge>
      ))}
    </span>
  );
}

function SearchResults({ hits, term }: { hits: WikiHit[]; term: string }) {
  const pathname = usePathname();
  const columns: Column<WikiHit>[] = [
    { key: "state", header: "State", cell: (h) => <HitBadges hit={h} /> },
    { key: "title", header: "Title", sortValue: (h) => h.title, cell: (h) => h.title },
    {
      key: "page",
      header: "Page",
      mono: true,
      sortValue: (h) => h.slug,
      cell: (h) => (
        // The query term rides in navigation state (FR-WK-28) — the reader reads
        // it to scroll to + highlight the first match; no URL param, no new
        // read-model field.
        <WikiLink href={pageHref(h.slug)} pathname={pathname} className={styles.menuLink} state={{ q: term }}>
          {h.slug}
        </WikiLink>
      ),
    },
    { key: "gen", header: "Generator", mono: true, sortValue: (h) => h.generator, cell: (h) => h.generator },
  ];
  return (
    <DataTable
      caption="Search results"
      columns={columns}
      rows={hits}
      rowKey={(h) => h.slug}
      pageSize={DEFAULT_TABLE_PAGE_SIZE}
    />
  );
}

/** The agent-page reader: the provenance callout, the single `h1` title, the
 *  server-rendered safe HTML body (with client Mermaid), the per-anchor freshness
 *  table, and the "On this page" rail. Returns two grid children (content + TOC).
 *  `refreshKey` bumps when generation writes a page, so an open page reloads its
 *  freshly-generated body as the run streams ([FR-WK-18]). */
function WikiPageReader({ slug, refreshKey }: { slug: string; refreshKey: number }) {
  const page = useApiResource<WikiPageView>(() => fetchWikiPage(slug), [slug, refreshKey]);
  // The search term a result link carried in navigation state (FR-WK-28); absent
  // for every other way of reaching a page (menu, direct load), so those open at
  // the top with no highlight by construction.
  const navState = useNavigationState<{ q?: string }>();
  const highlightTerm = navState?.q?.trim() || undefined;
  return (
    <AsyncResource resource={page} loadingLabel="Loading the page…">
      {(p) => <WikiPageBody page={p} highlightTerm={highlightTerm} />}
    </AsyncResource>
  );
}

function WikiPageBody({ page, highlightTerm }: { page: WikiPageView; highlightTerm?: string }) {
  const proseRef = useRef<HTMLDivElement>(null);
  const { theme } = useTheme();
  // Stores the original innerHTML of each .mermaid element before Mermaid
  // processes it into SVG, so the element can be restored for re-render when
  // the theme changes. Keyed by DOM element (refs survive React re-renders as
  // long as dangerouslySetInnerHTML does not replace the element).
  const sources = useRef(new Map<Element, string>());

  // Clear stale source entries when the page content changes (new DOM nodes
  // from dangerouslySetInnerHTML invalidate any previously stored refs).
  useEffect(() => {
    sources.current.clear();
  }, [page.rendered_html]);

  useEffect(() => {
    if (!proseRef.current) return;
    const container = proseRef.current;

    for (const el of container.querySelectorAll(".mermaid")) {
      if (!el.hasAttribute("data-processed")) {
        // Fresh element (new page or restored): capture its source.
        sources.current.set(el, el.innerHTML);
      } else {
        // Already processed (same page, theme toggled): restore source so
        // Mermaid can re-render it with the new themeVariables.
        // Safety: `src` was captured from this element's own innerHTML
        // (server-sanitized comrak output already in the DOM via
        // dangerouslySetInnerHTML above). We are restoring the server's own
        // output, not injecting any new content — the XSS boundary stays on
        // the server exactly as the parent dangerouslySetInnerHTML does.
        const src = sources.current.get(el);
        if (src !== undefined) {
          el.innerHTML = src;
          el.removeAttribute("data-processed");
        }
      }
    }

    void renderMermaidIn(container);
  }, [page.rendered_html, theme]);

  // Jump-to-match (S-271, FR-WK-28): scroll to + <mark> the first occurrence of
  // the search term the result link carried, over the already-rendered body — no
  // extra fetch. A term absent from the body (e.g. it matched only the title)
  // leaves the page open at the top with no error and no highlight. When there is
  // no term, clear any mark left by an earlier search-originated visit to this
  // same page instance (e.g. re-clicking its own already-active menu link) —
  // dangerouslySetInnerHTML only touches the DOM when the HTML string changes, so
  // an unchanged body would otherwise leave a stale highlight behind.
  useEffect(() => {
    if (!proseRef.current) return;
    if (!highlightTerm) {
      clearHighlight(proseRef.current);
      return;
    }
    const mark = highlightFirstMatch(proseRef.current, highlightTerm);
    mark?.scrollIntoView({ block: "center" });
  }, [page.rendered_html, highlightTerm]);

  const toc = extractToc(page.rendered_html);
  const signal = page.regen_pending || page.stale || page.has_missing;

  if (page.placeholder) {
    return (
      <>
        <div className={styles.content}>
          <Callout label="WIKI" tone="muted">
            <span>not yet generated</span>
          </Callout>
          <Card title={page.title}>
            <Callout label="Not yet generated" tone="muted">
              <span>
                No agent prose for this section yet — the embedded logos-wiki skill
                generates it off the <code>wiki status</code> work-list.
              </span>
            </Callout>
          </Card>
        </div>
        <TocRail entries={toc} />
      </>
    );
  }

  return (
    <>
      <div className={styles.content}>
        <Callout label="WIKI" tone={signal ? "signal" : "muted"}>
          <span className={styles.provenance}>
            generator: {page.generator ?? "—"} · {headLabel(page.written_head)} · built
            @revision {page.built_at_revision ?? 0}
          </span>
          {signal && (
            <span className={styles.badges}>
              {page.regen_pending && <Badge tone="orange">REGEN PENDING</Badge>}
              {page.stale && <Badge tone="red">STALE</Badge>}
              {page.has_missing && <Badge tone="red">MISSING ANCHOR</Badge>}
            </span>
          )}
        </Callout>
        <Card>
          <h1 className={styles.title}>{page.title}</h1>
          {page.regen_pending && (
            <Callout label="WIKI" tone="signal">
              <span className={styles.pending}>
                stale — regeneration pending · built at @revision{" "}
                {page.built_at_revision ?? 0}, graph now @revision {page.current_revision}
              </span>
            </Callout>
          )}
          {/*
           * The body is the server-rendered comrak output — already XSS-neutralized
           * (raw HTML / dangerous URLs dropped, mermaid fences rewritten). Mounting it
           * verbatim keeps the safety boundary on the server; no client Markdown engine.
           */}
          <div
            className={styles.prose}
            ref={proseRef}
            dangerouslySetInnerHTML={{ __html: page.rendered_html }}
          />
        </Card>
        {page.anchors.length > 0 && <AnchorsCard page={page} />}
      </div>
      <TocRail entries={toc} />
    </>
  );
}

/** The short HEAD label for the provenance line (mirrors the legacy `provenance_callout`). */
function headLabel(head: string | null): string {
  if (!head) return "no HEAD recorded";
  return `HEAD ${head.slice(0, 12)}`;
}

/** The per-anchor freshness table — every anchor the page documents, with its kind,
 *  entity key, and freshness verdict (badge + text, a11y). */
function AnchorsCard({ page }: { page: WikiPageView }) {
  const columns: Column<WikiPageView["anchors"][number]>[] = [
    {
      key: "fresh",
      header: "Freshness",
      cell: (a) => <Badge tone={a.freshness === "fresh" ? "green" : "red"}>{a.freshness.toUpperCase()}</Badge>,
    },
    { key: "kind", header: "Kind", cell: (a) => a.kind },
    { key: "entity", header: "Entity", mono: true, cell: (a) => a.entity_id },
  ];
  return (
    <Card title="Anchors">
      <DataTable
        caption="Page anchors"
        columns={columns}
        rows={page.anchors}
        rowKey={(a, i) => `${a.entity_id}#${i}`}
        pageSize={DEFAULT_TABLE_PAGE_SIZE}
      />
    </Card>
  );
}

/** The "On this page" rail — discrete in-page links to the body's headings, `h3`
 *  nested under `h2`. Always rendered (an empty page shows an honest note) so the
 *  three-column layout is stable. */
function TocRail({ entries }: { entries: TocEntry[] }) {
  return (
    <nav className={styles.toc} aria-label="On this page">
      <h2 className={styles.tocHeading}>On this page</h2>
      {entries.length === 0 ? (
        <p className={styles.tocEmpty}>No sections on this page.</p>
      ) : (
        <ul className={styles.tocList}>
          {entries.map((e) => (
            <li key={e.id} className={e.level === 3 ? styles.tocSub : undefined}>
              <a href={`#${e.id}`}>{e.label}</a>
            </li>
          ))}
        </ul>
      )}
    </nav>
  );
}
