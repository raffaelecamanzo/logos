/*
 * ConfigView component tests (S-191, FR-UI-12/13, FR-CF-06, NFR-SE-06/07) over
 * mocked config endpoints. They prove the migration's acceptance criteria at the
 * React boundary:
 *   - typed/raw-TOML round-trip (a typed edit patches the authoritative raw pane);
 *   - validate-then-write Save surfaces a 422 honestly with no fabricated success,
 *     and a mutating write carries the intent token (NFR-SE-06);
 *   - the chat key is write-only and never echoed onto the surface (NFR-SE-07);
 *   - the rules.toml confirm gate blocks the POST until confirmed (BR-35);
 *   - the explicit Apply is decoupled from Save and renders the honest outcome.
 */

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ToastProvider } from "../../components/index.ts";
import type { ConfigReadModel, VerifyReport } from "../../api/types.ts";
import { ConfigView } from "./ConfigView.tsx";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

/** A loaded read-model: an indexed config.toml with a `[chat]` section, a
 *  `[wiki]` section (S-224, FR-CF-07), and an absent rules.toml, plus a masked
 *  key (presence + last-4 only). */
function model(): ConfigReadModel {
  return {
    config: {
      path: ".logos/config.toml",
      exists: true,
      content:
        'languages = ["rust"]\nmax_file_size = 1048576\n\n[chat]\nprovider = "openai"\nbase_url = "https://openrouter.ai/api/v1"\n\n[wiki]\nmodel = "claude-wiki"\n',
      parsed: {
        languages: ["rust"],
        include: [],
        exclude: [],
        max_file_size: 1048576,
        framework_hints: [],
        chat: { provider: "openai", model: null, base_url: "https://openrouter.ai/api/v1" },
        wiki: { model: "claude-wiki" },
      },
    },
    rules: {
      path: ".logos/rules.toml",
      exists: false,
      content: "",
      parsed: { constraints: {}, metric_thresholds: {} },
    },
    chat_key: { present: true, last4: "9f3a" },
  };
}

interface MockResponse {
  ok: boolean;
  status: number;
  body: string;
}

type Route = (init: RequestInit | undefined) => MockResponse;

/** Route mocked `fetch` by `METHOD path`. The GET read-model defaults to the
 *  fixture; per-test routes override the mutating POSTs. Captures calls so tests
 *  can assert request bodies and the intent header. */
function mockFetch(routes: Record<string, Route>) {
  const calls: { url: string; method: string; body: string; intent: string | null }[] = [];
  const fn = vi.fn(async (url: string | URL, init?: RequestInit) => {
    const path = String(url);
    const method = (init?.method ?? "GET").toUpperCase();
    const body = typeof init?.body === "string" ? init.body : "";
    const intent = new Headers(init?.headers).get("x-logos-intent");
    calls.push({ url: path, method, body, intent });
    const route = routes[`${method} ${path}`];
    const r: MockResponse = route
      ? route(init)
      : method === "GET" && path === "/api/v1/config"
        ? { ok: true, status: 200, body: JSON.stringify(model()) }
        : { ok: false, status: 500, body: "no route" };
    return {
      ok: r.ok,
      status: r.status,
      json: async () => JSON.parse(r.body),
      text: async () => r.body,
    } as Response;
  });
  vi.stubGlobal("fetch", fn);
  return calls;
}

function renderView() {
  return render(
    <ToastProvider>
      <ConfigView />
    </ToastProvider>,
  );
}

/** The config.toml raw pane (the authoritative candidate). */
function configRaw(): HTMLTextAreaElement {
  return screen.getByLabelText(/Raw TOML — config\.toml/) as HTMLTextAreaElement;
}

describe("ConfigView typed/raw round-trip (FR-UI-12)", () => {
  it("patches a typed field into the authoritative raw-TOML candidate", async () => {
    mockFetch({});
    renderView();
    await screen.findByText(/CONFIG EDITOR/);

    // The provider select starts at the parsed value; switching it patches the
    // `provider` key inside the `[chat]` table of the raw pane in place.
    fireEvent.change(screen.getByLabelText("provider"), { target: { value: "anthropic" } });
    expect(configRaw().value).toContain('provider = "anthropic"');

    // A typed model edit inserts the key into the same table.
    fireEvent.change(screen.getByLabelText("model"), { target: { value: "claude-x" } });
    expect(configRaw().value).toContain('model = "claude-x"');

    // Clearing an optional scalar removes its key (revert to default).
    fireEvent.change(screen.getByLabelText("max_file_size"), { target: { value: "" } });
    expect(configRaw().value).not.toContain("max_file_size");
  });
});

describe("ConfigView [wiki] model field (S-224, FR-CF-07, FR-UI-12)", () => {
  it("renders beside the [chat] fields and initializes from the read-model's [wiki].model", async () => {
    mockFetch({});
    renderView();
    await screen.findByText(/CONFIG EDITOR/);

    expect(screen.getByText("[wiki]")).toBeInTheDocument();
    expect((screen.getByLabelText("wiki model") as HTMLInputElement).value).toBe("claude-wiki");
  });

  it("carries no key or provider control — only the model field (no new secret surface, NFR-SE-07)", async () => {
    mockFetch({});
    renderView();
    await screen.findByText(/CONFIG EDITOR/);

    const fieldset = screen.getByText("[wiki]").closest("fieldset")!;
    expect(within(fieldset).getAllByRole("textbox")).toHaveLength(1);
    expect(within(fieldset).queryByLabelText(/key/i)).not.toBeInTheDocument();
  });

  it("patches only [wiki].model in the raw candidate, leaving [chat].model untouched", async () => {
    mockFetch({});
    renderView();
    await screen.findByText(/CONFIG EDITOR/);

    fireEvent.change(screen.getByLabelText("wiki model"), { target: { value: "claude-wiki-2" } });
    expect(configRaw().value).toContain("[wiki]");
    expect(configRaw().value).toContain('model = "claude-wiki-2"');
    // The [chat] model field (a distinct table/key pair) is untouched.
    expect((screen.getByLabelText("model") as HTMLInputElement).value).toBe("");

    // Clearing it removes the key (revert to the [chat].model fallback).
    fireEvent.change(screen.getByLabelText("wiki model"), { target: { value: "" } });
    expect(configRaw().value).not.toContain('model = "claude-wiki-2"');
  });

  it("saves a valid [wiki].model edit through the same validated atomic write-back", async () => {
    const calls = mockFetch({
      "POST /config/save": () => ({
        ok: true,
        status: 200,
        body: JSON.stringify({ file: "config", path: ".logos/config.toml", bytes_written: 96, provenance_stamped: false }),
      }),
    });
    renderView();
    await screen.findByText(/CONFIG EDITOR/);

    fireEvent.change(screen.getByLabelText("wiki model"), { target: { value: "claude-wiki-2" } });
    fireEvent.click(screen.getByRole("button", { name: "Save config.toml" }));
    expect(await screen.findByText(/Saved \.logos\/config\.toml \(96 bytes\)/)).toBeInTheDocument();

    const save = calls.find((c) => c.url === "/config/save");
    expect(save?.body).toContain(encodeURIComponent('model = "claude-wiki-2"'));
  });

  it("rejects an invalid document inline with no partial write (BR-35), retaining the unsaved wiki edit", async () => {
    mockFetch({
      "POST /config/save": () => ({ ok: false, status: 422, body: "unknown key: wiki.bogus" }),
    });
    renderView();
    await screen.findByText(/CONFIG EDITOR/);

    fireEvent.change(screen.getByLabelText("wiki model"), { target: { value: "claude-wiki-2" } });
    fireEvent.click(screen.getByRole("button", { name: "Save config.toml" }));

    expect(await screen.findByText(/Validation error: unknown key: wiki\.bogus/)).toBeInTheDocument();
    expect(screen.queryByText(/Saved \.logos/)).not.toBeInTheDocument();
    // A rejected save neither writes the file nor discards the user's edit —
    // the candidate (and the typed field) still carry it, ready to retry.
    expect(configRaw().value).toContain('model = "claude-wiki-2"');
    expect((screen.getByLabelText("wiki model") as HTMLInputElement).value).toBe("claude-wiki-2");
  });

  it("initializes to blank when the read-model omits [wiki] entirely (older server)", async () => {
    const fixture = model();
    delete fixture.config.parsed.wiki;
    mockFetch({
      "GET /api/v1/config": () => ({ ok: true, status: 200, body: JSON.stringify(fixture) }),
    });
    renderView();
    await screen.findByText(/CONFIG EDITOR/);

    expect(screen.getByText("[wiki]")).toBeInTheDocument();
    expect((screen.getByLabelText("wiki model") as HTMLInputElement).value).toBe("");
  });
});

describe("ConfigView validate-then-write Save (FR-UI-12, NFR-SE-06)", () => {
  it("surfaces a 422 validation fault honestly and never reports success", async () => {
    mockFetch({
      "POST /config/save": () => ({ ok: false, status: 422, body: "unknown key: foo" }),
    });
    renderView();
    await screen.findByText(/CONFIG EDITOR/);

    fireEvent.click(screen.getByRole("button", { name: "Save config.toml" }));

    expect(await screen.findByText(/Validation error: unknown key: foo/)).toBeInTheDocument();
    // A rejected validation never claims a save happened (no fabricated success).
    expect(screen.queryByText(/Saved \.logos/)).not.toBeInTheDocument();
  });

  it("posts the whole raw document with the intent token, and reports the saved bytes", async () => {
    const calls = mockFetch({
      "POST /config/save": () => ({
        ok: true,
        status: 200,
        body: JSON.stringify({ file: "config", path: ".logos/config.toml", bytes_written: 64, provenance_stamped: false }),
      }),
    });
    renderView();
    await screen.findByText(/CONFIG EDITOR/);

    fireEvent.click(screen.getByRole("button", { name: "Save config.toml" }));
    expect(await screen.findByText(/Saved \.logos\/config\.toml \(64 bytes\)/)).toBeInTheDocument();

    const save = calls.find((c) => c.url === "/config/save");
    expect(save).toBeDefined();
    // The mutating request carries the per-session intent token (NFR-SE-06)…
    expect(save?.intent).toBe("test-intent-token");
    // …and posts the full raw candidate as `content` (no partial document).
    expect(save?.body).toContain("file=config");
    expect(save?.body).toContain(encodeURIComponent("max_file_size = 1048576"));
  });
});

describe("ConfigView chat key is write-only and never echoed (FR-CF-06, NFR-SE-07)", () => {
  it("shows only the masked key and never renders the typed secret", async () => {
    mockFetch({
      "POST /config/secret": () => ({
        ok: true,
        status: 200,
        body: JSON.stringify({ path: ".logos/secrets.toml", chat_key: { present: true, last4: "ef01" } }),
      }),
    });
    renderView();
    await screen.findByText(/CONFIG EDITOR/);

    // The read-model's masked key is shown as presence + last-4 only.
    expect(screen.getByText(/ends …9f3a/)).toBeInTheDocument();

    const secret = "sk-super-secret-7777";
    const input = screen.getByLabelText("api_key") as HTMLInputElement;
    fireEvent.change(input, { target: { value: secret } });
    fireEvent.click(screen.getByRole("button", { name: "Save key" }));

    // The masked outcome is surfaced; the raw secret is never rendered, and the
    // input is wiped so the typed key does not linger in the DOM (NFR-SE-07).
    expect(await screen.findByText(/Key saved \(ends …ef01\)\. It is stored in .* and never echoed\./)).toBeInTheDocument();
    expect(screen.queryByText(new RegExp(secret))).not.toBeInTheDocument();
    expect((screen.getByLabelText("api_key") as HTMLInputElement).value).toBe("");
  });

  it("reports a cleared key honestly when an empty value is saved", async () => {
    mockFetch({
      "POST /config/secret": () => ({
        ok: true,
        status: 200,
        body: JSON.stringify({ path: ".logos/secrets.toml", chat_key: { present: false, last4: null } }),
      }),
    });
    renderView();
    await screen.findByText(/CONFIG EDITOR/);

    fireEvent.click(screen.getByRole("button", { name: "Save key" }));
    expect(await screen.findByText(/Key cleared\. .* no longer holds a chat key\./)).toBeInTheDocument();
  });

  it("never echoes the secret route's response body on an error (NFR-SE-07)", async () => {
    // A server error whose body (hypothetically) contains key material must never
    // reach the surface — the client reports the status with a fixed, body-free
    // message, not the verbatim body the other routes safely show.
    const leak = "sk-leaked-key-0000";
    mockFetch({
      "POST /config/secret": () => ({ ok: false, status: 422, body: `bad key: ${leak}` }),
    });
    renderView();
    await screen.findByText(/CONFIG EDITOR/);

    fireEvent.change(screen.getByLabelText("api_key"), { target: { value: "whatever" } });
    fireEvent.click(screen.getByRole("button", { name: "Save key" }));

    expect(await screen.findByText(/Validation error: the server rejected the key write/)).toBeInTheDocument();
    expect(screen.queryByText(new RegExp(leak))).not.toBeInTheDocument();
  });

  it("handles a non-JSON 2xx from the secret route without surfacing the body", async () => {
    mockFetch({
      "POST /config/secret": () => ({ ok: true, status: 200, body: "OK" }),
    });
    renderView();
    await screen.findByText(/CONFIG EDITOR/);

    fireEvent.change(screen.getByLabelText("api_key"), { target: { value: "key" } });
    fireEvent.click(screen.getByRole("button", { name: "Save key" }));

    expect(await screen.findByText(/Key saved \(unexpected response format\)\./)).toBeInTheDocument();
    expect((screen.getByLabelText("api_key") as HTMLInputElement).value).toBe("");
  });
});

describe("ConfigView rules.toml confirm gate (BR-35)", () => {
  it("does not POST until the change is confirmed, then writes", async () => {
    const calls = mockFetch({
      "POST /config/save": () => ({
        ok: true,
        status: 200,
        body: JSON.stringify({ file: "rules", path: ".logos/rules.toml", bytes_written: 120, provenance_stamped: true }),
      }),
    });
    renderView();
    await screen.findByText(/CONFIG EDITOR/);

    // Saving rules.toml with no confirmation is blocked client-side — no POST.
    fireEvent.click(screen.getByRole("button", { name: "Save rules.toml" }));
    expect(await screen.findByText(/Confirm the change before saving rules\.toml/)).toBeInTheDocument();
    expect(calls.some((c) => c.url === "/config/save")).toBe(false);

    // Confirm, then save: the POST goes through and the provenance stamp is noted.
    fireEvent.click(screen.getByRole("checkbox"));
    fireEvent.click(screen.getByRole("button", { name: "Save rules.toml" }));
    expect(await screen.findByText(/A provenance comment was stamped into the file\./)).toBeInTheDocument();
    expect(calls.some((c) => c.url === "/config/save" && c.body.includes("file=rules"))).toBe(true);
  });
});

describe("ConfigView explicit Apply, decoupled from Save (FR-UI-13)", () => {
  it("renders the honest reconcile outcome and only Apply hits /config/apply", async () => {
    const calls = mockFetch({
      "POST /config/apply": () => ({
        ok: true,
        status: 200,
        body: JSON.stringify({
          action: "reconciled",
          reconciled_files: 2,
          full_index: false,
          unresolved_refs: 0,
          files_failed: [],
          warnings: [],
        }),
      }),
    });
    renderView();
    await screen.findByText(/CONFIG EDITOR/);

    fireEvent.click(screen.getByRole("button", { name: "Apply & reindex" }));
    expect(await screen.findByText(/Applied — reconciled 2 files\./)).toBeInTheDocument();

    // Apply posted only to /config/apply (never /config/save) — the two are decoupled.
    expect(calls.some((c) => c.url === "/config/apply")).toBe(true);
    expect(calls.some((c) => c.url === "/config/save")).toBe(false);
  });

  it("surfaces a rejected write (e.g. a guard 403) honestly", async () => {
    mockFetch({
      "POST /config/apply": () => ({ ok: false, status: 403, body: "missing or invalid intent token" }),
    });
    renderView();
    await screen.findByText(/CONFIG EDITOR/);

    fireEvent.click(screen.getByRole("button", { name: "Apply & reindex" }));
    expect(await screen.findByText(/Apply rejected \(403\): missing or invalid intent token/)).toBeInTheDocument();
  });

  it("downgrades a reconcile with failures/warnings to a warning, not a clean success", async () => {
    mockFetch({
      "POST /config/apply": () => ({
        ok: true,
        status: 200,
        body: JSON.stringify({
          action: "reconciled",
          reconciled_files: 5,
          full_index: true,
          unresolved_refs: 1,
          files_failed: ["src/broken.rs"],
          warnings: ["a degradation"],
        }),
      }),
    });
    renderView();
    await screen.findByText(/CONFIG EDITOR/);

    fireEvent.click(screen.getByRole("button", { name: "Apply & reindex" }));
    const panel = await screen.findByText(/Applied — reconciled 5 files\./);
    // The honest degradation channel surfaces the full-index note, the failed
    // file, and the warnings — never a clean success that hides them (NFR-CC-04).
    expect(panel.textContent).toContain("A full index was performed");
    expect(panel.textContent).toContain("Could not read/extract: src/broken.rs");
    expect(panel.textContent).toContain("Warnings: a degradation");
  });

  it("renders the honest rules.toml re-evaluation outcome", async () => {
    mockFetch({
      "POST /config/apply": () => ({
        ok: true,
        status: 200,
        body: JSON.stringify({
          action: "reevaluated",
          signal: 6905,
          violations: 2,
          freshness: "assumed fresh",
          warnings: [],
        }),
      }),
    });
    renderView();
    await screen.findByText(/CONFIG EDITOR/);

    // The rules.toml editor's Apply re-evaluates the gate (no reindex).
    fireEvent.click(screen.getByRole("button", { name: "Apply & re-evaluate" }));
    expect(
      await screen.findByText(/gate re-evaluated \(no reindex\)\. Signal 6905, 2 violations\. assumed fresh\./),
    ).toBeInTheDocument();
  });
});

describe("ConfigView graph consistency check (S-207, FR-UI-25, FR-GV-19)", () => {
  /** A clean-graph verify report; override for the drifted-graph fixture. */
  function verifyReport(overrides: Partial<VerifyReport> = {}): VerifyReport {
    return {
      ok: true,
      live: { files: 10, nodes: 100, edges: 200 },
      reindex: { files: 10, nodes: 100, edges: 200 },
      node_delta: 0,
      edge_delta: 0,
      file_delta: 0,
      leaked_total: 0,
      leaked_symbols: [],
      orphaned_total: 0,
      orphaned_symbols: [],
      structural: {
        ok: true,
        node_count: 100,
        distinct_symbol_ids: 100,
        duplicate_symbol_nodes: 0,
        dangling_file_refs: 0,
        dangling_edge_endpoints: 0,
        orphan_shingles: 0,
        faults: [],
        unadmitted_files: 0,
        unadmitted_sample: [],
        message: "sound",
      },
      message: "the live graph matches a fresh reindex",
      ...overrides,
    };
  }

  it("shows a loading state, then the CONSISTENT verdict on a healthy graph", async () => {
    let resolveVerify!: (res: Response) => void;
    const verifyPending = new Promise<Response>((resolve) => {
      resolveVerify = resolve;
    });
    const calls: { path: string; method: string; intent: string | null }[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (url: string | URL, init?: RequestInit) => {
        const path = String(url);
        const method = (init?.method ?? "GET").toUpperCase();
        calls.push({ path, method, intent: new Headers(init?.headers).get("x-logos-intent") });
        if (method === "GET" && path === "/api/v1/config") {
          return { ok: true, status: 200, json: async () => model(), text: async () => "" } as Response;
        }
        if (method === "POST" && path === "/api/v1/verify") return verifyPending;
        return { ok: false, status: 500, json: async () => ({}), text: async () => "no route" } as Response;
      }),
    );
    renderView();
    await screen.findByText(/CONFIG EDITOR/);

    fireEvent.click(screen.getByRole("button", { name: "Check graph consistency" }));

    // The seconds-to-minutes shadow reindex shows an explicit in-flight state
    // (FR-UI-07) — never a frozen control with no feedback.
    expect(await screen.findByText(/Re-indexing a shadow copy…/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Checking…" })).toBeDisabled();

    const report = verifyReport();
    resolveVerify({ ok: true, status: 200, json: async () => report, text: async () => "" } as Response);

    expect(await screen.findByText("CONSISTENT")).toBeInTheDocument();
    expect(screen.queryByText(/Re-indexing a shadow copy…/)).not.toBeInTheDocument();
    // The mutating verify request carries the per-session intent token (NFR-SE-06).
    const verifyCall = calls.find((c) => c.method === "POST" && c.path === "/api/v1/verify");
    expect(verifyCall?.intent).toBe("test-intent-token");
  });

  it("renders the DRIFT callout with the deltas and the leaked/orphaned-symbol sample", async () => {
    mockFetch({
      "POST /api/v1/verify": () => ({
        ok: true,
        status: 200,
        body: JSON.stringify(
          verifyReport({
            ok: false,
            live: { files: 11, nodes: 105, edges: 205 },
            reindex: { files: 10, nodes: 100, edges: 200 },
            node_delta: 5,
            edge_delta: 5,
            file_delta: 1,
            leaked_total: 2,
            leaked_symbols: ["pkg.gone.also_gone", "pkg.gone.gone"],
            orphaned_total: 1,
            orphaned_symbols: ["pkg.missing.fn"],
            structural: {
              ok: false,
              node_count: 105,
              distinct_symbol_ids: 103,
              duplicate_symbol_nodes: 2,
              dangling_file_refs: 0,
              dangling_edge_endpoints: 0,
              orphan_shingles: 1,
              faults: ["2 duplicate symbol nodes"],
              unadmitted_files: 0,
              unadmitted_sample: [],
              message: "faulty",
            },
            message: "the live graph has drifted from a fresh reindex",
          }),
        ),
      }),
    });
    renderView();
    await screen.findByText(/CONFIG EDITOR/);

    fireEvent.click(screen.getByRole("button", { name: "Check graph consistency" }));

    expect(await screen.findByText("DRIFT")).toBeInTheDocument();
    expect(screen.getByText(/the live graph has drifted from a fresh reindex/)).toBeInTheDocument();
    expect(screen.getByText(/Node delta/)).toHaveTextContent("Node delta 5 (live 105 vs reindex 100)");
    expect(screen.getByText(/Edge delta/)).toHaveTextContent("Edge delta 5 (live 205 vs reindex 200)");
    expect(screen.getByText(/File delta/)).toHaveTextContent("File delta 1 (live 11 vs reindex 10)");
    // The structural-check summary (duplicate-symbol rows, orphan rows) shown alongside.
    expect(screen.getByText(/2 duplicate-symbol rows/)).toBeInTheDocument();
    expect(screen.getByText(/1 orphan row/)).toBeInTheDocument();
    // The capped leaked/orphaned-symbol sample, mono, in a data table.
    expect(screen.getByText("pkg.gone.also_gone")).toBeInTheDocument();
    expect(screen.getByText("pkg.gone.gone")).toBeInTheDocument();
    expect(screen.getByText("pkg.missing.fn")).toBeInTheDocument();
    expect(screen.getAllByText("leaked")).toHaveLength(2);
    expect(screen.getByText("orphaned")).toBeInTheDocument();
    // Never a fabricated CONSISTENT verdict alongside the drift (NFR-RA-05).
    expect(screen.queryByText("CONSISTENT")).not.toBeInTheDocument();
  });

  it("shows the honest error panel on a read/verify fault, never a fabricated CONSISTENT", async () => {
    mockFetch({
      "POST /api/v1/verify": () => ({ ok: false, status: 500, body: "shadow reindex failed: disk full" }),
    });
    renderView();
    await screen.findByText(/CONFIG EDITOR/);

    fireEvent.click(screen.getByRole("button", { name: "Check graph consistency" }));

    expect(await screen.findByText(/Verify failed \(500\): shadow reindex failed: disk full/)).toBeInTheDocument();
    expect(screen.queryByText("CONSISTENT")).not.toBeInTheDocument();
    expect(screen.queryByText("DRIFT")).not.toBeInTheDocument();
  });

  it("renders the DRIFT callout with an honest empty-sample state on a structural-only fault", async () => {
    // The live/reindex censuses can match (zero leaked/orphaned symbols) while the
    // embedded structural checkpoint still fails — a distinct code path from the
    // populated-sample DRIFT case above: `symbolSampleRows` returns `[]`, so the
    // table must render its honest empty state rather than a blank table.
    mockFetch({
      "POST /api/v1/verify": () => ({
        ok: true,
        status: 200,
        body: JSON.stringify(
          verifyReport({
            ok: false,
            leaked_total: 0,
            leaked_symbols: [],
            orphaned_total: 0,
            orphaned_symbols: [],
            structural: {
              ok: false,
              node_count: 100,
              distinct_symbol_ids: 98,
              duplicate_symbol_nodes: 2,
              dangling_file_refs: 0,
              dangling_edge_endpoints: 0,
              orphan_shingles: 0,
              faults: ["2 duplicate symbol nodes"],
              unadmitted_files: 0,
              unadmitted_sample: [],
              message: "faulty",
            },
            message: "the structural checkpoint failed",
          }),
        ),
      }),
    });
    renderView();
    await screen.findByText(/CONFIG EDITOR/);

    fireEvent.click(screen.getByRole("button", { name: "Check graph consistency" }));

    expect(await screen.findByText("DRIFT")).toBeInTheDocument();
    expect(screen.getByText(/the structural checkpoint failed/)).toBeInTheDocument();
    expect(screen.getByText(/2 duplicate-symbol rows/)).toBeInTheDocument();
    expect(screen.getByText(/0 orphan rows/)).toBeInTheDocument();
    // No leaked/orphaned symbols in this sample — the honest empty state renders,
    // never a blank table (NFR-RA-05).
    expect(screen.getByText("No leaked or orphaned symbols in the sample.")).toBeInTheDocument();
    expect(screen.queryByRole("table")).not.toBeInTheDocument();
    expect(screen.queryByText("CONSISTENT")).not.toBeInTheDocument();
  });
});

describe("ConfigView load failure (NFR-RA-05)", () => {
  it("renders an honest error panel when the read-model fails to load", async () => {
    mockFetch({ "GET /api/v1/config": () => ({ ok: false, status: 500, body: "invalid policy file" }) });
    renderView();
    await waitFor(() => expect(screen.getByText(/HTTP 500/)).toBeInTheDocument());
    // No editor is rendered over a failed load (no fabricated form).
    expect(screen.queryByText(/CONFIG EDITOR/)).not.toBeInTheDocument();
  });
});
