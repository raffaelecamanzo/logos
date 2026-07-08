import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { ConfigReadModel, WikiNav, WikiStatus } from "../../api/types.ts";
import { ThemeProvider } from "../../theme/ThemeProvider.tsx";

// Mermaid is mocked so jsdom never loads the vendored UMD bundle.
vi.mock("./mermaid.ts", () => ({
  renderMermaidIn: vi.fn(() => Promise.resolve()),
  VENDORED_MERMAID_URL: "/assets/vendor/mermaid.min.js",
}));

// The wiki-generation client is mocked so the trigger needs no intent token / socket
// and so the test drives a hand-built SSE stream — the SSE contract is unchanged.
vi.mock("../../api/wikiGenClient.ts", () => ({
  WIKI_GENERATE_ROUTE: "/wiki/generate",
  fetchWikiConfig: vi.fn(),
  streamWikiGeneration: vi.fn(),
}));

import { fetchWikiConfig, streamWikiGeneration } from "../../api/wikiGenClient.ts";
import { WikiView } from "./WikiView.tsx";

const NAV: WikiNav = {
  tiers: [{ title: "Summary", items: [{ slug: "overview/project-overview", label: "Project Overview" }] }],
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

/** A config read-model with the given effective-model + key presence. */
function config(opts: {
  chatModel?: string | null;
  wikiModel?: string | null;
  provider?: "openai" | "anthropic";
  baseUrl?: string;
  keyPresent?: boolean;
}): ConfigReadModel {
  return {
    config: {
      path: ".logos/config.toml",
      exists: true,
      content: "",
      parsed: {
        languages: [],
        include: [],
        exclude: [],
        max_file_size: 1048576,
        framework_hints: [],
        chat: {
          provider: opts.provider ?? "openai",
          model: opts.chatModel ?? null,
          base_url: opts.baseUrl ?? "https://openrouter.ai/api/v1",
        },
        wiki: { model: opts.wikiModel ?? null },
      },
    },
    rules: { path: ".logos/rules.toml", exists: false, content: "", parsed: { constraints: {}, metric_thresholds: {} } },
    chat_key: { present: opts.keyPresent ?? true, last4: opts.keyPresent === false ? null : "9f3a" },
    defaults: DEFAULTS_FIXTURE,
  };
}

/** A minimal but shape-complete `defaults` projection (CR-067/BR-37) — this
 *  suite doesn't exercise default-rendering, so the values only need to match
 *  the wire shape, not the real server-computed numbers. */
const DEFAULTS_FIXTURE: ConfigReadModel["defaults"] = {
  config: {
    languages: [],
    include: ["**"],
    exclude: [],
    max_file_size: 2097152,
    framework_hints: [],
    chat: { provider: "openai", model: null, base_url: "https://openrouter.ai/api/v1" },
    wiki: {},
  },
  rules: {
    metric_thresholds: {
      nesting_depth: 4,
      brain_complexity: 15,
      brain_lines: 100,
      brain_nesting: 3,
      god_methods: 20,
      god_span: 500,
      clone_similarity: 0.85,
      clone_min_tokens: 50,
    },
    constraints: {},
  },
};

/** A one-shot SSE ReadableStream body from raw event text. */
function sseBody(text: string): ReadableStream<Uint8Array> {
  const bytes = new TextEncoder().encode(text);
  return new ReadableStream({
    start(controller) {
      controller.enqueue(bytes);
      controller.close();
    },
  });
}

/** Route the read-model GETs (nav/status/page) by URL fragment. */
function stubReadRoutes(routes: Record<string, unknown>) {
  const entries = Object.entries(routes).sort((a, b) => b[0].length - a[0].length);
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      const url = typeof input === "string" ? input : String((input as Request).url ?? input);
      for (const [frag, body] of entries) {
        if (url.includes(frag)) {
          return Promise.resolve({ ok: true, json: () => Promise.resolve(body) } as Response);
        }
      }
      return Promise.resolve({ ok: false, status: 404 } as Response);
    }),
  );
}

function go(path: string) {
  window.history.pushState({}, "", path);
}

function renderWiki() {
  return render(
    <ThemeProvider>
      <WikiView />
    </ThemeProvider>,
  );
}

beforeEach(() => {
  window.localStorage.clear();
  vi.mocked(fetchWikiConfig).mockReset();
  vi.mocked(streamWikiGeneration).mockReset();
  stubReadRoutes({ "/api/v1/wiki/nav": NAV, "/api/v1/wiki": FRESH_STATUS });
  go("/wiki");
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  window.history.pushState({}, "", "/");
});

describe("Wiki-tab generation trigger (S-178, FR-WK-18, FR-UI-19, NFR-SE-07)", () => {
  it("shows the configure-first state and starts no run when no provider is set", async () => {
    vi.mocked(fetchWikiConfig).mockResolvedValue(config({ chatModel: null, wikiModel: null }));
    renderWiki();

    expect(await screen.findByText(/Wiki generation needs an LLM provider/i)).toBeInTheDocument();
    // The wiki content still renders — configure-first is not an error, pages stay browsable.
    expect(await screen.findByRole("navigation", { name: "Wiki navigation" })).toBeInTheDocument();
    // No outbound run is triggered without a provider.
    expect(vi.mocked(streamWikiGeneration)).not.toHaveBeenCalled();
  });

  it("gates the first run behind the consent disclosure naming the endpoint (NFR-SE-07)", async () => {
    vi.mocked(fetchWikiConfig).mockResolvedValue(
      config({ provider: "anthropic", chatModel: "claude-x", keyPresent: true }),
    );
    // streamWikiGeneration should not fire until consent; make it hang if called early.
    vi.mocked(streamWikiGeneration).mockResolvedValue({ ok: true, body: sseBody("") } as Response);
    renderWiki();

    const banner = await screen.findByText(/Opening the Wiki refreshes stale pages/i);
    expect(banner).toBeInTheDocument();
    // The disclosure names the configured endpoint host (NFR-SE-07) — in the
    // sentence and the provider line.
    expect(screen.getAllByText(/api\.anthropic\.com/).length).toBeGreaterThan(0);
    // No outbound call before consent.
    expect(vi.mocked(streamWikiGeneration)).not.toHaveBeenCalled();

    // Accepting consent triggers exactly one run.
    fireEvent.click(screen.getByRole("button", { name: /Allow generation/i }));
    await waitFor(() => expect(vi.mocked(streamWikiGeneration)).toHaveBeenCalledTimes(1));
  });

  it("with consent remembered, streams per-page refreshes and reloads the read-models", async () => {
    window.localStorage.setItem("logos.wiki.consent", "1");
    vi.mocked(fetchWikiConfig).mockResolvedValue(config({ chatModel: "gpt-x", keyPresent: true }));
    const stream = sseBody(
      'event: started\ndata: {"event":"started","total":1}\n\n' +
        'event: page-started\ndata: {"event":"page-started","slug":"overview/project-overview","title":"Project Overview","index":1,"total":1}\n\n' +
        'event: page-written\ndata: {"event":"page-written","slug":"overview/project-overview","anchor_count":0,"replaced":true}\n\n' +
        'event: completed\ndata: {"event":"completed","pages_written":1,"pages_failed":0}\n\n',
    );
    vi.mocked(streamWikiGeneration).mockResolvedValue({ ok: true, body: stream } as Response);

    renderWiki();

    // Existing pages render immediately (the menu is up before generation completes).
    expect(await screen.findByRole("navigation", { name: "Wiki navigation" })).toBeInTheDocument();
    // Exactly one background run started.
    await waitFor(() => expect(vi.mocked(streamWikiGeneration)).toHaveBeenCalledTimes(1));
    // The run streams to completion and the banner reports it honestly.
    expect(await screen.findByText(/Generation complete/i)).toBeInTheDocument();

    // The page write bumped the refresh key, so the nav read-model reloaded (the
    // "refreshes stream in" behavior): fetch saw more than one /wiki/nav read.
    const navReads = vi
      .mocked(fetch)
      .mock.calls.filter(([input]) => String(input).includes("/api/v1/wiki/nav")).length;
    expect(navReads).toBeGreaterThan(1);

    // The single-fire guard held across the read-model reloads that re-rendered the
    // view — a re-render must not re-POST the trigger.
    expect(vi.mocked(streamWikiGeneration)).toHaveBeenCalledTimes(1);
  });

  it("surfaces an honest error banner when the trigger cannot start", async () => {
    window.localStorage.setItem("logos.wiki.consent", "1");
    vi.mocked(fetchWikiConfig).mockResolvedValue(config({ chatModel: "gpt-x", keyPresent: true }));
    vi.mocked(streamWikiGeneration).mockResolvedValue({ ok: false, status: 503, body: null } as Response);

    renderWiki();
    expect(await screen.findByText(/could not start \(status 503\)/i)).toBeInTheDocument();
    // The wiki content still renders — an error banner is additive, not a takeover.
    expect(screen.getByRole("navigation", { name: "Wiki navigation" })).toBeInTheDocument();
  });

  it("reports failed pages and an honest halt reason in the banner", async () => {
    window.localStorage.setItem("logos.wiki.consent", "1");
    vi.mocked(fetchWikiConfig).mockResolvedValue(config({ chatModel: "gpt-x", keyPresent: true }));
    vi.mocked(streamWikiGeneration).mockResolvedValue({
      ok: true,
      body: sseBody(
        'event: started\ndata: {"event":"started","total":1}\n\n' +
          'event: page-failed\ndata: {"event":"page-failed","slug":"overview/x","error":"over-cap body"}\n\n' +
          'event: halted\ndata: {"event":"halted","reason":"the per-run budget was spent"}\n\n' +
          'event: completed\ndata: {"event":"completed","pages_written":0,"pages_failed":1}\n\n',
      ),
    } as Response);

    renderWiki();
    expect(await screen.findByText(/1 failed/i)).toBeInTheDocument();
    expect(screen.getByText(/halted: the per-run budget was spent/i)).toBeInTheDocument();
  });

  it("aborts the in-flight generation fetch when the Wiki tab unmounts (FR-UI-19)", async () => {
    window.localStorage.setItem("logos.wiki.consent", "1");
    vi.mocked(fetchWikiConfig).mockResolvedValue(config({ chatModel: "gpt-x", keyPresent: true }));
    // A never-closing stream keeps the run in flight until unmount.
    vi.mocked(streamWikiGeneration).mockImplementation(
      (signal?: AbortSignal) =>
        new Promise<Response>(() => {
          void signal; // the pending fetch resolves only via abort
        }),
    );

    const { unmount } = renderWiki();
    await waitFor(() => expect(vi.mocked(streamWikiGeneration)).toHaveBeenCalledTimes(1));
    const signal = vi.mocked(streamWikiGeneration).mock.calls[0][0] as AbortSignal;
    expect(signal.aborted).toBe(false);

    unmount();
    expect(signal.aborted).toBe(true);
  });

  it("makes the per-page synthesis liveness timeout visible while a page is generating (CR-059, S-239, FR-UI-24)", async () => {
    window.localStorage.setItem("logos.wiki.consent", "1");
    vi.mocked(fetchWikiConfig).mockResolvedValue(config({ chatModel: "gpt-x", keyPresent: true }));
    vi.mocked(streamWikiGeneration).mockResolvedValue({
      ok: true,
      body: sseBody(
        'event: started\ndata: {"event":"started","total":1,"synthesis_timeout_secs":180}\n\n' +
          'event: page-started\ndata: {"event":"page-started","slug":"overview/x","title":"X","index":1,"total":1}\n\n',
      ),
    } as Response);

    renderWiki();
    // The 180s per-page timeout is a liveness guard, not a hang — it must be
    // visible while a page is actively synthesizing, not just after an eventual
    // halt names it.
    expect(await screen.findByText(/up to 180s to synthesize/i)).toBeInTheDocument();
    expect(screen.getByText(/a liveness guard, not a hang/i)).toBeInTheDocument();
  });

  it("hides the synthesis timeout hint once a page finishes writing, between pages (S-239)", async () => {
    window.localStorage.setItem("logos.wiki.consent", "1");
    vi.mocked(fetchWikiConfig).mockResolvedValue(config({ chatModel: "gpt-x", keyPresent: true }));
    vi.mocked(streamWikiGeneration).mockResolvedValue({
      ok: true,
      body: sseBody(
        'event: started\ndata: {"event":"started","total":2,"synthesis_timeout_secs":180}\n\n' +
          'event: page-started\ndata: {"event":"page-started","slug":"overview/x","title":"X","index":1,"total":2}\n\n' +
          'event: page-written\ndata: {"event":"page-written","slug":"overview/x","anchor_count":0,"replaced":true}\n\n',
      ),
    } as Response);

    renderWiki();
    // Between pages (no page currently in flight), the timeout hint must not
    // linger — it would otherwise read as an open-ended stall rather than the
    // liveness guard it names.
    expect(await screen.findByText(/1\/2 page\(s\) refreshed/i)).toBeInTheDocument();
    expect(screen.queryByText(/up to 180s to synthesize/i)).not.toBeInTheDocument();
  });

  it("never renders 'Generating…' once a halt is known, even before the terminal completed frame arrives (regression, S-239)", async () => {
    window.localStorage.setItem("logos.wiki.consent", "1");
    vi.mocked(fetchWikiConfig).mockResolvedValue(config({ chatModel: "gpt-x", keyPresent: true }));
    vi.mocked(streamWikiGeneration).mockResolvedValue({
      ok: true,
      body: sseBody(
        'event: started\ndata: {"event":"started","total":2,"synthesis_timeout_secs":180}\n\n' +
          'event: page-started\ndata: {"event":"page-started","slug":"overview/x","title":"X","index":1,"total":2}\n\n' +
          'event: halted\ndata: {"event":"halted","reason":"hard safety ceiling reached"}\n\n',
        // Deliberately no `completed` frame: the wiki-agent always emits one right
        // after `halted`, but each SSE frame is folded into state independently
        // (`wikiRuntime.tsx`'s `readSseStream` callback), so `phase` can still read
        // "running" for a beat after `halted` is already known. This fixture pins
        // that beat so the banner's behavior during it is asserted, not assumed.
      ),
    } as Response);

    renderWiki();
    expect(await screen.findByText(/Generation halted/i)).toBeInTheDocument();
    expect(screen.queryByText(/Generating…/i)).not.toBeInTheDocument();
    expect(screen.queryByText(/writing overview\/x/i)).not.toBeInTheDocument();
    expect(screen.queryByText(/liveness guard, not a hang/i)).not.toBeInTheDocument();
  });

  it("names a halted run honestly instead of reading as a clean completion (FR-UI-24, S-239)", async () => {
    window.localStorage.setItem("logos.wiki.consent", "1");
    vi.mocked(fetchWikiConfig).mockResolvedValue(config({ chatModel: "gpt-x", keyPresent: true }));
    vi.mocked(streamWikiGeneration).mockResolvedValue({
      ok: true,
      body: sseBody(
        'event: started\ndata: {"event":"started","total":2,"synthesis_timeout_secs":180}\n\n' +
          'event: halted\ndata: {"event":"halted","reason":"hard safety ceiling reached"}\n\n' +
          'event: completed\ndata: {"event":"completed","pages_written":0,"pages_failed":0}\n\n',
      ),
    } as Response);

    renderWiki();
    // Every run — halted or not — ends in the same terminal `completed` frame
    // (the wiki-agent always emits it). A halted run must never render the
    // unqualified "Generation complete" headline that a clean run gets.
    expect(await screen.findByText(/Generation halted/i)).toBeInTheDocument();
    expect(screen.queryByText(/Generation complete/i)).not.toBeInTheDocument();
    expect(screen.getByText(/halted: hard safety ceiling reached/i)).toBeInTheDocument();
  });

  it("surfaces an already-in-progress run honestly as a busy notice", async () => {
    window.localStorage.setItem("logos.wiki.consent", "1");
    vi.mocked(fetchWikiConfig).mockResolvedValue(config({ chatModel: "gpt-x", keyPresent: true }));
    vi.mocked(streamWikiGeneration).mockResolvedValue({
      ok: true,
      body: sseBody("event: busy\ndata: a wiki generation run is already in progress\n\n"),
    } as Response);

    renderWiki();
    expect(await screen.findByText(/already in progress/i)).toBeInTheDocument();
  });
});

describe("Wiki-tab re-attach to an in-flight run (S-223, FR-UI-19, FR-WK-18, NFR-CC-04)", () => {
  // The server-side re-attach contract (web::wikigen::WikiRunState::subscribe,
  // CR-056/S-222) replays a reopening trigger's SSE response with the run's history
  // first, then continues live — so from this hook's view, reopening the tab is just
  // a second `streamWikiGeneration` call whose stream already carries the cumulative
  // prefix. These tests simulate exactly that server contract to prove the SPA
  // renders the replayed+live sequence as one cumulative run, never a busy notice or
  // a reset "page 1 of N".

  it("reopening the tab mid-run shows cumulative progress, not a fresh run", async () => {
    window.localStorage.setItem("logos.wiki.consent", "1");
    vi.mocked(fetchWikiConfig).mockResolvedValue(config({ chatModel: "gpt-x", keyPresent: true }));

    // First open: the run starts and writes one of two pages before the tab closes
    // — the run itself (server-side) is still in flight.
    vi.mocked(streamWikiGeneration).mockResolvedValueOnce({
      ok: true,
      body: sseBody(
        'event: started\ndata: {"event":"started","total":2}\n\n' +
          'event: page-started\ndata: {"event":"page-started","slug":"overview/a","title":"A","index":1,"total":2}\n\n' +
          'event: page-written\ndata: {"event":"page-written","slug":"overview/a","anchor_count":0,"replaced":false}\n\n',
      ),
    } as Response);

    const first = renderWiki();
    expect(await screen.findByText(/1\/2 page\(s\) refreshed/i)).toBeInTheDocument();
    first.unmount();

    // Reopening the tab re-fires the trigger; the server re-attaches, replaying
    // `started` + the already-written page before continuing live to completion.
    vi.mocked(streamWikiGeneration).mockResolvedValueOnce({
      ok: true,
      body: sseBody(
        'event: started\ndata: {"event":"started","total":2}\n\n' +
          'event: page-written\ndata: {"event":"page-written","slug":"overview/a","anchor_count":0,"replaced":false}\n\n' +
          'event: page-started\ndata: {"event":"page-started","slug":"overview/b","title":"B","index":2,"total":2}\n\n' +
          'event: page-written\ndata: {"event":"page-written","slug":"overview/b","anchor_count":0,"replaced":false}\n\n' +
          'event: completed\ndata: {"event":"completed","pages_written":2,"pages_failed":0}\n\n',
      ),
    } as Response);

    renderWiki();
    await waitFor(() => expect(vi.mocked(streamWikiGeneration)).toHaveBeenCalledTimes(2));
    expect(await screen.findByText(/Generation complete/i)).toBeInTheDocument();
    // Cumulative across the reopen: 2 of 2 written, not reset to 1 of 2 or 0 of 2.
    expect(screen.getByText(/2\/2 page\(s\) refreshed/i)).toBeInTheDocument();
    expect(screen.queryByText(/already in progress/i)).not.toBeInTheDocument();
  });

  it("surfaces an in-flight run's honest halt on re-attach (NFR-CC-04)", async () => {
    window.localStorage.setItem("logos.wiki.consent", "1");
    vi.mocked(fetchWikiConfig).mockResolvedValue(config({ chatModel: "gpt-x", keyPresent: true }));

    vi.mocked(streamWikiGeneration).mockResolvedValueOnce({
      ok: true,
      body: sseBody('event: started\ndata: {"event":"started","total":2}\n\n'),
    } as Response);
    const first = renderWiki();
    await waitFor(() => expect(vi.mocked(streamWikiGeneration)).toHaveBeenCalledTimes(1));
    first.unmount();

    // Reopening re-attaches; the run then halts honestly (e.g. the hard safety
    // ceiling, [NFR-CC-04]) — the reattached observer sees the SAME halt, not a
    // silent success.
    vi.mocked(streamWikiGeneration).mockResolvedValueOnce({
      ok: true,
      body: sseBody(
        'event: started\ndata: {"event":"started","total":2}\n\n' +
          'event: halted\ndata: {"event":"halted","reason":"hard safety ceiling reached"}\n\n' +
          'event: completed\ndata: {"event":"completed","pages_written":0,"pages_failed":0}\n\n',
      ),
    } as Response);

    renderWiki();
    await waitFor(() => expect(vi.mocked(streamWikiGeneration)).toHaveBeenCalledTimes(2));
    expect(await screen.findByText(/halted: hard safety ceiling reached/i)).toBeInTheDocument();
  });
});
