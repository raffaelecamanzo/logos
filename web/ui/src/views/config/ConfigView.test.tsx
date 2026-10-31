/*
 * ConfigView component tests (S-191, FR-UI-12/13, FR-CF-06, NFR-SE-06/07) over
 * mocked config endpoints. They prove the migration's acceptance criteria at the
 * React boundary:
 *   - typed/raw-TOML round-trip (a typed edit patches the authoritative raw pane);
 *   - validate-then-write Save surfaces a 422 honestly with no fabricated success,
 *     and a mutating write carries the intent token (NFR-SE-06);
 *   - an inherited value or key in the read-model is never rendered or posted (NFR-SE-07);
 *   - the rules.toml confirm gate blocks the POST until confirmed (BR-35);
 *   - the explicit Apply is decoupled from Save and renders the honest outcome.
 */

import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ToastProvider } from "../../components/index.ts";
import type { ConfigReadModel, VerifyReport } from "../../api/types.ts";
import type { WorkspaceRoster } from "../../api/types.ts";
import { clearExtensions, registerExtensionUi, setActiveExtensions } from "../../extensions/registry.ts";
import { extensionsInfo, STUB_UI, stubInfo } from "../../test/extensions.tsx";
import { STUB_FIELDS, STUB_FIELDS_UI, STUB_NOTE } from "../../extensions/fixtures/stubConfigFields.ts";
import { ConfigView } from "./ConfigView.tsx";
import styles from "./ConfigView.module.css";
import { WorkspaceProvider } from "../../workspace/WorkspaceContext.tsx";
import { setScopedMember } from "../../workspace/scope.ts";
import { expectToolPanel } from "../../copy/expectWidgetCopy.ts";
import { expectOneWidgetStack, widgetTitle } from "../../test/widgetStack.ts";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  clearExtensions();
});

/** A loaded read-model, as the core serves it: an indexed config.toml that also
 *  carries `[chat]` and `[wiki]` tables (tables only a build composing extensions
 *  edits — here they are text in the raw pane), an absent rules.toml, and the
 *  core's masked key slice (presence + last-4 only). */
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
    // The single-root effective slice for this document (S-448): no model is
    // declared, so the policy half is unset; the member's own key is present.
    effective_chat: {
      policy: { provider: "openai", model: null, base_url: "https://openrouter.ai/api/v1" },
      policy_origin: "unset",
      credential: { present: true, last4: "9f3a" },
      credential_origin: "member",
      member_key_withheld: false,
    },
    // The single-root wiki resolution: the member's own `[wiki].model` (HF-1).
    effective_wiki: { model: "claude-wiki" },
    // The CR-067/BR-37 defaults projection: config.toml real defaults, plus
    // rules.toml real [metric_thresholds] defaults / [constraints] recommended
    // baselines — mirroring what the server's Config::default() /
    // Thresholds::default() / Constraints::recommended() actually produce.
    defaults: {
      config: {
        languages: [],
        include: ["**"],
        exclude: ["generated/**"],
        max_file_size: 2097152,
        framework_hints: [],
        chat: { provider: "openai", model: null, base_url: "https://openrouter.ai/api/v1" },
        wiki: {},
      },
      rules: {
        metric_thresholds: {
          nesting_depth: 4,
          brain_complexity: 15,
          brain_lines: 100,
          brain_nesting: 3,
          god_methods: 20,
          god_span: 500,
          clone_similarity: 0.85,
          clone_min_tokens: 50,
          duplicate_min_tokens: 50,
          cohesion_min_classes: 5,
        },
        constraints: {
          max_cycles: 0,
          max_cc: 15,
          max_fn_lines: 80,
          no_god_files: 40,
          max_fan_in: 30,
          max_fan_out: 30,
          max_dead: 0,
          max_duplicates: 0,
          max_nesting_depth: 4,
          max_brain_methods: 0,
          max_clone_ratio: 0.0,
          no_god_containers: true,
        },
      },
    },
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

/** The rendered hint text (help + default/recommended affordance) for the
 *  field labelled `label` — resolved via `aria-describedby` so fields sharing
 *  an identical hint string (e.g. two `[constraints]` recommended at 30) are
 *  disambiguated by their own control. */
function fieldHintText(label: string): string {
  const control = screen.getByLabelText(label);
  const describedBy = control.getAttribute("aria-describedby");
  expect(describedBy).toBeTruthy();
  return document.getElementById(describedBy!)!.textContent ?? "";
}

describe("ConfigView typed/raw round-trip (FR-UI-12)", () => {
  it("patches a typed field into the authoritative raw-TOML candidate", async () => {
    mockFetch({});
    renderView();
    await screen.findByText(/CONFIG EDITOR/);

    // A typed scalar edit patches its key in the raw pane in place…
    fireEvent.change(screen.getByLabelText("max_file_size"), { target: { value: "2048" } });
    expect(configRaw().value).toContain("max_file_size = 2048");

    // …a typed list edit inserts its key into the document…
    fireEvent.change(screen.getByLabelText("include"), { target: { value: "src/**" } });
    expect(configRaw().value).toContain('include = ["src/**"]');

    // Clearing an optional scalar removes its key (revert to default).
    fireEvent.change(screen.getByLabelText("max_file_size"), { target: { value: "" } });
    expect(configRaw().value).not.toContain("max_file_size");
  });
});

describe("ConfigView default / recommended-baseline affordance (CR-067, FR-UI-12, BR-37)", () => {
  it("renders the config.toml code default beside its current value", async () => {
    mockFetch({});
    renderView();
    await screen.findByText(/CONFIG EDITOR/);

    // Config::default().max_file_size == 2 MiB — a real code default, plainly stated.
    expect(fieldHintText("max_file_size")).toContain("Default: 2097152");
  });

  it("renders the [metric_thresholds] real code default beside its current value", async () => {
    mockFetch({});
    renderView();
    await screen.findByText(/CONFIG EDITOR/);

    // Thresholds::default() via MetricThresholds::effective(), keyed by the
    // rules.toml name (nesting_depth), not the engine field name (nest).
    expect(fieldHintText("nesting_depth")).toContain("Default: 4");
    expect(fieldHintText("brain_lines")).toContain("Default: 100");
  });

  it("states [constraints] have no code default and shows the recommended baseline", async () => {
    mockFetch({});
    renderView();
    await screen.findByText(/CONFIG EDITOR/);

    // Constraints have no code default (unset = not enforced); the curated
    // Constraints::recommended() baseline is shown alongside, never blurred
    // into a fabricated "default" (BR-37, e.g. max_fan_in -> 30).
    expect(fieldHintText("max_fan_in")).toContain("Default: unset → not enforced · Recommended: 30");
    expect(fieldHintText("max_fan_out")).toContain("Default: unset → not enforced · Recommended: 30");
    expect(fieldHintText("max_cc")).toContain("Recommended: 15");
    expect(fieldHintText("no_god_containers")).toContain("Recommended: true");
    // max_dead is the one constraint backed by the untagged MaxDead enum
    // (Absolute(u32) | Baseline{baseline,delta}) — its recommended baseline
    // (Absolute(0)) must still render as a plain number, not "[object Object]".
    expect(fieldHintText("max_dead")).toContain("Recommended: 0");
  });

  it("never fabricates a default when the field has none in the projection", async () => {
    // A projection carrying no value for a field (`null` on the wire) — the hint
    // states that honestly rather than inventing one.
    const m = model();
    m.defaults.rules.constraints.max_cc = null;
    mockFetch({ "GET /api/v1/config": () => ({ ok: true, status: 200, body: JSON.stringify(m) }) });
    renderView();
    await screen.findByText(/CONFIG EDITOR/);
    expect(fieldHintText("max_cc")).toContain("Recommended: unset");
    expect(fieldHintText("max_cc")).not.toContain("15");
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

describe("ConfigView round-trips only the literal document (S-448, NFR-RA-05)", () => {
  /** A member that declares no model and holds no key, inheriting both halves
   *  from its workspace — carried only in the `effective_chat` slice. */
  function inheritingModel(): ConfigReadModel {
    const m = model();
    return {
      ...m,
      chat_key: { present: false },
      effective_chat: {
        policy: { provider: "anthropic", model: "ws/inherited-model", base_url: "https://api.anthropic.com" },
        policy_origin: "workspace",
        credential: { present: true, last4: "wk42" },
        credential_origin: "workspace",
        member_key_withheld: false,
      },
    };
  }

  it("saves an inheriting member's own bytes and nothing it inherited", async () => {
    const calls = mockFetch({
      "GET /api/v1/config": () => ({ ok: true, status: 200, body: JSON.stringify(inheritingModel()) }),
      "POST /config/save": () => ({
        ok: true,
        status: 200,
        body: JSON.stringify({ file: "config", path: ".logos/config.toml", bytes_written: 120, provenance_stamped: false }),
      }),
    });
    renderView();
    await screen.findByText(/CONFIG EDITOR/);

    // The raw pane holds the literal document, not the slice, and nothing on the
    // page renders an inherited value or key.
    expect(configRaw().value).toBe(model().config.content);
    expect(screen.queryByText(/wk42/)).not.toBeInTheDocument();
    expect(screen.queryByText(/inherited-model/)).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Save config.toml" }));
    expect(await screen.findByText(/Saved \.logos\/config\.toml \(120 bytes\)/)).toBeInTheDocument();

    const save = calls.find((c) => c.url === "/config/save");
    expect(new URLSearchParams(save?.body).get("content")).toBe(model().config.content);
    expect(save?.body).not.toContain("inherited-model");
    expect(calls.some((c) => c.url === "/config/secret")).toBe(false);
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

  it("shows a reconcile's advisory notes as plain text without downgrading to a warning (HF-1)", async () => {
    mockFetch({
      "POST /config/apply": () => ({
        ok: true,
        status: 200,
        body: JSON.stringify({
          action: "reconciled",
          reconciled_files: 3,
          full_index: false,
          unresolved_refs: 0,
          files_failed: [],
          warnings: [],
          notes: ["2 minified JavaScript file(s) excluded from indexing by the `**/*.min.js` exclude glob"],
        }),
      }),
    });
    renderView();
    await screen.findByText(/CONFIG EDITOR/);

    fireEvent.click(screen.getByRole("button", { name: "Apply & reindex" }));
    const panel = await screen.findByText(/Applied — reconciled 3 files\./);
    expect(panel.textContent).toContain("Note: 2 minified JavaScript file(s) excluded from indexing");
    // A benign default is not a degradation: the banner stays a clean success.
    expect(panel.textContent).not.toContain("Warnings:");
    // Class PLACEMENT, via the module's own (proxied) names — see the css:false note.
    expect(panel.classList.contains(styles.warn)).toBe(false);
    expect(panel.classList.contains(styles.ok)).toBe(true);
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

describe("ConfigView load failure names the repair surface (HF-2, Sprint 77 review option 4i)", () => {
  const ROSTER: WorkspaceRoster = { workspace: "shop", default: "api", members: ["api", "web"] };

  afterEach(() => {
    setScopedMember(null);
    window.history.replaceState({}, "", "/");
  });

  it("single-root: no workspace note, text unchanged", async () => {
    mockFetch({ "GET /api/v1/config": () => ({ ok: false, status: 500, body: "invalid policy file" }) });
    renderView();
    expect(await screen.findByText(/HTTP 500/)).toBeInTheDocument();
    expect(screen.getByRole("alert")).not.toHaveTextContent(/workspace root/);
  });

  it("workspace mode: names both a member-file and a workspace-file fault, and links no editor", async () => {
    // The read fails identically whether the fault is this member's own file or the
    // inherited workspace-tier file (S-450) — the 500 body carries no field
    // distinguishing the two, so the note names both.
    //
    // `ConfigView`'s own resource fetches on mount with no dependency on the
    // workspace probe settling first (unlike the real shell, which holds every view
    // back until it does — `WorkspaceContext.tsx`), so the scope is set directly
    // here rather than raced against the provider's async roster resolution.
    setScopedMember("web");
    window.history.replaceState({}, "", "/config?repo=web");
    mockFetch({
      "GET /api/v1/workspace/roster": () => ({ ok: true, status: 200, body: JSON.stringify(ROSTER) }),
      "GET /api/v1/config?repo=web": () => ({ ok: false, status: 500, body: "invalid policy file" }),
    });
    render(
      <ToastProvider>
        <WorkspaceProvider>
          <ConfigView />
        </WorkspaceProvider>
      </ToastProvider>,
    );
    expect(await screen.findByText(/HTTP 500/)).toBeInTheDocument();
    const panel = await screen.findByRole("alert");
    await waitFor(() => expect(panel).toHaveTextContent(/the workspace root/));
    expect(panel).toHaveTextContent(/this member.s own root/);
    expect(panel).toHaveTextContent("<workspace-root>/.logos/config.toml");
    expect(panel).toHaveTextContent("<workspace-root>/.logos/secrets.toml");
    // No editor in this build repairs a workspace-root file, so the note promises
    // none and links nowhere.
    expect(panel).toHaveTextContent("Repair the file at fault by hand.");
    expect(within(panel).queryByRole("link")).toBeNull();
  });
});

// ── S-617 (CR-203, FR-UI-39/40): the editors are registered tool panels ──────

describe("the Config editors render as registered tool panels in one stack (S-617)", () => {
  it("config.toml, the consistency check and rules.toml, each under its TOOL_PANELS key", async () => {
    mockFetch({});
    const { container } = renderView();
    await screen.findByText(/CONFIG EDITOR/);
    const panels = expectOneWidgetStack(container);
    expect(panels.map(widgetTitle)).toEqual(["config.toml", "Graph consistency check", "rules.toml"]);
    expectToolPanel(panels[0], "policyFile");
    expectToolPanel(panels[1], "graphConsistency");
    expectToolPanel(panels[2], "policyFile");
  });
});

describe("the config panels of the mounted extensions (S-642, FR-UI-51)", () => {
  it("renders an active extension's member panel after the built-in ones, in the same stack", async () => {
    mockFetch({});
    registerExtensionUi(STUB_UI);
    setActiveExtensions(extensionsInfo(stubInfo()));
    const { container } = renderView();
    await screen.findByText(/CONFIG EDITOR/);

    const panel = await screen.findByTestId("stub-member-panel");
    expect(panel).toHaveTextContent("Stub config [stub] (member)");
    // The workspace's variant of the same panel is not this view's.
    expect(screen.queryByTestId("stub-workspace-panel")).toBeNull();
    // In the one widget stack, directly beneath the config document whose tables
    // the extensions' panels edit (S-643) — before the consistency check and rules.
    const stack = container.querySelector("[data-widget-stack]") as HTMLElement;
    expect(stack.contains(panel)).toBe(true);
    const titles = expectOneWidgetStack(container).map(widgetTitle);
    expect(titles[0]).toBe("config.toml");
    expect(titles.slice(-2)).toEqual(["Graph consistency check", "rules.toml"]);
    expect(panel.compareDocumentPosition(screen.getByRole("heading", { name: "Graph consistency check" })))
      .toBe(Node.DOCUMENT_POSITION_FOLLOWING);
  });

  it("renders it when the boot read settles after the view mounted", async () => {
    mockFetch({});
    registerExtensionUi(STUB_UI);
    renderView();
    await screen.findByText(/CONFIG EDITOR/);
    expect(screen.queryByTestId("stub-member-panel")).toBeNull();

    act(() => setActiveExtensions(extensionsInfo(stubInfo())));
    expect(await screen.findByTestId("stub-member-panel")).toBeInTheDocument();
  });

  it("renders no panel for an extension the server does not report active", async () => {
    mockFetch({});
    registerExtensionUi(STUB_UI);
    setActiveExtensions(extensionsInfo());
    renderView();
    await screen.findByText(/CONFIG EDITOR/);
    expect(screen.queryByTestId("stub-member-panel")).toBeNull();
  });

});

describe("this build composes no extension: no extension table in the document form (FR-DS-05)", () => {
  it("offers neither [chat] nor [wiki] fields, nor a key card — the built-in editors alone", async () => {
    mockFetch({});
    const { container } = renderView();
    await screen.findByText(/CONFIG EDITOR/);
    expect(expectOneWidgetStack(container).map(widgetTitle)).toEqual([
      "config.toml",
      "Graph consistency check",
      "rules.toml",
    ]);
    expect(container.querySelectorAll("legend")).not.toHaveLength(0);
    for (const legend of container.querySelectorAll("legend")) {
      expect(legend.textContent).not.toMatch(/chat|wiki/i);
    }
    expect(screen.queryByLabelText(/api_key|provider|base_url|model/)).toBeNull();
  });

  it("offers no [chat] or [wiki] fields beside an extension's member panel either", async () => {
    mockFetch({});
    registerExtensionUi(STUB_UI);
    setActiveExtensions(extensionsInfo(stubInfo()));
    const { container } = renderView();
    await screen.findByTestId("stub-member-panel");
    for (const legend of container.querySelectorAll("legend")) {
      expect(legend.textContent).not.toMatch(/chat|wiki/i);
    }
  });

  it("keeps an existing [chat] or [wiki] table in the raw candidate, so a save never drops it", async () => {
    const calls = mockFetch({
      "POST /config/save": () => ({
        ok: true,
        status: 200,
        body: JSON.stringify({ file: "config", path: ".logos/config.toml", bytes_written: 120, provenance_stamped: false }),
      }),
    });
    renderView();
    await screen.findByText(/CONFIG EDITOR/);
    expect(configRaw().value).toContain("[chat]");
    expect(configRaw().value).toContain("[wiki]");
    fireEvent.change(screen.getByLabelText("max_file_size"), { target: { value: "2048" } });
    fireEvent.click(screen.getByRole("button", { name: "Save config.toml" }));
    expect(await screen.findByText(/Saved \.logos\/config\.toml/)).toBeInTheDocument();
    const content = new URLSearchParams(calls.find((c) => c.url === "/config/save")?.body).get("content") ?? "";
    expect(content).toContain('[chat]\nprovider = "openai"');
    expect(content).toContain('[wiki]\nmodel = "claude-wiki"');
  });
});

describe("the mounted extensions' config field groups (S-730, FR-UI-54)", () => {
  /** A config.toml declaring the stub's table, one key per field kind. */
  const STUB_DOC = [
    'languages = ["rust"]',
    "max_file_size = 1048576",
    "",
    "[stub]",
    "count = 3",
    "ratio = 0.5",
    "enabled = true",
    'names = ["alpha", "beta"]',
    'title = "Hello"',
    'mode = "slow"',
    "",
  ].join("\n");

  /** The read-model over `content`, as the core serves it. */
  function withContent(content: string): ConfigReadModel {
    const m = model();
    return { ...m, config: { ...m.config, content } };
  }

  function serve(content: string, routes: Record<string, Route> = {}) {
    return mockFetch({
      "GET /api/v1/config": () => ({ ok: true, status: 200, body: JSON.stringify(withContent(content)) }),
      "POST /config/save": () => ({
        ok: true,
        status: 200,
        body: JSON.stringify({ file: "config", path: ".logos/config.toml", bytes_written: 120, provenance_stamped: false }),
      }),
      ...routes,
    });
  }

  function activateStub(ui = STUB_FIELDS_UI) {
    registerExtensionUi(ui);
    setActiveExtensions(extensionsInfo(stubInfo()));
  }

  async function savedContent(calls: ReturnType<typeof mockFetch>): Promise<string> {
    fireEvent.click(screen.getByRole("button", { name: "Save config.toml" }));
    await screen.findByText(/Saved \.logos\/config\.toml/);
    const saves = calls.filter((c) => c.method === "POST");
    // The existing Save, and nothing else: no second writer, no other route.
    expect(saves.map((c) => c.url)).toEqual(["/config/save"]);
    return new URLSearchParams(saves[0].body).get("content") ?? "";
  }

  it("renders an active stub's member group after the core groups, in the config.toml form", async () => {
    serve(STUB_DOC);
    activateStub();
    const { container } = renderView();
    await screen.findByLabelText("Stub count");

    const groups = container.querySelectorAll("[data-extension-fields]");
    expect(groups).toHaveLength(1); // the config.toml form only — never rules.toml
    const [ext] = groups;
    expect(ext.querySelector("legend")).toHaveTextContent("[stub]");
    // After the core group, before the raw pane, inside the config.toml panel.
    const core = screen.getByLabelText("max_file_size").closest("fieldset") as HTMLElement;
    expect(core.compareDocumentPosition(ext)).toBe(Node.DOCUMENT_POSITION_FOLLOWING);
    expect(ext.compareDocumentPosition(configRaw())).toBe(Node.DOCUMENT_POSITION_FOLLOWING);
    expect(expectOneWidgetStack(container).map(widgetTitle)).toEqual(["config.toml", "Graph consistency check", "rules.toml"]);

    // Every value is read from the candidate text.
    expect(screen.getByLabelText("Stub count")).toHaveValue(3);
    expect(screen.getByLabelText("Stub ratio")).toHaveValue(0.5);
    expect(screen.getByLabelText("Stub enabled")).toHaveValue("true");
    expect(screen.getByLabelText("Stub names")).toHaveValue("alpha\nbeta");
    expect(screen.getByLabelText("Stub title")).toHaveValue("Hello");
    expect(screen.getByLabelText("Stub mode")).toHaveValue("slow");
  });

  it("renders no extension field while the stub is registered but not active, and renders it once the boot read settles", async () => {
    serve(STUB_DOC);
    registerExtensionUi(STUB_FIELDS_UI);
    const { container } = renderView();
    await screen.findByText(/CONFIG EDITOR/);
    expect(container.querySelector("[data-extension-fields]")).toBeNull();
    act(() => setActiveExtensions(extensionsInfo()));
    expect(container.querySelector("[data-extension-fields]")).toBeNull();
    act(() => setActiveExtensions(extensionsInfo(stubInfo())));
    expect(await screen.findByLabelText("Stub count")).toBeInTheDocument();
  });

  it("renders each kind's control with its label, placeholder, help and default hint, and the note read-only", async () => {
    serve(STUB_DOC);
    activateStub();
    renderView();
    await screen.findByLabelText("Stub count");

    for (const f of STUB_FIELDS) {
      const hint = fieldHintText(f.label);
      expect(hint).toContain(f.help);
      expect(hint).toContain(f.defaultHint);
      if (f.placeholder) expect(screen.getByLabelText(f.label)).toHaveAttribute("placeholder", f.placeholder);
    }
    expect(screen.getByLabelText("Stub count")).toHaveAttribute("type", "number");
    expect(screen.getByLabelText("Stub ratio")).toHaveAttribute("step", "any");
    expect(screen.getByLabelText("Stub names").tagName).toBe("TEXTAREA");
    expect(screen.getByLabelText("Stub title")).toHaveAttribute("type", "text");
    const options = (label: string) =>
      within(screen.getByLabelText(label)).getAllByRole("option").map((o) => o.textContent);
    expect(options("Stub enabled")).toEqual(["(unset)", "true", "false"]);
    expect(options("Stub mode")).toEqual(["(unset)", "fast", "slow"]);

    const note = screen.getByText(STUB_NOTE);
    expect(note.tagName).toBe("P");
    expect(note.closest("label, input, textarea, select")).toBeNull();
  });

  it("round-trips typed → raw → typed for every kind, through the existing Save", async () => {
    const calls = serve(STUB_DOC);
    activateStub();
    renderView();
    await screen.findByLabelText("Stub count");

    const edits: [string, string, string][] = [
      ["Stub count", "7", "count = 7"],
      ["Stub ratio", "0.75", "ratio = 0.75"],
      ["Stub enabled", "false", "enabled = false"],
      ["Stub names", "gamma\ndelta", 'names = ["gamma", "delta"]'],
      ["Stub title", 'Say "hi"', 'title = "Say \\"hi\\""'],
      ["Stub mode", "fast", 'mode = "fast"'],
    ];
    for (const [label, value, line] of edits) {
      fireEvent.change(screen.getByLabelText(label), { target: { value } });
      expect(configRaw().value).toContain(`[stub]`);
      expect(configRaw().value.split("\n")).toContain(line);
    }
    const content = await savedContent(calls);
    expect(content).toBe(configRaw().value);

    // Reload over what was saved: every field reads back the value it wrote.
    cleanup();
    serve(content);
    renderView();
    await screen.findByLabelText("Stub count");
    expect(screen.getByLabelText("Stub count")).toHaveValue(7);
    expect(screen.getByLabelText("Stub ratio")).toHaveValue(0.75);
    expect(screen.getByLabelText("Stub enabled")).toHaveValue("false");
    expect(screen.getByLabelText("Stub names")).toHaveValue("gamma\ndelta");
    expect(screen.getByLabelText("Stub title")).toHaveValue('Say "hi"');
    expect(screen.getByLabelText("Stub mode")).toHaveValue("fast");
  });

  it("removes a key whose field is emptied, keeping the rest of the table", async () => {
    serve(STUB_DOC);
    activateStub();
    renderView();
    await screen.findByLabelText("Stub count");
    for (const f of STUB_FIELDS) fireEvent.change(screen.getByLabelText(f.label), { target: { value: "" } });
    const lines = configRaw().value.split("\n");
    expect(lines).toContain("[stub]");
    for (const f of STUB_FIELDS) expect(lines.some((l) => l.startsWith(`${f.key} =`))).toBe(false);
    expect(lines).toContain("max_file_size = 1048576");
  });

  it("shows a choice value outside its options as written, and never rewrites it", async () => {
    const calls = serve(STUB_DOC.replace('mode = "slow"', 'mode = "turbo"'));
    activateStub();
    renderView();
    await screen.findByLabelText("Stub mode");
    expect(screen.getByLabelText("Stub mode")).toHaveValue("turbo");
    expect(within(screen.getByLabelText("Stub mode")).getByRole("option", { name: "turbo (not one of the options)" })).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("Stub count"), { target: { value: "4" } });
    const content = await savedContent(calls);
    expect(content.split("\n")).toContain('mode = "turbo"');
    expect(content.split("\n")).toContain("count = 4");
  });

  it("leaves a value the reader cannot type untouched, shown read-only as written", async () => {
    const odd = STUB_DOC.replace("count = 3", "count = 0x10").replace('names = ["alpha", "beta"]', "names = []");
    const calls = serve(odd);
    activateStub();
    renderView();
    await screen.findByLabelText("Stub count");
    for (const [label, text] of [["Stub count", "0x10"], ["Stub names", "[]"]]) {
      const control = screen.getByLabelText(label);
      expect(control).toHaveValue(text);
      expect(control).toHaveAttribute("readonly");
      expect(fieldHintText(label)).toMatch(/left as written/);
    }
    fireEvent.change(screen.getByLabelText("Stub title"), { target: { value: "Edited" } });
    const content = await savedContent(calls);
    expect(content).toBe(odd.replace('title = "Hello"', 'title = "Edited"'));
  });

  it("shows a multi-line untyped value with its line breaks, read-only", async () => {
    serve(STUB_DOC.replace('title = "Hello"', 'title = """\nfirst\nsecond\n"""'));
    activateStub();
    renderView();
    const title = await screen.findByLabelText("Stub title");
    expect(title.tagName).toBe("TEXTAREA");
    expect(title).toHaveValue('"""\nfirst\nsecond\n"""');
    expect(title).toHaveAttribute("readonly");
  });

  it("shows a raw-pane edit of an untouched field's key in that field", async () => {
    serve(STUB_DOC);
    activateStub();
    renderView();
    await screen.findByLabelText("Stub title");
    fireEvent.change(configRaw(), { target: { value: STUB_DOC.replace('title = "Hello"', 'title = "From the raw pane"') } });
    expect(screen.getByLabelText("Stub title")).toHaveValue("From the raw pane");
  });

  it("agrees with a raw-pane edit of a key its field already edited, and keeps a partial number while typing", async () => {
    serve(STUB_DOC);
    activateStub();
    renderView();
    await screen.findByLabelText("Stub count");
    fireEvent.change(screen.getByLabelText("Stub count"), { target: { value: "7" } });
    expect(configRaw().value.split("\n")).toContain("count = 7");
    fireEvent.change(configRaw(), { target: { value: configRaw().value.replace("count = 7", "count = 9") } });
    expect(screen.getByLabelText("Stub count")).toHaveValue(9);
    // A list mid-edit keeps its blank line: the candidate still holds what it wrote.
    fireEvent.change(screen.getByLabelText("Stub names"), { target: { value: "alpha\n" } });
    expect(screen.getByLabelText("Stub names")).toHaveValue("alpha\n");
  });

  it("states a select kind's placeholder as its empty choice", async () => {
    serve(STUB_DOC);
    activateStub({
      ...STUB_UI,
      configFields: {
        "stub-config": {
          member: [{ fields: [{ ...STUB_FIELDS[5], placeholder: "(use the stub's own mode)" }, STUB_FIELDS[2]] }],
        },
      },
    });
    renderView();
    const mode = await screen.findByLabelText("Stub mode");
    expect(within(mode).getAllByRole("option")[0]).toHaveTextContent("(use the stub's own mode)");
    expect(within(screen.getByLabelText("Stub enabled")).getAllByRole("option")[0]).toHaveTextContent("(unset)");
  });

  it("computes a group's note from the view's host context, and renders none for null", async () => {
    serve(STUB_DOC);
    activateStub({
      ...STUB_UI,
      configFields: {
        "stub-config": {
          member: [
            { legend: "[stub]", note: (host) => (host?.model.config.exists ? `Read from ${host.model.config.path}` : null), fields: STUB_FIELDS.slice(0, 1) },
            { legend: "[stub.more]", note: () => null, fields: [{ ...STUB_FIELDS[4], table: "stub.more" }] },
          ],
        },
      },
    });
    const { container } = renderView();
    await screen.findByLabelText("Stub count");
    expect(screen.getByText("Read from .logos/config.toml").tagName).toBe("P");
    const groups = container.querySelectorAll("[data-extension-fields]");
    expect(groups).toHaveLength(2);
    // The note is the fieldset's own paragraph; a field's hint paragraph is nested in it.
    expect(groups[0].querySelector(":scope > p")).toHaveTextContent("Read from .logos/config.toml");
    expect(groups[1].querySelector(":scope > p")).toBeNull();
  });

  it("keeps what was typed in an edited field where the encoder would normalise it", async () => {
    serve(STUB_DOC);
    activateStub();
    renderView();
    await screen.findByLabelText("Stub title");
    fireEvent.change(screen.getByLabelText("Stub title"), { target: { value: "Hello " } });
    expect(screen.getByLabelText("Stub title")).toHaveValue("Hello ");
    expect(configRaw().value.split("\n")).toContain('title = "Hello"');
  });

  it("keeps the edit state of a key apart from the same key in a sub-table", async () => {
    serve(STUB_DOC + "\n[stub.sub]\ncount = 5\n");
    activateStub({
      ...STUB_UI,
      configFields: {
        "stub-config": {
          member: [
            { legend: "[stub]", fields: [STUB_FIELDS[0]] },
            { legend: "[stub.sub]", fields: [{ ...STUB_FIELDS[0], table: "stub.sub", label: "Sub count" }] },
          ],
        },
      },
    });
    renderView();
    await screen.findByLabelText("Sub count");
    fireEvent.change(screen.getByLabelText("Stub count"), { target: { value: "8" } });
    expect(screen.getByLabelText("Stub count")).toHaveValue(8);
    expect(screen.getByLabelText("Sub count")).toHaveValue(5);
  });
});
