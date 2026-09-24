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
  ManifestSaveOutcome,
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
  manifests?: WorkspaceManifestDocument[];
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
      if (url.startsWith("/api/v1/workspace/manifest")) return respond(manifests[Math.min(m++, manifests.length - 1)]);
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
