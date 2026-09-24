/*
 * WorkspaceConfigView (S-430, CR-137, FR-UI-38, ADR-31, ADR-56, ADR-66) — the
 * `app`-scoped Config view: the workspace's OWN configuration, edited from the one
 * place in the SPA whose mutating surface reaches a file that governs N
 * repositories.
 *
 * It is a stack of GROUPS, each naming the file it edits, and each owning its own
 * reads, candidate and save state:
 *
 *   - the manifest, `logos.workspace.toml` ({@link ManifestGroup}, S-430):
 *     `[workspace]`, `[workspace.warm]` and the full `[governance]` family;
 *   - the workspace chat tier, `<workspace-root>/.logos/` ({@link TierGroup},
 *     S-451, FR-WS-30, ADR-67): `[chat]`, `[wiki].model` and the chat credential
 *     every member that declares none inherits — and nothing else. The workspace
 *     root holds no graph, so the group offers no indexing key, no rules document
 *     and no Apply action, and says so on the surface rather than leaving the
 *     absence to be discovered (NFR-CC-04).
 *
 * The editing grammar is the member Config editor's (S-099, FR-UI-12), carried in
 * rather than re-invented: typed fields for the scalar/list keys, a raw-TOML pane
 * holding the whole document — the repeated `[[governance.*]]` tables are edited
 * there — and the raw pane is the **authoritative candidate** posted verbatim. A
 * typed field patches its one key into that text through the member editor's own
 * line-patcher (`../config/toml.ts`), so there is no second source of truth.
 * Validation is the server's: a candidate the parser rejects is refused before
 * the file is touched and the parser's message is shown inline. The tier group
 * follows the same grammar over its own raw pane; its credential is the one field
 * outside it — masked and write-only, never pre-filled (NFR-SE-07).
 *
 * Two properties are specific to the manifest and shape its save:
 *
 *   - **No silent clobber.** The read returns a fingerprint of the bytes it
 *     loaded; the save posts it back, and a manifest changed on disk since (by
 *     hand, or by `logos init --workspace`) is refused with a conflict that shows
 *     what is on disk now. The user then chooses — load the disk copy and discard
 *     their edits, or overwrite it with their edits — and is told which happened.
 *   - **Governance is advisory, said where it is edited.** Workspace rules are
 *     reported at the workspace level and can never move a member's gated signal
 *     (ADR-56). The statement sits beside the raw pane the rules are typed into,
 *     not only beside the findings.
 *
 * The findings `GET /api/v1/workspace/check` reports are rendered beside the
 * parsed governance. The running serve never re-reads the manifest, so after a
 * save that changes the rules those findings are over the rules it STARTED with;
 * `governance_in_effect` is what lets the view say so rather than present them as
 * the verdict on what was just saved (NFR-CC-04).
 *
 * `app`-scoped (FR-UI-35, ADR-66): declared in `nav.ts`, so the shell does not
 * remount it on a member switch, and no read below carries the member in its
 * dependency array, so nothing re-fetches when it changes. The member-scoped
 * Config editor (`../config/ConfigView.tsx`) is a different editor over different
 * files and is untouched by this one; its stylesheet is reused here unmodified so
 * its served class names do not rotate. Unreachable and unrendered in single-root
 * mode, and neither group's routes answer there.
 *
 * Absence wording here follows the one taxonomy rather than restating it:
 * `models::quality::absence` in `logos-core/src/models/quality.rs` (S-434) —
 * the closed sentinel vocabulary and the rules (R0-R5) every absence-
 * reporting site keeps. Enumerated from source by
 * `logos-core/tests/absence_taxonomy_audit.rs`.
 */

import { useRef, useState } from "react";
import type { ChangeEvent, ReactNode } from "react";

import { ApiError } from "../../intent.ts";
import { ConfigMutateError } from "../../api/configClient.ts";
import { AsyncResource, useApiResource } from "../../api/hooks.tsx";
import {
  fetchWorkspaceConfig,
  fetchWorkspaceGovernance,
  fetchWorkspaceManifest,
  saveWorkspaceConfig,
  saveWorkspaceManifest,
  saveWorkspaceSecret,
} from "../../api/workspaceClient.ts";
import type {
  ConfigReadModel,
  ManifestSaveOutcome,
  MaskedSecret,
  ParsedConfig,
  SecretWriteOutcome,
  WorkspaceGovernanceAnswer,
  WorkspaceManifest,
  WorkspaceManifestDocument,
} from "../../api/types.ts";
import {
  Badge,
  Button,
  Callout,
  Card,
  DataTable,
  EmptyState,
  ErrorPanel,
  LoadingState,
  SelectField,
  TextField,
  TextareaField,
} from "../../components/index.ts";
import { useWorkspace } from "../../workspace/WorkspaceContext.tsx";
import { WORKSPACE_CONFIG_FILE, WORKSPACE_SECRETS_FILE } from "../chat/chatModel.ts";
import { dropEmptyTable, patch, type TomlFieldType } from "../config/toml.ts";
import styles from "../config/ConfigView.module.css";

// ── The group frame (shared by every group this view will hold) ───────────────

/** One group of the workspace Config view: a card titled with what it configures
 *  and naming the file it writes, so no group can be mistaken for another's. */
export function ConfigGroup({
  title,
  file,
  children,
}: {
  title: string;
  /** The file this group edits, as the operator would find it on disk. */
  file: string;
  children: ReactNode;
}) {
  return (
    <Card title={title}>
      <div className={styles.fileHead}>
        <span className={styles.path}>{file}</span>
      </div>
      {children}
    </Card>
  );
}

// ── Typed fields (shared by every group's raw pane) ─────────────────────────────

/** A typed field: the `key` it patches in `table` of its group's raw candidate. */
interface TypedField {
  table: string;
  key: string;
  type: TomlFieldType;
  help: string;
  /** Pre-filled from the parsed document only — never a default it did not declare. */
  initial: string;
  /** The rendered label when `key` alone is ambiguous (`[wiki].model` beside
   *  `[chat].model`). Defaults to `key`. */
  label?: string;
  placeholder?: string;
  /** A closed set of string values, rendered as a select (the `[chat]` provider). */
  choices?: readonly { value: string; label: string }[];
}

/** Typed fields under one `[table]` legend, with optional read-only prose beneath
 *  them — never a control, never posted. */
interface FieldGroup {
  legend: string;
  fields: TypedField[];
  note?: ReactNode;
}

// ── The manifest's typed fields ────────────────────────────────────────────────

/** The typed fields over `[workspace]`, `[workspace.autodiscover]` and
 *  `[workspace.warm]`. The `[[governance.*]]` and `[[links]]` repeated tables do
 *  not formify and are edited in the raw pane (the S-099 grammar). */
function manifestFields(m: WorkspaceManifest): FieldGroup[] {
  const w = m.workspace;
  const autodiscover = w.autodiscover ? String(w.autodiscover.enabled) : "";
  const concurrency = w.warm?.concurrency;
  return [
    {
      legend: "[workspace]",
      fields: [
        { table: "workspace", key: "name", type: "str", initial: w.name, help: "The workspace name. Required." },
        { table: "workspace", key: "members", type: "list", initial: (w.members ?? []).join("\n"), help: "Member repository paths, relative to the manifest's directory. One per line." },
        { table: "workspace", key: "default", type: "str", initial: w.default ?? "", help: "The member served when none is selected. Leave blank to declare none." },
      ],
    },
    {
      legend: "[workspace.autodiscover]",
      fields: [
        { table: "workspace.autodiscover", key: "enabled", type: "bool", initial: autodiscover, help: "Union immediate child git repositories with the members above. \"(not declared)\" removes the [workspace.autodiscover] table, which leaves discovery off; enabled = false keeps it declared but off." },
      ],
    },
    {
      legend: "[workspace.warm]",
      fields: [
        { table: "workspace.warm", key: "concurrency", type: "int", initial: concurrency == null ? "" : String(concurrency), help: "How many member indexes the background warm may run at once. Leave blank for the core-derived default; the server states the legal range if a value is outside it." },
      ],
    },
  ];
}

function fieldId(f: TypedField): string {
  return `${f.table}.${f.key}`;
}

/** Each field's pre-fill, keyed by {@link fieldId}. */
function seedValues(groups: FieldGroup[]): Record<string, string> {
  const values: Record<string, string> = {};
  for (const g of groups) for (const f of g.fields) values[fieldId(f)] = f.initial;
  return values;
}

function initialValues(parsed: WorkspaceManifest | null): Record<string, string> {
  return parsed === null ? {} : seedValues(manifestFields(parsed));
}

function FieldControl({
  field,
  value,
  onChange,
}: {
  field: TypedField;
  value: string;
  onChange: (value: string) => void;
}) {
  const handle = (e: ChangeEvent<HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement>) =>
    onChange(e.target.value);
  const label = field.label ?? field.key;
  if (field.type === "list") {
    return <TextareaField label={label} hint={field.help} rows={3} value={value} onChange={handle} className="mono" spellCheck={false} />;
  }
  if (field.type === "bool") {
    return (
      <SelectField label={label} hint={field.help} value={value} onChange={handle}>
        <option value="">(not declared — off)</option>
        <option value="true">true</option>
        <option value="false">false</option>
      </SelectField>
    );
  }
  if (field.choices) {
    return (
      <SelectField label={label} hint={field.help} value={value} onChange={handle}>
        {field.choices.map((c) => (
          <option key={c.value} value={c.value}>
            {c.label}
          </option>
        ))}
      </SelectField>
    );
  }
  return (
    <TextField
      label={label}
      hint={field.help}
      type={field.type === "int" ? "number" : "text"}
      placeholder={field.placeholder}
      value={value}
      onChange={handle}
      className="mono"
    />
  );
}

/** A group's typed fields, one fieldset per `[table]` legend. */
function Fieldsets({
  groups,
  values,
  onChange,
}: {
  groups: FieldGroup[];
  values: Record<string, string>;
  onChange: (field: TypedField, value: string) => void;
}) {
  return (
    <>
      {groups.map((g) => (
        <fieldset key={g.legend} className={styles.group}>
          <legend className={styles.legend}>{g.legend}</legend>
          <div className={styles.fields}>
            {g.fields.map((f) => (
              <FieldControl key={fieldId(f)} field={f} value={values[fieldId(f)] ?? ""} onChange={(v) => onChange(f, v)} />
            ))}
          </div>
          {g.note}
        </fieldset>
      ))}
    </>
  );
}

// ── Governance: the parsed family beside the findings it produces ─────────────

/** The advisory statement, made AT THE POINT OF EDITING (ADR-56, FR-UI-38). */
function GovernanceAdvisory() {
  return (
    <Callout label="ADVISORY" tone="muted">
      Workspace governance is <strong>advisory</strong>. The <code>[governance]</code> rules in this
      manifest are reported at the workspace level only: saving them never moves any member&apos;s
      gated signal or its <code>logos check</code> verdict, and produces no per-repository gate output.
    </Callout>
  );
}

/**
 * What this page knows about the manifest on disk now: the parse of the bytes it
 * last read, or `null` when it no longer knows — a post-save re-read failed, or
 * found bytes this editor did not write. "Unknown" is its own state, never
 * rendered as "does not parse" (NFR-CC-04).
 */
type DiskView = { parsed: WorkspaceManifest | null; error: string | null; inEffect: boolean } | null;

function diskViewOf(doc: WorkspaceManifestDocument): DiskView {
  return { parsed: doc.parsed, error: doc.error, inEffect: doc.governance_in_effect };
}

/** The `[governance]` family as the manifest on disk declares it — read-only; the
 *  rules are edited in the raw pane. Renders nothing it was not sent. */
function DeclaredGovernance({ disk }: { disk: DiskView }) {
  if (disk === null) {
    return (
      <p className={styles.help}>
        The manifest on disk could not be re-read after this save, or changed again since, so its
        rules are not listed here.
      </p>
    );
  }
  const parsed = disk.parsed;
  if (parsed === null) {
    return <p className={styles.help}>The manifest on disk does not parse, so its rules cannot be listed.</p>;
  }
  const g = parsed.governance ?? {};
  const layers = g.service_layers ?? [];
  const boundaries = g.boundaries ?? [];
  const contracts = g.no_cross_service_callers ?? [];
  if (layers.length + boundaries.length + contracts.length === 0) {
    return <p className={styles.help}>The manifest declares no <code>[governance]</code> rules.</p>;
  }
  return (
    <>
      {layers.length > 0 && (
        <DataTable
          caption="[[governance.service_layers]]"
          captionVisible
          columns={[
            { key: "name", header: "Layer", cell: (r) => r.name, mono: true },
            { key: "members", header: "Members", cell: (r) => r.members.join(", "), mono: true },
          ]}
          rows={layers}
          rowKey={(r, i) => `${i}:${r.name}`}
        />
      )}
      {boundaries.length > 0 && (
        <DataTable
          caption="[[governance.boundaries]]"
          captionVisible
          columns={[
            { key: "rule", header: "Forbidden call", cell: (r) => `${r.from} → ${r.to}`, mono: true },
            { key: "reason", header: "Reason", cell: (r) => r.reason ?? "—" },
          ]}
          rows={boundaries}
          rowKey={(r, i) => `${i}:${r.from}->${r.to}`}
        />
      )}
      {contracts.length > 0 && (
        <DataTable
          caption="[[governance.no_cross_service_callers]]"
          captionVisible
          columns={[
            { key: "symbol", header: "Provider symbol", cell: (r) => r.symbol, mono: true },
            { key: "member", header: "Member", cell: (r) => r.member ?? "any", mono: true },
            { key: "reason", header: "Reason", cell: (r) => r.reason ?? "—" },
          ]}
          rows={contracts}
          rowKey={(r, i) => `${i}:${r.symbol}`}
        />
      )}
    </>
  );
}

/** The findings `GET /api/v1/workspace/check` reports, stated over the rules they
 *  were actually evaluated against. */
function GovernanceFindings({
  answer,
  inEffect,
}: {
  answer: WorkspaceGovernanceAnswer;
  /** `false` when the serve evaluates a different `[governance]` from the one on
   *  disk (a save since startup, or an unparsable file); `null` when unknown. */
  inEffect: boolean | null;
}) {
  const report = answer.governance;
  return (
    <div>
      {inEffect !== true && (
        <p className={styles.inherited}>
          {inEffect === false
            ? "These findings were evaluated against the [governance] rules this serve loaded when it started, which differ from the manifest on disk. They will reflect the saved rules after `logos serve` is restarted."
            : "Whether these findings reflect the manifest on disk could not be established."}
        </p>
      )}
      {!answer.complete && (
        <p className={styles.inherited}>
          The check is partial — these members could not be opened:{" "}
          {answer.degraded_rollup.degraded_members.join(", ")}.
        </p>
      )}
      {report === null ? (
        <p className={styles.help}>No governance rules were in effect, so nothing was checked.</p>
      ) : (
        <>
          <p className={styles.help}>
            {report.rules_checked} rule(s) checked over {report.bindings_checked} cross-service
            binding(s): {report.violations.length} violation(s).
          </p>
          {report.unknown_member_refs && report.unknown_member_refs.length > 0 && (
            <p className={styles.inherited}>
              Rules name members this workspace does not have: {report.unknown_member_refs.join(", ")}.
            </p>
          )}
          {report.violations.length > 0 && (
            <DataTable
              caption="Workspace governance findings"
              captionVisible
              columns={[
                { key: "rule", header: "Rule", cell: (v) => v.rule, mono: true },
                { key: "from", header: "Caller", cell: (v) => `${v.from.member} · ${v.from.symbol}`, mono: true },
                { key: "to", header: "Provider", cell: (v) => `${v.to.member} · ${v.to.symbol}`, mono: true },
                { key: "message", header: "Finding", cell: (v) => v.message },
              ]}
              rows={report.violations}
              rowKey={(v, i) => `${i}:${v.rule}`}
            />
          )}
        </>
      )}
    </div>
  );
}

// ── The save ───────────────────────────────────────────────────────────────────

type ResultKind = "ok" | "warn" | "error";
interface ResultMessage {
  kind: ResultKind;
  text: string;
}

/** What a restart changes, stated with every successful write: the serve keeps the
 *  manifest it started with. */
const RESTART_NOTE =
  "The running serve keeps the manifest it started with: members, warm concurrency and the governance findings take effect after `logos serve` is restarted. No member was reindexed.";

function describeOutcome(outcome: ManifestSaveOutcome, overwrote: boolean): ResultMessage {
  switch (outcome.outcome) {
    case "written":
      return {
        kind: "ok",
        text: `${overwrote ? "Overwrote the changes on disk with your edits — " : ""}Saved ${outcome.path} (${outcome.bytes_written} bytes). ${RESTART_NOTE}`,
      };
    case "unchanged":
      return { kind: "ok", text: `No change — ${outcome.path} on disk already matches; nothing was written.` };
    case "conflict":
      return {
        kind: "warn",
        text: `Not saved — ${outcome.path} changed on disk since this editor loaded it. Nothing was written. Choose below which copy wins.`,
      };
  }
}

/** The other resolution of a conflict, stated as plainly as the overwrite is. */
const DISCARDED: ResultMessage = {
  kind: "ok",
  text: "Loaded the version on disk — your unsaved edits were discarded and nothing was written.",
};

function describeError(e: unknown): ResultMessage {
  if (e instanceof ConfigMutateError) {
    const label = e.status === 422 ? "Validation error — nothing was written" : `Save failed (${e.status})`;
    return { kind: "error", text: `${label}: ${e.detail}` };
  }
  return { kind: "error", text: `Save failed: ${e instanceof Error ? e.message : "request error"}` };
}

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

/** The conflict state: what is on disk now, and the two explicit ways out. */
function ConflictPanel({
  disk,
  busy,
  onLoadDisk,
  onOverwrite,
}: {
  disk: string;
  busy: boolean;
  onLoadDisk: () => void;
  onOverwrite: () => void;
}) {
  return (
    <Callout label="CONFLICT" tone="signal">
      <p>
        <code>logos.workspace.toml</code> was changed on disk — by hand, or by{" "}
        <code>logos init --workspace</code> — after this editor loaded it. Your save was refused so
        that change is not silently overwritten.
      </p>
      <TextareaField label="The manifest on disk now" value={disk} readOnly rows={10} className="mono" spellCheck={false} />
      <div className={styles.actions}>
        <Button onClick={onLoadDisk} disabled={busy}>
          Load the version on disk (discard my edits)
        </Button>
        <Button variant="primary" onClick={onOverwrite} disabled={busy} aria-busy={busy}>
          Overwrite it with my edits
        </Button>
      </div>
    </Callout>
  );
}

// ── The manifest group ─────────────────────────────────────────────────────────

/** The editor over one loaded manifest. Keyed by the load in {@link ManifestGroup},
 *  so "load the version on disk" re-seeds it from scratch. */
function ManifestEditor({
  doc,
  notice,
  onReload,
}: {
  doc: WorkspaceManifestDocument;
  /** What the reload that seeded this editor did, stated on arrival. */
  notice: ResultMessage | null;
  /** Re-read the manifest from disk and re-seed this editor from it, stating
   *  `notice` once the new editor is up. */
  onReload: (notice: ResultMessage) => void;
}) {
  const [raw, setRaw] = useState(doc.content);
  const [fingerprint, setFingerprint] = useState(doc.fingerprint);
  const [values, setValues] = useState<Record<string, string>>(() => initialValues(doc.parsed));
  const [conflict, setConflict] = useState<Extract<ManifestSaveOutcome, { outcome: "conflict" }> | null>(null);
  const [result, setResult] = useState<ResultMessage | null>(notice);
  const [saving, setSaving] = useState(false);
  // What the governance read-back and the findings rider describe: the document
  // as last known to be on disk. Refreshed after a write; never from local edits.
  const [onDisk, setOnDisk] = useState<DiskView>(() => diskViewOf(doc));
  const governance = useApiResource<WorkspaceGovernanceAnswer>(() => fetchWorkspaceGovernance(), []);

  function onFieldChange(f: TypedField, value: string) {
    setValues((prev) => ({ ...prev, [fieldId(f)]: value }));
    setRaw((prev) => {
      const next = patch(prev, f.table, f.key, f.type, value);
      // A bare `[workspace.autodiscover]` means ON: an undeclared key must take
      // its table with it (see `dropEmptyTable`).
      return value === "" && f.table === "workspace.autodiscover" ? dropEmptyTable(next, f.table) : next;
    });
  }

  async function save(against: string, overwrote: boolean) {
    setSaving(true);
    setResult(null);
    try {
      const outcome = await saveWorkspaceManifest(raw, against);
      setResult(describeOutcome(outcome, overwrote));
      if (outcome.outcome === "conflict") {
        setConflict(outcome);
        return;
      }
      setConflict(null);
      setFingerprint(outcome.fingerprint);
      if (outcome.outcome === "written") {
        const saved = describeOutcome(outcome, overwrote);
        // Re-read only what the badge, the read-back and the rider state. A
        // fingerprint that no longer matches means the disk moved again: the page
        // stops describing it rather than describe bytes it did not write.
        fetchWorkspaceManifest().then(
          (fresh) => {
            if (fresh.fingerprint !== outcome.fingerprint) return setOnDisk(null);
            // A repair of an unparsable load has no typed state worth keeping, and
            // only a re-seed brings its typed fields up: reload with the message.
            if (doc.parsed === null) return onReload(saved);
            setOnDisk(diskViewOf(fresh));
          },
          () => setOnDisk(null),
        );
      }
    } catch (e) {
      setResult(describeError(e));
    } finally {
      setSaving(false);
    }
  }

  const groups = doc.parsed === null ? [] : manifestFields(doc.parsed);

  return (
    <>
      <div className={styles.fileHead}>
        {onDisk === null ? (
          <Badge tone="muted">on disk: unknown</Badge>
        ) : onDisk.error === null ? (
          <Badge tone="green">parses</Badge>
        ) : (
          <Badge tone="red">does not parse</Badge>
        )}
      </div>
      {onDisk !== null && onDisk.error !== null && (
        <ErrorPanel>
          The manifest on disk does not parse — every command in this workspace fails on it until it
          is repaired. Fix it in the raw pane below: {onDisk.error}
        </ErrorPanel>
      )}

      <Fieldsets groups={groups} values={values} onChange={onFieldChange} />
      {doc.parsed === null && (
        <p className={styles.help}>Typed fields are unavailable while the document does not parse.</p>
      )}

      <fieldset className={styles.group}>
        <legend className={styles.legend}>[governance]</legend>
        <GovernanceAdvisory />
        <p className={styles.help}>
          Service layers, boundaries and no-cross-service-callers contracts are repeated tables:
          edit them in the raw pane below. Declared on disk now:
        </p>
        <DeclaredGovernance disk={onDisk} />
        <h4 className={styles.legend}>Findings (GET /api/v1/workspace/check)</h4>
        <AsyncResource resource={governance} loadingLabel="Checking workspace governance…">
          {(answer) => <GovernanceFindings answer={answer} inEffect={onDisk?.inEffect ?? null} />}
        </AsyncResource>
      </fieldset>

      <TextareaField
        label="Raw TOML — logos.workspace.toml (the full document — [[governance.*]] and [[links]] edited here)"
        value={raw}
        onChange={(e) => setRaw(e.target.value)}
        rows={18}
        spellCheck={false}
        className="mono"
      />

      <div className={styles.actions}>
        <Button variant="primary" onClick={() => void save(fingerprint, false)} disabled={saving} aria-busy={saving}>
          {saving ? "Saving…" : "Save logos.workspace.toml"}
        </Button>
      </div>
      <p className={styles.help}>
        Save validates the whole document with the parser every <code>logos</code> command runs and
        replaces the file atomically; an invalid edit is refused and the file is left untouched. It
        writes only this file — no member&apos;s <code>.logos/</code> — and reindexes nothing.
      </p>
      <ResultPanel result={result} />
      {conflict && (
        <ConflictPanel
          disk={conflict.disk_content}
          busy={saving}
          onLoadDisk={() => onReload(DISCARDED)}
          onOverwrite={() => void save(conflict.disk_fingerprint, true)}
        />
      )}
    </>
  );
}

/** The manifest group: load `logos.workspace.toml`, then edit it. */
function ManifestGroup() {
  const [generation, setGeneration] = useState(0);
  const [notice, setNotice] = useState<ResultMessage | null>(null);
  // Each completed read is its own load, and the editor is keyed on THAT — not on
  // the fingerprint, which can repeat (a save, then `git checkout` of the file,
  // brings the first load's bytes back) and would then remount nothing, and not
  // on `generation`, which changes before the new document has arrived.
  const loads = useRef(0);
  const loaded = useApiResource<{ doc: WorkspaceManifestDocument; load: number }>(
    () => fetchWorkspaceManifest().then((doc) => ({ doc, load: ++loads.current })),
    [generation],
  );
  return (
    <ConfigGroup title="Workspace manifest" file="logos.workspace.toml">
      <AsyncResource resource={loaded} loadingLabel="Loading the manifest…">
        {(l) => (
          <ManifestEditor
            key={l.load}
            doc={l.doc}
            notice={notice}
            onReload={(next) => {
              setNotice(next);
              setGeneration((n) => n + 1);
            }}
          />
        )}
      </AsyncResource>
    </ConfigGroup>
  );
}

// ── The workspace chat tier group (S-451) ──────────────────────────────────────

/** The `[chat]` provider family (mirrors `ChatProvider`). */
const PROVIDERS = [
  { value: "openai", label: "openai — OpenAI-compatible (OpenRouter by default)" },
  { value: "anthropic", label: "anthropic — native Messages API" },
] as const;

/**
 * The tier's typed fields: `[chat]` provider/model/base_url and `[wiki].model` —
 * the keys this root is read for, and only those. Every other `[chat]` key (the
 * budget tree, retry policy, per-role overrides) is edited in the raw pane, as in
 * the member editor.
 *
 * `provider` and `base_url` pre-fill from the parse, which carries their code
 * defaults: on the parsed document "declared the default" and "declared nothing"
 * are indistinguishable for every `[chat]` key but `model` (ADR-67 §3). A default
 * is patched into the raw pane only if the field is edited.
 */
function tierFields(c: ParsedConfig): FieldGroup[] {
  return [
    {
      legend: "[chat]",
      fields: [
        { table: "chat", key: "provider", type: "str", choices: PROVIDERS, initial: c.chat.provider, help: "The provider family. openai is OpenAI-compatible (base_url defaults to OpenRouter); anthropic uses the native Messages endpoint." },
        { table: "chat", key: "model", type: "str", initial: c.chat.model ?? "", placeholder: "leave blank to declare no workspace [chat] table", help: "The model every inheriting member's chat uses. It is what makes this [chat] table inheritable: without it no member inherits the table." },
        { table: "chat", key: "base_url", type: "str", initial: c.chat.base_url, placeholder: "leave blank for the default (OpenRouter)", help: "The OpenAI-compatible endpoint for the openai provider (anthropic ignores this)." },
      ],
    },
    {
      legend: "[wiki]",
      fields: [
        { table: "wiki", key: "model", type: "str", label: "wiki model", initial: c.wiki?.model ?? "", placeholder: "leave blank to declare none", help: "A dedicated wiki-synthesis model, distinct from the chat model." },
      ],
      // Stated because the banner above is about inheritance, and this key is the
      // one it does not cover: the wiki service reads `[wiki]` from the member's
      // root only (`web/src/wikigen/configured.rs`); only the chat halves are
      // two-tier (NFR-CC-04).
      note: (
        <p className={styles.inherited}>
          <strong>Not inherited.</strong> A member&apos;s wiki model is its own <code>[wiki] model</code>,
          else its effective <code>[chat]</code> model — which is the model above for a member that
          inherits this <code>[chat]</code> table. No member reads a <code>[wiki] model</code> from the
          workspace root.
        </p>
      ),
    },
  ];
}

/** Which members take what from this root — the reach of a save, stated where it
 *  is made (ADR-67 §2, FR-WS-30). */
function InheritanceBanner() {
  return (
    <Callout label="INHERITED PER HALF" tone="muted">
      <p>
        What a member takes from this root depends on which half it declares itself — the policy and
        the credential are inherited separately:
      </p>
      <ul>
        <li>
          <strong>Policy.</strong> When this root declares a <code>[chat] model</code>, a member whose own{" "}
          <code>.logos/config.toml</code> declares none inherits this whole <code>[chat]</code> table, and
          dials its endpoint with this root&apos;s key only — never with a key of its own. A member that
          declares a <code>[chat] model</code> owns its whole table; with no model here, nothing is
          inherited.
        </li>
        <li>
          <strong>Credential.</strong> A member that does not inherit this root&apos;s <code>[chat]</code>{" "}
          table — it declares its own, or neither root declares one — and holds no key in its own{" "}
          <code>.logos/secrets.toml</code> uses the key saved here.
        </li>
      </ul>
    </Callout>
  );
}

/** What this group does NOT offer, said rather than left to be discovered
 *  (NFR-CC-04, FR-UI-38). */
function NotHere() {
  return (
    <Callout label="NOT HERE" tone="muted">
      This group edits the workspace root&apos;s chat policy and credential only. It has{" "}
      <strong>no indexing key</strong> (languages, include, exclude, max_file_size, framework_hints) —
      the workspace root is never indexed, so they would change nothing; <strong>no rules document</strong>{" "}
      — workspace rules are the manifest&apos;s <code>[governance]</code> family; and{" "}
      <strong>no Apply action</strong> — there is no graph here to reconcile or re-evaluate. A save
      takes effect on each inheriting member&apos;s next chat turn.
    </Callout>
  );
}

/** The honest secret-write message — never the response body (NFR-SE-07). The
 *  file is named as the operator finds it; the server's `path` is relative to a
 *  root this page does not otherwise name. */
function describeTierSecret(outcome: SecretWriteOutcome | null): string {
  if (outcome === null) return "Key saved (unexpected response format).";
  if (outcome.chat_key.present) {
    const tail = outcome.chat_key.last4 ? ` (ends …${outcome.chat_key.last4})` : "";
    return `Key saved${tail}. It is stored in ${WORKSPACE_SECRETS_FILE} and never echoed.`;
  }
  return `Key cleared. ${WORKSPACE_SECRETS_FILE} no longer holds a chat key.`;
}

/** The workspace credential: masked presence, a write-only input that is always
 *  blank on load, and its own save (FR-CF-06, NFR-SE-07). */
function TierSecret({ initial }: { initial: MaskedSecret }) {
  const [masked, setMasked] = useState<MaskedSecret>(initial);
  const [value, setValue] = useState("");
  const [result, setResult] = useState<ResultMessage | null>(null);
  const [saving, setSaving] = useState(false);

  async function onSave() {
    setSaving(true);
    setResult(null);
    try {
      const outcome = await saveWorkspaceSecret(value);
      // Drop the typed secret the moment it is persisted.
      setValue("");
      if (outcome) setMasked(outcome.chat_key);
      setResult({ kind: "ok", text: describeTierSecret(outcome) });
    } catch (e) {
      setResult(describeError(e));
    } finally {
      setSaving(false);
    }
  }

  return (
    <fieldset className={styles.group}>
      <legend className={styles.legend}>chat API key</legend>
      <div className={styles.fileHead}>
        {masked.present ? (
          <Badge tone="green">set · ends …{masked.last4 ?? ""}</Badge>
        ) : (
          <Badge tone="muted">not set</Badge>
        )}
        <span className={styles.path}>{WORKSPACE_SECRETS_FILE}</span>
      </div>
      <p className={styles.help}>
        The LLM API key inheriting members dial with. It is a secret: stored owner-only in the
        gitignored <code>{WORKSPACE_SECRETS_FILE}</code> and never echoed — this page shows only
        whether a key is set and its last 4 characters.
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
        <Button variant="primary" onClick={() => void onSave()} disabled={saving} aria-busy={saving}>
          {saving ? "Saving…" : "Save the workspace API key"}
        </Button>
      </div>
      <ResultPanel result={result} />
    </fieldset>
  );
}

/** The tier editor over one loaded read-model: typed fields + the authoritative
 *  raw pane over `config.toml`, then the credential. */
function TierEditor({ model }: { model: ConfigReadModel }) {
  const groups = tierFields(model.config.parsed);
  const [raw, setRaw] = useState(model.config.content);
  const [values, setValues] = useState<Record<string, string>>(() => seedValues(groups));
  const [exists, setExists] = useState(model.config.exists);
  const [result, setResult] = useState<ResultMessage | null>(null);
  const [saving, setSaving] = useState(false);

  function onFieldChange(f: TypedField, value: string) {
    setValues((prev) => ({ ...prev, [fieldId(f)]: value }));
    setRaw((prev) => patch(prev, f.table, f.key, f.type, value));
  }

  async function onSave() {
    setSaving(true);
    setResult(null);
    try {
      const outcome = await saveWorkspaceConfig(raw);
      setExists(true);
      setResult({
        kind: "ok",
        text: `Saved ${WORKSPACE_CONFIG_FILE} (${outcome.bytes_written} bytes). Members that inherit it use it from their next chat turn — no restart. No member's .logos/ was written and no member was reindexed.`,
      });
    } catch (e) {
      setResult(describeError(e));
    } finally {
      setSaving(false);
    }
  }

  return (
    <>
      <div className={styles.fileHead}>
        <Badge tone={exists ? "green" : "muted"}>{exists ? "on disk" : "not yet created"}</Badge>
      </div>
      <InheritanceBanner />
      <NotHere />
      <Fieldsets groups={groups} values={values} onChange={onFieldChange} />
      {/* Not labelled "Raw TOML — …" like the manifest's pane: each group's pane is
          named for its own file first, so neither label can be taken for the other. */}
      <TextareaField
        label={`Workspace tier — raw TOML, ${WORKSPACE_CONFIG_FILE} (the full document — the rest of [chat] edited here)`}
        value={raw}
        onChange={(e) => setRaw(e.target.value)}
        rows={12}
        spellCheck={false}
        className="mono"
      />
      <div className={styles.actions}>
        <Button variant="primary" onClick={() => void onSave()} disabled={saving} aria-busy={saving}>
          {saving ? "Saving…" : `Save ${WORKSPACE_CONFIG_FILE}`}
        </Button>
      </div>
      <p className={styles.help}>
        Save validates the whole document with the parser every <code>logos</code> command runs and
        replaces the file atomically; an invalid edit is refused and the file is left untouched. It
        writes only under the workspace root&apos;s <code>.logos/</code> — no member&apos;s — and
        reindexes nothing.
      </p>
      <ResultPanel result={result} />
      <TierSecret initial={model.chat_key} />
    </>
  );
}

/**
 * The workspace chat tier group: load `<workspace-root>/.logos/`, then edit it.
 *
 * Its failed read is stated INSIDE the group as a status, not through
 * {@link AsyncResource}'s alert: the groups load independently, and the page's
 * assertive region belongs to the outcome of a save the user just made — a load
 * failure here must not be announced over, or read as, the manifest group's
 * refusal. The manifest group stays fully usable beside it.
 */
function TierGroup() {
  const loaded = useApiResource<ConfigReadModel>(() => fetchWorkspaceConfig(), []);
  return (
    <ConfigGroup title="Workspace chat policy and credential" file={WORKSPACE_CONFIG_FILE}>
      {loaded.status === "loading" && <LoadingState label="Loading the workspace chat tier…" />}
      {loaded.status === "error" && (
        <Callout label="NOT LOADED" tone="signal">
          The workspace chat tier could not be loaded, so it cannot be edited here:{" "}
          {loaded.error instanceof ApiError
            ? `the request to ${loaded.error.path} failed (HTTP ${loaded.error.status}).`
            : (loaded.error?.message ?? "the request could not be completed.")}
        </Callout>
      )}
      {loaded.status === "ready" && loaded.data && <TierEditor model={loaded.data} />}
    </ConfigGroup>
  );
}

// ── The view ───────────────────────────────────────────────────────────────────

export function WorkspaceConfigView() {
  const { mode, error } = useWorkspace();

  if (mode === "loading") {
    return (
      <div className={styles.view}>
        <LoadingState label="Reading the workspace…" />
      </div>
    );
  }
  if (error) {
    return (
      <div className={styles.view}>
        <ErrorPanel>The workspace could not be read: {error.message}</ErrorPanel>
      </div>
    );
  }
  if (mode !== "workspace") {
    return (
      <div className={styles.view}>
        <EmptyState message="Not a workspace — this serve has a single repository root. Its configuration is edited in the per-service Config view." />
      </div>
    );
  }

  return (
    <div className={styles.view}>
      <Callout label="WORKSPACE CONFIG" tone="muted">
        This view answers for the whole workspace, not the selected service: it edits the
        workspace&apos;s own files, each named beside its group. A service&apos;s{" "}
        <code>.logos/config.toml</code> and <code>rules.toml</code> are edited in the per-service
        Config view.
      </Callout>
      {/* Each group is a sibling ConfigGroup owning its own reads and saves. */}
      <ManifestGroup />
      <TierGroup />
    </div>
  );
}
