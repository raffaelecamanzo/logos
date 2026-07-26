import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// Mock the network module so the surface is driven by hand-built SSE streams and
// config models — the SSE contract is exercised through the real `chatModel`
// reader/reducer the runtime adapter imports. assistant-ui supplies the thread,
// composer, and copy/stop/regenerate affordances over that adapter.
vi.mock("../../api/chatClient.ts", () => ({
  CHAT_ROUTE: "/chat",
  CHAT_THREADS_ROUTE: "/api/v1/chat/threads",
  fetchChatConfig: vi.fn(),
  streamChatTurn: vi.fn(),
  deleteChatThread: vi.fn(),
  fetchThreads: vi.fn(),
  fetchThreadMessages: vi.fn(),
}));

import {
  deleteChatThread,
  fetchChatConfig,
  fetchThreadMessages,
  fetchThreads,
  streamChatTurn,
} from "../../api/chatClient.ts";
import { ApiError } from "../../api/client.ts";
import type { ChatConfigReadModel, PersistedChatMessage, ThreadSummary } from "./chatModel.ts";
import { ChatView } from "./ChatView.tsx";

const mockFetchConfig = vi.mocked(fetchChatConfig);
const mockStreamTurn = vi.mocked(streamChatTurn);
const mockDeleteThread = vi.mocked(deleteChatThread);
const mockFetchThreads = vi.mocked(fetchThreads);
const mockFetchMessages = vi.mocked(fetchThreadMessages);

/** The rail's delete affordance for a conversation (named by its title so a
 *  multi-row list can never be mis-targeted). */
function deleteButton(title: string) {
  return screen.getByRole("button", { name: `Delete conversation “${title}”` });
}

/** A thread-list summary over the S-209 wire shape. */
function thread(id: number, title: string, updatedAt: number): ThreadSummary {
  return { id, title, updated_at: updatedAt };
}
/** A persisted transcript row over the S-209 messages wire shape. */
function persisted(role: PersistedChatMessage["role"], content: string, id = 1): PersistedChatMessage {
  return { id, role, content, created_at: 0, tool_traces: [] };
}

/** A configured read-model carrying a MASKED key whose last-4 must NEVER render. */
const MASKED_LAST4 = "SECRET4";
function configuredModel(provider: "anthropic" | "openai" = "openai"): ChatConfigReadModel {
  return {
    config: {
      parsed: {
        chat: {
          provider,
          model: "openrouter/some-model",
          base_url: "https://openrouter.ai/api/v1",
          max_tool_calls: 24,
          max_subagent_tool_calls: 8,
          max_replans: 3,
        },
      },
    },
    chat_key: { present: true, last4: MASKED_LAST4 },
  };
}

function unconfiguredModel(): ChatConfigReadModel {
  const m = configuredModel();
  return { ...m, chat_key: { present: false, last4: null } };
}

/** A streamed SSE Response from wire chunks (real ReadableStream body). */
function sseResponse(chunks: string[], status = 200): Response {
  const enc = new TextEncoder();
  const body = new ReadableStream<Uint8Array>({
    start(controller) {
      for (const c of chunks) controller.enqueue(enc.encode(c));
      controller.close();
    },
  });
  return { ok: status >= 200 && status < 300, status, body } as unknown as Response;
}

/** A never-closing SSE Response — used to assert the in-flight (running) state and
 *  the Stop control. The returned `cancel` lets a test close it deterministically. */
function pendingSseResponse(initialChunks: string[]): { response: Response; close: () => void } {
  const enc = new TextEncoder();
  let ctrl: ReadableStreamDefaultController<Uint8Array> | null = null;
  const body = new ReadableStream<Uint8Array>({
    start(controller) {
      ctrl = controller;
      for (const c of initialChunks) controller.enqueue(enc.encode(c));
    },
  });
  return {
    response: { ok: true, status: 200, body } as unknown as Response,
    close: () => ctrl?.close(),
  };
}

beforeEach(() => {
  window.localStorage.clear();
  vi.clearAllMocks();
  // Default: an empty rail with no stored selection, so the single-thread S-200
  // suites are unaffected. Rail suites override these per test.
  mockFetchThreads.mockResolvedValue([]);
  mockFetchMessages.mockResolvedValue([]);
  mockDeleteThread.mockResolvedValue({ ok: true, status: 204 } as unknown as Response);
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

/** Acknowledge the first-use consent banner so the composer is enabled. */
async function acceptConsent(user: ReturnType<typeof userEvent.setup>) {
  await user.click(await screen.findByRole("button", { name: "Start chatting" }));
}

/** Type a question and send it via the assistant-ui composer. */
async function ask(user: ReturnType<typeof userEvent.setup>, question: string) {
  await user.type(screen.getByRole("textbox", { name: "Your message" }), question);
  await user.click(screen.getByRole("button", { name: "Send" }));
}

describe("ChatView — configured chrome", () => {
  it("shows the consent banner naming the endpoint, with the composer gated", async () => {
    mockFetchConfig.mockResolvedValue(configuredModel());
    render(<ChatView />);
    // The consent banner discloses what is sent and to where (NFR-SE-07).
    expect(await screen.findByText(/source and graph excerpts/)).toBeInTheDocument();
    expect(screen.getAllByText(/openrouter\.ai/).length).toBeGreaterThan(0);
    // The composer input is disabled until the explicit acknowledgement.
    expect(screen.getByRole("textbox", { name: "Your message" })).toBeDisabled();
  });

  it("names the native Anthropic host for the anthropic provider", async () => {
    mockFetchConfig.mockResolvedValue(configuredModel("anthropic"));
    render(<ChatView />);
    expect((await screen.findAllByText(/api\.anthropic\.com/)).length).toBeGreaterThan(0);
  });

  it("enables the composer after consent is acknowledged", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    render(<ChatView />);
    await acceptConsent(user);
    expect(screen.getByRole("textbox", { name: "Your message" })).toBeEnabled();
  });
});

describe("ChatView — configure-first", () => {
  it("renders the honest configure-first state with no composer", async () => {
    mockFetchConfig.mockResolvedValue(unconfiguredModel());
    render(<ChatView />);
    expect(await screen.findByText(/needs an LLM provider/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Config" })).toHaveAttribute("href", "/config");
    expect(screen.queryByRole("button", { name: "Send" })).not.toBeInTheDocument();
  });
});

describe("ChatView — a streamed turn", () => {
  it("renders plan, subagent activity, streamed tokens, and the final answer", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockStreamTurn.mockResolvedValue(
      sseResponse([
        'event: plan\ndata: {"round":0,"steps":[{"role":"graph_navigator","instruction":"map callers"}]}\n\n',
        'event: step_started\ndata: {"index":0,"role":"graph_navigator","instruction":"map callers"}\n\n',
        'event: step_observed\ndata: {"index":0,"role":"graph_navigator","summary":"3 callers"}\n\n',
        'event: answer_delta\ndata: {"delta":"The riskiest "}\n\n',
        'event: answer_delta\ndata: {"delta":"code is X."}\n\n',
        'event: final_answer\ndata: {"answer":"The riskiest code is X."}\n\n',
      ]),
    );
    const { container } = render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "what is risky?");

    expect(await screen.findByText("The riskiest code is X.")).toBeInTheDocument();
    expect(screen.getByText("what is risky?")).toBeInTheDocument();
    expect(container.textContent).toContain("Plan");
    expect(container.textContent).toContain("Graph-Navigator");
    // The streamed message is the byte-identical form-encoded body (NFR-SE-06 path);
    // a fresh conversation carries no thread id (null → the server creates it).
    expect(mockStreamTurn).toHaveBeenCalledWith("what is risky?", null, expect.anything());
  });

  it("renders the answer as markdown with a copyable code block", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockStreamTurn.mockResolvedValue(
      sseResponse([
        'event: final_answer\ndata: {"answer":"Use **bold** and `inline` then:\\n\\n```rust\\nfn main() {}\\n```"}\n\n',
      ]),
    );
    const { container } = render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "show code");

    // Markdown structure, not raw markdown text.
    expect(await screen.findByText("bold")).toBeInTheDocument();
    expect(container.querySelector("strong")?.textContent).toBe("bold");
    expect(container.querySelector("code")?.textContent).toContain("inline");
    // The fenced block carries a language label and a per-block copy control.
    expect(screen.getByText("rust")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Copy code" })).toBeInTheDocument();
  });

  it("copies a fenced code block to the clipboard via its copy control", async () => {
    const user = userEvent.setup();
    const writeText = vi.spyOn(navigator.clipboard, "writeText").mockResolvedValue();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockStreamTurn.mockResolvedValue(
      sseResponse(['event: final_answer\ndata: {"answer":"```rust\\nfn main() {}\\n```"}\n\n']),
    );
    render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "show code");

    const copyCode = await screen.findByRole("button", { name: "Copy code" });
    await user.click(copyCode);
    expect(writeText).toHaveBeenCalledWith("fn main() {}");
    // The control reflects the copied state.
    expect(await screen.findByRole("button", { name: "Copy code" })).toHaveTextContent("Copied");
  });

  it("renders an honest halt, never a fabricated answer", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockStreamTurn.mockResolvedValue(
      sseResponse([
        'event: step_started\ndata: {"index":0,"role":"source_reader","instruction":"read"}\n\n',
        'event: halted\ndata: {"round":1,"bound":{"bound":"global_tool_calls","limit":24}}\n\n',
      ]),
    );
    render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "q");
    expect(
      await screen.findByText(/global per-turn tool-call ceiling was reached \(24 calls\)/),
    ).toBeInTheDocument();
  });

  it("renders the honest provider error verbatim (FR-UI-24 cause chain)", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    // The S-199 contract: a single-line plain-text `error` frame.
    const honest = "Chat failed during the planner stage: error sending request for url (provider)";
    mockStreamTurn.mockResolvedValue(sseResponse([`event: error\ndata: ${honest}\n\n`]));
    render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "q");
    expect(await screen.findByText(honest)).toBeInTheDocument();
  });

  it("surfaces a turn that closed without producing an answer", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockStreamTurn.mockResolvedValue(sseResponse([]));
    render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "q");
    expect(await screen.findByText(/ended without an answer/)).toBeInTheDocument();
  });

  it("surfaces an honest error when the turn fails to start (non-ok response)", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockStreamTurn.mockResolvedValue({ ok: false, status: 500, body: null } as unknown as Response);
    render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "q");
    expect(await screen.findByText(/could not start \(status 500\)/)).toBeInTheDocument();
  });
});

describe("ChatView — copy, stop, and regenerate (FR-UI-19, FR-UI-20)", () => {
  it("offers copy and regenerate on a completed turn", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockStreamTurn.mockResolvedValue(sseResponse(['event: final_answer\ndata: {"answer":"done"}\n\n']));
    render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "q");
    expect(await screen.findByText("done")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Copy answer" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Regenerate answer" })).toBeInTheDocument();
  });

  it("regenerate replaces the last assistant turn rather than appending a duplicate", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockStreamTurn.mockResolvedValueOnce(
      sseResponse(['event: final_answer\ndata: {"answer":"first answer"}\n\n']),
    );
    render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "q");
    expect(await screen.findByText("first answer")).toBeInTheDocument();

    mockStreamTurn.mockResolvedValueOnce(
      sseResponse(['event: final_answer\ndata: {"answer":"second answer"}\n\n']),
    );
    await user.click(screen.getByRole("button", { name: "Regenerate answer" }));

    expect(await screen.findByText("second answer")).toBeInTheDocument();
    // The replaced turn is gone (no duplicate assistant turn), and the single user
    // message is preserved — a presentation-level replace (ADR-45, FR-UI-20).
    await waitFor(() => expect(screen.queryByText("first answer")).not.toBeInTheDocument());
    expect(screen.getAllByText("q")).toHaveLength(1);
    expect(mockStreamTurn).toHaveBeenCalledTimes(2);
  });

  it("shows a Stop control while a turn is in flight and cancels it", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    const pending = pendingSseResponse([
      'event: answer_delta\ndata: {"delta":"thinking"}\n\n',
    ]);
    mockStreamTurn.mockResolvedValue(pending.response);
    render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "q");

    // While running, the Stop control is shown (Send is hidden).
    const stop = await screen.findByRole("button", { name: "Stop" });
    expect(stop).toBeInTheDocument();
    await user.click(stop);
    // After stop, the composer returns to its Send state...
    expect(await screen.findByRole("button", { name: "Send" })).toBeInTheDocument();
    // ...and the stopped turn is finalized over what streamed, so its Copy and
    // Regenerate actions are available (onCancel marks the turn ended).
    expect(screen.getByRole("button", { name: "Copy answer" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Regenerate answer" })).toBeInTheDocument();
    expect(screen.getByText("thinking")).toBeInTheDocument();
    pending.close();
  });
});

describe("ChatView — per-conversation delete (S-211, FR-UI-26, AC-1)", () => {
  it("has no global Clear-history control at all", async () => {
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockFetchThreads.mockResolvedValue([thread(5, "Conv A", 300)]);
    render(<ChatView />);
    await screen.findByRole("button", { name: "Conv A" });
    // The clear-all affordance and its result line are gone with the route (S-209).
    expect(screen.queryByRole("button", { name: "Clear history" })).not.toBeInTheDocument();
    expect(screen.queryByText(/History cleared/)).not.toBeInTheDocument();
  });

  it("requires a confirmation before deleting: the first click only arms it", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockFetchThreads.mockResolvedValue([thread(5, "Conv A", 300)]);
    render(<ChatView />);
    await screen.findByRole("button", { name: "Conv A" });

    await user.click(deleteButton("Conv A"));
    // Arming discloses the confirm step and issues NO write.
    expect(
      await screen.findByText(/Delete this conversation and its memory\?/),
    ).toBeInTheDocument();
    expect(mockDeleteThread).not.toHaveBeenCalled();
  });

  it("deletes exactly the confirmed conversation and drops its row", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockFetchThreads.mockReset();
    mockFetchThreads.mockResolvedValueOnce([thread(5, "Conv A", 300), thread(3, "Conv B", 100)]);
    // The post-delete refresh sees the server without the deleted conversation.
    mockFetchThreads.mockResolvedValue([thread(5, "Conv A", 300)]);
    render(<ChatView />);
    await screen.findByRole("button", { name: "Conv B" });

    await user.click(deleteButton("Conv B"));
    await user.click(await screen.findByRole("button", { name: "Delete" }));

    expect(mockDeleteThread).toHaveBeenCalledTimes(1);
    expect(mockDeleteThread).toHaveBeenCalledWith(3);
    // Only the deleted row leaves the rail; the other conversation stays.
    await waitFor(() => expect(screen.queryByRole("button", { name: "Conv B" })).not.toBeInTheDocument());
    expect(screen.getByRole("button", { name: "Conv A" })).toBeInTheDocument();
  });

  it("cancelling deletes nothing and keeps the conversation", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockFetchThreads.mockResolvedValue([thread(5, "Conv A", 300)]);
    render(<ChatView />);
    await screen.findByRole("button", { name: "Conv A" });

    await user.click(deleteButton("Conv A"));
    await user.click(await screen.findByRole("button", { name: "Cancel" }));

    expect(mockDeleteThread).not.toHaveBeenCalled();
    // The confirm step is disarmed and the conversation is still listed.
    expect(screen.queryByText(/Delete this conversation and its memory\?/)).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Conv A" })).toBeInTheDocument();
  });

  it("deleting the OPEN conversation resets the surface to a fresh composer", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockFetchThreads.mockReset();
    mockFetchThreads.mockResolvedValueOnce([thread(5, "Conv A", 300)]);
    mockFetchThreads.mockResolvedValue([]); // after the delete the server has none
    mockFetchMessages.mockResolvedValue([
      persisted("user", "open question", 1),
      persisted("assistant", "open answer", 2),
    ]);
    render(<ChatView />);
    await user.click(await screen.findByRole("button", { name: "Conv A" }));
    expect(await screen.findByText("open answer")).toBeInTheDocument();

    await user.click(deleteButton("Conv A"));
    await user.click(await screen.findByRole("button", { name: "Delete" }));

    // The restored transcript goes with the conversation, and the remembered
    // selection is dropped so a reload does not try to re-open a deleted thread.
    await waitFor(() => expect(screen.queryByText("open answer")).not.toBeInTheDocument());
    expect(screen.getByRole("button", { name: "Send" })).toBeInTheDocument();
    await waitFor(() => expect(window.localStorage.getItem("logos.chat.activeThread")).toBeNull());
  });

  it("leaves the open conversation intact when a DIFFERENT one is deleted", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockFetchThreads.mockReset();
    mockFetchThreads.mockResolvedValueOnce([thread(5, "Conv A", 300), thread(3, "Conv B", 100)]);
    mockFetchThreads.mockResolvedValue([thread(5, "Conv A", 300)]);
    mockFetchMessages.mockResolvedValue([
      persisted("user", "kept question", 1),
      persisted("assistant", "kept answer", 2),
    ]);
    render(<ChatView />);
    await user.click(await screen.findByRole("button", { name: "Conv A" }));
    expect(await screen.findByText("kept answer")).toBeInTheDocument();

    await user.click(deleteButton("Conv B"));
    await user.click(await screen.findByRole("button", { name: "Delete" }));

    await waitFor(() => expect(screen.queryByRole("button", { name: "Conv B" })).not.toBeInTheDocument());
    // The open transcript and the remembered selection are untouched.
    expect(screen.getByText("kept answer")).toBeInTheDocument();
    expect(window.localStorage.getItem("logos.chat.activeThread")).toBe("5");
  });

  it("keeps the row and says so honestly when the delete faults", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockFetchThreads.mockResolvedValue([thread(5, "Conv A", 300)]);
    mockDeleteThread.mockResolvedValue({ ok: false, status: 500 } as unknown as Response);
    render(<ChatView />);
    await screen.findByRole("button", { name: "Conv A" });

    await user.click(deleteButton("Conv A"));
    await user.click(await screen.findByRole("button", { name: "Delete" }));

    // An honest note rather than a row that vanishes while the conversation lives on.
    expect(
      await screen.findByText(/Could not delete that conversation \(status 500\)/),
    ).toBeInTheDocument();
    expect(mockDeleteThread).toHaveBeenCalledTimes(1);
    expect(mockDeleteThread).toHaveBeenCalledWith(5);
  });

  it("keeps the row and says so honestly when the delete transport fails", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockFetchThreads.mockResolvedValue([thread(5, "Conv A", 300)]);
    // A rejected promise, not a non-ok Response: the request never reached a status.
    mockDeleteThread.mockRejectedValue(new Error("network down"));
    render(<ChatView />);
    await screen.findByRole("button", { name: "Conv A" });

    await user.click(deleteButton("Conv A"));
    await user.click(await screen.findByRole("button", { name: "Delete" }));

    // The cause is surfaced verbatim, and the conversation stays — a transport fault
    // is not evidence the server deleted anything.
    expect(
      await screen.findByText(/Could not delete that conversation: network down/),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Conv A" })).toBeInTheDocument();
  });

  it("keeps the deleted row gone when the post-delete rail refresh fails", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockFetchThreads.mockReset();
    mockFetchThreads.mockResolvedValueOnce([thread(5, "Conv A", 300), thread(3, "Conv B", 100)]);
    // The delete succeeds, but the refresh that would re-read the order faults.
    mockFetchThreads.mockRejectedValue(new ApiError("chat/threads", 500));
    render(<ChatView />);
    await screen.findByRole("button", { name: "Conv B" });

    await user.click(deleteButton("Conv B"));
    await user.click(await screen.findByRole("button", { name: "Delete" }));

    // The delete's own authority stands: the row does not come back just because the
    // refresh failed, and the refresh failure is reported honestly rather than hidden.
    expect(await screen.findByText(/Could not load your conversations/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Conv B" })).not.toBeInTheDocument();
  });

  it("deleting the open conversation mid-stream aborts the turn and cannot be hijacked", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockFetchThreads.mockReset();
    mockFetchThreads.mockResolvedValueOnce([thread(9, "Conv 9", 400)]);
    mockFetchThreads.mockResolvedValue([]); // after the delete the server has none
    mockFetchMessages.mockResolvedValue([
      persisted("user", "hi", 1),
      persisted("assistant", "hello", 2),
    ]);
    const pending = pendingSseResponse(['event: answer_delta\ndata: {"delta":"thinking"}\n\n']);
    mockStreamTurn.mockResolvedValueOnce(pending.response);
    render(<ChatView />);
    await acceptConsent(user);
    await user.click(await screen.findByRole("button", { name: "Conv 9" }));
    await screen.findByText("hello");
    await ask(user, "a follow-up");
    await screen.findByRole("button", { name: "Stop" }); // in flight on thread 9

    // Delete the conversation the turn is streaming into, then let it settle.
    await user.click(deleteButton("Conv 9"));
    await user.click(await screen.findByRole("button", { name: "Delete" }));
    pending.close();

    // The surface resets to a fresh composer and the settling turn cannot rebind the
    // deleted thread: no streamed content survives and the selection is dropped.
    await waitFor(() => expect(screen.queryByText("thinking")).not.toBeInTheDocument());
    expect(screen.getByRole("button", { name: "Send" })).toBeInTheDocument();
    await waitFor(() => expect(window.localStorage.getItem("logos.chat.activeThread")).toBeNull());
  });

  it("treats an already-gone conversation (404) as deleted, not as a fault", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockFetchThreads.mockReset();
    mockFetchThreads.mockResolvedValueOnce([thread(5, "Conv A", 300)]);
    mockFetchThreads.mockResolvedValue([]);
    mockDeleteThread.mockResolvedValue({ ok: false, status: 404 } as unknown as Response);
    render(<ChatView />);
    await screen.findByRole("button", { name: "Conv A" });

    await user.click(deleteButton("Conv A"));
    await user.click(await screen.findByRole("button", { name: "Delete" }));

    // The user's intent ("this should not exist") is satisfied — the row goes and no
    // error is invented.
    await waitFor(() => expect(screen.queryByRole("button", { name: "Conv A" })).not.toBeInTheDocument());
    expect(screen.queryByText(/Could not delete/)).not.toBeInTheDocument();
  });

  it("arms only one row at a time (no two live destructive actions)", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockFetchThreads.mockResolvedValue([thread(5, "Conv A", 300), thread(3, "Conv B", 100)]);
    render(<ChatView />);
    await screen.findByRole("button", { name: "Conv B" });

    await user.click(deleteButton("Conv A"));
    await user.click(deleteButton("Conv B"));
    // Arming B disarms A: exactly one confirm panel is open.
    expect(screen.getAllByRole("button", { name: "Delete" })).toHaveLength(1);
    expect(deleteButton("Conv B")).toHaveAttribute("aria-expanded", "true");
    expect(deleteButton("Conv A")).toHaveAttribute("aria-expanded", "false");
  });

  it("disarms a pending confirm when the conversation is switched", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockFetchThreads.mockResolvedValue([thread(5, "Conv A", 300), thread(3, "Conv B", 100)]);
    mockFetchMessages.mockResolvedValue([persisted("user", "q", 1), persisted("assistant", "a", 2)]);
    render(<ChatView />);
    await screen.findByRole("button", { name: "Conv B" });

    // Arm the delete on Conv A, then move to Conv B without answering it.
    await user.click(deleteButton("Conv A"));
    expect(await screen.findByRole("button", { name: "Delete" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Conv B" }));
    await screen.findByText("a");

    // The armed prompt does not lie in wait behind the switch.
    await waitFor(() =>
      expect(screen.queryByRole("button", { name: "Delete" })).not.toBeInTheDocument(),
    );
    expect(deleteButton("Conv A")).toHaveAttribute("aria-expanded", "false");
    expect(mockDeleteThread).not.toHaveBeenCalled();
  });

  it("disarms a pending confirm when + New chat is started", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockFetchThreads.mockResolvedValue([thread(5, "Conv A", 300)]);
    mockFetchMessages.mockResolvedValue([persisted("user", "q", 1), persisted("assistant", "a", 2)]);
    render(<ChatView />);
    // Open a conversation first, so "+ New chat" is a real switch (active → null).
    await user.click(await screen.findByRole("button", { name: "Conv A" }));
    await screen.findByText("a");

    await user.click(deleteButton("Conv A"));
    expect(await screen.findByRole("button", { name: "Delete" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "+ New chat" }));

    await waitFor(() =>
      expect(screen.queryByRole("button", { name: "Delete" })).not.toBeInTheDocument(),
    );
    expect(mockDeleteThread).not.toHaveBeenCalled();
  });

  it("a stale rail refresh cannot resurrect a conversation deleted while a turn settled", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockFetchMessages.mockResolvedValue([
      persisted("user", "hi", 1),
      persisted("assistant", "hello", 2),
    ]);
    // The settling turn's rail refresh is held open by hand: its snapshot was taken
    // BEFORE the delete landed, and it resolves AFTER — the out-of-order arrival a
    // real network produces on its own.
    let releaseStale: (list: ThreadSummary[]) => void = () => {};
    const stale = new Promise<ThreadSummary[]>((resolve) => {
      releaseStale = resolve;
    });
    mockFetchThreads.mockReset();
    mockFetchThreads
      .mockResolvedValueOnce([thread(5, "Conv A", 300), thread(3, "Conv B", 100)]) // mount
      .mockReturnValueOnce(stale) // the settling turn's reconcile — held open
      .mockResolvedValue([thread(3, "Conv B", 100)]); // the truth: Conv A is gone
    const pending = pendingSseResponse(['event: answer_delta\ndata: {"delta":"…"}\n\n']);
    mockStreamTurn.mockResolvedValueOnce(pending.response);

    render(<ChatView />);
    await acceptConsent(user);
    await user.click(await screen.findByRole("button", { name: "Conv B" }));
    await screen.findByText("hello");
    await ask(user, "a question");
    await screen.findByRole("button", { name: "Stop" });

    // The turn ends, so its trailing reconcile issues the (now doomed) list read.
    pending.close();
    await waitFor(() => expect(mockFetchThreads).toHaveBeenCalledTimes(2));

    // Meanwhile the user deletes the OTHER conversation; its own refresh is current.
    await user.click(deleteButton("Conv A"));
    await user.click(await screen.findByRole("button", { name: "Delete" }));
    await waitFor(() =>
      expect(screen.queryByRole("button", { name: "Conv A" })).not.toBeInTheDocument(),
    );

    // Only now does the pre-delete snapshot arrive. It must not repaint Conv A —
    // the rail would otherwise lie about a conversation that is gone server-side.
    releaseStale([thread(5, "Conv A", 300), thread(3, "Conv B", 100)]);
    await waitFor(() => expect(screen.getByRole("button", { name: "Conv B" })).toBeInTheDocument());
    expect(screen.queryByRole("button", { name: "Conv A" })).not.toBeInTheDocument();
  });
});

describe("ChatView — secret masking (NFR-SE-07)", () => {
  it("never renders the masked chat key on the chat surface", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockStreamTurn.mockResolvedValue(sseResponse(['event: final_answer\ndata: {"answer":"done"}\n\n']));
    const { container } = render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "q");
    await screen.findByText("done");
    // The masked key's last-4 (and any `api_key` field) is structurally absent —
    // the configured body never receives `chat_key`.
    expect(container.textContent).not.toContain(MASKED_LAST4);
    expect(container.innerHTML).not.toContain("api_key");
  });
});

describe("ChatView — runtime adapter unit", () => {
  it("uses 'q' as the only sent payload, never the key", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockStreamTurn.mockResolvedValue(sseResponse(['event: final_answer\ndata: {"answer":"ok"}\n\n']));
    render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "the question");
    await screen.findByText("ok");
    const [question] = mockStreamTurn.mock.calls[0];
    expect(question).toBe("the question");
  });
});

describe("ChatView — conversation-history rail (S-210, FR-UI-26)", () => {
  it("lists conversations most-recent-first over GET /chat/threads", async () => {
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockFetchThreads.mockResolvedValue([
      thread(5, "Newest conversation", 300),
      thread(3, "Older conversation", 100),
    ]);
    render(<ChatView />);
    // The rail renders the server's most-recent-first order verbatim.
    const newest = await screen.findByRole("button", { name: "Newest conversation" });
    const older = screen.getByRole("button", { name: "Older conversation" });
    expect(newest).toBeInTheDocument();
    expect(older).toBeInTheDocument();
    // DOM order mirrors list order (most-recent first).
    expect(newest.compareDocumentPosition(older) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("select-to-restore hydrates a thread's full history via the messages endpoint", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockFetchThreads.mockResolvedValue([thread(5, "Conv A", 300)]);
    mockFetchMessages.mockResolvedValue([
      persisted("user", "a restored question", 1),
      persisted("assistant", "a restored answer", 2),
    ]);
    render(<ChatView />);
    await user.click(await screen.findByRole("button", { name: "Conv A" }));
    // The restored transcript renders (user text + the final assistant answer).
    expect(await screen.findByText("a restored question")).toBeInTheDocument();
    expect(screen.getByText("a restored answer")).toBeInTheDocument();
    expect(mockFetchMessages).toHaveBeenCalledWith(5);
  });

  it("+ New chat resets the composer and creates no empty row until first send", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockFetchThreads.mockResolvedValue([thread(5, "Conv A", 300)]);
    mockFetchMessages.mockResolvedValue([
      persisted("user", "old question", 1),
      persisted("assistant", "old answer", 2),
    ]);
    render(<ChatView />);
    // Restore a conversation, then reset with "+ New chat".
    await user.click(await screen.findByRole("button", { name: "Conv A" }));
    expect(await screen.findByText("old answer")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "+ New chat" }));
    // The log is cleared to a fresh composer…
    await waitFor(() => expect(screen.queryByText("old answer")).not.toBeInTheDocument());
    expect(screen.getByRole("button", { name: "Send" })).toBeInTheDocument();
    // …and no turn was streamed and no new thread was created (no empty rows): the
    // rail still shows exactly the one server-side conversation.
    expect(mockStreamTurn).not.toHaveBeenCalled();
    expect(screen.getAllByRole("button", { name: "Conv A" })).toHaveLength(1);
    // The stored selection is cleared, so a reload lands on a fresh composer.
    await waitFor(() => expect(window.localStorage.getItem("logos.chat.activeThread")).toBeNull());
  });

  it("marks the open conversation with aria-current", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockFetchThreads.mockResolvedValue([thread(5, "Conv A", 300), thread(3, "Conv B", 100)]);
    mockFetchMessages.mockResolvedValue([persisted("user", "q", 1), persisted("assistant", "a", 2)]);
    render(<ChatView />);
    await user.click(await screen.findByRole("button", { name: "Conv A" }));
    await screen.findByText("a");
    // The rail reflects which conversation is active; the others do not.
    expect(screen.getByRole("button", { name: "Conv A" })).toHaveAttribute("aria-current", "true");
    expect(screen.getByRole("button", { name: "Conv B" })).not.toHaveAttribute("aria-current");
  });

  it("persists the conversation on first send and continues it with its thread id", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    // Mount sees no conversations; after the first send the server has created one,
    // which the rail discovers as the most-recent (top) thread (S-209 returns no id
    // on the SSE stream).
    mockFetchThreads.mockReset();
    mockFetchThreads.mockResolvedValueOnce([]); // mount
    mockFetchThreads.mockResolvedValue([thread(9, "first question", 400)]); // after send
    mockStreamTurn
      .mockResolvedValueOnce(sseResponse(['event: final_answer\ndata: {"answer":"first answer"}\n\n']))
      .mockResolvedValueOnce(sseResponse(['event: final_answer\ndata: {"answer":"second answer"}\n\n']));
    render(<ChatView />);
    await acceptConsent(user);

    await ask(user, "first question");
    expect(await screen.findByText("first answer")).toBeInTheDocument();
    // The first send carried no thread id (a fresh conversation).
    expect(mockStreamTurn.mock.calls[0][1]).toBeNull();
    // The just-created conversation is adopted and appears in the rail.
    expect(await screen.findByRole("button", { name: "first question" })).toBeInTheDocument();

    await ask(user, "second turn");
    await screen.findByText("second answer");
    // The follow-up turn continues the SAME server thread (id 9), not a new one.
    expect(mockStreamTurn.mock.calls[1][1]).toBe(9);
  });

  it("restores the last-open conversation across a reload (persisted selection)", async () => {
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockFetchThreads.mockResolvedValue([thread(7, "Persisted conv", 500)]);
    mockFetchMessages.mockResolvedValue([
      persisted("user", "remembered question", 1),
      persisted("assistant", "remembered answer", 2),
    ]);
    // A prior session left thread 7 open.
    window.localStorage.setItem("logos.chat.activeThread", "7");
    render(<ChatView />);
    // On mount the runtime re-hydrates the stored selection with no user action.
    expect(await screen.findByText("remembered answer")).toBeInTheDocument();
    expect(screen.getByText("remembered question")).toBeInTheDocument();
    expect(mockFetchMessages).toHaveBeenCalledWith(7);
  });

  it("falls back to a fresh composer when the stored selection was deleted (404)", async () => {
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockFetchThreads.mockResolvedValue([]);
    mockFetchMessages.mockRejectedValue(new ApiError("chat/threads/7", 404));
    window.localStorage.setItem("logos.chat.activeThread", "7");
    render(<ChatView />);
    // No crash, no restored history — an honest empty composer.
    expect(await screen.findByRole("button", { name: "Send" })).toBeInTheDocument();
    // The stale stored selection is cleared.
    await waitFor(() => expect(window.localStorage.getItem("logos.chat.activeThread")).toBeNull());
  });

  it("collapses the rail behind a toggle (responsive disclosure, AC-3)", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockFetchThreads.mockResolvedValue([thread(5, "Conv A", 300)]);
    render(<ChatView />);
    // The toggle controls the rail region and starts collapsed on narrow viewports.
    const toggle = await screen.findByRole("button", { name: "Conversations" });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(toggle).toHaveAttribute("aria-controls", "chat-rail");

    await user.click(toggle);
    // Toggling discloses the rail (aria-expanded flips; the label reflects state).
    expect(await screen.findByRole("button", { name: "Hide conversations" })).toHaveAttribute(
      "aria-expanded",
      "true",
    );
  });
});

describe("ChatView — rail honest error paths (S-210)", () => {
  it("shows an honest note when the conversation list fails to load", async () => {
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockFetchThreads.mockRejectedValue(new ApiError("chat/threads", 500));
    render(<ChatView />);
    // The rail degrades to an honest note (not a silently empty list), and the chat
    // surface still mounts.
    expect(await screen.findByText(/Could not load your conversations/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "+ New chat" })).toBeInTheDocument();
  });

  it("keeps the open conversation and notes a fault when a restore faults (non-404)", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockFetchThreads.mockResolvedValue([thread(5, "Conv A", 300), thread(3, "Conv B", 100)]);
    mockFetchMessages.mockImplementation((id: number) =>
      id === 5
        ? Promise.resolve([persisted("user", "kept question", 1), persisted("assistant", "kept answer", 2)])
        : Promise.reject(new ApiError(`chat/threads/${id}`, 500)),
    );
    render(<ChatView />);
    await user.click(await screen.findByRole("button", { name: "Conv A" }));
    expect(await screen.findByText("kept answer")).toBeInTheDocument();

    // Selecting a conversation whose transcript faults (non-404) keeps the current
    // view intact and surfaces an honest note — never a blank/broken surface.
    await user.click(screen.getByRole("button", { name: "Conv B" }));
    expect(await screen.findByText(/Could not open that conversation/)).toBeInTheDocument();
    expect(screen.getByText("kept answer")).toBeInTheDocument();
  });

  it("notes an unrefreshable rail after a first send without silently forking", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    // Mount empty; the post-turn refresh (which would learn the new thread id) fails.
    mockFetchThreads.mockReset();
    mockFetchThreads.mockResolvedValueOnce([]); // mount
    mockFetchThreads.mockRejectedValue(new ApiError("chat/threads", 500)); // post-turn
    mockStreamTurn.mockResolvedValue(sseResponse(['event: final_answer\ndata: {"answer":"saved"}\n\n']));
    render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "first question");
    expect(await screen.findByText("saved")).toBeInTheDocument();
    // The turn is durable, but the id could not be learned — an honest note rather
    // than a silent state that would fork a second thread on the next send.
    expect(await screen.findByText(/could not refresh/)).toBeInTheDocument();
  });
});

describe("ChatView — multi-thread integrity (review-fix regressions)", () => {
  it("does not fork a second thread: a follow-up continues the adopted conversation", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockFetchThreads.mockReset();
    mockFetchThreads.mockResolvedValueOnce([]); // mount
    mockFetchThreads.mockResolvedValue([thread(9, "first question", 400)]); // post-send (thread 9 created)
    mockStreamTurn
      .mockResolvedValueOnce(sseResponse(['event: final_answer\ndata: {"answer":"a1"}\n\n']))
      .mockResolvedValueOnce(sseResponse(['event: final_answer\ndata: {"answer":"a2"}\n\n']));
    render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "first question");
    await screen.findByText("a1");
    // Adoption completes before the composer re-enables, so the follow-up carries
    // the adopted id — never a null that would create a duplicate thread.
    await ask(user, "second question");
    await screen.findByText("a2");
    expect(mockStreamTurn.mock.calls[0][1]).toBeNull();
    expect(mockStreamTurn.mock.calls[1][1]).toBe(9);
  });

  it("+ New chat during a streaming turn is not hijacked back to the old thread", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    // Start on an existing conversation (thread 9), then stream a pending turn on it.
    mockFetchThreads.mockResolvedValue([thread(9, "Conv 9", 400)]);
    mockFetchMessages.mockResolvedValue([persisted("user", "hi", 1), persisted("assistant", "hello", 2)]);
    const pending = pendingSseResponse(['event: answer_delta\ndata: {"delta":"thinking"}\n\n']);
    mockStreamTurn.mockResolvedValueOnce(pending.response);
    render(<ChatView />);
    await acceptConsent(user);
    await user.click(await screen.findByRole("button", { name: "Conv 9" }));
    await screen.findByText("hello");
    await ask(user, "a follow-up");
    await screen.findByRole("button", { name: "Stop" }); // the turn is in flight on thread 9

    // Mid-stream, the user starts a fresh conversation, then the aborted turn settles.
    await user.click(screen.getByRole("button", { name: "+ New chat" }));
    pending.close();

    // The fresh session must NOT be rebound to thread 9 by the aborted turn's
    // reconcile: the next send creates a new conversation (thread id null).
    mockStreamTurn.mockResolvedValueOnce(sseResponse(['event: final_answer\ndata: {"answer":"fresh"}\n\n']));
    await ask(user, "brand new");
    await screen.findByText("fresh");
    expect(mockStreamTurn.mock.calls[mockStreamTurn.mock.calls.length - 1][1]).toBeNull();
  });
});

describe("ChatView — single-thread behaviour unchanged (S-200 regression)", () => {
  it("streams a turn with no active thread exactly as before", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockStreamTurn.mockResolvedValue(sseResponse(['event: final_answer\ndata: {"answer":"unchanged"}\n\n']));
    render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "still works?");
    expect(await screen.findByText("unchanged")).toBeInTheDocument();
    // The S-200 body is byte-identical: `q` only, no thread id (null).
    expect(mockStreamTurn).toHaveBeenCalledWith("still works?", null, expect.anything());
  });
});

describe("ChatView — transcript column structure (S-300, FR-UI-31)", () => {
  it("nests the user turn's text in a bubble element inside the column row", async () => {
    // S-300 made the user MessagePrimitive.Root the full-measure column row and
    // moved the bubble treatment onto an inner element, so the bubble hugs the
    // COLUMN's right edge rather than the viewport's. That separation is a DOM
    // fact, and it is the only part of the realignment jsdom can see: the CSS
    // itself is asserted in `web/tests/spa_design_system.rs` (this suite runs
    // with `css: false`, so class names never reach the DOM). Without this
    // guard, deleting the wrapper would break the layout in every theme and no
    // test would notice.
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockStreamTurn.mockResolvedValue(sseResponse(['event: final_answer\ndata: {"answer":"answered"}\n\n']));
    render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "where does the column start?");
    const answer = await screen.findByText("answered");
    const question = screen.getByText("where does the column start?");

    // The viewport is the nearest ancestor holding BOTH turns; its children are
    // the two message roots.
    let viewport = question.parentElement;
    while (viewport && !viewport.contains(answer)) viewport = viewport.parentElement;
    expect(viewport).not.toBeNull();
    const userRoot = [...viewport!.children].find((c) => c.contains(question));
    expect(userRoot).toBeDefined();

    // The row is NOT the bubble: an element sits between the root and the text,
    // and it carries the whole message.
    const bubble = userRoot!.firstElementChild;
    expect(bubble?.tagName).toBe("DIV");
    expect(bubble).not.toBe(userRoot);
    expect(bubble?.textContent).toContain("where does the column start?");
  });
});

describe("ChatView — the Activity disclosure (S-301, FR-UI-31)", () => {
  // S-301 folded the separate plan list and the subagent pills into ONE native
  // <details> disclosure whose steps carry their FULL observed result as rendered
  // markdown (the `title=` hover tooltip is gone). The fold is the only <details>
  // an assistant turn renders, so `querySelector("details")` is an unambiguous
  // handle — this suite runs with `css: false`, so class names never reach the DOM
  // and the element/attribute structure is what can be asserted here.

  /** The turn's Activity fold, or `null` when the turn renders none. */
  function fold(container: HTMLElement): HTMLDetailsElement | null {
    return container.querySelector("details");
  }

  /** The frames of a turn that plans two steps, observes the first with a markdown
   *  result, and leaves the second running. */
  const STREAMING_FRAMES = [
    'event: plan\ndata: {"round":0,"steps":[{"role":"graph_navigator","instruction":"map callers"},{"role":"synthesizer","instruction":"write it up"}]}\n\n',
    'event: step_started\ndata: {"index":0,"role":"graph_navigator","instruction":"map callers"}\n\n',
    'event: step_observed\ndata: {"index":0,"role":"graph_navigator","summary":"found **3 callers** in `web/src`"}\n\n',
    'event: step_started\ndata: {"index":1,"role":"synthesizer","instruction":"write it up"}\n\n',
  ];

  it("holds the plan and every subagent step in one fold, open while the turn streams", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    const pending = pendingSseResponse(STREAMING_FRAMES);
    mockStreamTurn.mockResolvedValue(pending.response);
    const { container } = render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "who calls it?");

    // The last frame's step is the sync point: once it is on screen the whole
    // side-channel has been folded in.
    await waitFor(() => expect(container.textContent).toContain("Synthesizer"));

    // Exactly ONE disclosure carries the whole side-channel — no second fold, and
    // no plan list left rendering beside it.
    expect(container.querySelectorAll("details")).toHaveLength(1);
    const activity = fold(container)!;
    expect(activity.open).toBe(true);
    expect(activity.querySelector("summary")?.textContent).toContain("Activity");
    // The plan and BOTH steps live inside the fold, not around it.
    expect(activity.textContent).toContain("Plan");
    expect(activity.textContent).toContain("Graph-Navigator");
    expect(activity.textContent).toContain("map callers");
    expect(activity.textContent).toContain("write it up");
    pending.close();
  });

  it("renders each observed result as markdown, with no hover tooltip", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    const pending = pendingSseResponse(STREAMING_FRAMES);
    mockStreamTurn.mockResolvedValue(pending.response);
    const { container } = render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "who calls it?");
    await waitFor(() => expect(container.textContent).toContain("3 callers"));

    const activity = fold(container)!;
    // Markdown STRUCTURE, not the raw source: the result went through the same
    // react-markdown renderer the answer uses (never dangerouslySetInnerHTML).
    expect(activity.querySelector("strong")?.textContent).toBe("3 callers");
    expect(activity.querySelector("code")?.textContent).toBe("web/src");
    expect(activity.textContent).not.toContain("**3 callers**");
    // The result is readable in place — nothing inside the fold hides it behind a
    // native `title=` hover tooltip any more.
    expect(activity.querySelector("[title]")).toBeNull();
    pending.close();
  });

  it("auto-collapses when the answer is finalized, keeping its content to re-open", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockStreamTurn.mockResolvedValue(
      sseResponse([...STREAMING_FRAMES, 'event: final_answer\ndata: {"answer":"the answer"}\n\n']),
    );
    const { container } = render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "who calls it?");
    expect(await screen.findByText("the answer")).toBeInTheDocument();

    const activity = fold(container)!;
    expect(activity.open).toBe(false);
    // Collapsed, not discarded: re-opening shows the same plan and results.
    expect(activity.textContent).toContain("3 callers");
  });

  it("re-opens on click and then stays open across later transcript updates", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockStreamTurn.mockResolvedValueOnce(
      sseResponse([...STREAMING_FRAMES, 'event: final_answer\ndata: {"answer":"first"}\n\n']),
    );
    const { container } = render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "who calls it?");
    expect(await screen.findByText("first")).toBeInTheDocument();
    expect(fold(container)!.open).toBe(false);

    await user.click(fold(container)!.querySelector("summary")!);
    expect(fold(container)!.open).toBe(true);

    // A second turn re-renders the transcript; the user's choice outlives it (the
    // fold is persistent once re-opened, not re-collapsed on every update).
    mockStreamTurn.mockResolvedValueOnce(
      sseResponse(['event: final_answer\ndata: {"answer":"second"}\n\n']),
    );
    await ask(user, "and now?");
    expect(await screen.findByText("second")).toBeInTheDocument();
    expect(fold(container)!.open).toBe(true);
  });

  it("renders no fold at all for a restored answer-only turn", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockFetchThreads.mockResolvedValue([thread(7, "Restored", 100)]);
    // The store persists only the durable user + final-answer rows: the
    // plan/activity side-channel is ephemeral SSE, so a restored turn has neither.
    mockFetchMessages.mockResolvedValue([
      persisted("user", "old question", 1),
      persisted("assistant", "old answer", 2),
    ]);
    const { container } = render(<ChatView />);
    await user.click(await screen.findByRole("button", { name: "Restored" }));
    expect(await screen.findByText("old answer")).toBeInTheDocument();

    // No empty Activity fold, and no stray "Activity" label either.
    expect(fold(container)).toBeNull();
    expect(screen.queryByText("Activity")).not.toBeInTheDocument();
  });

  it("leaves the fold open on a halted turn and still names the bound honestly", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    // A halt never finalizes an answer, so there is nothing to collapse INTO: the
    // activity stays open as the honest record of how far the turn got (NFR-CC-04).
    mockStreamTurn.mockResolvedValue(
      sseResponse([
        'event: plan\ndata: {"round":0,"steps":[{"role":"source_reader","instruction":"read it"}]}\n\n',
        'event: step_started\ndata: {"index":0,"role":"source_reader","instruction":"read it"}\n\n',
        'event: halted\ndata: {"round":1,"bound":{"bound":"global_tool_calls","limit":24}}\n\n',
      ]),
    );
    const { container } = render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "q");
    expect(
      await screen.findByText(/global per-turn tool-call ceiling was reached \(24 calls\)/),
    ).toBeInTheDocument();
    expect(fold(container)!.open).toBe(true);
  });

  it("adopts an externally-driven open flip so the next click still closes the fold", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockStreamTurn.mockResolvedValue(
      sseResponse([...STREAMING_FRAMES, 'event: final_answer\ndata: {"answer":"done"}\n\n']),
    );
    const { container } = render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "who calls it?");
    expect(await screen.findByText("done")).toBeInTheDocument();
    const activity = fold(container)!;
    expect(activity.open).toBe(false);

    // Stand in for a find-in-page auto-expand: the browser sets `open` directly,
    // with no click on the summary. Without the `onToggle` sync the component would
    // still believe the fold is closed, and the click below would be spent
    // re-deriving `open: true` instead of closing it.
    activity.open = true;
    await waitFor(() => expect(activity.open).toBe(true));

    await user.click(activity.querySelector("summary")!);
    expect(activity.open).toBe(false);
  });

  it("renders no fold for a plan that carries zero steps", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    // `applyFrame` guards a malformed `steps` into `[]` rather than dropping the
    // frame, so `turn.plan` is PRESENT but carries nothing. Testing for the plan
    // object would open a fold onto a caption and an empty list — an empty Activity
    // fold, which the AC forbids.
    mockStreamTurn.mockResolvedValue(
      sseResponse([
        'event: plan\ndata: {"round":0,"steps":"not-an-array"}\n\n',
        'event: final_answer\ndata: {"answer":"answered anyway"}\n\n',
      ]),
    );
    const { container } = render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "q");
    expect(await screen.findByText("answered anyway")).toBeInTheDocument();
    expect(fold(container)).toBeNull();
    expect(screen.queryByText("Activity")).not.toBeInTheDocument();
  });

  it("leaves the fold open when the user stops a turn before any answer arrived", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    // Stop marks the turn `finalized` on abort (`onCancel`) whether or not anything
    // arrived — so `finalized` alone would collapse the trail at the exact moment
    // the user stopped to look at it. A turn that produced no answer, halt or error
    // keeps its activity open (NFR-CC-04).
    const pending = pendingSseResponse([
      'event: plan\ndata: {"round":0,"steps":[{"role":"source_reader","instruction":"read it"}]}\n\n',
      'event: step_started\ndata: {"index":0,"role":"source_reader","instruction":"read it"}\n\n',
    ]);
    mockStreamTurn.mockResolvedValue(pending.response);
    const { container } = render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "q");
    await waitFor(() => expect(container.textContent).toContain("Source-Reader"));

    await user.click(await screen.findByRole("button", { name: "Stop" }));
    expect(await screen.findByRole("button", { name: "Send" })).toBeInTheDocument();
    expect(fold(container)!.open).toBe(true);
    pending.close();
  });

  it("says so when an observed step reported no result, rather than showing a blank", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockStreamTurn.mockResolvedValue(
      sseResponse([
        'event: step_started\ndata: {"index":0,"role":"source_reader","instruction":"read it"}\n\n',
        // A malformed `summary` is dropped by the reducer rather than guessed at,
        // so the step is done with nothing observed — an honest empty, not a gap.
        'event: step_observed\ndata: {"index":0,"role":"source_reader","summary":42}\n\n',
        'event: final_answer\ndata: {"answer":"done anyway"}\n\n',
      ]),
    );
    const { container } = render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "q");
    expect(await screen.findByText("done anyway")).toBeInTheDocument();
    expect(fold(container)!.textContent).toContain("no result");
  });

  // The census on the summary is the ONLY thing a collapsed fold says about its
  // contents, and it has three shapes plus a singular/plural split. Nothing else
  // in the suite reads it — every other test stops at the word "Activity" — so
  // without these a reworded or mis-pluralised census ships unnoticed.

  /** The collapsed row's text: the "Activity" label plus its census. */
  function summaryText(container: HTMLElement): string {
    return fold(container)!.querySelector("summary")!.textContent ?? "";
  }

  it("counts the PLANNED steps before the first one starts", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    const pending = pendingSseResponse([STREAMING_FRAMES[0]]); // the plan, nothing started
    mockStreamTurn.mockResolvedValue(pending.response);
    const { container } = render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "who calls it?");
    await waitFor(() => expect(container.textContent).toContain("Activity"));

    // No step has reported in, so the census speaks in the planner's intent.
    expect(summaryText(container)).toContain("2 planned steps");
    pending.close();
  });

  it("counts observed progress while the steps are still running", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    const pending = pendingSseResponse(STREAMING_FRAMES);
    mockStreamTurn.mockResolvedValue(pending.response);
    const { container } = render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "who calls it?");
    await waitFor(() => expect(container.textContent).toContain("Synthesizer"));

    // Two steps started, one observed — the fraction, not the total.
    expect(summaryText(container)).toContain("1 of 2 steps");
    pending.close();
  });

  it("drops the fraction once every step has reported, and stays singular for one", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockStreamTurn.mockResolvedValue(
      sseResponse([
        'event: plan\ndata: {"round":0,"steps":[{"role":"source_reader","instruction":"read it"}]}\n\n',
        'event: step_started\ndata: {"index":0,"role":"source_reader","instruction":"read it"}\n\n',
        'event: step_observed\ndata: {"index":0,"role":"source_reader","summary":"read"}\n\n',
        'event: final_answer\ndata: {"answer":"done"}\n\n',
      ]),
    );
    const { container } = render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "q");
    expect(await screen.findByText("done")).toBeInTheDocument();

    // Everything reported, and one step is "1 step" — never "1 of 1 steps".
    expect(summaryText(container)).toContain("1 step");
    expect(summaryText(container)).not.toContain("1 of 1");
  });
});