/*
 * The Config view's TOML line-patcher (S-191, CR-049, FR-UI-12, BR-35) — the
 * deterministic core ported verbatim from the legacy `web/assets/config-editor.js`
 * so the React hybrid editor preserves the *exact* typed-field ⇄ raw-TOML
 * round-trip the server-rendered view had.
 *
 * Why a line-patcher and not a TOML library: the raw-TOML pane is the
 * **authoritative candidate** — the exact text POSTed to `/config/save` as
 * `content`. A typed field merely *patches its one key* into that text, so a typed
 * edit and a hand edit both flow into the one document the engine validates and
 * atomically writes ([FR-UI-12]). A patch defect can at worst yield a `422` the
 * engine names; it can never cause a silent partial write ([BR-35], the engine
 * re-validates the whole candidate). Keeping this pure (string in, string out) is
 * what makes the round-trip unit-testable without a DOM.
 *
 * The encoder is JSON-based: a TOML basic string is a JSON string for our
 * character set (double-quoted, backslash escapes), so `JSON.stringify` is a
 * correct, dependency-free encoder — matching the legacy module byte-for-byte.
 */

/** The bounded set of typed-field shapes the policy scalars take (mirrors the
 *  server's `FieldKind` / the `data-toml-type` attribute). */
export type TomlFieldType = "int" | "float" | "bool" | "list" | "str";

/** Encode a string as a TOML basic (double-quoted) string. */
function tomlString(s: string): string {
  return JSON.stringify(String(s));
}

/**
 * Serialise a typed field's value to its TOML right-hand side, or `null` when the
 * field is empty — an optional key that should be **removed** so it reverts to its
 * engine default. A cleared `list` removes the key too (never `key = []`, which is
 * a distinct "disable everything"); the explicit empty-array intent stays
 * available by hand in the raw pane.
 */
export function tomlValue(type: TomlFieldType, value: string): string | null {
  if (type === "list") {
    const items = value
      .split("\n")
      .map((s) => s.trim())
      .filter((s) => s.length > 0);
    if (items.length === 0) return null;
    return `[${items.map(tomlString).join(", ")}]`;
  }
  if (type === "bool") {
    return value === "true" ? "true" : value === "false" ? "false" : null;
  }
  if (type === "int" || type === "float") {
    return value.trim() === "" ? null : value.trim();
  }
  if (type === "str") {
    // An optional string scalar (the [chat] provider/model/base_url fields): a
    // blank field removes the key (revert to default/unset); a non-blank value is
    // a quoted TOML basic string. The provider <select> has no blank option, so it
    // always writes one of its two enumerated values.
    return value.trim() === "" ? null : tomlString(value.trim());
  }
  return tomlString(value);
}

/** Escape a string for safe inclusion in a `RegExp` source. */
function escapeRe(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/** The `[start, end)` line range owning `table` ("" = the top-level region above
 *  the first table header), and the table-header index (`-1` when absent). */
interface RegionBounds {
  start: number;
  end: number;
  headerIdx: number;
}

function regionBounds(lines: string[], table: string): RegionBounds {
  if (table === "") {
    let firstHeader = lines.length;
    for (let i = 0; i < lines.length; i++) {
      if (/^\s*\[/.test(lines[i])) {
        firstHeader = i;
        break;
      }
    }
    return { start: 0, end: firstHeader, headerIdx: -1 };
  }
  const headerRe = new RegExp("^\\s*\\[" + escapeRe(table) + "\\]\\s*(#.*)?$");
  let hdr = -1;
  for (let j = 0; j < lines.length; j++) {
    if (headerRe.test(lines[j])) {
      hdr = j;
      break;
    }
  }
  if (hdr === -1) {
    return { start: lines.length, end: lines.length, headerIdx: -1 };
  }
  let end = lines.length;
  for (let k = hdr + 1; k < lines.length; k++) {
    if (/^\s*\[/.test(lines[k])) {
      end = k;
      break;
    }
  }
  return { start: hdr + 1, end, headerIdx: hdr };
}

/**
 * Patch (replace / insert / remove) `key` in `table` of the raw TOML document
 * `raw`, returning the updated document. `table === ""` addresses a top-level key.
 * An empty field value (`tomlValue` ⇒ `null`) removes the key; a non-empty value
 * replaces an existing line, inserts after an existing table header, or creates an
 * absent table. Pure: same inputs ⇒ same output, no DOM.
 */
export function patch(
  raw: string,
  table: string,
  key: string,
  type: TomlFieldType,
  value: string,
): string {
  const serialised = tomlValue(type, value);
  const lines = raw.split("\n");
  const region = regionBounds(lines, table);
  const keyRe = new RegExp("^\\s*" + escapeRe(key) + "\\s*=");
  let found = -1;
  for (let i = region.start; i < region.end; i++) {
    if (keyRe.test(lines[i])) {
      found = i;
      break;
    }
  }
  if (serialised === null) {
    if (found >= 0) lines.splice(found, 1);
    return lines.join("\n");
  }
  const newLine = `${key} = ${serialised}`;
  if (found >= 0) {
    lines[found] = newLine;
  } else if (table !== "" && region.headerIdx >= 0) {
    lines.splice(region.start, 0, newLine); // right after the existing header
  } else if (table !== "") {
    lines.push(`[${table}]`); // create the absent table
    lines.push(newLine);
  } else {
    lines.splice(region.end, 0, newLine); // end of the top-level region
  }
  return lines.join("\n");
}
