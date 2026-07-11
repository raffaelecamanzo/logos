import { describe, expect, it } from "vitest";

import type { BridgeEdge, MemberTopics, WorkspaceStatus } from "../../api/types.ts";
import {
  buildServiceMap,
  memberOfServiceId,
  serviceId,
  serviceMembers,
  topicId,
  topicLinks,
  topicOfTopicId,
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

// ── S-256 / FR-WS-11: topics as first-class nodes on the service map ─────────

/** One member's topic inventory entry. */
function topics(member: string, ...entries: [string, number, number][]): MemberTopics {
  return {
    member,
    topics: entries.map(([topic, producers, consumers]) => ({ topic, producers, consumers })),
  };
}

describe("topicLinks (S-256, FR-WS-11)", () => {
  it("folds the per-member inventory onto ONE node per shared topic identity", () => {
    // `orders` is published by api and subscribed by billing — the same identity in
    // two members IS the coupling.
    const links = topicLinks([
      topics("api", ["orders", 1, 0]),
      topics("billing", ["orders", 0, 2]),
    ]);
    expect(links).toEqual([{ topic: "orders", producers: ["api"], consumers: ["billing"] }]);
  });

  it("records a member on both sides when it both publishes and subscribes", () => {
    const links = topicLinks([topics("relay", ["orders", 1, 1])]);
    expect(links).toEqual([{ topic: "orders", producers: ["relay"], consumers: ["relay"] }]);
  });

  it("omits a member from a side it has no sites on — a zero is not a producer", () => {
    const links = topicLinks([topics("api", ["orders", 0, 0])]);
    expect(links).toEqual([{ topic: "orders", producers: [], consumers: [] }]);
  });

  it("is deterministic: topics sort by key, members within a side sort by name", () => {
    const links = topicLinks([
      topics("zeta", ["shipments", 1, 0]),
      topics("alpha", ["shipments", 1, 0], ["orders", 1, 0]),
    ]);
    expect(links.map((l) => l.topic)).toEqual(["orders", "shipments"]);
    expect(links[1].producers).toEqual(["alpha", "zeta"]);
  });
});

describe("buildServiceMap with topics (S-256, FR-WS-11)", () => {
  it("draws a topic as its own node, in a namespace that never decodes as a service", () => {
    const map = buildServiceMap(
      [member("api"), member("billing")],
      [],
      [topics("api", ["orders", 1, 0]), topics("billing", ["orders", 0, 1])],
    );

    expect(map.loaded.nodes[topicId("orders")]).toMatchObject({
      label: "orders",
      kind: "topic",
      layer: "artifact",
    });
    // The two namespaces are disjoint — critical, because the canvas selects a MEMBER
    // for any id that decodes as a service. A topic must never do that.
    expect(memberOfServiceId(topicId("orders"))).toBeNull();
    expect(topicOfTopicId(topicId("orders"))).toBe("orders");
    expect(topicOfTopicId(serviceId("api"))).toBeNull();
  });

  it("renders a coupling as publisher → topic → subscriber, not one opaque line", () => {
    const map = buildServiceMap(
      [member("api"), member("billing")],
      [],
      [topics("api", ["orders", 1, 0]), topics("billing", ["orders", 0, 1])],
    );
    expect(map.loaded.edges).toEqual([
      { source: serviceId("api"), target: topicId("orders"), edge_type: "publishes" },
      { source: topicId("orders"), target: serviceId("billing"), edge_type: "subscribes" },
    ]);
  });

  it("ACCEPTANCE: a topic with a publisher and NO subscriber anywhere is still drawn", () => {
    // The per-repo promise (FR-WS-11): this topic has no cross-member binding at all,
    // so a map built from bindings alone would render it as an absence. It is not an
    // absence — it is an unconsumed topic, and the map must say so.
    const map = buildServiceMap([member("api")], [], [topics("api", ["orders", 1, 0])]);

    expect(map.topics).toEqual([{ topic: "orders", producers: ["api"], consumers: [] }]);
    expect(map.loaded.nodes[topicId("orders")]).toBeDefined();
    expect(map.loaded.edges).toEqual([
      { source: serviceId("api"), target: topicId("orders"), edge_type: "publishes" },
    ]);
    expect(map.links).toEqual([]); // no binding — and none is fabricated
  });

  it("draws a broker binding through its topic, never ALSO as a flat service line", () => {
    // The bridge resolves the same coupling as a `broker-topic` binding. Drawing both
    // the topic hops AND the flat line would render one coupling twice — once named,
    // once opaque. The topic hops win; the binding still counts in `links`.
    const map = buildServiceMap(
      [member("api"), member("billing")],
      [binding("api", "billing", "broker-topic")],
      [topics("api", ["orders", 1, 0]), topics("billing", ["orders", 0, 1])],
    );

    expect(map.loaded.edges.map((e) => e.edge_type).sort()).toEqual(["publishes", "subscribes"]);
    expect(map.loaded.edges.some((e) => e.edge_type === "broker-topic")).toBe(false);
    // …but the binding is still reported as a resolved coupling.
    expect(map.links).toEqual([
      { from: "api", to: "billing", relation: "broker-topic", count: 1 },
    ]);
  });

  it("keeps the HTTP/gRPC arms as direct service lines — only the broker arm re-routes", () => {
    const map = buildServiceMap(
      [member("web"), member("api")],
      [binding("web", "api", "route")],
      [],
    );
    expect(map.loaded.edges).toEqual([
      { source: serviceId("web"), target: serviceId("api"), edge_type: "route" },
    ]);
    expect(map.topics).toEqual([]);
  });

  it("never draws a topic edge to a member absent from the roster (NFR-RA-05)", () => {
    // The inventory names a member the roster does not carry (it was removed from the
    // manifest). The topic is still real, but the edge has no service node to land on —
    // and inventing one would fabricate a service.
    const map = buildServiceMap([member("api")], [], [topics("ghost", ["orders", 1, 0])]);
    expect(map.loaded.nodes[topicId("orders")]).toBeDefined();
    expect(map.loaded.nodes[serviceId("ghost")]).toBeUndefined();
    expect(map.loaded.edges).toEqual([]);
  });

  it("a workspace with no topics is byte-identical to the pre-S-256 map", () => {
    const withArg = buildServiceMap([member("api"), member("web")], [binding("api", "web")], []);
    const withoutArg = buildServiceMap([member("api"), member("web")], [binding("api", "web")]);
    expect(withArg.loaded).toEqual(withoutArg.loaded);
    expect(withArg.topics).toEqual([]);
    expect(withoutArg.loaded.edges).toEqual([
      { source: serviceId("api"), target: serviceId("web"), edge_type: "route" },
    ]);
  });
});

describe("buildServiceMap — the broker line is suppressed only when a topic carries it", () => {
  it("KEEPS the flat line when the binding is resolved but the inventory is empty", () => {
    // A member indexed by a pre-S-256 binary (ledger rows, no promoted topic nodes), or
    // one whose topic read degraded and was skipped: the coupling is REAL and resolved,
    // but no topic hop exists to carry it. Dropping the line unconditionally would make
    // a resolved coupling vanish from the canvas while still counting it in the table.
    const map = buildServiceMap(
      [member("api"), member("billing")],
      [binding("api", "billing", "broker-topic")],
      [], // no inventory
    );

    expect(map.loaded.edges).toEqual([
      { source: serviceId("api"), target: serviceId("billing"), edge_type: "broker-topic" },
    ]);
    expect(map.links).toEqual([
      { from: "api", to: "billing", relation: "broker-topic", count: 1 },
    ]);
  });

  it("KEEPS the flat line when a topic exists but does not carry THIS pair", () => {
    // `orders` couples api → billing; the resolved binding is api → shipping (some other
    // topic whose inventory we do not have). The unrelated topic must not suppress it.
    const map = buildServiceMap(
      [member("api"), member("billing"), member("shipping")],
      [binding("api", "shipping", "broker-topic")],
      [topics("api", ["orders", 1, 0]), topics("billing", ["orders", 0, 1])],
    );

    const flat = map.loaded.edges.filter((e) => e.edge_type === "broker-topic");
    expect(flat).toEqual([
      { source: serviceId("api"), target: serviceId("shipping"), edge_type: "broker-topic" },
    ]);
  });

  it("SUPPRESSES the flat line only when the topic hop demonstrably replaces it", () => {
    const map = buildServiceMap(
      [member("api"), member("billing")],
      [binding("api", "billing", "broker-topic")],
      [topics("api", ["orders", 1, 0]), topics("billing", ["orders", 0, 1])],
    );
    expect(map.loaded.edges.some((e) => e.edge_type === "broker-topic")).toBe(false);
    expect(map.loaded.edges.map((e) => e.edge_type).sort()).toEqual(["publishes", "subscribes"]);
  });
});
