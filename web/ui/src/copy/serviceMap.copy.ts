/*
 * The service-map catalogue (S-614, CR-203 §3.2 D items 6–9, FR-UI-39,
 * FR-UI-42): Cross-service bindings, Binding evidence, Declared contracts and
 * Cross-context model hint — the four widgets under the Workspace tab's Service
 * map. The coverage tab speaks from `coverage.copy.ts`.
 *
 * Every figure the sentences carry is the view's count of what it renders
 * (NFR-MA-02): `SERVICE_MAP_TEXT` formats the numbers it is handed and computes
 * none, so the view tests name a key, not the prose.
 */

import type { BINDING_KIND_FILTERS } from "../views/workspace/serviceMapModel.ts";

import { gloss, plural, type CopyEntry } from "./types.ts";

// ── Cross-service bindings (item 6) ──────────────────────────────────────────

export const crossServiceBindings: CopyEntry = {
  what: "Which service calls which: one row per consumer, provider and binding kind, with the number of calls each line on the map stands for.",
  why: "A change to a provider can break every consumer listed against it, so this is where to find who depends on a service before changing it.",
};

// ── Binding evidence (item 7) ────────────────────────────────────────────────

export const bindingEvidence: CopyEntry = {
  what: "The committed configuration behind each binding not written at the call site — per end, the key, its value or why it has none, and the files that define it — with identical rows merged and Calls counting the calls each row stands for.",
  why: "A binding taken from configuration holds only while that value is right, so a missing key or a placeholder value is a coupling the repository does not prove.",
};

// ── Declared contracts (item 8) ──────────────────────────────────────────────

export const declaredContracts: CopyEntry = {
  what: "The contracts each member declares by holding another service's API document without implementing it — with the member that document belongs to, or with a named external service — and the documents and matched calls behind each.",
  why: "A declared contract is written down, never observed in code, so none of these is a binding above; it shows which services expect each other.",
};

// ── Cross-context model hint (item 9) ────────────────────────────────────────

export const crossContextHint: CopyEntry = {
  what: ["Services built against the model libraries of two or more ", gloss("boundedContext", "bounded contexts"), "."],
  why: "Such a service may couple contexts that should stay apart; a build dependency is not a runtime call, so none of this is drawn on the map.",
};

// ── Figure-row, absence and evidence sentences ───────────────────────────────

/** The short names the binding-kind filter offers, by relation (FR-UI-42:
 *  HTTP, gRPC, broker). Keyed by the filter's own tuple, so a kind offered
 *  without a label is a `tsc -b` error. */
export const BINDING_KIND_LABEL: Readonly<Record<(typeof BINDING_KIND_FILTERS)[number], string>> = {
  route: "HTTP",
  "grpc-call": "gRPC",
  "broker-topic": "Broker",
};

export const SERVICE_MAP_TEXT = {
  /** The bindings figure: rows the filter keeps, of every row. */
  bindingsShown: (shown: number, total: number) =>
    `${shown} of ${total} ${plural(total, "binding", "bindings")} shown`,
  /** No binding resolved — the map is not empty when topics are drawn. */
  noBindings: (topics: number) =>
    topics === 0
      ? "No cross-service bindings resolved yet — every service is drawn, and the Cross-service coverage tab gives the reason for each reference left unlinked."
      : `No service calls another directly yet; the map draws ${topics} broker ${plural(topics, "topic", "topics")}, and a topic is a coupling even before anything subscribes to it.`,
  /** The filter kept nothing. */
  noneMatch: "No binding matches this filter.",
  /** The filter's labels. */
  filterText: "Consumer or provider",
  filterTextHint: "Part of a service name; matched in either column.",
  filterKind: "Binding kind",
  filterProvenance: "Provenance",
  anyKind: "All kinds",
  anyProvenance: "All provenances",
  /** The evidence figure: the links not observed at a call site that the
   *  filter keeps, of all of them. Not "with configuration evidence": a link
   *  whose provenance was never stated is counted, and has none to show. */
  evidenceShown: (shown: number, total: number) =>
    `Shown: ${shown} of ${total} ${plural(total, "binding", "bindings")} not observed at a call site`,
  /** A link with evidence names no key. Reachable two ways, so it draws no
   *  conclusion about why: an `unstated` end, or a config end naming no key. */
  noKeyNamed:
    "No configuration key is named for this coupling, so there is nothing here to evidence it either way — its Provenance breakdown above says what is known.",
  /** The declared figure, from the widget's own tables. `calls` is `null`
   *  when no drawn link names an external: then no call was matched against
   *  one, and "0 calls matched" would state an answer to a question never
   *  asked (the contracts table's "—", NFR-CC-04). */
  declaredFigure: (links: number, documents: number, calls: number | null) =>
    `${links} declared ${plural(links, "contract", "contracts")}, from ${documents} ${plural(documents, "document", "documents")}${
      calls === null ? "" : ` · ${calls} ${plural(calls, "call", "calls")} matched to a named external`
    }`,
  /** A relation that names externals and declares nothing. */
  noDeclaredLinks:
    "No member on this map declares a contract. The externals below are still named: each is held by a declared mock member standing in for it.",
  /** The hint figure. */
  hintFigure: (n: number) =>
    `${n} ${plural(n, "member depends", "members depend")} on the model libraries of two or more contexts`,
  /** How a context is named, for the evidence. */
  hintNaming:
    "A context is named by its model library's coordinate: <group>.<context>:kafka-models or <context>-kafka-models.",
} as const;
