import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, onTestFinished, vi } from "vitest";

import type {
  BridgeEdge,
  BuildDependencyHeadline,
  CrossServiceCoverage,
  XserviceBuildDeps,
} from "../../api/types.ts";
import {
  buildDependencies,
  COVERAGE_TEXT,
  coverageByIntake,
  declaredRelations,
  specConformance,
} from "../../copy/coverage.copy.ts";
import { expectWidgetCopy } from "../../copy/expectWidgetCopy.ts";
import { expectOneWidgetStack, widgetTitle } from "../../test/widgetStack.ts";
import {
  HEALTHY_COVERAGE,
  MULTI_REASON_COVERAGE,
  oneDegraded,
  PARTIAL_COVERAGE,
} from "./appViewFixtures.ts";
import { removeHiddenWidgetEntry } from "../../test/hiddenWidgets.ts";
import { WorkspaceProvider } from "../../workspace/WorkspaceContext.tsx";
import { scopedMember, setScopedMember } from "../../workspace/scope.ts";
import {
  BOUND_EXTERNAL,
  DECLARED_CONTRACTS,
  EMPTY_COVERAGE,
  stubApi,
} from "../../workspace/testFixtures.ts";
import { EDGE_COLOR } from "../graph/graphModel.ts";
import { DECLARED_EDGE_TYPE } from "./serviceMapModel.ts";
import { WorkspaceView } from "./WorkspaceView.tsx";

// The service map mounts the real ECharts canvas, which needs a layout engine jsdom
// does not have. Stub it down to what the view actually contracts with: the node set
// it was handed, and the click that focuses a member.
vi.mock("../graph/GraphCanvas.tsx", () => ({
  GraphCanvas: ({
    loaded,
    onNodeClick,
  }: {
    loaded: {
      nodes: Record<string, { id: string; label: string; kind?: string | null }>;
      edges: { source: string; target: string; edge_type: string | null; admitted?: boolean }[];
    };
    onNodeClick: (id: string) => void;
  }) => (
    <div data-testid="canvas">
      <span data-testid="canvas-edges">{loaded.edges.length}</span>
      {/* The provenance channel, surfaced as DOM (S-419). The real canvas strokes
          it into a <canvas> bitmap, which has no DOM to assert against at all, so
          the double states the two things the contract is made of: that the line
          is marked admitted, and that its `edge_type` is STILL the relation arm
          (CR-132 AC3 — provenance must never become a new edge type).

          Filtered, not mapped: an edge carrying no marker renders nothing here, so
          the literal-only DOM this file records stays byte-identical. */}
      {loaded.edges
        .filter((e) => e.admitted)
        .map((e) => (
          <span key={`${e.source}->${e.target}`} data-testid="canvas-admitted-edge">
            {`${e.source}->${e.target}:${e.edge_type}`}
          </span>
        ))}
      {/* The topic HOPS, surfaced as DOM (S-424, FR-WS-27 AC6). Same reason as
          above — the real canvas strokes them into a <canvas> bitmap with no DOM
          to assert against — and the same discipline: FILTERED to the two hop
          types, never mapped over every edge, so a workspace with no topic hop
          (which is every fixture recorded before this story, the literal-only
          byte-for-byte snapshot included) renders exactly the DOM it did before.

          What this lets a spec assert is the thing FR-WS-27 AC6 is about: that a
          resolved broker coupling is drawn as `publisher -> topic -> subscriber`
          and NOT as a flat service line. Reading `map.loaded` instead would pass
          for a hop the view never renders. */}
      {loaded.edges
        .filter((e) => e.edge_type === "publishes" || e.edge_type === "subscribes")
        .map((e) => (
          <span key={`hop:${e.source}->${e.target}`} data-testid="canvas-topic-hop">
            {`${e.source}->${e.target}:${e.edge_type}`}
          </span>
        ))}
      {/* The FLAT broker line, likewise surfaced so its survival can be asserted
          directly. Without it, "the flat line is still drawn" could only be
          inferred from a total edge count — and a regression that dropped the
          line while emitting any other second edge would keep that inference
          green. Filtered to `broker-topic`, so no fixture without one renders
          anything new. */}
      {loaded.edges
        .filter((e) => e.edge_type === "broker-topic")
        .map((e) => (
          <span key={`flat:${e.source}->${e.target}`} data-testid="canvas-flat-broker-edge">
            {`${e.source}->${e.target}:${e.edge_type}`}
          </span>
        ))}
      {/* The BUILD layer's edges (S-464), surfaced as DOM for the same reason as
          the hops above, and FILTERED to the `build` class so every fixture
          without a build layer — the recorded snapshots included — renders
          exactly the DOM it did before. The edge type is printed, so "a distinct
          class" is asserted on what the canvas was handed, not on the model. */}
      {loaded.edges
        .filter((e) => e.edge_type === "build")
        .map((e) => (
          <span key={`build:${e.source}->${e.target}`} data-testid="canvas-build-edge">
            {`${e.source}->${e.target}:${e.edge_type}`}
          </span>
        ))}
      {/* The DECLARED layer (S-461): its edges and its external nodes, surfaced
          as DOM for the reason the build edges are, and FILTERED to the
          `declares-contract` class and the `external` node kind, so a workspace
          that declares nothing — every snapshot recorded before this story —
          renders exactly the DOM it did. An external is printed with its id, so
          "keyed by identity, labelled by name" is asserted on what the canvas
          was handed. */}
      {loaded.edges
        .filter((e) => e.edge_type === "declares-contract")
        .map((e) => (
          <span key={`declared:${e.source}->${e.target}`} data-testid="canvas-declared-edge">
            {`${e.source}->${e.target}:${e.edge_type}`}
          </span>
        ))}
      {Object.values(loaded.nodes)
        .filter((n) => n.kind === "external")
        .map((n) => (
          <span key={`external:${n.id}`} data-testid="canvas-external-node">
            {`${n.id}=${n.label}`}
          </span>
        ))}
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
  // Both ends observed at the call site — the baseline this file records the
  // unchanged DOM for (CR-132 AC6).
  from_value: { provenance: "literal" },
  to_value: { provenance: "literal" },
};

/** A resolved **broker** coupling: `api` publishes, `web` subscribes, on the arm
 *  that is drawn through its topic node rather than as a flat line (S-256,
 *  FR-WS-11). Both ends literal, so the provenance channel stays out of the way
 *  and the hop assertions below are about the hop and nothing else. */
const BROKER_BINDING: BridgeEdge = {
  relation: "broker-topic",
  from: { member: "api", symbol: "emit" },
  to: { member: "web", symbol: "onCommand" },
  from_value: { provenance: "literal" },
  to_value: { provenance: "literal" },
};

/** The same coupling, but with the PROVIDER end admitted from committed
 *  configuration — ADR-64's named shape, and the fixture every provenance
 *  rendering below is asserted against. */
const ADMITTED_BINDING: BridgeEdge = {
  relation: "route",
  from: { member: "api", symbol: "op2" },
  to: { member: "web", symbol: "route2" },
  from_value: { provenance: "literal" },
  to_value: {
    provenance: "config-bound",
    bound: [
      {
        key: "billing.base-url",
        source: "properties",
        values: [
          {
            value: "http://billing:8080",
            profiles: ["docker"],
            unprofiled: false,
            sources: ["src/main/resources/application-docker.yml"],
          },
          {
            value: "http://billing.svc:8080",
            profiles: [],
            unprofiled: true,
            sources: ["src/main/resources/application.yml"],
          },
        ],
      },
    ],
  },
};

/** A coupling whose consumer end names a key no committed source admits. */
const REFUSED_BINDING: BridgeEdge = {
  relation: "grpc-call",
  from: { member: "api", symbol: "stub" },
  to: { member: "web", symbol: "svc" },
  from_value: {
    provenance: "config-unresolved",
    keys: ["billing.grpc.target"],
    refusal: "missing-key",
  },
  to_value: { provenance: "literal" },
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

  /* CR-132 AC6 / FR-UI-29: a workspace with NO admitted binding must render
     byte-for-byte as it did before the provenance channel existed.

     The recorded file was written from the tree as it stood BEFORE that channel
     was added, and is never re-recorded: `-u` on this spec would silently turn
     the criterion into "renders however it renders today", which is the one thing
     it exists to prevent. The subtree is the service-map panel itself (the mocked
     canvas's parent), so a change anywhere in the map's own DOM — a legend
     section, a table column, an evidence list — fails it. */
  it("renders a literal-only service map identically to the DOM recorded before provenance", async () => {
    stubApi({ providers: [BINDING] });
    mount();
    await waitFor(() => expect(screen.getByTestId("canvas")).toBeInTheDocument());
    const panel = screen.getByTestId("canvas").parentElement!;
    await expect(panel.innerHTML).toMatchFileSnapshot(
      "./__snapshots__/service-map.literal-only.html",
    );
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

  // ── S-419 / CR-132 / ADR-64: bridge-edge provenance, asserted on RENDERED DOM ──
  // Every fixture below reads the DOM the user is shown. Asserting the model
  // instead would pass for a breakdown the view never renders, which is the
  // failure mode this project has a standing rule against.

  it("renders the per-kind breakdown in the table — never one label for a mixed link", async () => {
    stubApi({ providers: [BINDING, ADMITTED_BINDING] });
    mount();
    await waitFor(() => expect(screen.getByTestId("canvas")).toBeInTheDocument());

    // The column exists…
    expect(screen.getByRole("columnheader", { name: /Provenance/ })).toBeInTheDocument();
    // …and the ONE row both bindings aggregate into states both kinds, with counts.
    const row = screen.getByRole("cell", { name: "api" }).closest("tr")!;
    expect(within(row).getByRole("cell", { name: /Written at the call site/ })).toHaveTextContent(
      /Written at the call site\s*1/,
    );
    expect(within(row).getByRole("cell", { name: /Written at the call site/ })).toHaveTextContent(
      /Admitted from committed configuration\s*1/,
    );
    // The weight is still 2 — the breakdown decomposes the line, it does not replace it.
    expect(within(row).getByRole("cell", { name: "2" })).toBeInTheDocument();
  });

  it("lists EVERY kind present on one row, not just the first two", async () => {
    // Three bindings under one (consumer, provider, arm) key, spanning three
    // kinds. The cell renders per-kind rows; with only ever two kinds present a
    // renderer that showed the first two would have passed.
    const third: BridgeEdge = {
      relation: "route",
      from: { member: "api", symbol: "op3" },
      to: { member: "web", symbol: "route3" },
      from_value: { provenance: "config-unresolved", keys: ["x.y"], refusal: "uncommitted" },
      to_value: { provenance: "literal" },
    };
    stubApi({ providers: [BINDING, ADMITTED_BINDING, third] });
    mount();
    await waitFor(() => expect(screen.getByTestId("canvas")).toBeInTheDocument());

    const cell = screen
      .getByRole("cell", { name: /Written at the call site/ });
    expect(cell).toHaveTextContent(/Written at the call site\s*1/);
    expect(cell).toHaveTextContent(/Admitted from committed configuration\s*1/);
    expect(cell).toHaveTextContent(/Names a key the committed sources do not admit\s*1/);
    // Three kinds, three rows — the breakdown is complete, not truncated.
    expect(cell.querySelectorAll("li")).toHaveLength(3);
  });

  it("marks the canvas line admitted while its edge_type stays the relation arm", async () => {
    stubApi({ providers: [ADMITTED_BINDING] });
    mount();
    await waitFor(() => expect(screen.getByTestId("canvas")).toBeInTheDocument());
    expect(screen.getByTestId("canvas-admitted-edge")).toHaveTextContent(
      "service:api->service:web:route",
    );
  });

  it("renders the legend's provenance section IF AND ONLY IF a link is non-literal", async () => {
    // Literal only → no section. The legend is unchanged from S-250.
    stubApi({ providers: [BINDING] });
    const { unmount } = mount();
    await waitFor(() => expect(screen.getByTestId("canvas")).toBeInTheDocument());
    const legend = screen.getByText("Legend").closest("details")!;
    expect(within(legend).queryByText("Provenance")).toBeNull();
    expect(within(legend).queryByText(/Admitted from committed configuration/)).toBeNull();
    unmount();
    cleanup();

    // One admitted binding → the section, with both strokes drawn in one arm's hue.
    stubApi({ providers: [ADMITTED_BINDING] });
    mount();
    await waitFor(() => expect(screen.getByTestId("canvas")).toBeInTheDocument());
    // Scoped to the legend: "Provenance" is also the bindings table's column
    // header, and an unscoped query would pass on the wrong element.
    const legendAfter = screen.getByText("Legend").closest("details")!;
    const heading = within(legendAfter).getByText("Provenance");
    const rows = heading.nextElementSibling!.querySelectorAll("line");
    expect(rows).toHaveLength(2);
    // The provenance channel is the STROKE; the hue is still the arm's, identical
    // on both rows — that is what makes it a second channel rather than a new arm.
    expect(rows[0].getAttribute("stroke-dasharray")).toBe("0");
    expect(rows[1].getAttribute("stroke-dasharray")).toBe("9 3 2 3");
    expect(rows[0].getAttribute("stroke")).toBe(rows[1].getAttribute("stroke"));
  });

  it("names the evidence behind an ADMITTED end: key, every overlay, and its sources", async () => {
    stubApi({ providers: [ADMITTED_BINDING] });
    mount();
    await waitFor(() => expect(screen.getByTestId("canvas")).toBeInTheDocument());

    const detail = screen.getByText(/Binding evidence/).closest("section")!;
    // The key appears once per overlay row — that repetition IS the requirement,
    // so this asserts two, not one.
    expect(within(detail).getAllByRole("cell", { name: "billing.base-url" })).toHaveLength(2);
    // ONE ROW PER OVERLAY — a key its overlays spell differently proves several
    // values, and showing one of them would report a divergence as the value.
    expect(within(detail).getByRole("cell", { name: "http://billing:8080" })).toBeInTheDocument();
    expect(
      within(detail).getByRole("cell", { name: "http://billing.svc:8080" }),
    ).toBeInTheDocument();
    expect(within(detail).getByRole("cell", { name: "docker" })).toBeInTheDocument();
    // `unprofiled` is stated in words, never left to an empty profiles cell.
    expect(within(detail).getByRole("cell", { name: "unprofiled source" })).toBeInTheDocument();
    expect(
      within(detail).getByRole("cell", {
        name: "src/main/resources/application-docker.yml",
      }),
    ).toBeInTheDocument();
    // Both overlay rows name the same end — it is the same provider end, proved
    // twice over.
    expect(within(detail).getAllByText(/Provider · web/)).toHaveLength(2);
  });

  it("names a REFUSED end's keys and its refusal, never an empty value cell", async () => {
    stubApi({ providers: [REFUSED_BINDING] });
    mount();
    await waitFor(() => expect(screen.getByTestId("canvas")).toBeInTheDocument());

    const detail = screen.getByText(/Binding evidence/).closest("section")!;
    expect(within(detail).getByRole("cell", { name: "billing.grpc.target" })).toBeInTheDocument();
    expect(
      within(detail).getByRole("cell", { name: "No committed source defines it" }),
    ).toBeInTheDocument();
    expect(within(detail).getByText(/Consumer · api/)).toBeInTheDocument();
    // …and the table says so too, rather than calling the coupling observed.
    expect(
      screen.getByRole("cell", { name: /Names a key the committed sources do not admit/ }),
    ).toBeInTheDocument();
  });

  it("counts a binding whose from_value never arrived as `unstated`, never literal", async () => {
    // The wire is not runtime-validated, so a payload can reach the view without
    // the field the type declares required. Rendering it as "Written at the call
    // site" would report an unread field as observed evidence (CR-132 AC2).
    const { from_value: _dropped, ...withoutProvenance } = BINDING;
    stubApi({ providers: [withoutProvenance] });
    mount();
    await waitFor(() => expect(screen.getByTestId("canvas")).toBeInTheDocument());
    const cell = screen.getByRole("cell", { name: /Provenance not stated/ });
    expect(cell).toHaveTextContent(/Provenance not stated\s*1/);
    // …and the line is NOT also claimed as observed. Scoped to the cell: the
    // legend's own provenance section names the literal stroke on every
    // non-literal workspace, and an unscoped query would match that instead.
    expect(within(cell).queryByText(/Written at the call site/)).toBeNull();
  });

  it("renders NO evidence card and NO provenance column for a literal-only workspace", async () => {
    stubApi({ providers: [BINDING] });
    mount();
    await waitFor(() => expect(screen.getByTestId("canvas")).toBeInTheDocument());
    expect(screen.queryByText(/Binding evidence/)).toBeNull();
    expect(screen.queryByRole("columnheader", { name: /Provenance/ })).toBeNull();
    expect(screen.queryByTestId("canvas-admitted-edge")).toBeNull();
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

  // ── S-424 / FR-WS-27 AC6: the resolved coupling is drawn as a HOP ──────────
  // The inventory and the bindings key a config-bound broker site the same way
  // now, so `drawnThroughATopic` fires and the flat service line is replaced.
  // Both fixtures below read the DOM the user is shown; asserting `map.loaded`
  // would pass for a hop the view never renders.

  it("renders a resolved broker coupling as publisher → topic → subscriber, not a flat line", async () => {
    // The shape S-424 creates: ONE topic node, named by the committed value, with
    // `api` producing and `web` consuming it — plus the bridge binding for the
    // same coupling. Before S-424 the inventory carried two placeholder-keyed
    // topics instead, neither of which had both ends, so this rendered as a flat
    // `broker-topic` line and the hop was drawn for nothing.
    stubApi({
      providers: [BROKER_BINDING],
      topics: [
        { member: "api", topics: [{ topic: "archive-commands", producers: 1, consumers: 0 }] },
        { member: "web", topics: [{ topic: "archive-commands", producers: 0, consumers: 1 }] },
      ],
    });
    mount();
    await waitFor(() => expect(screen.getByTestId("canvas")).toBeInTheDocument());

    // The hop, both legs, in the direction the message travels.
    expect(screen.getAllByTestId("canvas-topic-hop").map((n) => n.textContent)).toEqual([
      "service:api->topic:archive-commands:publishes",
      "topic:archive-commands->service:web:subscribes",
    ]);
    // …and NOT also as a flat line — asserted on the line's own absence, not
    // inferred from a total. `textContent` compared exactly, because jest-dom's
    // `toHaveTextContent` is a SUBSTRING match and would accept 12, 20, 21 or 23.
    expect(screen.queryAllByTestId("canvas-flat-broker-edge")).toHaveLength(0);
    expect(screen.getByTestId("canvas-edges").textContent).toBe("2");
    // The topic is a node the user can see, labelled by the committed value.
    expect(screen.getByRole("button", { name: "archive-commands" })).toBeInTheDocument();
    // The coupling is still COUNTED — the hop replaces the drawn line, not the fact.
    expect(
      screen.getByRole("cell", { name: "Broker (publish ↔ subscribe)" }),
    ).toBeInTheDocument();
  });

  it("keeps the flat broker line when no topic hop carries the pair — a coupling is never silently un-drawn", async () => {
    // The same binding with an inventory that does not carry the pair (a member
    // last indexed before S-424, whose topics are still placeholder-keyed). The
    // line must stay, or the map under-draws the workspace (NFR-CC-04).
    stubApi({
      providers: [BROKER_BINDING],
      topics: [
        {
          member: "api",
          topics: [
            { topic: "${spring.kafka.topics.archive-commands}", producers: 1, consumers: 0 },
          ],
        },
      ],
    });
    mount();
    await waitFor(() => expect(screen.getByTestId("canvas")).toBeInTheDocument());

    // One publish hop (the topic exists, with a producer and no consumer)…
    expect(screen.getAllByTestId("canvas-topic-hop").map((n) => n.textContent)).toEqual([
      "service:api->topic:${spring.kafka.topics.archive-commands}:publishes",
    ]);
    // …plus the flat line, NAMED, because no hop carries api → web. That the
    // surviving edge is this one is the whole claim; a count alone would pass
    // for any second edge at all.
    expect(screen.getAllByTestId("canvas-flat-broker-edge").map((n) => n.textContent)).toEqual([
      "service:api->service:web:broker-topic",
    ]);
    expect(screen.getByTestId("canvas-edges").textContent).toBe("2");
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

  // S-612 (FR-UI-41): the Cross-service impact tab is hidden through the
  // register, so the Workspace tab offers exactly the other two.
  it("renders exactly two tabs: Service map and Cross-service coverage", async () => {
    stubApi({ coverage: COVERAGE, providers: [BINDING] });
    mount();
    await screen.findByRole("tab", { name: /service map/i });
    expect(screen.getAllByRole("tab").map((t) => t.textContent)).toEqual([
      "Service map",
      "Cross-service coverage",
    ]);
  });

  // S-612 (FR-UI-41): the per-arm board is hidden through the register; the
  // headline, spec conformance and by-intake boards stay.
  it("renders no per-arm board on the coverage tab", async () => {
    stubApi({ coverage: COVERAGE, providers: [BINDING] });
    mount();
    await userEvent.click(await screen.findByRole("tab", { name: /cross-service coverage/i }));
    // The title glosses "intake", so its accessible name carries the gloss after it.
    expect(screen.getByRole("heading", { name: /^Coverage by intake/ })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Coverage by relation arm" })).toBeNull();
    expect(screen.queryByRole("table", { name: /by relation arm/i })).toBeNull();
    expect(screen.queryByText("Target read from")).toBeNull();
  });

  // The per-arm tests below run with the board's register entry removed: they
  // are what proves the board returns intact when the entry goes (S-612).
  it("shows the per-arm board whose columns RECONCILE with the headline above them", async () => {
  onTestFinished(removeHiddenWidgetEntry("coverage-by-relation-arm"));
    stubApi({ coverage: COVERAGE, providers: [BINDING] });
    mount();
    await userEvent.click(await screen.findByRole("tab", { name: /cross-service coverage/i }));

    // The ratio is the server's (33.3%), not bound/total (1/5 = 20%): the two
    // no-provider references are outside the denominator (ADR-53).
    expect(screen.getAllByText("33.3%").length).toBeGreaterThan(0);
    expect(screen.getByText(/2 call a service outside this workspace/)).toBeInTheDocument();

    // The `route` row: 1 bound, 0 ambiguous, 1 unbound, 2 no-provider. The wire `bucket`
    // says "unbound" for the no-provider pair, but the summary's `unbound` counter
    // excludes them — so folding them in would print "3 unbound" inches below a headline
    // that says "1 unbound". Pin the split.
    const routeRow = screen.getByRole("cell", { name: /HTTP \(OpenAPI ↔ route\)/ }).closest("tr")!;
    const cells = [...routeRow.querySelectorAll("td")].map((c) => c.textContent);
    expect(cells.slice(1, 5)).toEqual(["1", "0", "1", "2"]);
    // In the arm row; the headline's action names the same reason across arms.
    expect(within(routeRow).getByText(/Path could not be composed/)).toBeInTheDocument();
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
    expect(screen.queryByText(/calls a service outside it/)).toBeNull();
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

    expect(screen.getByText(COVERAGE_TEXT.capturedOutside(2))).toBeInTheDocument();
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
    // Each coverage widget states the absence in its figure row (FR-UI-40), and
    // none draws a bar over nothing.
    const panel = screen.getByRole("tabpanel");
    const absences = [...panel.querySelectorAll("[data-widget-absence]")].map((a) => a.textContent ?? "");
    expect(absences.filter((a) => /no cross-boundary references found/i.test(a))).toHaveLength(3);
    expect(panel.querySelectorAll("meter")).toHaveLength(0);
  });

  // CR-118 §4.5: the provider-identity fields are OPTIONAL, and the view must
  // render rows both with and without them. `COVERAGE` above is the WITHOUT
  // shape (an older store, or a current one with no provider to name) and is
  // already exercised by every test in this block; this is the WITH shape,
  // asserted to render the identical board — the widened payload informs the
  // per-reference detail, it does not move a count on this screen.
  it("renders the identical board from rows carrying the CR-118 provider fields", async () => {
  onTestFinished(removeHiddenWidgetEntry("coverage-by-relation-arm"));
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
  // The tab is hidden through the register (S-612, FR-UI-41). Every test here
  // removes its entry, so this block is what proves the tab returns intact.
  let restore: () => void;
  beforeEach(() => {
    restore = removeHiddenWidgetEntry("cross-service-impact");
  });
  afterEach(() => restore());

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

  // ── CR-125 / BR-53: an unresolved egress must not read as an absence ────────

  /** The residue exactly as the API composed it — the single-composed-field
   *  discipline the `summary` field exists for. Asserted WHOLE, and carrying a
   *  sentinel clause absent from the numeric fields, so a view that recomposed
   *  the line locally from `unresolved_sites`/`measured_sites` fails. */
  const RESIDUE_SUMMARY =
    "no resolved cross-service impacts; 141 of 146 captured outbound sites in scope did not resolve across 2 members (base-url-runtime 141); 31 more have no provider in this workspace";
  const RESIDUE = {
    members_in_scope: 2,
    measured_sites: 146,
    unresolved_sites: 141,
    no_provider_in_workspace: 31,
    by_reason: [{ reason: "base-url-runtime", sites: 141 }],
    covers_all_members: true,
    summary: RESIDUE_SUMMARY,
  };

  it("renders an empty answer over a NON-ZERO residue as unresolved, naming the count", async () => {
    stubApi({ impact: { ...IMPACT_DEGRADED, unresolved_egress: RESIDUE } });
    mount();
    await traceSymbol();

    // The count IS the answer here: "no cross-service impact" and "141 outbound
    // sites I could not resolve" must not render alike (CR-125 §2). The whole
    // composed line, so a locally-recomposed substitute cannot pass.
    expect(await screen.findByText(RESIDUE_SUMMARY)).toBeInTheDocument();
    expect(screen.queryByText(/no cross-service impact —/i)).not.toBeInTheDocument();
  });

  it("still reports the residue when the answer is NOT empty", async () => {
    // A partial answer is still partial: the residue rides every reachability
    // answer, not only the one that resolved nothing (CR-125 §3.2).
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
              upstream: [],
              downstream_label: "Calls",
              downstream: [],
              docs_label: "Docs",
              docs: [],
              suggestions: [],
              warnings: [],
            },
          },
        ],
        unresolved_egress: RESIDUE,
      },
    });
    mount();
    await traceSymbol();
    expect(await screen.findByText(RESIDUE_SUMMARY)).toBeInTheDocument();
  });

  it("leaves the empty state untouched when the residue is ZERO", async () => {
    // No `unresolved_egress` key at all is how a zero residue arrives; the
    // rendering must be exactly what it was before CR-125.
    stubApi({ impact: IMPACT_DEGRADED });
    mount();
    await traceSymbol();
    expect(await screen.findByText(/no cross-service impact/i)).toBeInTheDocument();
    expect(screen.queryByText(/did not resolve/i)).not.toBeInTheDocument();
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

    expect(await screen.findByText(COVERAGE_TEXT.specNotMeasured(2))).toBeInTheDocument();
    expect(screen.queryByText(/0\.0% ·/)).toBeNull();
    expect(screen.queryByText(/100\.0% ·/)).toBeNull();
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
    // The figure is the rate's own numerator and denominator, both the server's.
    expect(screen.getByText(COVERAGE_TEXT.outboundResolved(0, 2))).toBeInTheDocument();
    // And the retired vocabulary is absent from the whole rendered surface.
    expect(container.textContent).not.toMatch(/bound[ -]ratio/i);
  });

  /** **A NON-ZERO headline renders the server's line, not a recomposed one**
   *  ([CR-127], [FR-WS-05]).
   *
   *  Every other fixture on this surface carries `0`, which is what the server
   *  could send while the count excluded `config-bound` rows — so the rendering of
   *  a NON-ZERO line was untested. Asserted on rendered
   *  DOM text, so a view that rebuilt the sentence from the numbers beside it and
   *  drifted from the CLI and MCP renderings fails here — verified by making
   *  `WorkspaceView` recompose the line, which fails this test and its
   *  absent-rate sibling.
   *
   *  **The triple below is illustrative, and deliberately not an estate reading.**
   *  It was the reference workspace's headline when S-403 wrote this fixture and
   *  stopped being one when S-420 (CR-133) closed the HTTP arm. This file is on
   *  neither roster that enumerates the sites a re-measurement sweeps, so a figure
   *  kept current here goes quietly stale — which is exactly what it did. What the
   *  test needs is a non-zero count whose composed line the view must not rebuild;
   *  any triple serves. The estate's figures, dated and with their denominators,
   *  live in `logos-core/tests/config_bound_admission.rs`. */
  it("renders a NON-ZERO resolved-edge headline as the server composed it", async () => {
    stubApi({
      coverage: {
        ...COVERAGE,
        // The invocation half agrees with the rate: 15 resolved of 117 captured.
        by_intake: {
          ...COVERAGE.by_intake,
          invocation: { bound: 15, ambiguous: 1, unbound: 101, no_provider_in_workspace: 0 },
        },
        resolved_cross_service_edges: 15,
        egress_resolution: 15 / 117,
        egress_resolution_measured: 117,
        resolved_edges_summary:
          "15 resolved cross-service edges; egress resolution 0.128 (15 of 117 egress sites resolved)",
      } as typeof COVERAGE,
    });
    const { container } = mount();

    expect(
      await screen.findByText(
        "15 resolved cross-service edges; egress resolution 0.128 (15 of 117 egress sites resolved)",
      ),
    ).toBeInTheDocument();
    expect(scoreBarClassIn(container, /^Resolved cross-service edges/)).toBeTruthy();
    expect(screen.getByText(COVERAGE_TEXT.outboundResolved(15, 117))).toBeInTheDocument();
    // The count in the sentence is not contradicted anywhere on the surface: the
    // exact string "0 resolved cross-service edges" must be absent, which is the
    // shipped defect's own rendering (CR-127 §3.1).
    expect(container.textContent).not.toMatch(/0 resolved cross-service edges/);
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
    expect(screen.getByText(COVERAGE_TEXT.outboundNotMeasured)).toBeInTheDocument();
    expect(screen.queryByText(/outbound call sites? resolved$/)).toBeNull();
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
    expect(await screen.findByText(/33\.3% ·/)).toBeInTheDocument();
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

    // Every coverage widget states the qualified absence — never the whole-workspace claim.
    expect(
      await screen.findAllByText(/among the 9 of 72 workspace members that could be read/i),
    ).toHaveLength(3);
    expect(
      screen.queryByText(/No cross-boundary references found in this workspace/i),
      "the unqualified whole-workspace claim must be gone",
    ).toBeNull();
    // And the shortfall rider renders in the empty branch too.
    expect(screen.getByText(/every figure here is a minimum/i)).toBeInTheDocument();
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
  onTestFinished(removeHiddenWidgetEntry("coverage-by-relation-arm"));
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

// ── S-464 / CR-148 / FR-WS-33: the build layer, asserted on RENDERED DOM ──────
// A build dependency is never a runtime coupling (BR-58): the layer is off by
// default, draws in its own class when on, collapses declared platforms, and the
// cross-context hint is never an edge. Every assertion reads what the user is
// shown (or what the canvas was handed), never `buildLayer`'s return value.

/** `web` is a declared platform: `api`'s parent edge into it is collapsed. */
const BUILD_HEADLINE: BuildDependencyHeadline = {
  build_dependency_pairs: { pairs: 1, parent: 0, dependency: 1, managed: 0, "bom-import": 0 },
  references: { references: 5, to_member: 1, to_platform: 1, external: 3 } as BuildDependencyHeadline["references"],
  members: { members: 2, read: 2, with_manifests: 2, manifests: 2, manifests_read: 2 },
  platform_apart: {
    members: ["web"],
    build_dependency_pairs: { pairs: 1, parent: 1, dependency: 0, managed: 0, "bom-import": 0 },
    summary: "1 pairs (parent 1 · dependency 0 · managed 0 · bom-import 0) into 1 declared platform member",
  },
  collisions: [],
  platform_candidates: [],
  summary:
    "1 pairs (parent 0 · dependency 1 · managed 0 · bom-import 0) built against another member, from 1 of 5 referenced artifacts; a build dependency, never a runtime coupling",
};

const PLATFORM_ROW = {
  from: "api",
  to: "web",
  kind: "parent" as const,
  scope: null,
  artifact: "com.acme:web",
  references: 1,
  platform: true as const,
};
const DEPENDENCY_ROW = {
  from: "web",
  to: "api",
  kind: "dependency" as const,
  scope: null,
  artifact: "com.acme:api-client",
  references: 2,
};

const BUILD_DEPS: XserviceBuildDeps = {
  headline: BUILD_HEADLINE,
  members: [
    { member: "api", builds_against: [PLATFORM_ROW], built_against_by: [DEPENDENCY_ROW] },
    { member: "web", builds_against: [DEPENDENCY_ROW], built_against_by: [PLATFORM_ROW] },
  ],
  cross_context: [
    {
      member: "api",
      contexts: ["archive", "mailbox"],
      libraries: [
        { context: "archive", artifact: "com.acme.archive:kafka-models", member: "archive-kafka-models" },
        { context: "mailbox", artifact: "com.acme.mailbox:kafka-models", member: "mailbox-kafka-models" },
      ],
    },
  ],
};

function buildEdges(): string[] {
  return screen.queryAllByTestId("canvas-build-edge").map((e) => e.textContent ?? "");
}

describe("WorkspaceView — the build layer (S-464, FR-UI-29, FR-WS-33)", () => {
  it("renders NO toggle and fetches nothing over a workspace with no build manifest", async () => {
    const calls = stubApi({ providers: [BINDING] });
    mount();
    await waitFor(() => expect(screen.getByTestId("canvas")).toBeInTheDocument());
    expect(screen.queryByRole("checkbox", { name: /builds against/i })).toBeNull();
    // The service map says nothing about a build layer. (The coverage tab's Build
    // dependencies widget states the absence — asserted below.)
    expect(within(screen.getByRole("tabpanel")).queryByText(/build dependenc/i)).toBeNull();
    expect(calls().some((u) => u.includes("workspace/build-deps"))).toBe(false);
    expect(buildEdges()).toEqual([]);
  });

  it("draws NO build edge until the legend toggle is switched on — it is off by default", async () => {
    stubApi({ providers: [BINDING], buildDependency: BUILD_HEADLINE, buildDeps: BUILD_DEPS });
    mount();
    const toggle = await screen.findByRole("checkbox", { name: /draw what each member builds against/i });
    // Wait for the relation to arrive (the hint card renders from it), so "no
    // edge" is asserted with the data present rather than merely not loaded yet.
    await screen.findByText("Cross-context model hint");
    expect(toggle).not.toBeChecked();
    expect(buildEdges()).toEqual([]);
    expect(screen.getByTestId("canvas-edges")).toHaveTextContent("1");
    expect(screen.queryByRole("table", { name: /accessible twin of the build layer/i })).toBeNull();
    // Off, the legend documents no build class — only the toggle and the note.
    expect(screen.queryByText("Builds against (from its build manifest)")).toBeNull();
    // The note carries the server's composed headline line, never a figure of its
    // own (BR-51): pairs by kind beside their denominator.
    const note = toggle.closest("div")!.querySelector("p:last-of-type")!;
    expect(note.textContent).toContain(BUILD_HEADLINE.summary);
  });

  it("with the toggle ON, draws build edges in their own class and collapses platform members", async () => {
    stubApi({ providers: [BINDING], buildDependency: BUILD_HEADLINE, buildDeps: BUILD_DEPS });
    mount();
    await screen.findByText("Cross-context model hint");
    await userEvent.click(screen.getByRole("checkbox", { name: /draw what each member builds against/i }));

    // The one non-platform row is drawn, in the `build` class; `api`'s parent
    // edge into the declared platform `web` is NOT drawn — it is collapsed.
    expect(buildEdges()).toEqual(["service:web->service:api:build"]);
    expect(screen.getByTestId("canvas-edges")).toHaveTextContent("2");
    // The runtime edge is untouched beside it: one route line, still one.
    expect(screen.getByText(/HTTP \(OpenAPI ↔ route\)/)).toBeInTheDocument();

    const collapsed = screen.getAllByTestId("collapsed-platform").map((e) => e.textContent);
    expect(collapsed).toEqual(["web (1 member builds against it)"]);

    const table = screen.getByRole("table", { name: /accessible twin of the build layer/i });
    const cells = [...within(table).getAllByRole("cell")].map((c) => c.textContent);
    expect(cells).toEqual(["web", "api", "dependency", "com.acme:api-client", "2"]);
    expect(screen.getByText("Builds against (from its build manifest)")).toBeInTheDocument();

    // And off again: the layer leaves the canvas entirely.
    await userEvent.click(screen.getByRole("checkbox", { name: /draw what each member builds against/i }));
    expect(buildEdges()).toEqual([]);
    expect(screen.queryAllByTestId("collapsed-platform")).toEqual([]);
    expect(screen.queryByText("Builds against (from its build manifest)")).toBeNull();
  });

  it("lists the cross-context hint with every library named, and NEVER draws it as an edge", async () => {
    stubApi({ providers: [BINDING], buildDependency: BUILD_HEADLINE, buildDeps: BUILD_DEPS });
    mount();
    const card = (await screen.findByText("Cross-context model hint")).closest("section")!;
    const hint = within(card).getByRole("table");
    expect(within(hint).getByRole("cell", { name: "api" })).toBeInTheDocument();
    expect(within(hint).getByText("com.acme.archive:kafka-models")).toBeInTheDocument();
    expect(within(hint).getByText("com.acme.mailbox:kafka-models")).toBeInTheDocument();
    expect(within(hint).getByRole("cell", { name: "archive, mailbox" })).toBeInTheDocument();

    // Toggle on: the hint's libraries are no canvas edge, and their producers
    // (not roster services) are no canvas node.
    await userEvent.click(screen.getByRole("checkbox", { name: /draw what each member builds against/i }));
    expect(buildEdges().some((e) => e.includes("kafka-models"))).toBe(false);
    expect(screen.queryByRole("button", { name: "archive-kafka-models" })).toBeNull();
  });

  it("agrees the verb with the count: one member builds/depends, two members build/depend", async () => {
    stubApi({ providers: [BINDING], buildDependency: BUILD_HEADLINE, buildDeps: BUILD_DEPS });
    mount();
    expect(await screen.findByText(/^1 member depends on the model libraries/)).toBeInTheDocument();
    cleanup();

    const batchRow = { ...PLATFORM_ROW, from: "batch" };
    stubApi({
      providers: [BINDING],
      buildDependency: BUILD_HEADLINE,
      buildDeps: {
        ...BUILD_DEPS,
        members: [
          ...BUILD_DEPS.members,
          { member: "batch", builds_against: [batchRow], built_against_by: [] },
        ],
        cross_context: [...BUILD_DEPS.cross_context, { ...BUILD_DEPS.cross_context[0], member: "web" }],
      },
    });
    mount();
    expect(await screen.findByText(/^2 members depend on the model libraries/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("checkbox", { name: /draw what each member builds against/i }));
    expect(screen.getAllByTestId("collapsed-platform").map((e) => e.textContent)).toEqual([
      "web (2 members build against it)",
    ]);
  });

  it("renders NO hint card when no member depends on two contexts' model libraries", async () => {
    stubApi({
      providers: [BINDING],
      buildDependency: BUILD_HEADLINE,
      buildDeps: { ...BUILD_DEPS, cross_context: [] },
    });
    mount();
    await userEvent.click(
      await screen.findByRole("checkbox", { name: /draw what each member builds against/i }),
    );
    // The relation arrived (its layer is drawn), and still no card: an empty hint
    // is not a "0 members" statement.
    expect(await screen.findByRole("table", { name: /accessible twin of the build layer/i })).toBeInTheDocument();
    expect(screen.queryByText("Cross-context model hint")).toBeNull();
    expect(screen.queryByText(/depends? on the model/)).toBeNull();
  });

  it("states a FAILED build read with the toggle off — never a silent 'no hint'", async () => {
    stubApi({ providers: [BINDING], buildDependency: BUILD_HEADLINE, buildDepsStatus: 500 });
    mount();
    const toggle = await screen.findByRole("checkbox", { name: /draw what each member builds against/i });
    expect(toggle).not.toBeChecked();
    expect(await screen.findByText(/the build relation could not be read/i)).toHaveTextContent(
      /unknown, not absent/,
    );
    expect(screen.queryByText("Cross-context model hint")).toBeNull();
    // On, it still says so — no spinner in its place, and nothing drawn.
    await userEvent.click(toggle);
    expect(screen.getByText(/the build relation could not be read/i)).toBeInTheDocument();
    expect(screen.queryByText(/reading the build relation/i)).toBeNull();
    expect(buildEdges()).toEqual([]);
  });

  it("shows the build read IN FLIGHT once the toggle is on, and nothing drawn yet", async () => {
    stubApi({ providers: [BINDING], buildDependency: BUILD_HEADLINE, buildDepsStatus: "pending" });
    mount();
    const toggle = await screen.findByRole("checkbox", { name: /draw what each member builds against/i });
    expect(screen.queryByText(/reading the build relation/i)).toBeNull();
    await userEvent.click(toggle);
    expect(screen.getByText(/reading the build relation/i)).toBeInTheDocument();
    expect(buildEdges()).toEqual([]);
  });

  it("states the build headline on the coverage tab, apart from every runtime board", async () => {
    stubApi({ coverage: COVERAGE, providers: [BINDING], buildDependency: BUILD_HEADLINE, buildDeps: BUILD_DEPS });
    mount();
    await userEvent.click(await screen.findByRole("tab", { name: /cross-service coverage/i }));
    const card = screen.getByRole("heading", { name: "Build dependencies" }).closest("section")!;
    expect(card.textContent).toContain(BUILD_HEADLINE.summary);
    expect(card.textContent).toContain("never a runtime coupling");
    expect(card.textContent).toContain("Declared platform web");
    // The runtime headline is still the runtime headline: the roll-up counts no build pair.
    expect(screen.getByText(/2 services/).textContent).toMatch(/1 bound · 1 ambiguous · 1 unbound/);
  });

  it("states platform candidates, colliding artifacts and unread members on the coverage card", async () => {
    const headline: BuildDependencyHeadline = {
      ...BUILD_HEADLINE,
      members: { ...BUILD_HEADLINE.members, read: 1, unread: ["web"] },
      platform_candidates: [{ member: "api", in_degree: 2, of: 3 }],
      collisions: [{ artifact: "com.acme:common", producers: ["api", "web"], references: 7 }],
    };
    stubApi({ coverage: COVERAGE, providers: [BINDING], buildDependency: headline, buildDeps: BUILD_DEPS });
    mount();
    await userEvent.click(await screen.findByRole("tab", { name: /cross-service coverage/i }));
    const card = screen.getByRole("heading", { name: "Build dependencies" }).closest("section")!;
    const text = card.textContent ?? "";
    expect(text).toContain("Platform candidates (a hint; nothing is classified until declared): api (2 of 3)");
    expect(text).toContain("Produced by more than one member, so resolved to neither: com.acme:common (api, web)");
    // The unread member is unknown, never reported as having no build dependency (NFR-CC-04).
    expect(text).toContain("Build facts could not be read for web — their build dependencies are unknown, not absent.");
  });

  it("never reads an unread member's reason off Object.prototype", async () => {
    const headline: BuildDependencyHeadline = {
      ...BUILD_HEADLINE,
      members: { ...BUILD_HEADLINE.members, read: 0, unread: ["constructor", "toString"] },
    };
    stubApi({ coverage: COVERAGE, providers: [BINDING], buildDependency: headline, buildDeps: BUILD_DEPS });
    mount();
    await userEvent.click(await screen.findByRole("tab", { name: /cross-service coverage/i }));
    const card = screen.getByRole("heading", { name: "Build dependencies" }).closest("section")!;
    expect(card.textContent).toContain(
      "Build facts could not be read for constructor, toString — their build dependencies are unknown, not absent.",
    );
  });

  it("names each unread member's server-stated reason, so an upgraded store never reads as having no manifests", async () => {
    const headline: BuildDependencyHeadline = {
      ...BUILD_HEADLINE,
      members: {
        ...BUILD_HEADLINE.members,
        read: 0,
        unread: ["web", "api"],
        unread_reasons: { api: "build facts could not be read", web: "build facts not yet extracted" },
      },
    };
    stubApi({ coverage: COVERAGE, providers: [BINDING], buildDependency: headline, buildDeps: BUILD_DEPS });
    mount();
    await userEvent.click(await screen.findByRole("tab", { name: /cross-service coverage/i }));
    const card = screen.getByRole("heading", { name: "Build dependencies" }).closest("section")!;
    expect(card.textContent).toContain(
      "Build facts could not be read for web (build facts not yet extracted), api (build facts could not be read) — their build dependencies are unknown, not absent.",
    );
  });

  /* A workspace with no build manifest states the absence on the coverage tab and
     draws no build figure. Until S-613 this was a byte-for-byte recording of the
     tab from before the build layer existed; CR-203 rewrote every widget on the
     tab, and an absent relation is now a stated absence in its widget's figure
     row (FR-UI-40) rather than a missing card, so the recording was retired and
     its claim — no build layer is fabricated — is asserted directly. */
  it("states a manifest-less workspace's build relation as absent, with no build figure", async () => {
    const calls = stubApi({ coverage: COVERAGE, providers: [BINDING] });
    mount();
    await userEvent.click(await screen.findByRole("tab", { name: /cross-service coverage/i }));
    const card = screen.getByRole("heading", { name: "Build dependencies" }).closest("section")!;
    expect(card.querySelector("[data-widget-absence]")?.textContent).toBe(COVERAGE_TEXT.buildAbsent);
    expect(card.querySelector('[data-widget-part="evidence"]')).toBeNull();
    expect(calls().some((u) => u.includes("workspace/build-deps"))).toBe(false);
  });
});

// ── S-461 / CR-147 / FR-WS-31: declared contracts and named externals ─────────
// A declared contract is never an observed call (BR-57): its own edge class, a
// legend entry, externals as named nodes keyed by identity, and edge detail
// naming the document, the identity score or matched operation, and the
// base-path source. Every assertion reads what the user is shown (or what the
// canvas was handed), never `declaredLayer`'s return value.

/** The coverage payload over a workspace that declares — the runtime figures
 *  are the empty workspace's, so every declared figure below is the relation's. */
const DECLARING: CrossServiceCoverage = {
  ...EMPTY_COVERAGE,
  declared_contracts: DECLARED_CONTRACTS,
  bound_external: BOUND_EXTERNAL,
};

function declaredEdges(): string[] {
  return screen.queryAllByTestId("canvas-declared-edge").map((e) => e.textContent ?? "");
}

function externalNodes(): string[] {
  return screen.queryAllByTestId("canvas-external-node").map((e) => e.textContent ?? "");
}

describe("WorkspaceView — declared contracts and named externals (S-461, FR-UI-29, FR-WS-31)", () => {
  it("draws declared contracts in their own class, and externals as nodes keyed by identity and labelled by name", async () => {
    stubApi({ providers: [BINDING], coverage: DECLARING });
    mount();
    await waitFor(() => expect(declaredEdges()).toHaveLength(4));
    expect(declaredEdges()).toEqual([
      "service:api->external:api:pss.yaml:declares-contract",
      "service:api->service:web:declares-contract",
      "service:web->external:api:pss.yaml:declares-contract",
      "service:web->external:web:legacy/pss.yaml:declares-contract",
    ]);
    // Two externals titled PSS are two nodes, not one merged by name.
    expect(externalNodes()).toEqual(["external:api:pss.yaml=PSS", "external:web:legacy/pss.yaml=PSS"]);
    expect(within(screen.getByTestId("canvas")).getAllByRole("button", { name: "PSS" })).toHaveLength(2);
    // The runtime binding is untouched beside them: one route line plus four declared.
    expect(screen.getByTestId("canvas-edges")).toHaveTextContent("5");
    expect(screen.queryAllByTestId("canvas-admitted-edge")).toEqual([]);
  });

  it("clicking an external selects no member — it is not a service", async () => {
    stubApi({ providers: [BINDING], coverage: DECLARING });
    setScopedMember("web");
    mount();
    const [pss] = await within(await screen.findByTestId("canvas")).findAllByRole("button", { name: "PSS" });
    await userEvent.click(pss);
    expect(scopedMember()).toBe("web");
  });

  it("gives the declared class and the external node a legend entry, with the server's summary beside them", async () => {
    stubApi({ providers: [BINDING], coverage: DECLARING });
    mount();
    const legend = (await screen.findByText("Declared contracts", { selector: "span" })).closest("details")!;
    // The swatch is the class's own — its hue and its dotted stroke, the same
    // two channels the canvas draws the edge with — never another arm's.
    const swatch = within(legend).getByText("Declares a contract (a vendored spec)").closest("li")!.querySelector("line")!;
    expect(swatch.getAttribute("stroke")).toBe(EDGE_COLOR[DECLARED_EDGE_TYPE]);
    expect(swatch.getAttribute("stroke")).not.toBe(EDGE_COLOR.build);
    expect(swatch.getAttribute("stroke-dasharray")).toBe("2 3");
    expect(within(legend).getByText("Named external — not a member (topics share this hue)")).toBeInTheDocument();
    expect(within(legend).getByText(/never an observed call/)).toHaveTextContent(
      DECLARED_CONTRACTS.headline.summary,
    );
  });

  it("the edge detail names each document with its identity score or external, and each bound call's operation and base-path source", async () => {
    stubApi({ providers: [BINDING], coverage: DECLARING });
    mount();
    const card = (await screen.findByRole("heading", { name: "Declared contracts" })).closest("section")!;

    // The accessible twin: one row per drawn link.
    const twin = within(card).getByRole("table", { name: /accessible twin of the declared layer/i });
    const rows = within(twin)
      .getAllByRole("row")
      .slice(1)
      .map((r) => within(r).getAllByRole("cell").map((c) => c.textContent));
    expect(rows).toEqual([
      ["api", "PSS (named external api:pss.yaml)", "1", "1"],
      ["api", "web", "1", "—"],
      ["web", "PSS (named external api:pss.yaml)", "1", "0"],
      ["web", "PSS (named external web:legacy/pss.yaml)", "1", "1"],
    ]);

    // Identity: the document, the score, and the member's own document it matched.
    const identity = within(card).getByRole("table", {
      name: "Documents by which api declares a contract with web",
      hidden: true,
    });
    // The whole row: the vendored DOCUMENT, then what it declares — a missing
    // or empty document cell must fail here, not pass a `toContain`.
    expect(within(identity).getAllByRole("cell", { hidden: true }).map((c) => c.textContent)).toEqual([
      "specs/web.yaml",
      "Document identity: 3 of 3 operations match web's own api/openapi.yaml",
    ]);
    // An external target: the document, and the external named WITH its
    // identity — two externals here are both titled PSS.
    const external = within(card).getByRole("table", {
      name: "Documents by which web declares a contract with PSS (web:legacy/pss.yaml)",
      hidden: true,
    });
    expect(within(external).getAllByRole("cell", { hidden: true }).map((c) => c.textContent)).toEqual([
      "legacy/pss.yaml",
      "Named external PSS web:legacy/pss.yaml",
    ]);

    // The bound call: target, matched operation, base path and its source.
    const calls = within(card).getByRole("table", {
      name: /Calls from api bound to PSS \(api:pss\.yaml\)/,
      hidden: true,
    });
    expect(within(calls).getAllByRole("cell", { hidden: true }).map((c) => c.textContent)).toEqual([
      "GET ${pss.uri-get-mailbox}",
      "GET /prov/domain/{}/user/{}",
      "/prov",
      "Deploy overlay · deploy-coll/values.yaml · envfrom.pssbaseurl",
    ]);
    // The other base-path shape: a host-only base URL committed by application
    // configuration in two files — the path stated as none, the origin named,
    // and EVERY source listed, not the first.
    const hostOnly = within(card).getByRole("table", {
      name: /Calls from web bound to PSS \(web:legacy\/pss\.yaml\)/,
      hidden: true,
    });
    const hostOnlyCells = within(hostOnly).getAllByRole("cell", { hidden: true });
    expect(hostOnlyCells.slice(0, 3).map((c) => c.textContent)).toEqual([
      "POST ${legacy.uri-send}",
      "POST /v1/send",
      "none (host only)",
    ]);
    expect(within(hostOnlyCells[3]).getAllByRole("listitem", { hidden: true }).map((li) => li.textContent)).toEqual([
      "Application configuration · src/main/resources/application.yml · legacy.base-url",
      "Application configuration · src/test/resources/application-it.yml · legacy.base-url",
    ]);
    // The refused `web` call is never shown as a binding.
    expect(within(card).queryByText("GET /folder")).toBeNull();

    // The registry: both PSS groups, each with its identity, declarers and stand-ins.
    const registry = within(card).getByRole("table", { name: /Named externals/ });
    const registryRows = within(registry)
      .getAllByRole("row")
      .slice(1)
      .map((r) => within(r).getAllByRole("cell").map((c) => c.textContent));
    expect(registryRows).toEqual([
      ["PSS api:pss.yaml", "api, web", "pss-mock", "3"],
      ["PSS web:legacy/pss.yaml", "web", "—", "1"],
    ]);
    expect(within(card).getByText(BOUND_EXTERNAL.headline.summary)).toBeInTheDocument();
  });

  it("the coverage tab states both server headlines in their own card, the bound call still under no provider", async () => {
    stubApi({ providers: [BINDING], coverage: DECLARING });
    mount();
    await userEvent.click(await screen.findByRole("tab", { name: "Cross-service coverage" }));
    const card = screen.getByRole("heading", { name: "Declared contracts and named externals" }).closest("section")!;
    expect(within(card).getByText(DECLARED_CONTRACTS.headline.summary)).toBeInTheDocument();
    expect(within(card).getByText(BOUND_EXTERNAL.headline.summary)).toBeInTheDocument();
    expect(within(card).getByText(COVERAGE_TEXT.externalStaysApart)).toBeInTheDocument();
  });

  it("the coverage tab states the declared headline alone when no member declares a named external", async () => {
    // Identity-only declarations: the server then sends no join at all.
    stubApi({ providers: [BINDING], coverage: { ...EMPTY_COVERAGE, declared_contracts: DECLARED_CONTRACTS } });
    mount();
    await userEvent.click(await screen.findByRole("tab", { name: "Cross-service coverage" }));
    const card = screen.getByRole("heading", { name: "Declared contracts and named externals" }).closest("section")!;
    expect(within(card).getByText(DECLARED_CONTRACTS.headline.summary)).toBeInTheDocument();
    expect(within(card).queryByText(COVERAGE_TEXT.externalStaysApart)).toBeNull();
    expect(within(card).queryByText(BOUND_EXTERNAL.headline.summary)).toBeNull();
  });

  it("over a relation a mock only stands in for, draws the external with no edge, no edge legend row and no empty twin table", async () => {
    stubApi({
      providers: [BINDING],
      coverage: {
        ...EMPTY_COVERAGE,
        declared_contracts: {
          ...DECLARED_CONTRACTS,
          contracts: [],
          externals: [
            { id: "pss-mock:source.yaml", name: "PSS", copies: [], declared_by: [], stand_ins: ["pss-mock"] },
          ],
        },
      },
    });
    mount();
    await waitFor(() => expect(externalNodes()).toEqual(["external:pss-mock:source.yaml=PSS"]));
    expect(declaredEdges()).toEqual([]);
    const legend = screen.getByText("Declared contracts", { selector: "span" }).closest("details")!;
    expect(within(legend).queryByText("Declares a contract (a vendored spec)")).toBeNull();
    expect(within(legend).getByText("Named external — not a member (topics share this hue)")).toBeInTheDocument();

    const card = screen.getByRole("heading", { name: "Declared contracts" }).closest("section")!;
    expect(within(card).queryByRole("table", { name: /accessible twin of the declared layer/i })).toBeNull();
    expect(card).toHaveTextContent("No member on this map declares a contract.");
    const registry = within(card).getByRole("table", { name: /Named externals/ });
    expect(within(registry).getAllByRole("cell").map((c) => c.textContent)).toEqual([
      "PSS pss-mock:source.yaml",
      "—",
      "pss-mock",
      "0",
    ]);
  });

  it("renders no declared class, legend section, node or widget over a workspace that declares nothing", async () => {
    stubApi({ providers: [BINDING] });
    mount();
    await waitFor(() => expect(screen.getByTestId("canvas-edges")).toHaveTextContent("1"));
    expect(declaredEdges()).toEqual([]);
    expect(externalNodes()).toEqual([]);
    expect(screen.queryByText("Declared contracts", { selector: "span" })).toBeNull();
    expect(screen.queryByRole("heading", { name: "Declared contracts" })).toBeNull();
    await userEvent.click(screen.getByRole("tab", { name: "Cross-service coverage" }));
    // FR-UI-29 AC8, kept by CR-203: no declared widget without a vendored spec.
    expect(screen.queryByRole("heading", { name: "Declared contracts and named externals" })).toBeNull();
  });
});

// ── S-613: the Cross-service coverage tab (CR-203 §3.2 D items 4 and 10) ───────

/** The coverage tab's widgets, top to bottom. The per-arm board is hidden (S-612),
 *  and Declared contracts renders only when a member vendors a spec (FR-UI-29
 *  AC8, kept by CR-203). */
const COVERAGE_TAB_WIDGETS = [
  "Resolved cross-service edges",
  "Spec conformance (declared endpoints vs controllers)",
  "Coverage by intake",
  "Declared contracts and named externals",
  "Build dependencies",
];
const WITHOUT_DECLARED = COVERAGE_TAB_WIDGETS.filter((t) => !t.startsWith("Declared"));

/** The four states (S-613 AC1), as what `stubApi` serves. The healthy and
 *  partial states vendor a spec; the empty and degraded ones do not. */
const COVERAGE_TAB_STATES: Record<
  "healthy" | "partial coverage" | "nothing measured" | "degraded member",
  NonNullable<Parameters<typeof stubApi>[0]>
> = {
  healthy: { coverage: { ...HEALTHY_COVERAGE, declared_contracts: DECLARED_CONTRACTS }, buildDependency: BUILD_HEADLINE },
  "partial coverage": {
    coverage: { ...PARTIAL_COVERAGE, declared_contracts: DECLARED_CONTRACTS, bound_external: BOUND_EXTERNAL },
    buildDependency: { ...BUILD_HEADLINE, members: { ...BUILD_HEADLINE.members, read: 1, unread: ["web"] } },
  },
  "nothing measured": { coverage: EMPTY_COVERAGE },
  "degraded member": { coverage: PARTIAL_COVERAGE, degradedRollup: oneDegraded("web") },
};

async function openCoverageTab(opts: Parameters<typeof stubApi>[0]) {
  stubApi({ providers: [BINDING], buildDeps: BUILD_DEPS, ...opts });
  mount();
  await userEvent.click(await screen.findByRole("tab", { name: /cross-service coverage/i }));
  return screen.getByRole("tabpanel");
}

function tabWidget(panel: HTMLElement, title: string): HTMLElement {
  const found = expectOneWidgetStack(panel).filter((w) => widgetTitle(w) === title);
  expect(found, title).toHaveLength(1);
  return found[0];
}

describe("WorkspaceView — the coverage tab explains itself, in one stack (S-613)", () => {
  it("has a state to check for each of the four the story names", () => {
    expect(Object.keys(COVERAGE_TAB_STATES)).toEqual(["healthy", "partial coverage", "nothing measured", "degraded member"]);
  });

  it.each(Object.entries(COVERAGE_TAB_STATES))(
    "%s: every widget is a Widget under the tab's one WidgetStack and carries the message standard",
    async (_state, opts) => {
      const panel = await openCoverageTab(opts);
      const widgets = expectOneWidgetStack(panel);
      expect(widgets.map(widgetTitle)).toEqual(
        opts.coverage?.declared_contracts ? COVERAGE_TAB_WIDGETS : WITHOUT_DECLARED,
      );
      for (const w of widgets) expectWidgetCopy(w);
    },
  );

  it("each widget renders its own catalogue entry for its state", async () => {
    const panel = await openCoverageTab(COVERAGE_TAB_STATES["partial coverage"]);
    expectWidgetCopy(tabWidget(panel, "Spec conformance (declared endpoints vs controllers)"), specConformance, {
      notMatched: 7,
    });
    expectWidgetCopy(tabWidget(panel, "Coverage by intake"), coverageByIntake, { finding: "captured-resolves" });
    expectWidgetCopy(tabWidget(panel, "Declared contracts and named externals"), declaredRelations);
    expectWidgetCopy(tabWidget(panel, "Build dependencies"), buildDependencies, { unread: 1 });
  });

  it("glosses 'intake' in the Coverage by intake title", async () => {
    const panel = await openCoverageTab({ coverage: MULTI_REASON_COVERAGE });
    const title = tabWidget(panel, "Coverage by intake").querySelector('[data-widget-part="title"]')!;
    expect(title.querySelector("dfn")?.getAttribute("data-term")).toBe("intake");
  });

  it("the captured-call finding asks for action only when captured calls resolve nowhere", async () => {
    const panel = await openCoverageTab({ coverage: COVERAGE });
    const intake = tabWidget(panel, "Coverage by intake");
    expectWidgetCopy(intake, coverageByIntake, { finding: "captured-unresolved" });
    expect(intake.querySelector('[data-widget-copy="where"]')?.textContent).toBe("command logos workspace status");
  });
});
