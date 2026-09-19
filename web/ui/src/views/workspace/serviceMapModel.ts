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
 *   - an **admitted** binding — one the repository proved from committed
 *     configuration rather than one observed at a call site — is never drawn as
 *     though it were observed (S-419, ADR-64). Every link states its per-kind
 *     breakdown, and the canvas edge carries an `admitted` marker the styling
 *     reads as a second channel over the relation arm.
 *
 * No React, no ECharts, no fetch — every function here is pure (NFR-RA-06).
 */

import type {
  BridgeEdge,
  ConfigValueRefusal,
  MemberTopics,
  MemberWarmStateLabel,
  ValueProvenance,
  WorkspaceStatus,
} from "../../api/types.ts";
import type { CanvasEdge, LoadedSet } from "../graph/graphModel.ts";

/** One service as the map knows it — read from the workspace status fan-out, which
 *  is the only source that knows whether a member actually has an index. */
export interface ServiceMember {
  /** The repo-qualified member name. */
  name: string;
  /** The member's warm state as the SERVER derived it (S-323, FR-WS-15) — the single
   *  author of this vocabulary. The map used to re-derive the same three states from
   *  `indexed`/`error` in TypeScript; two classifiers over the same inputs agree only
   *  until the server's precedence changes (and it will: a durable warm-failure record
   *  turns a warm-failed-but-openable member from `deferred` into `degraded`), so the
   *  view reads the label rather than recomputing it. */
  warmState: MemberWarmStateLabel;
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
    warmState: m.warm_state,
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

// ── Edge provenance (S-419, CR-132, ADR-64, FR-WS-19 AC6) ───────────────────
// Every BridgeEdge has carried `from_value` / `to_value` since S-410; until this
// story the map read neither, so a coupling the repository ADMITTED from
// committed configuration drew exactly like one observed at a call site. ADR-64
// forbids that at every surface, which is what the four kinds below repair.

/** The kind ONE aggregated binding is counted under on a {@link ServiceLink}.
 *
 *  `unstated` is the fourth kind and is not on the wire: it is what a binding
 *  whose value field is absent — or carries a token this build does not know —
 *  is counted as. Folding it into `literal` would report an unread field as
 *  observed evidence, the precise failure ADR-64 names (CR-132 AC2). */
export type LinkProvenanceKind = "literal" | "config-bound" | "config-unresolved" | "unstated";

/** Every kind, in the order a breakdown is stated: best-evidenced first. */
export const LINK_PROVENANCE_KINDS: readonly LinkProvenanceKind[] = [
  "literal",
  "config-bound",
  "config-unresolved",
  "unstated",
];

/** How each kind is worded. `literal` reuses `coverageModel`'s wording VERBATIM:
 *  the coverage board and the service map sit in one view, and one fact stated
 *  two ways there reads as two facts. */
export const LINK_PROVENANCE_LABEL: Record<LinkProvenanceKind, string> = {
  literal: "Written at the call site",
  "config-bound": "Admitted from committed configuration",
  "config-unresolved": "Names a key the committed sources do not admit",
  unstated: "Provenance not stated",
};

/** The human label for each refusal a `config-unresolved` end travels with.
 *
 *  These MIRROR `ValueRefusal::label()` in `logos-core/src/resolve/binding.rs`,
 *  which is the vocabulary's one author — only sentence-cased for a table cell.
 *  The three are not interchangeable and naming them loosely sends an operator
 *  to the wrong remedy (NFR-CC-04): `uncommitted` means the value arrives at
 *  runtime from something the repository does not commit — an environment
 *  variable with no committed default, a config server, a secret store — so
 *  there is no key to go and define; `missing-key` means the committed sources
 *  prove no value for the operand, which IS the "go and define it" case. */
export const CONFIG_REFUSAL_LABEL: Record<ConfigValueRefusal, string> = {
  uncommitted: "Not committed by the repository",
  "placeholder-value": "The committed value is itself a placeholder",
  "missing-key": "No committed source defines it",
};

/** How many of a link's bindings fall under each kind. Every kind is present even
 *  at zero, so a reader never has to infer a missing kind's count from an absent
 *  field (NFR-CC-04) — and so the four always sum to {@link ServiceLink.count}. */
export type ProvenanceBreakdown = Record<LinkProvenanceKind, number>;

/** A breakdown with every kind at zero. */
function emptyBreakdown(): ProvenanceBreakdown {
  return { literal: 0, "config-bound": 0, "config-unresolved": 0, unstated: 0 };
}

/** The kind ONE end of a binding proves.
 *
 *  The `default` arm catches both an ABSENT field (the wire is not runtime
 *  validated, so a required type does not make one turn up) and a `provenance`
 *  token a later arm adds that this build does not know. Both are `unstated`
 *  rather than `literal`, for the reason `provenanceLabel` renders an
 *  unrecognised token verbatim rather than as an empty string: a target shown
 *  with no statement of where it came from is the indistinguishability ADR-64
 *  forbids. */
function endKind(value: ValueProvenance | undefined): LinkProvenanceKind {
  switch (value?.provenance) {
    case "literal":
      return "literal";
    case "config-bound":
      return "config-bound";
    case "config-unresolved":
      return "config-unresolved";
    default:
      return "unstated";
  }
}

/** How well-evidenced a kind is — LOWER is better evidenced. */
const KIND_RANK: Record<LinkProvenanceKind, number> = {
  literal: 0,
  "config-bound": 1,
  "config-unresolved": 2,
  unstated: 3,
};

/**
 * The kind one whole binding is counted under: the WEAKER of its two ends.
 *
 * A binding is only as observed as its least-observed end. An edge whose
 * consumer end is a literal and whose provider end was admitted from committed
 * configuration is an admitted edge — counting it as `literal` because one end
 * happens to be one would restore exactly the indistinguishability this story
 * removes. This is also what makes CR-132 AC2 hold without a special case: an
 * absent `from_value` ranks worst, so the binding is `unstated` however well
 * evidenced its other end is.
 */
export function edgeProvenanceKind(edge: BridgeEdge): LinkProvenanceKind {
  const from = endKind(edge.from_value);
  const to = endKind(edge.to_value);
  return KIND_RANK[to] > KIND_RANK[from] ? to : from;
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
  /** How those bindings split across the four provenance kinds (S-419).
   *
   *  A BREAKDOWN, never a single label: a link aggregating one literal and one
   *  admitted binding is neither "observed" nor "admitted", and either word
   *  applied to the whole line is false about half of it (CR-132 §10). */
  provenance: ProvenanceBreakdown;
  /** The bindings this line stands for, in payload order — the evidence
   *  {@link linkEvidence} reads. Retained rather than pre-digested so the detail
   *  and the breakdown cannot drift: both are derived from this one list. */
  bindings: BridgeEdge[];
}

/** Does this link stand for any binding that was NOT observed at a call site?
 *  The gate on every rendering the provenance channel adds — the canvas stroke,
 *  the legend section, the table column and the evidence detail all appear only
 *  when it holds, which is what keeps a literal-only workspace rendering exactly
 *  as it did before this story (CR-132 AC6). */
export function hasNonLiteralBinding(link: ServiceLink): boolean {
  const p = link.provenance;
  return p["config-bound"] + p["config-unresolved"] + p.unstated > 0;
}

/** One row of a link's evidence detail: what one end of one binding proves, at
 *  one overlay (S-419, FR-WS-19 AC2/AC6).
 *
 *  ONE ROW PER OVERLAY, not per key: a key whose overlays disagree proves
 *  several values, and ADR-64 retains every one of them rather than averaging.
 *  Collapsing them here would show an overlay divergence as though the
 *  repository proved a single value. */
export interface EvidenceRow {
  /** Which end of the binding proved this — the consumer's key and the
   *  provider's are routinely spelled differently, which is why they are two
   *  fields on the wire and two rows here. */
  end: "consumer" | "provider";
  /** The member at that end. */
  member: string;
  /** The configuration key. */
  key: string;
  /** The committed value this overlay proves, or `null` on a refusal — where
   *  there IS no value, and rendering an empty string would read as one. */
  value: string | null;
  /** The profiles proving it, sorted as the payload sorted them. */
  profiles: string[];
  /** Whether the UNPROFILED source proves it — carried even when `false`, so it
   *  is never inferred from an empty `profiles` (mirrors `ProfiledValue`). */
  unprofiled: boolean;
  /** The project-relative files proving it — the "defining sources" half. */
  sources: string[];
  /** The refusal, on a `config-unresolved` end; `null` otherwise. */
  refusal: ConfigValueRefusal | null;
}

/** The evidence rows for one end of one binding. A `literal` end yields none —
 *  there is no configuration to name — and neither does an `unstated` one, which
 *  is the absence of a claim rather than a claim about absence. */
function endEvidence(
  end: EvidenceRow["end"],
  member: string,
  value: ValueProvenance | undefined,
): EvidenceRow[] {
  if (value?.provenance === "config-bound") {
    // `bound` is one entry PER KEY the target names and each entry's `values` is
    // one per distinct committed value: `${svc.host}${svc.path}` is two keys, and
    // rendering only the first would hide the second key's evidence entirely.
    return value.bound.flatMap((b) =>
      b.values.map((v) => ({
        end,
        member,
        key: b.key,
        value: v.value,
        profiles: v.profiles,
        unprofiled: v.unprofiled,
        sources: v.sources,
        refusal: null,
      })),
    );
  }
  if (value?.provenance === "config-unresolved") {
    // One row per key, each carrying the refusal — the keys are what the target
    // NAMED, and a refusal stated once beside a joined key list would not say
    // which of them the sources failed to admit.
    return value.keys.map((key) => ({
      end,
      member,
      key,
      value: null,
      profiles: [],
      unprofiled: false,
      sources: [],
      refusal: value.refusal,
    }));
  }
  return [];
}

/** Every evidence row behind one link, consumer end before provider end within
 *  each binding, bindings in payload order. */
export function linkEvidence(link: ServiceLink): EvidenceRow[] {
  return link.bindings.flatMap((b) => [
    ...endEvidence("consumer", b.from.member, b.from_value),
    ...endEvidence("provider", b.to.member, b.to_value),
  ]);
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
      layer: m.warmState === "warm" ? "code" : "doc",
    };
  }

  // Topic nodes + their publish/subscribe edges. A topic whose member is not in the
  // roster is skipped for the same reason a binding to an unknown member is: the
  // canvas resolves links by node id, and inventing the endpoint would fabricate a
  // service (NFR-RA-05).
  const topics = topicLinks(inventory);
  const topicEdges: LoadedSet["edges"] = [];
  /* Which ordered member pairs are coupled on the broker arm by at least one
     binding that was NOT observed at a call site (S-419, CR-132 AC5).

     The topic hops are built from the promoted topic INVENTORY, which carries no
     provenance; the provenance lives on the BINDINGS, which the flat line is
     built from. So a hop is marked from the binding it stands for.

     Since S-424 (FR-WS-27) the two sources agree on the key: the promotion pass
     and the bridge resolve a broker operand through ONE identify function, so a
     config-bound coupling is keyed the same way in the inventory and in the
     bindings, `drawnThroughATopic` fires for it, and it is the HOP that is drawn
     rather than the flat line. That is what makes this marker load-bearing today
     rather than a provision for later: until S-424 the inventory and the bridge
     disagreed for every config-bound site, so the flat line was what was
     actually drawn and an unmarked hop hid nothing. It would now. */
  const admittedBrokerPairs = new Set<string>();
  for (const b of bindings) {
    if (b.relation !== "broker-topic") continue;
    // ANY non-literal kind, deliberately wider than the criterion's phrase
    // "backed by a `config-bound` binding". That same criterion requires the
    // hop to carry "the same marker as the flat line they replace", and the
    // flat line's marker is `hasNonLiteralBinding` — so gating the hop on
    // `config-bound` alone would make a refused or unstated coupling lose its
    // marker precisely by being drawn through its topic, which is the
    // indistinguishability the story removes.
    if (edgeProvenanceKind(b) === "literal") continue;
    admittedBrokerPairs.add(`${b.from.member}\u0000${b.to.member}`);
  }
  /** Is this producer→consumer pair carried by an admitted broker binding? */
  const brokerPairAdmitted = (producer: string, consumer: string): boolean =>
    admittedBrokerPairs.has(`${producer}\u0000${consumer}`);
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
      // The publish hop stands for this producer's binding to EVERY cross-member
      // subscriber of the topic (a broker publish fans out, FR-WS-10), so it is
      // admitted when any one of those bindings is.
      const admitted = t.consumers.some((c) => brokerPairAdmitted(member, c));
      topicEdges.push({
        source: serviceId(member),
        target: topicId(t.topic),
        edge_type: "publishes",
        // Set only when true: an absent slot keeps this edge object identical to
        // the pre-S-419 one, which is what CR-132 AC6 pins.
        ...(admitted ? { admitted: true } : {}),
      });
    }
    for (const member of t.consumers) {
      if (!nodes[serviceId(member)]) continue;
      const admitted = t.producers.some((p) => brokerPairAdmitted(p, member));
      topicEdges.push({
        // A subscribe points FROM the topic TO the consuming service — the direction
        // the message actually travels, so the map reads as a flow
        // (producer → topic → consumer) rather than as two arrows into a sink.
        source: topicId(t.topic),
        target: serviceId(member),
        edge_type: "subscribes",
        ...(admitted ? { admitted: true } : {}),
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
    let existing = byKey.get(key);
    if (!existing) {
      existing = { ...link, count: 0, provenance: emptyBreakdown(), bindings: [] };
      byKey.set(key, existing);
    }
    existing.count += 1;
    // The breakdown and the retained binding move together, so the four counts
    // and the evidence detail can never disagree about the same line (S-419).
    existing.provenance[edgeProvenanceKind(b)] += 1;
    existing.bindings.push(b);
  }

  const links = [...byKey.values()].sort(
    (a, b) =>
      a.from.localeCompare(b.from) || a.to.localeCompare(b.to) || a.relation.localeCompare(b.relation),
  );

  /** Is this broker coupling already drawn as `from → topic → to`? Only then may the
   *  flat service→service line be suppressed as a duplicate.
   *
   *  The two data sources are INDEPENDENT: bindings come from the ledger, topics from
   *  the promoted graph. Since S-424 (FR-WS-27) they agree on the KEY — one identify
   *  function resolves the operand for both tiers — but agreeing on the key is not the
   *  same as both being present. A member indexed by a pre-S-256 binary and not yet
   *  re-synced has the ledger rows but no promoted nodes; a member last indexed before
   *  S-424 has its topics keyed on the placeholder until its next sync; and a member
   *  whose topic read degrades is skipped from the inventory entirely. Suppressing the
   *  line unconditionally would make a RESOLVED coupling vanish from the canvas in
   *  exactly those cases, while still counting it in the links table — the map would
   *  quietly under-draw the workspace (NFR-CC-04). So the line is dropped only when a
   *  topic hop demonstrably replaces it. */
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
          .map((l): CanvasEdge => ({
            source: serviceId(l.from),
            target: serviceId(l.to),
            // The canvas colours/styles an edge by its wire type; the relation arm IS
            // that type here (`route` / `grpc-call`, or `broker-topic` when no topic
            // hop carries it), so the legend grammar carries straight over.
            edge_type: l.relation,
            // Provenance rides as a SECOND channel, never as a new `edge_type`:
            // arm × kind would triple the legend and break the shared grammar
            // FR-UI-29 requires (CR-132 §7). Set only when true — see above.
            ...(hasNonLiteralBinding(l) ? { admitted: true } : {}),
          })),
        ...topicEdges,
      ],
    },
    links,
    topics,
    // Un-indexed and degraded are DIFFERENT facts: "no index yet" is a state the user
    // can fix by indexing; "could not be read" is a fault. Reporting the second as
    // the first would send them to the wrong remedy. Both read the server's
    // `warm_state` rather than re-deriving the split here (S-323, FR-WS-15), so the
    // vocabulary has exactly one author.
    awaitingIndex: members.filter((m) => m.warmState === "deferred").map((m) => m.name),
    degraded: members.filter((m) => m.warmState === "degraded").map((m) => m.name),
  };
}
