import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// The prompt-only presets (S-625, FR-UI-49) end to end in the SPA: the composer's "+"
// menu, the clearable chip, the per-conversation-and-scope memory, and that every
// send path carries the open conversation's preset. The network module is mocked as
// `ChatView.test.tsx` mocks it, so the real runtime, rail and composer run.
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
  renameChatThread: vi.fn(),
  fetchThreads: vi.fn(),
  fetchThreadMessages: vi.fn(),
}));
vi.mock("../wiki/mermaid.ts", () => ({
  renderMermaidIn: vi.fn(() => Promise.resolve()),
  unadoptMermaidStyleFor: vi.fn(),
  VENDORED_MERMAID_URL: "/assets/vendor/mermaid.min.js",
}));
import {
  deleteChatThread,
  fetchChatConfig,
  fetchThreadMessages,
  fetchThreads,
  streamChatTurn,
  truncateChatThread,
} from "../../api/chatClient.ts";
import { chatStorageKey, type ChatConfigReadModel, type PersistedChatMessage, type ThreadSummary } from "./chatModel.ts";
import { ChatView } from "./ChatView.tsx";
import {
  CHAT_PRESETS,
  isChatPresetId,
  PRESETS_KEY,
  readStoredPresets,
  storePresets,
  withPreset,
  type ChatPresetId,
} from "./presets.ts";

const mockFetchConfig = vi.mocked(fetchChatConfig);
const mockFetchThreads = vi.mocked(fetchThreads);
const mockFetchMessages = vi.mocked(fetchThreadMessages);
const mockStreamTurn = vi.mocked(streamChatTurn);
const mockDeleteThread = vi.mocked(deleteChatThread);
const mockTruncate = vi.mocked(truncateChatThread);

function thread(id: number, title: string, updatedAt: number): ThreadSummary {
  return { id, title, updated_at: updatedAt };
}

function persisted(role: PersistedChatMessage["role"], content: string, id: number): PersistedChatMessage {
  return { id, role, content, created_at: 0, tool_traces: [] };
}

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
      credential: { present: true },
      credential_origin: "member",
      member_key_withheld: false,
    },
  };
}

/** A fresh, closed SSE response answering `answer`. */
function answered(answer: string): Response {
  const enc = new TextEncoder();
  const body = new ReadableStream<Uint8Array>({
    start(controller) {
      controller.enqueue(enc.encode(`event: final_answer\ndata: {"answer":"${answer}"}\n\n`));
      controller.close();
    },
  });
  return { ok: true, status: 200, body } as unknown as Response;
}

/** The member chat's storage key for the presets (the plain ChatView is `single`). */
const SINGLE_PRESETS = chatStorageKey(PRESETS_KEY, "single");

beforeEach(() => {
  window.localStorage.clear();
  vi.clearAllMocks();
  mockFetchConfig.mockResolvedValue(configuredModel());
  mockFetchThreads.mockResolvedValue([]);
  mockFetchMessages.mockResolvedValue([]);
  mockStreamTurn.mockImplementation(async () => answered("ok"));
  mockTruncate.mockResolvedValue({ ok: true, status: 204 } as unknown as Response);
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

type User = ReturnType<typeof userEvent.setup>;

async function acceptConsent(user: User) {
  await user.click(await screen.findByRole("button", { name: "Start chatting" }));
}

async function ask(user: User, question: string) {
  await user.type(screen.getByRole("textbox", { name: "Your message" }), question);
  await user.click(screen.getByRole("button", { name: "Send" }));
}

function plusButton() {
  return screen.getByRole("button", { name: "Composer options" });
}

async function choose(user: User, label: string) {
  await user.click(plusButton());
  await user.click(within(screen.getByRole("menu", { name: "Preset" })).getByRole("menuitemradio", { name: label }));
}

/** The chip's clear control for `label`, or null when no chip shows. */
function clearChip(label: string) {
  return screen.queryByRole("button", { name: `Clear the ${label} preset` });
}

/** The preset each streamed turn was sent under, in send order. */
function sentPresets(): ChatPresetId[] {
  return mockStreamTurn.mock.calls.map((call) => call[3]);
}

describe("presets.ts — the four presets and their storage (S-625)", () => {
  it("lists exactly the server's four wire ids, Default first, each with a one-line description", () => {
    expect(CHAT_PRESETS.map((p) => p.id)).toEqual(["default", "impact_analysis", "onboarding", "troubleshooting"]);
    expect(CHAT_PRESETS.map((p) => p.label)).toEqual(["Default", "Impact analysis", "Onboarding", "Troubleshooting"]);
    for (const p of CHAT_PRESETS) {
      expect(p.description.length).toBeGreaterThan(0);
      expect(p.description).not.toContain("\n");
    }
  });

  it("recognises a wire id only exactly", () => {
    for (const p of CHAT_PRESETS) expect(isChatPresetId(p.id)).toBe(true);
    for (const near of ["Default", "impact-analysis", " onboarding", "troubleshoot", "", null, 3]) {
      expect(isChatPresetId(near)).toBe(false);
    }
  });

  it("keeps only well-formed entries: an integer id naming a non-Default preset", () => {
    window.localStorage.setItem(
      SINGLE_PRESETS,
      JSON.stringify({
        "7": "onboarding",
        "8": "default",
        "9": "custom",
        "1.5": "onboarding",
        "07": "onboarding",
        x: "troubleshooting",
        "10": "troubleshooting",
      }),
    );
    expect([...readStoredPresets("single")]).toEqual([
      [7, "onboarding"],
      [10, "troubleshooting"],
    ]);
    for (const junk of ["not json", "[1,2]", "null", '"onboarding"']) {
      window.localStorage.setItem(SINGLE_PRESETS, junk);
      expect(readStoredPresets("single").size).toBe(0);
    }
  });

  it("stores per scope, drops a Default choice and clears the key when nothing is steered", () => {
    let presets = withPreset(new Map(), 4, "impact_analysis");
    storePresets("single", presets);
    expect(readStoredPresets("single").get(4)).toBe("impact_analysis");
    expect(readStoredPresets("workspace").size).toBe(0);
    presets = withPreset(presets, 4, "default");
    expect(presets.size).toBe(0);
    storePresets("single", presets);
    expect(window.localStorage.getItem(SINGLE_PRESETS)).toBeNull();
  });
});

describe("ChatView — the + preset menu (S-625, FR-UI-49)", () => {
  it("waits for consent, then opens a menu of the four presets with Default checked", async () => {
    const user = userEvent.setup();
    render(<ChatView />);
    await screen.findByRole("button", { name: "Start chatting" });
    expect(plusButton()).toBeDisabled();
    await acceptConsent(user);

    await user.click(plusButton());
    expect(plusButton()).toHaveAttribute("aria-expanded", "true");
    const menu = screen.getByRole("menu", { name: "Preset" });
    expect(plusButton()).toHaveAttribute("aria-controls", menu.id);
    const items = within(menu).getAllByRole("menuitemradio");
    expect(items).toHaveLength(4);
    for (const [i, p] of CHAT_PRESETS.entries()) {
      expect(items[i]).toHaveAccessibleName(p.label);
      expect(items[i]).toHaveAccessibleDescription(p.description);
      expect(items[i]).toHaveAttribute("aria-checked", String(p.id === "default"));
    }
    // It opens on the checked item.
    expect(items[0]).toHaveFocus();
  });

  it("shows a chosen non-Default preset as a chip, sends under it, and clearing returns to Default", async () => {
    const user = userEvent.setup();
    render(<ChatView />);
    await acceptConsent(user);
    expect(clearChip("Impact analysis")).toBeNull();

    await choose(user, "Impact analysis");
    expect(screen.queryByRole("menu")).toBeNull();
    expect(plusButton()).toHaveFocus();
    expect(clearChip("Impact analysis")).toBeInTheDocument();
    expect(clearChip("Impact analysis")!.closest("form")).toBe(
      screen.getByRole("textbox", { name: "Your message" }).closest("form"),
    );
    await ask(user, "what breaks if I change alpha?");
    await waitFor(() => expect(sentPresets()).toEqual(["impact_analysis"]));

    await user.click(clearChip("Impact analysis")!);
    expect(clearChip("Impact analysis")).toBeNull();
    expect(screen.getByRole("textbox", { name: "Your message" })).toHaveFocus();
    await user.click(plusButton());
    expect(screen.getByRole("menuitemradio", { name: "Default" })).toHaveAttribute("aria-checked", "true");
    await user.keyboard("{Escape}");
    await ask(user, "and now?");
    await waitFor(() => expect(sentPresets()).toEqual(["impact_analysis", "default"]));
  });

  it("moves with the arrow keys, Home and End, picks with Enter, and Esc closes it and goes no further", async () => {
    const user = userEvent.setup();
    render(<ChatView />);
    await acceptConsent(user);
    await user.click(plusButton());
    const item = (label: string) => screen.getByRole("menuitemradio", { name: label });
    await user.keyboard("{ArrowDown}");
    expect(item("Impact analysis")).toHaveFocus();
    await user.keyboard("{ArrowUp}{ArrowUp}");
    expect(item("Troubleshooting")).toHaveFocus(); // wraps
    await user.keyboard("{Home}");
    expect(item("Default")).toHaveFocus();
    await user.keyboard("{End}{Enter}");
    expect(clearChip("Troubleshooting")).toBeInTheDocument();

    // Esc is the menu's: it stops at the menu, so the chat's document-level
    // Esc-stops-the-turn shortcut (S-620) never sees it.
    const seen: string[] = [];
    const listener = (e: KeyboardEvent) => seen.push(e.key);
    document.addEventListener("keydown", listener);
    await user.click(plusButton());
    await user.keyboard("{Escape}");
    document.removeEventListener("keydown", listener);
    expect(screen.queryByRole("menu")).toBeNull();
    expect(plusButton()).toHaveFocus();
    expect(seen).toEqual([]);
  });

  it("opens on the conversation's own preset, and Tab or the + again closes it", async () => {
    const user = userEvent.setup();
    render(<ChatView />);
    await acceptConsent(user);
    await choose(user, "Troubleshooting");
    await user.click(plusButton());
    expect(screen.getByRole("menuitemradio", { name: "Troubleshooting" })).toHaveFocus();

    await user.tab();
    expect(screen.queryByRole("menu")).toBeNull();
    expect(plusButton()).toHaveAttribute("aria-expanded", "false");

    await user.click(plusButton());
    expect(screen.getByRole("menu", { name: "Preset" })).toBeInTheDocument();
    await user.click(plusButton());
    expect(screen.queryByRole("menu")).toBeNull();
    expect(plusButton()).toHaveAttribute("aria-expanded", "false");
  });

  it("closes on a pointer down outside it, keeping the conversation's preset", async () => {
    const user = userEvent.setup();
    render(<ChatView />);
    await acceptConsent(user);
    await user.click(plusButton());
    await user.click(screen.getByRole("textbox", { name: "Your message" }));
    expect(screen.queryByRole("menu")).toBeNull();
    expect(plusButton()).toHaveAttribute("aria-expanded", "false");
  });
});

describe("ChatView — a preset is the conversation's, per scope (S-625, FR-UI-49)", () => {
  /** Two conversations; each restores its own transcript. */
  function twoConversations() {
    mockFetchThreads.mockResolvedValue([thread(9, "Conv A", 500), thread(10, "Conv B", 400)]);
    mockFetchMessages.mockImplementation(async (_routes, id) => [
      persisted("user", `question ${id}`, 1),
      persisted("assistant", `answer ${id}`, 2),
    ]);
  }

  it("survives a reload per conversation, while another conversation keeps its own", async () => {
    const user = userEvent.setup();
    twoConversations();
    const view = render(<ChatView />);
    await acceptConsent(user);
    await user.click(await screen.findByRole("button", { name: "Conv A" }));
    await screen.findByText("answer 9");
    await choose(user, "Onboarding");
    expect(clearChip("Onboarding")).toBeInTheDocument();

    // The other conversation is Default, and switching back restores Onboarding.
    await user.click(screen.getByRole("button", { name: "Conv B" }));
    await screen.findByText("answer 10");
    expect(clearChip("Onboarding")).toBeNull();
    await choose(user, "Troubleshooting");
    await user.click(screen.getByRole("button", { name: "Conv A" }));
    await screen.findByText("answer 9");
    expect(clearChip("Onboarding")).toBeInTheDocument();

    // Reload: the last-open conversation (A) comes back under its own preset…
    view.unmount();
    render(<ChatView />);
    await screen.findByText("answer 9");
    expect(clearChip("Onboarding")).toBeInTheDocument();
    await ask(user, "where do I start?");
    await waitFor(() => expect(sentPresets().at(-1)).toBe("onboarding"));
    // …and B still has its own.
    await user.click(screen.getByRole("button", { name: "Conv B" }));
    await screen.findByText("answer 10");
    expect(clearChip("Troubleshooting")).toBeInTheDocument();
    await ask(user, "why does it fail?");
    await waitFor(() => expect(sentPresets().at(-1)).toBe("troubleshooting"));
  });

  it("is keyed by the chat's scope: another scope's choice for the same id does not apply", async () => {
    const user = userEvent.setup();
    twoConversations();
    window.localStorage.setItem(chatStorageKey(PRESETS_KEY, "workspace"), JSON.stringify({ "9": "onboarding" }));
    window.localStorage.setItem(chatStorageKey(PRESETS_KEY, "member:api"), JSON.stringify({ "9": "onboarding" }));
    render(<ChatView />);
    await acceptConsent(user);
    await user.click(await screen.findByRole("button", { name: "Conv A" }));
    await screen.findByText("answer 9");
    expect(clearChip("Onboarding")).toBeNull();
    await ask(user, "q");
    await waitFor(() => expect(sentPresets()).toEqual(["default"]));
  });

  it("binds a new conversation's choice to the thread its first send creates; + New chat starts at Default", async () => {
    const user = userEvent.setup();
    mockFetchThreads.mockReset();
    mockFetchThreads.mockResolvedValueOnce([]); // mount
    mockFetchThreads.mockResolvedValue([thread(11, "first question", 600)]); // after the send
    mockFetchMessages.mockResolvedValue([persisted("user", "first question", 1), persisted("assistant", "ok", 2)]);
    const view = render(<ChatView />);
    await acceptConsent(user);
    await choose(user, "Impact analysis");
    await ask(user, "first question");
    expect(await screen.findByRole("button", { name: "first question" })).toHaveAttribute("aria-current");
    expect(sentPresets()).toEqual(["impact_analysis"]);
    expect(readStoredPresets("single").get(11)).toBe("impact_analysis");
    expect(clearChip("Impact analysis")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /New chat/ }));
    expect(clearChip("Impact analysis")).toBeNull();

    view.unmount();
    render(<ChatView />);
    await user.click(await screen.findByRole("button", { name: "first question" }));
    await screen.findByText("ok");
    expect(clearChip("Impact analysis")).toBeInTheDocument();
  });

  it("does not hand a deleted conversation's preset to a new one that reuses its id", async () => {
    const user = userEvent.setup();
    // Conversation 11 was steered, then deleted; the store hands id 11 to the next one.
    window.localStorage.setItem(SINGLE_PRESETS, JSON.stringify({ "11": "troubleshooting" }));
    mockFetchThreads.mockReset();
    mockFetchThreads.mockResolvedValueOnce([]); // mount
    mockFetchThreads.mockResolvedValue([thread(11, "a new question", 700)]); // after the send
    render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "a new question");
    expect(await screen.findByRole("button", { name: "a new question" })).toHaveAttribute("aria-current");
    expect(clearChip("Troubleshooting")).toBeNull();
    expect(readStoredPresets("single").has(11)).toBe(false);
    await ask(user, "and a follow-up");
    await waitFor(() => expect(sentPresets()).toEqual(["default", "default"]));
  });

  it("forgets a deleted conversation's preset, so an unadopted thread reusing its id reads as Default", async () => {
    const user = userEvent.setup();
    mockDeleteThread.mockResolvedValue({ ok: true, status: 204 } as unknown as Response);
    mockFetchThreads.mockReset();
    mockFetchThreads.mockResolvedValueOnce([thread(11, "Old", 500)]); // mount
    mockFetchThreads.mockResolvedValueOnce([]); // after the delete
    mockFetchThreads.mockRejectedValueOnce(new Error("offline")); // the first send's reconcile
    mockFetchThreads.mockResolvedValue([thread(11, "fresh question", 700)]); // after the reload
    mockFetchMessages.mockImplementation(async (_routes, id) => [
      persisted("user", `question ${id}`, 1),
      persisted("assistant", `answer ${id}`, 2),
    ]);
    const view = render(<ChatView />);
    await acceptConsent(user);
    await user.click(await screen.findByRole("button", { name: "Old" }));
    await screen.findByText("answer 11");
    await choose(user, "Troubleshooting");
    await user.click(screen.getByRole("button", { name: "Delete conversation “Old”" }));
    await user.click(await screen.findByRole("button", { name: "Delete" }));
    await waitFor(() => expect(screen.queryByRole("button", { name: "Old" })).toBeNull());
    expect(readStoredPresets("single").has(11)).toBe(false);

    // The store hands id 11 to the next conversation, whose rail refresh fails, so
    // it is never adopted ("reopen it from the rail").
    await ask(user, "fresh question");
    await screen.findByText(/history list could not refresh/);
    view.unmount();
    render(<ChatView />);
    await user.click(await screen.findByRole("button", { name: "fresh question" }));
    await screen.findByText("answer 11");
    expect(clearChip("Troubleshooting")).toBeNull();
    await ask(user, "a follow-up");
    await waitFor(() => expect(sentPresets()).toEqual(["default", "default"]));
  });

  it("changes only its own entry, never another tab's remembered preset", async () => {
    const user = userEvent.setup();
    twoConversations();
    render(<ChatView />);
    await acceptConsent(user);
    await user.click(await screen.findByRole("button", { name: "Conv A" }));
    await screen.findByText("answer 9");
    // Another tab of the same chat steers conversation 10 after this one mounted.
    storePresets("single", new Map([[10, "troubleshooting"]]));
    await choose(user, "Onboarding");
    expect(Object.fromEntries(readStoredPresets("single"))).toEqual({ 9: "onboarding", 10: "troubleshooting" });
  });

  it("drops an unsent conversation's pick on + New chat: the new chat starts at Default", async () => {
    const user = userEvent.setup();
    render(<ChatView />);
    await acceptConsent(user);
    await choose(user, "Onboarding");
    expect(clearChip("Onboarding")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /New chat/ }));
    expect(clearChip("Onboarding")).toBeNull();
    await ask(user, "hello");
    await waitFor(() => expect(sentPresets()).toEqual(["default"]));
  });

  // Regenerate and the stalled-turn Retry (S-626) are the one `onReload` path.
  it("re-runs Regenerate (the onReload path Retry shares) under the conversation's preset", async () => {
    const user = userEvent.setup();
    twoConversations();
    render(<ChatView />);
    await acceptConsent(user);
    await user.click(await screen.findByRole("button", { name: "Conv A" }));
    await screen.findByText("answer 9");
    await choose(user, "Troubleshooting");
    await user.click(screen.getByRole("button", { name: "Regenerate answer" }));
    await waitFor(() => expect(mockStreamTurn).toHaveBeenCalledTimes(1));
    expect(mockStreamTurn.mock.calls[0][1]).toBe("question 9");
    expect(mockStreamTurn.mock.calls[0][2]).toBe(9);
    expect(sentPresets()).toEqual(["troubleshooting"]);
  });

  // The stalled-turn Retry (S-626) is a re-run too: it must carry the preset the
  // conversation holds when Retry is pressed, not the one its stalled send went under.
  it("re-runs a stalled turn's Retry under the preset chosen after the stall, cutting once", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
      twoConversations();
      render(<ChatView />);
      await acceptConsent(user);
      await user.click(await screen.findByRole("button", { name: "Conv A" }));
      await screen.findByText("answer 9");
      await choose(user, "Impact analysis");

      // A stream that opens and then goes silent: no byte after the plan.
      const enc = new TextEncoder();
      let close: () => void = () => {};
      const body = new ReadableStream<Uint8Array>({
        start(controller) {
          controller.enqueue(enc.encode('event: plan\ndata: {"round":0,"steps":[]}\n\n'));
          close = () => controller.close();
        },
      });
      mockStreamTurn.mockResolvedValueOnce({ ok: true, status: 200, body } as unknown as Response);
      await ask(user, "slow one");
      await vi.advanceTimersByTimeAsync(45_000);
      const notice = screen.getByRole("alert");

      await choose(user, "Troubleshooting");
      mockStreamTurn.mockResolvedValueOnce(answered("back"));
      await user.click(within(notice).getByRole("button", { name: "Retry" }));
      expect(await screen.findByText("back")).toBeInTheDocument();
      expect(sentPresets()).toEqual(["impact_analysis", "troubleshooting"]);
      expect(mockStreamTurn.mock.calls[1].slice(1, 3)).toEqual(["slow one", 9]);
      // The stalled question is the store's second: cut there once, never duplicated.
      expect(mockTruncate).toHaveBeenCalledTimes(1);
      expect(mockTruncate).toHaveBeenCalledWith(expect.anything(), 9, 1);
      close();
    } finally {
      vi.useRealTimers();
    }
  });

  // S-624: an edited question is resent down the same `runTurn` every send takes, so it
  // runs under the conversation's preset — not the Default an ad-hoc send would carry.
  it("resends an edited question under the conversation's non-Default preset", async () => {
    const user = userEvent.setup();
    twoConversations();
    render(<ChatView />);
    await acceptConsent(user);
    await user.click(await screen.findByRole("button", { name: "Conv A" }));
    await screen.findByText("answer 9");
    await choose(user, "Impact analysis");

    await user.click(screen.getByRole("button", { name: "Edit message" }));
    const field = screen.getByRole("textbox", { name: "Edit your message" });
    await user.clear(field);
    await user.type(field, "question 9, sharper");
    await user.click(screen.getByRole("button", { name: "Resend" }));

    await waitFor(() => expect(mockStreamTurn).toHaveBeenCalledTimes(1));
    expect(mockTruncate).toHaveBeenCalledWith(expect.anything(), 9, 0);
    expect(mockStreamTurn.mock.calls[0][1]).toBe("question 9, sharper");
    expect(mockStreamTurn.mock.calls[0][2]).toBe(9);
    expect(sentPresets()).toEqual(["impact_analysis"]);
  });
});
