/*
 * Pure service-map model (S-250, CR-061, FR-UI-29) — the app-level graph, folded
 * down onto the *existing* canvas contract so it renders through the unchanged
 * §4.4 ECharts component (`GraphCanvas`) rather than a second canvas.
 *
 * Services are nodes; resolved cross-service bindings are edges. The fold is:
 *   member name              → the canvas node id (its own namespace, `service:<m>`,
 *                              so it can never collide with a SCIP symbol id)
 *   BridgeEdge(from → to)    → one canvas edge per (consumer, provider, relation),
 *                              deduped: 40 route bindings between two services are
 *                              ONE line, weighted, not 40 overdrawn ones.
 *
 * Honesty (NFR-CC-04, NFR-RA-05):
 *   - an **unbound** reference is never an edge. It has no provider to point at, so
 *     drawing one would fabricate a binding. Sparsity is reported as coverage
 *     (`coverageModel`), never as a hairline nobody notices.
 *   - a member with no index yet is still a node (it exists!) but is marked
 *     `awaitingIndex`, so the view can render it muted rather than as a service that
 *     genuinely has no couplings.
 *   - a self-binding (a member bound to itself) is not a *cross*-service edge; the
 *     bridge does not emit one, and this model would not draw it if it did.
 *
 * No React, no ECharts, no fetch — every function here is pure (NFR-RA-06).
 */

import type { BridgeEdge, MemberTopics, WorkspaceStatus } from "../../api/types.ts";
import type { LoadedSet } from "../graph/graphModel.ts";

/** One service as the map knows it — read from the workspace status fan-out, which
 *  is the only source that knows whether a member actually has an index. */
export interface ServiceMember {
  /** The repo-qualified member name. */
  name: string;
  /** `false` when the member has no index yet. */
  indexed: boolean;
  /** The per-member degradation the fan-out reported, when it reported one — an
   *  engine that FAILED is not the same thing as one that is merely un-indexed, and
   *  the map must not say it is (NFR-CC-04). */
  error: string | null;
}

/** Project the workspace status fan-out onto the service roster the map draws. */
export function serviceMembers(status: WorkspaceStatus): ServiceMember[] {
  return status.members.map((m) => ({
    name: m.member,
    indexed: m.result?.indexed ?? false,
    error: m.error ?? null,
  }));
}

/** The canvas-id namespace for a service node — never collides with a SCIP symbol. */
const SERVICE_ID_PREFIX = "service:";

/** The canvas-id namespace for a topic node (S-256) — a *separate* namespace, so a
 *  topic id can never decode as a service id. That matters beyond tidiness: the
 *  canvas's `onNodeClick` selects a member for any id that decodes as a service, so
 *  a topic sharing the namespace would silently "select" a member named after it. */
const TOPIC_ID_PREFIX = "topic:";

/** The canvas node id for a member. */
export function serviceId(member: string): string {
  return `${SERVICE_ID_PREFIX}${member}`;
}

/** The member a canvas node id names, or `null` when it is not a service node. */
export function memberOfServiceId(id: string): string | null {
  return id.startsWith(SERVICE_ID_PREFIX) ? id.slice(SERVICE_ID_PREFIX.length) : null;
}

/** The canvas node id for a topic. Topics are keyed by their identity ALONE — not by
 *  member — because that shared identity is exactly what couples two services
 *  (FR-WS-11): one `orders` node with a publisher on one side and a subscriber on
 *  the other IS the cross-service binding, drawn. */
export function topicId(topic: string): string {
  return `${TOPIC_ID_PREFIX}${topic}`;
}

/** The topic a canvas node id names, or `null` when it is not a topic node. */
export function topicOfTopicId(id: string): string | null {
  return id.startsWith(TOPIC_ID_PREFIX) ? id.slice(TOPIC_ID_PREFIX.length) : null;
}

/** One service-to-service coupling: every binding between two members under one
 *  relation arm, collapsed to a single weighted edge. */
export interface ServiceLink {
  /** The consuming member. */
  from: string;
  /** The providing member. */
  to: string;
  /** The relation arm (`route`, `grpc-call`, `broker-topic`). */
  relation: string;
  /** How many individual bindings this one line stands for — never rounded. */
  count: number;
}

/** One topic as the map draws it: the shared identity, and which members produce and
 *  consume it. A topic with producers in one member and consumers in another IS a
 *  cross-service coupling — rendered as two hops through the topic rather than as one
 *  opaque service→service line (S-256, FR-WS-11). */
export interface TopicLink {
  /** The topic key — its own identity, independent of any member. */
  topic: string;
  /** Members publishing to it, sorted. */
  producers: string[];
  /** Members subscribing from it, sorted. */
  consumers: string[];
}

/** The service map: the canvas set plus the roll-up figures the view states. */
export interface ServiceMap {
  /** The set the unchanged `GraphCanvas` renders. */
  loaded: LoadedSet;
  /** The deduped service-to-service couplings, sorted deterministically. */
  links: ServiceLink[];
  /** The topics on the canvas, sorted by key (S-256). */
  topics: TopicLink[];
  /** Members with no index yet — rendered muted, never as "no couplings". */
  awaitingIndex: string[];
  /** Members whose engine could not be read — stated as *unavailable*, never folded
   *  in with the merely un-indexed ones. */
  degraded: string[];
}

/** The dedup key for a service coupling. */
function linkKey(l: Pick<ServiceLink, "from" | "to" | "relation">): string {
  // `\u0000` as the separator: it cannot occur in a member name or a relation
  // token, so two distinct links can never collide on one key. Written as an
  // ESCAPE, never a raw NUL byte — a literal control character would make git
  // treat this source file as binary (unreviewable diffs, unmergeable in a
  // parallel-worktree sprint).
  return `${l.from}\u0000${l.to}\u0000${l.relation}`;
}

/**
 * Fold the per-member topic inventory into the shared topic view the map draws.
 *
 * Keyed by topic identity ALONE: `orders` published by `api` and subscribed by
 * `billing` is ONE node with a producer edge and a consumer edge — which is the
 * cross-member binding made visible, without the map ever having to consult the
 * bridge (FR-WS-11).
 *
 * A member that reports a topic with `producers: 0, consumers: 0` (a promoted topic
 * whose sites were all removed but which has not been reconciled away yet) still
 * yields the topic node, with no edges — honest, and never a fabricated coupling.
 */
export function topicLinks(inventory: readonly MemberTopics[]): TopicLink[] {
  const byTopic = new Map<string, TopicLink>();
  for (const member of inventory) {
    for (const t of member.topics) {
      let link = byTopic.get(t.topic);
      if (!link) {
        link = { topic: t.topic, producers: [], consumers: [] };
        byTopic.set(t.topic, link);
      }
      if (t.producers > 0) link.producers.push(member.member);
      if (t.consumers > 0) link.consumers.push(member.member);
    }
  }
  const links = [...byTopic.values()];
  for (const l of links) {
    l.producers.sort((a, b) => a.localeCompare(b));
    l.consumers.sort((a, b) => a.localeCompare(b));
  }
  return links.sort((a, b) => a.topic.localeCompare(b.topic));
}

/**
 * Build the service map from the member roster, the resolved cross-service
 * bindings, and the promoted topic inventory. Deterministic: members keep manifest
 * order, links sort by (from, to, relation) and topics by key, so two runs over the
 * same workspace render identically.
 *
 * Topics are drawn as first-class nodes (S-256, FR-WS-11) rather than folded into an
 * opaque service→service `broker-topic` line, so the map answers *which* topic
 * couples two services — and shows a topic that is published but not yet consumed
 * anywhere, which has no binding to fold.
 */
export function buildServiceMap(
  members: readonly ServiceMember[],
  bindings: readonly BridgeEdge[],
  inventory: readonly MemberTopics[] = [],
): ServiceMap {
  const nodes: LoadedSet["nodes"] = {};
  for (const m of members) {
    nodes[serviceId(m.name)] = {
      id: serviceId(m.name),
      label: m.name,
      // The canvas renders `kind` in its tooltip; "service" is the honest kind of
      // an app-level node (it is not a symbol).
      kind: "service",
      // A member awaiting an index (or one whose engine could not be read) is drawn
      // in the muted `doc` hue rather than the code hue, so "no data yet" never reads
      // as "indexed, but uncoupled".
      layer: m.indexed && !m.error ? "code" : "doc",
    };
  }

  // Topic nodes + their publish/subscribe edges. A topic whose member is not in the
  // roster is skipped for the same reason a binding to an unknown member is: the
  // canvas resolves links by node id, and inventing the endpoint would fabricate a
  // service (NFR-RA-05).
  const topics = topicLinks(inventory);
  const topicEdges: LoadedSet["edges"] = [];
  for (const t of topics) {
    nodes[topicId(t.topic)] = {
      id: topicId(t.topic),
      label: t.topic,
      // The honest kind of the node — it mirrors the `topic` NodeKind the graph now
      // carries, so the canvas tooltip says the same word the CLI and MCP do.
      kind: "topic",
      // `layer` is this map's HUE channel, not an ontology claim: S-250 already draws an
      // un-indexed member in the `doc` hue for the same reason. The artifact hue simply
      // separates topics from services at a glance. It deliberately does NOT mirror the
      // graph's own layer for a `Topic` — S-255 kept the broker kinds in the CODE
      // subgraph (`is_config()` excludes them), which is what the node views render.
      layer: "artifact",
    };
    for (const member of t.producers) {
      if (!nodes[serviceId(member)]) continue;
      topicEdges.push({
        source: serviceId(member),
        target: topicId(t.topic),
        edge_type: "publishes",
      });
    }
    for (const member of t.consumers) {
      if (!nodes[serviceId(member)]) continue;
      topicEdges.push({
        // A subscribe points FROM the topic TO the consuming service — the direction
        // the message actually travels, so the map reads as a flow
        // (producer → topic → consumer) rather than as two arrows into a sink.
        source: topicId(t.topic),
        target: serviceId(member),
        edge_type: "subscribes",
      });
    }
  }

  const byKey = new Map<string, ServiceLink>();
  for (const b of bindings) {
    // A binding whose endpoints are not both in the roster cannot be drawn — the
    // canvas resolves links by node id, and inventing a node for an unknown member
    // would fabricate a service (NFR-RA-05).
    if (!nodes[serviceId(b.from.member)] || !nodes[serviceId(b.to.member)]) continue;
    if (b.from.member === b.to.member) continue; // not a cross-service edge
    // A `broker-topic` binding is now drawn THROUGH its topic node (publisher →
    // topic → subscriber), so also drawing the direct service→service line would
    // render the same coupling twice — once opaque, once named. The topic hop is the
    // better of the two (it says *which* topic), so the flat line is dropped rather
    // than doubled. It still counts in `links`, which the view states as a figure.
    const link = { from: b.from.member, to: b.to.member, relation: b.relation };
    const key = linkKey(link);
    const existing = byKey.get(key);
    if (existing) existing.count += 1;
    else byKey.set(key, { ...link, count: 1 });
  }

  const links = [...byKey.values()].sort(
    (a, b) =>
      a.from.localeCompare(b.from) || a.to.localeCompare(b.to) || a.relation.localeCompare(b.relation),
  );

  /** Is this broker coupling already drawn as `from → topic → to`? Only then may the
   *  flat service→service line be suppressed as a duplicate.
   *
   *  The two data sources are INDEPENDENT: bindings come from the ledger, topics from
   *  the promoted graph. They can disagree — a member indexed by a pre-S-256 binary and
   *  not yet re-synced has the ledger rows but no promoted nodes, and a member whose
   *  topic read degrades is skipped from the inventory entirely. Suppressing the line
   *  unconditionally would make a RESOLVED coupling vanish from the canvas in exactly
   *  those cases, while still counting it in the links table — the map would quietly
   *  under-draw the workspace (NFR-CC-04). So the line is dropped only when a topic hop
   *  demonstrably replaces it. */
  const drawnThroughATopic = (l: ServiceLink): boolean =>
    topics.some((t) => t.producers.includes(l.from) && t.consumers.includes(l.to));

  return {
    loaded: {
      nodes,
      edges: [
        ...links
          // The broker arm is drawn through its topic node instead (see above) — but
          // only where a topic actually carries it; otherwise the direct line stays, so
          // a resolved coupling is never silently un-drawn.
          .filter((l) => l.relation !== "broker-topic" || !drawnThroughATopic(l))
          .map((l) => ({
            source: serviceId(l.from),
            target: serviceId(l.to),
            // The canvas colours/styles an edge by its wire type; the relation arm IS
            // that type here (`route` / `grpc-call`, or `broker-topic` when no topic
            // hop carries it), so the legend grammar carries straight over.
            edge_type: l.relation,
          })),
        ...topicEdges,
      ],
    },
    links,
    topics,
    // Un-indexed and degraded are DIFFERENT facts: "no index yet" is a state the user
    // can fix by indexing; "could not be read" is a fault. Reporting the second as
    // the first would send them to the wrong remedy.
    awaitingIndex: members.filter((m) => !m.indexed && !m.error).map((m) => m.name),
    degraded: members.filter((m) => m.error !== null).map((m) => m.name),
  };
}
