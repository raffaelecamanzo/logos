/*
 * WorkspaceConfigView (S-430, CR-137, FR-UI-38, ADR-31, ADR-56, ADR-66) — the
 * `app`-scoped Config view: the workspace's OWN configuration, edited from the one
 * place in the SPA whose mutating surface reaches a file that governs N
 * repositories.
 *
 * It is a stack of GROUPS, each naming the file it edits. Today there is one — the
 * manifest, `logos.workspace.toml` ({@link ManifestGroup}): `[workspace]`,
 * `[workspace.warm]` and the full `[governance]` family. S-451 adds the workspace
 * `[chat]` / `[wiki]` / credential group as a sibling {@link ConfigGroup} in
 * {@link WorkspaceConfigView}'s body; nothing here has to be restructured for it,
 * and each group owns its own reads, candidate and save state.
 *
 * The editing grammar is the member Config editor's (S-099, FR-UI-12), carried in
 * rather than re-invented: typed fields for the scalar/list keys, a raw-TOML pane
 * holding the whole document — the repeated `[[governance.*]]` tables are edited
 * there — and the raw pane is the **authoritative candidate** posted verbatim. A
 * typed field patches its one key into that text through the member editor's own
 * line-patcher (`../config/toml.ts`), so there is no second source of truth.
 * Validation is the server's: a candidate the manifest parser rejects is refused
 * before the file is touched and the parser's message is shown inline.
 *
 * Two properties are specific to this file and shape the save:
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
 * mode.
 *
 * Absence wording here follows the one taxonomy rather than restating it:
 * `models::quality::absence` in `logos-core/src/models/quality.rs` (S-434) —
 * the closed sentinel vocabulary and the rules (R0-R5) every absence-
 * reporting site keeps. Enumerated from source by
 * `logos-core/tests/absence_taxonomy_audit.rs`.
 */

import { useRef, useState } from "react";
import type { ChangeEvent, ReactNode } from "react";

import { ConfigMutateError } from "../../api/configClient.ts";
import { AsyncResource, useApiResource } from "../../api/hooks.tsx";
import {
  fetchWorkspaceGovernance,
  fetchWorkspaceManifest,
  saveWorkspaceManifest,
} from "../../api/workspaceClient.ts";
import type {
  ManifestSaveOutcome,
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
import { patch, type TomlFieldType } from "../config/toml.ts";
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

// ── The manifest's typed fields ────────────────────────────────────────────────

/** A typed manifest field: the `key` it patches in `table` of the raw candidate. */
interface ManifestField {
  table: string;
  key: string;
  type: TomlFieldType;
  help: string;
  /** Pre-filled from the parsed manifest only — never a default it did not declare. */
  initial: string;
}

/** The typed fields over `[workspace]`, `[workspace.autodiscover]` and
 *  `[workspace.warm]`. The `[[governance.*]]` and `[[links]]` repeated tables do
 *  not formify and are edited in the raw pane (the S-099 grammar). */
function manifestFields(m: WorkspaceManifest): { legend: string; fields: ManifestField[] }[] {
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
        { table: "workspace.autodiscover", key: "enabled", type: "bool", initial: autodiscover, help: "Union immediate child git repositories with the members above. A bare [workspace.autodiscover] table turns this on; enabled = false keeps it declared but off." },
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

function fieldId(f: ManifestField): string {
  return `${f.table}.${f.key}`;
}

function initialValues(parsed: WorkspaceManifest | null): Record<string, string> {
  const values: Record<string, string> = {};
  if (parsed === null) return values;
  for (const g of manifestFields(parsed)) for (const f of g.fields) values[fieldId(f)] = f.initial;
  return values;
}

function FieldControl({
  field,
  value,
  onChange,
}: {
  field: ManifestField;
  value: string;
  onChange: (value: string) => void;
}) {
  const handle = (e: ChangeEvent<HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement>) =>
    onChange(e.target.value);
  if (field.type === "list") {
    return <TextareaField label={field.key} hint={field.help} rows={3} value={value} onChange={handle} className="mono" spellCheck={false} />;
  }
  if (field.type === "bool") {
    return (
      <SelectField label={field.key} hint={field.help} value={value} onChange={handle}>
        <option value="">(not declared)</option>
        <option value="true">true</option>
        <option value="false">false</option>
      </SelectField>
    );
  }
  return (
    <TextField
      label={field.key}
      hint={field.help}
      type={field.type === "int" ? "number" : "text"}
      value={value}
      onChange={handle}
      className="mono"
    />
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

/** The `[governance]` family as the manifest on disk declares it — read-only; the
 *  rules are edited in the raw pane. Renders nothing it was not sent. */
function DeclaredGovernance({ parsed }: { parsed: WorkspaceManifest | null }) {
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
  const [onDisk, setOnDisk] = useState({ parsed: doc.parsed, inEffect: doc.governance_in_effect as boolean | null });
  const governance = useApiResource<WorkspaceGovernanceAnswer>(() => fetchWorkspaceGovernance(), []);

  function onFieldChange(f: ManifestField, value: string) {
    setValues((prev) => ({ ...prev, [fieldId(f)]: value }));
    setRaw((prev) => patch(prev, f.table, f.key, f.type, value));
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
        // Re-read only what the read-back and the rider state. A fingerprint that
        // no longer matches means the disk moved again: drop the claims rather
        // than describe a document this editor did not write (NFR-CC-04).
        fetchWorkspaceManifest().then(
          (fresh) =>
            setOnDisk(
              fresh.fingerprint === outcome.fingerprint
                ? { parsed: fresh.parsed, inEffect: fresh.governance_in_effect }
                : { parsed: null, inEffect: null },
            ),
          () => setOnDisk({ parsed: null, inEffect: null }),
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
        {doc.error === null ? <Badge tone="green">parses</Badge> : <Badge tone="red">does not parse</Badge>}
      </div>
      {doc.error !== null && (
        <ErrorPanel>
          The manifest on disk does not parse — every command in this workspace fails on it until it
          is repaired. Fix it in the raw pane below: {doc.error}
        </ErrorPanel>
      )}

      {groups.map((g) => (
        <fieldset key={g.legend} className={styles.group}>
          <legend className={styles.legend}>{g.legend}</legend>
          <div className={styles.fields}>
            {g.fields.map((f) => (
              <FieldControl key={fieldId(f)} field={f} value={values[fieldId(f)] ?? ""} onChange={(v) => onFieldChange(f, v)} />
            ))}
          </div>
        </fieldset>
      ))}
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
        <DeclaredGovernance parsed={onDisk.parsed} />
        <h4 className={styles.legend}>Findings (GET /api/v1/workspace/check)</h4>
        <AsyncResource resource={governance} loadingLabel="Checking workspace governance…">
          {(answer) => <GovernanceFindings answer={answer} inEffect={onDisk.inEffect} />}
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
      {/* Each group is a sibling ConfigGroup. S-451 adds the workspace [chat] /
          [wiki] / credential group here, beside the manifest. */}
      <ManifestGroup />
    </div>
  );
}
