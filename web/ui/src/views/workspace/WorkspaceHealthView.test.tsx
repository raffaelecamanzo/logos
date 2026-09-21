/*
 * Workspace Health (S-428, FR-UI-36, FR-WS-15, FR-WS-16, FR-WS-13, ADR-56).
 *
 * The view answers "is this workspace's picture of itself current, and which
 * members are not answering?", so the assertions here are about the roll-up being
 * stated in WORDS with its denominator, every unopenable member being named, the
 * governance findings being labelled advisory, and a workspace with no rules
 * getting the honest empty rather than a green tick.
 */

import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { WorkspaceProvider } from "../../workspace/WorkspaceContext.tsx";
import { setScopedMember } from "../../workspace/scope.ts";
import {
  degradedMember,
  governanceAnswer,
  governanceReport,
  memberStatus,
  oneDegraded,
  statusInfo,
  stubAppApi,
  warmRollup,
  workspaceStatus,
  type AppStubOptions,
} from "./appViewFixtures.ts";
import { WorkspaceHealthView } from "./WorkspaceHealthView.tsx";

/** The two reads this view issues, each with the card that read alone supplies.
 *  See the Dashboard spec's note: when the inner read fails, the outer read's
 *  cards are correctly still rendered, so the assertion is scoped to the card
 *  the FAILED read owns. */
const READS = [
  { path: "/api/v1/workspace/status", card: /^Members$/ },
  { path: "/api/v1/workspace/check", card: /^Workspace rules$/ },
] as const;

/** The view under test, inside the provider that establishes workspace mode. */
const tree = () => (
  <WorkspaceProvider>
    <WorkspaceHealthView />
  </WorkspaceProvider>
);

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  setScopedMember(null);
});

async function mount(opts: AppStubOptions = {}) {
  const calls = stubAppApi(opts);
  const utils = render(
    <WorkspaceProvider>
      <WorkspaceHealthView />
    </WorkspaceProvider>,
  );
  await screen.findByRole("heading", { name: /^Members$/ });
  return { ...utils, calls };
}

/** The `<section>` a design-system `Card` renders for `title`. */
function card(title: RegExp): HTMLElement {
  const section = screen.getByRole("heading", { name: title }).closest("section");
  expect(section).not.toBeNull();
  return section as HTMLElement;
}

/** A workspace with one member that could not be opened. */
const PARTIAL: AppStubOptions = {
  status: workspaceStatus({
    members: [memberStatus("api"), memberStatus("orders"), degradedMember("web")],
    warm_rollup: warmRollup({ warm: 2, degraded: 1 }),
    degraded_rollup: oneDegraded("web"),
  }),
  governance: governanceAnswer(governanceReport(), {
    complete: false,
    degraded_rollup: oneDegraded("web"),
  }),
};

describe("the degraded roll-up is the verdict, stated in words (AC3)", () => {
  it("says how many members answered, out of how many — never a bare count", async () => {
    await mount(PARTIAL);
    // The denominator travels with the figure: "2 answered" alone is unreadable
    // without the roster size (FR-WS-16, NFR-CC-04).
    expect(screen.getByText(/2 of 3 members answered/i)).toBeInTheDocument();
  });

  it("carries the verdict in TEXT, so colour is never the only signal (NFR-CC-04)", async () => {
    await mount(PARTIAL);
    const verdict = card(/^Members answering$/);
    // The badge itself reads as words. A tone class alone would leave the state
    // invisible to a screen reader and to a monochrome display.
    expect(within(verdict).getByText(/incomplete/i)).toBeInTheDocument();
  });

  it("reports an all-answered workspace as complete, with the same denominator", async () => {
    await mount();
    expect(screen.getByText(/3 of 3 members answered/i)).toBeInTheDocument();
    expect(within(card(/^Members answering$/)).getByText(/complete/i)).toBeInTheDocument();
  });
});

describe("every unopenable member is drawn degraded and NAMED (AC4)", () => {
  it("names it in the roll-up sentence", async () => {
    await mount(PARTIAL);
    expect(within(card(/^Members answering$/)).getByText(/web/)).toBeInTheDocument();
  });

  it("keeps its row in the member table, badged, with its reason", async () => {
    await mount(PARTIAL);
    const row = within(card(/^Members$/)).getByRole("row", { name: /web/ });
    // BOTH axes read degraded here, and they are separate facts: index presence
    // (warm) and store openability (open). A view that merged them would make an
    // un-indexed member indistinguishable from one nothing can open (FR-WS-15).
    expect(within(row).getByText(/degraded — indexing failed/i)).toBeInTheDocument();
    expect(within(row).getByText(/degraded — could not be opened/i)).toBeInTheDocument();
    // The classified cause's sentence, so an operator is sent to the right
    // remedy — a descriptor exhaustion is not fixed by re-indexing (FR-WS-16).
    expect(within(row).getByText(/file descriptors/i)).toBeInTheDocument();
    // And nothing on the row is fabricated: with no result there is neither a
    // freshness nor a reference-resolution figure to state, and both cells say
    // so rather than printing a zero (NFR-CC-04).
    expect(within(row).getAllByText(/^not read$/i)).toHaveLength(2);
    expect(within(row).queryByText("0.0%")).toBeNull();
  });

  it("marks the warm roll-up and the topic inventory as computed over fewer members", async () => {
    await mount(PARTIAL);
    // `covers_all_members: false` makes every other figure in the payload a lower
    // bound; a view that renders them unqualified states a whole-workspace fact
    // it does not have (NFR-CC-04).
    expect(screen.getAllByText(/lower bound|fewer than all/i).length).toBeGreaterThan(0);
  });
});

describe("per-member freshness and warm state (FR-WS-15, AC3)", () => {
  it("renders each member's own freshness line and warm state", async () => {
    await mount({
      status: workspaceStatus({
        members: [
          memberStatus("api", { result: statusInfo(), warm_state: "warm" }),
          memberStatus("orders", { result: statusInfo(), warm_state: "deferred" }),
          memberStatus("web", { result: statusInfo(), warm_state: "warm" }),
        ],
      }),
    });
    const orders = within(card(/^Members$/)).getByRole("row", { name: /orders/ });
    // `deferred` is honest and NON-alarming: the member indexes lazily on first
    // query, which is not a failure (FR-WS-15).
    expect(within(orders).getByText(/deferred/i)).toBeInTheDocument();
    expect(within(orders).getByText(/Indexed .* ago/)).toBeInTheDocument();
  });

  it("reports an absent `warming` count as not knowable — never as zero", async () => {
    // The server OMITS `warming` when no trustworthy live signal exists. Rendering
    // the absence as `0` would tell a reader nothing is being indexed right now,
    // which is a claim the payload does not make (NFR-CC-04).
    await mount();
    expect(screen.getByText(/not knowable/i)).toBeInTheDocument();
  });
});

describe("governance is advisory and its empty is honest (ADR-56, AC3)", () => {
  it("labels the findings advisory on the surface", async () => {
    await mount();
    expect(within(card(/^Workspace rules$/)).getByText(/advisory/i)).toBeInTheDocument();
  });

  it("renders a workspace declaring NO rules as nothing checked — not as a pass", async () => {
    await mount({ governance: governanceAnswer(null) });
    const rules = card(/^Workspace rules$/);
    expect(within(rules).getByText(/no workspace rules are declared/i)).toBeInTheDocument();
    // The untruth this AC exists to prevent: a green verdict over an unchecked
    // workspace (the same defect S-437 and S-438 removed from two other surfaces).
    expect(within(rules).queryByText(/^pass$/i)).toBeNull();
    expect(within(rules).queryByText(/no violations/i)).toBeNull();
  });

  it("states how many bindings a clean report was quantified over", async () => {
    // A clean report over ZERO bindings is "nothing was bound to check", not
    // "everything is fine" — the rider is what tells them apart (NFR-CC-04).
    await mount({
      governance: governanceAnswer(
        governanceReport({ violations: [], rules_checked: 2, bindings_checked: 0 }),
      ),
    });
    const rules = card(/^Workspace rules$/);
    expect(
      within(rules).getByText(/nothing was bound to check, so a clean result here says nothing/i),
    ).toBeInTheDocument();
  });

  it("names a rule referring to a member this workspace does not have", async () => {
    // Such a rule can never match, so it is silently narrowed — the false
    // all-clear this module refuses to produce (NFR-CC-04, ADR-53).
    await mount({
      governance: governanceAnswer(governanceReport({ unknown_member_refs: ["billing"] })),
    });
    expect(within(card(/^Workspace rules$/)).getByText(/billing/)).toBeInTheDocument();
  });
});

describe("no aggregate of per-member signals is rendered (BR-56, AC5)", () => {
  it("shows each member's own reference resolution and no mean of them", async () => {
    await mount({
      status: workspaceStatus({
        members: [
          memberStatus("api", {
            result: statusInfo({ resolution_coverage: 0.4, refs_resolved: 40, refs_total: 100 }),
          }),
          memberStatus("orders", {
            result: statusInfo({ resolution_coverage: 0.6, refs_resolved: 600, refs_total: 1000 }),
          }),
          memberStatus("web", {
            result: statusInfo({
              resolution_coverage: 0.8,
              refs_resolved: 8_000,
              refs_total: 10_000,
            }),
          }),
        ],
      }),
    });
    const members = card(/^Members$/);
    // Each member's own figure, with the denominator it was computed over — three
    // different denominators, so a bare percentage cannot satisfy this (CR-111).
    for (const own of [
      "40.0% (40 of 100 refs)",
      "60.0% (600 of 1,000 refs)",
      "80.0% (8,000 of 10,000 refs)",
    ]) {
      expect(within(members).getByText(own)).toBeInTheDocument();
    }
    // The mean of the three is 0.6, which `orders` legitimately owns — so the
    // roll-up is pinned by counting rather than by absence.
    expect(within(members).getAllByText(/60\.0%/)).toHaveLength(1);
  });

  it("renders no element announcing itself as a workspace-wide quality signal", async () => {
    await mount();
    // BR-56 is about rendering an AGGREGATE AS A SIGNAL, so the guard reads the
    // places a figure announces itself — card titles and column headers — and not
    // page prose. Matching all text instead fires on this view's own disclaimer,
    // which contains the words "quality signal" and "mean" precisely because it is
    // explaining that neither is computed here.
    //
    // The vocabulary is the SIBLING per-member Dashboard's own: its card is titled
    // "Quality index" and its empty state says "quality signal"
    // (`views/dashboard/DashboardView.tsx`). An earlier spelling matched only
    // /workspace (signal|score)/, which an aggregate added in that idiom would
    // have walked straight past (found in the S-428 review).
    const labels = [
      ...screen.getAllByRole("heading").map((h) => h.textContent ?? ""),
      ...screen.getAllByRole("columnheader").map((c) => c.textContent ?? ""),
    ];
    // The floor: an empty `labels` would make every assertion below vanish rather
    // than fail — a guard that switches itself off exactly when the surface it
    // guards disappears.
    expect(labels.length).toBeGreaterThan(5);
    for (const forbidden of [
      /quality (index|signal)/i,
      /workspace (signal|score)/i,
      /overall (signal|score|health)/i,
      /\baverage\b|\bmean\b|\btotal signal\b/i,
    ]) {
      expect(labels.filter((l) => forbidden.test(l))).toEqual([]);
    }
  });
});

describe("the topic inventory (FR-WS-11)", () => {
  it("lists each member's promoted topics, repo-qualified", async () => {
    await mount({
      status: workspaceStatus({
        topics: [{ member: "orders", topics: [{ topic: "orders.created", producers: 1, consumers: 2 }] }],
      }),
    });
    const topics = card(/^Promoted broker topics$/);
    expect(within(topics).getByText("orders.created")).toBeInTheDocument();
    expect(within(topics).getByText("orders")).toBeInTheDocument();
  });

  it("renders the honest empty when no member promoted a topic", async () => {
    await mount({ status: workspaceStatus({ topics: [] }) });
    expect(
      within(card(/^Promoted broker topics$/)).getByText(/no member has promoted/i),
    ).toBeInTheDocument();
  });
});

describe("the view is app-level and honest about its mode", () => {
  it("reads the two unscoped fan-outs and carries no ?repo= from the shell", async () => {
    setScopedMember("api");
    const { calls } = await mount();
    const reads = calls().filter((u) => !u.startsWith("/api/v1/workspace/roster"));
    expect(reads).toEqual(["/api/v1/workspace/status", "/api/v1/workspace/check"]);
  });

  it("states honestly that a single-root serve is not a workspace", async () => {
    stubAppApi({ probeStatus: 404 });
    render(
      <WorkspaceProvider>
        <WorkspaceHealthView />
      </WorkspaceProvider>,
    );
    await waitFor(() => expect(screen.getByText(/Not a workspace/)).toBeInTheDocument());
    expect(screen.queryByRole("heading", { name: /^Members$/ })).toBeNull();
  });
});

describe("a failed read is stated, never papered over (NFR-RA-05)", () => {
  /** Answer `path` with `code` and everything else normally — so the spec can
   *  fail ONE of the two reads and see what the view does with the other. */
  function failing(path: string, code = 500) {
    const calls = stubAppApi();
    const ok = globalThis.fetch as unknown as (u: string) => Promise<Response>;
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string) =>
        url.startsWith(path)
          ? Promise.resolve({
              ok: false,
              status: code,
              json: () => Promise.resolve({ error: "boom" }),
            } as Response)
          : ok(url),
      ),
    );
    return calls;
  }

  it("has a read to fail for every read this view issues", () => {
    // The floor: `it.each([])` registers zero tests silently, so an empty roster
    // would delete the coverage below rather than fail it.
    expect(READS.length).toBe(2);
  });

  it.each(READS)("states the failure when $path fails", async ({ path, card }) => {
    // Both views wire TWO resources into nested `AsyncResource` frames. The
    // generic hook has its own specs, but those prove the primitive works in
    // isolation — not that THIS view passed the right resource into the right
    // frame. A swap or a shared reference would leave one failure invisible,
    // which is the state this spec exists to rule out.
    failing(path);
    render(tree());
    const panel = await screen.findByText(
      new RegExp(`${path.replace("/api/v1/", "")}.*failed`, "i"),
    );
    expect(panel).toBeInTheDocument();
    // …and the card that read alone supplies is ABSENT rather than filled with
    // a fabricated figure (NFR-CC-04).
    expect(screen.queryByRole("heading", { name: card })).toBeNull();
  });

  it("states a failed mode probe as a failure, not as a plain repo", async () => {
    // The THIRD error source: the roster probe behind `useWorkspace()`. A 500
    // there must not be read as single-root — that would silently hide the
    // workspace UI (NFR-RA-05), which is why `probeWorkspace` rethrows.
    failing("/api/v1/workspace/roster");
    render(tree());
    await waitFor(() =>
      expect(screen.getByText(/could not be read/i)).toBeInTheDocument(),
    );
    expect(screen.queryByText(/Not a workspace/)).toBeNull();
  });
});
