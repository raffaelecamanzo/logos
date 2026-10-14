/*
 * The config field groups the mounted extensions add to a Config form (S-730,
 * FR-UI-54, ADR-74 Amendment 1).
 *
 * The member form (`ConfigView`, a member's `config.toml`) and the workspace form
 * (`WorkspaceConfigView`, the workspace root's `config.toml`) each render one of
 * these after their core groups, over the raw candidate they already hold: the
 * registry decides which groups are mounted (`registry.ts`), this renders them.
 * A field reads its value from the candidate text and patches its one key back
 * into it through the core fields' line-patcher (`../views/config/toml.ts`), so the
 * form's existing Save writes it and an extension is never a second writer of the
 * file. Renders nothing when no extension contributes a group.
 *
 * Three rules keep a field honest about the document:
 *
 *   - an unedited field shows what the candidate declares now, so a raw-pane edit
 *     of its key is what the field shows;
 *   - a value the reader cannot type (a hex integer, a multi-line string, an inline
 *     table…) is shown as written, read-only, and never patched;
 *   - a `choice` value outside its options is shown as written and stays selected,
 *     so nothing rewrites it until the user picks another.
 */

import { useState } from "react";
import type { ChangeEvent, ReactNode } from "react";

import { SelectField, TextField, TextareaField } from "../components/index.ts";
import { patch, readValue, type TomlFieldType, type TomlRead } from "../views/config/toml.ts";
import styles from "../views/config/ConfigView.module.css";
import {
  mountedConfigFields,
  useExtensions,
  type ConfigFieldDescriptor,
  type ConfigFieldGroup,
  type ConfigPanelHost,
  type ConfigPanelScope,
} from "./registry.ts";

/** The TOML shape a field patches as: a `choice` is a string. */
function tomlType(field: ConfigFieldDescriptor): TomlFieldType {
  return field.kind === "choice" ? "str" : field.kind;
}

/** Does the candidate's `read` of `field` still say what typing `value` wrote? A
 *  value mid-edit (`1.`, a list with a blank line) is kept while it does; a raw-pane
 *  edit of the key since then makes the field show the candidate instead. */
function holds(read: TomlRead, field: ConfigFieldDescriptor, value: string): boolean {
  const type = tomlType(field);
  const wrote = readValue(patch("", field.table, field.key, type, value), field.table, field.key, type);
  return JSON.stringify(read) === JSON.stringify(wrote);
}

/** A field's key in the form's edit state. */
function fieldId(field: ConfigFieldDescriptor): string {
  return `${field.table}.${field.key}`;
}

/** The field's help, its default hint (styled as the core fields' is) and, for a
 *  value it cannot type, why it is read-only. */
function fieldHint(field: ConfigFieldDescriptor, untyped: boolean): ReactNode {
  return (
    <>
      {field.help}
      {untyped && " This value is written in a form this field cannot edit, so it is left as written; change it in the raw pane."}
      {field.defaultHint && <span className={styles.defaultHint}>{field.defaultHint}</span>}
    </>
  );
}

function FieldControl({
  field,
  value,
  onChange,
}: {
  field: ConfigFieldDescriptor;
  value: string;
  onChange: (value: string) => void;
}) {
  const handle = (e: ChangeEvent<HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement>) =>
    onChange(e.target.value);
  const hint = fieldHint(field, false);
  if (field.kind === "list") {
    return (
      <TextareaField label={field.label} hint={hint} rows={3} value={value} onChange={handle} placeholder={field.placeholder} className="mono" spellCheck={false} />
    );
  }
  if (field.kind === "bool" || field.kind === "choice") {
    const options: readonly string[] = field.kind === "choice" ? field.options : ["true", "false"];
    return (
      <SelectField label={field.label} hint={hint} value={value} onChange={handle}>
        <option value="">{field.placeholder ?? "(unset)"}</option>
        {value !== "" && !options.includes(value) && <option value={value}>{`${value} (not one of the options)`}</option>}
        {options.map((o) => (
          <option key={o} value={o}>
            {o}
          </option>
        ))}
      </SelectField>
    );
  }
  const numeric = field.kind === "int" || field.kind === "float";
  return (
    <TextField
      label={field.label}
      hint={hint}
      type={numeric ? "number" : "text"}
      {...(field.kind === "float" ? { step: "any" } : {})}
      value={value}
      onChange={handle}
      placeholder={field.placeholder}
      className="mono"
    />
  );
}

/** A value the field cannot type, shown as written and read-only — over several
 *  lines when it spans several, since a one-line input would drop its line breaks. */
function UntypedValue({ field, text }: { field: ConfigFieldDescriptor; text: string }) {
  const hint = fieldHint(field, true);
  if (text.includes("\n")) {
    return <TextareaField label={field.label} hint={hint} value={text} readOnly rows={text.split("\n").length} className="mono" spellCheck={false} />;
  }
  return <TextField label={field.label} hint={hint} value={text} readOnly className="mono" />;
}

/** The mounted field groups of `scope`, re-rendering when the boot read settles. */
export function useMountedConfigFields(scope: ConfigPanelScope): readonly ConfigFieldGroup[] {
  useExtensions();
  return mountedConfigFields(scope).map((g) => g.group);
}

/**
 * The mounted extensions' field groups of `scope` over the candidate `raw`. An edit
 * hands `onPatch` the update to apply to the candidate; the caller's Save writes
 * it. `host` is the view's host context, for a note computed from it.
 */
export function ExtensionConfigFields({
  scope,
  raw,
  onPatch,
  host,
}: {
  scope: ConfigPanelScope;
  raw: string;
  onPatch: (update: (raw: string) => string) => void;
  host?: ConfigPanelHost;
}) {
  const groups = useMountedConfigFields(scope);
  // Only what the user typed is held here, and only while the candidate still holds
  // what it wrote; an untouched field, or one whose key the raw pane has since
  // changed, reads the candidate.
  const [edited, setEdited] = useState<Record<string, string>>({});

  function onChange(field: ConfigFieldDescriptor, value: string) {
    setEdited((prev) => ({ ...prev, [fieldId(field)]: value }));
    onPatch((prev) => patch(prev, field.table, field.key, tomlType(field), value));
  }

  return (
    <>
      {groups.map((g, gi) => {
        const note = typeof g.note === "function" ? g.note(host) : g.note;
        return (
          <fieldset key={`${g.legend ?? ""}#${gi}`} className={styles.group} data-extension-fields>
            {g.legend && <legend className={styles.legend}>{g.legend}</legend>}
            {note && <p className={styles.inherited}>{note}</p>}
            <div className={styles.fields}>
              {g.fields.map((f) => {
                const id = fieldId(f);
                const read = readValue(raw, f.table, f.key, tomlType(f));
                if (edited[id] !== undefined && holds(read, f, edited[id])) {
                  return <FieldControl key={id} field={f} value={edited[id]} onChange={(v) => onChange(f, v)} />;
                }
                if (read.kind === "untyped") return <UntypedValue key={id} field={f} text={read.text} />;
                const value = read.kind === "typed" ? read.value : "";
                return <FieldControl key={id} field={f} value={value} onChange={(v) => onChange(f, v)} />;
              })}
            </div>
          </fieldset>
        );
      })}
    </>
  );
}
