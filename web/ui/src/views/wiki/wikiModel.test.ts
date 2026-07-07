import { describe, expect, it } from "vitest";

import type { WikiStatus } from "../../api/types.ts";
import { extractToc, freshnessSummary, pageHref, wikiRoute } from "./wikiModel.ts";

describe("wikiRoute sub-route parsing (S-189)", () => {
  it("maps the landing, search, and page paths", () => {
    expect(wikiRoute("/wiki")).toEqual({ kind: "landing" });
    expect(wikiRoute("/wiki/search")).toEqual({ kind: "search" });
    expect(wikiRoute("/wiki/page/overview/getting-started")).toEqual({
      kind: "page",
      slug: "overview/getting-started",
    });
  });

  it("decodes an encoded slug and falls back to the landing for an empty slug", () => {
    expect(wikiRoute("/wiki/page/a%2Fb")).toEqual({ kind: "page", slug: "a/b" });
    expect(wikiRoute("/wiki/page/")).toEqual({ kind: "landing" });
  });

  it("pageHref builds the reader path", () => {
    expect(pageHref("overview/architecture")).toBe("/wiki/page/overview/architecture");
  });
});

describe("extractToc reads the server-injected toc ids (S-189, FR-WK-11)", () => {
  it("collects h2/h3 entries with their ids and tag-stripped labels", () => {
    const html =
      '<h2 id="toc-intro">Intro</h2><p>x</p><h3 id="toc-use-engine">Use <code>Engine</code></h3>';
    expect(extractToc(html)).toEqual([
      { level: 2, id: "toc-intro", label: "Intro" },
      { level: 3, id: "toc-use-engine", label: "Use Engine" },
    ]);
  });

  it("skips an id-less heading (it cannot be linked)", () => {
    expect(extractToc("<h2>No id</h2>")).toEqual([]);
  });
});

describe("freshnessSummary phrasing (S-189, FR-WK-12)", () => {
  function status(over: Partial<WikiStatus>): WikiStatus {
    return {
      page_count: 3,
      fresh_count: 3,
      stale_count: 0,
      missing_anchor_count: 0,
      revision_stale_count: 0,
      current_revision: 9,
      freshness_fraction: 1,
      ...over,
    };
  }

  it("reads FRESH with the exact 'all fresh' phrase when clean on every axis", () => {
    const s = freshnessSummary(status({}));
    expect(s.state).toBe("FRESH");
    expect(s.tone).toBe("pass");
    expect(s.detail).toContain("3 pages · all fresh");
    expect(s.detail).toContain("graph @revision 9");
  });

  it("reads STALE with an honest breakdown and the regeneration-pending phrase", () => {
    const s = freshnessSummary(status({ fresh_count: 1, stale_count: 1, revision_stale_count: 1 }));
    expect(s.state).toBe("STALE");
    expect(s.tone).toBe("signal");
    expect(s.detail).toContain("1 stale");
    expect(s.detail).toContain("regeneration pending");
  });
});
