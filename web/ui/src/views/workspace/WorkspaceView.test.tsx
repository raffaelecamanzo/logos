import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
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
 *  server's ratio excludes the no-provider pair from its denominator: 1/3 = 33.3%.
 *
 *  Split by intake (S-377/CR-120): the declared endpoints are the bound one and
 *  the two orphans, the captured call sites are the tied gRPC stub call and the
 *  HTTP client call whose path did not compose. So `invocation.bound` is **0** —
 *  the reference workspace's own shape, at this fixture's scale. */
const COVERAGE: CrossServiceCoverage = {
  references: [
    {
      relation: "route",
      from: { member: "api", symbol: "a" },
      bucket: "bound",
      state: "bound",
      intake: "contract-surface",
      provenance: "literal" as const,
    },
    {
      relation: "route",
      from: { member: "api", symbol: "b" },
      bucket: "unbound",
      state: "unbound",
      reason: "path-not-composed",
      intake: "invocation",
      // S-382: one config-bound row, so the rendered "Target read from" column
      // is asserted against a real admitted value rather than only literals.
      provenance: "config-bound" as const,
      bound: [
        {
          key: "orders.base",
          source: "placeholder" as const,
          values: [
            { value: "/orders", profiles: ["docker"], unprofiled: false, sources: ["a.yml"] },
          ],
        },
      ],
    },
    {
      relation: "route",
      from: { member: "api", symbol: "c" },
      bucket: "unbound",
      state: "unbound",
      reason: "no-provider-in-workspace",
      intake: "contract-surface",
      provenance: "literal" as const,
    },
    {
      relation: "route",
      from: { member: "api", symbol: "d" },
      bucket: "unbound",
      state: "unbound",
      reason: "no-provider-in-workspace",
      intake: "contract-surface",
      provenance: "literal" as const,
    },
    {
      relation: "grpc-call",
      from: { member: "api", symbol: "e" },
      bucket: "ambiguous",
      state: "unbound",
      reason: "ambiguous",
      intake: "invocation",
      provenance: "literal" as const,
    },
  ],
  bound: 1,
  ambiguous: 1,
  unbound: 1,
  no_provider_in_workspace: 2,
  by_intake: {
    contract_surface: { bound: 1, ambiguous: 0, unbound: 0, no_provider_in_workspace: 2 },
    invocation: { bound: 0, ambiguous: 1, unbound: 1, no_provider_in_workspace: 0 },
  },
  spec_conformance_ratio: 0.3333,
  spec_conformance_measured: 3,
  spec_conformance_summary: "0.333 (1 of 3 measured; 2 excluded as no-provider-in-workspace)",
  // The CR-120 headline over the same fixture: the two `invocation` references
  // are the tied gRPC stub call and the uncomposed HTTP client call, so **no**
  // edge resolved from a captured call site — 0 of 2 egress sites — while
  // `bound: 1` above reads as a workspace that binds. That contrast is the whole
  // reason the headline moved, so the fixture carries it.
  resolved_cross_service_edges: 0,
  egress_resolution: 0,
  egress_resolution_measured: 2,
  resolved_edges_summary:
    "0 resolved cross-service edges; egress resolution 0.000 (0 of 2 egress sites resolved)",
  members_read: 2,
  members_total: 2,
  covers_all_members: true,
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

  // S-377/CR-120: the headline counts two populations as one, and the arm board
  // cannot separate them — `route` carries both, because an OpenAPI operation and
  // an HTTP client call are the same arm. So the split is its own board, and its
  // two rows must reconcile with the headline exactly as the arm rows do.
  it("shows the by-intake board, and its rows RECONCILE with the headline above them", async () => {
    stubApi({ coverage: COVERAGE, providers: [BINDING] });
    mount();
    await userEvent.click(await screen.findByRole("tab", { name: /cross-service coverage/i }));

    // contract-surface: 1 bound, 0 ambiguous, 0 unbound, 2 no-provider (3 refs).
    const declared = screen.getByRole("cell", { name: /contract-surface/ }).closest("tr")!;
    expect([...declared.querySelectorAll("td")].map((c) => c.textContent).slice(1, 6)).toEqual([
      "1",
      "0",
      "0",
      "2",
      "3",
    ]);
    // invocation: 0 bound, 1 ambiguous, 1 unbound, 0 no-provider (2 refs). The
    // ZERO is the point — this fixture is the reference workspace's shape.
    const captured = screen.getByRole("cell", { name: /invocation/ }).closest("tr")!;
    expect([...captured.querySelectorAll("td")].map((c) => c.textContent).slice(1, 6)).toEqual([
      "0",
      "1",
      "1",
      "0",
      "2",
    ]);
    // And the finding is said in words, not left to be read off two zeros.
    expect(screen.getByText(/No captured call site in this workspace resolves/)).toBeInTheDocument();
  });

  // The other zero. `invocation.bound === 0` with no invocation references at all
  // is honest absence, and saying "nothing resolves" over it would be a fabricated
  // finding — the mirror image of the fabrication NFR-CC-04 usually guards.
  // The mutation this closes: drop `captured.bound === 0` from the gate and the
  // view prints "nothing resolves" over a workspace whose call sites DO resolve —
  // a fabricated finding, and the whole suite stayed green without this fixture,
  // because every other case here has `invocation.bound === 0`.
  it("says NOTHING when the invocation population resolves — no fabricated finding", async () => {
    const healthy: CrossServiceCoverage = {
      ...COVERAGE,
      references: COVERAGE.references.map((ref) =>
        ref.intake === "invocation" ? { ...ref, bucket: "bound" as const, state: "bound" as const, reason: undefined } : ref,
      ),
      bound: 3,
      ambiguous: 0,
      unbound: 0,
      by_intake: {
        contract_surface: { bound: 1, ambiguous: 0, unbound: 0, no_provider_in_workspace: 2 },
        invocation: { bound: 2, ambiguous: 0, unbound: 0, no_provider_in_workspace: 0 },
      },
    };
    stubApi({ coverage: healthy, providers: [BINDING] });
    mount();
    await userEvent.click(await screen.findByRole("tab", { name: /cross-service coverage/i }));

    const captured = screen.getByRole("cell", { name: /invocation/ }).closest("tr")!;
    expect([...captured.querySelectorAll("td")].map((c) => c.textContent).slice(1, 6)).toEqual([
      "2",
      "0",
      "0",
      "0",
      "2",
    ]);
    expect(screen.queryByText(/No captured call site in this workspace resolves/)).toBeNull();
    expect(screen.queryByText(/points\s+at a service outside it/)).toBeNull();
    expect(screen.queryByText(/honest absence, not a resolution failure/)).toBeNull();
  });

  // ADR-53's separate bucket, at the view: a population made ENTIRELY of calls
  // that leave this workspace has not failed to resolve, and calling it a failure
  // is the mirror of the fabrication NFR-CC-04 usually guards. This is the state
  // the 84-member reference estate is in for its one HTTP client call, so it is
  // not a hypothetical shape.
  it("calls an all-outside-the-workspace invocation population what it is, not a failure", async () => {
    const outward: CrossServiceCoverage = {
      ...COVERAGE,
      references: COVERAGE.references.map((ref) =>
        ref.intake === "invocation"
          ? {
              ...ref,
              bucket: "unbound" as const,
              state: "unbound" as const,
              reason: "no-provider-in-workspace" as const,
            }
          : ref,
      ),
      bound: 1,
      ambiguous: 0,
      unbound: 0,
      no_provider_in_workspace: 4,
      by_intake: {
        contract_surface: { bound: 1, ambiguous: 0, unbound: 0, no_provider_in_workspace: 2 },
        invocation: { bound: 0, ambiguous: 0, unbound: 0, no_provider_in_workspace: 2 },
      },
    };
    stubApi({ coverage: outward, providers: [BINDING] });
    mount();
    await userEvent.click(await screen.findByRole("tab", { name: /cross-service coverage/i }));

    expect(screen.getByText(/points\s+at a service outside it/)).toBeInTheDocument();
    expect(screen.queryByText(/No captured call site in this workspace resolves/)).toBeNull();
    expect(screen.queryByText(/honest absence, not a resolution failure/)).toBeNull();
  });

  it("calls an absent invocation population absence, not a resolution failure", async () => {
    const declaredOnly: CrossServiceCoverage = {
      ...COVERAGE,
      references: COVERAGE.references.map((ref) => ({
        ...ref,
        intake: "contract-surface" as const,
        provenance: "literal" as const,
      })),
      by_intake: {
        contract_surface: { bound: 1, ambiguous: 1, unbound: 1, no_provider_in_workspace: 2 },
        invocation: { bound: 0, ambiguous: 0, unbound: 0, no_provider_in_workspace: 0 },
      },
    };
    stubApi({ coverage: declaredOnly, providers: [BINDING] });
    mount();
    await userEvent.click(await screen.findByRole("tab", { name: /cross-service coverage/i }));

    expect(screen.getByText(/honest absence, not a resolution failure/)).toBeInTheDocument();
    expect(screen.queryByText(/No captured call site in this workspace resolves/)).toBeNull();
  });

  it("renders the coverage empty state — never a fabricated 100% over nothing", async () => {
    stubApi();
    mount();
    await userEvent.click(await screen.findByRole("tab", { name: /cross-service coverage/i }));
    expect(screen.getByText(/no cross-boundary references found/i)).toBeInTheDocument();
  });

  // CR-118 §4.5: the provider-identity fields are OPTIONAL, and the view must
  // render rows both with and without them. `COVERAGE` above is the WITHOUT
  // shape (an older store, or a current one with no provider to name) and is
  // already exercised by every test in this block; this is the WITH shape,
  // asserted to render the identical board — the widened payload informs the
  // per-reference detail, it does not move a count on this screen.
  it("renders the identical board from rows carrying the CR-118 provider fields", async () => {
    const widened: CrossServiceCoverage = {
      ...COVERAGE,
      references: COVERAGE.references.map((ref) =>
        ref.bucket === "bound"
          ? { ...ref, to: { member: "web", symbol: "route_get" }, intake: "contract-surface" as const }
          : ref.bucket === "ambiguous"
            ? {
                ...ref,
                candidates: {
                  disposition: "tied-between" as const,
                  providers: [
                    { member: "mailbox-aggregator-api", symbol: "route_agg" },
                    { member: "funnel-aggregator-api", symbol: "route_funnel" },
                    { member: "mailbox-core", symbol: "route_core" },
                  ],
                  total: 3,
                  omitted: 0,
                  summary: "3 tied providers, all listed; none bound",
                },
              }
            : ref,
      ),
    };
    stubApi({ coverage: widened, providers: [BINDING] });
    mount();
    await userEvent.click(await screen.findByRole("tab", { name: /cross-service coverage/i }));

    expect(screen.getAllByText("33.3%").length).toBeGreaterThan(0);
    // The `route` arm carries `to`; the `grpc-call` arm is the one carrying
    // `candidates`. Assert BOTH — asserting only the first leaves the arm whose
    // rows gained the tied-candidate set covered by the headline alone.
    const routeRow = screen.getByRole("cell", { name: /HTTP \(OpenAPI ↔ route\)/ }).closest("tr")!;
    expect([...routeRow.querySelectorAll("td")].map((c) => c.textContent).slice(1, 5)).toEqual([
      "1",
      "0",
      "1",
      "2",
    ]);
    const grpcRow = screen.getByRole("cell", { name: /gRPC/ }).closest("tr")!;
    expect([...grpcRow.querySelectorAll("td")].map((c) => c.textContent).slice(1, 5)).toEqual([
      "0",
      "1",
      "0",
      "0",
    ]);
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

  // ── S-326 / FR-WS-05 / FR-WS-16 / NFR-CC-04 ───────────────────────────────

  /** The score bar inside ONE named card, by class.
   *
   *  Scoped by card since S-376: the coverage panel draws **two** bars now — the
   *  `egress_resolution` headline and the `spec_conformance_ratio` beneath it — so an
   *  unscoped `querySelector("meter")` silently returns whichever the layout puts
   *  first, and the muting assertion below would compare the wrong bar's tone. It
   *  did exactly that when the headline card was added, which is why this helper
   *  exists rather than an index. */
  function scoreBarClassIn(container: HTMLElement, cardTitle: RegExp): string | undefined {
    // `hidden: true` because the coverage board lives in a tabpanel that carries
    // `hidden` until its tab is selected, and Testing Library's role queries skip
    // hidden subtrees by default. The assertions here are about which card a bar
    // belongs to, not about tab state, so the panel is queried where it is.
    const heading = within(container).getByRole("heading", { name: cardTitle, hidden: true });
    return heading.closest("section")?.querySelector("meter")?.className ?? undefined;
  }

  it("renders an ABSENT spec-conformance ratio as 'not measured', with no bar", async () => {
    // The server omits `spec_conformance_ratio` when nothing was measured. A bar is a
    // quantity: a 0-width one reads "nothing bound" and a full one "everything
    // bound", and neither is true when there was nothing to bind (CR-100).
    const { spec_conformance_ratio: _omitted, ...noRatio } = COVERAGE;
    stubApi({
      coverage: {
        ...noRatio,
        references: COVERAGE.references.filter((r) => r.reason === "no-provider-in-workspace"),
        bound: 0,
        ambiguous: 0,
        unbound: 0,
        no_provider_in_workspace: 2,
        spec_conformance_measured: 0,
        spec_conformance_summary: "0 of 0 measured; 2 excluded as no-provider-in-workspace",
      } as typeof COVERAGE,
    });
    mount();

    expect(await screen.findByText(/spec conformance not measured/i)).toBeInTheDocument();
    expect(screen.queryByText(/0\.0% bound/)).toBeNull();
    expect(screen.queryByText(/100\.0% bound/)).toBeNull();
    // CR-111: the excluded count is STILL reported when the ratio itself is absent
    // (S-327) — the server's composed line renders regardless.
    expect(
      await screen.findByText("0 of 0 measured; 2 excluded as no-provider-in-workspace"),
    ).toBeInTheDocument();
  });

  // ── CR-111 / FR-WS-05: the ratio never travels without its scale ───────────

  it("presents the denominator and excluded count adjacent to the score bar", async () => {
    stubApi({ coverage: COVERAGE });
    mount();

    expect(
      await screen.findByText("0.333 (1 of 3 measured; 2 excluded as no-provider-in-workspace)"),
    ).toBeInTheDocument();
  });

  it("renders the score bar MUTED rather than a confident fill when the excluded bucket dominates the denominator", async () => {
    // The pec-services shape this CR fixes: 6 bound, 1 unbound (denominator 7),
    // 899 excluded — the excluded bucket vastly outweighs what was measured.
    const dominated = {
      ...COVERAGE,
      bound: 6,
      ambiguous: 0,
      unbound: 1,
      no_provider_in_workspace: 899,
      spec_conformance_ratio: 6 / 7,
      spec_conformance_measured: 7,
      spec_conformance_summary: "0.857 (6 of 7 measured; 899 excluded as no-provider-in-workspace)",
    };
    stubApi({ coverage: dominated });
    const { container } = mount();
    await screen.findByText("0.857 (6 of 7 measured; 899 excluded as no-provider-in-workspace)");
    const mutedClass = scoreBarClassIn(container, /^Spec conformance/);
    cleanup();

    // The complement: a healthy ratio (excluded well below the denominator) fills
    // with the confident `default` tone, a DIFFERENT class from the muted one above.
    stubApi({ coverage: COVERAGE });
    const { container: healthyContainer } = mount();
    await screen.findByText("0.333 (1 of 3 measured; 2 excluded as no-provider-in-workspace)");
    const defaultClass = scoreBarClassIn(healthyContainer, /^Spec conformance/);

    expect(mutedClass).toBeTruthy();
    expect(defaultClass).toBeTruthy();
    expect(mutedClass).not.toBe(defaultClass);
  });

  // ── S-376 / CR-120 / BR-51: the headline is a resolved-edge count ──────────

  /** **The resolved-edge count and the egress rate are rendered together**, and the
   *  retired bound-ratio is nowhere on the surface ([BR-51], AC3/AC4).
   *
   *  Driven through the real view over the real model, not against a projection of
   *  the fixture: the assertion is on rendered DOM text, which is what an operator
   *  reads. The fixture's shape is the reference estate's in miniature — `bound: 1`
   *  beside `0` resolved edges over 2 captured egress sites — so a surface that
   *  renders only the pooled count would look healthy here, which is the misreading
   *  the headline exists to prevent. */
  it("renders the resolved-edge headline with its egress rate beside it", async () => {
    stubApi({ coverage: COVERAGE });
    const { container } = mount();

    // The server's composed line, verbatim: the count and the rate in one string,
    // so the view cannot render one without the other.
    expect(
      await screen.findByText(
        "0 resolved cross-service edges; egress resolution 0.000 (0 of 2 egress sites resolved)",
      ),
    ).toBeInTheDocument();
    // …with its own bar, distinct from the spec-conformance one below it.
    expect(scoreBarClassIn(container, /^Resolved cross-service edges/)).toBeTruthy();
    expect(screen.getByText(/0\.0% of egress sites resolve/)).toBeInTheDocument();
    // And the retired vocabulary is absent from the whole rendered surface.
    expect(container.textContent).not.toMatch(/bound[ -]ratio/i);
  });

  /** An absent egress rate renders "not measured", never a bar — [CR-100]'s rule on
   *  the successor figure, and the state the honest-empty fixture is in. */
  it("renders an ABSENT egress resolution as 'not measured', with no bar", async () => {
    const { egress_resolution: _omitted, ...noRate } = COVERAGE;
    stubApi({
      coverage: {
        ...noRate,
        resolved_cross_service_edges: 0,
        egress_resolution_measured: 0,
        resolved_edges_summary:
          "0 resolved cross-service edges; egress resolution not measured (0 of 0 egress sites)",
      } as typeof COVERAGE,
    });
    const { container } = mount();

    expect(
      await screen.findByText(
        "0 resolved cross-service edges; egress resolution not measured (0 of 0 egress sites)",
      ),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/egress resolution not measured — no outbound call site was captured/),
    ).toBeInTheDocument();
    expect(screen.queryByText(/of egress sites resolve/)).toBeNull();
    expect(scoreBarClassIn(container, /^Resolved cross-service edges/)).toBeUndefined();
  });

  it("labels a partially-opened workspace as covering fewer than all members", async () => {
    stubApi({
      coverage: {
        ...COVERAGE,
        members_read: 9,
        members_total: 72,
        covers_all_members: false,
      },
    });
    mount();

    expect(
      await screen.findByText(/computed over 9 of 72 workspace members/i),
    ).toBeInTheDocument();
  });

  it("says nothing about partial coverage when every member was read", async () => {
    stubApi({ coverage: COVERAGE });
    mount();

    // The ratio card renders, so the assertion below is about absence of the
    // banner rather than about the card not having loaded yet.
    expect(await screen.findByText(/33\.3% bound/)).toBeInTheDocument();
    expect(screen.queryByText(/workspace members/i)).toBeNull();
  });

  /* **The CR-100 shape, and the regression that hid it.** 63 of 72 members
   * unopened and the 9 survivors holding no cross-boundary reference: the
   * coverage set is EMPTY *and* the workspace is partial. The empty branch used
   * to short-circuit before the shortfall rider, so the panel asserted "No
   * cross-boundary references found in this workspace" — a positive claim about
   * all 72 members made from 9, which moved the fabrication from
   * `spec_conformance_ratio: 1.0` to the empty state rather than removing it. */
  it("does not claim an empty coverage set describes the whole workspace when it is partial", async () => {
    stubApi({
      coverage: {
        ...COVERAGE,
        references: [],
        bound: 0,
        ambiguous: 0,
        unbound: 0,
        no_provider_in_workspace: 0,
        members_read: 9,
        members_total: 72,
        covers_all_members: false,
      },
    });
    mount();

    expect(
      await screen.findByText(/among the 9 of 72 workspace members that could be read/i),
    ).toBeInTheDocument();
    expect(
      screen.queryByText(/No cross-boundary references found in this workspace/i),
      "the unqualified whole-workspace claim must be gone",
    ).toBeNull();
    // And the shortfall rider renders in the empty branch too.
    expect(screen.getByText(/every figure here is a lower bound/i)).toBeInTheDocument();
  });

  it("states the shortfall without attributing a cause the marker cannot know", async () => {
    // `covers_all_members` is also false when a member opened FINE and its
    // contract-surface read failed, so the banner must not say "could not be
    // opened" — that would send an operator to `ulimit -n` for a read fault.
    stubApi({
      coverage: { ...COVERAGE, members_read: 1, members_total: 2, covers_all_members: false },
    });
    mount();

    const banner = await screen.findByText(/did not contribute/i);
    expect(banner).toHaveTextContent(/contract surface could not be read/i);
  });

  it("names the members that could not be opened, from the degraded roll-up", async () => {
    // The names come from `degraded_rollup` — the field that actually knows which
    // members failed to OPEN — not from the coverage marker.
    stubApi({
      coverage: COVERAGE,
      degradedRollup: {
        members: 3,
        opened: 1,
        not_attempted: 0,
        degraded_members: ["filters-api", "orders"],
        covers_all_members: false,
      },
    });
    mount();

    expect(await screen.findByText(/2 members could not be opened/i)).toBeInTheDocument();
    expect(screen.getByText(/filters-api, orders/)).toBeInTheDocument();
  });

  it("names where each arm's targets were read from, so an admitted value is not shown as an observed one", async () => {
    // ADR-64 states its boundary as a condition on the SURFACES: "an admitted
    // value must never be indistinguishable from an observed one". This asserts
    // the rendered column, not the model — a label function with no caller
    // satisfies the type checker and leaves a dashboard viewer unable to tell
    // the two apart.
    stubApi({ coverage: COVERAGE });
    mount();
    expect(await screen.findByText("Target read from")).toBeInTheDocument();
    expect(
      await screen.findByText(/Read from `orders.base` \(docker\)/),
    ).toBeInTheDocument();
    expect(await screen.findAllByText("Written at the call site")).not.toHaveLength(0);
  });
});
