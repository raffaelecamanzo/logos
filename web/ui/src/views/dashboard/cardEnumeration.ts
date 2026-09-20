/*
 * The deterministic half of the S-438 Dashboard card audit (CR-141 §3). Extracts
 * every `<Card title="...">`-returning component from DashboardView.tsx's source
 * text so the census is a mechanical read, not a hand count re-done by eye each
 * time a card is added — see cardEnumeration.test.ts for the recorded audit and
 * the proof that an added card is not silently missed.
 */

export interface DashboardCardEntry {
  /** The `...Card` function name, e.g. `RuleFindingsCard`. */
  component: string;
  /** The literal string passed to `<Card title="...">`; omitted for a dynamic title. */
  title: string | undefined;
}

const CARD_COMPONENT = /^function (\w*Card)\(/gm;
const CARD_TITLE = /<Card\s+title="([^"]+)"/;

/** Finds every top-level `function ...Card(...)` in `source` and the literal
 *  title of the first `<Card title="...">` it renders. Order matches source order. */
export function enumerateDashboardCards(source: string): DashboardCardEntry[] {
  const starts = [...source.matchAll(CARD_COMPONENT)];
  return starts.map((match, i) => {
    const bodyStart = match.index ?? 0;
    const bodyEnd = i + 1 < starts.length ? (starts[i + 1].index ?? source.length) : source.length;
    const body = source.slice(bodyStart, bodyEnd);
    const titleMatch = CARD_TITLE.exec(body);
    return { component: match[1], title: titleMatch?.[1] };
  });
}
