/*
 * Widget (S-611, CR-203, FR-UI-39, FR-UI-40). The one frame every widget renders
 * through. It composes `Card` — the brand grammar (3px red top rule, radius,
 * padding; ADR-44) is unchanged — and lays out five parts, top to bottom, each
 * marked `data-widget-part`:
 *
 *   1. title       — the title left, at most one status badge right, one line;
 *   2. figure      — the key figure(s), or the statement of an absence;
 *   3. explanation — what the widget shows, and why it matters (then any
 *                    `note`: payload text the catalogue cannot hold);
 *   4. action      — "What you can do", the action text, and the where chip;
 *   5. evidence    — the table, chart or list (the children).
 *
 * The words come from a catalogue entry (`copy`) evaluated at the widget's
 * `state`, so a view never writes copy inline and a wording change edits one
 * catalogue. An absence renders as a left-aligned statement in the figure row;
 * the centred `EmptyState` is for a view with no widget at all, never here.
 */

import type { ReactNode } from "react";

import { NOTHING_TO_DO, type CopyEntry, type CopyText, type WidgetAction } from "../copy/types.ts";

import { Card } from "./Card.tsx";
import { Term } from "./Term.tsx";
import styles from "./Widget.module.css";

/** The figure row holds a figure OR an absence statement, never both. */
type FigureRow =
  | { figure?: ReactNode; absence?: undefined }
  | {
      figure?: undefined;
      /** A named absence ("Nothing measured yet — …"), stated in the figure row. */
      absence: ReactNode;
    };

/** `state` may be omitted only when the entry's action takes none. */
type StateProp<S> = undefined extends S ? { state?: S } : { state: S };

export type WidgetProps<S> = {
  /** The title (≤8 words), left on the title row. */
  title: ReactNode;
  /** At most one status badge, right on the title row. */
  badge?: ReactNode;
  /** The widget's catalogue entry. */
  copy: CopyEntry<S>;
  /**
   * Secondary detail for the explanation, after why: text the read-model
   * supplies (its own caveats, rendered verbatim), which a catalogue cannot
   * hold. Not catalogue copy, so not one of the `data-widget-copy` parts.
   */
  note?: ReactNode;
  /** The evidence: a table, chart or list. */
  children?: ReactNode;
  className?: string;
} & FigureRow &
  StateProp<S>;

/** Renders catalogue text, glossing each `Gloss` segment through `Term`. */
export function CopyTextView({ text }: { text: CopyText }) {
  if (typeof text === "string") return <>{text}</>;
  return (
    <>
      {text.map((seg, i) =>
        typeof seg === "string" ? (
          seg
        ) : (
          <Term key={i} term={seg.term}>
            {seg.text}
          </Term>
        ),
      )}
    </>
  );
}

/**
 * One table row's own action, as a cell (S-613's Members column, S-614's
 * evidence column): the action text, then where and its target on a second
 * line. A `none` action reads `none` — by default the one "Nothing to do"
 * sentence; a catalogue whose `none` says something more specific passes it.
 */
export function ActionCell({ action, none = NOTHING_TO_DO }: { action: WidgetAction; none?: CopyText }) {
  if (action.kind === "none") {
    return (
      <span className="muted">
        <CopyTextView text={none} />
      </span>
    );
  }
  return (
    <>
      <CopyTextView text={action.text} />
      <br />
      <span className="muted">{action.where}</span>
      {action.target !== undefined && (
        <>
          {" "}
          <code>{action.target}</code>
        </>
      )}
    </>
  );
}

/**
 * Whether a node renders anything. `false`, `null`, `""` and `[]` render
 * nothing in React, so a part holding only one of them is left out rather than
 * rendered empty — an empty part would still take a gap in the frame.
 */
function isRendered(node: ReactNode): boolean {
  if (node === undefined || node === null || node === false || node === "") return false;
  return !(Array.isArray(node) && node.length === 0);
}

export function Widget<S>(props: WidgetProps<S>) {
  const { title, badge, copy, note, figure, absence, children, className } = props;
  const action = copy.action((props as { state: S }).state);
  const hasAbsence = isRendered(absence);

  return (
    <Card className={[styles.widget, className].filter(Boolean).join(" ")}>
      <div className={styles.frame} data-widget="">
        <div className={styles.titleRow} data-widget-part="title">
          <h3 className={styles.title}>{title}</h3>
          {isRendered(badge) && <div className={styles.badge}>{badge}</div>}
        </div>

        {(hasAbsence || isRendered(figure)) && (
          <div className={styles.figureRow} data-widget-part="figure">
            {hasAbsence ? (
              <p className={styles.absence} data-widget-absence="">
                {absence}
              </p>
            ) : (
              figure
            )}
          </div>
        )}

        <div className={styles.explanation} data-widget-part="explanation">
          <p className={styles.body} data-widget-copy="what">
            <CopyTextView text={copy.what} />
          </p>
          <p className={styles.body} data-widget-copy="why">
            <CopyTextView text={copy.why} />
          </p>
          {isRendered(note) && (
            <div className={styles.note} data-widget-note="">
              {note}
            </div>
          )}
        </div>

        <div className={styles.action} data-widget-part="action" data-action-kind={action.kind}>
          <p className={styles.body}>
            <span className={styles.actionLabel}>What you can do:</span>{" "}
            <span data-widget-copy="action">
              {action.kind === "act" ? <CopyTextView text={action.text} /> : NOTHING_TO_DO}
            </span>
          </p>
          {action.kind === "act" && (
            <p className={styles.where} data-widget-copy="where">
              <span className={styles.whereKind}>{action.where}</span>
              {action.target !== undefined && (
                <>
                  {" "}
                  <code className={styles.target}>{action.target}</code>
                </>
              )}
            </p>
          )}
        </div>

        {isRendered(children) && (
          <div className={styles.evidence} data-widget-part="evidence">
            {children}
          </div>
        )}
      </div>
    </Card>
  );
}
