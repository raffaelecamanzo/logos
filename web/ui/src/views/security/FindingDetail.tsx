/*
 * The Security page's finding detail (S-670, FR-SC-09, FR-SC-13): one finding in
 * the code graph — its enclosing symbol, whether a route reaches it, its blast
 * radius, and the confidence label on each claim (S-669).
 *
 * `unknown` reachability renders as unknown, never as unreachable, with
 * `unresolved_because` stated when set. The payload carries the blast radius as
 * sizes only, so the lists come from `GET /api/v1/impact` on the enclosing
 * symbol — the same answer `logos impact` gives. No source snippet is rendered;
 * a secret-class message reads `[redacted]`.
 */

import { fetchImpact } from "../../api/client.ts";
import { AsyncResource, useApiResource } from "../../api/hooks.tsx";
import type { ImpactEntry, ImpactResult, SecurityFinding } from "../../api/types.ts";
import { Badge, DataTable, DEFAULT_TABLE_PAGE_SIZE, Widget, type Column } from "../../components/index.ts";
import { CONTEXT_TEXT, SECURITY_ABSENCE, UNCLASSIFIED_TEXT, findingDetail } from "../../copy/security.copy.ts";
import { findingLocation, findingMessage, type Classification } from "./securityModel.ts";
import styles from "./SecurityView.module.css";

export function FindingDetail({
  finding,
  classification,
}: {
  finding: SecurityFinding | null;
  classification: Classification | null;
}) {
  if (finding === null || classification === null) {
    return <Widget title="Finding detail" copy={findingDetail} absence={SECURITY_ABSENCE.noneSelected} />;
  }
  const { enclosing, reachability, blast_radius: blast } = finding.context;
  const reach =
    reachability.verdict === "reachable" && reachability.route !== null
      ? CONTEXT_TEXT.reachable(reachability.route.name, reachability.route.hops)
      : reachability.verdict === "unreachable"
        ? CONTEXT_TEXT.unreachable
        : CONTEXT_TEXT.unknown(reachability.unresolved_because);
  return (
    <Widget
      title="Finding detail"
      badge={<Badge tone="muted">{finding.severity}</Badge>}
      copy={findingDetail}
      figure={
        <span>
          <span className="mono">{finding.rule_id}</span> at <span className="mono">{findingLocation(finding)}</span>
        </span>
      }
    >
      <dl className={styles.facts}>
        <dt>Message</dt>
        <dd>{findingMessage(finding)}</dd>
        <dt>OWASP</dt>
        <dd>
          {classification.categories.length > 0 ? (
            <span className="mono">{classification.categories.join(", ")}</span>
          ) : classification.unclassified !== null ? (
            UNCLASSIFIED_TEXT[classification.unclassified]
          ) : (
            "—"
          )}
        </dd>
        <dt>Enclosing symbol</dt>
        <dd>
          {enclosing.symbol !== null ? (
            <span className="mono">{enclosing.symbol.name}</span>
          ) : (
            CONTEXT_TEXT.unbound(enclosing.unbound_because ?? "no_location")
          )}{" "}
          <span className="muted">({CONTEXT_TEXT.confidence(enclosing.confidence)})</span>
        </dd>
        <dt>Route reach</dt>
        <dd data-reachability={reachability.verdict}>
          {reach} <span className="muted">({CONTEXT_TEXT.confidence(reachability.confidence)})</span>
        </dd>
        <dt>Blast radius</dt>
        <dd>
          {blast.upstream !== undefined && blast.downstream !== undefined
            ? CONTEXT_TEXT.blast(blast.upstream, blast.downstream)
            : "—"}{" "}
          <span className="muted">({CONTEXT_TEXT.confidence(blast.confidence)})</span>
        </dd>
      </dl>
      {enclosing.symbol !== null && <ImpactLists symbol={enclosing.symbol.symbol} />}
    </Widget>
  );
}

const ENTRY_COLUMNS: Column<ImpactEntry>[] = [
  { key: "name", header: "Symbol", mono: true, sortValue: (e) => e.name, cell: (e) => e.name },
  { key: "file", header: "File", mono: true, sortValue: (e) => e.file ?? "", cell: (e) => e.file ?? "—" },
  { key: "dist", header: "Distance", numeric: true, sortValue: (e) => e.distance, cell: (e) => e.distance },
];

/** The blast-radius lists, from `impact` on the enclosing symbol. */
function ImpactLists({ symbol }: { symbol: string }) {
  const impact = useApiResource<ImpactResult>(() => fetchImpact(symbol), [symbol]);
  return (
    <AsyncResource resource={impact} loadingLabel="Loading the blast radius…">
      {(result) =>
        result.resolved === null ? (
          // Unresolved is unknown, never "nothing depends on it" (NFR-CC-04).
          <div data-impact="unresolved">
            <p>{CONTEXT_TEXT.impactUnresolved}</p>
            {result.warnings.map((w) => (
              <p key={w} className="muted">
                {w}
              </p>
            ))}
          </div>
        ) : (
          <>
            <DataTable
              caption={result.upstream_label}
              captionVisible
              columns={ENTRY_COLUMNS}
              rows={result.upstream}
              rowKey={(e) => e.symbol}
              pageSize={DEFAULT_TABLE_PAGE_SIZE}
              empty="Nothing depends on it."
            />
            <DataTable
              caption={result.downstream_label}
              captionVisible
              columns={ENTRY_COLUMNS}
              rows={result.downstream}
              rowKey={(e) => e.symbol}
              pageSize={DEFAULT_TABLE_PAGE_SIZE}
              empty="It depends on nothing."
            />
          </>
        )
      }
    </AsyncResource>
  );
}
