import { act, cleanup, render, renderHook, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// The conversation title (S-623, FR-UI-48) end to end in the SPA: the runtime hands
// a streamed `thread_title` to the rail, and the rail's rename goes through the
// intent-guarded client for whichever chat it serves. The network module is mocked
// as `ChatView.test.tsx` mocks it, so the real reader, reducer and runtime run.
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
  fetchChatConfig,
  fetchThreadMessages,
  fetchThreads,
  MEMBER_CHAT_ROUTES,
  renameChatThread,
  streamChatTurn,
  WORKSPACE_CHAT_ROUTES,
  type ChatRoutes,
} from "../../api/chatClient.ts";
import type { ChatConfigReadModel, ThreadSummary } from "./chatModel.ts";
import { ChatView } from "./ChatView.tsx";
import { useThreadRail } from "./useThreadRail.ts";

const mockFetchConfig = vi.mocked(fetchChatConfig);
const mockFetchThreads = vi.mocked(fetchThreads);
const mockFetchMessages = vi.mocked(fetchThreadMessages);
const mockStreamTurn = vi.mocked(streamChatTurn);
const mockRename = vi.mocked(renameChatThread);

function thread(id: number, title: string, updatedAt: number): ThreadSummary {
  return { id, title, updated_at: updatedAt };
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

/** An SSE response the test writes into and closes by hand. */
function openSse(): { response: Response; push: (chunk: string) => void; close: () => void } {
  const enc = new TextEncoder();
  let ctrl: ReadableStreamDefaultController<Uint8Array> | null = null;
  const body = new ReadableStream<Uint8Array>({
    start(controller) {
      ctrl = controller;
    },
  });
  return {
    response: { ok: true, status: 200, body } as unknown as Response,
    push: (chunk) => ctrl?.enqueue(enc.encode(chunk)),
    close: () => ctrl?.close(),
  };
}

function status(code: number): Response {
  return { ok: code >= 200 && code < 300, status: code } as unknown as Response;
}

beforeEach(() => {
  window.localStorage.clear();
  vi.clearAllMocks();
  mockFetchConfig.mockResolvedValue(configuredModel());
  mockFetchMessages.mockResolvedValue([]);
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("the runtime applies thread_title to the rail", () => {
  it("titles the new conversation while its answer is still streaming, then adopts it", async () => {
    const user = userEvent.setup();
    mockFetchThreads.mockReset();
    mockFetchThreads.mockResolvedValueOnce([thread(3, "An older conversation", 100)]); // mount
    mockFetchThreads.mockResolvedValue([
      thread(9, "Risk in alpha", 400),
      thread(3, "An older conversation", 100),
    ]); // the trailing re-read
    const sse = openSse();
    mockStreamTurn.mockResolvedValueOnce(sse.response);
    render(<ChatView />);
    await user.click(await screen.findByRole("button", { name: "Start chatting" }));
    await user.type(screen.getByRole("textbox", { name: "Your message" }), "what is risky here?");
    await user.click(screen.getByRole("button", { name: "Send" }));

    // The wire order (FR-UI-19): the answer, then its final record, then the title.
    sse.push('event: answer_delta\ndata: {"delta":"The risky part is alpha."}\n\n');
    sse.push('event: final_answer\ndata: {"answer":"The risky part is alpha."}\n\n');
    sse.push('event: thread_title\ndata: {"event":"thread_title","thread_id":9,"title":"Risk in alpha"}\n\n');
    // The stream is still open — no re-read has run — and the rail already names it.
    const rail = screen.getByRole("navigation", { name: "Conversations" });
    expect(await within(rail).findByRole("button", { name: "Risk in alpha" })).toBeInTheDocument();
    expect(mockFetchThreads).toHaveBeenCalledTimes(1);

    await act(async () => {
      sse.close();
    });
    // The re-read adopts the conversation the send created: it is now the open one.
    await waitFor(() =>
      expect(within(rail).getByRole("button", { name: "Risk in alpha" })).toHaveAttribute(
        "aria-current",
        "true",
      ),
    );
    expect(within(rail).getAllByRole("button", { name: "Risk in alpha" })).toHaveLength(1);
    expect(screen.queryByText(/TITLE:/)).not.toBeInTheDocument();
  });

  it("a malformed thread_title frame is dropped, never guessed at", async () => {
    const user = userEvent.setup();
    mockFetchThreads.mockReset();
    mockFetchThreads.mockResolvedValue([]);
    const sse = openSse();
    mockStreamTurn.mockResolvedValueOnce(sse.response);
    render(<ChatView />);
    await user.click(await screen.findByRole("button", { name: "Start chatting" }));
    await user.type(screen.getByRole("textbox", { name: "Your message" }), "q");
    await user.click(screen.getByRole("button", { name: "Send" }));
    sse.push('event: thread_title\ndata: {"thread_id":"9","title":"Not a number id"}\n\n');
    sse.push('event: thread_title\ndata: {"thread_id":9,"title":"   "}\n\n');
    // A delta after them: once it renders, both titles have been read. Assert while
    // the stream is open — the trailing re-read would repaint the rail anyway.
    sse.push('event: answer_delta\ndata: {"delta":"still streaming"}\n\n');
    expect(await screen.findByText("still streaming")).toBeInTheDocument();
    const rail = screen.getByRole("navigation", { name: "Conversations" });
    expect(within(rail).queryByRole("button", { name: "Not a number id" })).not.toBeInTheDocument();
    expect(within(rail).queryAllByRole("listitem")).toHaveLength(0);
    await act(async () => {
      sse.close();
    });
  });
});

describe("renaming the open conversation never interrupts it", () => {
  it("a double-click on the open row while its turn streams opens the field and leaves the turn running", async () => {
    const user = userEvent.setup();
    mockFetchThreads.mockReset();
    mockFetchThreads.mockResolvedValue([thread(7, "Open conversation", 300)]);
    mockFetchMessages.mockResolvedValue([]);
    window.localStorage.setItem("logos.chat.activeThread:single", "7");
    const sse = openSse();
    mockStreamTurn.mockResolvedValueOnce(sse.response);
    render(<ChatView />);
    await user.click(await screen.findByRole("button", { name: "Start chatting" }));
    await waitFor(() => expect(mockFetchMessages).toHaveBeenCalledTimes(1)); // the mount restore
    await user.type(screen.getByRole("textbox", { name: "Your message" }), "a follow-up");
    await user.click(screen.getByRole("button", { name: "Send" }));
    sse.push('event: answer_delta\ndata: {"delta":"streaming on"}\n\n');
    expect(await screen.findByText("streaming on")).toBeInTheDocument();

    const rail = screen.getByRole("navigation", { name: "Conversations" });
    await user.dblClick(within(rail).getByRole("button", { name: "Open conversation" }));
    expect(
      within(rail).getByRole("textbox", { name: "Rename conversation “Open conversation”" }),
    ).toBeInTheDocument();
    const signal = mockStreamTurn.mock.calls[0][4] as AbortSignal;
    expect(signal.aborted, "the running turn was not aborted").toBe(false);
    expect(mockFetchMessages, "and the transcript was not re-fetched").toHaveBeenCalledTimes(1);
    expect(screen.getByRole("button", { name: "Stop" })).toBeInTheDocument();
    await act(async () => {
      sse.close();
    });
  });
});

describe("the rail renames through the chat it serves", () => {
  /** The rail hook over `routes`, loaded with one conversation. */
  async function railOver(routes: ChatRoutes) {
    mockFetchThreads.mockReset();
    mockFetchThreads.mockResolvedValueOnce([thread(5, "Old name", 200)]);
    const hook = renderHook(() =>
      useThreadRail({
        routes,
        activeThreadIdRef: { current: null },
        adoptThread: vi.fn(),
        resetSurface: vi.fn(),
      }),
    );
    await act(() => hook.result.current.loadThreads());
    return hook;
  }

  for (const [name, routes] of [
    ["member", MEMBER_CHAT_ROUTES],
    ["workspace", WORKSPACE_CHAT_ROUTES],
  ] as const) {
    it(`a ${name} rename is sent to its own chat and the row takes the stored name`, async () => {
      const { result } = await railOver(routes);
      mockRename.mockResolvedValueOnce(status(204));
      mockFetchThreads.mockResolvedValueOnce([thread(5, "Binder map", 200)]);
      await act(() => result.current.renameThread(5, "Binder map"));
      expect(mockRename).toHaveBeenCalledWith(routes, 5, "Binder map");
      expect(result.current.threads).toEqual([thread(5, "Binder map", 200)]);
      expect(result.current.threadsError).toBeNull();
    });
  }

  it("a refused rename keeps the title and says so", async () => {
    const { result } = await railOver(MEMBER_CHAT_ROUTES);
    mockRename.mockResolvedValueOnce(status(400));
    await act(() => result.current.renameThread(5, "x"));
    expect(result.current.threads).toEqual([thread(5, "Old name", 200)]);
    expect(result.current.threadsError).toBe("Could not rename that conversation (status 400).");
  });

  /** A list read the test resolves by hand. */
  function deferredList() {
    let resolve: (list: ThreadSummary[]) => void = () => {};
    const promise = new Promise<ThreadSummary[]>((r) => (resolve = r));
    return { promise, resolve };
  }

  it("the row takes the new name at once, before the re-read answers", async () => {
    const { result } = await railOver(MEMBER_CHAT_ROUTES);
    mockRename.mockResolvedValueOnce(status(204));
    const reread = deferredList();
    mockFetchThreads.mockReturnValueOnce(reread.promise);
    let renaming: Promise<void> = Promise.resolve();
    await act(async () => {
      renaming = result.current.renameThread(5, "Binder map");
      await Promise.resolve();
    });
    await waitFor(() => expect(result.current.threads[0].title).toBe("Binder map"));
    await act(async () => {
      reread.resolve([thread(5, "Binder map", 200)]);
      await renaming;
    });
  });

  it("a list read already in flight cannot paint the old title back over a rename", async () => {
    const { result } = await railOver(MEMBER_CHAT_ROUTES);
    const stale = deferredList();
    mockFetchThreads.mockReturnValueOnce(stale.promise); // started before the rename
    const reread = deferredList();
    mockFetchThreads.mockReturnValueOnce(reread.promise); // the rename's own re-read
    mockRename.mockResolvedValueOnce(status(204));
    let staleRead: Promise<void> = Promise.resolve();
    let renaming: Promise<void> = Promise.resolve();
    await act(async () => {
      staleRead = result.current.loadThreads();
      renaming = result.current.renameThread(5, "Binder map");
      await Promise.resolve();
    });
    await act(async () => {
      stale.resolve([thread(5, "Old name", 200)]);
      await staleRead;
    });
    expect(result.current.threads[0].title, "the stale read was superseded").toBe("Binder map");
    await act(async () => {
      reread.resolve([thread(5, "Binder map", 200)]);
      await renaming;
    });
  });

  it("a list read already in flight cannot drop a row a streamed title added", async () => {
    const { result } = await railOver(MEMBER_CHAT_ROUTES);
    const stale = deferredList();
    mockFetchThreads.mockReturnValueOnce(stale.promise);
    let staleRead: Promise<void> = Promise.resolve();
    act(() => {
      staleRead = result.current.loadThreads();
    });
    act(() => result.current.applyTitle(9, "Brand new"));
    await act(async () => {
      stale.resolve([thread(5, "Old name", 200)]);
      await staleRead;
    });
    expect(result.current.threads.map((t) => t.id)).toEqual([9, 5]);
  });

  it("renaming a conversation that is gone drops its row and says so", async () => {
    const { result } = await railOver(MEMBER_CHAT_ROUTES);
    mockRename.mockResolvedValueOnce(status(404));
    mockFetchThreads.mockResolvedValueOnce([]);
    await act(() => result.current.renameThread(5, "Too late"));
    expect(mockFetchThreads).toHaveBeenCalledTimes(2);
    expect(result.current.threads).toEqual([]);
    expect(result.current.threadsError).toBe("That conversation no longer exists.");
  });

  it("an intent-less (403) or failed rename keeps the title too", async () => {
    const { result } = await railOver(MEMBER_CHAT_ROUTES);
    mockRename.mockResolvedValueOnce(status(403));
    await act(() => result.current.renameThread(5, "Forged"));
    expect(result.current.threads[0].title).toBe("Old name");
    mockRename.mockRejectedValueOnce(new Error("offline"));
    await act(() => result.current.renameThread(5, "Offline"));
    expect(result.current.threads[0].title).toBe("Old name");
    expect(result.current.threadsError).toBe("Could not rename that conversation: offline");
  });

  it("applyTitle retitles a listed row in place and adds a missing one at the top", async () => {
    const { result } = await railOver(MEMBER_CHAT_ROUTES);
    act(() => result.current.applyTitle(5, "Retitled"));
    expect(result.current.threads.map((t) => [t.id, t.title])).toEqual([[5, "Retitled"]]);
    act(() => result.current.applyTitle(9, "Brand new"));
    expect(result.current.threads.map((t) => [t.id, t.title])).toEqual([
      [9, "Brand new"],
      [5, "Retitled"],
    ]);
  });
});
