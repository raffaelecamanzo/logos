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

import type { BridgeEdge, WorkspaceStatus } from "../../api/types.ts";
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

/** The canvas node id for a member. */
export function serviceId(member: string): string {
  return `${SERVICE_ID_PREFIX}${member}`;
}

/** The member a canvas node id names, or `null` when it is not a service node. */
export function memberOfServiceId(id: string): string | null {
  return id.startsWith(SERVICE_ID_PREFIX) ? id.slice(SERVICE_ID_PREFIX.length) : null;
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

/** The service map: the canvas set plus the roll-up figures the view states. */
export interface ServiceMap {
  /** The set the unchanged `GraphCanvas` renders. */
  loaded: LoadedSet;
  /** The deduped service-to-service couplings, sorted deterministically. */
  links: ServiceLink[];
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
 * Build the service map from the member roster and the resolved cross-service
 * bindings. Deterministic: members keep manifest order, links sort by
 * (from, to, relation), so two runs over the same workspace render identically.
 */
export function buildServiceMap(
  members: readonly ServiceMember[],
  bindings: readonly BridgeEdge[],
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

  const byKey = new Map<string, ServiceLink>();
  for (const b of bindings) {
    // A binding whose endpoints are not both in the roster cannot be drawn — the
    // canvas resolves links by node id, and inventing a node for an unknown member
    // would fabricate a service (NFR-RA-05).
    if (!nodes[serviceId(b.from.member)] || !nodes[serviceId(b.to.member)]) continue;
    if (b.from.member === b.to.member) continue; // not a cross-service edge
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

  return {
    loaded: {
      nodes,
      edges: links.map((l) => ({
        source: serviceId(l.from),
        target: serviceId(l.to),
        // The canvas colours/styles an edge by its wire type; the relation arm IS
        // that type here (`route` / `grpc-call` / `broker-topic`), so the legend
        // grammar carries straight over.
        edge_type: l.relation,
      })),
    },
    links,
    // Un-indexed and degraded are DIFFERENT facts: "no index yet" is a state the user
    // can fix by indexing; "could not be read" is a fault. Reporting the second as
    // the first would send them to the wrong remedy.
    awaitingIndex: members.filter((m) => !m.indexed && !m.error).map((m) => m.name),
    degraded: members.filter((m) => m.error !== null).map((m) => m.name),
  };
}
