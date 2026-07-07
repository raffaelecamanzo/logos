/*
 * The structured + relational whole-graph query bar (S-186, FR-UI-14, ADR-35) —
 * re-homed into React over the read-only `/api/v1/query` endpoint. A field-filter
 * form (a search term refined by kind/layer/file) or a relational verb
 * (callers/callees/impact of a target symbol); a chosen verb takes precedence. The
 * query runs over the WHOLE graph (not the rendered set), so a selected hit may not
 * be on the canvas — selecting one centers and locks it via `onSelect` (the
 * locked-selection mechanism). An empty result is the server's honest "no matches"
 * note, never an error (NFR-CC-04, FR-NV-09). Runs on submit (a user action), so it
 * uses the imperative `runQuery` client rather than an on-mount resource hook.
 */

import { useState } from "react";

import { runQuery } from "../../api/index.ts";
import type { QueryHit, QueryResponse } from "../../api/types.ts";
import { Button, Card, DataTable, SelectField, TextField, type Column } from "../../components/index.ts";
import styles from "./GraphView.module.css";

/** S-197: graph query results use 15 rows/page (FR-UI-14), distinct from the shared 20. */
const QUERY_PAGE_SIZE = 15;

export interface GraphQueryProps {
  /** Center + lock the selected hit's node id on the canvas. */
  onSelect: (id: string) => void;
}

export function GraphQuery({ onSelect }: GraphQueryProps) {
  const [text, setText] = useState("");
  const [kind, setKind] = useState("");
  const [layer, setLayer] = useState("");
  const [file, setFile] = useState("");
  const [verb, setVerb] = useState("");
  const [target, setTarget] = useState("");
  const [results, setResults] = useState<QueryResponse | null>(null);
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = async () => {
    setRunning(true);
    setError(null);
    try {
      const params = verb
        ? { verb, target: target.trim() }
        : { q: text.trim(), kind: kind.trim(), layer, file: file.trim() };
      const response = await runQuery(params);
      setResults(response);
      // Center + lock the top hit immediately (the listed hits are highlighted).
      if (response.hits.length > 0) onSelect(response.hits[0].id);
    } catch {
      setError("The query could not be run.");
      setResults(null);
    } finally {
      setRunning(false);
    }
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Enter") {
      e.preventDefault();
      void submit();
    }
  };

  return (
    <Card title="Query the whole graph">
      <div className={styles.query} onKeyDown={onKeyDown}>
        <TextField
          label="Search"
          placeholder="Search the whole graph…"
          value={text}
          onChange={(e) => setText(e.target.value)}
          autoComplete="off"
        />
        <TextField
          label="Kind"
          placeholder="e.g. function, struct, requirement"
          value={kind}
          onChange={(e) => setKind(e.target.value)}
          autoComplete="off"
        />
        <SelectField label="Layer" value={layer} onChange={(e) => setLayer(e.target.value)}>
          <option value="">Any layer</option>
          <option value="code">Code</option>
          <option value="doc">Docs</option>
          <option value="artifact">Artifacts</option>
        </SelectField>
        <TextField
          label="File"
          placeholder="file path…"
          value={file}
          onChange={(e) => setFile(e.target.value)}
          autoComplete="off"
        />
        <SelectField label="Relation" value={verb} onChange={(e) => setVerb(e.target.value)}>
          <option value="">— relation —</option>
          <option value="callers-of">Callers of</option>
          <option value="callees-of">Callees of</option>
          <option value="impact-of">Impact of</option>
        </SelectField>
        <TextField
          label="Target"
          placeholder="target symbol…"
          value={target}
          onChange={(e) => setTarget(e.target.value)}
          autoComplete="off"
        />
        {/* Wrapper gives the button align-self: flex-end so its baseline matches
            the input fields despite having no field-label above it (S-197). */}
        <div className={styles.querySubmit}>
          <Button onClick={() => void submit()} disabled={running}>
            {running ? "Querying…" : "Query"}
          </Button>
        </div>
      </div>

      <div role="region" aria-label="Query results" aria-live="polite">
        {error && <p className={styles.notice}>{error}</p>}
        {results && <QueryResults results={results} onSelect={onSelect} />}
      </div>
    </Card>
  );
}

const QUERY_COLUMNS: Column<QueryHit & { onSelect: (id: string) => void }>[] = [
  {
    key: "index",
    header: "#",
    numeric: true,
    cell: (r) => r.rank,
    sortValue: (r) => r.rank,
  },
  {
    key: "name",
    header: "Name",
    mono: true,
    cell: (r) => (
      <button type="button" className={styles.queryRowBtn} onClick={() => r.onSelect(r.id)}>
        {r.label}
      </button>
    ),
    sortValue: (r) => r.label,
  },
  {
    key: "path",
    header: "Path",
    mono: true,
    cell: (r) => r.file ?? "—",
    sortValue: (r) => r.file ?? "",
  },
];

function QueryResults({
  results,
  onSelect,
}: {
  results: QueryResponse;
  onSelect: (id: string) => void;
}) {
  if (results.hits.length === 0) {
    return <p className="muted">{results.note ?? "No matches."}</p>;
  }
  const total = results.total;
  const summary =
    total > results.hits.length
      ? `${results.hits.length} of ${total} matches`
      : `${results.hits.length} ${results.hits.length === 1 ? "match" : "matches"}`;
  // Attach onSelect to each row so the cell renderer can call it without closure
  // capture issues when the column definition is module-level.
  const rows = results.hits.map((h) => ({ ...h, onSelect }));
  return (
    <>
      <p className="muted">{summary} — select a result to center and lock it.</p>
      <DataTable
        caption="Query results"
        columns={QUERY_COLUMNS}
        rows={rows}
        rowKey={(r) => r.id}
        pageSize={QUERY_PAGE_SIZE}
        empty="No matches."
      />
    </>
  );
}
