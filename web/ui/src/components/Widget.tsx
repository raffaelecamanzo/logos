/*
 * Widget (S-611, CR-203, CR-206, FR-UI-39, FR-UI-40). The one frame every widget
 * renders through. It composes `Card` — the brand grammar (3px red top rule,
 * radius, padding; ADR-44) is unchanged — and lays out four parts, top to
 * bottom, each marked `data-widget-part`:
 *
 *   1. title       — the title left, at most one status badge right, one line;
 *   2. figure      — the key figure(s), or the statement of an absence (which
 *                    names the command that fixes it, when one does);
 *   3. explanation — what the widget shows, and why it matters (then any
 *                    `note`: payload text the catalogue cannot hold);
 *   4. evidence    — the table, chart or list (the children).
 *
 * There is no action line: CR-206 removed "What you can do" and its where chip,
 * and CR-207 removed the per-row action columns that still rendered it.
 *
 * The words come from a catalogue entry (`copy`), so a view never writes copy
 * inline and a wording change edits one catalogue. One entry is typed to have
 * no explanation (`NoExplanationEntry`, CR-208 — Project Overview); its frame
 * leaves the explanation part out and is marked `data-widget-no-explanation`. An absence renders as a
 * left-aligned statement in the figure row; the centred `EmptyState` is for a
 * view with no widget at all, never here.
 *
 * PANEL MODE (S-617): a tool panel — a query form, an editor, the chat, a wiki
 * page — presents no figure, so it is exempt from why. It is
 * rendered as `<Widget panel="key" title=…>`: the title row, the panel's one
 * line from the `TOOL_PANELS` register as its explanation (`what`), then the
 * tool itself in the evidence part. The frame is marked `data-widget-panel`.
 */

import type { ReactNode } from "react";

import { TOOL_PANELS, type ToolPanelKey } from "../copy/toolPanels.ts";
import type { CopyEntry, CopyText, NoExplanationEntry } from "../copy/types.ts";

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

/** A figure widget: its words from a catalogue entry. */
type CopyMode = {
  /** The widget's catalogue entry — or the one entry with no explanation. */
  copy: CopyEntry | NoExplanationEntry;
  panel?: undefined;
  /**
   * Secondary detail for the explanation, after why: text the read-model
   * supplies (its own caveats, rendered verbatim), which a catalogue cannot
   * hold. Not catalogue copy, so not one of the `data-widget-copy` parts.
   */
  note?: ReactNode;
} & FigureRow;

/** A tool panel: its one line from the `TOOL_PANELS` register, and no figure
 *  or why. */
type PanelMode = {
  /** The panel's key in `TOOL_PANELS`. */
  panel: ToolPanelKey;
  copy?: undefined;
  note?: undefined;
  figure?: undefined;
  absence?: undefined;
};

export type WidgetProps = {
  /** The title (≤8 words), left on the title row. */
  title: ReactNode;
  /** At most one status badge, right on the title row. */
  badge?: ReactNode;
  /** The evidence: a table, chart or list — or, for a panel, the tool itself. */
  children?: ReactNode;
  className?: string;
  /** Removed by CR-206 with the action line it drove. Typed `never` so a stale
   *  state is a type error even inside a spread (`{...common}`), which JSX does
   *  not check for excess properties. */
  state?: never;
} & (CopyMode | PanelMode);

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
 * Secondary detail in a figure row — a figure's unit or qualifier ("files
 * ranked", "of lines covered"), a scope line, a not-current note. It is set at
 * the body size in muted ink, whatever the figure's own size: muted tone, not a
 * different size, marks the detail (FR-UI-40). The one figure-row qualifier
 * every view uses, so the same role reads the same on every page. `block`
 * renders it as its own line (a `<p>`); otherwise it runs inline beside the
 * figure.
 */
export function FigureNote({ children, block = false }: { children: ReactNode; block?: boolean }) {
  const Tag = block ? "p" : "span";
  return (
    <Tag className={styles.figureNote} data-figure-note="">
      {children}
    </Tag>
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

/** The title row: the title left, at most one badge right. */
function TitleRow({ title, badge }: { title: ReactNode; badge?: ReactNode }) {
  return (
    <div className={styles.titleRow} data-widget-part="title">
      <h3 className={styles.title}>{title}</h3>
      {isRendered(badge) && <div className={styles.badge}>{badge}</div>}
    </div>
  );
}

/** The evidence part, left out when it holds nothing. */
function Evidence({ children }: { children?: ReactNode }) {
  if (!isRendered(children)) return null;
  return (
    <div className={styles.evidence} data-widget-part="evidence">
      {children}
    </div>
  );
}

export function Widget(props: WidgetProps) {
  const { title, badge, children, className } = props;
  const cardClass = [styles.widget, className].filter(Boolean).join(" ");

  if (props.panel !== undefined) {
    return (
      <Card className={cardClass}>
        <div className={styles.frame} data-widget="" data-widget-panel={props.panel}>
          <TitleRow title={title} badge={badge} />
          <div className={styles.explanation} data-widget-part="explanation">
            <p className={styles.body} data-widget-copy="what">
              <CopyTextView text={TOOL_PANELS[props.panel].what} />
            </p>
          </div>
          <Evidence>{children}</Evidence>
        </div>
      </Card>
    );
  }

  const { copy, note, figure, absence } = props;
  const hasAbsence = isRendered(absence);
  const noExplanation = "noExplanation" in copy;

  return (
    <Card className={cardClass}>
      {/* The exception is marked on the frame, as a panel is, so a layout check
          excuses this one frame by declaration rather than by a missing part. */}
      <div className={styles.frame} data-widget="" data-widget-no-explanation={noExplanation ? "" : undefined}>
        <TitleRow title={title} badge={badge} />

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

        {noExplanation ? (
          isRendered(note) && (
            <div className={styles.explanation} data-widget-part="explanation">
              <div className={styles.note} data-widget-note="">
                {note}
              </div>
            </div>
          )
        ) : (
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
        )}

        <Evidence>{children}</Evidence>
      </div>
    </Card>
  );
}
