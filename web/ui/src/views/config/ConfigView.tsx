/*
 * ConfigView (S-191, CR-049, FR-UI-12/13, FR-CF-06, ADR-31) — the Config policy
 * editor migrated to React over the unchanged intent-guarded config POSTs. The
 * last interactive tab and the SPA's first **mutating** surface.
 *
 * Hybrid editing (FR-UI-12): each file is edited two ways over ONE candidate
 * document — typed form fields for the scalar/list keys (and the `[chat]`
 * provider/model/base_url controls), plus a raw-TOML pane holding the full
 * document including the repeated-table sections that do not formify. The raw pane
 * is the **authoritative candidate** posted verbatim to `/config/save`; a typed
 * field patches its one key into that text (`./toml.ts`), so there is no second
 * source of truth and a patch defect can at worst yield a `422`, never a partial
 * write (BR-35).
 *
 * Save ≠ Apply (FR-UI-13): Save validates-then-atomic-writes and runs no pipeline;
 * the explicit Apply reconciles (config.toml) or re-evaluates the gate
 * (rules.toml) over the saved file. A `rules.toml` Save is gated behind an
 * explicit confirmation (BR-35). The chat API key is a write-only/masked secret
 * (FR-CF-06, NFR-SE-07) edited in its own section — never echoed onto this surface.
 *
 * Renders exclusively through the S-193 design system; every read is GET-only.
 */

import { useState } from "react";
import type { ChangeEvent, ReactNode } from "react";

import {
  ConfigMutateError,
  applyConfig,
  fetchConfig,
  saveConfig,
  saveSecret,
  verifyGraph,
  type PolicyFile,
} from "../../api/configClient.ts";
import { AsyncResource, useApiResource } from "../../api/hooks.tsx";
import type {
  ConfigApplyOutcome,
  ConfigReadModel,
  ConfigWriteOutcome,
  FileView,
  MaskedSecret,
  ParsedConfig,
  ParsedRules,
  SecretWriteOutcome,
  VerifyReport,
} from "../../api/types.ts";
import {
  Badge,
  Button,
  Callout,
  Card,
  DataTable,
  ErrorPanel,
  LoadingState,
  SelectField,
  TextField,
  TextareaField,
} from "../../components/index.ts";
import { patch, type TomlFieldType } from "./toml.ts";
import styles from "./ConfigView.module.css";

// ── Typed-field model ─────────────────────────────────────────────────────────

/** How a typed field renders. `provider` is a string select; the rest map 1:1 to
 *  a {@link TomlFieldType}. */
type Control = "int" | "float" | "str" | "list" | "bool" | "provider";

/** A typed field that patches its `key` in `table` of the raw candidate. */
interface FieldDescriptor {
  /** The owning `[table]`, or `""` for a top-level key. */
  table: string;
  key: string;
  control: Control;
  help: string;
  /** Pre-filled value from the parsed projection (never fabricated). */
  initial: string;
  placeholder?: string;
  /** Overrides the rendered/accessible label when `key` collides with another
   *  table's field (e.g. `[wiki].model` beside `[chat].model`). Defaults to `key`. */
  label?: string;
}

/** A group of typed fields rendered under an optional `[legend]`. */
interface FieldGroup {
  legend?: string;
  fields: FieldDescriptor[];
}

/** The TOML serialisation type for a control (provider serialises as a string). */
function controlTomlType(control: Control): TomlFieldType {
  switch (control) {
    case "int":
      return "int";
    case "float":
      return "float";
    case "list":
      return "list";
    case "bool":
      return "bool";
    default:
      return "str"; // str | provider
  }
}

/** A stable per-file state key for a field. */
function fieldId(d: FieldDescriptor): string {
  return `${d.table}.${d.key}`;
}

// ── Parsed → initial typed-field value (honest pre-fill, never fabricated) ──────

function initList(items: string[] | undefined): string {
  return (items ?? []).join("\n");
}

function initInt(v: number | null | undefined): string {
  return v == null ? "" : String(v);
}

/** A `[constraints]` bool (`no_god_containers`) → tri-state select value. */
function initBool(v: unknown): string {
  return v === true ? "true" : v === false ? "false" : "";
}

/** `max_dead` typed field: only the **absolute** integer form is typed here; a
 *  delta-table contract is edited in the raw pane (mirrors `MaxDead::as_absolute`). */
function initMaxDead(v: unknown): string {
  return typeof v === "number" ? String(v) : "";
}

/** The `config.toml` typed fields: top-level scalars/lists + the `[chat]` group. */
function configGroups(c: ParsedConfig): FieldGroup[] {
  return [
    {
      fields: [
        { table: "", key: "languages", control: "list", initial: initList(c.languages), help: "Language plugins to enable. One per line." },
        { table: "", key: "include", control: "list", initial: initList(c.include), help: "Include globs (root-relative). One per line." },
        { table: "", key: "exclude", control: "list", initial: initList(c.exclude), help: "Exclude globs, unioned with gitignore. One per line." },
        { table: "", key: "max_file_size", control: "int", initial: initInt(c.max_file_size), help: "Skip files larger than this (bytes)." },
        { table: "", key: "framework_hints", control: "list", initial: initList(c.framework_hints), help: "Framework hints biasing extraction. One per line." },
      ],
    },
    {
      legend: "[chat]",
      fields: [
        { table: "chat", key: "provider", control: "provider", initial: c.chat.provider, help: "The provider family. openai is OpenAI-compatible (base_url defaults to OpenRouter); anthropic uses the native Messages endpoint." },
        { table: "chat", key: "model", control: "str", initial: c.chat.model ?? "", placeholder: "leave blank to unset", help: "The model the chat agent uses (a Claude id, or an OpenRouter model slug for openai). Required — until set the Chat tab stays “not yet usable”." },
        { table: "chat", key: "base_url", control: "str", initial: c.chat.base_url, placeholder: "leave blank for the default (OpenRouter)", help: "The OpenAI-compatible endpoint for the openai provider (anthropic ignores this). Leave blank to fall back to OpenRouter." },
      ],
    },
    {
      // S-224/FR-CF-07: the `[wiki]` section carries only `model` — provider,
      // base_url, and the API key are inherited from `[chat]`, so this single
      // field fully surfaces it (no new secret surface, NFR-SE-07).
      legend: "[wiki]",
      fields: [
        {
          table: "wiki",
          key: "model",
          control: "str",
          label: "wiki model",
          initial: c.wiki?.model ?? "",
          placeholder: "leave blank to inherit [chat].model",
          help: "The model used for wiki page synthesis, distinct from the chat model. Leave blank to fall back to [chat].model.",
        },
      ],
    },
  ];
}

/** The `rules.toml` typed fields: the `[constraints]` and `[metric_thresholds]`
 *  tables. Repeated tables ([[layers]], …) are edited in the raw pane. */
function rulesGroups(r: ParsedRules): FieldGroup[] {
  const con = r.constraints ?? {};
  const mt = r.metric_thresholds ?? {};
  const cInt = (key: string, help: string): FieldDescriptor => ({ table: "constraints", key, control: "int", initial: initInt(con[key] as number | null | undefined), help });
  const tInt = (key: string, help: string): FieldDescriptor => ({ table: "metric_thresholds", key, control: "int", initial: initInt(mt[key]), help });
  return [
    {
      legend: "[constraints]",
      fields: [
        cInt("max_cycles", "Max dependency cycles."),
        cInt("max_cc", "Max cyclomatic complexity."),
        cInt("max_fn_lines", "Max function lines."),
        cInt("no_god_files", "God-file line threshold."),
        cInt("max_fan_in", "Max fan-in."),
        cInt("max_fan_out", "Max fan-out."),
        { table: "constraints", key: "max_dead", control: "int", initial: initMaxDead(con.max_dead), help: "Max dead symbols (absolute; delta form via the raw pane)." },
        cInt("max_duplicates", "Max duplicate blocks."),
        cInt("max_nesting_depth", "Max nesting depth."),
        cInt("max_brain_methods", "Max brain methods."),
        { table: "constraints", key: "max_clone_ratio", control: "float", initial: initInt(con.max_clone_ratio as number | null | undefined), help: "Max clone ratio (0–1)." },
        { table: "constraints", key: "no_god_containers", control: "bool", initial: initBool(con.no_god_containers), help: "Forbid god containers." },
      ],
    },
    {
      legend: "[metric_thresholds]",
      fields: [
        tInt("nesting_depth", "Nesting-depth calibration."),
        tInt("brain_complexity", "Brain-method complexity."),
        tInt("brain_lines", "Brain-method lines."),
        tInt("brain_nesting", "Brain-method nesting."),
        tInt("god_methods", "God-class method count."),
        tInt("god_span", "God-class line span."),
        { table: "metric_thresholds", key: "clone_similarity", control: "float", initial: initInt(mt.clone_similarity), help: "Clone similarity (0–1)." },
        tInt("clone_min_tokens", "Clone minimum tokens."),
      ],
    },
  ];
}

// ── Honest result-message formatting (ported from the legacy client module) ─────

type ResultKind = "ok" | "warn" | "error";
interface ResultMessage {
  kind: ResultKind;
  text: string;
}

function plural(n: number, word: string): string {
  return `${n} ${word}${n === 1 ? "" : "s"}`;
}

function describeSaved(outcome: ConfigWriteOutcome): string {
  let note = `Saved ${outcome.path} (${outcome.bytes_written} bytes).`;
  if (outcome.provenance_stamped) note += " A provenance comment was stamped into the file.";
  note += " Changes are not applied until you Apply (reindex / re-evaluate).";
  return note;
}

function describeWriteError(status: number, detail: string): string {
  const label = status === 422 ? "Validation error" : status === 400 ? "Bad request" : `Save failed (${status})`;
  return `${label}: ${detail}`;
}

function describeApply(outcome: ConfigApplyOutcome): ResultMessage {
  let kind: ResultKind = "ok";
  let note: string;
  if (outcome.action === "reconciled") {
    note = `Applied — reconciled ${plural(outcome.reconciled_files, "file")}.`;
    if (outcome.full_index) note += " A full index was performed (the graph had not been indexed yet).";
    note += ` ${plural(outcome.unresolved_refs, "unresolved reference")}.`;
    if (outcome.files_failed.length > 0) {
      kind = "warn";
      note += ` Could not read/extract: ${outcome.files_failed.join(", ")}.`;
    }
  } else {
    const signal = outcome.signal == null ? "n/a" : outcome.signal;
    note = `Applied — gate re-evaluated (no reindex). Signal ${signal}, ${plural(outcome.violations, "violation")}. ${outcome.freshness}.`;
  }
  if (outcome.warnings.length > 0) {
    kind = "warn";
    note += ` Warnings: ${outcome.warnings.join("; ")}.`;
  }
  note += " Reload the affected views to see the updated graph / gate.";
  return { kind, text: note };
}

function describeApplyError(status: number, detail: string): string {
  const label = status === 400 ? "Bad request" : status >= 500 ? `Apply failed (${status})` : `Apply rejected (${status})`;
  return `${label}: ${detail}`;
}

/** The honest verify-fault message — a rejected/failed check, never a
 *  fabricated `CONSISTENT` (NFR-RA-05, NFR-UX-04). */
function describeVerifyError(status: number, detail: string): string {
  const label = status >= 500 ? `Verify failed (${status})` : `Verify rejected (${status})`;
  return `${label}: ${detail}`;
}

/** The honest secret-write message — NEVER the raw response body (NFR-SE-07). */
function describeSecret(outcome: SecretWriteOutcome | null): string {
  if (outcome === null) return "Key saved (unexpected response format).";
  if (outcome.chat_key.present) {
    const tail = outcome.chat_key.last4 ? ` (ends …${outcome.chat_key.last4})` : "";
    return `Key saved${tail}. It is stored in ${outcome.path} and never echoed.`;
  }
  return `Key cleared. ${outcome.path} no longer holds a chat key.`;
}

function errorText(e: unknown): string {
  return e instanceof Error ? e.message : "request error";
}

// ── Components ────────────────────────────────────────────────────────────────

/** The honest Save/Apply result panel — assertive for errors, polite otherwise. */
function ResultPanel({ result }: { result: ResultMessage | null }) {
  if (!result) return null;
  return (
    <p
      className={`${styles.result} ${styles[result.kind]}`}
      role={result.kind === "error" ? "alert" : "status"}
      aria-live="polite"
    >
      {result.text}
    </p>
  );
}

/** Render one typed field control bound to its raw-patch handler. */
function FieldControl({
  d,
  value,
  onChange,
}: {
  d: FieldDescriptor;
  value: string;
  onChange: (value: string) => void;
}) {
  const handle = (e: ChangeEvent<HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement>) =>
    onChange(e.target.value);
  const label = d.label ?? d.key;
  if (d.control === "list") {
    return <TextareaField label={label} hint={d.help} rows={3} value={value} onChange={handle} className="mono" spellCheck={false} />;
  }
  if (d.control === "bool") {
    return (
      <SelectField label={label} hint={d.help} value={value} onChange={handle}>
        <option value="">(unset)</option>
        <option value="true">true</option>
        <option value="false">false</option>
      </SelectField>
    );
  }
  if (d.control === "provider") {
    return (
      <SelectField label={label} hint={d.help} value={value} onChange={handle}>
        <option value="openai">openai — OpenAI-compatible (OpenRouter by default)</option>
        <option value="anthropic">anthropic — native Messages API</option>
      </SelectField>
    );
  }
  const numeric = d.control === "int" || d.control === "float";
  return (
    <TextField
      label={label}
      hint={d.help}
      type={numeric ? "number" : "text"}
      {...(d.control === "float" ? { step: "any" } : {})}
      placeholder={d.placeholder}
      value={value}
      onChange={handle}
      className="mono"
    />
  );
}

/** One policy-file editor: typed fields + the authoritative raw pane + Save (with
 *  the rules confirm gate) + the explicit Apply, each with its own result panel. */
function FileEditor({
  file,
  view,
  groups,
  isRules,
}: {
  file: PolicyFile;
  view: FileView<unknown>;
  groups: FieldGroup[];
  isRules: boolean;
}) {
  const [raw, setRaw] = useState<string>(view.content);
  const [values, setValues] = useState<Record<string, string>>(() => {
    const init: Record<string, string> = {};
    for (const g of groups) for (const f of g.fields) init[fieldId(f)] = f.initial;
    return init;
  });
  const [confirmed, setConfirmed] = useState(false);
  const [saveResult, setSaveResult] = useState<ResultMessage | null>(null);
  const [applyResult, setApplyResult] = useState<ResultMessage | null>(null);
  const [saving, setSaving] = useState(false);
  const [applying, setApplying] = useState(false);

  function onFieldChange(d: FieldDescriptor, value: string) {
    setValues((prev) => ({ ...prev, [fieldId(d)]: value }));
    // Patch the one key into the authoritative raw candidate — the single posted
    // source of truth (FR-UI-12). A hand edit and a typed edit both flow here.
    setRaw((prev) => patch(prev, d.table, d.key, controlTomlType(d.control), value));
  }

  async function onSave() {
    // rules.toml confirmation gate (BR-35): no confirmation ⇒ no POST, no write.
    if (isRules && !confirmed) {
      setSaveResult({
        kind: "warn",
        text: "Confirm the change before saving rules.toml — this changes what the gate enforces.",
      });
      return;
    }
    setSaving(true);
    setSaveResult(null);
    try {
      const outcome = await saveConfig(file, raw);
      setSaveResult({ kind: "ok", text: describeSaved(outcome) });
    } catch (e) {
      setSaveResult(
        e instanceof ConfigMutateError
          ? { kind: "error", text: describeWriteError(e.status, e.detail) }
          : { kind: "error", text: `Save failed: ${errorText(e)}` },
      );
    } finally {
      setSaving(false);
    }
  }

  async function onApply() {
    setApplying(true);
    setApplyResult(null);
    try {
      const outcome = await applyConfig(file);
      setApplyResult(describeApply(outcome));
    } catch (e) {
      setApplyResult(
        e instanceof ConfigMutateError
          ? { kind: "error", text: describeApplyError(e.status, e.detail) }
          : { kind: "error", text: `Apply failed: ${errorText(e)}` },
      );
    } finally {
      setApplying(false);
    }
  }

  const title = `${file}.toml`;
  const applyLabel = isRules ? "Apply & re-evaluate" : "Apply & reindex";
  const applyHelp = isRules
    ? "Re-evaluates the gate against the current graph over the saved rules.toml — no reindex. Save first if you have unsaved edits."
    : "Reconciles the graph to the saved config.toml admission policy. Save first if you have unsaved edits.";

  return (
    <Card title={title}>
      <div className={styles.fileHead}>
        <Badge tone={view.exists ? "green" : "muted"}>{view.exists ? "on disk" : "not yet created"}</Badge>
        <span className={styles.path}>{view.path}</span>
      </div>

      {groups.map((g, gi) => (
        <fieldset key={g.legend ?? `g${gi}`} className={styles.group}>
          {g.legend && <legend className={styles.legend}>{g.legend}</legend>}
          <div className={styles.fields}>
            {g.fields.map((d) => (
              <FieldControl
                key={fieldId(d)}
                d={d}
                value={values[fieldId(d)]}
                onChange={(v) => onFieldChange(d, v)}
              />
            ))}
          </div>
        </fieldset>
      ))}

      <TextareaField
        label={`Raw TOML — ${title} (the full document — repeated tables edited here)`}
        value={raw}
        onChange={(e) => setRaw(e.target.value)}
        rows={16}
        spellCheck={false}
        className="mono"
      />

      {isRules && (
        <label className={styles.confirm}>
          <input
            type="checkbox"
            checked={confirmed}
            onChange={(e) => setConfirmed(e.target.checked)}
          />{" "}
          I understand this changes what the <code>gate</code> enforces.
          <span className={styles.help}>
            A confirmed save stamps a provenance comment into the file (BR-35); the stamped file
            still parses via the standard load path.
          </span>
        </label>
      )}

      <div className={styles.actions}>
        <Button variant="primary" onClick={onSave} disabled={saving} aria-busy={saving}>
          {saving ? "Saving…" : `Save ${title}`}
        </Button>
      </div>
      <ResultPanel result={saveResult} />

      <div className={styles.actions}>
        <Button onClick={onApply} disabled={applying} aria-busy={applying}>
          {applying ? (isRules ? "Re-evaluating…" : "Reindexing…") : applyLabel}
        </Button>
      </div>
      <p className={styles.help}>{applyHelp}</p>
      <ResultPanel result={applyResult} />
    </Card>
  );
}

/** The chat API key editor — the one write-only/masked secret (FR-CF-06,
 *  NFR-SE-07). The input is never pre-filled (the browser never receives the
 *  stored key); only the masked presence (set + last-4 / not set) is shown, and a
 *  successful write updates that masked state — the secret is never echoed. */
function SecretEditor({ initial }: { initial: MaskedSecret }) {
  const [masked, setMasked] = useState<MaskedSecret>(initial);
  const [value, setValue] = useState("");
  const [result, setResult] = useState<ResultMessage | null>(null);
  const [saving, setSaving] = useState(false);

  async function onSave() {
    setSaving(true);
    setResult(null);
    try {
      const outcome = await saveSecret(value);
      // Clear the typed secret from state the moment it is persisted.
      setValue("");
      if (outcome) setMasked(outcome.chat_key);
      setResult({ kind: "ok", text: describeSecret(outcome) });
    } catch (e) {
      setResult(
        e instanceof ConfigMutateError
          ? { kind: "error", text: describeWriteError(e.status, e.detail) }
          : { kind: "error", text: `Save failed: ${errorText(e)}` },
      );
    } finally {
      setSaving(false);
    }
  }

  return (
    <Card title="chat API key">
      <div className={styles.fileHead}>
        {masked.present ? (
          <Badge tone="green">set · ends …{masked.last4 ?? ""}</Badge>
        ) : (
          <Badge tone="muted">not set</Badge>
        )}
        <span className={styles.path}>.logos/secrets.toml</span>
      </div>
      <p className={styles.help}>
        The LLM API key for the chat agent. It is a secret: stored in the gitignored{" "}
        <code>.logos/secrets.toml</code> and never echoed — this page only shows whether a key is
        set and its last 4 characters.
      </p>
      <TextField
        label="api_key"
        type="password"
        autoComplete="off"
        spellCheck={false}
        value={value}
        onChange={(e) => setValue(e.target.value)}
        placeholder="enter a new key to replace, or leave blank to clear"
        hint="Write-only. Always blank on load; type a new key to replace, or save it empty to remove the key."
        className="mono"
      />
      <div className={styles.actions}>
        <Button variant="primary" onClick={onSave} disabled={saving} aria-busy={saving}>
          {saving ? "Saving…" : "Save key"}
        </Button>
      </div>
      <ResultPanel result={result} />
    </Card>
  );
}

// ── Graph consistency check (S-207, CR-052, FR-UI-25, FR-GV-19, ADR-46) ────────
// The one Config-tab control that posts (rather than reads): the intent-guarded
// `POST /api/v1/verify` deep check, beside the config.toml Apply action it most
// relates to. The shadow reindex can run seconds-to-minutes (FR-UI-07), so the
// control shows an explicit loading state; a fault renders the honest error
// panel — never a fabricated CONSISTENT (NFR-RA-05, NFR-UX-04).

/** One row of the capped leaked/orphaned-symbol sample table. */
interface SymbolSampleRow {
  kind: "leaked" | "orphaned";
  symbol: string;
}

function symbolSampleRows(report: VerifyReport): SymbolSampleRow[] {
  return [
    ...report.leaked_symbols.map((symbol): SymbolSampleRow => ({ kind: "leaked", symbol })),
    ...report.orphaned_symbols.map((symbol): SymbolSampleRow => ({ kind: "orphaned", symbol })),
  ];
}

/** The verify report (frontend-design §4.14): a green `CONSISTENT` badge on a
 *  clean graph, or a red `DRIFT` callout with the live-vs-reindex deltas, the
 *  structural-check summary, and the capped leaked/orphaned-symbol sample (mono,
 *  in a data table). */
function VerifyReportPanel({ report }: { report: VerifyReport }) {
  if (report.ok) {
    return (
      <div className={styles.verifyResult}>
        <Badge tone="green">CONSISTENT</Badge>
        <p className={styles.help}>{report.message}</p>
      </div>
    );
  }

  const sample = symbolSampleRows(report);
  return (
    <div className={styles.verifyResult}>
      <Callout label="DRIFT" tone="signal">
        <p>{report.message}</p>
        <ul className={styles.deltaList}>
          <li>{`Node delta ${report.node_delta} (live ${report.live.nodes} vs reindex ${report.reindex.nodes})`}</li>
          <li>{`Edge delta ${report.edge_delta} (live ${report.live.edges} vs reindex ${report.reindex.edges})`}</li>
          <li>{`File delta ${report.file_delta} (live ${report.live.files} vs reindex ${report.reindex.files})`}</li>
        </ul>
      </Callout>

      <p className={styles.help}>
        Structural check ({report.structural.ok ? "sound" : "faulty"}):{" "}
        {plural(report.structural.duplicate_symbol_nodes, "duplicate-symbol row")},{" "}
        {plural(report.structural.dangling_file_refs, "dangling file ref")},{" "}
        {plural(report.structural.dangling_edge_endpoints, "dangling edge endpoint")},{" "}
        {plural(report.structural.orphan_shingles, "orphan row")}.
      </p>

      <DataTable
        columns={[
          {
            key: "kind",
            header: "Kind",
            cell: (r) => <Badge tone={r.kind === "leaked" ? "red" : "orange"}>{r.kind}</Badge>,
          },
          { key: "symbol", header: "Symbol", cell: (r) => r.symbol, mono: true },
        ]}
        rows={sample}
        rowKey={(r, i) => `${r.kind}:${i}:${r.symbol}`}
        caption={`Leaked (${report.leaked_total}) / orphaned (${report.orphaned_total}) symbol sample`}
        captionVisible
        empty={<p className={styles.help}>No leaked or orphaned symbols in the sample.</p>}
      />
    </div>
  );
}

/** The "Check graph consistency" control (FR-UI-12) beside the Apply action. */
function GraphConsistencyCard() {
  const [checking, setChecking] = useState(false);
  const [report, setReport] = useState<VerifyReport | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function onCheck() {
    setChecking(true);
    setError(null);
    setReport(null);
    try {
      const result = await verifyGraph();
      setReport(result);
    } catch (e) {
      setError(
        e instanceof ConfigMutateError
          ? describeVerifyError(e.status, e.detail)
          : `Verify failed: ${errorText(e)}`,
      );
    } finally {
      setChecking(false);
    }
  }

  return (
    <Card title="Graph consistency check">
      <p className={styles.help}>
        Re-indexes the project into a throwaway shadow copy and compares it to the live graph —
        this can take a while on a large repo.
      </p>
      <div className={styles.actions}>
        <Button onClick={onCheck} disabled={checking} aria-busy={checking}>
          {checking ? "Checking…" : "Check graph consistency"}
        </Button>
      </div>
      {checking && <LoadingState label="Re-indexing a shadow copy…" />}
      {!checking && error && <ErrorPanel>{error}</ErrorPanel>}
      {!checking && report && <VerifyReportPanel report={report} />}
    </Card>
  );
}

/** The editors over a loaded read-model. */
function ConfigEditor({ model }: { model: ConfigReadModel }): ReactNode {
  return (
    <div className={styles.view}>
      <Callout label="CONFIG EDITOR" tone="muted">
        Edit <code>.logos/config.toml</code> and <code>.logos/rules.toml</code> in place.{" "}
        <strong>Save</strong> validates the whole document and writes it atomically — an invalid
        edit is rejected inline with no partial write. Save does <strong>not</strong> reindex or
        re-evaluate the gate; that is the separate <em>Apply</em> step. A <code>rules.toml</code>{" "}
        save changes what the gate enforces, so it requires explicit confirmation.
      </Callout>
      <FileEditor file="config" view={model.config} groups={configGroups(model.config.parsed)} isRules={false} />
      <GraphConsistencyCard />
      <SecretEditor initial={model.chat_key} />
      <FileEditor file="rules" view={model.rules} groups={rulesGroups(model.rules.parsed)} isRules />
    </div>
  );
}

/** The Config tab (FR-UI-12) — load the read-model, then render the editors. A
 *  present-but-invalid policy file fails the read loud; the shared honesty panel
 *  surfaces the fault rather than a fabricated form (NFR-RA-05). */
export function ConfigView() {
  const model = useApiResource<ConfigReadModel>(() => fetchConfig(), []);
  return (
    <AsyncResource resource={model} loadingLabel="Loading the config…">
      {(m) => <ConfigEditor model={m} />}
    </AsyncResource>
  );
}
