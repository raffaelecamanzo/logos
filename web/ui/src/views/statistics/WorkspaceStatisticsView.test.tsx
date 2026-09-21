/*
 * The `app`-scoped Statistics view (S-429 T2, CR-137, FR-UI-37, NFR-CC-04).
 *
 * Driven over a FIXTURE payload, never over the fan-out: T1's server-side
 * criteria (the engine-free read, the connection budget, the three unread
 * classifications) are proven in `logos-core` and `web` without a view, and these
 * specs prove the rendering duties without a workspace. The two halves of the
 * story are separately falsifiable, which is what the sprint's Testing note asks
 * for.
 *
 * So the assertions here are about: every total carrying its member denominator,
 * every unread member being named with its reason and the three reasons reading
 * apart, the awaiting-data state keying on the MEMBER-SCOPED predicate rather than
 * on `members_read`, the deliberate absences (no latency percentiles, no
 * artifact bindings, no quality signal), and the app-scoped read carrying no
 * member.
 */

import { act, cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import type {
  StatsInfo,
  UnreadMember,
  WorkspaceRoster,
  WorkspaceStatistics,
} from "../../api/types.ts";
import { WorkspaceProvider, useWorkspace } from "../../workspace/WorkspaceContext.tsx";
import { scopedMember, setScopedMember } from "../../workspace/scope.ts";

// Mock the ECharts seam: jsdom has no real canvas. The fake instance records the
// options it is given, so a spec can prove a window re-query re-rendered the
// surfaces rather than only the copy around them.
const setOption = vi.fn();
vi.mock("./echarts.ts", () => ({
  createStatChart: () => ({
    setOption: (...args: unknown[]) => setOption(...args),
    resize: vi.fn(),
    dispose: vi.fn(),
  }),
}));

import { isStatsEmpty, topTools, type UsageProjections } from "./statsModel.ts";
import { WorkspaceStatisticsView } from "./WorkspaceStatisticsView.tsx";

/** A three-member roster, local rather than shared, and both existing alternatives
 *  were considered.
 *
 *  `workspace/testFixtures.ts`'s roster has TWO members, and every assertion below
 *  is about a denominator: "2 of 3" is the only shape that tells a stated
 *  denominator apart from a count, because with two members "1 of 2" and a bare "1"
 *  and a hard-coded "2" are indistinguishable in the rendered text.
 *
 *  `views/workspace/appViewFixtures.ts` DOES export a value-identical three-member
 *  roster, and it is declined for a different reason: that module is documented as
 *  serving the two S-428 workspace views, and importing `views/workspace/` machinery
 *  into `views/statistics/` would invert the directory dependency this view was
 *  deliberately placed to avoid. Five lines of constants with no divergence
 *  consequence is the cheaper side of that trade. */
const ROSTER: WorkspaceRoster = {
  workspace: "shop",
  default: "api",
  members: ["api", "orders", "web"],
};

/**
 * A populated aggregate over a fully-read three-member workspace.
 *
 * `context` calls are `100 + windowDays` (107 vs 130) — a unique per-window figure,
 * so a spec can prove a data-table twin (not just the callout copy) refreshed when
 * the window changed.
 */
function aggregate(windowDays: number, over: Partial<WorkspaceStatistics> = {}): WorkspaceStatistics {
  return {
    workspace: "shop",
    window_days: windowDays,
    members_total: 3,
    members_read: 3,
    covers_all_members: true,
    unread: [],
    calls_total: 42,
    calls_by_tool: [
      { surface: "cli", tool: "context", calls: 100 + windowDays, ok_calls: 50 },
      { surface: "mcp", tool: "search", calls: 20, ok_calls: 19 },
    ],
    activity_by_day: [
      { day: "2026-09-19", calls: 20, ok_calls: 20 },
      { day: "2026-09-20", calls: 22, ok_calls: 21 },
    ],
    calls_by_origin: [
      { origin: "main", calls: 30, ok_calls: 29 },
      { origin: "dev", calls: 12, ok_calls: 12 },
    ],
    reads_saved_estimate: 88,
    tokens_saved_estimate: 12_345,
    ...over,
  };
}

/** One unread member of each kind — the shape that pins "named apart". */
const ABSENT: UnreadMember = {
  member: "orders",
  reason: "absent",
  detail: "no telemetry recorded yet (telemetry.db not found)",
};
const LOCKED: UnreadMember = {
  member: "web",
  reason: "locked",
  detail: "database is locked",
};
const UNREADABLE: UnreadMember = {
  member: "billing",
  reason: "unreadable",
  detail: "opening telemetry store read-only at /w/billing/.logos/telemetry.db: file is not a database",
};

/** The partial aggregate: two of three members read, one named unreadable. This is
 *  the shape the denominator duty is actually about. */
function partial(windowDays: number): WorkspaceStatistics {
  return aggregate(windowDays, {
    members_read: 2,
    covers_all_members: false,
    unread: [UNREADABLE],
  });
}

/** Nothing recorded anywhere, and every member's store absent — the awaiting-data
 *  case FR-UI-37 names, in the shape it actually arrives in. */
function nothingRecorded(windowDays: number): WorkspaceStatistics {
  return aggregate(windowDays, {
    members_read: 0,
    covers_all_members: false,
    unread: [
      { member: "api", reason: "absent", detail: ABSENT.detail },
      { member: "orders", reason: "absent", detail: ABSENT.detail },
      { member: "web", reason: "absent", detail: ABSENT.detail },
    ],
    calls_total: 0,
    calls_by_tool: [],
    activity_by_day: [],
    calls_by_origin: [],
    reads_saved_estimate: 0,
    tokens_saved_estimate: 0,
  });
}

/** Every member read, none of them with an event in the window. The BOUNDARY
 *  between the two candidate empty predicates: `members_read === members_total`
 *  here, so a view keying on `members_read` would render a grid of zeros. */
function readButEventless(windowDays: number): WorkspaceStatistics {
  return aggregate(windowDays, {
    members_read: 3,
    covers_all_members: true,
    unread: [],
    calls_total: 0,
    calls_by_tool: [],
    activity_by_day: [],
    calls_by_origin: [],
    reads_saved_estimate: 0,
    tokens_saved_estimate: 0,
  });
}

/** A populated aggregate whose only unread member is ABSENT. The distinction the
 *  view has to draw: `covers_all_members` is false, but nothing failed to be read
 *  and nothing was recorded by that member, so the figures are EXACT — calling them
 *  a lower bound here is a false caveat, and this is the dominant state of a young
 *  or large workspace. */
function absentOnly(windowDays: number): WorkspaceStatistics {
  return aggregate(windowDays, {
    members_read: 2,
    covers_all_members: false,
    unread: [ABSENT],
  });
}

/** Nothing summed AND a member that could not be READ. `calls_total` is the
 *  server's sum over the members it could read, so the zero says nothing whatever
 *  about the locked and unreadable ones — the state in which "no member recorded any
 *  telemetry" is a measurement this view never took. */
function emptyWithFailedRead(windowDays: number): WorkspaceStatistics {
  return aggregate(windowDays, {
    members_read: 1,
    covers_all_members: false,
    unread: [LOCKED, UNREADABLE],
    calls_total: 0,
    calls_by_tool: [],
    activity_by_day: [],
    calls_by_origin: [],
    reads_saved_estimate: 0,
    tokens_saved_estimate: 0,
  });
}

/** The member-scoped wire shape carrying the SAME four projections as
 *  `aggregate(7)`, for the cross-scope consistency assertion at the bottom. */
function memberStats(): StatsInfo {
  const agg = aggregate(7);
  return {
    window_days: agg.window_days,
    calls_total: agg.calls_total,
    calls_by_tool: agg.calls_by_tool,
    latency_p50_ms: 3,
    latency_p95_ms: 11,
    latency_p99_ms: 30,
    reads_saved_estimate: agg.reads_saved_estimate,
    tokens_saved_estimate: agg.tokens_saved_estimate,
    artifact_bindings: {},
    activity_by_day: agg.activity_by_day,
    calls_by_origin: agg.calls_by_origin,
    warnings: [],
  };
}

/** Stub `fetch` over the surface this view drives, echoing the requested
 *  `?window=` into the payload, and record every URL called. */
function stubApi(
  build: (windowDays: number) => WorkspaceStatistics,
  { probeStatus = 200 }: { probeStatus?: number } = {},
): () => string[] {
  const calls: string[] = [];
  const json = (body: unknown, ok = true, code = 200) =>
    Promise.resolve({ ok, status: code, json: () => Promise.resolve(body) } as Response);
  vi.stubGlobal(
    "fetch",
    vi.fn((url: string) => {
      calls.push(url);
      if (url.startsWith("/api/v1/workspace/roster")) {
        return json(ROSTER, probeStatus === 200, probeStatus);
      }
      if (url.startsWith("/api/v1/workspace/statistics")) {
        const days = Number(new URL(url, "http://x").searchParams.get("window") ?? 7);
        return json(build(days));
      }
      return json({});
    }),
  );
  return () => calls;
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  setOption.mockClear();
  setScopedMember(null);
  switcher.current = null;
});

/** Mount the view inside the provider that establishes workspace mode, and wait for
 *  the aggregate to land. */
async function mount(
  build: (windowDays: number) => WorkspaceStatistics = aggregate,
  opts: { probeStatus?: number } = {},
) {
  const calls = stubApi(build, opts);
  const utils = render(
    <WorkspaceProvider>
      <CaptureSwitch />
      <WorkspaceStatisticsView />
    </WorkspaceProvider>,
  );
  if ((opts.probeStatus ?? 200) === 200) {
    await screen.findByRole("heading", { name: /^Statistics$/ });
    await screen.findByText(/this view answers for the whole workspace/i);
  }
  return { ...utils, calls };
}

/** A handle on the shell's member switch, captured from inside the provider — the
 *  selector itself lives in the sidebar, which is not in this view's tree. */
const switcher: { current: ((name: string) => void) | null } = { current: null };
function CaptureSwitch() {
  switcher.current = useWorkspace().selectMember;
  return null;
}

/** The `<section>` a design-system `Card` renders for `title`. */
function card(title: RegExp): HTMLElement {
  const section = screen.getByRole("heading", { name: title }).closest("section");
  expect(section).not.toBeNull();
  return section as HTMLElement;
}

// ── AC1: the four projections, summed, with the denominator ────────────────────

describe("the aggregate's four projections are rendered (AC1)", () => {
  it("renders the value estimate, labeled an estimate rather than a measurement", async () => {
    await mount();
    expect(screen.getByText("12,345")).toBeInTheDocument();
    expect(screen.getByText("88")).toBeInTheDocument();
    // The word carries the honesty (NFR-CC-04); a figure alone reads as measured.
    // Matched on the sentence rather than on "an estimate", which the `<em>` splits
    // across two nodes — and on the disclaimer, so a view that drops the word but
    // keeps the styling still fails.
    expect(screen.getByText(/Not a measured figure/i)).toBeInTheDocument();
    expect(screen.getByText("estimate")).toBeInTheDocument();
  });

  it("renders usage over time as a chart AND its accessible table twin", async () => {
    await mount();
    const activity = card(/^Usage over time$/);
    expect(within(activity).getByRole("img")).toBeInTheDocument();
    // The twin is the accessible surface: a canvas is never the only channel.
    expect(within(activity).getByRole("row", { name: /2026-09-20/ })).toBeInTheDocument();
  });

  it("renders top tools ranked, with the same twin duty", async () => {
    await mount();
    const tools = card(/^Top tools$/);
    expect(within(tools).getByRole("img")).toBeInTheDocument();
    const row = within(tools).getByRole("row", { name: /context/ });
    // 107 = 100 + the default 7-day window: the aggregate's own figure, not a
    // re-derived one.
    expect(within(row).getByText("107")).toBeInTheDocument();
  });

  it("renders the dev-vs-main split, and says it can sum to less than total calls", async () => {
    await mount();
    const origin = card(/^Dev vs main$/);
    expect(within(origin).getByRole("row", { name: /^dev/ })).toBeInTheDocument();
    expect(within(origin).getByRole("row", { name: /^main/ })).toBeInTheDocument();
    expect(
      within(origin).getByText(/rolled-up days carry no origin/i),
    ).toBeInTheDocument();
  });
});

describe("every total states the member denominator it is a sum over (AC1, NFR-CC-04)", () => {
  it("states it inside EVERY figure surface on the page, not once at the top", async () => {
    const { container } = await mount();
    // Derived from the RENDERED tree, not from a list of card titles written here.
    // A hardcoded roster of three cards (or a bare count of five notes) passes
    // unchanged the day a fourth figure surface is added without a note — which is
    // exactly how this project has previously shipped a selector list that a later
    // story extended past. So: walk every card, and require the note in each one
    // that carries a figure.
    const cards = [...container.querySelectorAll("section")].filter((el) =>
      el.querySelector("h3"),
    );
    // The denominator of the walk itself: a selector that matched nothing would make
    // every assertion below vacuously true.
    expect(cards.length).toBeGreaterThanOrEqual(3);
    for (const el of cards) {
      const title = el.querySelector("h3")?.textContent ?? "";
      // The unread table is the one card that carries no total — it carries the
      // names. Its own header badge states the same population.
      if (/Members not summed/i.test(title)) {
        expect(el.textContent).toMatch(/of\s*3\s*contributed nothing/i);
        continue;
      }
      expect(within(el as HTMLElement).getByText(/Summed over/i)).toBeInTheDocument();
    }
    // …and the two callouts above the cards carry it too: the population statement
    // and the value estimate, which is itself a total.
    const notes = screen.getAllByText(/Summed over/i);
    expect(notes.length).toBe(cards.length + 2);
  });

  it("pluralises the denominator against the roster size, not against a guess", async () => {
    // The near miss for the one-member workspace: "1 of 1 workspace members" reads
    // as a rounding of something bigger.
    await mount((d) =>
      aggregate(d, { members_total: 1, members_read: 1, covers_all_members: true }),
    );
    for (const note of screen.getAllByText(/Summed over/i)) {
      expect(note.textContent).toMatch(/1\s*of\s*1\s*workspace member\b/i);
      expect(note.textContent).not.toMatch(/workspace members/i);
    }
  });

  it("carries the numerator AND the denominator, never a bare count", async () => {
    await mount();
    for (const note of screen.getAllByText(/Summed over/i)) {
      // "3 of 3", inside the sentence. A rendered "3" with no "of 3" beside it is
      // exactly the unattributed total this criterion refuses.
      expect(note.textContent).toMatch(/3\s*of\s*3\s*workspace members/i);
    }
  });

  it("says a partial aggregate is a LOWER BOUND, and how many members are missing", async () => {
    await mount(partial);
    for (const note of screen.getAllByText(/Summed over/i)) {
      expect(note.textContent).toMatch(/2\s*of\s*3\s*workspace members/i);
      expect(note.textContent).toMatch(/lower bound/i);
    }
    // Every note now names the count itself, and the callout adds the consequence.
    // Asserted per-note rather than as an occurrence count, which would just pin the
    // number of surfaces again.
    for (const note of screen.getAllByText(/Summed over/i)) {
      expect(note.textContent).toMatch(/1 member could not be read/i);
    }
    expect(
      screen.getByText(/every figure on this page is lower than the workspace's true usage/i),
    ).toBeInTheDocument();
  });

  it("does not call a complete aggregate a lower bound", async () => {
    await mount();
    expect(screen.queryByText(/lower bound/i)).toBeNull();
    for (const note of screen.getAllByText(/Summed over/i)) {
      expect(note.textContent).toMatch(/the whole roster/i);
    }
  });
});

describe("a read FAILURE and an absent store are never conflated (AC1/AC2, NFR-CC-04)", () => {
  // Server-side, `absent` is `Ok(None)` — a SUCCESSFUL determination that the member
  // has no telemetry store. `locked`/`unreadable` are read failures. The two demand
  // opposite statements, and `covers_all_members` is false for both, so it cannot
  // word either. On the reference estate nearly every member is absent, which is why
  // getting this wrong would put a false caveat on the page permanently.

  it("does NOT call the figures a lower bound when the only unread member is absent", async () => {
    await mount(absentOnly);
    for (const note of screen.getAllByText(/Summed over/i)) {
      expect(note.textContent).toMatch(/2\s*of\s*3\s*workspace members/i);
      // Nothing was recorded by that member, so nothing is missing from the sums.
      expect(note.textContent).not.toMatch(/lower bound/i);
      expect(note.textContent).toMatch(/no recorded usage is missing/i);
    }
    // …and the page does not say a figure is understated, because none is, and does
    // not assert that a read failed, because none did.
    expect(screen.queryByText(/lower than the workspace's true usage/i)).toBeNull();
    expect(screen.queryByText(/member could not be read/i)).toBeNull();
    expect(screen.queryByText(/usage in this window is unknown/i)).toBeNull();
    // Including the table's own caption, which used to assert a read failure about
    // every row in it — the same conflation one level down.
    const caption = card(/^Members not summed$/).querySelector("caption");
    expect(caption?.textContent).toMatch(/contributed nothing to the figures above/i);
    expect(caption?.textContent).not.toMatch(/could not be read/i);
  });

  it("says an absent store is the ordinary state of a new service, not a fault", async () => {
    await mount(absentOnly);
    // The phrase is in the callout AND in the table row's reason sentence — both are
    // meant to be there, so assert presence in both rather than uniqueness.
    expect(screen.getAllByText(/no telemetry store yet/i).length).toBeGreaterThanOrEqual(2);
    expect(
      screen.getByText(/ordinary state of a new service rather than a fault/i),
    ).toBeInTheDocument();
    expect(
      within(card(/^Members not summed$/)).getByText(/Not a fault/i),
    ).toBeInTheDocument();
  });

  it("DOES call the figures a lower bound when a read actually failed", async () => {
    // The positive half. Without it the spec above passes over a view that never
    // says "lower bound" at all.
    await mount(partial);
    for (const note of screen.getAllByText(/Summed over/i)) {
      expect(note.textContent).toMatch(/lower bound/i);
      expect(note.textContent).toMatch(/usage in this window is unknown/i);
    }
  });

  it("treats an UNRECOGNISED reason token as a read failure, not as an absent store", async () => {
    // The safe direction, and the near miss: a reason arm the server grows against a
    // shipped bundle must produce a caveat that may be unnecessary rather than
    // suppress one that is needed. Keyed on exact inequality to "absent", never on a
    // substring — note this token CONTAINS neither "absent" nor any known arm.
    await mount((d) =>
      aggregate(d, {
        members_read: 2,
        covers_all_members: false,
        unread: [{ member: "billing", reason: "quarantined" as UnreadMember["reason"], detail: "d" }],
      }),
    );
    for (const note of screen.getAllByText(/Summed over/i)) {
      expect(note.textContent).toMatch(/lower bound/i);
    }
    expect(screen.queryByText(/no recorded usage is missing/i)).toBeNull();
  });

  it("words a single unread member in the singular throughout", async () => {
    await mount(partial);
    // One member, one row. "the members named below" / "They are named below" for a
    // single row is the deviation the sibling view does not make.
    expect(screen.getByText(/It is named below, with its reason/i)).toBeInTheDocument();
    expect(screen.queryByText(/They are named below/i)).toBeNull();
    for (const note of screen.getAllByText(/Summed over/i)) {
      expect(note.textContent).toMatch(/1 member could not be read, so its usage/i);
      expect(note.textContent).not.toMatch(/their usage/i);
    }
  });
});

// ── AC2: every unread member named, with its reason, apart ─────────────────────

describe("every unread member is NAMED with its reason (AC2, NFR-RA-05)", () => {
  it("gives it a row carrying its name, its reason token and the reason in words", async () => {
    await mount(partial);
    const unread = card(/^Members not summed$/);
    const row = within(unread).getByRole("row", { name: /billing/ });
    expect(within(row).getByText("unreadable")).toBeInTheDocument();
    expect(within(row).getByText(/corrupt, permission-denied, or not a database/i)).toBeInTheDocument();
    // The server's own diagnostic, verbatim — not a wording this view invents.
    expect(within(row).getByText(/file is not a database/)).toBeInTheDocument();
  });

  it("names every one of them, not a count of them", async () => {
    await mount((d) =>
      aggregate(d, {
        members_total: 4,
        members_read: 1,
        covers_all_members: false,
        unread: [ABSENT, LOCKED, UNREADABLE],
      }),
    );
    const unread = card(/^Members not summed$/);
    for (const member of ["orders", "web", "billing"]) {
      expect(within(unread).getByRole("row", { name: new RegExp(member) })).toBeInTheDocument();
    }
  });

  it("renders the three reasons APART — an absent store is not reported as a fault", async () => {
    await mount((d) =>
      aggregate(d, {
        members_total: 4,
        members_read: 1,
        covers_all_members: false,
        unread: [ABSENT, LOCKED, UNREADABLE],
      }),
    );
    const unread = card(/^Members not summed$/);
    const words = (member: string) =>
      within(unread).getByRole("row", { name: new RegExp(member) }).textContent ?? "";
    // Three different sentences, each saying what to do about it. Collapsing them to
    // "unavailable" would report the first as an incident and the last as routine.
    expect(words("orders")).toMatch(/nobody has run Logos in this member\. Not a fault/i);
    expect(words("web")).toMatch(/transient; try again/i);
    expect(words("billing")).toMatch(/corrupt, permission-denied/i);
    // And no two of them read the same.
    const sentences = ["orders", "web", "billing"].map(words);
    expect(new Set(sentences).size).toBe(3);
  });

  it("degrades honestly on a reason token it does not know (the near miss)", async () => {
    // The three arms are a CLOSED union in `types.ts`, so this cannot be reached
    // through the type system — it is reached by the server growing a fourth arm
    // against a shipped bundle. The lookup must then render the raw token, never
    // the string "undefined", and must not ink an unknown state as a fault.
    await mount((d) =>
      aggregate(d, {
        members_read: 2,
        covers_all_members: false,
        unread: [
          { member: "billing", reason: "quarantined" as UnreadMember["reason"], detail: "why" },
        ],
      }),
    );
    const row = within(card(/^Members not summed$/)).getByRole("row", { name: /billing/ });
    // NAMED: the token itself reaches the reader, rather than being swallowed.
    expect(row.textContent).toContain("quarantined");
    // NOT INKED AS A FAULT on the strength of being unrecognised — the tone falls
    // back to muted. Asserted on the badge's class because that is where a tone
    // lives; the sweep proved that dropping the fallback leaves it with no tone at
    // all, which this catches.
    const badge = row.querySelector("span[class*=badge]");
    expect(badge?.className).toMatch(/muted/);
    expect(badge?.className).not.toMatch(/\bred|\borange/);
    // The server's own diagnostic still reaches the reader — it is the only sentence
    // about this row, so losing it would leave a bare token.
    expect(row.textContent).toContain("why");
    // And no invented sentence, and no leaked `undefined`.
    expect(row.textContent).not.toMatch(/undefined/);
    expect(row.textContent).not.toMatch(/unavailable/i);
  });

  it("renders no unread card at all when every member was read", async () => {
    await mount();
    expect(screen.queryByRole("heading", { name: /^Members not summed$/ })).toBeNull();
  });
});

// ── AC4: the awaiting-data state, on the member-scoped predicate ───────────────

describe("a workspace with no telemetry awaits data rather than reporting zeros (AC4)", () => {
  it("renders the awaiting-data state and NO zeroed figure surface", async () => {
    await mount(nothingRecorded);
    expect(screen.getByText(/No member recorded any telemetry in this window/i)).toBeInTheDocument();
    // Not one of the four figure surfaces is drawn: a zero here reads as a measured
    // "nobody uses Logos", which an empty store does not say (NFR-CC-04).
    for (const surface of [/^Usage over time$/, /^Top tools$/, /^Dev vs main$/]) {
      expect(screen.queryByRole("heading", { name: surface })).toBeNull();
    }
    expect(screen.queryByText(/tokens and/i)).toBeNull();
  });

  it("STILL states the denominator and STILL names every unread member", async () => {
    await mount(nothingRecorded);
    // The empty state replaces the figures, not the two statements the view owes
    // unconditionally. A workspace where nothing has telemetry is precisely the case
    // where every member is `absent`, so hiding the names behind the empty state
    // would drop AC2 exactly where it is most informative.
    // Exactly one: the population callout's. The four figure surfaces are gated
    // away, so a second one here would mean a figure surface leaked through.
    const notes = screen.getAllByText(/Summed over/i);
    expect(notes).toHaveLength(1);
    expect(notes[0].textContent).toMatch(/0\s*of\s*3\s*workspace members/i);
    const unread = card(/^Members not summed$/);
    for (const member of ["api", "orders", "web"]) {
      expect(within(unread).getByRole("row", { name: new RegExp(member) })).toBeInTheDocument();
    }
  });

  it("awaits data when every member IS read but none recorded anything", async () => {
    // THE PREDICATE BOUNDARY. `members_read === members_total === 3` here, so a view
    // keying its empty state on `members_read === 0` renders a grid of zeros. The
    // predicate is the member-scoped view's own `calls_total === 0`.
    const model = readButEventless(7);
    expect(model.members_read).toBe(model.members_total);
    expect(isStatsEmpty(model)).toBe(true);

    await mount(readButEventless);
    expect(screen.getByText(/No member recorded any telemetry in this window/i)).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: /^Top tools$/ })).toBeNull();
    // …and it is not reported as a partial read: every member answered.
    expect(screen.queryByText(/lower bound/i)).toBeNull();
    expect(screen.queryByRole("heading", { name: /^Members not summed$/ })).toBeNull();
  });

  it("does NOT claim nothing was recorded when a member could not be READ", async () => {
    // `calls_total === 0` is the server's sum over the members it COULD read, so with
    // a locked or unreadable member in the roster a zero is not evidence that nothing
    // was recorded anywhere. Claiming it would be a measurement this view never took,
    // and `logos stats` would be the wrong remedy — the blocker is the store.
    await mount(emptyWithFailedRead);
    expect(screen.queryByText(/No member recorded any telemetry/i)).toBeNull();
    expect(screen.queryByText("logos stats")).toBeNull();
    // What it says instead: scoped to what was read, and honest about the rest.
    expect(
      screen.getByText(/None of the 1 member whose telemetry could be read recorded anything/i),
    ).toBeInTheDocument();
    expect(screen.getByText(/2 members could not be read at all/i)).toBeInTheDocument();
    expect(screen.getByText(/knows nothing about their usage/i)).toBeInTheDocument();
    // And the callout above does not claim a figure is understated: there is no
    // figure on the page to understate.
    expect(screen.queryByText(/every figure on this page is lower/i)).toBeNull();
    // …and the members are still named.
    const unread = card(/^Members not summed$/);
    for (const member of ["web", "billing"]) {
      expect(within(unread).getByRole("row", { name: new RegExp(member) })).toBeInTheDocument();
    }
  });

  it("DOES claim it when every unread member is merely absent", async () => {
    // The positive half: the universal claim is earned when no read failed, because
    // every member then either recorded nothing or has no store at all.
    await mount(nothingRecorded);
    expect(screen.getByText(/No member recorded any telemetry in this window/i)).toBeInTheDocument();
    expect(screen.getByText("logos stats")).toBeInTheDocument();
  });

  it("does NOT await data on a populated aggregate", async () => {
    // The negative half: without it the two specs above pass over a view that is
    // permanently empty.
    await mount();
    expect(screen.queryByText(/No member recorded any telemetry/i)).toBeNull();
    expect(screen.getByRole("heading", { name: /^Top tools$/ })).toBeInTheDocument();
  });
});

// ── AC5/AC6: the deliberate absences ──────────────────────────────────────────

describe("the view carries no quality signal and no per-member shape (AC6, BR-56)", () => {
  it("renders no quality-signal element anywhere", async () => {
    const { container } = await mount();
    // It aggregates usage, not quality: the per-repository signal is defined against
    // one repository's baseline, so a workspace-wide roll-up of it has no referent.
    expect(screen.queryByText(/\bsignal\b/i)).toBeNull();
    expect(screen.queryByText(/\bbaseline\b/i)).toBeNull();
    expect(screen.queryByText(/\bgate\b/i)).toBeNull();
    expect(container.querySelector("progress")).toBeNull();
    expect(screen.queryByRole("progressbar")).toBeNull();
  });

  it("renders no latency percentile and no artifact-binding figure", async () => {
    await mount();
    // Their absence from the payload is deliberate — a summed p95 is a fabricated
    // figure, and artifact bindings are the live-graph property that makes
    // `Engine::stats` engine-bound. The view must not imply either exists.
    expect(screen.queryByText(/latency/i)).toBeNull();
    expect(screen.queryByText(/p50|p95|p99/i)).toBeNull();
    expect(screen.queryByText(/artifact binding/i)).toBeNull();
  });
});

// ── AC5: app-scoped — no member on the read, and a window that re-queries ─────

describe("the read is app-scoped (AC5, FR-UI-35)", () => {
  it("carries no member on the aggregate request, even with one selected", async () => {
    setScopedMember("orders");
    const { calls } = await mount();
    const reads = calls().filter((u) => u.startsWith("/api/v1/workspace/statistics"));
    expect(reads.length).toBeGreaterThan(0);
    for (const url of reads) {
      // Narrowing the fan-out to the selected member would answer a different
      // question from the one this view asks.
      expect(url).not.toContain("repo=");
    }
  });

  it("does NOT re-read the aggregate when the member changes", async () => {
    // The half of AC5 that `App.workspace.test.tsx` cannot reach: that spec drives a
    // STAND-IN app-scoped view, so it pins the shell's mount key and says nothing
    // about what THIS component fetches. The shell not remounting the view is only
    // half the property — a view that named the member in its own dependency array
    // would re-read anyway, inside the same mount.
    const { calls } = await mount();
    const reads = () => calls().filter((u) => u.startsWith("/api/v1/workspace/statistics"));
    expect(reads()).toHaveLength(1);

    await act(async () => {
      switcher.current?.("web");
    });

    // The switch really happened — the transport moved, so this is not a test in
    // which nothing changed.
    await waitFor(() => expect(scopedMember()).toBe("web"));
    expect(reads()).toHaveLength(1);
  });

  it("re-queries the aggregate — and re-renders every surface — on a window change", async () => {
    const { calls } = await mount();
    expect(screen.getByText("12,345")).toBeInTheDocument();
    const before = setOption.mock.calls.length;

    await userEvent.selectOptions(screen.getByLabelText(/Window/i), "30");

    await waitFor(() => {
      // The twin, not just the callout: 130 = 100 + 30.
      expect(within(card(/^Top tools$/)).getByText("130")).toBeInTheDocument();
    });
    expect(calls().some((u) => u.includes("window=30"))).toBe(true);
    expect(setOption.mock.calls.length).toBeGreaterThan(before);
  });

  it("offers the same windows the member-scoped view offers", async () => {
    await mount();
    const select = screen.getByLabelText(/Window/i) as HTMLSelectElement;
    expect([...select.options].map((o) => o.value)).toEqual(["7", "30", "90"]);
    expect(select.value).toBe("7");
  });
});

// ── AC5: unreachable and unrendered in single-root mode ───────────────────────

describe("single-root mode renders no workspace figure (AC5)", () => {
  it("states that this serve is not a workspace, and draws nothing else", async () => {
    await mount(aggregate, { probeStatus: 404 });
    await screen.findByText(/Not a workspace/i);
    // No figure, no denominator, no unread table — a hand-typed URL must land on an
    // honest statement rather than on a blank shell or a fabricated aggregate.
    expect(screen.queryByText(/Summed over/i)).toBeNull();
    expect(screen.queryByRole("heading", { name: /^Top tools$/ })).toBeNull();
    expect(screen.queryByRole("heading", { name: /^Members not summed$/ })).toBeNull();
  });
});

// ── NFR-RA-05: a failed read is a failed read ─────────────────────────────────

describe("a failed aggregate read is reported, never imputed (NFR-RA-05)", () => {
  it("renders the honest error panel instead of an empty or zeroed page", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string) => {
        if (url.startsWith("/api/v1/workspace/roster")) {
          return Promise.resolve({
            ok: true,
            status: 200,
            json: () => Promise.resolve(ROSTER),
          } as Response);
        }
        return Promise.resolve({
          ok: false,
          status: 500,
          json: () => Promise.resolve({}),
        } as Response);
      }),
    );
    render(
      <WorkspaceProvider>
        <WorkspaceStatisticsView />
      </WorkspaceProvider>,
    );
    await waitFor(() => {
      expect(screen.getByText(/failed \(HTTP 500\)/i)).toBeInTheDocument();
    });
    // Critically NOT the awaiting-data state: a broken read is not an empty store.
    expect(screen.queryByText(/No member recorded any telemetry/i)).toBeNull();
  });
});

// ── The guard this view owes because of what it SHARES ────────────────────────
//
// Note what is NOT here: a check that every `styles.X` key this view references is
// defined in the stylesheet it shares with the member-scoped view. It cannot live in
// Vitest — `vitest.config.ts` sets `css: false`, and for a `*.module.css` Vite's
// CSS-modules plugin wins over the `?raw` query, so the glob hands back the empty
// proxy object rather than the stylesheet text (measured, not assumed). That guard is
// in `web/tests/spa_design_system.rs` instead, which reads stylesheets off disk — the
// same reason that file already owns every other stylesheet contract in this project.

describe("the guard this view owes because it SHARES rather than copies", () => {
  it("answers identically to the member-scoped view over identical projections", () => {
    // The consistency BETWEEN two behaviours, which neither view's own specs can see.
    // Both assignments are plain (no cast), so `tsc -b` fails if either wire shape
    // drifts out of the contract `statsModel.ts` documents — that claim was prose
    // until now. The runtime half is the one that matters: the two scopes must not be
    // able to rank the same tools differently, which is exactly what a second copy of
    // the arithmetic would eventually do.
    const fromAggregate: UsageProjections = aggregate(7);
    const fromMember: UsageProjections = memberStats();
    expect(topTools(fromAggregate).rows).toEqual(topTools(fromMember).rows);
    expect(isStatsEmpty(fromAggregate)).toBe(isStatsEmpty(fromMember));
    // …and the shared predicate really is shared: the member-scoped view's sidebar
    // probe keys on this same function, so a divergence here would desynchronise the
    // nav muting from the tab's own empty state.
    expect(isStatsEmpty({ calls_total: 0 })).toBe(true);
    expect(isStatsEmpty({ calls_total: 1 })).toBe(false);
  });
});
