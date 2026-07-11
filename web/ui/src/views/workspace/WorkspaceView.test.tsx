import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { BridgeEdge, CrossServiceCoverage } from "../../api/types.ts";
import { WorkspaceProvider } from "../../workspace/WorkspaceContext.tsx";
import { scopedMember, setScopedMember } from "../../workspace/scope.ts";
import { stubApi } from "../../workspace/testFixtures.ts";
import { WorkspaceView } from "./WorkspaceView.tsx";

// The service map mounts the real ECharts canvas, which needs a layout engine jsdom
// does not have. Stub it down to what the view actually contracts with: the node set
// it was handed, and the click that focuses a member.
vi.mock("../graph/GraphCanvas.tsx", () => ({
  GraphCanvas: ({
    loaded,
    onNodeClick,
  }: {
    loaded: { nodes: Record<string, { id: string; label: string }>; edges: unknown[] };
    onNodeClick: (id: string) => void;
  }) => (
    <div data-testid="canvas">
      <span data-testid="canvas-edges">{loaded.edges.length}</span>
      {Object.values(loaded.nodes).map((n) => (
        <button key={n.id} type="button" onClick={() => onNodeClick(n.id)}>
          {n.label}
        </button>
      ))}
    </div>
  ),
}));

/** 1 bound · 1 ambiguous · 1 unbound · 2 with no provider in this workspace. The
 *  server's ratio excludes the no-provider pair from its denominator: 1/3 = 33.3%. */
const COVERAGE: CrossServiceCoverage = {
  references: [
    { relation: "route", from: { member: "api", symbol: "a" }, bucket: "bound", state: "bound" },
    {
      relation: "route",
      from: { member: "api", symbol: "b" },
      bucket: "unbound",
      state: "unbound",
      reason: "path-not-composed",
    },
    {
      relation: "route",
      from: { member: "api", symbol: "c" },
      bucket: "unbound",
      state: "unbound",
      reason: "no-provider-in-workspace",
    },
    {
      relation: "route",
      from: { member: "api", symbol: "d" },
      bucket: "unbound",
      state: "unbound",
      reason: "no-provider-in-workspace",
    },
    {
      relation: "grpc-call",
      from: { member: "api", symbol: "e" },
      bucket: "ambiguous",
      state: "unbound",
      reason: "ambiguous",
    },
  ],
  bound: 1,
  ambiguous: 1,
  unbound: 1,
  no_provider_in_workspace: 2,
  bound_ratio: 0.3333,
};

const BINDING: BridgeEdge = {
  relation: "route",
  from: { member: "api", symbol: "op" },
  to: { member: "web", symbol: "route" },
};

function mount() {
  return render(
    <WorkspaceProvider>
      <WorkspaceView />
    </WorkspaceProvider>,
  );
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  setScopedMember(null);
});

describe("WorkspaceView (S-250, FR-UI-29)", () => {
  it("does NOT claim 'not a workspace' while the probe is still in flight", async () => {
    // The guard must distinguish "single-root" from "not known yet". Asserting a mode
    // we have not established would flash a falsehood at every real workspace on the
    // way in (NFR-CC-04) — and would make the single-root test below tautological.
    vi.stubGlobal("fetch", vi.fn(() => new Promise<Response>(() => {})));
    mount();
    expect(screen.queryByText(/not a workspace/i)).toBeNull();
    expect(await screen.findByText(/reading the workspace/i)).toBeInTheDocument();
  });

  it("says so honestly once single-root mode is SETTLED", async () => {
    stubApi({ probeStatus: 404 });
    mount();
    expect(await screen.findByText(/not a workspace/i)).toBeInTheDocument();
  });

  it("rolls the workspace up: its name, its services, and the coverage headline", async () => {
    stubApi({ coverage: COVERAGE, providers: [BINDING] });
    mount();
    expect(await screen.findByText("shop")).toBeInTheDocument();
    const rollup = screen.getByText(/2 services/);
    expect(rollup.textContent).toMatch(/1 bound · 1 ambiguous · 1 unbound/);
  });

  it("draws services as nodes and resolved bindings as edges on the shared canvas", async () => {
    stubApi({ providers: [BINDING] });
    mount();
    await waitFor(() => expect(screen.getByTestId("canvas")).toBeInTheDocument());
    expect(screen.getByTestId("canvas-edges")).toHaveTextContent("1");
    expect(screen.getByRole("cell", { name: "api" })).toBeInTheDocument();
    expect(screen.getByText(/HTTP \(OpenAPI ↔ route\)/)).toBeInTheDocument();
  });

  it("clicking a service focuses its member — the shell selector follows the canvas", async () => {
    stubApi({ providers: [BINDING] });
    mount();
    await waitFor(() => expect(screen.getByTestId("canvas")).toBeInTheDocument());
    await userEvent.click(screen.getByRole("button", { name: "web" }));
    expect(scopedMember()).toBe("web");
  });

  it("states an empty service map honestly rather than drawing a fabricated edge", async () => {
    stubApi({ providers: [] });
    mount();
    expect(await screen.findByText(/no cross-service bindings resolved yet/i)).toBeInTheDocument();
    expect(screen.getByTestId("canvas-edges")).toHaveTextContent("0");
  });

  // ── S-256 / FR-WS-11: topics on the map ────────────────────────────────────

  it("draws a broker topic as its own node on the service map", async () => {
    stubApi({
      providers: [],
      topics: [
        { member: "api", topics: [{ topic: "orders", producers: 1, consumers: 0 }] },
        { member: "web", topics: [{ topic: "orders", producers: 0, consumers: 1 }] },
      ],
    });
    mount();
    await waitFor(() => expect(screen.getByTestId("canvas")).toBeInTheDocument());

    // The topic is a node on the canvas alongside the services…
    expect(screen.getByRole("button", { name: "orders" })).toBeInTheDocument();
    // …and the coupling is drawn as two hops (api → orders → web), not one flat line.
    expect(screen.getByTestId("canvas-edges")).toHaveTextContent("2");
    expect(screen.getByText(/1 topic ·/)).toBeInTheDocument();
  });

  it("clicking a TOPIC selects no member — it is not a service", async () => {
    // The two id namespaces are disjoint precisely so this cannot happen: were a topic
    // id to decode as a service id, clicking `orders` would scope the whole shell to a
    // member named "orders" that does not exist.
    stubApi({
      providers: [],
      topics: [{ member: "api", topics: [{ topic: "orders", producers: 1, consumers: 0 }] }],
    });
    mount();
    await waitFor(() => expect(screen.getByTestId("canvas")).toBeInTheDocument());

    // The shell already opens on the manifest default, so the invariant is that a topic
    // click leaves the selection UNCHANGED — and above all never scopes to "orders".
    const before = scopedMember();
    await userEvent.click(screen.getByRole("button", { name: "orders" }));
    expect(scopedMember()).toBe(before);
    expect(scopedMember()).not.toBe("orders");
    // …while clicking a real service still focuses it.
    await userEvent.click(screen.getByRole("button", { name: "web" }));
    expect(scopedMember()).toBe("web");
  });

  it("ACCEPTANCE: a published-but-unconsumed topic is drawn, not reported as empty", async () => {
    // No binding exists (nobody subscribes), so the pre-S-256 map would have shown the
    // "no cross-service bindings" empty state and nothing else. The topic is real — a
    // per-repo topic is visible before any cross-repo match (FR-WS-11).
    stubApi({
      providers: [],
      topics: [{ member: "api", topics: [{ topic: "orders", producers: 1, consumers: 0 }] }],
    });
    mount();
    await waitFor(() => expect(screen.getByTestId("canvas")).toBeInTheDocument());

    expect(screen.getByRole("button", { name: "orders" })).toBeInTheDocument();
    expect(screen.queryByText(/no cross-service bindings resolved yet/i)).toBeNull();
  });

  it("shows the per-arm board whose columns RECONCILE with the headline above them", async () => {
    stubApi({ coverage: COVERAGE, providers: [BINDING] });
    mount();
    await userEvent.click(await screen.findByRole("tab", { name: /cross-service coverage/i }));

    // The ratio is the server's (33.3%), not bound/total (1/5 = 20%): the two
    // no-provider references are outside the denominator (ADR-53).
    expect(screen.getAllByText("33.3%").length).toBeGreaterThan(0);
    expect(screen.getByText(/2 with no provider in this workspace/)).toBeInTheDocument();

    // The `route` row: 1 bound, 0 ambiguous, 1 unbound, 2 no-provider. The wire `bucket`
    // says "unbound" for the no-provider pair, but the summary's `unbound` counter
    // excludes them — so folding them in would print "3 unbound" inches below a headline
    // that says "1 unbound". Pin the split.
    const routeRow = screen.getByRole("cell", { name: /HTTP \(OpenAPI ↔ route\)/ }).closest("tr")!;
    const cells = [...routeRow.querySelectorAll("td")].map((c) => c.textContent);
    expect(cells.slice(1, 5)).toEqual(["1", "0", "1", "2"]);
    expect(screen.getByText(/Path could not be composed/)).toBeInTheDocument();
  });

  it("renders the coverage empty state — never a fabricated 100% over nothing", async () => {
    stubApi();
    mount();
    await userEvent.click(await screen.findByRole("tab", { name: /cross-service coverage/i }));
    expect(screen.getByText(/no cross-boundary references found/i)).toBeInTheDocument();
  });
});

describe("WorkspaceView — cross-service impact (S-250, FR-UI-29)", () => {
  /** An impact answer: one healthy seed member, one degraded, no cross-service reach. */
  const IMPACT_DEGRADED = {
    query: "get_user",
    seed: [
      {
        member: "api",
        result: {
          query: "get_user",
          resolved: { symbol: "s", name: "get_user", kind: "function", file: "a.rs", line: 1 },
          depth: 2,
          upstream_label: "Callers",
          upstream: [
            { symbol: "c1", name: "handler", kind: "function", file: "h.rs", line: 9, distance: 1 },
          ],
          downstream_label: "Calls",
          downstream: [],
          docs_label: "Docs",
          docs: [],
          suggestions: [],
          warnings: [],
        },
      },
      { member: "web", error: "engine failed to start" },
    ],
    cross_service: [],
  };

  async function traceSymbol() {
    await userEvent.click(await screen.findByRole("tab", { name: /cross-service impact/i }));
    await userEvent.type(screen.getByLabelText(/symbol/i), "get_user");
    await userEvent.click(screen.getByRole("button", { name: /trace impact/i }));
  }

  it("invites a symbol before it fetches anything", async () => {
    stubApi();
    mount();
    await userEvent.click(await screen.findByRole("tab", { name: /cross-service impact/i }));
    expect(screen.getByText(/name a symbol to trace its impact/i)).toBeInTheDocument();
  });

  it("states a DEGRADED seed member rather than rendering it as zero impact", async () => {
    stubApi({ impact: IMPACT_DEGRADED });
    mount();
    await traceSymbol();
    // A member that could not be read has UNKNOWN impact, not none (NFR-RA-05).
    expect(await screen.findByText(/Degraded: engine failed to start/)).toBeInTheDocument();
    // The healthy member's impact is still shown.
    expect(screen.getByRole("cell", { name: "handler" })).toBeInTheDocument();
  });

  it("states the honest empty when no binding reaches the symbol from another service", async () => {
    stubApi({ impact: IMPACT_DEGRADED });
    mount();
    await traceSymbol();
    expect(await screen.findByText(/no cross-service impact/i)).toBeInTheDocument();
  });

  it("names the binding each far-side impact was stitched across", async () => {
    stubApi({
      impact: {
        ...IMPACT_DEGRADED,
        cross_service: [
          {
            via: BINDING,
            member: "web",
            impact: {
              query: "get_user",
              resolved: { symbol: "w", name: "get_user", kind: "route", file: "m.rs", line: 3 },
              depth: 2,
              upstream_label: "Callers",
              upstream: [
                { symbol: "w1", name: "route_handler", kind: "function", file: "m.rs", line: 4, distance: 1 },
              ],
              downstream_label: "Calls",
              downstream: [],
              docs_label: "Docs",
              docs: [],
              suggestions: [],
              warnings: [],
            },
          },
        ],
      },
    });
    mount();
    await traceSymbol();
    // The card heading names the far-side member AND the arm it was reached over (the
    // table's caption repeats it, so query the heading specifically).
    expect(
      await screen.findByRole("heading", {
        name: /web — reached across a HTTP \(OpenAPI ↔ route\) binding/,
      }),
    ).toBeInTheDocument();
    expect(screen.getByRole("cell", { name: "route_handler" })).toBeInTheDocument();
  });
});
