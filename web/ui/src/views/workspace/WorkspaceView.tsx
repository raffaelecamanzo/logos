/*
 * The Workspace tab (S-250, CR-061, FR-UI-29; frontend-design §4.16/§4.17) — the
 * app-level cross-service surface, in workspace mode only.
 *
 * Three panels over the S-249 `/api/v1/workspace/*` read-models, of which the
 * hidden-widget register (S-612, FR-UI-41) hides the third — so the tab renders
 * two, and `GET /api/v1/workspace/impact`, `logos xservice impact` and MCP
 * `xservice_impact` still serve the impact answer:
 *   - Service map — services as nodes, resolved cross-service bindings as edges,
 *     rendered through the UNCHANGED §4.4 ECharts canvas (`GraphCanvas`) with the
 *     same legend grammar. Clicking a service focuses its member (the shell
 *     selector switches, and every other view re-fetches scoped to it).
 *   - Cross-service coverage — the resolved-edge headline, spec conformance and
 *     the by-intake board (the per-arm board is hidden through the register).
 *   - Cross-service impact — a symbol's impact in its own member(s) plus each
 *     far-side impact stitched across a binding.
 *
 * The build layer (S-464, CR-148, FR-WS-33): when any member holds a build
 * manifest, the map's legend gains a toggle — OFF by default — that draws what
 * each member builds against in its own `build` edge class, with declared
 * platform members collapsed; a cross-context model hint lists members depending
 * on two or more contexts' model libraries, never as an edge; and the coverage
 * tab states the build headline apart from every runtime figure. A build
 * dependency is never a runtime coupling (BR-58). A workspace with no build
 * manifest draws no build layer on the map; since S-613 its coverage tab's Build
 * dependencies widget states that absence rather than being left out.
 *
 * The declared layer (S-461, CR-147, FR-WS-31): when a member holds a vendored
 * spec, the map draws each declared contract in its own `declares-contract` edge
 * class — to the member whose own spec the document is, or to a named external
 * drawn as its own node — with a legend section; the evidence names each
 * document, its identity score or external, and every call bound to the
 * external with its matched operation and base-path source. The coverage tab
 * renders both server headlines in their own widget. A declared contract is never
 * an observed call (BR-57), and a workspace with no vendored spec draws no
 * declared layer and renders no declared widget (FR-UI-29 AC8, kept by CR-203).
 *
 * The coverage tab (S-613, CR-203 §3.2 D items 4 and 10): its widgets — the
 * three coverage boards, Declared contracts (when a member vendors a spec) and
 * Build dependencies — sit in one `WidgetStack`, with their words in
 * `copy/coverage.copy.ts`.
 *
 * The service map tab (S-614, CR-203 §3.2 D items 6–9, FR-UI-42): the map with
 * its legend and notes, then its widgets — Cross-service bindings (filtered by
 * text, binding kind and provenance), Binding evidence (identical rows merged
 * with a Calls count), Declared contracts (one Documents and one Bound calls
 * table), the build twin and the Cross-context model hint — all in one
 * `WidgetStack`, with their words in `copy/serviceMap.copy.ts`. The filter
 * narrows the bindings table and the evidence, never the canvas.
 *
 * Honesty (NFR-CC-04, NFR-RA-05): an unbound reference is never drawn as an edge
 * (its absence is *reported* as coverage, not hidden); a member with no index is a
 * muted node, not a service with "no couplings"; a workspace with no bindings gets
 * the awaiting-data state, never a fabricated 100%.
 *
 * Every read here is a GET (ADR-28). In single-root mode this view is unreachable —
 * no nav item is rendered — and it says so honestly if navigated to by hand.
 *
 * Absence wording here follows the one taxonomy rather than restating it:
 * `models::quality::absence` in `logos-core/src/models/quality.rs` (S-434) —
 * the closed sentinel vocabulary and the rules (R0-R5) every absence-
 * reporting site keeps. Enumerated from source by
 * `logos-core/tests/absence_taxonomy_audit.rs`.
 */

import { useState } from "react";

import { AsyncResource, useApiResource } from "../../api/index.ts";
import {
  fetchWorkspaceBindings,
  fetchWorkspaceBuildDeps,
  fetchWorkspaceImpact,
  fetchWorkspaceStatus,
} from "../../api/workspaceClient.ts";
import type {
  BoundExternal,
  BuildDependencyHeadline,
  CrossContextHint,
  CrossServiceCoverage,
  CrossServiceImpact,
  ImpactEntry,
  ImpactResult,
  MemberTopics,
  WorkspaceStatus,
  XserviceBuildDeps,
  XserviceImpact,
  XserviceRouteProviders,
} from "../../api/types.ts";
import {
  ActionCell,
  Badge,
  Button,
  Callout,
  Card,
  DataTable,
  DEFAULT_TABLE_PAGE_SIZE,
  EmptyState,
  ErrorPanel,
  LoadingState,
  SelectField,
  Tabs,
  TextField,
  Widget,
  WidgetStack,
  type Column,
} from "../../components/index.ts";
import {
  buildDependencies,
  COVERAGE_TEXT,
  declaredRelations,
} from "../../copy/coverage.copy.ts";
import {
  BINDING_KIND_LABEL,
  bindingEvidence,
  crossContextHint,
  crossServiceBindings,
  declaredContracts,
  evidenceRowAction,
  SERVICE_MAP_TEXT,
} from "../../copy/serviceMap.copy.ts";
import { useWorkspace } from "../../workspace/WorkspaceContext.tsx";
import { GraphCanvas } from "../graph/GraphCanvas.tsx";
import { ADMITTED_DASH } from "../graph/graphModel.ts";
import { EdgeRow } from "../graph/Legend.tsx";
import { isWidgetHidden } from "../hiddenWidgets.ts";
import {
  ARM_LABEL,
  armLabel,
  buildCoverageDashboard,
  type CoverageDashboard,
} from "./coverageModel.ts";
import { CoveragePanel } from "./CoverageBoards.tsx";
import {
  BINDING_KIND_FILTERS,
  BUILD_EDGE_TYPE,
  buildLayer,
  buildServiceMap,
  CONFIG_REFUSAL_LABEL,
  DECLARED_EDGE_TYPE,
  declaredLayer,
  filterLinks,
  groupEvidence,
  hasNonLiteralBinding,
  LINK_PROVENANCE_KINDS,
  LINK_PROVENANCE_LABEL,
  linkEvidence,
  memberOfServiceId,
  NO_LINK_FILTER,
  serviceMembers,
  type BoundCall,
  type BuildLink,
  type DeclaredLayer,
  type DeclaredLink,
  type EvidenceGroup,
  type LinkFilter,
  type ServiceLink,
  type ServiceMember,
} from "./serviceMapModel.ts";
import graphStyles from "../graph/GraphView.module.css";
import styles from "./Workspace.module.css";

/** The relation arms the service map can draw — the legend's rows. Derived from the
 *  arm-label map so a new arm cannot be added to the dashboard yet silently omitted
 *  from the legend. */
const RELATION_ARMS = Object.keys(ARM_LABEL);

export function WorkspaceView() {
  const { mode, workspace, members, error } = useWorkspace();
  // The fan-out status (per-member freshness + coverage). Fetched HERE, not in the
  // shell: it constructs every member's engine, which is right for the tab that shows
  // cross-service coverage and wrong for a probe on every page load (NFR-PE-10).
  const status = useApiResource<WorkspaceStatus>(() => fetchWorkspaceStatus(), []);

  // The probe has not answered yet. We do NOT know the mode, so we must not assert
  // one: claiming "not a workspace" here would flash a falsehood at every real
  // workspace on its way in (NFR-CC-04 — an honest "reading…" is not an empty state).
  if (mode === "loading") {
    return (
      <div className={styles.view}>
        <LoadingState label="Reading the workspace…" />
      </div>
    );
  }

  // The probe failed outright — say so; a broken read is not a plain repo (NFR-RA-05).
  if (error) {
    return (
      <div className={styles.view}>
        <ErrorPanel>The workspace status could not be read: {error.message}</ErrorPanel>
      </div>
    );
  }

  // Settled, and this is genuinely a single-root serve: a hand-typed `/workspace` in a
  // plain repo gets the honest answer rather than a broken fetch.
  if (mode !== "workspace") {
    return (
      <div className={styles.view}>
        <EmptyState message="Not a workspace — this serve has a single repository root. Start Logos at a directory with a logos.workspace.toml to federate members." />
      </div>
    );
  }

  return (
    <div className={styles.view}>
      <AsyncResource resource={status} loadingLabel="Loading the workspace…">
        {(model) => <WorkspaceContent workspace={workspace} members={members} status={model} />}
      </AsyncResource>
    </div>
  );
}

function WorkspaceContent({
  workspace,
  members,
  status,
}: {
  workspace: string | null;
  members: string[];
  status: WorkspaceStatus;
}) {
  const coverage = buildCoverageDashboard(status.coverage);
  const services = serviceMembers(status);

  return (
    <>
      <Callout label="Workspace" tone="signal">
        <span>
          <span className="mono">{workspace}</span> · {members.length} service
          {members.length === 1 ? "" : "s"} ·{" "}
          {coverage.isEmpty ? (
            "no cross-service references yet"
          ) : (
            <>
              {coverage.bound} bound · {coverage.ambiguous} ambiguous · {coverage.unbound} unbound
            </>
          )}
        </span>
      </Callout>

      <Tabs
        label="Workspace views"
        tabs={[
          {
            id: "map",
            label: "Service map",
            panel: (
              <ServiceMapPanel
                services={services}
                topics={status.topics ?? []}
                build={status.build_dependency}
                coverage={status.coverage}
              />
            ),
          },
          {
            id: "coverage",
            label: "Cross-service coverage",
            // One stack for every widget on the tab (S-613, FR-UI-40): the
            // coverage boards are a fragment, so they and the two relation
            // widgets are siblings at one gap — the tab panel itself has none.
            panel: (
              <WidgetStack>
                <CoveragePanel dashboard={coverage} degraded={status.degraded_rollup} />
                <DeclaredRelationsCard dashboard={coverage} />
                <BuildDependencyCard headline={status.build_dependency} />
              </WidgetStack>
            ),
          },
          ...(isWidgetHidden("cross-service-impact")
            ? []
            : [{ id: "impact", label: "Cross-service impact", panel: <ImpactPanel /> }]),
        ]}
      />
    </>
  );
}

// ── Service map (frontend-design §4.16) ──────────────────────────────────────

function ServiceMapPanel({
  services,
  topics,
  build,
  coverage,
}: {
  services: ServiceMember[];
  topics: MemberTopics[];
  /** The status payload's build headline — present only when some member holds
   *  a build manifest. Absent, the build layer does not exist: no toggle, no
   *  fetch, and the map renders exactly as it did before it (S-464). */
  build?: BuildDependencyHeadline;
  /** The status payload's coverage, the one source of the declared relations
   *  (S-461) — read from what the view already fetched, never a second request. */
  coverage: CrossServiceCoverage;
}) {
  const bindings = useApiResource<XserviceRouteProviders>(() => fetchWorkspaceBindings(), []);
  // Read beside the bindings rather than on the toggle, so the cross-context hint
  // (a report, not a layer) is shown without drawing anything. Never fetched when
  // the status says there is no relation to read.
  const hasBuild = build !== undefined;
  const deps = useApiResource<XserviceBuildDeps | null>(
    () => (hasBuild ? fetchWorkspaceBuildDeps() : Promise.resolve(null)),
    [hasBuild],
  );
  return (
    <AsyncResource resource={bindings} loadingLabel="Loading the service map…">
      {(model) => (
        <ServiceMap
          services={services}
          providers={model}
          topics={topics}
          build={build}
          coverage={coverage}
          deps={deps.data ?? null}
          depsError={deps.error ?? null}
        />
      )}
    </AsyncResource>
  );
}

// ── The build layer (S-464, FR-WS-33) ────────────────────────────────────────

const BUILD_KIND_LABEL: Record<string, string> = {
  parent: "parent",
  dependency: "dependency",
  managed: "managed (a version pin)",
  "bom-import": "BOM import",
};

/** The build layer's accessible twin: one row per drawn member pair. */
const BUILD_LINK_COLUMNS: Column<BuildLink>[] = [
  { key: "from", header: "Member", mono: true, cell: (l) => l.from, sortValue: (l) => l.from },
  { key: "to", header: "Builds against", mono: true, cell: (l) => l.to, sortValue: (l) => l.to },
  {
    key: "kinds",
    header: "Kind",
    cell: (l) => l.kinds.map((k) => BUILD_KIND_LABEL[k] ?? k).join(", "),
    sortValue: (l) => l.kinds.join(","),
  },
  {
    key: "artifacts",
    header: "Artifact",
    mono: true,
    cell: (l) => l.artifacts.join(", "),
    sortValue: (l) => l.artifacts.join(","),
  },
  {
    key: "references",
    header: "References",
    numeric: true,
    cell: (l) => l.references,
    sortValue: (l) => l.references,
  },
];

const HINT_COLUMNS: Column<CrossContextHint>[] = [
  { key: "member", header: "Member", mono: true, cell: (h) => h.member, sortValue: (h) => h.member },
  {
    key: "contexts",
    header: "Contexts",
    cell: (h) => h.contexts.join(", "),
    sortValue: (h) => h.contexts.length,
  },
  {
    key: "libraries",
    header: "Model libraries",
    cell: (h) => (
      <ul className={styles.reasons}>
        {h.libraries.map((l) => (
          <li key={l.artifact}>
            <span className="mono">{l.artifact}</span> <span className="muted">from {l.member}</span>
          </li>
        ))}
      </ul>
    ),
    sortValue: (h) => h.libraries.length,
  },
];

/** The cross-context model hint (S-464, CR-148 §3.2 D; CR-203 §3.2 D item 9) —
 *  a REPORT, never an edge: nothing here reaches the canvas, whatever the build
 *  toggle says. A review hint, never a failure: its badge says so, and its
 *  action points at the member's build manifest. */
function CrossContextHintCard({ hints }: { hints: CrossContextHint[] }) {
  if (hints.length === 0) return null;
  return (
    <Widget
      title="Cross-context model hint"
      badge={<Badge tone="muted">Review hint</Badge>}
      copy={crossContextHint}
      figure={
        <div className={styles.figure}>
          <p className={styles.statement}>{SERVICE_MAP_TEXT.hintFigure(hints.length)}</p>
        </div>
      }
    >
      <p className="muted">{SERVICE_MAP_TEXT.hintNaming}</p>
      <DataTable
        caption="Members depending on two or more contexts' model libraries"
        columns={HINT_COLUMNS}
        rows={hints}
        rowKey={(h) => h.member}
        pageSize={DEFAULT_TABLE_PAGE_SIZE}
      />
    </Widget>
  );
}

/** The build headline on the coverage tab (S-464, frontend-design §4.17;
 *  CR-203 §3.2 D item 10) — its own widget, after every runtime board,
 *  rendering the server's composed lines (BR-51) and never a figure of its own.
 *  An absent headline is stated as an absence (no member holds a build
 *  manifest), never left as a missing widget the reader cannot tell apart from
 *  a failed read. */
function BuildDependencyCard({ headline }: { headline?: BuildDependencyHeadline }) {
  if (!headline) {
    return (
      <Widget title="Build dependencies" copy={buildDependencies} state={{ unread: 0 }} absence={COVERAGE_TEXT.buildAbsent} />
    );
  }
  const unread = headline.members.unread ?? [];
  const reasons = headline.members.unread_reasons ?? {};
  // Own keys only: a member named `constructor` must never read an inherited value.
  const reasonOf = (member: string) => (Object.hasOwn(reasons, member) ? reasons[member] : undefined);
  // One node or none: a list of `false`s would still render an empty evidence
  // part, which takes a gap in the frame.
  const hasEvidence =
    headline.platform_apart !== undefined ||
    headline.platform_candidates.length > 0 ||
    headline.collisions.length > 0 ||
    unread.length > 0;
  return (
    <Widget
      title="Build dependencies"
      copy={buildDependencies}
      state={{ unread: unread.length }}
      figure={
        <div className={styles.figure}>
          <p className={styles.statement}>{headline.summary}</p>
        </div>
      }
    >
      {hasEvidence && (
        <>
          {headline.platform_apart && (
            <p className="muted">
              Declared platform <span className="mono">{headline.platform_apart.members.join(", ")}</span>{" "}
              — counted apart: {headline.platform_apart.summary}
            </p>
          )}
          {headline.platform_candidates.length > 0 && (
            <p className="muted">
              Platform candidates (a hint; nothing is classified until declared):{" "}
              {headline.platform_candidates.map((c, i) => (
                <span key={c.member}>
                  {i > 0 && ", "}
                  <span className="mono">{c.member}</span> ({c.in_degree} of {c.of})
                </span>
              ))}
            </p>
          )}
          {headline.collisions.length > 0 && (
            <p className="muted">
              Produced by more than one member, so resolved to neither:{" "}
              {headline.collisions.map((c, i) => (
                <span key={c.artifact}>
                  {i > 0 && ", "}
                  <span className="mono">{c.artifact}</span> ({c.producers.join(", ")})
                </span>
              ))}
            </p>
          )}
          {unread.length > 0 && (
            <p className="muted">
              Build facts could not be read for{" "}
              {unread.map((member, i) => (
                <span key={member}>
                  {i > 0 && ", "}
                  <span className="mono">{member}</span>
                  {reasonOf(member) && ` (${reasonOf(member)})`}
                </span>
              ))}{" "}
              — their build dependencies are unknown, not absent.
            </p>
          )}
        </>
      )}
    </Widget>
  );
}

// ── The declared layer (S-461, FR-WS-31) ─────────────────────────────────────

/** Where a bound call's base path was committed, in words. */
const BASE_ORIGIN_LABEL: Record<string, string> = {
  "deploy-overlay": "Deploy overlay",
  "application-config": "Application configuration",
};

/** The other end of a declared link, as a cell: a member by name, or a named
 *  external by name WITH its identity — two externals can share a name. */
function counterparty(to: DeclaredLink["to"]) {
  return to.kind === "member" ? (
    <span className="mono">{to.member}</span>
  ) : (
    <>
      {to.name} <span className="muted">(named external</span>{" "}
      <span className="mono muted">{to.external}</span>
      <span className="muted">)</span>
    </>
  );
}

function counterpartyText(to: DeclaredLink["to"]): string {
  return to.kind === "member" ? to.member : `${to.name} (${to.external})`;
}

/** The declared layer's accessible twin: one row per drawn link. */
const DECLARED_LINK_COLUMNS: Column<DeclaredLink>[] = [
  { key: "from", header: "Member", mono: true, cell: (l) => l.from, sortValue: (l) => l.from },
  {
    key: "to",
    header: "Declares a contract to",
    cell: (l) => counterparty(l.to),
    sortValue: (l) => counterpartyText(l.to),
  },
  {
    key: "documents",
    header: "Documents",
    numeric: true,
    cell: (l) => l.contracts.length,
    sortValue: (l) => l.contracts.length,
  },
  {
    key: "bound",
    header: "Calls bound",
    numeric: true,
    // A member link has no join: it is not zero calls bound, it is no question
    // asked (NFR-CC-04).
    cell: (l) => (l.to.kind === "member" ? <span className="muted">—</span> : l.bound.length),
    sortValue: (l) => l.bound.length,
  },
];

/** One document behind a declared link, as a row of the one Documents table. */
interface DeclaredDocumentRow {
  link: DeclaredLink;
  contract: DeclaredLink["contracts"][number];
}

/** One call bound to a link's external, as a row of the one Bound calls table. */
interface BoundCallRow {
  link: DeclaredLink;
  call: BoundCall;
}

/** The two columns every flattened declared row leads with (CR-203 §3.2 D
 *  item 8): the declaring member and its counterparty — what the per-link
 *  disclosure's summary used to say once for all of its rows. */
function declaredEndColumns<R extends { link: DeclaredLink }>(): Column<R>[] {
  return [
    { key: "member", header: "Member", mono: true, cell: (r) => r.link.from, sortValue: (r) => r.link.from },
    {
      key: "counterparty",
      header: "Counterparty",
      cell: (r) => counterparty(r.link.to),
      sortValue: (r) => counterpartyText(r.link.to),
    },
  ];
}

/** One document behind a declared link: what it is, and why it names the
 *  counterparty — the identity score, or the external it groups into. */
const DECLARED_DOCUMENT_COLUMNS: Column<DeclaredDocumentRow>[] = [
  ...declaredEndColumns<DeclaredDocumentRow>(),
  {
    key: "document",
    header: "Document",
    mono: true,
    cell: ({ contract: c }) => c.document,
    sortValue: ({ contract: c }) => c.document,
  },
  {
    key: "identity",
    header: "Declares",
    cell: ({ contract: c }) =>
      c.target.kind === "member" ? (
        <>
          Document identity: {c.target.shared} of {c.target.total} operations match{" "}
          <span className="mono">{c.target.member}</span>&apos;s own{" "}
          <span className="mono">{c.target.document}</span>
        </>
      ) : (
        <>
          Named external {c.target.name} <span className="mono muted">{c.target.external}</span>
        </>
      ),
    sortValue: ({ contract: c }) => c.target.kind,
  },
];

/** One call bound to a link's external: the matched operation and the base
 *  path's source — the evidence the join rests on. */
const BOUND_CALL_COLUMNS: Column<BoundCallRow>[] = [
  ...declaredEndColumns<BoundCallRow>(),
  {
    key: "call",
    header: "Call",
    mono: true,
    cell: ({ call: b }) => b.target,
    sortValue: ({ call: b }) => b.target,
  },
  {
    key: "operation",
    header: "Matched operation",
    mono: true,
    cell: ({ call: b }) => b.operation,
    sortValue: ({ call: b }) => b.operation,
  },
  {
    key: "base",
    header: "Base path",
    mono: true,
    // An empty base path is a base URL with no path — say so rather than render
    // an empty cell that reads as "not known".
    cell: ({ call: b }) => (b.base.path === "" ? <span className="muted">none (host only)</span> : b.base.path),
    sortValue: ({ call: b }) => b.base.path,
  },
  {
    key: "source",
    header: "Base-path source",
    cell: ({ call: b }) => (
      <ul className={styles.reasons}>
        {b.base.sources.map((src) => (
          <li key={`${src.file}:${src.key}`}>
            {BASE_ORIGIN_LABEL[b.base.origin] ?? b.base.origin} ·{" "}
            <span className="mono">{src.file}</span> · <span className="mono">{src.key}</span>
          </li>
        ))}
      </ul>
    ),
    sortValue: ({ call: b }) => b.base.origin,
  },
];

/** The named-external registry: name AND identity, since names repeat. */
const EXTERNAL_COLUMNS: Column<DeclaredLayerExternal>[] = [
  {
    key: "name",
    header: "Named external",
    cell: (e) => (
      <>
        {e.name} <span className="mono muted">{e.id}</span>
      </>
    ),
    sortValue: (e) => e.name,
  },
  {
    key: "declared_by",
    header: "Declared by",
    mono: true,
    cell: (e) => (e.declared_by.length > 0 ? e.declared_by.join(", ") : <span className="muted">—</span>),
    sortValue: (e) => e.declared_by.length,
  },
  {
    key: "stand_ins",
    header: "Stood in for by",
    mono: true,
    cell: (e) => (e.stand_ins.length > 0 ? e.stand_ins.join(", ") : <span className="muted">—</span>),
    sortValue: (e) => e.stand_ins.length,
  },
  {
    key: "copies",
    header: "Copies",
    numeric: true,
    cell: (e) => e.copies.length,
    sortValue: (e) => e.copies.length,
  },
];

type DeclaredLayerExternal = DeclaredLayer["externals"][number];

/** The declared layer's twin and its evidence (S-461; CR-203 §3.2 D item 8) —
 *  the edge detail, flat: one contracts table, then ONE Documents table (each
 *  document with its identity score or external) and ONE Bound calls table
 *  (each call matched to an external, with its operation and base-path
 *  source), every row naming its member and counterparty, then the named
 *  externals. No per-link disclosure: one table per kind of fact. */
function DeclaredContractsCard({ layer, join }: { layer: DeclaredLayer; join?: BoundExternal }) {
  const documents: DeclaredDocumentRow[] = layer.links.flatMap((link) =>
    link.contracts.map((contract) => ({ link, contract })),
  );
  const calls: BoundCallRow[] = layer.links.flatMap((link) => link.bound.map((call) => ({ link, call })));
  // Calls are matched only against a named external; with none drawn, the
  // figure states no count of them (the twin's "Calls bound" reads "—").
  const anyExternal = layer.links.some((l) => l.to.kind === "external");
  return (
    <Widget
      title="Declared contracts"
      copy={declaredContracts}
      state={{ documents: documents.length }}
      figure={
        <div className={styles.figure}>
          <p className={styles.statement}>
            {SERVICE_MAP_TEXT.declaredFigure(layer.links.length, documents.length, anyExternal ? calls.length : null)}
          </p>
        </div>
      }
    >
      {/* The server's composed join line carries wire tokens, so it is the
          evidence, verbatim (BR-51), never the figure. */}
      {join && <p className="muted mono">{join.headline.summary}</p>}
      {/* A relation can name externals and declare nothing: a declared `mock`
          stands in for an external no member vendors. Then there is no link to
          tabulate, and an empty twin table would read as a table that failed to
          fill (NFR-CC-04) — so it says what is true instead. */}
      {layer.links.length === 0 ? (
        <p className="muted">{SERVICE_MAP_TEXT.noDeclaredLinks}</p>
      ) : (
        <>
          <DataTable
            caption="Declared contracts (the accessible twin of the declared layer)"
            columns={DECLARED_LINK_COLUMNS}
            rows={layer.links}
            rowKey={(l) => `${l.from}->${counterpartyText(l.to)}`}
            pageSize={DEFAULT_TABLE_PAGE_SIZE}
          />
          <DataTable
            caption="Documents by which each member declares a contract with its counterparty"
            columns={DECLARED_DOCUMENT_COLUMNS}
            rows={documents}
            rowKey={(r) => `${r.link.from}->${counterpartyText(r.link.to)}:${r.contract.document}`}
            pageSize={DEFAULT_TABLE_PAGE_SIZE}
          />
        </>
      )}
      {calls.length > 0 && (
        <DataTable
          caption="Calls bound to a named external — each still counted as no provider here"
          columns={BOUND_CALL_COLUMNS}
          rows={calls}
          rowKey={(r, i) => `${r.link.from}:${r.call.from.symbol}:${r.call.target}:${i}`}
          pageSize={DEFAULT_TABLE_PAGE_SIZE}
        />
      )}
      {layer.externals.length > 0 && (
        <DataTable
          caption="Named externals (APIs no member's own spec is)"
          columns={EXTERNAL_COLUMNS}
          rows={layer.externals}
          rowKey={(e) => e.id}
          pageSize={DEFAULT_TABLE_PAGE_SIZE}
        />
      )}
    </Widget>
  );
}

/** The declared relations on the coverage tab (S-461, frontend-design §4.17;
 *  CR-203 §3.2 D item 10) — their own widget after every runtime board,
 *  rendering the server's composed lines (BR-51) and never a figure of its own.
 *  Absent both, no widget: FR-UI-29's CR-203 amendment keeps AC8's condition,
 *  "no declared widget without a vendored spec". */
function DeclaredRelationsCard({ dashboard }: { dashboard: CoverageDashboard }) {
  const { declaredContracts, boundExternal } = dashboard;
  if (!declaredContracts && !boundExternal) return null;
  // The figure is in plain words from the headlines' counts; the server's
  // composed lines carry wire tokens (`no-provider-in-workspace`,
  // `egress_resolution`), so they are the evidence, verbatim (FR-UI-39).
  return (
    <Widget
      title="Declared contracts and named externals"
      copy={declaredRelations}
      figure={
        <div className={styles.figure}>
          {declaredContracts && (
            <p>{COVERAGE_TEXT.declaredPairs(declaredContracts.declared_contract_pairs, declaredContracts.named_externals)}</p>
          )}
          {boundExternal && (
            <p className={styles.statement}>
              {COVERAGE_TEXT.externalsMatched(boundExternal.bound_external, boundExternal.no_provider_rows)}
            </p>
          )}
        </div>
      }
    >
      {declaredContracts && <p className="muted mono">{declaredContracts.summary}</p>}
      {boundExternal && (
        <>
          <p className="muted mono">{boundExternal.summary}</p>
          <p className="muted">{COVERAGE_TEXT.externalStaysApart}</p>
        </>
      )}
    </Widget>
  );
}

const LINK_COLUMNS: Column<ServiceLink>[] = [
  { key: "from", header: "Consumer", mono: true, cell: (l) => l.from, sortValue: (l) => l.from },
  { key: "to", header: "Provider", mono: true, cell: (l) => l.to, sortValue: (l) => l.to },
  {
    key: "relation",
    header: "Binding",
    cell: (l) => armLabel(l.relation),
    sortValue: (l) => l.relation,
  },
  {
    key: "count",
    header: "Bindings",
    numeric: true,
    cell: (l) => l.count,
    sortValue: (l) => l.count,
  },
];

/** The Provenance column (S-419, CR-132 AC4) — the accessible twin of the canvas's
 *  stroke channel, so the distinction ADR-64 requires survives for a reader who
 *  never sees the canvas at all.
 *
 *  Always a per-kind BREAKDOWN, one row per kind actually present, never one
 *  label for the line: a link aggregating an observed binding and an admitted one
 *  is neither, and a single word there is false about half of it. Kinds at zero
 *  are omitted from the cell (a column of "literal 2 · config-bound 0 · …" is
 *  noise) but every kind is still carried in the model, which is where a caller
 *  reads a zero from. */
const PROVENANCE_COLUMN: Column<ServiceLink> = {
  key: "provenance",
  header: "Provenance",
  cell: (l) => (
    <ul className={styles.reasons}>
      {LINK_PROVENANCE_KINDS.filter((k) => l.provenance[k] > 0).map((k) => (
        <li key={k}>
          {LINK_PROVENANCE_LABEL[k]} <Badge tone="muted">{l.provenance[k]}</Badge>
        </li>
      ))}
    </ul>
  ),
  // Sorted by how many of the line's bindings are NOT observed: the rows a reader
  // opened this column for come to the top.
  sortValue: (l) => l.count - l.provenance.literal,
};

/** The columns the bindings table renders.
 *
 *  A function, not a constant, because the Provenance column appears only when
 *  the workspace HAS a non-literal binding. FR-UI-29 keeps a literal-only
 *  workspace free of every provenance rendering (CR-132 AC6; re-asserted on
 *  the S-614 layout), and an always-present column of "Written at the call site" on every row would
 *  break that while telling a reader nothing. */
function linkColumns(withProvenance: boolean): Column<ServiceLink>[] {
  return withProvenance ? [...LINK_COLUMNS, PROVENANCE_COLUMN] : LINK_COLUMNS;
}

/** The evidence detail's columns (S-419, CR-132 AC4; FR-WS-19 AC2/AC6), over
 *  grouped rows (S-614): each states one fact once, with how many calls it
 *  stands for and what its refusal asks of the reader. "Calls" is explained in
 *  the widget's what rather than glossed: a `Term` inside the sort button
 *  would nest one interactive element in another. */
const EVIDENCE_COLUMNS: Column<EvidenceGroup>[] = [
  {
    key: "end",
    header: "End",
    cell: (r) => `${r.end === "consumer" ? "Consumer" : "Provider"} · ${r.member}`,
    sortValue: (r) => `${r.end}:${r.member}`,
  },
  { key: "key", header: "Key", mono: true, cell: (r) => r.key, sortValue: (r) => r.key },
  {
    key: "value",
    header: "Committed value",
    mono: true,
    // A refusal has NO value, and an empty cell would read as an empty string the
    // sources proved. It states the refusal instead (NFR-CC-04).
    cell: (r) =>
      r.value === null ? (
        <span className="muted">
          {/* An unknown token is shown verbatim, never as an empty cell. */}
          {r.refusal === null ? "—" : (CONFIG_REFUSAL_LABEL[r.refusal] ?? r.refusal)}
        </span>
      ) : (
        r.value
      ),
    sortValue: (r) => r.value ?? "",
  },
  {
    key: "profiles",
    header: "Profiles",
    // `unprofiled` is stated in words rather than left to an empty profile list:
    // an estate that genuinely declares a `default` profile must stay
    // distinguishable from one that declares none (the `ProfiledValue` contract).
    cell: (r) => {
      const parts = [...r.profiles];
      if (r.unprofiled) parts.push("unprofiled source");
      return parts.length > 0 ? parts.join(", ") : <span className="muted">—</span>;
    },
    sortValue: (r) => r.profiles.join(","),
  },
  {
    key: "sources",
    header: "Defining sources",
    mono: true,
    cell: (r) =>
      r.sources.length > 0 ? r.sources.join(", ") : <span className="muted">—</span>,
    sortValue: (r) => r.sources.join(","),
  },
  { key: "calls", header: "Calls", numeric: true, cell: (r) => r.calls, sortValue: (r) => r.calls },
  {
    key: "action",
    header: "What you can do",
    // The one `none` a row has is a value that arrives at runtime.
    cell: (r) => <ActionCell action={evidenceRowAction(r)} none={SERVICE_MAP_TEXT.arrivesAtRuntime} />,
  },
];

/** The evidence behind every non-literal link (S-419, CR-132 AC4; CR-203
 *  §3.2 D item 7).
 *
 *  Rendered only for the links that HAVE something to evidence, and the whole
 *  widget only when at least one does — the same gate as the legend section and
 *  the table column, so a literal-only workspace gains no evidence widget. The
 *  bindings filter narrows it with the table (`shown`); its figure states how
 *  many of the links with evidence are shown. Each link's rows are grouped, so
 *  one fact is one row with its Calls count (FR-UI-42). */
function BindingEvidence({ links, shown }: { links: ServiceLink[]; shown: ServiceLink[] }) {
  const admitted = links.filter(hasNonLiteralBinding);
  if (admitted.length === 0) return null;
  const visible = shown.filter(hasNonLiteralBinding).map((l) => ({ link: l, rows: groupEvidence(linkEvidence(l)) }));
  const shownRows = visible.flatMap((v) => v.rows);
  const state = {
    define: shownRows.filter((r) => r.refusal === "missing-key").length,
    replace: shownRows.filter((r) => r.refusal === "placeholder-value").length,
  };
  return (
    <Widget
      title="Binding evidence"
      copy={bindingEvidence}
      state={state}
      figure={
        <div className={styles.figure}>
          <p className={styles.statement}>{SERVICE_MAP_TEXT.evidenceShown(visible.length, admitted.length)}</p>
        </div>
      }
    >
      {visible.map(({ link: l, rows }) => (
        <details key={`${l.from}->${l.to}:${l.relation}`}>
          <summary>
            <span className="mono">
              {l.from} → {l.to}
            </span>{" "}
            · {armLabel(l.relation)}
          </summary>
          {rows.length === 0 ? (
            // Reachable two ways, and the wording must not pick one of them:
            // a link is non-literal when any binding is `unstated` (an end
            // whose value never arrived — genuinely not stated), and ALSO
            // when a `config-bound` end arrives with an empty `bound: []`
            // (stated, but naming no key). Saying "its provenance was not
            // stated" would be false in the second case — a fabricated
            // explanation in place of a fabricated value (NFR-CC-04). So it
            // states what is observable — no key reached this view — and
            // draws no conclusion about why.
            <p className="muted">{SERVICE_MAP_TEXT.noKeyNamed}</p>
          ) : (
            <DataTable
              caption={`Configuration evidence for ${l.from} → ${l.to} (${armLabel(l.relation)})`}
              columns={EVIDENCE_COLUMNS}
              rows={rows}
              rowKey={(r, i) => `${r.end}:${r.key}:${i}`}
              pageSize={DEFAULT_TABLE_PAGE_SIZE}
            />
          )}
        </details>
      ))}
    </Widget>
  );
}

/** The bindings filter's controls (CR-203 §3.2 D item 6, FR-UI-42): text over
 *  the two services, a binding kind, and — only when the Provenance column
 *  exists — a provenance kind. */
function LinkFilterControls({
  filter,
  onChange,
  withProvenance,
}: {
  filter: LinkFilter;
  onChange: (filter: LinkFilter) => void;
  withProvenance: boolean;
}) {
  return (
    <div className={styles.filters} role="search" aria-label="Filter the cross-service bindings">
      <TextField
        label={SERVICE_MAP_TEXT.filterText}
        hint={SERVICE_MAP_TEXT.filterTextHint}
        type="search"
        value={filter.text}
        onChange={(e) => onChange({ ...filter, text: e.target.value })}
      />
      <SelectField
        label={SERVICE_MAP_TEXT.filterKind}
        value={filter.kind}
        onChange={(e) => onChange({ ...filter, kind: e.target.value as LinkFilter["kind"] })}
      >
        <option value="all">{SERVICE_MAP_TEXT.anyKind}</option>
        {BINDING_KIND_FILTERS.map((k) => (
          <option key={k} value={k}>
            {BINDING_KIND_LABEL[k]}
          </option>
        ))}
      </SelectField>
      {withProvenance && (
        <SelectField
          label={SERVICE_MAP_TEXT.filterProvenance}
          value={filter.provenance}
          onChange={(e) => onChange({ ...filter, provenance: e.target.value as LinkFilter["provenance"] })}
        >
          <option value="all">{SERVICE_MAP_TEXT.anyProvenance}</option>
          {LINK_PROVENANCE_KINDS.map((k) => (
            <option key={k} value={k}>
              {LINK_PROVENANCE_LABEL[k]}
            </option>
          ))}
        </SelectField>
      )}
    </div>
  );
}

/** The bindings table (S-250; CR-203 §3.2 D item 6) — the accessible twin of
 *  the map, filtered. Always rendered: with no binding resolved it states that
 *  absence in its figure row, where the centred empty state used to stand. */
function CrossServiceBindings({
  links,
  shown,
  topics,
  withProvenance,
  filter,
  onFilter,
}: {
  links: ServiceLink[];
  shown: ServiceLink[];
  topics: number;
  withProvenance: boolean;
  filter: LinkFilter;
  onFilter: (filter: LinkFilter) => void;
}) {
  if (links.length === 0) {
    return (
      <Widget title="Cross-service bindings" copy={crossServiceBindings} absence={SERVICE_MAP_TEXT.noBindings(topics)} />
    );
  }
  return (
    <Widget
      title="Cross-service bindings"
      copy={crossServiceBindings}
      figure={
        <div className={styles.figure}>
          <p className={styles.statement} role="status">
            {SERVICE_MAP_TEXT.bindingsShown(shown.length, links.length)}
          </p>
        </div>
      }
    >
      <LinkFilterControls filter={filter} onChange={onFilter} withProvenance={withProvenance} />
      {shown.length === 0 ? (
        <p className="muted">{SERVICE_MAP_TEXT.noneMatch}</p>
      ) : (
        <DataTable
          caption="Cross-service bindings (the accessible twin of the service map)"
          columns={linkColumns(withProvenance)}
          rows={shown}
          rowKey={(l) => `${l.from}->${l.to}:${l.relation}`}
          pageSize={DEFAULT_TABLE_PAGE_SIZE}
        />
      )}
    </Widget>
  );
}

function ServiceMap({
  services,
  providers,
  topics,
  build,
  coverage,
  deps,
  depsError,
}: {
  services: ServiceMember[];
  providers: XserviceRouteProviders;
  topics: MemberTopics[];
  build?: BuildDependencyHeadline;
  coverage: CrossServiceCoverage;
  deps: XserviceBuildDeps | null;
  depsError: Error | null;
}) {
  const { selectMember } = useWorkspace();
  // OFF by default (CR-148 §3.2 D, BR-58): runtime coupling stays the picture a
  // reader lands on, and the build layer is something they ask for.
  const [showBuild, setShowBuild] = useState(false);
  const map = buildServiceMap(services, providers.providers, topics);
  const layer = showBuild && deps ? buildLayer(deps, services) : null;
  // Drawn whenever the status carries the relation (S-461); `null` otherwise.
  const declared = declaredLayer(coverage.declared_contracts, coverage.bound_external, services);
  // With neither layer the canvas gets the runtime set itself, not a copy — the
  // map is the pre-S-464 map, object for object.
  const loaded =
    layer || declared
      ? {
          nodes: declared ? { ...map.loaded.nodes, ...declared.nodes } : map.loaded.nodes,
          edges: [...map.loaded.edges, ...(declared?.edges ?? []), ...(layer?.edges ?? [])],
        }
      : map.loaded;
  /* The one gate on every rendering the provenance channel adds (S-419,
     CR-132 AC3/AC6). A workspace whose bindings were all observed at call sites
     has nothing to distinguish, so it gains none of them — no legend section,
     no table column, no provenance filter, no evidence widget. */
  const anyAdmitted = map.links.some(hasNonLiteralBinding);
  // The bindings filter (S-614, FR-UI-42) narrows the table and the evidence,
  // never `loaded`: the canvas always draws the whole map. The provenance
  // choice applies only while its control exists.
  const [filter, setFilter] = useState<LinkFilter>(NO_LINK_FILTER);
  const shown = filterLinks(map.links, anyAdmitted ? filter : { ...filter, provenance: "all" });

  // One stack for the tab (S-614, FR-UI-40): the map with its legend and notes
  // is its first child, and every widget below it sits at the stack's one gap.
  return (
    <WidgetStack>
      <div className={styles.panel}>
        {/* Clicking a service focuses its member: the shell selector switches to it and
            every other view re-fetches scoped to that member (frontend-design §4.16).
            A topic node is NOT a member, so clicking it selects nothing — `memberOfServiceId`
            returns null for a `topic:` id, which is why the two namespaces are distinct. */}
        <GraphCanvas
          loaded={loaded}
          selection={{ seed: null, focusId: null, lockedId: null, locatedId: null, depth: 0 }}
          onNodeClick={(id) => {
            const member = memberOfServiceId(id);
            if (member) selectMember(member);
          }}
        />

        <details className={graphStyles.legend} open>
          <summary>Legend</summary>
          <div className={graphStyles.legendBody}>
            <span className={graphStyles.legendHeading}>Cross-service bindings</span>
            <ul className={graphStyles.legendList}>
              {RELATION_ARMS.map((arm) => (
                <EdgeRow type={arm} key={arm} />
              ))}
            </ul>
            {map.topics.length > 0 && (
              <>
                <span className={graphStyles.legendHeading}>Broker topics</span>
                <ul className={graphStyles.legendList}>
                  <EdgeRow type="publishes" />
                  <EdgeRow type="subscribes" />
                </ul>
              </>
            )}
            {/* Provenance is a SECOND channel over the arm hue, so both rows are
                drawn in one arm's colour and differ only in stroke — the same
                distinction the canvas makes. Rendered only when the workspace has
                something to distinguish (CR-132 AC3). */}
            {anyAdmitted && (
              <>
                <span className={graphStyles.legendHeading}>Provenance</span>
                <ul className={graphStyles.legendList}>
                  <EdgeRow type="route" dash="0" label="Written at the call site" />
                  <EdgeRow
                    type="route"
                    dash={ADMITTED_DASH.join(" ")}
                    label="Admitted from committed configuration"
                  />
                </ul>
                <p className={graphStyles.legendNote}>
                  The stroke says where a coupling came from; the hue still says which arm it
                  crosses. An admitted line was proved by a committed configuration value, not
                  observed at a call site — the table below states the split per coupling.
                </p>
              </>
            )}
            {/* The declared layer (S-461): its own edge class and node kind,
                rendered only when the relation exists. */}
            {declared && coverage.declared_contracts && (
              <>
                <span className={graphStyles.legendHeading}>Declared contracts</span>
                <ul className={graphStyles.legendList}>
                  {/* The edge row only when an edge is drawn — a legend entry for
                      a line the canvas never shows would describe nothing. */}
                  {declared.edges.length > 0 && (
                    <EdgeRow type={DECLARED_EDGE_TYPE} label="Declares a contract (a vendored spec)" />
                  )}
                  <li className={graphStyles.legendRow}>
                    <span
                      className={`${graphStyles.legendDot} ${graphStyles.legendDotArtifact}`}
                      aria-hidden="true"
                    />
                    <span>Named external — not a member (topics share this hue)</span>
                  </li>
                </ul>
                <p className={graphStyles.legendNote}>
                  Declared by a spec document a member holds and does not implement — never an
                  observed call, and counted apart from every binding above:{" "}
                  {coverage.declared_contracts.headline.summary}
                </p>
              </>
            )}
            {/* The build layer's toggle (S-464): rendered only when a relation
                exists, and unchecked until the reader checks it. */}
            {build && (
              <>
                <span className={graphStyles.legendHeading}>Build dependencies</span>
                <label className={graphStyles.check}>
                  <input
                    type="checkbox"
                    checked={showBuild}
                    onChange={(e) => setShowBuild(e.target.checked)}
                  />{" "}
                  Draw what each member builds against
                </label>
                {showBuild && (
                  <ul className={graphStyles.legendList}>
                    <EdgeRow type={BUILD_EDGE_TYPE} label="Builds against (from its build manifest)" />
                  </ul>
                )}
                <p className={graphStyles.legendNote}>
                  Drawn in its own class and counted apart from every binding above:{" "}
                  {build.summary}
                </p>
              </>
            )}
          </div>
        </details>

        {/* A failed read is stated whatever the toggle says: the cross-context hint is
            read from the same answer and shown with the toggle off, so an unstated
            failure would read as "no hint" (NFR-CC-04). Only a workspace whose status
            carries a build headline ever reads the relation, so a manifest-less one
            never reaches this. */}
        {depsError ? (
          <ErrorPanel>
            The build relation could not be read: {depsError.message} — the build layer and the
            cross-context model hint are unknown, not absent.
          </ErrorPanel>
        ) : (
          showBuild && !deps && <LoadingState label="Reading the build relation…" />
        )}

        {layer && layer.collapsed.length > 0 && (
          <p className="muted">
            Platform members collapsed:{" "}
            {layer.collapsed.map((c, i) => (
              <span key={c.member} data-testid="collapsed-platform">
                {i > 0 && ", "}
                <span className="mono">{c.member}</span> ({c.inbound}{" "}
                {c.inbound === 1 ? "member builds" : "members build"} against it)
              </span>
            ))}{" "}
            — declared <span className="mono">platform</span>, so their inbound build edges are
            counted apart and not drawn.
          </p>
        )}

        {map.topics.length > 0 && (
          <p className="muted">
            {map.topics.length} topic{map.topics.length === 1 ? "" : "s"} · a topic is drawn as its
            own node, so a coupling reads as{" "}
            <span className="mono">publisher → topic → subscriber</span>. A topic with no subscriber
            yet is still drawn — it is unconsumed, not absent.
          </p>
        )}

        {map.awaitingIndex.length > 0 && (
          <p className="muted">
            Awaiting index: <span className="mono">{map.awaitingIndex.join(", ")}</span> — drawn muted;
            their couplings are unknown, not absent.
          </p>
        )}

        {map.degraded.length > 0 && (
          <p className="muted">
            Unavailable: <span className="mono">{map.degraded.join(", ")}</span> — these members could
            not be read (a fault, not an empty index); their couplings are unknown.
          </p>
        )}
      </div>

      {/* A map with topics but no resolved bindings is NOT empty — a published topic is
          real coupling the user can see and act on, even before anything subscribes to
          it (S-256, FR-WS-11) — so the absence the widget states names the topics. */}
      <CrossServiceBindings
        links={map.links}
        shown={shown}
        topics={map.topics.length}
        withProvenance={anyAdmitted}
        filter={filter}
        onFilter={setFilter}
      />

      <BindingEvidence links={map.links} shown={shown} />

      {declared && <DeclaredContractsCard layer={declared} join={coverage.bound_external} />}

      {layer && layer.links.length > 0 && (
        <Card title="Build dependencies">
          <DataTable
            caption="Build dependencies (the accessible twin of the build layer)"
            columns={BUILD_LINK_COLUMNS}
            rows={layer.links}
            rowKey={(l) => `${l.from}->${l.to}`}
            pageSize={DEFAULT_TABLE_PAGE_SIZE}
          />
        </Card>
      )}

      {deps && <CrossContextHintCard hints={deps.cross_context} />}
    </WidgetStack>
  );
}

// ── Cross-service impact ─────────────────────────────────────────────────────

const IMPACT_COLUMNS: Column<ImpactEntry>[] = [
    { key: "name", header: "Symbol", mono: true, cell: (e) => e.name, sortValue: (e) => e.name },
    { key: "kind", header: "Kind", cell: (e) => e.kind, sortValue: (e) => e.kind },
    {
      key: "file",
      header: "File",
      mono: true,
      cell: (e) => e.file ?? <span className="muted">n/a</span>,
      sortValue: (e) => e.file ?? "",
    },
  {
    key: "distance",
    header: "Distance",
    numeric: true,
    cell: (e) => e.distance,
    sortValue: (e) => e.distance,
  },
];

function ImpactTable({ label, impact }: { label: string; impact: ImpactResult }) {
  const rows = impact.upstream;
  return (
    <Card title={label}>
      {impact.resolved === null ? (
        <p className="muted">
          <span className="mono">{impact.query}</span> resolves to no symbol here.
        </p>
      ) : rows.length === 0 ? (
        <p className="muted">No callers reach it in this member.</p>
      ) : (
        <DataTable
          caption={`${impact.upstream_label} — ${label}`}
          columns={IMPACT_COLUMNS}
          rows={rows}
          rowKey={(e) => e.symbol}
          pageSize={DEFAULT_TABLE_PAGE_SIZE}
        />
      )}
    </Card>
  );
}

function CrossServicePanel({ far }: { far: CrossServiceImpact }) {
  return (
    <ImpactTable
      label={`${far.member} — reached across a ${armLabel(far.via.relation)} binding`}
      impact={far.impact}
    />
  );
}

function ImpactPanel() {
  const [symbol, setSymbol] = useState("");
  const [query, setQuery] = useState("");
  const impact = useApiResource<XserviceImpact | null>(
    () => (query ? fetchWorkspaceImpact(query) : Promise.resolve(null)),
    [query],
  );

  return (
    <div className={styles.panel}>
      <form
        className={styles.impactForm}
        onSubmit={(e) => {
          e.preventDefault();
          setQuery(symbol.trim());
        }}
      >
        <TextField
          label="Symbol"
          hint="A symbol name or canonical SCIP symbol; its impact is traced in every member and across every resolved binding."
          value={symbol}
          onChange={(e) => setSymbol(e.target.value)}
        />
        <Button type="submit" disabled={symbol.trim() === ""}>
          Trace impact
        </Button>
      </form>

      {query === "" ? (
        <EmptyState message="Name a symbol to trace its impact across services." />
      ) : (
        <AsyncResource resource={impact} loadingLabel="Tracing the cross-service impact…">
          {(model) =>
            model === null ? null : (
              <>
                {model.seed.map((m) =>
                  m.result ? (
                    <ImpactTable key={m.member} label={`${m.member} (seed)`} impact={m.result} />
                  ) : (
                    <Card key={m.member} title={`${m.member} (seed)`}>
                      <p className="muted">Degraded: {m.error ?? "this member could not be read"}.</p>
                    </Card>
                  ),
                )}
                {/* CR-125/BR-53: the residue rides EVERY reachability answer,
                    not only the empty one. An empty answer over a non-zero
                    residue is UNRESOLVED, named with its count, never a bare
                    empty set — and a partial answer is still partial. The text
                    is the payload's own composed line, so this view cannot state
                    a figure the API did not compute. */}
                {model.unresolved_egress && (
                  <Card title="Unresolved egress">
                    <p className="muted">{model.unresolved_egress.summary}</p>
                  </Card>
                )}
                {model.cross_service.length === 0 ? (
                  model.unresolved_egress ? null : (
                    <EmptyState message="No cross-service impact — no resolved binding reaches this symbol from another service. (An unmaterialized binding is unknown, not absent — see Cross-service coverage.)" />
                  )
                ) : (
                  model.cross_service.map((far) => (
                    <CrossServicePanel key={`${far.member}:${far.via.from.symbol}`} far={far} />
                  ))
                )}
              </>
            )
          }
        </AsyncResource>
      )}
    </div>
  );
}
