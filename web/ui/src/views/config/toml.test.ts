/*
 * Unit tests for the Config editor's TOML line-patcher (S-191, FR-UI-12) — the
 * deterministic typed-field ⇄ raw-TOML round-trip. Pure string-in/string-out, so
 * no DOM is needed. Mirrors the legacy `config-editor.js` patch semantics: replace
 * in place, insert after an existing header, create an absent table, and remove a
 * cleared optional key.
 */

import { describe, expect, it } from "vitest";

import legacyEditor from "../../../../assets/config-editor.js?raw";
import { dropEmptyTable, patch, readValue, tomlValue, type TomlFieldType } from "./toml.ts";

describe("tomlValue — typed-field serialisation (S-191, FR-UI-12)", () => {
  it("encodes a non-empty list, and removes the key when cleared", () => {
    expect(tomlValue("list", "rust\npython")).toBe('["rust", "python"]');
    // A cleared list removes the key (revert to default), never `key = []`.
    expect(tomlValue("list", "   \n  ")).toBeNull();
  });

  it("encodes a string scalar quoted, and removes it when blank", () => {
    expect(tomlValue("str", "claude-x")).toBe('"claude-x"');
    expect(tomlValue("str", "  ")).toBeNull();
  });

  it("passes int/float through trimmed, removing on blank", () => {
    expect(tomlValue("int", " 42 ")).toBe("42");
    expect(tomlValue("float", "0.85")).toBe("0.85");
    expect(tomlValue("int", "")).toBeNull();
  });

  it("maps the bool tri-state (true/false/unset)", () => {
    expect(tomlValue("bool", "true")).toBe("true");
    expect(tomlValue("bool", "false")).toBe("false");
    expect(tomlValue("bool", "")).toBeNull();
  });
});

describe("patch — typed field into the raw candidate (S-191, FR-UI-12)", () => {
  it("replaces an existing top-level key in place", () => {
    const raw = 'languages = ["rust"]\nmax_file_size = 1048576\n';
    expect(patch(raw, "", "max_file_size", "int", "2097152")).toBe(
      'languages = ["rust"]\nmax_file_size = 2097152\n',
    );
  });

  it("replaces a key inside its owning [table], not a same-named top-level key", () => {
    const raw = 'model = "top"\n\n[tool]\nprovider = "openai"\nmodel = "old"\n';
    const next = patch(raw, "tool", "model", "str", "claude-x");
    expect(next).toContain('[tool]\nprovider = "openai"\nmodel = "claude-x"');
    // The top-level same-named key is untouched (region-scoped replace).
    expect(next).toContain('model = "top"');
  });

  it("inserts a new key right after an existing table header", () => {
    const raw = '[tool]\nprovider = "openai"\n';
    expect(patch(raw, "tool", "model", "str", "claude-x")).toBe(
      '[tool]\nmodel = "claude-x"\nprovider = "openai"\n',
    );
  });

  it("creates an absent table when the key has nowhere to go", () => {
    const raw = 'languages = ["rust"]\n';
    const next = patch(raw, "constraints", "max_cc", "int", "10");
    expect(next).toContain("[constraints]");
    expect(next).toContain("max_cc = 10");
  });

  it("removes a cleared optional key (revert to default), leaving the rest intact", () => {
    const raw = '[constraints]\nmax_cc = 10\nmax_fn_lines = 80\n';
    expect(patch(raw, "constraints", "max_cc", "int", "")).toBe(
      '[constraints]\nmax_fn_lines = 80\n',
    );
  });

  it("round-trips: a value set then cleared returns the original document", () => {
    const raw = '[tool]\nprovider = "openai"\n';
    const set = patch(raw, "tool", "model", "str", "claude-x");
    const cleared = patch(set, "tool", "model", "str", "");
    expect(cleared).toBe(raw);
  });
});

describe("patch — a multi-line array value is one value (S-430, FR-UI-38)", () => {
  // The exact shape `logos init --workspace` writes (`toml::to_string_pretty`):
  // the dominant real-world `members`, so it is the fixture, not a tidy one-liner.
  const INIT_WRITTEN = [
    "[workspace]",
    'name = "shop"',
    "members = [",
    '    "api",',
    '    "web",',
    "]",
    'default = "api"',
    "",
    "[workspace.warm]",
    "concurrency = 2",
  ].join("\n");

  it("replaces every line of the array, leaving no dangling tail", () => {
    const out = patch(INIT_WRITTEN, "workspace", "members", "list", "api\nweb\nworker");
    expect(out).toBe(
      [
        "[workspace]",
        'name = "shop"',
        'members = ["api", "web", "worker"]',
        'default = "api"',
        "",
        "[workspace.warm]",
        "concurrency = 2",
      ].join("\n"),
    );
  });

  it("removes every line of the array when the field is cleared", () => {
    const out = patch(INIT_WRITTEN, "workspace", "members", "list", "");
    expect(out.split("\n").slice(0, 3)).toEqual(["[workspace]", 'name = "shop"', 'default = "api"']);
  });

  it("does not count a bracket inside a string or a comment", () => {
    const raw = ["[workspace]", 'members = [ # a [ comment', '    "a]b",', "]", 'default = "x"'].join("\n");
    const out = patch(raw, "workspace", "members", "list", "c");
    expect(out).toBe(["[workspace]", 'members = ["c"]', 'default = "x"'].join("\n"));
  });

  it("does not count brackets inside a multi-line string, and never cuts past its array", () => {
    // Valid TOML (checked with tomllib) whose `]` and `[` sit inside `"""`/`'''`
    // strings. Scanning each line afresh deleted `default` and `name` on the first,
    // and left a dangling `[''', "y"]` on the second.
    const basic = ["[workspace]", 'members = ["""', 'api""", "web"]', 'default = "api"', 'name = """', ']shop"""'].join("\n");
    expect(patch(basic, "workspace", "members", "list", "api\nweb")).toBe(
      ["[workspace]", 'members = ["api", "web"]', 'default = "api"', 'name = """', ']shop"""'].join("\n"),
    );
    const literal = ["[workspace]", "members = ['''x", "[''', \"y\"]", 'default = "x"'].join("\n");
    expect(patch(literal, "workspace", "members", "list", "z")).toBe(
      ["[workspace]", 'members = ["z"]', 'default = "x"'].join("\n"),
    );
  });

  it("leaves a one-line array a one-line replacement", () => {
    const raw = ["[workspace]", 'members = ["a"]', 'default = "a"'].join("\n");
    expect(patch(raw, "workspace", "members", "list", "b")).toBe(
      ["[workspace]", 'members = ["b"]', 'default = "a"'].join("\n"),
    );
  });

  it("touches only the key's own line when an array is never closed", () => {
    const raw = ["[workspace]", "members = [", '"a",'].join("\n");
    expect(patch(raw, "workspace", "members", "list", "b")).toBe(
      ["[workspace]", 'members = ["b"]', '"a",'].join("\n"),
    );
  });
});

describe("dropEmptyTable — a table whose presence is a value (S-430, FR-UI-38)", () => {
  it("removes a header left holding no key, and nothing else", () => {
    const raw = ["[workspace]", 'name = "a"', "", "[workspace.autodiscover]", "", "[workspace.warm]", "concurrency = 2"].join("\n");
    expect(dropEmptyTable(raw, "workspace.autodiscover")).toBe(
      ["[workspace]", 'name = "a"', "", "", "[workspace.warm]", "concurrency = 2"].join("\n"),
    );
  });

  it("treats a comment as no key: the header goes, the operator's comment stays", () => {
    const raw = ["[workspace.autodiscover]", "# turned off while api/ is split", "[workspace.warm]"].join("\n");
    expect(dropEmptyTable(raw, "workspace.autodiscover")).toBe(["# turned off while api/ is split", "[workspace.warm]"].join("\n"));
  });

  it("keeps a table that still declares a key, and leaves an absent one alone", () => {
    const raw = ["[workspace.autodiscover]", "# why", "enabled = true"].join("\n");
    expect(dropEmptyTable(raw, "workspace.autodiscover")).toBe(raw);
    expect(dropEmptyTable('[workspace]\nname = "a"', "workspace.autodiscover")).toBe('[workspace]\nname = "a"');
  });
});

describe("patch — a multi-line array survives a typed edit of its table's other keys (sprint-79 HF-1)", () => {
  // `read_roots` has no typed control: it is edited in the raw pane, so a typed
  // provider/model/base_url edit must carry it through byte-for-byte — including
  // the multi-line array shape `toml::to_string_pretty` writes.
  const raw =
    '[tool]\nread_roots = [\n  "../logos-docs",\n  "/srv/specs",\n]\nmodel = "old/model"\nbase_url = "https://x.example/v1"\n';
  const roots = 'read_roots = [\n  "../logos-docs",\n  "/srv/specs",\n]\n';

  it("replaces model after the array without touching it", () => {
    const out = patch(raw, "tool", "model", "str", "new/model");
    expect(out).toContain(roots);
    expect(out).toContain('model = "new/model"');
    expect(out).not.toContain("old/model");
  });

  it("removes a cleared base_url without touching the array", () => {
    const out = patch(raw, "tool", "base_url", "str", "");
    expect(out).toBe('[tool]\nread_roots = [\n  "../logos-docs",\n  "/srv/specs",\n]\nmodel = "old/model"\n');
  });

  it("inserts an absent provider after the header, above the array", () => {
    const out = patch(raw, "tool", "provider", "str", "anthropic");
    expect(out.startsWith('[tool]\nprovider = "anthropic"\n' + roots)).toBe(true);
  });

  it("never mistakes an array item spelling `model =` for the key (the near miss)", () => {
    const tricky = '[tool]\nread_roots = [\n  "model = inside",\n]\nmodel = "old/model"\n';
    const out = patch(tricky, "tool", "model", "str", "new/model");
    expect(out).toBe('[tool]\nread_roots = [\n  "model = inside",\n]\nmodel = "new/model"\n');
  });
});

describe("readValue — a typed field's value read from the candidate text (S-730, FR-UI-54)", () => {
  const DOC = [
    'top = "level"',
    "",
    "[probe]",
    "count = 1_000 # a comment",
    "ratio = 0.25",
    "on = false",
    "label = 'literal'",
    'escaped = "a \\"quoted\\" word"',
    "names = [",
    '    "alpha",',
    "    'beta', # a comment inside the array",
    "]",
    "",
    "[other]",
    "count = 7",
    "",
  ].join("\n");

  it("reads each kind from its own table, in the form its field shows", () => {
    expect(readValue(DOC, "probe", "count", "int")).toEqual({ kind: "typed", value: "1000" });
    expect(readValue(DOC, "other", "count", "int")).toEqual({ kind: "typed", value: "7" });
    expect(readValue(DOC, "probe", "ratio", "float")).toEqual({ kind: "typed", value: "0.25" });
    expect(readValue(DOC, "probe", "on", "bool")).toEqual({ kind: "typed", value: "false" });
    expect(readValue(DOC, "probe", "label", "str")).toEqual({ kind: "typed", value: "literal" });
    expect(readValue(DOC, "probe", "escaped", "str")).toEqual({ kind: "typed", value: 'a "quoted" word' });
    expect(readValue(DOC, "probe", "names", "list")).toEqual({ kind: "typed", value: "alpha\nbeta" });
    expect(readValue(DOC, "", "top", "str")).toEqual({ kind: "typed", value: "level" });
  });

  it("reports a key the table does not declare, or a table the document lacks, as absent", () => {
    expect(readValue(DOC, "probe", "missing", "str")).toEqual({ kind: "absent" });
    expect(readValue(DOC, "nowhere", "count", "int")).toEqual({ kind: "absent" });
    // The near miss: `count` is declared in [probe], never at the top level.
    expect(readValue(DOC, "", "count", "int")).toEqual({ kind: "absent" });
  });

  it("leaves a value the field cannot show as untyped, stating it as written", () => {
    const doc = [
      "[probe]",
      'count = "12"',
      "hex = 0xff",
      "ratio = inf",
      "on = 1",
      "name = 42",
      'multi = """',
      'two lines"""',
      "inline = { a = 1 }",
      "mixed = [1, \"a\"]",
      "empty = []",
      'blank = ""',
      'padded = " x "',
      'unicode = "\\U0001F600"',
      'items = ["ok", " padded "]',
      "",
    ].join("\n");
    expect(readValue(doc, "probe", "count", "int")).toEqual({ kind: "untyped", text: '"12"' });
    expect(readValue(doc, "probe", "hex", "int")).toEqual({ kind: "untyped", text: "0xff" });
    expect(readValue(doc, "probe", "ratio", "float")).toEqual({ kind: "untyped", text: "inf" });
    expect(readValue(doc, "probe", "on", "bool")).toEqual({ kind: "untyped", text: "1" });
    expect(readValue(doc, "probe", "name", "str")).toEqual({ kind: "untyped", text: "42" });
    expect(readValue(doc, "probe", "multi", "str")).toEqual({ kind: "untyped", text: '"""\ntwo lines"""' });
    expect(readValue(doc, "probe", "inline", "str")).toEqual({ kind: "untyped", text: "{ a = 1 }" });
    expect(readValue(doc, "probe", "mixed", "list")).toEqual({ kind: "untyped", text: '[1, "a"]' });
    // `[]` is "nothing", which a cleared list field cannot say: clearing removes the key.
    expect(readValue(doc, "probe", "empty", "list")).toEqual({ kind: "untyped", text: "[]" });
    // An empty or padded string would be trimmed (or removed) by the field's encoder.
    expect(readValue(doc, "probe", "blank", "str")).toEqual({ kind: "untyped", text: '""' });
    expect(readValue(doc, "probe", "padded", "str")).toEqual({ kind: "untyped", text: '" x "' });
    expect(readValue(doc, "probe", "unicode", "str")).toEqual({ kind: "untyped", text: '"\\U0001F600"' });
    expect(readValue(doc, "probe", "items", "list")).toEqual({ kind: "untyped", text: '["ok", " padded "]' });
  });

  it("round-trips typed → raw → typed for every kind, and raw → typed → raw leaves the text as patched", () => {
    const cases: [TomlFieldType, string][] = [
      ["int", "42"],
      ["int", "-3"],
      ["float", "0.5"],
      ["float", "1e-3"],
      ["bool", "true"],
      ["bool", "false"],
      ["str", 'with "quotes" and \\ backslash'],
      ["list", "one\ntwo # not a comment"],
    ];
    for (const [type, value] of cases) {
      const raw = patch("[probe]\nkeep = 1\n", "probe", "k", type, value);
      expect(readValue(raw, "probe", "k", type)).toEqual({ kind: "typed", value });
      expect(patch(raw, "probe", "k", type, value)).toBe(raw);
    }
  });

  it("reads a cleared field's key as absent: an empty value removes it", () => {
    for (const type of ["int", "float", "bool", "str", "list"] as const) {
      const raw = patch('[probe]\nk = "x"\n', "probe", "k", type, "");
      expect(readValue(raw, "probe", "k", type)).toEqual({ kind: "absent" });
    }
  });
});

describe("a line inside an earlier key's value is never a key or a table header (S-730 review)", () => {
  it("reads and patches the real key, not a `key =` line inside a multi-line string", () => {
    const raw = '[stub]\nabout = """\nmode = "fake"\n"""\nmode = "real"\n';
    expect(readValue(raw, "stub", "mode", "str")).toEqual({ kind: "typed", value: "real" });
    expect(patch(raw, "stub", "mode", "str", "b")).toBe(raw.replace('mode = "real"', 'mode = "b"'));
  });

  it("never takes a `[table]` line inside another table's multi-line string for that table's header", () => {
    const raw = "[other]\ntext = '''\n[stub]\nmode = \"x\"\n'''\n\n[stub]\nmode = \"real\"\n";
    expect(readValue(raw, "stub", "mode", "str")).toEqual({ kind: "typed", value: "real" });
    expect(patch(raw, "stub", "mode", "str", "b")).toBe(raw.replace('mode = "real"', 'mode = "b"'));
  });

  it("does not end a table, or the top-level region, at a line inside a multi-line value", () => {
    const nested = '[stub]\nmatrix = [\n  ["a", "b"],\n]\nmode = "real"\n';
    expect(readValue(nested, "stub", "mode", "str")).toEqual({ kind: "typed", value: "real" });
    expect(patch(nested, "stub", "mode", "str", "b")).toBe(nested.replace('mode = "real"', 'mode = "b"'));
    const top = 'about = """\n[stub]\n"""\ntitle = "real"\n';
    expect(readValue(top, "", "title", "str")).toEqual({ kind: "typed", value: "real" });
    expect(readValue(top, "stub", "title", "str")).toEqual({ kind: "absent" });
  });
});

describe("readValue — the number and list shapes near the edges (S-730 review)", () => {
  it("types a leading `+` and drops it, as a number input states it", () => {
    expect(readValue("[p]\nk = +5\n", "p", "k", "int")).toEqual({ kind: "typed", value: "5" });
  });

  it("leaves a leading-zero integer and a list missing its separator untyped", () => {
    expect(readValue("[p]\nk = 007\n", "p", "k", "int")).toEqual({ kind: "untyped", text: "007" });
    expect(readValue('[p]\nk = ["a" "b"]\n', "p", "k", "list")).toEqual({ kind: "untyped", text: '["a" "b"]' });
  });
});

/*
 * `escapeRe` builds a RegExp from a table or key name (HF-4). A static scanner flags
 * `new RegExp(...)` over a non-literal; what makes it safe is that no regex
 * metacharacter survives the escape, so the name matches only itself. Every
 * character `RegExp` gives a meaning outside a character class is in the name below,
 * and the lookalikes are the lines an unescaped metacharacter would also match.
 */
const METACHARS = ".*+?^${}()|[]\\";

/** The legacy editor's `escapeRe`, read out of the shipped script. */
const legacyEscapeRe = new Function(
  `${/function escapeRe\(s\) \{[\s\S]*?\n {2}\}/.exec(legacyEditor)![0]}\nreturn escapeRe;`,
)() as (s: string) => string;

describe("escapeRe — a name full of regex metacharacters matches only itself (HF-4)", () => {
  it("the legacy editor's escapeRe escapes every metacharacter and nothing else", () => {
    for (const c of METACHARS) {
      expect(legacyEscapeRe(c)).toBe("\\" + c);
      // `new RegExp(escapeRe(c))` matches the character and nothing else.
      expect(new RegExp("^" + legacyEscapeRe(c) + "$").test(c)).toBe(true);
      expect(new RegExp("^" + legacyEscapeRe(c) + "$").test("x")).toBe(false);
    }
    expect(legacyEscapeRe("a-b_c/d")).toBe("a-b_c/d"); // not special outside a class
  });

  it("the SPA patcher addresses the one table whose name is made of metacharacters, and no lookalike", () => {
    const table = "a.b*c+d?e^f$g{1}(h)|i[j]k\\l";
    // The first header is the table name's own prefix, before an unescaped `|`; it must
    // stay a different table from the real one.
    const raw = `[a.b*c+d?e^f$g{1}(h)]\nk = "prefix"\n\n[aXb]\nk = "lookalike"\n\n[${table}]\nk = "real"\n\n[a.b]\nk = "other"\n`;
    expect(readValue(raw, table, "k", "str")).toEqual({ kind: "typed", value: "real" });
    expect(patch(raw, table, "k", "str", "b")).toBe(raw.replace('k = "real"', 'k = "b"'));
    // `a.b` is a real table here, and `.` does not reach `aXb`.
    expect(readValue(raw, "a.b", "k", "str")).toEqual({ kind: "typed", value: "other" });
    expect(readValue("[aXb]\nk = 1\n", "a.b", "k", "int")).toEqual({ kind: "absent" });
  });

  it("the SPA patcher addresses the one key whose name is made of metacharacters, and no lookalike", () => {
    const key = "a.b*c+d?e^f$g{1}(h)|i[j]k\\l";
    const raw = `[t]\naXb = 1\na.b*c+d?e^f$g{1}(h) = 0\n${key} = 2\n`;
    expect(readValue(raw, "t", key, "int")).toEqual({ kind: "typed", value: "2" });
    expect(patch(raw, "t", key, "int", "9")).toBe(raw.replace(`${key} = 2`, `${key} = 9`));
    // The key never matches a line it only resembles: `.` is not "any character",
    // `*` is not a repeat, `(` does not open a group (it would throw unescaped).
    expect(readValue("[t]\naXb = 1\n", "t", "a.b", "int")).toEqual({ kind: "absent" });
    expect(readValue("[t]\nab = 1\n", "t", "a*b", "int")).toEqual({ kind: "absent" });
    expect(() => patch("[t]\nk = 1\n", "t", "(", "int", "1")).not.toThrow();
    expect(() => patch("[(]\nk = 1\n", "(", "k", "int", "2")).not.toThrow();
  });
});

describe("the legacy editor builds its two patterns through escapeRe (HF-4)", () => {
  // The legacy script is a classic script in an IIFE, so its `regionBounds` and `patch`
  // cannot be called from here: what the suppressions rely on is that no table or key name
  // reaches `new RegExp` unescaped, which is read off the source.
  it("passes the table and the key through escapeRe at both `new RegExp` sites, and has no other", () => {
    const sites = legacyEditor.split("\n").filter((l) => l.includes("new RegExp("));
    expect(sites).toHaveLength(2);
    expect(sites[0]).toContain("escapeRe(table)");
    expect(sites[1]).toContain("escapeRe(key)");
  });
});
