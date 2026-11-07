/*
 * The declared-topic layer, as a pure projection (S-491, FR-WS-36): which
 * declarations become a `declares-topic` edge, and the canvas rules that keep
 * the class apart from a measured coupling. The rendered map is asserted in
 * `WorkspaceView.declaredTopics.test.tsx`; this file pins the model behind it.
 */
import { describe, expect, it } from "vitest";

import type { MemberTopics, TopicDeclaration, XserviceDeclaredTopics } from "../../api/types.ts";
import { NO_DECLARED_TOPICS } from "../../workspace/testFixtures.ts";
import { degreeMap } from "../graph/graphModel.ts";
import { buildSeries } from "../graph/graphOptions.ts";
import {
  buildServiceMap,
  DECLARED_TOPIC_EDGE_TYPE,
  declaredTopicLayer,
  serviceId,
  topicId,
  type ServiceMember,
} from "./serviceMapModel.ts";

const warm = (name: string): ServiceMember => ({ name, warmState: "warm", indexed: true, error: null });
const MEMBERS = [warm("api"), warm("web"), warm("audit")];
const INVENTORY: MemberTopics[] = [
  { member: "api", topics: [{ topic: "orders", producers: 1, consumers: 0 }] },
  { member: "web", topics: [{ topic: "orders", producers: 0, consumers: 1 }] },
];
const MAP = buildServiceMap(MEMBERS, [], INVENTORY);

function decl(over: Partial<TopicDeclaration>): TopicDeclaration {
  return {
    member: "audit",
    topic: "orders",
    provenance: "config-declared",
    key: "spring.kafka.topics.orders",
    file: "src/main/resources/application.yml",
    profile: null,
    shape: "spring-kafka-topics",
    status: "net-new",
    site: { site: "config-only" },
    ...over,
  };
}

function answer(declarations: TopicDeclaration[], considered = declarations.length): XserviceDeclaredTopics {
  const byTopic = new Map<string, TopicDeclaration[]>();
  for (const d of declarations) byTopic.set(d.topic, [...(byTopic.get(d.topic) ?? []), d]);
  return {
    ...NO_DECLARED_TOPICS,
    headline: { ...NO_DECLARED_TOPICS.headline, declarations_considered: considered },
    topics: [...byTopic].map(([topic, ds]) => ({ topic, declarations: ds })),
  };
}

describe("declaredTopicLayer (S-491)", () => {
  it("is null over a workspace that declares nothing, an unread answer, or one with no headline", () => {
    expect(declaredTopicLayer(NO_DECLARED_TOPICS, MAP)).toBeNull();
    expect(declaredTopicLayer(null, MAP)).toBeNull();
    expect(declaredTopicLayer({} as XserviceDeclaredTopics, MAP)).toBeNull();
  });

  it("is not null when nothing was considered but a member could not be read — unknown is not absent", () => {
    const unread = {
      ...NO_DECLARED_TOPICS,
      headline: {
        ...NO_DECLARED_TOPICS.headline,
        members: { members: 2, read: 1, declaring: 0, unread: ["web"] },
      },
    };
    const layer = declaredTopicLayer(unread, MAP)!;
    expect(layer).not.toBeNull();
    expect(layer.edges).toEqual([]);
  });

  it("draws a net-new membership from the member to the topic's node, in its own class", () => {
    const layer = declaredTopicLayer(answer([decl({})]), MAP)!;
    expect(layer.edges).toEqual([
      { source: serviceId("audit"), target: topicId("orders"), edge_type: DECLARED_TOPIC_EDGE_TYPE },
    ]);
    expect(DECLARED_TOPIC_EDGE_TYPE).not.toBe("publishes");
    expect(DECLARED_TOPIC_EDGE_TYPE).not.toBe("subscribes");
  });

  it("never draws or lists an already-observed declaration as new", () => {
    const observed = decl({ member: "web", status: "observed", site: undefined });
    const layer = declaredTopicLayer(answer([observed]), MAP)!;
    expect(layer.edges).toEqual([]);
    expect(layer.netNew).toEqual([]);
  });

  it("draws one line per pair, however many keys or files declare it, and lists each declaration", () => {
    const two = [decl({}), decl({ key: "app.orders.topic", file: "src/main/resources/application-docker.yml", profile: "docker" })];
    const layer = declaredTopicLayer(answer(two), MAP)!;
    expect(layer.edges).toHaveLength(1);
    expect(layer.netNew).toEqual(two);
  });

  it("lists, but does not draw, a declaration whose topic or member is not on the map", () => {
    const offMap = [decl({ topic: "ghost" }), decl({ member: "stranger" })];
    const layer = declaredTopicLayer(answer(offMap), MAP)!;
    expect(layer.edges).toEqual([]);
    expect(layer.netNew).toEqual(offMap);
  });

  it("draws onto every schema-guarded node of the declared topic", () => {
    // The inventory keys a topic with its message-schema guard; the relation
    // names the topic alone (`declared_topics::topic_of` strips the guard).
    const guarded = buildServiceMap(MEMBERS, [], [
      {
        member: "api",
        topics: [
          { topic: "orders#com.acme.OrderCreated", producers: 1, consumers: 0 },
          { topic: "orders#com.acme.OrderUpdated", producers: 1, consumers: 0 },
          { topic: "ordersarchive", producers: 1, consumers: 0 },
        ],
      },
    ]);
    const layer = declaredTopicLayer(answer([decl({})]), guarded)!;
    expect(layer.edges.map((e) => e.target)).toEqual([
      topicId("orders#com.acme.OrderCreated"),
      topicId("orders#com.acme.OrderUpdated"),
    ]);
  });

  it("keeps the panel when only declared-only or refused values were considered", () => {
    const layer = declaredTopicLayer(answer([], 1), MAP)!;
    expect(layer).not.toBeNull();
    expect(layer.edges).toEqual([]);
    expect(layer.netNew).toEqual([]);
  });
});

describe("the canvas keeps a declaration apart from a coupling (S-491)", () => {
  const layer = declaredTopicLayer(answer([decl({})]), MAP)!;
  const loaded = { nodes: MAP.loaded.nodes, edges: [...MAP.loaded.edges, ...layer.edges] };
  const visible = new Set(Object.keys(loaded.nodes));

  it("never counts a declared edge in a node's degree", () => {
    expect(degreeMap(loaded, visible)).toEqual(degreeMap(MAP.loaded, visible));
  });

  it("draws the declared edge with no arrowhead, and every other edge as before", () => {
    const sel = { seed: null, focusId: null, lockedId: null, locatedId: null, depth: 0 };
    const links = buildSeries(loaded, sel, [0, 0], 1, 0.2, 4).links;
    const declared = links.filter((l) => l.edge_type === DECLARED_TOPIC_EDGE_TYPE);
    expect(declared).toHaveLength(1);
    expect(declared[0]).toHaveProperty("symbol", ["none", "none"]);
    for (const l of links.filter((x) => x.edge_type !== DECLARED_TOPIC_EDGE_TYPE)) {
      expect(l).not.toHaveProperty("symbol");
    }
  });
});
