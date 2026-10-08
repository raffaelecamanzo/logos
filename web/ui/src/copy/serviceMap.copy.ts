/*
 * The service-map catalogue (S-614, CR-203 §3.2 D items 6–9, FR-UI-39,
 * FR-UI-42): Cross-service bindings, Binding evidence, Declared contracts and
 * Cross-context model hint — the four widgets under the Workspace tab's Service
 * map. The coverage tab speaks from `coverage.copy.ts`; a fact both tabs state
 * (a config refusal's remedy) is worded there once and reused here.
 *
 * Every figure the sentences carry is the view's count of what it renders
 * (NFR-MA-02): `SERVICE_MAP_TEXT` formats the numbers it is handed and computes
 * none, so the view tests name a key, not the prose.
 */

import type { ConfigValueRefusal } from "../api/types.ts";
import type { BINDING_KIND_FILTERS } from "../views/workspace/serviceMapModel.ts";

import { NOT_RESOLVED_REMEDY } from "./coverage.copy.ts";
import { gloss, noAction, plural, type CopyEntry, type WidgetAction } from "./types.ts";

// ── Cross-service bindings (item 6) ──────────────────────────────────────────

export const crossServiceBindings: CopyEntry = {
  what: "Which service calls which: one row per consumer, provider and binding kind, with the number of calls each line on the map stands for.",
  why: "A change to a provider can break every consumer listed against it, so this is where to find who depends on a service before changing it.",
  action: () => noAction,
};

// ── Binding evidence (item 7) ────────────────────────────────────────────────

export interface BindingEvidenceState {
  /** Shown rows whose key no committed source defines. */
  readonly define: number;
  /** Shown rows whose committed value is a placeholder. */
  readonly replace: number;
}

export const bindingEvidence: CopyEntry<BindingEvidenceState> = {
  what: "The committed configuration behind each binding not written at the call site — per end, the key, its value or why it has none, and the files that define it — with identical rows merged and Calls counting the calls each row stands for.",
  why: "A binding taken from configuration holds only while that value is right, so a missing key or a placeholder value is a coupling the repository does not prove.",
  action: ({ define, replace }) =>
    define + replace === 0
      ? noAction
      : {
          kind: "act",
          where: "configuration",
          text: "Define each key no committed source defines, and replace each placeholder value, in the member's configuration; each row names its key and says what to do.",
        },
};

/** One evidence row, as its own action reads it. */
export interface EvidenceRowFacts {
  readonly member: string;
  readonly key: string;
  readonly refusal: ConfigValueRefusal | null;
  readonly sources: readonly string[];
}

/**
 * One evidence row's own action (the Binding evidence table's last column),
 * following its refusal (CR-203 item 7). The two refusals the repository can
 * fix reuse the coverage tab's remedies — sentence and where — so the two tabs
 * cannot word one remedy two ways. A value the repository does not commit has
 * nothing to fix in the repository: that is the one `none`, and its sentence is
 * `SERVICE_MAP_TEXT.arrivesAtRuntime`.
 *
 * Exhaustive over the closed union, so a refusal added to `ConfigValueRefusal`
 * without an action here is a `tsc -b` error. The wire is not runtime
 * validated, so a token this build does not know still reaches the last arm:
 * it is pointed at the command that states its reason, never told to correct a
 * value the row does not show.
 */
export function evidenceRowAction(row: EvidenceRowFacts): WidgetAction {
  switch (row.refusal) {
    case "missing-key":
      return remedyAction(row, NOT_RESOLVED_REMEDY["config-key-missing"]);
    case "placeholder-value":
      return remedyAction(row, NOT_RESOLVED_REMEDY["config-placeholder-value"]);
    case "uncommitted":
      return noAction;
    case null:
      // A committed value: right or wrong, its file is the one the row names.
      return {
        kind: "act",
        where: "configuration",
        target: row.sources.join(", "),
        text: "If this value is wrong, correct it in the file named under Defining sources.",
      };
    default: {
      const unknown: never = row.refusal;
      void unknown;
      return {
        kind: "act",
        where: "command",
        target: "logos workspace status",
        text: "This refusal is newer than this page; read its reason in the workspace status.",
      };
    }
  }
}

/** A refusal's action from its coverage-tab remedy, scoped to the row's member. */
function remedyAction(row: EvidenceRowFacts, remedy: (typeof NOT_RESOLVED_REMEDY)[keyof typeof NOT_RESOLVED_REMEDY]): WidgetAction {
  return { kind: "act", where: remedy.where, target: row.key, text: `In ${row.member}, ${remedy.remedy}.` };
}

// ── Declared contracts (item 8) ──────────────────────────────────────────────

export const declaredContracts: CopyEntry<{ documents: number }> = {
  what: "The contracts each member declares by holding another service's API document without implementing it — with the member that document belongs to, or with a named external service — and the documents and matched calls behind each.",
  why: "A declared contract is written down, never observed in code, so none of these is a binding above; it shows which services expect each other.",
  action: ({ documents }) =>
    documents === 0
      ? noAction
      : {
          kind: "act",
          where: "documentation",
          text: "Check each counterparty; where one is wrong, correct or remove the API document the Documents table names.",
        },
};

// ── Cross-context model hint (item 9) ────────────────────────────────────────

export const crossContextHint: CopyEntry = {
  what: ["Services built against the model libraries of two or more ", gloss("boundedContext", "bounded contexts"), "."],
  why: "Such a service may couple contexts that should stay apart; a build dependency is not a runtime call, so none of this is drawn on the map.",
  action: () => ({
    kind: "act",
    where: "configuration",
    target: "pom.xml / build.gradle",
    text: "Review this dependency in each listed member's build manifest; it is a hint for review, not a failure.",
  }),
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
  /** The one `none` an evidence row has: the value arrives at runtime. */
  arrivesAtRuntime:
    "Nothing to fix in the repository: the value arrives at runtime, from an environment variable with no committed default.",
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
