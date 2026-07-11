import { describe, expect, it } from "vitest";

import type { BridgeEdge, WorkspaceStatus } from "../../api/types.ts";
import {
  buildServiceMap,
  memberOfServiceId,
  serviceId,
  serviceMembers,
  type ServiceMember,
} from "./serviceMapModel.ts";

function member(name: string, indexed = true): ServiceMember {
  return { name, indexed, error: null };
}

function binding(from: string, to: string, relation = "route", symbol = "sym"): BridgeEdge {
  return {
    relation,
    from: { member: from, symbol: `${from}/${symbol}` },
    to: { member: to, symbol: `${to}/${symbol}` },
  };
}

describe("buildServiceMap (S-250, FR-UI-29)", () => {
  it("renders every member as a canvas node, in its own id namespace", () => {
    const map = buildServiceMap([member("api"), member("web")], []);
    expect(Object.keys(map.loaded.nodes).sort()).toEqual(["service:api", "service:web"]);
    expect(map.loaded.nodes[serviceId("api")]).toMatchObject({ label: "api", kind: "service" });
    // The namespace round-trips, so a canvas click resolves back to the member.
    expect(memberOfServiceId(serviceId("web"))).toBe("web");
    expect(memberOfServiceId("logos . . . `lib.rs`/f().")).toBeNull();
  });

  it("collapses every binding between two services under one arm into ONE weighted edge", () => {
    const map = buildServiceMap(
      [member("api"), member("web")],
      [binding("api", "web", "route", "a"), binding("api", "web", "route", "b")],
    );
    expect(map.links).toEqual([{ from: "api", to: "web", relation: "route", count: 2 }]);
    expect(map.loaded.edges).toEqual([
      { source: "service:api", target: "service:web", edge_type: "route" },
    ]);
  });

  it("keeps distinct relation arms as distinct edges (the legend colours them apart)", () => {
    const map = buildServiceMap(
      [member("api"), member("web")],
      [binding("api", "web", "route"), binding("api", "web", "grpc-call")],
    );
    expect(map.links.map((l) => l.relation)).toEqual(["grpc-call", "route"]);
    expect(map.loaded.edges).toHaveLength(2);
  });

  it("draws NO edge for a workspace with no resolved bindings — sparsity is reported, never faked", () => {
    const map = buildServiceMap([member("api"), member("web")], []);
    expect(map.loaded.edges).toEqual([]);
    expect(map.links).toEqual([]);
    // The services themselves still exist — an empty map is not an empty workspace.
    expect(Object.keys(map.loaded.nodes)).toHaveLength(2);
  });

  it("marks an un-indexed member awaiting-index and mutes it, rather than showing it uncoupled", () => {
    const map = buildServiceMap([member("api"), member("web", false)], []);
    expect(map.awaitingIndex).toEqual(["web"]);
    expect(map.loaded.nodes[serviceId("web")].layer).toBe("doc");
    expect(map.loaded.nodes[serviceId("api")].layer).toBe("code");
  });

  it("keeps a DEGRADED member apart from a merely un-indexed one — different facts, different remedies", () => {
    const broken: ServiceMember = { name: "web", indexed: false, error: "store is locked" };
    const map = buildServiceMap([member("api"), broken], []);
    expect(map.degraded).toEqual(["web"]);
    // It is NOT reported as "awaiting index" — that would send the user to `logos
    // index` for a fault indexing cannot fix.
    expect(map.awaitingIndex).toEqual([]);
    expect(map.loaded.nodes[serviceId("web")].layer).toBe("doc");
  });

  it("projects the status fan-out onto the roster, degradation and all", () => {
    const status = {
      workspace: "shop",
      members: [
        { member: "api", result: { indexed: true } },
        { member: "web", error: "engine failed to start" },
      ],
      coverage: { references: [], bound: 0, ambiguous: 0, unbound: 0, no_provider_in_workspace: 0, bound_ratio: 1 },
    } as unknown as WorkspaceStatus;
    expect(serviceMembers(status)).toEqual([
      { name: "api", indexed: true, error: null },
      { name: "web", indexed: false, error: "engine failed to start" },
    ]);
  });

  it("never fabricates a service for a binding endpoint outside the roster", () => {
    const map = buildServiceMap([member("api")], [binding("api", "ghost")]);
    expect(Object.keys(map.loaded.nodes)).toEqual([serviceId("api")]);
    expect(map.loaded.edges).toEqual([]);
  });

  it("drops a self-binding — it is not a CROSS-service edge", () => {
    const map = buildServiceMap([member("api")], [binding("api", "api")]);
    expect(map.loaded.edges).toEqual([]);
  });

  it("is deterministic: links sort by (from, to, relation)", () => {
    const map = buildServiceMap(
      [member("a"), member("b"), member("c")],
      [binding("c", "a"), binding("a", "b"), binding("a", "c", "grpc-call")],
    );
    expect(map.links.map((l) => `${l.from}->${l.to}:${l.relation}`)).toEqual([
      "a->b:route",
      "a->c:grpc-call",
      "c->a:route",
    ]);
  });
});
