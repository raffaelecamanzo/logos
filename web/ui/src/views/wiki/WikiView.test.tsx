import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { WikiHit, WikiNav, WikiPageView, WikiStatus } from "../../api/types.ts";
import { ThemeProvider } from "../../theme/ThemeProvider.tsx";
import { useTheme } from "../../theme/theme.ts";

// The Mermaid seam is mocked so jsdom never loads the 3 MB vendored UMD bundle; the
// reader test asserts the reader *invokes* it once the safe HTML has mounted.
vi.mock("./mermaid.ts", () => ({
  renderMermaidIn: vi.fn(() => Promise.resolve()),
  VENDORED_MERMAID_URL: "/assets/vendor/mermaid.min.js",
}));

import { renderMermaidIn } from "./mermaid.ts";
import { WikiView } from "./WikiView.tsx";

const NAV: WikiNav = {
  tiers: [
    { title: "Summary", items: [{ slug: "overview/project-overview", label: "Project Overview" }] },
    { title: "Design", items: [{ slug: "overview/architecture", label: "Architecture" }] },
    { title: "Specs", items: [{ slug: "specs/functional-requirements", label: "Functional Requirements" }] },
  ],
  search_label: "Search",
};

const FRESH_STATUS: WikiStatus = {
  page_count: 3,
  fresh_count: 3,
  stale_count: 0,
  missing_anchor_count: 0,
  revision_stale_count: 0,
  current_revision: 9,
  freshness_fraction: 1,
};

/** Route a stubbed fetch by URL fragment, longest fragment first (so `/wiki/nav`
 *  wins over `/wiki`). A `null` body answers 404. */
function stubRoutes(routes: Record<string, unknown>) {
  const entries = Object.entries(routes).sort((a, b) => b[0].length - a[0].length);
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      const url = typeof input === "string" ? input : String((input as Request).url ?? input);
      for (const [frag, body] of entries) {
        if (url.includes(frag)) {
          return body === null
            ? Promise.resolve({ ok: false, status: 404 } as Response)
            : Promise.resolve({ ok: true, json: () => Promise.resolve(body) } as Response);
        }
      }
      return Promise.resolve({ ok: false, status: 404 } as Response);
    }),
  );
}

function go(path: string) {
  window.history.pushState({}, "", path);
}

/** Render WikiView inside the ThemeProvider it requires in the real app. */
function renderWiki() {
  return render(
    <ThemeProvider>
      <WikiView />
    </ThemeProvider>,
  );
}

beforeEach(() => {
  vi.mocked(renderMermaidIn).mockClear();
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  window.history.pushState({}, "", "/");
});

describe("WikiView landing over mocked /api/v1 (S-189, FR-UI-06)", () => {
  it("renders the four-tier menu, the freshness banner, and the IA intro", async () => {
    go("/wiki");
    stubRoutes({ "/api/v1/wiki/nav": NAV, "/api/v1/wiki": FRESH_STATUS });
    renderWiki();
    // The four-tier menu (Summary / Design / Specs + Search).
    const menu = await screen.findByRole("navigation", { name: "Wiki navigation" });
    for (const tier of ["Summary", "Design", "Specs"]) {
      expect(within(menu).getByText(tier)).toBeInTheDocument();
    }
    expect(within(menu).getByRole("link", { name: "Search" })).toBeInTheDocument();
    // The verdict-first freshness banner reads the exact "all fresh" phrase. It is
    // fed by a SEPARATE `/api/v1/wiki` status fetch from the nav above, so it must
    // be awaited (`findByText`) — a synchronous `getByText` races the status
    // resolution and flakes under full-suite parallel load.
    expect(await screen.findByText(/all fresh/i)).toBeInTheDocument();
    expect(screen.getByText(/agent page\(s\) stored/i)).toBeInTheDocument();
  });
});

describe("WikiView page reader (S-189, FR-UI-06)", () => {
  const PAGE: WikiPageView = {
    slug: "overview/project-overview",
    title: "Project Overview",
    // Server-rendered comrak HTML: a GFM table, an anchored heading, and a mermaid block.
    rendered_html:
      '<p>Intro prose.</p><h2 id="toc-design">Design</h2><table><thead><tr><th>A</th></tr></thead><tbody><tr><td>1</td></tr></tbody></table><div class="mermaid">graph LR; a--&gt;b</div>',
    placeholder: false,
    generator: "logos-wiki",
    written_head: "abcdef0123456789",
    marker: "generated content — not extracted",
    built_at_revision: 9,
    anchors: [{ kind: "function", entity_id: "scip::f", freshness: "fresh" }],
    stale: false,
    has_missing: false,
    regen_pending: false,
    current_revision: 9,
  };

  it("mounts the server-rendered safe HTML, shows the single title, and runs Mermaid", async () => {
    go("/wiki/page/overview/project-overview");
    stubRoutes({ "/api/v1/wiki/nav": NAV, "/api/v1/wiki/page/": PAGE });
    renderWiki();

    // The single page h1 title (rendered once, from the metadata).
    expect(await screen.findByRole("heading", { level: 1, name: "Project Overview" })).toBeInTheDocument();
    // The GFM table from the server-rendered body is mounted as a real <table>
    // (the page also carries the anchors DataTable, so there are two tables).
    expect(screen.getAllByRole("table").length).toBeGreaterThanOrEqual(2);
    expect(screen.getByRole("columnheader", { name: "A" })).toBeInTheDocument();
    expect(screen.getByText("Intro prose.")).toBeInTheDocument();
    // The provenance line names the generator.
    expect(screen.getByText(/generator: logos-wiki/i)).toBeInTheDocument();
    // The per-anchor freshness table renders.
    expect(screen.getByText("scip::f")).toBeInTheDocument();
    // The "On this page" rail picks up the server-injected toc id.
    const toc = screen.getByRole("navigation", { name: "On this page" });
    expect(within(toc).getByRole("link", { name: "Design" })).toBeInTheDocument();
    // Mermaid is invoked client-side once the safe HTML has mounted.
    await waitFor(() => expect(renderMermaidIn).toHaveBeenCalledTimes(1));
  });

  it("surfaces the staleness signal badges and the regeneration-pending banner", async () => {
    go("/wiki/page/overview/project-overview");
    const stalePage: WikiPageView = {
      ...PAGE,
      rendered_html: "<p>Body.</p>",
      stale: true,
      has_missing: true,
      regen_pending: true,
      built_at_revision: 7,
      current_revision: 9,
    };
    stubRoutes({ "/api/v1/wiki/nav": NAV, "/api/v1/wiki/page/": stalePage });
    renderWiki();
    // All three staleness signals render as text-bearing badges (a11y: colour + text).
    expect(await screen.findByText("STALE")).toBeInTheDocument();
    expect(screen.getByText("MISSING ANCHOR")).toBeInTheDocument();
    expect(screen.getByText("REGEN PENDING")).toBeInTheDocument();
    // The derived "regeneration pending" banner names both revisions (FR-WK-12).
    expect(screen.getByText(/regeneration pending/i)).toBeInTheDocument();
    expect(screen.getByText(/built at @revision 7, graph now @revision 9/i)).toBeInTheDocument();
  });

  it("renders the honest 'not yet generated' placeholder for a scaffold slug", async () => {
    go("/wiki/page/overview/getting-started");
    const placeholder: WikiPageView = {
      ...PAGE,
      slug: "overview/getting-started",
      title: "Getting Started",
      rendered_html: "",
      placeholder: true,
      generator: null,
      written_head: null,
      marker: null,
      built_at_revision: null,
      anchors: [],
    };
    stubRoutes({ "/api/v1/wiki/nav": NAV, "/api/v1/wiki/page/": placeholder });
    renderWiki();
    // The honest "not yet generated" state is stated in the verdict and the card.
    expect((await screen.findAllByText(/not yet generated/i)).length).toBeGreaterThan(0);
    expect(screen.getByText(/No agent prose for this section yet/i)).toBeInTheDocument();
  });

  it("renders an honest error panel when the page read 404s", async () => {
    go("/wiki/page/no/such/slug");
    stubRoutes({ "/api/v1/wiki/nav": NAV, "/api/v1/wiki/page/": null });
    renderWiki();
    await waitFor(() => expect(screen.getByRole("alert")).toBeInTheDocument());
  });

  it("paginates the page-anchors table at 20 rows/page (S-195, FR-UI-11)", async () => {
    go("/wiki/page/overview/project-overview");
    const manyAnchors: WikiPageView = {
      ...PAGE,
      anchors: Array.from({ length: 25 }, (_, i) => ({
        kind: "function",
        entity_id: `scip::f${String(i).padStart(2, "0")}`,
        freshness: "fresh",
      })),
    };
    stubRoutes({ "/api/v1/wiki/nav": NAV, "/api/v1/wiki/page/": manyAnchors });
    renderWiki();
    const table = await screen.findByRole("table", { name: "Page anchors" });
    // 20 body rows + the header row — the previously-unpaginated anchors table caps at 20.
    expect(within(table).getAllByRole("row").length).toBe(20 + 1);
    expect(screen.getByText(/Showing 1–20 of 25/)).toBeInTheDocument();
  });

  it("re-renders Mermaid diagrams when the theme toggles (S-196, ADR-44)", async () => {
    go("/wiki/page/overview/project-overview");
    stubRoutes({ "/api/v1/wiki/nav": NAV, "/api/v1/wiki/page/": PAGE });

    // Render with a minimal ThemeToggle button alongside WikiView so the
    // test can drive a theme change through the real ThemeContext.
    // toggleTheme() always flips, so this works regardless of the initial
    // jsdom theme (matchMedia.matches:false → dark default → toggles to light).
    function ThemeToggle() {
      const { toggleTheme } = useTheme();
      return <button onClick={toggleTheme}>Toggle theme</button>;
    }

    render(
      <ThemeProvider>
        <ThemeToggle />
        <WikiView />
      </ThemeProvider>,
    );

    // Wait for the initial render + first Mermaid call.
    await waitFor(() => expect(renderMermaidIn).toHaveBeenCalledTimes(1));

    // Simulate Mermaid marking the diagram processed (the real bundle does this).
    document.querySelectorAll(".mermaid").forEach((el) => el.setAttribute("data-processed", "true"));

    // Trigger the theme toggle — this should fire the useEffect([rendered_html, theme])
    // and call renderMermaidIn a second time with the updated themeVariables.
    fireEvent.click(screen.getByRole("button", { name: "Toggle theme" }));

    await waitFor(() => expect(renderMermaidIn).toHaveBeenCalledTimes(2));
  });
});

describe("WikiView search (S-189, FR-WK-05)", () => {
  const HITS: WikiHit[] = [
    {
      slug: "overview/project-overview",
      title: "Project Overview",
      generator: "logos-wiki",
      written_head: "abc",
      stale: false,
      has_missing: false,
      built_at_revision: 9,
      revision_pending: false,
    },
  ];

  it("pre-fills the query from the URL and lists the staleness-flagged hits", async () => {
    go("/wiki/search?q=project");
    stubRoutes({ "/api/v1/wiki/nav": NAV, "/api/v1/wiki/search": HITS });
    renderWiki();
    const table = await screen.findByRole("table", { name: "Search results" });
    expect(within(table).getByText("Project Overview")).toBeInTheDocument();
    // A fresh hit carries the FRESH state badge (colour + text).
    expect(within(table).getByText("FRESH")).toBeInTheDocument();
  });

  it("paginates the search results at 20 rows/page (S-195, FR-UI-11)", async () => {
    go("/wiki/search?q=project");
    const hits: WikiHit[] = Array.from({ length: 25 }, (_, i) => ({
      ...HITS[0],
      slug: `overview/page-${String(i).padStart(2, "0")}`,
      title: `Page ${i}`,
    }));
    stubRoutes({ "/api/v1/wiki/nav": NAV, "/api/v1/wiki/search": hits });
    render(<WikiView />);
    const table = await screen.findByRole("table", { name: "Search results" });
    // 20 body rows + the header row — the previously-unpaginated table now caps at 20.
    expect(within(table).getAllByRole("row").length).toBe(20 + 1);
    expect(screen.getByText(/Showing 1–20 of 25/)).toBeInTheDocument();
  });

  it("flags a stale / missing-anchor / regeneration-pending hit with its badges", async () => {
    go("/wiki/search?q=project");
    const staleHit: WikiHit = {
      ...HITS[0],
      stale: true,
      has_missing: true,
      revision_pending: true,
    };
    stubRoutes({ "/api/v1/wiki/nav": NAV, "/api/v1/wiki/search": [staleHit] });
    renderWiki();
    const table = await screen.findByRole("table", { name: "Search results" });
    // The same staleness vocabulary the page reader uses — never just FRESH.
    expect(within(table).getByText("REGEN PENDING")).toBeInTheDocument();
    expect(within(table).getByText("STALE")).toBeInTheDocument();
    expect(within(table).getByText("MISSING ANCHOR")).toBeInTheDocument();
    expect(within(table).queryByText("FRESH")).not.toBeInTheDocument();
  });

  it("shows the type-to-search hint when the query is empty", async () => {
    go("/wiki/search");
    stubRoutes({ "/api/v1/wiki/nav": NAV, "/api/v1/wiki/search": [] });
    renderWiki();
    expect(await screen.findByText(/Type to search the wiki/i)).toBeInTheDocument();
  });
});

describe("WikiView search result jump-to-match (S-271, FR-WK-28)", () => {
  const HIT: WikiHit = {
    slug: "overview/project-overview",
    title: "Project Overview",
    generator: "logos-wiki",
    written_head: "abc",
    stale: false,
    has_missing: false,
    built_at_revision: 9,
    revision_pending: false,
  };

  it("clicking a result carries the query term and scrolls to + <mark>-highlights the first match", async () => {
    go("/wiki/search?q=sandbox");
    const page: WikiPageView = {
      slug: HIT.slug,
      title: HIT.title,
      rendered_html: "<p>Intro prose about the sandbox escape refusal.</p>",
      placeholder: false,
      generator: "logos-wiki",
      written_head: "abcdef0123456789",
      marker: "generated content — not extracted",
      built_at_revision: 9,
      anchors: [],
      stale: false,
      has_missing: false,
      regen_pending: false,
      current_revision: 9,
    };
    stubRoutes({ "/api/v1/wiki/nav": NAV, "/api/v1/wiki/search": [HIT], "/api/v1/wiki/page/": page });
    renderWiki();

    const table = await screen.findByRole("table", { name: "Search results" });
    const scrollSpy = vi.spyOn(Element.prototype, "scrollIntoView").mockImplementation(() => {});
    fireEvent.click(within(table).getByRole("link", { name: HIT.slug }));

    // Navigating to the reader mounts the page (server-rendered body, single h1).
    expect(await screen.findByRole("heading", { level: 1, name: "Project Overview" })).toBeInTheDocument();
    const mark = document.querySelector("mark.wiki-search-hit");
    expect(mark).not.toBeNull();
    expect(mark?.textContent).toBe("sandbox");
    expect(scrollSpy).toHaveBeenCalled();
  });

  it("opens at the top with no error and no highlight when the term is absent from the body", async () => {
    go("/wiki/search?q=nonexistentterm");
    const page: WikiPageView = {
      slug: HIT.slug,
      title: HIT.title,
      rendered_html: "<p>Body prose with no relation to the query.</p>",
      placeholder: false,
      generator: "logos-wiki",
      written_head: "abcdef0123456789",
      marker: "generated content — not extracted",
      built_at_revision: 9,
      anchors: [],
      stale: false,
      has_missing: false,
      regen_pending: false,
      current_revision: 9,
    };
    stubRoutes({ "/api/v1/wiki/nav": NAV, "/api/v1/wiki/search": [HIT], "/api/v1/wiki/page/": page });
    renderWiki();

    const table = await screen.findByRole("table", { name: "Search results" });
    fireEvent.click(within(table).getByRole("link", { name: HIT.slug }));

    expect(await screen.findByRole("heading", { level: 1, name: "Project Overview" })).toBeInTheDocument();
    expect(document.querySelector("mark.wiki-search-hit")).toBeNull();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("opens at the top with no highlight when the page is reached without a search term (menu navigation)", async () => {
    go("/wiki");
    const page: WikiPageView = {
      slug: "overview/architecture",
      title: "Architecture",
      rendered_html: "<p>Sandbox escape is discussed here too.</p>",
      placeholder: false,
      generator: "logos-wiki",
      written_head: "abcdef0123456789",
      marker: "generated content — not extracted",
      built_at_revision: 9,
      anchors: [],
      stale: false,
      has_missing: false,
      regen_pending: false,
      current_revision: 9,
    };
    stubRoutes({ "/api/v1/wiki/nav": NAV, "/api/v1/wiki": FRESH_STATUS, "/api/v1/wiki/page/": page });
    renderWiki();

    const menu = await screen.findByRole("navigation", { name: "Wiki navigation" });
    fireEvent.click(within(menu).getByRole("link", { name: "Architecture" }));

    expect(await screen.findByRole("heading", { level: 1, name: "Architecture" })).toBeInTheDocument();
    expect(document.querySelector("mark.wiki-search-hit")).toBeNull();
  });

  it("clears a stale highlight when the same page is re-reached with no search term (menu re-click)", async () => {
    go("/wiki/search?q=sandbox");
    const page: WikiPageView = {
      slug: HIT.slug,
      title: HIT.title,
      rendered_html: "<p>Intro prose about the sandbox escape refusal.</p>",
      placeholder: false,
      generator: "logos-wiki",
      written_head: "abcdef0123456789",
      marker: "generated content — not extracted",
      built_at_revision: 9,
      anchors: [],
      stale: false,
      has_missing: false,
      regen_pending: false,
      current_revision: 9,
    };
    stubRoutes({ "/api/v1/wiki/nav": NAV, "/api/v1/wiki/search": [HIT], "/api/v1/wiki/page/": page });
    renderWiki();
    vi.spyOn(Element.prototype, "scrollIntoView").mockImplementation(() => {});

    // Reach the page via the search result — it gets highlighted (as above).
    const table = await screen.findByRole("table", { name: "Search results" });
    fireEvent.click(within(table).getByRole("link", { name: HIT.slug }));
    await screen.findByRole("heading", { level: 1, name: "Project Overview" });
    expect(document.querySelector("mark.wiki-search-hit")).not.toBeNull();

    // Re-clicking the SAME page's own (now-active) menu entry carries no search
    // term. The slug is unchanged, so the fetched page (and its rendered_html)
    // never changes either — the stale mark must still be cleared explicitly,
    // not merely rely on the body being re-mounted.
    const menu = screen.getByRole("navigation", { name: "Wiki navigation" });
    fireEvent.click(within(menu).getByRole("link", { name: "Project Overview" }));

    await waitFor(() => expect(document.querySelector("mark.wiki-search-hit")).toBeNull());
  });
});
