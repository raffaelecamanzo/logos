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
 *
 * The detail sits below every other widget, so choosing a finding would change
 * nothing the reader can see. Each `shown` request (HF-1) therefore moves focus
 * to the heading, for keyboard and screen-reader users, and scrolls the widget
 * into view — smoothly, unless the user prefers reduced motion.
 */

import { useEffect, useRef } from "react";

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
  shown,
}: {
  finding: SecurityFinding | null;
  classification: Classification | null;
  /** How many times a finding has been chosen: each increment brings the detail into view. */
  shown: number;
}) {
  const heading = useRef<HTMLSpanElement>(null);
  // Only a change in `shown` is a Show: the counter outlives this widget, which
  // unmounts while a reload finds no findings, so a remount must not replay it.
  const seen = useRef(shown);
  useEffect(() => {
    const el = heading.current;
    if (shown === seen.current || el === null) return;
    seen.current = shown;
    // The scroll below is the one that moves the page; focusing must not jump it first.
    el.focus({ preventScroll: true });
    const reduced = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
    (el.closest(`.${styles.detail}`) ?? el).scrollIntoView({ behavior: reduced ? "auto" : "smooth", block: "start" });
  }, [shown]);
  const title = (
    <span ref={heading} tabIndex={-1} className={styles.detailTitle}>
      Finding detail
    </span>
  );
  if (finding === null || classification === null) {
    return <Widget title={title} className={styles.detail} copy={findingDetail} absence={SECURITY_ABSENCE.noneSelected} />;
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
      title={title}
      className={styles.detail}
      badge={<Badge tone="muted">{finding.severity}</Badge>}
      copy={findingDetail}
      figure={
        <span>
          <span className="mono">{finding.rule_id}</span> at <span className="mono">{findingLocation(finding)}</span>
        </span>
      }
    >
      <dl className={styles.facts}>
        {finding.severity_reason !== undefined && (
          <>
            <dt>Severity</dt>
            <dd>
              {finding.severity}: {finding.severity_reason}
            </dd>
          </>
        )}
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

/**
 * The blast-radius lists, from `impact` on the enclosing symbol. Each list — its
 * title and table, or its empty-state sentence — is one `impactSection`, and a
 * section that follows another is spaced from it by one token.
 */
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
              className={styles.impactSection}
              caption={result.upstream_label}
              captionVisible
              columns={ENTRY_COLUMNS}
              rows={result.upstream}
              rowKey={(e) => e.symbol}
              pageSize={DEFAULT_TABLE_PAGE_SIZE}
              empty="Nothing depends on it."
            />
            <DataTable
              className={styles.impactSection}
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
