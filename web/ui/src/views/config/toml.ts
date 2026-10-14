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
    // An optional string scalar (a manifest's `default`, say): a blank field
    // removes the key (revert to default/unset); a non-blank value is a quoted
    // TOML basic string.
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

/** A line that declares a key (`key = …`), as opposed to a header, a comment or a
 *  blank. */
const KEY_DECL_RE = /^\s*[^\s#[][^=]*=/;

/**
 * Which lines continue a value begun on an earlier line — the tail of a multi-line
 * array or string ({@link valueSpan}). Such a line is never a key or a table
 * header, whatever it spells: `mode = "x"` inside a `"""` string, or the `["a"]`
 * row of a nested array, is the value's content (S-730 review).
 */
function continuations(lines: string[]): boolean[] {
  const inside = lines.map(() => false);
  let i = 0;
  while (i < lines.length) {
    const span = KEY_DECL_RE.test(lines[i]) ? valueSpan(lines, i) : 1;
    for (let k = i + 1; k < i + span; k++) inside[k] = true;
    i += span;
  }
  return inside;
}

function regionBounds(lines: string[], table: string): RegionBounds {
  const inside = continuations(lines);
  if (table === "") {
    let firstHeader = lines.length;
    for (let i = 0; i < lines.length; i++) {
      if (!inside[i] && /^\s*\[/.test(lines[i])) {
        firstHeader = i;
        break;
      }
    }
    return { start: 0, end: firstHeader, headerIdx: -1 };
  }
  const headerRe = new RegExp("^\\s*\\[" + escapeRe(table) + "\\]\\s*(#.*)?$");
  let hdr = -1;
  for (let j = 0; j < lines.length; j++) {
    if (!inside[j] && headerRe.test(lines[j])) {
      hdr = j;
      break;
    }
  }
  if (hdr === -1) {
    return { start: lines.length, end: lines.length, headerIdx: -1 };
  }
  let end = lines.length;
  for (let k = hdr + 1; k < lines.length; k++) {
    if (!inside[k] && /^\s*\[/.test(lines[k])) {
      end = k;
      break;
    }
  }
  return { start: hdr + 1, end, headerIdx: hdr };
}

/**
 * How many lines the `key = value` beginning at `lines[at]` occupies: `1`, unless
 * its value is an array still open at the end of that line — the multi-line shape
 * `toml::to_string_pretty` writes, which is how `logos init --workspace` writes a
 * manifest's `members` — in which case every line through the one that closes it.
 *
 * Without this, patching such a key replaced only its first line and left the
 * array's tail dangling: invalid TOML the server then (correctly) refused, so the
 * typed field could not edit the shape the tool itself writes. Brackets inside a
 * quoted string or after a `#` are not counted. An array never closed before EOF
 * spans only its own line, which is what the patcher did before this existed.
 */
function valueSpan(lines: string[], at: number): number {
  let depth = 0;
  let open: MultilineQuote | null = null;
  for (let i = at; i < lines.length; i++) {
    const scan = bracketDelta(i === at ? lines[i].slice(lines[i].indexOf("=") + 1) : lines[i], open);
    depth += scan.delta;
    open = scan.open;
    if (depth <= 0 && open === null) return i - at + 1;
  }
  return 1;
}

/** A TOML multi-line string delimiter — the one quoting that spans lines. */
type MultilineQuote = '"""' | "'''";

/** Where the string opened by `quote` closes in `text` from index `from`: the
 *  index just past its closing delimiter, or `-1` when it runs past the line.
 *  Only basic strings (`"`, `"""`) honour a backslash escape. */
function closeOf(text: string, from: number, quote: string): number {
  for (let j = from; j < text.length; j++) {
    if (text[j] === "\\" && quote.startsWith('"')) j++;
    else if (text.startsWith(quote, j)) return j + quote.length;
  }
  return -1;
}

/** The net `[` minus `]` count on one line of TOML, skipping brackets inside a
 *  quoted string and everything after a `#` comment — and carrying a multi-line
 *  string `open` at the line's end into the next line, whose brackets are string
 *  content until it closes (a line never ends inside a one-line string). */
function bracketDelta(
  text: string,
  open: MultilineQuote | null,
): { delta: number; open: MultilineQuote | null } {
  let delta = 0;
  let j = 0;
  if (open !== null) {
    j = closeOf(text, 0, open);
    if (j < 0) return { delta, open };
  }
  while (j < text.length) {
    const c = text[j];
    if (c === "#") break;
    if (c === '"' || c === "'") {
      const quote = text.startsWith(c.repeat(3), j) ? c.repeat(3) : c;
      const after = closeOf(text, j + quote.length, quote);
      if (after < 0) return { delta, open: quote.length === 3 ? (quote as MultilineQuote) : null };
      j = after;
      continue;
    }
    if (c === "[") delta++;
    else if (c === "]") delta--;
    j++;
  }
  return { delta, open: null };
}

/** The index of the line declaring `key` within `region`, or `-1`. {@link patch}
 *  and {@link readValue} both address a key through this, so the line a field
 *  reads is always the line it writes. */
function keyLine(lines: string[], region: RegionBounds, key: string): number {
  const keyRe = new RegExp("^\\s*" + escapeRe(key) + "\\s*=");
  const inside = continuations(lines);
  for (let i = region.start; i < region.end; i++) {
    if (!inside[i] && keyRe.test(lines[i])) return i;
  }
  return -1;
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
  const found = keyLine(lines, region, key);
  if (serialised === null) {
    if (found >= 0) lines.splice(found, valueSpan(lines, found));
    return lines.join("\n");
  }
  const newLine = `${key} = ${serialised}`;
  if (found >= 0) {
    lines.splice(found, valueSpan(lines, found), newLine);
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

/** What the candidate declares for one key, as a typed field would hold it. */
export type TomlRead =
  | { kind: "absent" }
  /** The field's string form — the value {@link tomlValue} writes back as it was read. */
  | { kind: "typed"; value: string }
  /** A value the field cannot hold without rewriting it, stated as written. */
  | { kind: "untyped"; text: string };

type Token = { t: "str"; v: string } | { t: "punct"; v: string } | { t: "bare"; v: string };

/** Split a right-hand side into strings, `[` `]` `,` and bare words, dropping
 *  whitespace and comments; `null` for a shape the reader does not decode — a
 *  multi-line string, an unterminated one, or a basic-string escape JSON does not
 *  share with TOML (`\U…`, `\e`). */
function tokens(text: string): Token[] | null {
  const out: Token[] = [];
  let j = 0;
  while (j < text.length) {
    const c = text[j];
    if (c === '"' || c === "'") {
      const str = stringToken(text, j, c);
      if (str === null) return null;
      out.push(str.token);
      j = str.end;
      continue;
    }
    const step = /^\s/.test(c) ? 1 : c === "#" ? commentLength(text, j) : 0;
    if (step > 0) {
      j += step;
      continue;
    }
    const v = "[],".includes(c) ? c : /^[^\s,[\]#"']+/.exec(text.slice(j))![0];
    out.push({ t: "[],".includes(c) ? "punct" : "bare", v });
    j += v.length;
  }
  return out;
}

/** The length of the comment starting at `text[at]`, up to (not including) its line end. */
function commentLength(text: string, at: number): number {
  const nl = text.indexOf("\n", at);
  return (nl < 0 ? text.length : nl) - at;
}

/** The one-line string opened by `quote` at `text[at]`, decoded, and the index just
 *  past it; `null` for a multi-line or unterminated string, or an escape JSON does
 *  not share with TOML. A literal (`'`) string is its content verbatim. */
function stringToken(text: string, at: number, quote: string): { token: Token; end: number } | null {
  if (text.startsWith(quote.repeat(3), at)) return null;
  const end = closeOf(text, at + 1, quote);
  if (end < 0 || text.slice(at, end).includes("\n")) return null;
  const body = text.slice(at, end);
  if (quote === "'") return { token: { t: "str", v: body.slice(1, -1) }, end };
  try {
    return { token: { t: "str", v: JSON.parse(body) as string }, end };
  } catch {
    return null;
  }
}

/** A string a text field holds and {@link tomlValue} writes back unchanged: not
 *  empty, not padded (the encoder trims), on one line. */
function fieldString(s: string): boolean {
  return s !== "" && s === s.trim() && !/[\r\n]/.test(s);
}

const INT_RE = /^[+-]?(0|[1-9](_?\d)*)$/;
const FLOAT_RE = /^[+-]?(0|[1-9](_?\d)*)(\.\d(_?\d)*)?([eE][+-]?\d(_?\d)*)?$/;

/** The field form of a decimal TOML number: digit separators and a leading `+`
 *  dropped, which is how a number input states it. */
function fieldNumber(bare: string): string {
  return bare.replace(/_/g, "").replace(/^\+/, "");
}

/** `toks` as the field of `type` would hold it, or `null` when it cannot. */
function typedValue(type: TomlFieldType, toks: Token[]): string | null {
  if (type === "list") {
    const items: string[] = [];
    const [open, close] = [toks[0], toks[toks.length - 1]];
    if (toks.length < 2 || open.t !== "punct" || open.v !== "[" || close.t !== "punct" || close.v !== "]") return null;
    const inner = toks.slice(1, -1);
    for (let i = 0; i < inner.length; i += 2) {
      const item = inner[i];
      const sep = inner[i + 1];
      if (item.t !== "str" || !fieldString(item.v)) return null;
      if (sep !== undefined && (sep.t !== "punct" || sep.v !== ",")) return null;
      items.push(item.v);
    }
    // `[]` declares "nothing", which a cleared field cannot say: clearing removes the key.
    return items.length === 0 ? null : items.join("\n");
  }
  if (toks.length !== 1) return null;
  const [tok] = toks;
  if (type === "str") return tok.t === "str" && fieldString(tok.v) ? tok.v : null;
  if (tok.t !== "bare") return null;
  if (type === "bool") return tok.v === "true" || tok.v === "false" ? tok.v : null;
  const re = type === "int" ? INT_RE : FLOAT_RE;
  return re.test(tok.v) ? fieldNumber(tok.v) : null;
}

/**
 * Read `key` in `table` of the raw candidate as a typed field of `type` would hold
 * it (S-730, FR-UI-54) — the line {@link patch} would write. A value the field
 * cannot hold without changing it — a hex integer, a multi-line or padded string,
 * an inline table, an empty array, a mixed array — is `untyped` and carried as
 * written, so the caller can show it and leave it alone. Pure, like
 * {@link patch}.
 */
export function readValue(raw: string, table: string, key: string, type: TomlFieldType): TomlRead {
  const lines = raw.split("\n");
  const at = keyLine(lines, regionBounds(lines, table), key);
  if (at < 0) return { kind: "absent" };
  const span = lines.slice(at, at + valueSpan(lines, at));
  span[0] = span[0].slice(span[0].indexOf("=") + 1);
  const text = span.join("\n");
  const toks = tokens(text);
  const value = toks === null ? null : typedValue(type, toks);
  return value === null ? { kind: "untyped", text: text.trim() } : { kind: "typed", value };
}

/**
 * Remove the `[table]` header from `raw` when the table no longer holds any key —
 * only blank and comment lines between it and the next header. Returns `raw`
 * unchanged when the table is absent or still declares a key.
 *
 * For a table whose mere PRESENCE means something: a bare
 * `[workspace.autodiscover]` turns discovery ON (its `enabled` defaults to
 * `true`), so clearing `enabled` must take the header with it, or "not declared"
 * would save the opposite of an operator's `enabled = false`. Pure, like
 * {@link patch}.
 */
export function dropEmptyTable(raw: string, table: string): string {
  const lines = raw.split("\n");
  const region = regionBounds(lines, table);
  if (region.headerIdx < 0) return raw;
  const holdsKey = lines.slice(region.start, region.end).some((l) => l.trim() !== "" && !l.trim().startsWith("#"));
  if (holdsKey) return raw;
  lines.splice(region.headerIdx, 1);
  return lines.join("\n");
}
