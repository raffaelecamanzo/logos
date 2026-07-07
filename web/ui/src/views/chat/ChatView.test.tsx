import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// Mock the network module so the surface is driven by hand-built SSE streams and
// config models — the SSE contract is exercised through the real `chatModel`
// reader/reducer the runtime adapter imports. assistant-ui supplies the thread,
// composer, and copy/stop/regenerate affordances over that adapter.
vi.mock("../../api/chatClient.ts", () => ({
  CHAT_ROUTE: "/chat",
  CHAT_CLEAR_ROUTE: "/chat/clear",
  fetchChatConfig: vi.fn(),
  streamChatTurn: vi.fn(),
  clearChatHistory: vi.fn(),
}));

import { clearChatHistory, fetchChatConfig, streamChatTurn } from "../../api/chatClient.ts";
import type { ChatConfigReadModel } from "./chatModel.ts";
import { ChatView } from "./ChatView.tsx";

const mockFetchConfig = vi.mocked(fetchChatConfig);
const mockStreamTurn = vi.mocked(streamChatTurn);
const mockClear = vi.mocked(clearChatHistory);

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
    // The streamed message is the byte-identical form-encoded body (NFR-SE-06 path).
    expect(mockStreamTurn).toHaveBeenCalledWith("what is risky?", expect.anything());
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

describe("ChatView — Clear-history", () => {
  it("wipes the log on a confirmed clear", async () => {
    const user = userEvent.setup();
    vi.spyOn(window, "confirm").mockReturnValue(true);
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockStreamTurn.mockResolvedValue(sseResponse(['event: final_answer\ndata: {"answer":"hi there"}\n\n']));
    mockClear.mockResolvedValue({ ok: true, status: 200 } as unknown as Response);
    render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "q");
    expect(await screen.findByText("hi there")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Clear history" }));
    await waitFor(() => expect(screen.queryByText("hi there")).not.toBeInTheDocument());
    expect(screen.getByText("History cleared.")).toBeInTheDocument();
    expect(mockClear).toHaveBeenCalledOnce();
  });

  it("reports an honest error and keeps the log when clear fails", async () => {
    const user = userEvent.setup();
    vi.spyOn(window, "confirm").mockReturnValue(true);
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockStreamTurn.mockResolvedValue(sseResponse(['event: final_answer\ndata: {"answer":"kept answer"}\n\n']));
    mockClear.mockResolvedValue({ ok: false, status: 500 } as unknown as Response);
    render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "q");
    expect(await screen.findByText("kept answer")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Clear history" }));
    expect(await screen.findByText(/Could not clear history \(status 500\)/)).toBeInTheDocument();
    // The log is preserved on a failed clear (no silent wipe).
    expect(screen.getByText("kept answer")).toBeInTheDocument();
  });

  it("does not clear when the confirm is declined", async () => {
    const user = userEvent.setup();
    vi.spyOn(window, "confirm").mockReturnValue(false);
    mockFetchConfig.mockResolvedValue(configuredModel());
    render(<ChatView />);
    await acceptConsent(user);
    await user.click(screen.getByRole("button", { name: "Clear history" }));
    expect(mockClear).not.toHaveBeenCalled();
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