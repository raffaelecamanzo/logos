import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { BridgeEdge, CrossServiceCoverage } from "../../api/types.ts";
import { WorkspaceProvider } from "../../workspace/WorkspaceContext.tsx";
import { scopedMember, setScopedMember } from "../../workspace/scope.ts";
import { WorkspaceView } from "./WorkspaceView.tsx";

// The service map mounts the real ECharts canvas, which needs a layout engine jsdom
// does not have. Stub the canvas down to what the view actually contracts with: the
// node set it was handed, and the click that focuses a member.
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

const COVERAGE_WITH_REFS: CrossServiceCoverage = {
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
      relation: "grpc-call",
      from: { member: "api", symbol: "c" },
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

const EMPTY_COVERAGE: CrossServiceCoverage = {
  references: [],
  bound: 0,
  ambiguous: 0,
  unbound: 0,
  no_provider_in_workspace: 0,
  bound_ratio: 1,
};

const BINDING: BridgeEdge = {
  relation: "route",
  from: { member: "api", symbol: "op" },
  to: { member: "web", symbol: "route" },
};

function stubWorkspace({
  coverage = EMPTY_COVERAGE,
  providers = [],
  probeStatus = 200,
}: { coverage?: CrossServiceCoverage; providers?: BridgeEdge[]; probeStatus?: number } = {}) {
  vi.stubGlobal(
    "fetch",
    vi.fn((url: string) => {
      if (url.startsWith("/api/v1/workspace/status")) {
        return Promise.resolve({
          ok: probeStatus === 200,
          status: probeStatus,
          json: () =>
            Promise.resolve({
              workspace: "shop",
              members: [
                { member: "api", result: { indexed: true } },
                { member: "web", result: { indexed: true } },
              ],
              coverage,
            }),
        } as Response);
      }
      if (url.startsWith("/api/v1/workspace/route-providers")) {
        return Promise.resolve({
          ok: true,
          status: 200,
          json: () => Promise.resolve({ providers }),
        } as Response);
      }
      return Promise.resolve({ ok: true, status: 200, json: () => Promise.resolve({}) } as Response);
    }),
  );
}

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
  it("says so honestly in single-root mode instead of fetching a surface that is not there", async () => {
    stubWorkspace({ probeStatus: 404 });
    mount();
    expect(await screen.findByText(/not a workspace/i)).toBeInTheDocument();
  });

  it("rolls the workspace up: its name, its services, and the coverage headline", async () => {
    stubWorkspace({ coverage: COVERAGE_WITH_REFS, providers: [BINDING] });
    mount();
    expect(await screen.findByText("shop")).toBeInTheDocument();
    // The roll-up is one line: service count AND the coverage headline together (the
    // same counts also appear inside the coverage panel, so assert on this element).
    const rollup = screen.getByText(/2 services/);
    expect(rollup.textContent).toMatch(/1 bound · 1 ambiguous · 1 unbound/);
  });

  it("draws services as nodes and resolved bindings as edges on the shared canvas", async () => {
    stubWorkspace({ providers: [BINDING] });
    mount();
    await waitFor(() => expect(screen.getByTestId("canvas")).toBeInTheDocument());
    expect(screen.getByTestId("canvas-edges")).toHaveTextContent("1");
    // The accessible twin of the canvas names the binding in full.
    expect(screen.getByRole("cell", { name: "api" })).toBeInTheDocument();
    expect(screen.getByText(/HTTP \(OpenAPI ↔ route\)/)).toBeInTheDocument();
  });

  it("clicking a service focuses its member — the shell selector follows the canvas", async () => {
    stubWorkspace({ providers: [BINDING] });
    mount();
    await waitFor(() => expect(screen.getByTestId("canvas")).toBeInTheDocument());
    await userEvent.click(screen.getByRole("button", { name: "web" }));
    expect(scopedMember()).toBe("web");
  });

  it("states an empty service map honestly rather than drawing a fabricated edge", async () => {
    stubWorkspace({ providers: [] });
    mount();
    expect(await screen.findByText(/no cross-service bindings resolved yet/i)).toBeInTheDocument();
    expect(screen.getByTestId("canvas-edges")).toHaveTextContent("0");
  });

  it("shows bound/ambiguous/unbound per arm with the reasons, and the server's ratio verbatim", async () => {
    stubWorkspace({ coverage: COVERAGE_WITH_REFS, providers: [BINDING] });
    mount();
    await userEvent.click(await screen.findByRole("tab", { name: /cross-service coverage/i }));

    // The ratio is the server's (33.3%), NOT bound/total (1/3 of 5 references = 20%):
    // `no-provider-in-workspace` is excluded from the denominator (ADR-53).
    expect(screen.getAllByText("33.3%").length).toBeGreaterThan(0);
    expect(screen.getByText(/2 with no provider in this workspace/)).toBeInTheDocument();
    // One row per relation arm, with the unbound reason spelled out.
    expect(screen.getByRole("cell", { name: /HTTP \(OpenAPI ↔ route\)/ })).toBeInTheDocument();
    expect(screen.getByText(/Path could not be composed/)).toBeInTheDocument();
  });

  it("renders the coverage empty state — never a fabricated 100% over nothing", async () => {
    stubWorkspace({ coverage: EMPTY_COVERAGE });
    mount();
    await userEvent.click(await screen.findByRole("tab", { name: /cross-service coverage/i }));
    expect(screen.getByText(/no cross-boundary references found/i)).toBeInTheDocument();
  });
});
