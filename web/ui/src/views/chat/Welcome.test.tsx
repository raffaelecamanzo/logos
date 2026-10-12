import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// The network module is mocked as ChatView.test.tsx mocks it: the chat's own calls
// (config, threads, the turn stream) are `vi.fn`s, so "no provider request is made"
// is a mock COUNT. The read-model reads the chips make go through `fetch`, which each
// test stubs by hand — they are a different seam, and the one under test here.
vi.mock("../../api/chatClient.ts", () => ({
  CHAT_ROUTE: "/chat",
  CHAT_THREADS_ROUTE: "/api/v1/chat/threads",
  MEMBER_CHAT_ROUTES: { turn: "/chat", threads: "/api/v1/chat/threads", memberScoped: true },
  WORKSPACE_CHAT_ROUTES: {
    turn: "/workspace/chat",
    threads: "/api/v1/workspace/chat/threads",
    memberScoped: false,
  },
  fetchChatConfig: vi.fn(),
  fetchWorkspaceChatConfig: vi.fn(),
  fetchWorkspaceChatReadRoots: vi.fn(),
  streamChatTurn: vi.fn(),
  deleteChatThread: vi.fn(),
  truncateChatThread: vi.fn(),
  fetchThreads: vi.fn(),
  fetchThreadMessages: vi.fn(),
}));

import {
  fetchChatConfig,
  fetchThreadMessages,
  fetchThreads,
  fetchWorkspaceChatConfig,
  fetchWorkspaceChatReadRoots,
  MEMBER_CHAT_ROUTES,
  streamChatTurn,
  WORKSPACE_CHAT_ROUTES,
} from "../../api/chatClient.ts";
import type { ChatConfigReadModel, WorkspaceChatConfigReadModel } from "./chatModel.ts";
import { ChatView } from "./ChatView.tsx";
import { WorkspaceChatView } from "./WorkspaceChatView.tsx";
import { WorkspaceProvider } from "../../workspace/WorkspaceContext.tsx";
import { setScopedMember } from "../../workspace/scope.ts";
import { ROSTER } from "../../workspace/testFixtures.ts";
import { readStoredPresets } from "./presets.ts";

const mockFetchConfig = vi.mocked(fetchChatConfig);
const mockStreamTurn = vi.mocked(streamChatTurn);
const mockFetchThreads = vi.mocked(fetchThreads);
const mockFetchMessages = vi.mocked(fetchThreadMessages);
const mockFetchWorkspaceConfig = vi.mocked(fetchWorkspaceChatConfig);
const mockFetchReadRoots = vi.mocked(fetchWorkspaceChatReadRoots);

function configuredModel(): ChatConfigReadModel {
  return {
    effective_chat: {
      policy: {
        provider: "openai",
        model: "openrouter/some-model",
        base_url: "https://openrouter.ai/api/v1",
        max_tool_calls: 24,
        max_subagent_tool_calls: 8,
        max_replans: 3,
      },
      policy_origin: "member",
      credential: { present: true, last4: "SECRET4" } as { present: boolean },
      credential_origin: "member",
      member_key_withheld: false,
    },
  };
}

function workspaceTier(): WorkspaceChatConfigReadModel {
  return { ...configuredModel(), config: { error: null }, chat_key_error: null };
}

function sseResponse(chunks: string[]): Response {
  const enc = new TextEncoder();
  const body = new ReadableStream<Uint8Array>({
    start(controller) {
      for (const c of chunks) controller.enqueue(enc.encode(c));
      controller.close();
    },
  });
  return { ok: true, status: 200, body } as unknown as Response;
}

/** A `/files` bundle over the slice the chips read. */
function filesBundle(paths: string[], fileCount = 1200) {
  return {
    status: { indexed: true, file_count: fileCount },
    hotspots: { files: paths.map((path) => ({ path })) },
  };
}

interface Stub {
  /** Every URL fetched. */
  calls: string[];
}

/** Stub `fetch` over the read-models the welcome reads. `files` answers the plain
 *  board and `untested` the `?untested` one; a function answers per call. */
function stubReads(opts: {
  files?: unknown;
  untested?: unknown;
  /** Make every `/files` read fail. */
  filesFail?: boolean;
  /** Never settle the `/files` reads. */
  filesPending?: boolean;
  /** Answer the roster probe (workspace mode); absent ⇒ 404, i.e. a single root. */
  roster?: boolean;
}): Stub {
  const calls: string[] = [];
  const json = (body: unknown, code = 200) =>
    Promise.resolve({ ok: code === 200, status: code, json: () => Promise.resolve(body) } as Response);
  vi.stubGlobal(
    "fetch",
    vi.fn((url: string) => {
      calls.push(url);
      if (url.startsWith("/api/v1/workspace/roster")) {
        return opts.roster ? json(ROSTER) : json({}, 404);
      }
      if (url.startsWith("/api/v1/files")) {
        if (opts.filesPending) return new Promise<Response>(() => {});
        if (opts.filesFail) return json({ error: "boom" }, 500);
        return json(url.includes("untested") ? (opts.untested ?? {}) : (opts.files ?? {}));
      }
      return json({});
    }),
  );
  return { calls };
}

beforeEach(() => {
  window.localStorage.clear();
  vi.clearAllMocks();
  mockFetchThreads.mockResolvedValue([]);
  mockFetchMessages.mockResolvedValue([]);
  mockFetchReadRoots.mockResolvedValue([]);
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  setScopedMember(null);
  window.history.replaceState({}, "", "/");
});

const chipsGroup = () => screen.findByRole("group", { name: "Suggested questions" });
const chipTexts = (group: HTMLElement) => within(group).getAllByRole("button").map((b) => b.textContent);

async function acceptConsent(user: ReturnType<typeof userEvent.setup>) {
  await user.click(await screen.findByRole("button", { name: "Start chatting" }));
}

/** Pick a preset from the composer's "+" menu (S-625). */
async function choosePreset(user: ReturnType<typeof userEvent.setup>, label: string) {
  await user.click(screen.getByRole("button", { name: "Composer options" }));
  await user.click(within(screen.getByRole("menu", { name: "Preset" })).getByRole("menuitemradio", { name: label }));
}

describe("Welcome — the member chat's chips (S-621, FR-UI-46)", () => {
  it("greets an empty thread and offers chips naming the facts the read-models returned", async () => {
    stubReads({
      files: filesBundle(["src/engine.rs", "src/store.rs"], 1200),
      untested: filesBundle(["src/legacy.rs"]),
    });
    mockFetchConfig.mockResolvedValue(configuredModel());
    render(<ChatView />);

    expect(await screen.findByText(/What would you like to know about this project\?/)).toBeInTheDocument();
    const group = await chipsGroup();
    expect(chipTexts(group)).toEqual([
      "Why is `src/engine.rs` the top hotspot, and what depends on it?",
      "What would it take to cover `src/legacy.rs`, the riskiest untested file?",
      "Give me a tour of this project's 1,200 indexed files.",
    ]);
  });

  it("falls back to generic chips, naming nothing, when the read-models are empty", async () => {
    stubReads({ files: filesBundle([], 0), untested: filesBundle([], 0) });
    mockFetchConfig.mockResolvedValue(configuredModel());
    render(<ChatView />);

    const texts = chipTexts(await chipsGroup());
    expect(texts.length).toBeGreaterThanOrEqual(2);
    expect(texts.length).toBeLessThanOrEqual(4);
    for (const text of texts) {
      expect(text).not.toMatch(/`/);
      expect(text).not.toMatch(/\d/);
    }
  });

  it("falls back to the same generic chips when the reads fail or answer an unknown shape", async () => {
    stubReads({ filesFail: true });
    mockFetchConfig.mockResolvedValue(configuredModel());
    const failed = render(<ChatView />);
    const afterFailure = chipTexts(await chipsGroup());
    failed.unmount();

    stubReads({}); // every read answers `{}`
    render(<ChatView />);
    const afterEmptyShape = chipTexts(await chipsGroup());
    expect(afterEmptyShape).toEqual(afterFailure);
    expect(afterEmptyShape.join(" ")).not.toMatch(/`|\d/);
  });

  it("names only what each board returned: a hotspot without an untested board turns only that chip generic", async () => {
    stubReads({ files: filesBundle(["src/engine.rs"], 7), untested: filesBundle([], 7) });
    mockFetchConfig.mockResolvedValue(configuredModel());
    render(<ChatView />);

    const texts = chipTexts(await chipsGroup());
    expect(texts[0]).toContain("`src/engine.rs`");
    expect(texts[1]).not.toMatch(/`/);
    expect(texts[2]).toContain("7 indexed files");
  });

  it("hides the chips until the reads settle, then shows them", async () => {
    stubReads({ filesPending: true });
    mockFetchConfig.mockResolvedValue(configuredModel());
    render(<ChatView />);

    expect(await screen.findByText(/What would you like to know about this project\?/)).toBeInTheDocument();
    expect(screen.queryByRole("group", { name: "Suggested questions" })).not.toBeInTheDocument();
  });

  it("issues no provider request before a send, and a chip is inert before consent", async () => {
    const user = userEvent.setup();
    const { calls } = stubReads({ files: filesBundle(["src/engine.rs"]), untested: filesBundle([]) });
    mockFetchConfig.mockResolvedValue(configuredModel());
    render(<ChatView />);

    const chips = within(await chipsGroup()).getAllByRole("button");
    for (const chip of chips) expect(chip).toBeDisabled();
    // The greeting says why the chips are greyed out, and what to do about it.
    expect(screen.getByText(/Accept the notice above/)).toBeInTheDocument();
    await user.click(chips[0]!);

    expect(mockStreamTurn).not.toHaveBeenCalled();
    // The only requests the welcome made were the two read-model GETs.
    expect(calls.filter((u) => !u.startsWith("/api/v1/files"))).toEqual([]);
    // Still a welcome: nothing was appended to the thread.
    expect(screen.getByRole("group", { name: "Suggested questions" })).toBeInTheDocument();
  });

  it("after consent, a chip sends its text as the first message and the chips disappear", async () => {
    const user = userEvent.setup();
    stubReads({ files: filesBundle(["src/engine.rs"]), untested: filesBundle(["src/legacy.rs"]) });
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockStreamTurn.mockResolvedValue(
      sseResponse(['event: final_answer\ndata: {"answer":"Because it churns."}\n\n']),
    );
    render(<ChatView />);
    await acceptConsent(user);

    const group = await chipsGroup();
    const chip = within(group).getAllByRole("button")[0]!;
    const text = chip.textContent!;
    expect(chip).toBeEnabled();
    expect(screen.getByText(/Start from a suggestion, or ask your own question below/)).toBeInTheDocument();
    expect(screen.queryByText(/Accept the notice above/)).not.toBeInTheDocument();
    await user.click(chip);

    // Sent through the runtime's own path: the very text, a fresh conversation, once,
    // under the conversation's preset (S-625) — Default here.
    await waitFor(() => expect(mockStreamTurn).toHaveBeenCalledTimes(1));
    expect(mockStreamTurn).toHaveBeenCalledWith(MEMBER_CHAT_ROUTES, text, null, "default", expect.anything());
    expect(await screen.findByText("Because it churns.")).toBeInTheDocument();
    expect(screen.getByText(text)).toBeInTheDocument(); // the user's bubble
    expect(screen.queryByRole("group", { name: "Suggested questions" })).not.toBeInTheDocument();
    expect(screen.queryByText(/What would you like to know about/)).not.toBeInTheDocument();
  });

  // The Default above is also the fallback, so it cannot tell "the conversation's
  // preset" from "Default": a chip sent under another one must carry that one, and
  // the thread its first send creates keeps it (S-625 × S-621).
  it("a chip sends under the empty conversation's non-Default preset, which the new thread keeps", async () => {
    const user = userEvent.setup();
    stubReads({ files: filesBundle(["src/engine.rs"]), untested: filesBundle(["src/legacy.rs"]) });
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockFetchThreads.mockResolvedValueOnce([]);
    mockFetchThreads.mockResolvedValue([{ id: 4, title: "chip question", updated_at: 100 }]);
    mockStreamTurn.mockResolvedValue(sseResponse(['event: final_answer\ndata: {"answer":"ok"}\n\n']));
    render(<ChatView />);
    await acceptConsent(user);
    await choosePreset(user, "Impact analysis");

    const chip = within(await chipsGroup()).getAllByRole("button")[0]!;
    const text = chip.textContent!;
    await user.click(chip);
    await waitFor(() => expect(mockStreamTurn).toHaveBeenCalledTimes(1));
    expect(mockStreamTurn).toHaveBeenCalledWith(MEMBER_CHAT_ROUTES, text, null, "impact_analysis", expect.anything());
    expect(await screen.findByRole("button", { name: "chip question" })).toHaveAttribute("aria-current");
    expect(readStoredPresets("single").get(4)).toBe("impact_analysis");
  });
});

describe("Welcome — a restoring conversation (S-621 review)", () => {
  it("holds the chips while a stored conversation's messages are still on their way", async () => {
    const user = userEvent.setup();
    stubReads({ files: filesBundle(["src/engine.rs"]), untested: filesBundle([]) });
    window.localStorage.setItem("logos.chat.activeThread:single", "7");
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockFetchThreads.mockResolvedValue([{ id: 7, title: "Earlier chat", updated_at: 100 }]);
    let release!: (rows: Awaited<ReturnType<typeof fetchThreadMessages>>) => void;
    mockFetchMessages.mockReturnValue(new Promise((resolve) => (release = resolve)));
    render(<ChatView />);
    await acceptConsent(user);

    // The thread looks empty — the greeting and its chips are up — but a click would
    // send into conversation 7 and the restore would then wipe it from view.
    const chips = within(await chipsGroup()).getAllByRole("button");
    for (const chip of chips) expect(chip).toBeDisabled();
    await user.click(chips[0]!);
    expect(mockStreamTurn).not.toHaveBeenCalled();

    release([
      { id: 1, role: "user", content: "earlier question", created_at: 0, tool_traces: [] },
      { id: 2, role: "assistant", content: "earlier answer", created_at: 0, tool_traces: [] },
    ]);
    expect(await screen.findByText("earlier answer")).toBeInTheDocument();
    expect(screen.queryByRole("group", { name: "Suggested questions" })).not.toBeInTheDocument();
  });
});

describe("Welcome — the Workspace Chat's chips (S-621, FR-UI-46)", () => {
  function renderWorkspaceChat(member = "web") {
    window.history.replaceState({}, "", `/workspace/chat?repo=${member}`);
    mockFetchWorkspaceConfig.mockResolvedValue(workspaceTier());
    render(
      <WorkspaceProvider>
        <WorkspaceChatView />
      </WorkspaceProvider>,
    );
  }

  it("builds its chips from the roster and the selected member, and reads nothing else", async () => {
    const { calls } = stubReads({ roster: true });
    renderWorkspaceChat("web");

    expect(await screen.findByText(/What would you like to know about this workspace\?/)).toBeInTheDocument();
    expect(chipTexts(await chipsGroup())).toEqual([
      "How do the 2 members of `shop` depend on each other?",
      "Which services talk to each other over a message broker?",
      "What does `web` do, and which other members depend on it?",
    ]);
    // Engine-free (NFR-PE-10): no member-store read — in particular not the status
    // fan-out — is made on the welcome's behalf.
    expect(calls.filter((u) => /\/api\/v1\/(workspace\/status|files)/.test(u))).toEqual([]);
  });

  it("names the member the shell has selected, not the manifest default", async () => {
    stubReads({ roster: true });
    renderWorkspaceChat("api");
    expect(chipTexts(await chipsGroup())[2]).toBe("What does `api` do, and which other members depend on it?");
  });

  it("is inert before consent; after consent a chip sends as the first workspace message", async () => {
    const user = userEvent.setup();
    stubReads({ roster: true });
    mockStreamTurn.mockResolvedValue(
      sseResponse(['event: final_answer\ndata: {"answer":"They meet on orders."}\n\n']),
    );
    renderWorkspaceChat();

    const group = await chipsGroup();
    const chip = within(group).getAllByRole("button")[1]!;
    const text = chip.textContent!;
    expect(chip).toBeDisabled();
    await user.click(chip);
    expect(mockStreamTurn).not.toHaveBeenCalled();

    await acceptConsent(user);
    expect(chip).toBeEnabled();
    await user.click(chip);
    await waitFor(() => expect(mockStreamTurn).toHaveBeenCalledTimes(1));
    expect(mockStreamTurn).toHaveBeenCalledWith(WORKSPACE_CHAT_ROUTES, text, null, "default", expect.anything());
    expect(await screen.findByText("They meet on orders.")).toBeInTheDocument();
    expect(screen.queryByRole("group", { name: "Suggested questions" })).not.toBeInTheDocument();
  });

  it("a chip sends under the workspace conversation's non-Default preset, kept in the workspace scope", async () => {
    const user = userEvent.setup();
    stubReads({ roster: true });
    mockFetchThreads.mockResolvedValueOnce([]);
    mockFetchThreads.mockResolvedValue([{ id: 3, title: "workspace chip", updated_at: 100 }]);
    mockStreamTurn.mockResolvedValue(sseResponse(['event: final_answer\ndata: {"answer":"ok"}\n\n']));
    renderWorkspaceChat();
    await acceptConsent(user);
    await choosePreset(user, "Onboarding");

    const chip = within(await chipsGroup()).getAllByRole("button")[1]!;
    const text = chip.textContent!;
    await user.click(chip);
    await waitFor(() => expect(mockStreamTurn).toHaveBeenCalledTimes(1));
    expect(mockStreamTurn).toHaveBeenCalledWith(WORKSPACE_CHAT_ROUTES, text, null, "onboarding", expect.anything());
    expect(await screen.findByRole("button", { name: "workspace chip" })).toHaveAttribute("aria-current");
    expect(readStoredPresets("workspace").get(3)).toBe("onboarding");
    expect(readStoredPresets("single").size).toBe(0);
  });
});
