/*
 * The Workspace tab (S-250, CR-061, FR-UI-29; frontend-design §4.16/§4.17) — the
 * app-level cross-service surface, in workspace mode only.
 *
 * Three panels over the S-249 `/api/v1/workspace/*` read-models:
 *   - Service map — services as nodes, resolved cross-service bindings as edges,
 *     rendered through the UNCHANGED §4.4 ECharts canvas (`GraphCanvas`) with the
 *     same legend grammar. Clicking a service focuses its member (the shell
 *     selector switches, and every other view re-fetches scoped to it).
 *   - Cross-service coverage — the advisory 3-state bound/ambiguous/unbound board
 *     per relation arm, with unbound references grouped by reason.
 *   - Cross-service impact — a symbol's impact in its own member(s) plus each
 *     far-side impact stitched across a binding.
 *
 * The build layer (S-464, CR-148, FR-WS-33): when any member holds a build
 * manifest, the map's legend gains a toggle — OFF by default — that draws what
 * each member builds against in its own `build` edge class, with declared
 * platform members collapsed; a cross-context model hint lists members depending
 * on two or more contexts' model libraries, never as an edge; and the coverage
 * tab states the build headline apart from every runtime figure. A build
 * dependency is never a runtime coupling (BR-58), and a workspace with no build
 * manifest renders every panel exactly as before.
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
  BuildDependencyHeadline,
  CrossContextHint,
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
  Badge,
  Button,
  Callout,
  Card,
  DataTable,
  DEFAULT_TABLE_PAGE_SIZE,
  EmptyState,
  ErrorPanel,
  LoadingState,
  Tabs,
  TextField,
  type Column,
} from "../../components/index.ts";
import { useWorkspace } from "../../workspace/WorkspaceContext.tsx";
import { GraphCanvas } from "../graph/GraphCanvas.tsx";
import { ADMITTED_DASH } from "../graph/graphModel.ts";
import { EdgeRow } from "../graph/Legend.tsx";
import {
  ARM_LABEL,
  armLabel,
  buildCoverageDashboard,
} from "./coverageModel.ts";
import { CoveragePanel } from "./CoverageBoards.tsx";
import {
  BUILD_EDGE_TYPE,
  buildLayer,
  buildServiceMap,
  CONFIG_REFUSAL_LABEL,
  hasNonLiteralBinding,
  LINK_PROVENANCE_KINDS,
  LINK_PROVENANCE_LABEL,
  linkEvidence,
  memberOfServiceId,
  serviceMembers,
  type BuildLink,
  type EvidenceRow,
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
              />
            ),
          },
          {
            id: "coverage",
            label: "Cross-service coverage",
            panel: (
              <>
                <CoveragePanel dashboard={coverage} degraded={status.degraded_rollup} />
                <BuildDependencyCard headline={status.build_dependency} />
              </>
            ),
          },
          { id: "impact", label: "Cross-service impact", panel: <ImpactPanel /> },
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
}: {
  services: ServiceMember[];
  topics: MemberTopics[];
  /** The status payload's build headline — present only when some member holds
   *  a build manifest. Absent, the build layer does not exist: no toggle, no
   *  fetch, and the map renders exactly as it did before it (S-464). */
  build?: BuildDependencyHeadline;
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

/** The cross-context model hint (S-464, CR-148 §3.2 D) — a REPORT, never an
 *  edge: nothing here reaches the canvas, whatever the build toggle says. */
function CrossContextHintCard({ hints }: { hints: CrossContextHint[] }) {
  if (hints.length === 0) return null;
  return (
    <Card title="Cross-context model hint">
      <p className="muted">
        {hints.length} member{hints.length === 1 ? "" : "s"} depend on the model libraries of two
        or more bounded contexts — a context is named by its model library&apos;s coordinate,{" "}
        <span className="mono">&lt;group&gt;.&lt;context&gt;:kafka-models</span> or{" "}
        <span className="mono">&lt;context&gt;-kafka-models</span>. A hint for review, drawn as no
        edge: a build dependency is not a runtime coupling.
      </p>
      <DataTable
        caption="Members depending on two or more contexts' model libraries"
        columns={HINT_COLUMNS}
        rows={hints}
        rowKey={(h) => h.member}
        pageSize={DEFAULT_TABLE_PAGE_SIZE}
      />
    </Card>
  );
}

/** The build headline on the coverage tab (S-464, frontend-design §4.17) — its
 *  own card, after every runtime board, rendering the server's composed lines
 *  (BR-51) and never a figure of its own. Absent headline, no card. */
function BuildDependencyCard({ headline }: { headline?: BuildDependencyHeadline }) {
  if (!headline) return null;
  const unread = headline.members.unread ?? [];
  return (
    <Card title="Build dependencies">
      <p className="muted">
        What members build against, joined from their Maven/Gradle manifests. A build dependency,
        never a runtime coupling: no figure above counts it.
      </p>
      <p>{headline.summary}</p>
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
          Build facts could not be read for <span className="mono">{unread.join(", ")}</span> —
          their build dependencies are unknown, not absent.
        </p>
      )}
    </Card>
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
 *  workspace rendering byte-for-byte as it did before this story (CR-132 AC6),
 *  and an always-present column of "Written at the call site" on every row would
 *  break that while telling a reader nothing. */
function linkColumns(withProvenance: boolean): Column<ServiceLink>[] {
  return withProvenance ? [...LINK_COLUMNS, PROVENANCE_COLUMN] : LINK_COLUMNS;
}

/** The evidence detail's columns (S-419, CR-132 AC4; FR-WS-19 AC2/AC6). */
const EVIDENCE_COLUMNS: Column<EvidenceRow>[] = [
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
          {r.refusal === null ? "—" : CONFIG_REFUSAL_LABEL[r.refusal]}
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
];

/** The evidence behind every non-literal link (S-419, CR-132 AC4).
 *
 *  Rendered only for the links that HAVE something to evidence, and the whole
 *  card only when at least one does — the same gate as the legend section and the
 *  table column, so a literal-only workspace renders exactly as before. */
function BindingEvidence({ links }: { links: ServiceLink[] }) {
  const admitted = links.filter(hasNonLiteralBinding);
  if (admitted.length === 0) return null;
  return (
    <Card title="Binding evidence">
      <p className="muted">
        What admitted each coupling below, per end: the configuration key, the committed value,
        and the files that prove it. One row per overlay — a key its overlays spell differently
        proves several values, and every one of them is carried rather than one shown as though
        it were the value.
      </p>
      {admitted.map((l) => {
        const rows = linkEvidence(l);
        return (
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
              <p className="muted">
                No configuration key is named for this coupling, so there is nothing here to
                evidence it either way — its Provenance breakdown above says what is known.
              </p>
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
        );
      })}
    </Card>
  );
}

function ServiceMap({
  services,
  providers,
  topics,
  build,
  deps,
  depsError,
}: {
  services: ServiceMember[];
  providers: XserviceRouteProviders;
  topics: MemberTopics[];
  build?: BuildDependencyHeadline;
  deps: XserviceBuildDeps | null;
  depsError: Error | null;
}) {
  const { selectMember } = useWorkspace();
  // OFF by default (CR-148 §3.2 D, BR-58): runtime coupling stays the picture a
  // reader lands on, and the build layer is something they ask for.
  const [showBuild, setShowBuild] = useState(false);
  const map = buildServiceMap(services, providers.providers, topics);
  const layer = showBuild && deps ? buildLayer(deps, services) : null;
  // With the toggle off the canvas gets the runtime set itself, not a copy — the
  // map is the pre-S-464 map, object for object.
  const loaded = layer
    ? { nodes: map.loaded.nodes, edges: [...map.loaded.edges, ...layer.edges] }
    : map.loaded;
  /* The one gate on every rendering the provenance channel adds (S-419,
     CR-132 AC3/AC6). A workspace whose bindings were all observed at call sites
     has nothing to distinguish, so it renders exactly the DOM it rendered before
     this story — no legend section, no table column, no evidence card. */
  const anyAdmitted = map.links.some(hasNonLiteralBinding);

  return (
    <div className={styles.panel}>
      {/* A map with topics but no resolved bindings is NOT empty — a published topic is
          real coupling the user can see and act on, even before anything subscribes to
          it (S-256, FR-WS-11). Reporting it as "nothing here" would hide the very thing
          promoting topics to first-class nodes was meant to reveal. */}
      {map.links.length === 0 && map.topics.length === 0 ? (
        <EmptyState message="No cross-service bindings resolved yet — every service is drawn, and the Cross-service coverage tab reports why each reference has not bound." />
      ) : null}

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
              <span className="mono">{c.member}</span> ({c.inbound} member
              {c.inbound === 1 ? "" : "s"} build against it)
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

      {map.links.length > 0 && (
        <Card title="Cross-service bindings">
          <DataTable
            caption="Cross-service bindings (the accessible twin of the service map)"
            columns={linkColumns(anyAdmitted)}
            rows={map.links}
            rowKey={(l) => `${l.from}->${l.to}:${l.relation}`}
            pageSize={DEFAULT_TABLE_PAGE_SIZE}
          />
        </Card>
      )}

      <BindingEvidence links={map.links} />

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
    </div>
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
