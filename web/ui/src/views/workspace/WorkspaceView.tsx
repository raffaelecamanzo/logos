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
 * Honesty (NFR-CC-04, NFR-RA-05): an unbound reference is never drawn as an edge
 * (its absence is *reported* as coverage, not hidden); a member with no index is a
 * muted node, not a service with "no couplings"; a workspace with no bindings gets
 * the awaiting-data state, never a fabricated 100%.
 *
 * Every read here is a GET (ADR-28). In single-root mode this view is unreachable —
 * no nav item is rendered — and it says so honestly if navigated to by hand.
 */

import { useState } from "react";

import { AsyncResource, useApiResource } from "../../api/index.ts";
import {
  fetchWorkspaceBindings,
  fetchWorkspaceImpact,
  fetchWorkspaceStatus,
} from "../../api/workspaceClient.ts";
import type {
  CrossServiceImpact,
  ImpactEntry,
  ImpactResult,
  WorkspaceStatus,
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
  ScoreBar,
  Tabs,
  TextField,
  type Column,
} from "../../components/index.ts";
import { useWorkspace } from "../../workspace/WorkspaceContext.tsx";
import { GraphCanvas } from "../graph/GraphCanvas.tsx";
import { EdgeRow } from "../graph/Legend.tsx";
import {
  ARM_LABEL,
  armLabel,
  buildCoverageDashboard,
  reasonLabel,
  type ArmCoverage,
  type CoverageDashboard,
} from "./coverageModel.ts";
import {
  buildServiceMap,
  memberOfServiceId,
  serviceMembers,
  type ServiceLink,
  type ServiceMember,
} from "./serviceMapModel.ts";
import graphStyles from "../graph/GraphView.module.css";
import styles from "./Workspace.module.css";

/** The relation arms the service map can draw — the legend's rows. Derived from the
 *  arm-label map so a new arm cannot be added to the dashboard yet silently omitted
 *  from the legend. */
const RELATION_ARMS = Object.keys(ARM_LABEL);

/** A percentage rendered from a 0–1 ratio, at one decimal — never rounded up to a
 *  flattering figure. */
function pct(ratio: number): string {
  return `${(ratio * 100).toFixed(1)}%`;
}

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
          { id: "map", label: "Service map", panel: <ServiceMapPanel services={services} /> },
          {
            id: "coverage",
            label: "Cross-service coverage",
            panel: <CoveragePanel dashboard={coverage} />,
          },
          { id: "impact", label: "Cross-service impact", panel: <ImpactPanel /> },
        ]}
      />
    </>
  );
}

// ── Service map (frontend-design §4.16) ──────────────────────────────────────

function ServiceMapPanel({ services }: { services: ServiceMember[] }) {
  const bindings = useApiResource<XserviceRouteProviders>(() => fetchWorkspaceBindings(), []);
  return (
    <AsyncResource resource={bindings} loadingLabel="Loading the service map…">
      {(model) => <ServiceMap services={services} providers={model} />}
    </AsyncResource>
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

function ServiceMap({
  services,
  providers,
}: {
  services: ServiceMember[];
  providers: XserviceRouteProviders;
}) {
  const { selectMember } = useWorkspace();
  const map = buildServiceMap(services, providers.providers);

  return (
    <div className={styles.panel}>
      {map.links.length === 0 ? (
        <EmptyState message="No cross-service bindings resolved yet — every service is drawn, and the Cross-service coverage tab reports why each reference has not bound." />
      ) : null}

      {/* Clicking a service focuses its member: the shell selector switches to it and
          every other view re-fetches scoped to that member (frontend-design §4.16). */}
      <GraphCanvas
        loaded={map.loaded}
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
        </div>
      </details>

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
            columns={LINK_COLUMNS}
            rows={map.links}
            rowKey={(l) => `${l.from}->${l.to}:${l.relation}`}
            pageSize={DEFAULT_TABLE_PAGE_SIZE}
          />
        </Card>
      )}
    </div>
  );
}

// ── Cross-service coverage (frontend-design §4.17) ───────────────────────────

const ARM_COLUMNS: Column<ArmCoverage>[] = [
  {
    key: "relation",
    header: "Arm",
    cell: (a) => armLabel(a.relation),
    sortValue: (a) => a.relation,
  },
  { key: "bound", header: "Bound", numeric: true, cell: (a) => a.bound, sortValue: (a) => a.bound },
  {
    key: "ambiguous",
    header: "Ambiguous",
    numeric: true,
    cell: (a) => a.ambiguous,
    sortValue: (a) => a.ambiguous,
  },
  {
    key: "unbound",
    header: "Unbound",
    numeric: true,
    cell: (a) => a.unbound,
    sortValue: (a) => a.unbound,
  },
  {
    // Its own column, never folded into Unbound — so this row's figures sum to the
    // headline's above it (ADR-53).
    key: "noProvider",
    header: "No provider here",
    numeric: true,
    cell: (a) => a.noProvider,
    sortValue: (a) => a.noProvider,
  },
  {
    key: "reasons",
    // "Not bound", not "Unbound": ambiguity and no-provider are their own buckets,
    // so calling their reasons "unbound" would contradict the columns beside them.
    header: "Not bound because",
    cell: (a) =>
      a.reasons.length === 0 ? (
        <span className="muted">—</span>
      ) : (
        <ul className={styles.reasons}>
          {a.reasons.map((r) => (
            <li key={r.reason}>
              {reasonLabel(r.reason)} <Badge tone="muted">{r.count}</Badge>
            </li>
          ))}
        </ul>
      ),
    sortValue: (a) => a.reasons.length,
  },
];

function CoveragePanel({ dashboard }: { dashboard: CoverageDashboard }) {
  if (dashboard.isEmpty) {
    return (
      <div className={styles.panel}>
        <EmptyState message="No cross-boundary references found in this workspace — nothing to bind, so no coverage is reported (never a fabricated 100%)." />
      </div>
    );
  }

  return (
    <div className={styles.panel}>
      <Card title="Cross-boundary coverage">
        {/* Advisory only — never a gate input (ADR-53). The ratio is the server's,
            displayed verbatim: `no-provider-in-workspace` is deliberately outside its
            denominator, so recomputing it here would contradict the CLI. */}
        <div className={styles.ratio}>
          <ScoreBar value={dashboard.boundRatio} max={1} label={pct(dashboard.boundRatio)} />
          <span className="mono">{pct(dashboard.boundRatio)} bound</span>
        </div>
        <p className="muted">
          {dashboard.bound} bound · {dashboard.ambiguous} ambiguous · {dashboard.unbound} unbound ·{" "}
          {dashboard.noProviderInWorkspace} with no provider in this workspace (reported apart, and
          excluded from the ratio — a call to a service outside this workspace is not a broken
          binding). Advisory: this figure is never a quality-gate input.
        </p>
      </Card>

      <Card title="Coverage by relation arm">
        <DataTable
          caption="Cross-service coverage by relation arm"
          columns={ARM_COLUMNS}
          rows={dashboard.arms}
          rowKey={(a) => a.relation}
          pageSize={DEFAULT_TABLE_PAGE_SIZE}
        />
      </Card>
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
                {model.cross_service.length === 0 ? (
                  <EmptyState message="No cross-service impact — no resolved binding reaches this symbol from another service. (An unmaterialized binding is unknown, not absent — see Cross-service coverage.)" />
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
