import { describe, expect, it } from "vitest";

import type {
  BridgeEdge,
  BuildsAgainst,
  ConfigBoundKey,
  ConfigValueRefusal,
  MemberTopics,
  ValueProvenance,
  WorkspaceStatus,
  XserviceBuildDeps,
} from "../../api/types.ts";
import { BOUND_EXTERNAL, DECLARED_CONTRACTS } from "../../workspace/testFixtures.ts";
import {
  BUILD_EDGE_TYPE,
  buildLayer,
  buildServiceMap,
  CONFIG_REFUSAL_LABEL,
  DECLARED_EDGE_TYPE,
  declaredLayer,
  externalNodeId,
  edgeProvenanceKind,
  hasNonLiteralBinding,
  LINK_PROVENANCE_KINDS,
  LINK_PROVENANCE_LABEL,
  linkEvidence,
  memberOfServiceId,
  serviceId,
  serviceMembers,
  topicId,
  topicLinks,
  topicOfTopicId,
  type ServiceMember,
} from "./serviceMapModel.ts";

function member(name: string, indexed = true): ServiceMember {
  // `warmState` is what the map now classifies on, and the server derives it from
  // exactly these inputs — so the helper mirrors that derivation rather than letting
  // a fixture describe a member the server could never produce.
  return { name, indexed, error: null, warmState: indexed ? "warm" : "deferred" };
}

/** A LITERAL binding — both ends observed at the call site.
 *
 *  `from_value` / `to_value` are stated rather than omitted because S-419 made
 *  them required: the server has emitted them unconditionally since S-410, and a
 *  fixture that leaves them out describes a payload the server cannot produce —
 *  while quietly exercising the `unstated` path in every test that only meant to
 *  say "an ordinary binding". */
function binding(from: string, to: string, relation = "route", symbol = "sym"): BridgeEdge {
  return {
    relation,
    from: { member: from, symbol: `${from}/${symbol}` },
    to: { member: to, symbol: `${to}/${symbol}` },
    from_value: { provenance: "literal" },
    to_value: { provenance: "literal" },
  };
}

/** The whole expected shape of a link every one of whose bindings is literal —
 *  identity, weight, breakdown and retained bindings. Spelled out rather than
 *  matched loosely: `toMatchObject` here would stop noticing a breakdown that
 *  silently drifted away from the count beside it. */
function literalLink(from: string, to: string, relation: string, bindings: BridgeEdge[]) {
  return {
    from,
    to,
    relation,
    count: bindings.length,
    provenance: {
      literal: bindings.length,
      "config-bound": 0,
      "config-unresolved": 0,
      unstated: 0,
    },
    bindings,
  };
}

/** One committed configuration key, with one value proved by one overlay. */
function boundKey(
  key: string,
  value: string,
  profiles: string[] = ["docker"],
  sources: string[] = ["src/main/resources/application-docker.yml"],
): ConfigBoundKey {
  return { key, source: "properties", values: [{ value, profiles, unprofiled: false, sources }] };
}

/** A binding with an explicit provenance on either end. */
function bindingWith(
  from: string,
  to: string,
  ends: { from_value?: ValueProvenance; to_value?: ValueProvenance },
  relation = "route",
  symbol = "sym",
): BridgeEdge {
  return {
    ...binding(from, to, relation, symbol),
    ...ends,
  } as BridgeEdge;
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
    expect(map.links).toEqual([
      literalLink("api", "web", "route", [
        binding("api", "web", "route", "a"),
        binding("api", "web", "route", "b"),
      ]),
    ]);
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
    const broken: ServiceMember = {
      name: "web",
      indexed: false,
      error: "store is locked",
      warmState: "degraded",
    };
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
        { member: "api", result: { indexed: true }, warm_state: "warm", open_state: "opened" },
        {
          member: "web",
          error: "engine failed to start",
          warm_state: "degraded",
          open_state: "degraded",
          degraded_reason: "engine failed to start",
          degraded_diagnostic: "engine failed to start",
        },
      ],
      warm_rollup: { members: 2, warm: 1, deferred: 0, degraded: 1 },
      degraded_rollup: {
        members: 2,
        opened: 1,
        not_attempted: 0,
        degraded_members: ["web"],
        covers_all_members: false,
      },
      // `spec_conformance_ratio` OMITTED, as the server omits it when nothing was measured
      // (S-326): a fixture carrying `1` here would model a payload the server can
      // no longer emit, and the `as unknown as` cast means `tsc` would not notice.
      coverage: {
        references: [],
        bound: 0,
        ambiguous: 0,
        unbound: 0,
        no_provider_in_workspace: 0,
        members_read: 1,
        members_total: 2,
        covers_all_members: false,
      },
    } as unknown as WorkspaceStatus;
    // The server's `warm_state` is carried through verbatim — the projection reads
    // the label, it does not re-derive it (S-323, FR-WS-15).
    expect(serviceMembers(status)).toEqual([
      { name: "api", indexed: true, error: null, warmState: "warm" },
      { name: "web", indexed: false, error: "engine failed to start", warmState: "degraded" },
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
      literalLink("api", "billing", "broker-topic", [binding("api", "billing", "broker-topic")]),
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
      literalLink("api", "billing", "broker-topic", [binding("api", "billing", "broker-topic")]),
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

describe("edge provenance (S-419, CR-132, ADR-64, FR-WS-19 AC6)", () => {
  const ADMITTED: ValueProvenance = {
    provenance: "config-bound",
    bound: [boundKey("spring.kafka.topics.archivecommands", "archive-commands")],
  };
  const REFUSED: ValueProvenance = {
    provenance: "config-unresolved",
    keys: ["orders.topic"],
    refusal: "missing-key",
  };

  it("counts one binding under the WEAKER of its two ends, never the better one", () => {
    // The whole defect: an edge with one observed end and one admitted end is an
    // ADMITTED edge. Reading the literal end would restore the indistinguishability.
    expect(edgeProvenanceKind(binding("api", "web"))).toBe("literal");
    expect(
      edgeProvenanceKind(bindingWith("api", "web", { to_value: ADMITTED })),
    ).toBe("config-bound");
    expect(
      edgeProvenanceKind(bindingWith("api", "web", { from_value: ADMITTED })),
    ).toBe("config-bound");
    // A refusal outranks an admission: the coupling rests on a key nothing proved.
    expect(
      edgeProvenanceKind(bindingWith("api", "web", { from_value: ADMITTED, to_value: REFUSED })),
    ).toBe("config-unresolved");
  });

  it("counts an ABSENT value as `unstated`, never as `literal` (CR-132 AC2)", () => {
    // The type says required; the wire is not runtime-validated, so this shape
    // reaches the model in practice — from an older server, or a proxy that
    // dropped a field. Defaulting it to `literal` would report an unread field as
    // observed evidence.
    const missingFrom = { ...binding("api", "web") } as Partial<BridgeEdge>;
    delete missingFrom.from_value;
    expect(edgeProvenanceKind(missingFrom as BridgeEdge)).toBe("unstated");

    // …and an end whose token this build does not know is `unstated` too, not
    // silently the best case.
    const unknown = bindingWith("api", "web", {
      to_value: { provenance: "some-later-arm" } as unknown as ValueProvenance,
    });
    expect(edgeProvenanceKind(unknown)).toBe("unstated");
  });

  it("states a mixed link as a BREAKDOWN, never as one kind (CR-132 AC1)", () => {
    const map = buildServiceMap(
      [member("api"), member("web")],
      [
        binding("api", "web", "route", "a"),
        bindingWith("api", "web", { to_value: ADMITTED }, "route", "b"),
      ],
    );
    expect(map.links).toHaveLength(1);
    expect(map.links[0].count).toBe(2);
    expect(map.links[0].provenance).toEqual({
      literal: 1,
      "config-bound": 1,
      "config-unresolved": 0,
      unstated: 0,
    });
    // The four kinds always sum to the weight, so neither can drift from the other.
    const p = map.links[0].provenance;
    expect(p.literal + p["config-bound"] + p["config-unresolved"] + p.unstated).toBe(
      map.links[0].count,
    );
    expect(hasNonLiteralBinding(map.links[0])).toBe(true);
  });

  it("marks the flat canvas line as admitted, keeping `edge_type` the relation arm", () => {
    const map = buildServiceMap(
      [member("api"), member("web")],
      [bindingWith("api", "web", { from_value: ADMITTED })],
    );
    expect(map.loaded.edges).toEqual([
      { source: "service:api", target: "service:web", edge_type: "route", admitted: true },
    ]);
  });

  it("leaves a literal-only line's canvas edge object untouched (CR-132 AC6)", () => {
    // Not `admitted: false` — an ABSENT slot, so the object is byte-identical to
    // the one the pre-S-419 model produced.
    const map = buildServiceMap([member("api"), member("web")], [binding("api", "web")]);
    expect(map.loaded.edges).toEqual([
      { source: "service:api", target: "service:web", edge_type: "route" },
    ]);
    expect("admitted" in map.loaded.edges[0]).toBe(false);
    expect(hasNonLiteralBinding(map.links[0])).toBe(false);
  });

  it("carries the marker onto the TOPIC HOPS that replace a flat broker line (AC5)", () => {
    const map = buildServiceMap(
      [member("api"), member("billing")],
      [bindingWith("api", "billing", { from_value: ADMITTED }, "broker-topic")],
      [topics("api", ["orders", 1, 0]), topics("billing", ["orders", 0, 1])],
    );
    // The flat line is suppressed — the hops are what is drawn…
    expect(map.loaded.edges.map((e) => e.edge_type).sort()).toEqual(["publishes", "subscribes"]);
    // …so both of them must carry the provenance the flat line would have had.
    expect(map.loaded.edges.every((e) => e.admitted === true)).toBe(true);
  });

  it("does NOT mark a topic hop whose backing binding was observed in code", () => {
    const map = buildServiceMap(
      [member("api"), member("billing")],
      [binding("api", "billing", "broker-topic")],
      [topics("api", ["orders", 1, 0]), topics("billing", ["orders", 0, 1])],
    );
    expect(map.loaded.edges.some((e) => "admitted" in e)).toBe(false);
  });

  it("does NOT mark a topic hop from a broker binding pointing the OTHER way", () => {
    // The pair key is ORDERED. `billing → api` is a different coupling from
    // `api → billing`, and marking the api→orders→billing hops from it would
    // attribute one coupling's provenance to another — the near miss a
    // direction-blind membership test admits.
    const map = buildServiceMap(
      [member("api"), member("billing")],
      [bindingWith("billing", "api", { from_value: ADMITTED }, "broker-topic")],
      [topics("api", ["orders", 1, 0]), topics("billing", ["orders", 0, 1])],
    );
    expect(map.loaded.edges.map((e) => e.edge_type).sort()).toEqual([
      "broker-topic",
      "publishes",
      "subscribes",
    ]);
    // The hops carry no marker; only the flat billing → api line it really is.
    const hops = map.loaded.edges.filter((e) => e.edge_type !== "broker-topic");
    expect(hops.some((e) => "admitted" in e)).toBe(false);
  });

  it("keeps the kind roster and the label map in step", () => {
    // Three declarations name the four kinds (the roster, the label map, the
    // zeroed breakdown). The compiler catches a MISSING one; nothing catches a
    // roster that has drifted out of order or dropped an entry, and a kind
    // missing from the roster renders as nothing at all in the table cell.
    expect([...LINK_PROVENANCE_KINDS].sort()).toEqual(
      Object.keys(LINK_PROVENANCE_LABEL).sort(),
    );
    const map = buildServiceMap([member("api"), member("web")], [binding("api", "web")]);
    expect(Object.keys(map.links[0].provenance).sort()).toEqual([...LINK_PROVENANCE_KINDS].sort());
  });

  it("carries EVERY refusal token through to its own label, and never confuses two", () => {
    // Review finding: only `missing-key` was exercised, so the other two labels
    // could be replaced with garbage and the suite stayed green — which is how
    // `uncommitted` came to carry `missing-key`'s wording. The three refusals
    // are different remedies: `uncommitted` means the value arrives from
    // something the repository does not commit (nothing to go and define);
    // `missing-key` means the committed sources prove no value (go and define
    // it). Labelling one as the other sends an operator the wrong way
    // (NFR-CC-04), so every token is pinned, and pinned against the wording
    // `ValueRefusal::label()` in logos-core/src/resolve/binding.rs authors.
    const refusals: ConfigValueRefusal[] = ["uncommitted", "placeholder-value", "missing-key"];
    for (const refusal of refusals) {
      const map = buildServiceMap(
        [member("api"), member("web")],
        [
          bindingWith("api", "web", {
            from_value: { provenance: "config-unresolved", keys: ["k"], refusal },
          }),
        ],
      );
      expect(linkEvidence(map.links[0])[0].refusal).toBe(refusal);
    }
    expect(CONFIG_REFUSAL_LABEL).toEqual({
      uncommitted: "Not committed by the repository",
      "placeholder-value": "The committed value is itself a placeholder",
      "missing-key": "No committed source defines it",
    });
    // The label map covers the union exactly — a token added server-side with no
    // label here would render as `undefined` in the table cell.
    expect(Object.keys(CONFIG_REFUSAL_LABEL).sort()).toEqual([...refusals].sort());
  });

  it("lists one evidence row PER OVERLAY, per end, naming key, sources and profiles", () => {
    const twoOverlays: ValueProvenance = {
      provenance: "config-bound",
      bound: [
        {
          key: "orders.topic",
          source: "properties",
          values: [
            { value: "orders-v1", profiles: ["docker"], unprofiled: false, sources: ["a.yml"] },
            { value: "orders-v2", profiles: ["k8s"], unprofiled: true, sources: ["b.yml"] },
          ],
        },
      ],
    };
    const map = buildServiceMap(
      [member("api"), member("web")],
      [bindingWith("api", "web", { from_value: twoOverlays, to_value: REFUSED })],
    );
    expect(linkEvidence(map.links[0])).toEqual([
      {
        end: "consumer",
        member: "api",
        key: "orders.topic",
        value: "orders-v1",
        profiles: ["docker"],
        unprofiled: false,
        sources: ["a.yml"],
        refusal: null,
      },
      {
        end: "consumer",
        member: "api",
        key: "orders.topic",
        value: "orders-v2",
        profiles: ["k8s"],
        unprofiled: true,
        sources: ["b.yml"],
        refusal: null,
      },
      {
        end: "provider",
        member: "web",
        key: "orders.topic",
        value: null,
        profiles: [],
        unprofiled: false,
        sources: [],
        refusal: "missing-key",
      },
    ]);
  });

  it("yields every key's evidence when one target names several (never only the first)", () => {
    const twoKeys: ValueProvenance = {
      provenance: "config-bound",
      bound: [boundKey("svc.host", "http://billing"), boundKey("svc.path", "/orders")],
    };
    const map = buildServiceMap(
      [member("api"), member("web")],
      [bindingWith("api", "web", { from_value: twoKeys })],
    );
    expect(linkEvidence(map.links[0]).map((r) => r.key)).toEqual(["svc.host", "svc.path"]);
  });

  it("treats a config-bound end with an EMPTY bound list as admitted, but evidences nothing", () => {
    // A malformed/empty `bound: []` is `config-bound` on the wire, so the link
    // is correctly NOT literal — but it names no key, so there is no evidence
    // to show. The two facts must not be conflated: `coverageModel`'s
    // `provenanceLabel` has an explicit `keys.length === 0` branch for the same
    // payload, and the view's empty-evidence wording is worded not to claim the
    // provenance was unstated (it was stated; it was just empty).
    const map = buildServiceMap(
      [member("api"), member("web")],
      [bindingWith("api", "web", { from_value: { provenance: "config-bound", bound: [] } })],
    );
    expect(map.links[0].provenance["config-bound"]).toBe(1);
    expect(hasNonLiteralBinding(map.links[0])).toBe(true);
    expect(linkEvidence(map.links[0])).toEqual([]);
  });

  it("yields one evidence row PER KEY on a config-unresolved end naming several", () => {
    // AC3 says "its keys" — plural. Only one key was ever proven, so the
    // per-key `.map` in the refusal branch was untested at plurality, unlike
    // its config-bound sibling. Each key carries the SHARED refusal.
    const map = buildServiceMap(
      [member("api"), member("web")],
      [
        bindingWith("api", "web", {
          from_value: {
            provenance: "config-unresolved",
            keys: ["svc.host", "svc.path"],
            refusal: "placeholder-value",
          },
        }),
      ],
    );
    expect(linkEvidence(map.links[0])).toEqual([
      {
        end: "consumer",
        member: "api",
        key: "svc.host",
        value: null,
        profiles: [],
        unprofiled: false,
        sources: [],
        refusal: "placeholder-value",
      },
      {
        end: "consumer",
        member: "api",
        key: "svc.path",
        value: null,
        profiles: [],
        unprofiled: false,
        sources: [],
        refusal: "placeholder-value",
      },
    ]);
  });

  it("states ALL FOUR kinds on one link when its bindings span every one", () => {
    // The breakdown was only ever proven across TWO kinds, so a renderer or an
    // accumulator that handled the first two and dropped the rest would pass.
    // AC1's claim is that the line is never reduced to one label — this pins it
    // at full width, and pins the sum against the weight.
    const map = buildServiceMap(
      [member("api"), member("web")],
      [
        binding("api", "web", "route", "a"),
        bindingWith("api", "web", { to_value: ADMITTED }, "route", "b"),
        bindingWith("api", "web", { to_value: REFUSED }, "route", "c"),
        (() => {
          const e = { ...binding("api", "web", "route", "d") } as Partial<BridgeEdge>;
          delete e.to_value;
          return e as BridgeEdge;
        })(),
      ],
    );
    expect(map.links[0].provenance).toEqual({
      literal: 1,
      "config-bound": 1,
      "config-unresolved": 1,
      unstated: 1,
    });
    expect(map.links[0].count).toBe(4);
  });

  it("yields NO evidence rows for a literal or unstated end — absence is not a claim", () => {
    const map = buildServiceMap([member("api"), member("web")], [binding("api", "web")]);
    expect(linkEvidence(map.links[0])).toEqual([]);
  });
});

// ── The build layer (S-464, FR-WS-33) ───────────────────────────────────────

function row(from: string, to: string, kind: BuildsAgainst["kind"], artifact: string, extra: Partial<BuildsAgainst> = {}): BuildsAgainst {
  return { from, to, kind, scope: null, artifact, references: 1, ...extra };
}

/** Every row sits in its `from`'s `builds_against` AND its `to`'s
 *  `built_against_by`, exactly as the server sends it. */
function deps(rows: BuildsAgainst[], platforms: string[] = []): XserviceBuildDeps {
  const names = [...new Set(rows.flatMap((r) => [r.from, r.to]))];
  return {
    headline: {
      build_dependency_pairs: { pairs: 0, parent: 0, dependency: 0, managed: 0, "bom-import": 0 },
      references: { references: 0, to_member: 0, external: 0 },
      members: { members: names.length, read: names.length, with_manifests: 0, manifests: 0, manifests_read: 0 },
      ...(platforms.length
        ? {
            platform_apart: {
              members: platforms,
              build_dependency_pairs: { pairs: 0, parent: 0, dependency: 0, managed: 0, "bom-import": 0 },
              summary: "",
            },
          }
        : {}),
      collisions: [],
      platform_candidates: [],
      summary: "",
    },
    members: names.map((m) => ({
      member: m,
      builds_against: rows.filter((r) => r.from === m),
      built_against_by: rows.filter((r) => r.to === m),
    })),
    cross_context: [],
  };
}

describe("buildLayer (S-464)", () => {
  it("draws one `build` edge per member pair, reading each row once, kinds in order", () => {
    const layer = buildLayer(
      deps([
        row("api", "lib", "managed", "g:lib"),
        row("api", "lib", "dependency", "g:lib", { references: 3 }),
        row("api", "lib", "dependency", "g:lib-extra"),
        row("web", "lib", "dependency", "g:lib"),
      ]),
      [member("api"), member("lib"), member("web")],
    );
    expect(layer.edges).toEqual([
      { source: serviceId("api"), target: serviceId("lib"), edge_type: BUILD_EDGE_TYPE },
      { source: serviceId("web"), target: serviceId("lib"), edge_type: BUILD_EDGE_TYPE },
    ]);
    expect(layer.links[0]).toEqual({
      from: "api",
      to: "lib",
      kinds: ["dependency", "managed"],
      artifacts: ["g:lib", "g:lib-extra"],
      references: 5,
    });
    expect(BUILD_EDGE_TYPE).toBe("build");
  });

  it("collapses a declared platform: its inbound rows are not drawn, their members counted", () => {
    const layer = buildLayer(
      deps(
        [
          row("api", "starter", "parent", "g:starter", { platform: true }),
          row("web", "starter", "parent", "g:starter", { platform: true }),
          row("web", "starter", "managed", "g:starter", { platform: true }),
          // OUT of the platform is an ordinary edge.
          row("starter", "lib", "managed", "g:lib"),
        ],
        ["starter"],
      ),
      [member("api"), member("web"), member("starter"), member("lib")],
    );
    expect(layer.links.map((l) => `${l.from}->${l.to}`)).toEqual(["starter->lib"]);
    expect(layer.collapsed).toEqual([{ member: "starter", inbound: 2 }]);
  });

  it("collapses on the `platform` flag, listing a declared platform with no inbound row at zero", () => {
    const layer = buildLayer(deps([row("api", "lib", "dependency", "g:lib")], ["mock"]), [
      member("api"),
      member("lib"),
    ]);
    expect(layer.links).toHaveLength(1);
    expect(layer.collapsed).toEqual([{ member: "mock", inbound: 0 }]);
  });

  it("drops a row whose end is not a roster service — never a fabricated node", () => {
    const layer = buildLayer(
      deps([row("api", "ghost", "dependency", "g:ghost"), row("api", "api", "dependency", "g:self")]),
      [member("api")],
    );
    expect(layer.edges).toEqual([]);
    expect(layer.links).toEqual([]);
  });
});

// ── The declared layer (S-461, FR-WS-31, BR-57) ─────────────────────────────

describe("declaredLayer (S-461)", () => {
  const roster = [member("api"), member("web")];

  it("is null when the status carries neither relation — the map is then the pre-S-461 map", () => {
    expect(declaredLayer(undefined, undefined, roster)).toBeNull();
  });

  it("keys an external node by IDENTITY and labels it by name, so two externals titled PSS are two nodes", () => {
    const layer = declaredLayer(DECLARED_CONTRACTS, BOUND_EXTERNAL, roster)!;
    expect(Object.values(layer.nodes).map((n) => [n.id, n.label, n.kind])).toEqual([
      ["external:api:pss.yaml", "PSS", "external"],
      ["external:web:legacy/pss.yaml", "PSS", "external"],
    ]);
    // Never in the service namespace: clicking one must select no member.
    expect(memberOfServiceId(externalNodeId("api:pss.yaml"))).toBeNull();
  });

  it("draws one declares-contract edge per (holder, counterparty) — two PSS copies from one holder would be one", () => {
    const layer = declaredLayer(DECLARED_CONTRACTS, BOUND_EXTERNAL, roster)!;
    expect(layer.edges.map((e) => `${e.source}->${e.target}:${e.edge_type}`)).toEqual([
      "service:api->external:api:pss.yaml:declares-contract",
      "service:api->service:web:declares-contract",
      "service:web->external:api:pss.yaml:declares-contract",
      "service:web->external:web:legacy/pss.yaml:declares-contract",
    ]);
    expect(layer.edges.every((e) => e.edge_type === DECLARED_EDGE_TYPE && !e.admitted)).toBe(true);

    const twice = {
      ...DECLARED_CONTRACTS,
      contracts: [
        ...DECLARED_CONTRACTS.contracts,
        { ...DECLARED_CONTRACTS.contracts[0], document: "second-pss.yaml" },
      ],
    };
    const merged = declaredLayer(twice, undefined, roster)!;
    expect(merged.edges).toHaveLength(4);
    const apiPss = merged.links.find((l) => l.from === "api" && l.to.kind === "external" && l.to.external === "api:pss.yaml")!;
    expect(apiPss.contracts.map((c) => c.document)).toEqual(["pss.yaml", "second-pss.yaml"]);
  });

  it("attaches each BOUND call to its member's link to that external, with the operation and base-path evidence; a refused row attaches nowhere", () => {
    const layer = declaredLayer(DECLARED_CONTRACTS, BOUND_EXTERNAL, roster)!;
    const withCalls = layer.links.filter((l) => l.bound.length > 0);
    expect(withCalls.map((l) => [l.from, l.to])).toEqual([
      ["api", { kind: "external", external: "api:pss.yaml", name: "PSS" }],
    ]);
    // The server's bound row itself, carried whole.
    expect(withCalls[0].bound).toEqual([BOUND_EXTERNAL.rows[0]]);
    expect(withCalls[0].bound).toEqual([
      {
        from: { member: "api", symbol: "local fetch_mailbox" },
        state: "bound-external",
        external: "api:pss.yaml",
        name: "PSS",
        target: "GET ${pss.uri-get-mailbox}",
        document: "pss.yaml",
        operation: "GET /prov/domain/{}/user/{}",
        base: {
          path: "/prov",
          origin: "deploy-overlay",
          sources: [{ file: "deploy-coll/values.yaml", key: "envfrom.pssbaseurl" }],
        },
      },
    ]);
    // `web`'s refused call is judged and reported by the server, never drawn:
    // `web` declares the same external and still carries no bound call.
    const webPss = layer.links.find((l) => l.from === "web" && l.to.kind === "external" && l.to.external === "api:pss.yaml")!;
    expect(webPss.bound).toEqual([]);
  });

  it("draws a node for an external only a mock stands in for — with no edge, since nothing declares it", () => {
    const standInOnly = {
      ...DECLARED_CONTRACTS,
      contracts: [],
      externals: [{ id: "pss-mock:source.yaml", name: "PSS", copies: [], declared_by: [], stand_ins: ["pss-mock"] }],
    };
    const layer = declaredLayer(standInOnly, undefined, roster)!;
    expect(Object.keys(layer.nodes)).toEqual(["external:pss-mock:source.yaml"]);
    // The HUE channel is the non-member one, never the service one.
    expect(layer.nodes["external:pss-mock:source.yaml"].layer).toBe("artifact");
    expect(layer.edges).toEqual([]);
    expect(layer.links).toEqual([]);
  });

  it("drops a contract whose holder or member target is not a roster service, and a self-identity", () => {
    const stray = {
      ...DECLARED_CONTRACTS,
      contracts: [
        { ...DECLARED_CONTRACTS.contracts[1], holder: "ghost" },
        { ...DECLARED_CONTRACTS.contracts[1], target: { ...DECLARED_CONTRACTS.contracts[1].target, member: "ghost" } },
        { ...DECLARED_CONTRACTS.contracts[1], holder: "web" },
      ],
    } as typeof DECLARED_CONTRACTS;
    const layer = declaredLayer(stray, undefined, roster)!;
    expect(layer.links).toEqual([]);
    expect(layer.edges).toEqual([]);
  });
});
