/*
 * WorkspaceConfigView (S-430, FR-UI-38) — the app-level Config editor over
 * `logos.workspace.toml`. Pins what makes a Save button over a file that governs N
 * repositories safe to render: the candidate is the raw pane verbatim, posted with
 * the load fingerprint; a conflict is shown and resolved only by an explicit
 * choice; a parse refusal is inline; governance is stated advisory where it is
 * edited; and the view is app-scoped — a member switch re-issues nothing.
 */

import { act, cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import type {
  ConfigReadModel,
  ManifestSaveOutcome,
  MaskedSecret,
  WorkspaceGovernanceAnswer,
  WorkspaceManifestDocument,
  WorkspaceRoster,
} from "../../api/types.ts";
import { WorkspaceProvider, useWorkspace } from "../../workspace/WorkspaceContext.tsx";
import { setScopedMember } from "../../workspace/scope.ts";
import { WorkspaceConfigView } from "./WorkspaceConfigView.tsx";

const ROSTER: WorkspaceRoster = { workspace: "shop", default: "api", members: ["api", "web"] };

/** The shape `logos init --workspace` writes — a multi-line `members` — plus a
 *  hand-added governance family: the dominant real manifest, not a tidy one. */
const CONTENT = [
  "[workspace]",
  'name = "shop"',
  "members = [",
  '    "api",',
  '    "web",',
  "]",
  'default = "api"',
  "",
  "[[governance.service_layers]]",
  'name = "edge"',
  'members = ["api"]',
  "",
  "[[governance.boundaries]]",
  'from = "edge"',
  'to = "core"',
  "",
].join("\n");

function doc(over: Partial<WorkspaceManifestDocument> = {}): WorkspaceManifestDocument {
  return {
    path: "logos.workspace.toml",
    content: CONTENT,
    fingerprint: "fp-loaded",
    parsed: {
      workspace: { name: "shop", members: ["api", "web"], default: "api" },
      governance: {
        service_layers: [{ name: "edge", members: ["api"] }],
        boundaries: [{ from: "edge", to: "core", reason: null }],
      },
    },
    error: null,
    governance_in_effect: true,
    ...over,
  };
}

const CLEAN_CHECK: WorkspaceGovernanceAnswer = {
  complete: true,
  degraded_rollup: { members: 2, opened: 2, not_attempted: 0, degraded_members: [], covers_all_members: true },
  governance: {
    workspace: "shop",
    rules_checked: 1,
    bindings_checked: 3,
    violations: [
      {
        rule: "workspace-boundary:edge->core",
        rule_type: "workspace-boundary",
        severity: "error",
        relation: "route",
        from: { member: "api", symbol: "api::fetchUser" },
        to: { member: "web", symbol: "web::get_user" },
        message: "edge must not call core",
      },
    ],
  },
} as WorkspaceGovernanceAnswer;

interface Posted {
  url: string;
  form: URLSearchParams;
  intent: string | null;
}

/** Stub `fetch` over the surface this view drives. `manifests` is served in turn
 *  (the last one repeats); `saves` answers each POST in turn with a status + body. */
function stubApi({
  manifests = [doc()],
  check = CLEAN_CHECK,
  saves = [],
  probeStatus = 200,
}: {
  /** `null` answers that read with a `500`. */
  manifests?: (WorkspaceManifestDocument | null)[];
  check?: WorkspaceGovernanceAnswer;
  saves?: { status: number; body: unknown }[];
  probeStatus?: number;
} = {}) {
  const gets: string[] = [];
  const posts: Posted[] = [];
  let m = 0;
  let s = 0;
  const respond = (body: unknown, status = 200) =>
    Promise.resolve({
      ok: status >= 200 && status < 300,
      status,
      json: () => Promise.resolve(body),
      text: () => Promise.resolve(JSON.stringify(body)),
    } as Response);
  vi.stubGlobal(
    "fetch",
    vi.fn((url: string, init?: RequestInit) => {
      if (init?.method === "POST") {
        posts.push({
          url,
          form: new URLSearchParams(String(init.body)),
          intent: new Headers(init.headers).get("x-logos-intent"),
        });
        const reply = saves[Math.min(s++, saves.length - 1)];
        return respond(reply.body, reply.status);
      }
      gets.push(url);
      if (url.startsWith("/api/v1/workspace/roster")) return respond(ROSTER, probeStatus);
      if (url.startsWith("/api/v1/workspace/manifest")) {
        const next = manifests[Math.min(m++, manifests.length - 1)];
        return next === null ? respond({ error: "boom" }, 500) : respond(next);
      }
      if (url.startsWith("/api/v1/workspace/check")) return respond(check);
      return respond({});
    }),
  );
  return { gets, posts };
}

const switcher: { current: ((name: string) => void) | null } = { current: null };
function CaptureSwitch() {
  switcher.current = useWorkspace().selectMember;
  return null;
}

async function mount(opts: Parameters<typeof stubApi>[0] = {}) {
  const api = stubApi(opts);
  render(
    <WorkspaceProvider>
      <CaptureSwitch />
      <WorkspaceConfigView />
    </WorkspaceProvider>,
  );
  if ((opts.probeStatus ?? 200) === 200) await screen.findByLabelText(/^Raw TOML/);
  return api;
}

function rawPane(): HTMLTextAreaElement {
  return screen.getByLabelText(/^Raw TOML/) as HTMLTextAreaElement;
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  setScopedMember(null);
  switcher.current = null;
});

describe("the hybrid editor over the manifest (FR-UI-38 AC1)", () => {
  it("loads the literal document into the raw pane and pre-fills typed fields from the parse only", async () => {
    await mount();
    expect(rawPane().value).toBe(CONTENT);
    expect(screen.getByLabelText("name")).toHaveValue("shop");
    expect(screen.getByLabelText("members")).toHaveValue("api\nweb");
    expect(screen.getByLabelText("default")).toHaveValue("api");
    // Undeclared keys are blank, never a default the manifest did not state.
    expect(screen.getByLabelText("concurrency")).toHaveValue(null);
    expect(screen.getByLabelText("enabled")).toHaveValue("");
    expect(screen.getByText("logos.workspace.toml", { selector: "span" })).toBeInTheDocument();
  });

  it("posts the raw pane VERBATIM with the load fingerprint and the intent token", async () => {
    const written: ManifestSaveOutcome = {
      outcome: "written",
      path: "logos.workspace.toml",
      bytes_written: 42,
      fingerprint: "fp-saved",
    };
    const { posts } = await mount({ saves: [{ status: 200, body: written }] });
    const user = userEvent.setup();

    // A typed edit on the init-written multi-line `members` patches the whole array.
    const members = screen.getByLabelText("members");
    await user.clear(members);
    await user.type(members, "api\nweb\nworker");
    expect(rawPane().value).toContain('members = ["api", "web", "worker"]');
    expect(rawPane().value).not.toContain('    "web",');

    await user.click(screen.getByRole("button", { name: /Save logos\.workspace\.toml/ }));
    const status = await screen.findByText(/Saved logos\.workspace\.toml \(42 bytes\)/);
    expect(status).toHaveTextContent(/after `logos serve` is restarted/);
    expect(status).toHaveTextContent(/No member was reindexed/);

    expect(posts).toHaveLength(1);
    expect(posts[0].url).toBe("/api/v1/workspace/manifest/save");
    expect(posts[0].form.get("content")).toBe(rawPane().value);
    expect(posts[0].form.get("fingerprint")).toBe("fp-loaded");
    expect(posts[0].intent).toBe("test-intent-token");
  });

  it("holds the fingerprint the save returned, so the next save is made against it", async () => {
    const { posts } = await mount({
      saves: [
        { status: 200, body: { outcome: "written", path: "logos.workspace.toml", bytes_written: 1, fingerprint: "fp-2" } },
        { status: 200, body: { outcome: "unchanged", path: "logos.workspace.toml", fingerprint: "fp-2" } },
      ],
      manifests: [doc(), doc({ fingerprint: "fp-2" })],
    });
    const user = userEvent.setup();
    const save = screen.getByRole("button", { name: /Save logos\.workspace\.toml/ });
    await user.click(save);
    await screen.findByText(/Saved logos\.workspace\.toml/);
    await user.click(save);
    await screen.findByText(/No change — logos\.workspace\.toml on disk already matches; nothing was written/);
    expect(posts.map((p) => p.form.get("fingerprint"))).toEqual(["fp-loaded", "fp-2"]);
  });
});

describe("the typed [workspace.autodiscover] and [workspace.warm] fields", () => {
  const TUNED = [
    "[workspace]",
    'name = "shop"',
    "",
    "[workspace.autodiscover]",
    "enabled = false",
    "",
    "[workspace.warm]",
    "concurrency = 2",
    "",
  ].join("\n");
  const tuned = () =>
    doc({
      content: TUNED,
      parsed: { workspace: { name: "shop", autodiscover: { enabled: false }, warm: { concurrency: 2 } } },
    });

  it("pre-fill from the parse and patch their own tables", async () => {
    await mount({ manifests: [tuned()] });
    expect(screen.getByLabelText("enabled")).toHaveValue("false");
    expect(screen.getByLabelText("concurrency")).toHaveValue(2);
    const user = userEvent.setup();
    await user.clear(screen.getByLabelText("concurrency"));
    await user.type(screen.getByLabelText("concurrency"), "3");
    await user.selectOptions(screen.getByLabelText("enabled"), "true");
    expect(rawPane().value).toBe(TUNED.replace("concurrency = 2", "concurrency = 3").replace("enabled = false", "enabled = true"));
  });

  it("'(not declared — off)' removes the table, so an explicit OFF is never saved as a bare ON table", async () => {
    await mount({ manifests: [tuned()] });
    await userEvent.setup().selectOptions(screen.getByLabelText("enabled"), "");
    expect(rawPane().value).not.toContain("[workspace.autodiscover]");
    expect(rawPane().value).not.toContain("enabled");
    expect(rawPane().value).toContain("[workspace.warm]\nconcurrency = 2");
  });
});

describe("validate, then write (FR-UI-38 AC2)", () => {
  it("renders the parser's refusal inline and says nothing was written", async () => {
    await mount({
      saves: [{ status: 422, body: { error: "unknown field `membrs`, expected one of `name`, `members`" } }],
    });
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: /Save logos\.workspace\.toml/ }));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(/Validation error — nothing was written: unknown field `membrs`/);
  });

  it("states an I/O fault as a failed save, not as a validation error", async () => {
    await mount({ saves: [{ status: 500, body: { error: "writing logos.workspace.toml: disk full" } }] });
    await userEvent.setup().click(screen.getByRole("button", { name: /Save logos\.workspace\.toml/ }));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Save failed (500): writing logos.workspace.toml: disk full");
    expect(alert).not.toHaveTextContent(/Validation error/);
  });

  it("renders a manifest that cannot be read as the honest error panel, with no editor", async () => {
    stubApi({ manifests: [null] });
    render(
      <WorkspaceProvider>
        <WorkspaceConfigView />
      </WorkspaceProvider>,
    );
    expect(await screen.findByText(/The request to \/api\/v1\/workspace\/manifest failed \(HTTP 500\)/)).toBeInTheDocument();
    expect(screen.queryByLabelText(/^Raw TOML/)).toBeNull();
  });

  it("delivers a manifest broken on disk for repair: the fault named, no fabricated typed fields", async () => {
    const broken = "[workspace]\nname = \"shop\"\nmembrs = []\n";
    await mount({ manifests: [doc({ content: broken, parsed: null, error: "unknown field `membrs`" })] });
    expect(rawPane().value).toBe(broken);
    expect(screen.getByText(/does not parse — every command in this workspace fails on it/)).toHaveTextContent(
      /unknown field `membrs`/,
    );
    expect(screen.queryByLabelText("name")).toBeNull();
    expect(screen.getByText(/Typed fields are unavailable while the document does not parse/)).toBeInTheDocument();
  });
});

describe("no silent clobber (FR-UI-38 AC3)", () => {
  const conflict: ManifestSaveOutcome = {
    outcome: "conflict",
    path: "logos.workspace.toml",
    loaded_fingerprint: "fp-loaded",
    disk_fingerprint: "fp-disk",
    disk_content: "# edited by hand\n[workspace]\nname = \"shop\"\n",
  };

  it("renders the conflict, the copy on disk, and the refusal — nothing is written", async () => {
    await mount({ saves: [{ status: 409, body: conflict }] });
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: /Save logos\.workspace\.toml/ }));
    expect(await screen.findByText(/Not saved — logos\.workspace\.toml changed on disk/)).toHaveTextContent(
      /Nothing was written/,
    );
    const panel = screen.getByText("CONFLICT").closest("section") as HTMLElement;
    expect(within(panel).getByLabelText("The manifest on disk now")).toHaveValue(conflict.disk_content);
    expect(within(panel).getByText(/Your save was refused/)).toBeInTheDocument();
  });

  it("overwrites only on the explicit choice, against the disk's fingerprint, and says so", async () => {
    const { posts } = await mount({
      saves: [
        { status: 409, body: conflict },
        { status: 200, body: { outcome: "written", path: "logos.workspace.toml", bytes_written: 9, fingerprint: "fp-mine" } },
      ],
    });
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: /Save logos\.workspace\.toml/ }));
    await user.click(await screen.findByRole("button", { name: /Overwrite it with my edits/ }));
    expect(await screen.findByText(/Overwrote the changes on disk with your edits/)).toBeInTheDocument();
    expect(posts.map((p) => p.form.get("fingerprint"))).toEqual(["fp-loaded", "fp-disk"]);
    expect(posts[1].form.get("content")).toBe(CONTENT);
    expect(screen.queryByText("CONFLICT")).toBeNull();
  });

  it("loading the disk copy re-reads the manifest and discards the edits", async () => {
    const onDisk = doc({ content: conflict.disk_content, fingerprint: "fp-disk" });
    const { posts, gets } = await mount({ saves: [{ status: 409, body: conflict }], manifests: [doc(), onDisk] });
    const user = userEvent.setup();
    await user.type(rawPane(), "# my edit\n");
    await user.click(screen.getByRole("button", { name: /Save logos\.workspace\.toml/ }));
    await user.click(await screen.findByRole("button", { name: /Load the version on disk/ }));
    await waitFor(() => expect(rawPane().value).toBe(conflict.disk_content));
    // …and it says which resolution happened, as the overwrite path does (AC3).
    expect(await screen.findByText(/Loaded the version on disk — your unsaved edits were discarded/)).toBeInTheDocument();
    expect(gets.filter((u) => u.startsWith("/api/v1/workspace/manifest"))).toHaveLength(2);
    expect(posts).toHaveLength(1);
  });

  it("re-seeds from disk even when the disk holds the FIRST load's bytes again", async () => {
    // Save A→B, then `git checkout` puts A back: the next save conflicts with A, and
    // the reload returns A — the same fingerprint the editor was first mounted with.
    const toA = { ...conflict, loaded_fingerprint: "fp-B", disk_fingerprint: "fp-loaded", disk_content: CONTENT };
    await mount({
      saves: [
        { status: 200, body: { outcome: "written", path: "logos.workspace.toml", bytes_written: 1, fingerprint: "fp-B" } },
        { status: 409, body: toA },
      ],
      manifests: [doc(), doc({ fingerprint: "fp-B" }), doc()],
    });
    const user = userEvent.setup();
    const save = screen.getByRole("button", { name: /Save logos\.workspace\.toml/ });
    await user.click(save);
    await screen.findByText(/Saved logos\.workspace\.toml/);
    await user.type(rawPane(), "# my later edit\n");
    await user.click(save);
    await user.click(await screen.findByRole("button", { name: /Load the version on disk/ }));
    await waitFor(() => expect(rawPane().value).toBe(CONTENT));
    expect(screen.queryByText("CONFLICT")).toBeNull();
  });
});

describe("governance is advisory, beside its findings (FR-UI-38 AC4, ADR-56)", () => {
  it("states the advisory contract inside the [governance] group, where the rules are edited", async () => {
    await mount();
    const group = screen.getByText("[governance]", { selector: "legend" }).closest("fieldset") as HTMLElement;
    expect(within(group).getByText(/Workspace governance is/)).toHaveTextContent(
      /advisory.*never moves any member's gated signal/,
    );
  });

  it("renders the declared family and the check's findings together", async () => {
    await mount();
    const group = screen.getByText("[governance]", { selector: "legend" }).closest("fieldset") as HTMLElement;
    expect(within(group).getByText("edge → core")).toBeInTheDocument();
    expect(await within(group).findByText(/1 rule\(s\) checked over 3 cross-service binding\(s\): 1 violation/)).toBeInTheDocument();
    expect(within(group).getByText("workspace-boundary:edge->core")).toBeInTheDocument();
  });

  it("renders the honest empty — nothing was checked — never a passing report", async () => {
    await mount({ check: { ...CLEAN_CHECK, governance: null } });
    expect(await screen.findByText(/No governance rules were in effect, so nothing was checked/)).toBeInTheDocument();
    expect(screen.queryByText(/0 violation/)).toBeNull();
  });

  it("says the findings predate the manifest on disk when the serve evaluates other rules", async () => {
    await mount({ manifests: [doc({ governance_in_effect: false })] });
    expect(await screen.findByText(/evaluated against the \[governance\] rules this serve loaded when it started/)).toBeInTheDocument();
  });

  it("after a save that changes the rules, re-reads the rider rather than assuming it", async () => {
    await mount({
      manifests: [doc(), doc({ fingerprint: "fp-saved", governance_in_effect: false })],
      saves: [{ status: 200, body: { outcome: "written", path: "logos.workspace.toml", bytes_written: 1, fingerprint: "fp-saved" } }],
    });
    expect(screen.queryByText(/this serve loaded when it started/)).toBeNull();
    await userEvent.setup().click(screen.getByRole("button", { name: /Save logos\.workspace\.toml/ }));
    expect(await screen.findByText(/this serve loaded when it started/)).toBeInTheDocument();
  });
});

describe("after a save, the page describes only the disk it has read (NFR-CC-04)", () => {
  const WRITTEN = { status: 200, body: { outcome: "written", path: "logos.workspace.toml", bytes_written: 1, fingerprint: "fp-saved" } };
  const save = () => userEvent.setup().click(screen.getByRole("button", { name: /Save logos\.workspace\.toml/ }));

  it("a repair save re-seeds the editor: typed fields up, the fault gone, the save still stated", async () => {
    const broken = doc({ content: "[workspace]\nname = \"shop\"\nmembrs = []\n", parsed: null, error: "unknown field `membrs`" });
    await mount({ manifests: [broken, doc({ fingerprint: "fp-saved" })], saves: [WRITTEN] });
    expect(screen.queryByLabelText("name")).toBeNull();
    await save();
    expect(await screen.findByLabelText("name")).toHaveValue("shop");
    expect(screen.getByText(/Saved logos\.workspace\.toml/)).toBeInTheDocument();
    expect(screen.getByText("parses")).toBeInTheDocument();
    expect(screen.queryByText(/does not parse/)).toBeNull();
  });

  it("a re-read that finds bytes this editor did not write is 'unknown', never 'does not parse'", async () => {
    await mount({ manifests: [doc(), doc({ fingerprint: "fp-someone-else" })], saves: [WRITTEN] });
    await save();
    expect(await screen.findByText(/could not be re-read after this save, or changed again since/)).toBeInTheDocument();
    expect(screen.getByText("on disk: unknown")).toBeInTheDocument();
    expect(screen.getByText(/Whether these findings reflect the manifest on disk could not be established/)).toBeInTheDocument();
    expect(screen.queryByText(/does not parse/)).toBeNull();
  });

  it("a re-read that fails drops the claims the same way", async () => {
    await mount({ manifests: [doc(), null], saves: [WRITTEN] });
    await save();
    expect(await screen.findByText(/could not be re-read after this save/)).toBeInTheDocument();
    expect(screen.queryByText("edge → core")).toBeNull();
  });

  it("the declared rules listing is the re-read's, not the load's", async () => {
    const edited = doc({
      fingerprint: "fp-saved",
      parsed: { ...doc().parsed!, governance: { boundaries: [{ from: "edge", to: "billing", reason: null }] } },
    });
    await mount({ manifests: [doc(), edited], saves: [WRITTEN] });
    expect(screen.getByText("edge → core")).toBeInTheDocument();
    await save();
    expect(await screen.findByText("edge → billing")).toBeInTheDocument();
    expect(screen.queryByText("edge → core")).toBeNull();
  });
});

describe("app-scoped (FR-UI-38 AC5, ADR-66)", () => {
  it("re-issues neither read on a member switch, and never carries ?repo=", async () => {
    const { gets } = await mount();
    await screen.findByText(/violation/);
    const reads = () => gets.filter((u) => /workspace\/(manifest|check)/.test(u));
    const before = reads().length;
    expect(before).toBe(2);
    await act(async () => switcher.current?.("web"));
    await act(async () => new Promise((r) => setTimeout(r, 20)));
    expect(reads()).toHaveLength(before);
    expect(reads().every((u) => !u.includes("repo="))).toBe(true);
  });

  it("states it is not a workspace in single-root mode and reads nothing", async () => {
    const { gets } = await mount({ probeStatus: 404 });
    expect(await screen.findByText(/Not a workspace/)).toBeInTheDocument();
    expect(gets.some((u) => u.includes("workspace/manifest"))).toBe(false);
  });
});

// ── S-451: the workspace chat tier, a sibling group of the same view ──────────
//
// `<workspace-root>/.logos/config.toml` ([chat], [wiki].model) and the credential
// beside it, over the S-450 routes. The stub below serves the manifest group too,
// so every assertion here is made with S-430's group mounted beside this one.

/** The shape a workspace tier is declared in on the reference estate: a `[chat]`
 *  table naming a model, plus a dedicated wiki model. */
const TIER_CONTENT = [
  "# the estate's one chat policy",
  "[chat]",
  'provider = "anthropic"',
  'model = "claude-ws"',
  "",
  "[wiki]",
  'model = "claude-wiki"',
  "",
].join("\n");

const DEFAULT_BASE_URL = "https://openrouter.ai/api/v1";

function tier(over: { content?: string; exists?: boolean; model?: string | null; wiki?: string | null; key?: MaskedSecret } = {}): ConfigReadModel {
  const model = over.model === undefined ? "claude-ws" : over.model;
  const wiki = over.wiki === undefined ? "claude-wiki" : over.wiki;
  const key = over.key ?? { present: true, last4: "ab12" };
  const chat = { provider: "anthropic" as const, model, base_url: DEFAULT_BASE_URL };
  return {
    config: {
      path: ".logos/config.toml",
      exists: over.exists ?? true,
      content: over.content ?? TIER_CONTENT,
      parsed: { languages: [], include: [], exclude: [], max_file_size: 1048576, framework_hints: [], chat, wiki: { model: wiki } },
    },
    rules: { path: ".logos/rules.toml", exists: false, content: "", parsed: { constraints: {}, metric_thresholds: {} } },
    chat_key: key,
    effective_chat: {
      policy: chat,
      policy_origin: model ? "member" : "unset",
      credential: key,
      credential_origin: key.present ? "member" : "unset",
      member_key_withheld: false,
    },
  } as unknown as ConfigReadModel;
}

/** Stub `fetch` for both groups. `tiers` is served in turn (the last repeats;
 *  `null` answers a `500`, a string answers that literal JSON); `replies` answers
 *  each POST by its route. */
function stubTier({
  tiers = [tier()],
  replies = {},
  probeStatus = 200,
}: {
  tiers?: (ConfigReadModel | null | string)[];
  replies?: Record<string, { status: number; body: unknown }>;
  probeStatus?: number;
} = {}) {
  const gets: string[] = [];
  const posts: Posted[] = [];
  let t = 0;
  const respond = (body: unknown, status = 200) =>
    Promise.resolve({
      ok: status >= 200 && status < 300,
      status,
      json: () => Promise.resolve(typeof body === "string" ? JSON.parse(body) : body),
      text: () => Promise.resolve(typeof body === "string" ? body : JSON.stringify(body)),
    } as Response);
  vi.stubGlobal(
    "fetch",
    vi.fn((url: string, init?: RequestInit) => {
      if (init?.method === "POST") {
        posts.push({
          url,
          form: new URLSearchParams(String(init.body)),
          intent: new Headers(init.headers).get("x-logos-intent"),
        });
        const reply = replies[url] ?? { status: 599, body: { error: `unstubbed POST ${url}` } };
        return respond(reply.body, reply.status);
      }
      gets.push(url);
      if (url.startsWith("/api/v1/workspace/roster")) return respond(ROSTER, probeStatus);
      if (url.startsWith("/api/v1/workspace/manifest")) return respond(doc());
      if (url.startsWith("/api/v1/workspace/check")) return respond(CLEAN_CHECK);
      if (url.startsWith("/api/v1/workspace/config")) {
        const next = tiers[Math.min(t++, tiers.length - 1)];
        return next === null ? respond({ error: "boom" }, 500) : respond(next);
      }
      return respond({ error: `unstubbed GET ${url}` }, 599);
    }),
  );
  return { gets, posts };
}

async function mountTier(opts: Parameters<typeof stubTier>[0] = {}) {
  const api = stubTier(opts);
  render(
    <WorkspaceProvider>
      <CaptureSwitch />
      <WorkspaceConfigView />
    </WorkspaceProvider>,
  );
  if ((opts.probeStatus ?? 200) === 200) await screen.findByLabelText(/^Raw TOML/);
  return api;
}

/** The tier group's card — the nearest ancestor of its raw pane that holds its
 *  title. Every query below is scoped to it, so a match can never be the
 *  manifest group's. */
function tierCard(): HTMLElement {
  const raw = screen.getByLabelText(/^Workspace tier — raw TOML/);
  let el: HTMLElement | null = raw;
  while (el && !within(el).queryByText("Workspace chat policy and credential")) el = el.parentElement;
  return el as HTMLElement;
}

async function mountedTier(opts: Parameters<typeof stubTier>[0] = {}) {
  const api = await mountTier(opts);
  await screen.findByLabelText(/^Workspace tier — raw TOML/);
  return { ...api, card: tierCard() };
}

const TIER_RAW = /^Workspace tier — raw TOML/;
const SAVE_TIER = /^Save <workspace-root>\/\.logos\/config\.toml$/;
const SAVE_KEY = /^Save the workspace API key$/;

describe("the workspace chat tier round-trips in the manifest group's grammar (S-451 AC1)", () => {
  it("loads the literal document into its own raw pane and pre-fills [chat]/[wiki] from the parse", async () => {
    const { card } = await mountedTier();
    expect((within(card).getByLabelText(TIER_RAW) as HTMLTextAreaElement).value).toBe(TIER_CONTENT);
    expect(within(card).getByLabelText("provider")).toHaveValue("anthropic");
    expect(within(card).getByLabelText("model")).toHaveValue("claude-ws");
    expect(within(card).getByLabelText("base_url")).toHaveValue(DEFAULT_BASE_URL);
    expect(within(card).getByLabelText("wiki model")).toHaveValue("claude-wiki");
    // The manifest group is still there, unchanged, beside it.
    expect(rawPane().value).toBe(CONTENT);
  });

  it("patches typed edits into the raw pane and posts it VERBATIM to the workspace route", async () => {
    const written = { file: "config", path: ".logos/config.toml", bytes_written: 77, provenance_stamped: false };
    const { card, posts } = await mountedTier({
      replies: { "/api/v1/workspace/config/save": { status: 200, body: written } },
    });
    const user = userEvent.setup();
    const model = within(card).getByLabelText("model");
    await user.clear(model);
    await user.type(model, "claude-next");
    await user.clear(within(card).getByLabelText("wiki model"));
    const raw = within(card).getByLabelText(TIER_RAW) as HTMLTextAreaElement;
    // Each typed edit patched its own table, and nothing else moved.
    const [chatTable, wikiTable] = raw.value.split("[wiki]");
    expect(chatTable).toContain('model = "claude-next"');
    expect(chatTable).toContain('provider = "anthropic"');
    expect(chatTable).not.toContain("claude-ws");
    expect(raw.value.startsWith("# the estate's one chat policy\n[chat]\n")).toBe(true);
    // A blanked wiki model removes the key rather than declaring `model = ""`.
    expect(wikiTable.trim()).toBe("");

    await user.click(within(card).getByRole("button", { name: SAVE_TIER }));
    const status = await within(card).findByText(/Saved <workspace-root>\/\.logos\/config\.toml \(77 bytes\)/);
    expect(status).toHaveTextContent(/next chat turn/);
    expect(status).toHaveTextContent(/No member's \.logos\/ was written and no member was reindexed/);

    expect(posts).toHaveLength(1);
    expect(posts[0].url).toBe("/api/v1/workspace/config/save");
    expect(posts[0].form.get("file")).toBe("config");
    expect(posts[0].form.get("content")).toBe(raw.value);
    expect(posts[0].intent).toBe("test-intent-token");
  });

  it("renders the server's refusal inline and says nothing was written", async () => {
    const { card } = await mountedTier({
      replies: { "/api/v1/workspace/config/save": { status: 422, body: { error: "unknown field `languags`" } } },
    });
    await userEvent.setup().click(within(card).getByRole("button", { name: SAVE_TIER }));
    expect(await within(card).findByRole("alert")).toHaveTextContent(
      "Validation error — nothing was written: unknown field `languags`",
    );
  });

  it("states a tier that is not yet created, and stops saying so once a save creates it", async () => {
    const written = { file: "config", path: ".logos/config.toml", bytes_written: 1, provenance_stamped: false };
    const { card } = await mountedTier({
      tiers: [tier({ content: "", exists: false, model: null, wiki: null, key: { present: false } })],
      replies: { "/api/v1/workspace/config/save": { status: 200, body: written } },
    });
    expect(within(card).getByText("not yet created")).toBeInTheDocument();
    // Undeclared: blank, never a value the tier did not state.
    expect(within(card).getByLabelText("model")).toHaveValue("");
    expect(within(card).getByLabelText("wiki model")).toHaveValue("");
    await userEvent.setup().click(within(card).getByRole("button", { name: SAVE_TIER }));
    await within(card).findByText(/Saved/);
    expect(within(card).queryByText("not yet created")).toBeNull();
    expect(within(card).getByText("on disk")).toBeInTheDocument();
  });

  it("keeps the two groups' saves apart: neither posts to the other's route", async () => {
    const written = { file: "config", path: ".logos/config.toml", bytes_written: 1, provenance_stamped: false };
    const { card, posts } = await mountedTier({
      replies: { "/api/v1/workspace/config/save": { status: 200, body: written } },
    });
    await userEvent.setup().click(within(card).getByRole("button", { name: SAVE_TIER }));
    await within(card).findByText(/Saved/);
    expect(posts.map((p) => p.url)).toEqual(["/api/v1/workspace/config/save"]);
    expect(screen.queryByText(/Saved logos\.workspace\.toml/)).toBeNull();
  });
});

describe("the workspace credential is masked and write-only (S-451 AC1, NFR-SE-07)", () => {
  it("is never pre-filled and shows only presence and the last 4", async () => {
    const { card } = await mountedTier();
    const input = within(card).getByLabelText("api_key") as HTMLInputElement;
    expect(input.type).toBe("password");
    expect(input.value).toBe("");
    expect(within(card).getByText("set · ends …ab12")).toBeInTheDocument();
  });

  it("posts the typed key to the workspace route, clears it, and shows only the masked outcome", async () => {
    const raw = "sk-typed-workspace-key-zz99";
    const { card, posts } = await mountedTier({
      replies: {
        "/api/v1/workspace/config/secret": {
          status: 200,
          body: { path: ".logos/secrets.toml", chat_key: { present: true, last4: "zz99" } },
        },
      },
    });
    const user = userEvent.setup();
    const input = within(card).getByLabelText("api_key") as HTMLInputElement;
    await user.type(input, raw);
    await user.click(within(card).getByRole("button", { name: SAVE_KEY }));
    expect(await within(card).findByText(/Key saved \(ends …zz99\)/)).toHaveTextContent(
      /<workspace-root>\/\.logos\/secrets\.toml/,
    );
    expect(input.value).toBe("");
    expect(within(card).getByText("set · ends …zz99")).toBeInTheDocument();
    expect(posts).toHaveLength(1);
    expect(posts[0].url).toBe("/api/v1/workspace/config/secret");
    expect(posts[0].form.get("api_key")).toBe(raw);
    expect(posts[0].intent).toBe("test-intent-token");
    expect(document.body.textContent).not.toContain(raw);
  });

  it("a blank save clears the key, and says so", async () => {
    const { card, posts } = await mountedTier({
      replies: {
        "/api/v1/workspace/config/secret": { status: 200, body: { path: ".logos/secrets.toml", chat_key: { present: false } } },
      },
    });
    await userEvent.setup().click(within(card).getByRole("button", { name: SAVE_KEY }));
    expect(await within(card).findByText(/Key cleared/)).toBeInTheDocument();
    expect(within(card).getByText("not set")).toBeInTheDocument();
    expect(posts[0].form.get("api_key")).toBe("");
  });

  it("never renders the key route's error body, which could carry key material", async () => {
    const raw = "sk-echoed-by-a-bad-server-ee11";
    const { card } = await mountedTier({
      replies: { "/api/v1/workspace/config/secret": { status: 422, body: { error: `bad store near ${raw}` } } },
    });
    const user = userEvent.setup();
    await user.type(within(card).getByLabelText("api_key"), raw);
    await user.click(within(card).getByRole("button", { name: SAVE_KEY }));
    expect(await within(card).findByRole("alert")).toHaveTextContent("the server rejected the key write");
    expect(document.body.textContent).not.toContain(raw);
  });
});

describe("the key route's replies never reach the page (S-451, NFR-SE-07)", () => {
  it("a non-JSON 2xx is 'saved, format not understood' — the body is never rendered and the badge is unmoved", async () => {
    const raw = "sk-in-an-html-reply-hh22";
    const { card } = await mountedTier({
      replies: { "/api/v1/workspace/config/secret": { status: 200, body: `<html>stored ${raw}</html>` } },
    });
    const user = userEvent.setup();
    await user.type(within(card).getByLabelText("api_key"), raw);
    await user.click(within(card).getByRole("button", { name: SAVE_KEY }));
    expect(await within(card).findByText("Key saved (unexpected response format).")).toBeInTheDocument();
    expect(within(card).getByText("set · ends …ab12")).toBeInTheDocument();
    expect(document.body.textContent).not.toContain(raw);
  });

  it("a 5xx carries the fixed detail, never its body, exactly as a 422 does", async () => {
    const raw = "sk-echoed-in-a-500-ff33";
    const { card } = await mountedTier({
      replies: { "/api/v1/workspace/config/secret": { status: 500, body: { error: `writing secrets.toml near ${raw}` } } },
    });
    const user = userEvent.setup();
    await user.type(within(card).getByLabelText("api_key"), raw);
    await user.click(within(card).getByRole("button", { name: SAVE_KEY }));
    expect(await within(card).findByRole("alert")).toHaveTextContent("Save failed (500): the server rejected the key write");
    expect(document.body.textContent).not.toContain(raw);
  });
});

describe("the tier's reach is stated on the surface (S-451 AC2, AC3)", () => {
  it("names each file beside its group", async () => {
    const { card } = await mountedTier();
    expect(within(card).getByText("<workspace-root>/.logos/config.toml", { selector: "span" })).toBeInTheDocument();
    expect(within(card).getByText("<workspace-root>/.logos/secrets.toml", { selector: "span" })).toBeInTheDocument();
  });

  it("states that members inherit each half they do not declare (ADR-67)", async () => {
    const { card } = await mountedTier();
    const banner = within(card).getByText("INHERITED PER HALF").closest("section") as HTMLElement;
    // The policy half is conditional on THIS root declaring a model (ADR-67 §3)…
    expect(banner).toHaveTextContent(
      /When this root declares a \[chat\] model, a member whose own \.logos\/config\.toml declares none inherits this whole \[chat\] table/,
    );
    expect(banner).toHaveTextContent(/with no model here, nothing is inherited/);
    // …and the credential reaches every member that does not inherit the policy,
    // including one whose policy is unset at both roots (ADR-67 §2).
    expect(banner).toHaveTextContent(/does not inherit this root's \[chat\] table — it declares its own, or neither root declares one/);
    expect(banner).toHaveTextContent(/holds no key .* uses the key saved here/);
    // HF-1: the direction a member key never travels is stated, not left implied.
    expect(banner).toHaveTextContent(/with this root's key only/);
  });

  it("does not claim members inherit the [wiki] model, which no member reads from this root", async () => {
    const { card } = await mountedTier();
    const wiki = within(card).getByText("[wiki]", { selector: "legend" }).closest("fieldset") as HTMLElement;
    expect(wiki).toHaveTextContent(/Not inherited/);
    expect(wiki).toHaveTextContent(/its own \[wiki\] model, else its effective \[chat\] model/);
    const banner = within(card).getByText("INHERITED PER HALF").closest("section") as HTMLElement;
    expect(banner).not.toHaveTextContent(/\[wiki\]/);
  });

  it("says there is no indexing key, no rules document and no apply action here", async () => {
    const { card } = await mountedTier();
    const notHere = within(card).getByText("NOT HERE").closest("section") as HTMLElement;
    expect(notHere).toHaveTextContent(/no indexing key/);
    expect(notHere).toHaveTextContent(/languages, include, exclude, max_file_size, framework_hints/);
    expect(notHere).toHaveTextContent(/no rules document/);
    expect(notHere).toHaveTextContent(/no Apply action/);
    // …and the absence it states is real.
    for (const key of ["languages", "include", "exclude", "max_file_size", "framework_hints"]) {
      expect(within(card).queryByLabelText(key)).toBeNull();
    }
    expect(screen.queryByRole("button", { name: /Apply/ })).toBeNull();
    expect(screen.queryByText(/rules\.toml/, { selector: "span" })).toBeNull();
  });
});

describe("the tier group's own read (S-451, NFR-RA-05)", () => {
  it("a failed read is stated inside the group, offers no Save, and leaves the manifest group working", async () => {
    await mountTier({ tiers: [null] });
    expect(await screen.findByText(/The workspace chat tier could not be loaded/)).toHaveTextContent(/HTTP 500/);
    expect(screen.queryByRole("button", { name: SAVE_TIER })).toBeNull();
    expect(screen.queryByRole("button", { name: SAVE_KEY })).toBeNull();
    expect(screen.getByRole("button", { name: /Save logos\.workspace\.toml/ })).toBeInTheDocument();
  });

  it("a 2xx that is not the read-model is refused, never seeded into an editor that could save it", async () => {
    await mountTier({ tiers: ["{}"] });
    expect(await screen.findByText(/The workspace chat tier could not be loaded/)).toHaveTextContent(
      /without a config document/,
    );
    expect(screen.queryByLabelText(TIER_RAW)).toBeNull();
    expect(screen.queryByRole("button", { name: SAVE_TIER })).toBeNull();
  });
});

describe("a malformed 2xx is stated in the group, never a crash of the page (S-451, NFR-RA-05)", () => {
  const valid = () => JSON.parse(JSON.stringify(tier())) as Record<string, Record<string, unknown>>;

  it.each([
    ["no key state", () => { const m = valid(); delete m.chat_key; return m; }],
    ["no parsed document", () => { const m = valid(); m.config.parsed = null; return m; }],
    ["no parsed [chat]", () => { const m = valid(); m.config.parsed = { languages: [] }; return m; }],
  ])("a read-model with %s is refused and the manifest group stays usable", async (_label, make) => {
    await mountTier({ tiers: [JSON.stringify(make())] });
    expect(await screen.findByText(/The workspace chat tier could not be loaded/)).toBeInTheDocument();
    expect(screen.queryByLabelText(TIER_RAW)).toBeNull();
    expect(screen.getByRole("button", { name: /Save logos\.workspace\.toml/ })).toBeInTheDocument();
  });

  it("a key reply with no key state is 'format not understood', and the badge is unmoved", async () => {
    const { card } = await mountedTier({
      replies: { "/api/v1/workspace/config/secret": { status: 200, body: { path: ".logos/secrets.toml" } } },
    });
    await userEvent.setup().click(within(card).getByRole("button", { name: SAVE_KEY }));
    expect(await within(card).findByText("Key saved (unexpected response format).")).toBeInTheDocument();
    expect(within(card).getByText("set · ends …ab12")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Save logos\.workspace\.toml/ })).toBeInTheDocument();
  });
});

describe("the tier group is app-scoped and workspace-only (S-451 AC5)", () => {
  it("re-issues no tier read on a member switch, and never carries ?repo=", async () => {
    const { gets } = await mountedTier();
    const reads = () => gets.filter((u) => u.startsWith("/api/v1/workspace/config"));
    expect(reads()).toHaveLength(1);
    // Two switches, so at least one is a real change whichever member the URL an
    // earlier test left behind opened on — a switch to the current member moves
    // nothing and would prove nothing.
    for (const name of ["api", "web"]) {
      await act(async () => switcher.current?.(name));
      await act(async () => new Promise((r) => setTimeout(r, 20)));
    }
    expect(reads()).toHaveLength(1);
    expect(reads().every((u) => !u.includes("repo="))).toBe(true);
  });

  it("is neither rendered nor read in single-root mode", async () => {
    const { gets } = await mountTier({ probeStatus: 404 });
    expect(await screen.findByText(/Not a workspace/)).toBeInTheDocument();
    expect(screen.queryByText("Workspace chat policy and credential")).toBeNull();
    expect(gets.some((u) => u.includes("workspace/config"))).toBe(false);
  });
});
