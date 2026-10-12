import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// Mock the network module so the surface is driven by hand-built SSE streams and
// config models — the SSE contract is exercised through the real `chatModel`
// reader/reducer the runtime adapter imports. assistant-ui supplies the thread,
// composer, and copy/stop/regenerate affordances over that adapter.
vi.mock("../../api/chatClient.ts", () => ({
  CHAT_ROUTE: "/chat",
  CHAT_THREADS_ROUTE: "/api/v1/chat/threads",
  // The two route sets are plain data, mirrored so the assertions below can name
  // which chat a call went to (S-485).
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

// The Mermaid render seam is mocked exactly as the wiki tests mock it, so jsdom
// never loads the 3 MB vendored UMD bundle (S-302). The per-component behaviour of
// the viewer lives in `MermaidBlock.test.tsx`; the suites here assert only that a
// diagram reaches BOTH surfaces that render through `MarkdownAnswer`.
vi.mock("../wiki/mermaid.ts", () => ({
  renderMermaidIn: vi.fn(() => Promise.resolve()),
  unadoptMermaidStyleFor: vi.fn(),
  VENDORED_MERMAID_URL: "/assets/vendor/mermaid.min.js",
}));

import { renderMermaidIn } from "../wiki/mermaid.ts";
import {
  deleteChatThread,
  fetchChatConfig,
  fetchThreadMessages,
  fetchThreads,
  fetchWorkspaceChatConfig,
  fetchWorkspaceChatReadRoots,
  MEMBER_CHAT_ROUTES,
  streamChatTurn,
  truncateChatThread,
  WORKSPACE_CHAT_ROUTES,
  type ChatRoutes,
} from "../../api/chatClient.ts";
import { ApiError } from "../../api/client.ts";
import type {
  ChatConfigReadModel,
  ChatOrigin,
  MemberChatReadRoots,
  PersistedChatMessage,
  ThreadSummary,
  WorkspaceChatConfigReadModel,
} from "./chatModel.ts";
import { ChatView } from "./ChatView.tsx";
import { WorkspaceChatView } from "./WorkspaceChatView.tsx";
import chatStyles from "./Chat.module.css";
import buttonStyles from "../../components/Button.module.css";
import cardStyles from "../../components/Card.module.css";
import { useWorkspace, WorkspaceProvider } from "../../workspace/WorkspaceContext.tsx";
import { setScopedMember } from "../../workspace/scope.ts";
import { stubApi } from "../../workspace/testFixtures.ts";
import { expectToolPanel } from "../../copy/expectWidgetCopy.ts";

const mockFetchConfig = vi.mocked(fetchChatConfig);
const mockStreamTurn = vi.mocked(streamChatTurn);
const mockDeleteThread = vi.mocked(deleteChatThread);
const mockTruncate = vi.mocked(truncateChatThread);
const mockFetchThreads = vi.mocked(fetchThreads);
const mockFetchMessages = vi.mocked(fetchThreadMessages);
const mockFetchWorkspaceConfig = vi.mocked(fetchWorkspaceChatConfig);
const mockFetchReadRoots = vi.mocked(fetchWorkspaceChatReadRoots);
const mockRenderMermaid = vi.mocked(renderMermaidIn);

/** Stand in for the vendored Mermaid bundle: paint an SVG into every unprocessed
 *  `.mermaid` target and set Mermaid's own `data-processed` latch, skipping a target
 *  that already carries it — as the real `mermaid.run` does. Without this, a test
 *  asserting a "diagram" would in fact be asserting the escaped-source fallback. */
function paintDiagrams() {
  mockRenderMermaid.mockImplementation((container: HTMLElement) => {
    for (const el of container.querySelectorAll<HTMLElement>(".mermaid")) {
      if (el.getAttribute("data-processed") === "true") continue;
      el.setAttribute("data-processed", "true");
      el.innerHTML = "<svg><g class='node'></g></svg>";
    }
    return Promise.resolve();
  });
}

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

/** A configured read-model (S-448's effective slice): the member declares both
 *  halves. The wire's masked last-4 must NEVER render — the chat mirror does not
 *  even carry it, but a fixture that sends it proves nothing leaks it through. */
const MASKED_LAST4 = "SECRET4";
function configuredModel(
  provider: "anthropic" | "openai" = "openai",
  origins: { policy?: ChatOrigin; credential?: ChatOrigin } = {},
): ChatConfigReadModel {
  return {
    effective_chat: {
      policy: {
        provider,
        model: "openrouter/some-model",
        base_url: "https://openrouter.ai/api/v1",
        max_tool_calls: 24,
        max_subagent_tool_calls: 8,
        max_replans: 3,
      },
      policy_origin: origins.policy ?? "member",
      credential: { present: true, last4: MASKED_LAST4 } as { present: boolean },
      credential_origin: origins.credential ?? "member",
      member_key_withheld: false,
    },
  };
}

/** A read-model whose resolution found `absent` half(s) undeclared at every root. */
function unconfiguredModel(
  absent: "model" | "key" | "both" = "key",
  presentOrigin: ChatOrigin = "member",
): ChatConfigReadModel {
  const m = configuredModel();
  const e = m.effective_chat;
  if (absent !== "key") {
    e.policy = { ...e.policy, model: null };
    e.policy_origin = "unset";
  } else {
    e.policy_origin = presentOrigin;
  }
  if (absent !== "model") {
    e.credential = { present: false };
    e.credential_origin = "unset";
  } else {
    e.credential_origin = presentOrigin;
  }
  return m;
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
 *  the Stop control. The returned `close` lets a test close it deterministically,
 *  and `push` writes more wire bytes into the open stream (S-626). */
function pendingSseResponse(initialChunks: string[]): {
  response: Response;
  close: () => void;
  push: (chunk: string) => void;
} {
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
    push: (chunk: string) => ctrl?.enqueue(enc.encode(chunk)),
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
  mockTruncate.mockResolvedValue({ ok: true, status: 204 } as unknown as Response);
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

  it("discloses the EFFECTIVE endpoint when the policy and key are inherited", async () => {
    // The member's own config.toml names no endpoint at all; the banner must name the
    // one a turn will actually dial, and say where it came from (NFR-SE-07).
    mockFetchConfig.mockResolvedValue(
      configuredModel("anthropic", { policy: "workspace", credential: "workspace" }),
    );
    render(<ChatView />);
    const banner = (await screen.findByText(/source and graph excerpts/)).closest("section");
    expect(banner).toHaveTextContent(/api\.anthropic\.com \(the anthropic endpoint inherited from the workspace root\)/);
    expect(banner).toHaveTextContent("The API key is inherited from the workspace root.");
    expect(banner?.textContent).not.toContain(MASKED_LAST4);
  });

  it("discloses the key's origin on its own when only the key is inherited", async () => {
    // Member policy + workspace key: the endpoint is the member's, the key is not. The
    // mirror (workspace policy + member key) is no longer a shape the seam produces:
    // an inherited policy takes the workspace key only (HF-1, ADR-67 §2).
    mockFetchConfig.mockResolvedValue(configuredModel("openai", { policy: "member", credential: "workspace" }));
    render(<ChatView />);
    const banner = (await screen.findByText(/source and graph excerpts/)).closest("section");
    expect(banner).toHaveTextContent("(the configured openai endpoint)");
    expect(banner).toHaveTextContent("The API key is inherited from the workspace root.");
  });

  it("calls a member-declared endpoint the configured one", async () => {
    mockFetchConfig.mockResolvedValue(configuredModel());
    render(<ChatView />);
    const banner = (await screen.findByText(/source and graph excerpts/)).closest("section");
    expect(banner).toHaveTextContent("(the configured openai endpoint)");
    expect(banner?.textContent).not.toMatch(/inherited/);
  });

  it("enables the composer after consent is acknowledged", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    render(<ChatView />);
    await acceptConsent(user);
    expect(screen.getByRole("textbox", { name: "Your message" })).toBeEnabled();
  });
});

describe("ChatView — a calmer red accent (S-618, CR-204 L7, FR-UI-45)", () => {
  // Class PLACEMENT only (CSS modules are a proxy under `css: false`); that the
  // neutral class actually fills neutral is `spa_design_system`'s job.
  it("renders 'Start chatting' as the neutral secondary button, never the red primary", async () => {
    mockFetchConfig.mockResolvedValue(configuredModel());
    render(<ChatView />);
    const start = await screen.findByRole("button", { name: "Start chatting" });
    expect(start.classList).toContain(buttonStyles.secondary);
    expect(start.classList).not.toContain(buttonStyles.primary);
  });
});

describe("ChatView — persistent status band (S-309, FR-UI-33)", () => {
  it("is absent until consent is given — the first-use gate stays verbatim ahead of it", async () => {
    mockFetchConfig.mockResolvedValue(configuredModel());
    render(<ChatView />);
    await screen.findByRole("button", { name: "Start chatting" });
    expect(screen.queryByText("CHAT")).not.toBeInTheDocument();
  });

  it("names the provider, endpoint host, model and the budget-tree bounds once consented", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    render(<ChatView />);
    await acceptConsent(user);

    const band = (await screen.findByText("CHAT")).closest("section");
    expect(band).toHaveTextContent("openai · openrouter.ai · openrouter/some-model");
    expect(band).toHaveTextContent("24 tool calls");
    expect(band).toHaveTextContent("8 per subagent");
    expect(band).toHaveTextContent("3 replans");
    // The masked key is structurally absent from the ChatPolicy slice this band
    // renders — a fixture that carries it proves the surface never leaks it in.
    expect(band?.textContent).not.toContain(MASKED_LAST4);
  });

  it("stays visible after the first message, unlike the compact line it replaces", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockStreamTurn.mockResolvedValue(sseResponse(['event: final_answer\ndata: {"answer":"Done."}\n\n']));
    render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "hello");
    await screen.findByText("Done.");

    const band = screen.getByText("CHAT").closest("section");
    expect(band).toHaveTextContent("openai · openrouter.ai · openrouter/some-model");
    expect(band).toHaveTextContent("24 tool calls");
  });

  it("leads the view above both panes, in the slot the consent banner vacates", async () => {
    // §4.13's lead callout is ONE element whose state moves from consent to band.
    // Inside the conversation pane instead, it would sit after the rail toggle —
    // and after the whole list when opened — once the panes stack below ~1023px.
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    render(<ChatView />);
    const slot = (await screen.findByText(/source and graph excerpts/)).closest("section")!.parentElement;
    await acceptConsent(user);

    const band = (await screen.findByText("CHAT")).closest("section")!;
    expect(band.parentElement).toBe(slot);
    expect(band.compareDocumentPosition(railToggle()) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("states an undeclared model honestly rather than a fabricated default (NFR-CC-04)", async () => {
    const user = userEvent.setup();
    // `resolve_chat` only resolves a policy origin once its model is declared, so
    // `ready: true` with no model is not a shape the server produces today — the
    // TypeScript type stays optional, and this pins the honest fallback for the
    // day that invariant changes.
    const model = configuredModel();
    model.effective_chat.policy = { ...model.effective_chat.policy, model: null };
    mockFetchConfig.mockResolvedValue(model);
    render(<ChatView />);
    await acceptConsent(user);

    const band = (await screen.findByText("CHAT")).closest("section");
    expect(band).toHaveTextContent("openai · openrouter.ai · no model configured");
    expect(band?.textContent).not.toContain("undefined");
  });

  it("keeps the budget figures out of the empty-thread greeting, without leaving it empty or duplicating them", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    render(<ChatView />);
    await acceptConsent(user);

    const hint = await screen.findByText(/What would you like to know about this project\?/);
    expect(hint.textContent).not.toMatch(/tool call/);
    expect(hint.textContent).not.toMatch(/replan/);
    expect(hint.textContent?.trim().length).toBeGreaterThan(0);
    // The bounds render exactly once on the page — in the band, not duplicated.
    expect(screen.getAllByText(/24 tool calls/)).toHaveLength(1);
  });
});

describe("ChatView — configure-first", () => {
  it("renders the honest configure-first state with no composer", async () => {
    mockFetchConfig.mockResolvedValue(unconfiguredModel());
    render(<ChatView />);
    expect(await screen.findByText(/Chat is not configured yet/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Config" })).toHaveAttribute("href", "/config");
    expect(screen.queryByRole("button", { name: "Send" })).not.toBeInTheDocument();
  });
});

describe("ChatView — configure-first names the root, the absent half and the origin (S-452)", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    setScopedMember(null);
    window.history.replaceState({}, "", "/");
  });

  /** Render under the real shell provider over a stubbed two-member roster, opened
   *  on `web` — NOT the manifest default (`api`), so a verdict that fell back to
   *  the default member could not pass. */
  async function renderInWorkspace(model: ChatConfigReadModel) {
    stubApi();
    window.history.replaceState({}, "", "/chat?repo=web");
    mockFetchConfig.mockResolvedValue(model);
    render(
      <WorkspaceProvider>
        <ChatView />
      </WorkspaceProvider>,
    );
    return screen.findByText(/Chat is not configured yet for web —/);
  }

  it("single-root: names this repository, never a member, as a muted status advisory", async () => {
    mockFetchConfig.mockResolvedValue(unconfiguredModel("key"));
    render(<ChatView />);
    const summary = await screen.findByText(
      "Chat is not configured yet for this repository — no API key is declared.",
    );
    expect(screen.getByText("The provider model is declared by this repository.")).toBeInTheDocument();
    // A muted advisory with a status role — not an error (FR-UI-18).
    const advisory = summary.closest("section");
    expect(advisory).toHaveAttribute("role", "status");
    expect(advisory?.className).toMatch(/_muted_/);
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(advisory?.textContent).not.toMatch(/workspace/i);
  });

  it("workspace: names the selected member, the absent half and the inherited origin", async () => {
    const summary = await renderInWorkspace(unconfiguredModel("model", "workspace"));
    expect(summary).toHaveTextContent(
      "Chat is not configured yet for web — no provider model is declared by web or by the workspace root.",
    );
    expect(screen.getByText("The API key is inherited from the workspace root.")).toBeInTheDocument();
    // Never the manifest default member, which is what an unscoped read answers from.
    expect(summary.closest("section")?.textContent).not.toMatch(/\bapi\b(?! key)/i);
  });

  it("workspace: names a member key withheld from the inherited endpoint, and how to use it (HF-1)", async () => {
    const m = unconfiguredModel("key", "workspace");
    m.effective_chat.member_key_withheld = true;
    const summary = await renderInWorkspace(m);
    expect(summary).toHaveTextContent(
      "Chat is not configured yet for web — no API key is declared by the workspace root.",
    );
    expect(screen.getByText("The provider model is inherited from the workspace root.")).toBeInTheDocument();
    expect(
      screen.getByText(
        "The API key web declares is not used with the inherited workspace endpoint — setting a [chat] model on web makes it use its own key.",
      ),
    ).toBeInTheDocument();
    // The action is the member's own model — adding a member key would still be withheld.
    const advisory = summary.closest("section");
    expect(advisory).toHaveTextContent("Choose a provider model in the Config tab for web");
    expect(advisory).not.toHaveTextContent("Add an API key");
    expect(advisory).toHaveTextContent(
      "Or declare an API key once for every member of the workspace, in <workspace-root>/.logos/secrets.toml.",
    );
    expect(screen.getByText("<workspace-root>/.logos/secrets.toml").tagName).toBe("CODE");
    expect(advisory).toHaveTextContent("The workspace Config view edits that file.");
  });

  it("workspace: links THIS member's Config tab, and the workspace Config view for the workspace half", async () => {
    await renderInWorkspace(unconfiguredModel("both"));
    const link = screen.getByRole("link", { name: "Config" });
    expect(link).toHaveAttribute("href", "/config?repo=web");
    // S-451 built the workspace-tier editor, so the workspace half is a link to it —
    // carrying this member, so coming back reopens this member's chat.
    const workspace = screen.getByRole("link", { name: "workspace Config" });
    expect(workspace).toHaveAttribute("href", "/workspace-config?repo=web");
    expect(screen.getAllByRole("link")).toHaveLength(2);
    // The files that editor writes are still named beside it.
    const config = screen.getByText("<workspace-root>/.logos/config.toml");
    const secrets = screen.getByText("<workspace-root>/.logos/secrets.toml");
    expect(config.tagName).toBe("CODE");
    expect(secrets.tagName).toBe("CODE");
    expect(workspace.closest("p")).toHaveTextContent(
      "Or declare them once for every member of the workspace, in <workspace-root>/.logos/config.toml and <workspace-root>/.logos/secrets.toml. The workspace Config view edits both files.",
    );
  });
});

describe("ChatView — config read failure names the repair surface (HF-2, Sprint 77 review option 4i)", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    setScopedMember(null);
    window.history.replaceState({}, "", "/");
  });

  it("single-root: an honest error panel with no workspace link", async () => {
    mockFetchConfig.mockRejectedValue(new ApiError("config", 500));
    render(<ChatView />);
    expect(await screen.findByText(/HTTP 500/)).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "workspace Config" })).not.toBeInTheDocument();
  });

  it("workspace mode, the workspace-tier file broken: links the workspace Config view, preserving ?repo=", async () => {
    stubApi();
    window.history.replaceState({}, "", "/chat?repo=web");
    mockFetchConfig.mockRejectedValue(new ApiError("config", 500));
    render(
      <WorkspaceProvider>
        <ChatView />
      </WorkspaceProvider>,
    );
    expect(await screen.findByText(/HTTP 500/)).toBeInTheDocument();
    const link = await screen.findByRole("link", { name: "workspace Config" });
    expect(link).toHaveAttribute("href", "/workspace-config?repo=web");
  });

  it("workspace mode, a member-file fault: the same link and wording — the 500 body cannot tell the two apart", async () => {
    // The read fails identically whether the fault is this member's own file or the
    // inherited workspace-tier file (S-450); `ApiError` carries no body at all, so
    // there is no field to branch on either way (see `workspaceConfigRepairHref`).
    stubApi();
    window.history.replaceState({}, "", "/chat?repo=web");
    mockFetchConfig.mockRejectedValue(new ApiError("config", 500));
    render(
      <WorkspaceProvider>
        <ChatView />
      </WorkspaceProvider>,
    );
    const panel = await screen.findByRole("alert");
    expect(panel).toHaveTextContent(/this member.s own root/);
    expect(panel).toHaveTextContent(/the workspace root/);
    expect(panel).toHaveTextContent("<workspace-root>/.logos/config.toml");
    expect(panel).toHaveTextContent("<workspace-root>/.logos/secrets.toml");
    // The linked view repairs an invalid config.toml in place but NOT an unreadable
    // secrets.toml (`UnreadableTierSecret`: "Repair or remove … by hand"), so the
    // note must not promise it repairs both (S-451 story re-review).
    expect(panel).toHaveTextContent(
      "repairs an invalid <workspace-root>/.logos/config.toml in place; an unreadable <workspace-root>/.logos/secrets.toml is repaired by hand.",
    );
    expect(panel).not.toHaveTextContent(/repair the workspace-root files/);
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
    expect(mockStreamTurn).toHaveBeenCalledWith(MEMBER_CHAT_ROUTES, "what is risky?", null, "default", expect.anything());
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

  it("renders a mermaid fence in the finalized answer as a diagram (S-302, FR-UI-32)", async () => {
    const user = userEvent.setup();
    paintDiagrams();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockStreamTurn.mockResolvedValue(
      sseResponse([
        'event: final_answer\ndata: {"answer":"Shape:\\n\\n```mermaid\\ngraph LR;\\n  a-->b\\n```\\n"}\n\n',
      ]),
    );
    render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "draw it");

    await waitFor(() =>
      expect(document.querySelector(".mermaid svg")).not.toBeNull(),
    );
    // The diagram carries its own controls; the shared Copy control still copies the
    // raw fence source, not the SVG.
    expect(screen.getByRole("button", { name: "Zoom in" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Show source" })).toBeInTheDocument();
    const writeText = vi.spyOn(navigator.clipboard, "writeText").mockResolvedValue();
    await user.click(screen.getByRole("button", { name: "Copy code" }));
    expect(writeText).toHaveBeenCalledWith("graph LR;\n  a-->b");
  });

  it("renders a mermaid fence inside an Activity step result too (S-301 + S-302)", async () => {
    const user = userEvent.setup();
    paintDiagrams();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockStreamTurn.mockResolvedValue(
      sseResponse([
        'event: step_started\ndata: {"index":0,"role":"graph_navigator","instruction":"map callers"}\n\n',
        'event: step_observed\ndata: {"index":0,"role":"graph_navigator","summary":"```mermaid\\ngraph TD;\\n  x-->y\\n```"}\n\n',
        'event: final_answer\ndata: {"answer":"Done."}\n\n',
      ]),
    );
    render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "who calls it?");

    // The step result renders through the SAME MarkdownAnswer, so the diagram is
    // drawn inside the Activity fold without a second integration point. The fold
    // auto-collapses onto the answer, so open it to read the step.
    await screen.findByText("Done.");
    const fold = screen.getByText("Activity").closest("details")!;
    await user.click(screen.getByText("Activity"));
    expect(fold.open).toBe(true);
    // Assert a DIAGRAM, scoped to the fold — a native <details> keeps its children
    // mounted even while collapsed, so querying the whole document (or asserting the
    // fallback source) would pass without the diagram ever reaching this surface.
    await waitFor(() => expect(fold.querySelector(".mermaid svg")).not.toBeNull());
    expect(fold.querySelector(".mermaid")?.getAttribute("data-processed")).toBe("true");
  });

  it("renders a GFM table as a real <table> inside the scroll wrapper, on both surfaces (S-311, FR-UI-31/32)", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    const table = "| A | B |\n| --- | --- |\n| 1 | 2 |";
    mockStreamTurn.mockResolvedValue(
      sseResponse([
        'event: step_started\ndata: {"index":0,"role":"source_reader","instruction":"read"}\n\n',
        `event: step_observed\ndata: {"index":0,"role":"source_reader","summary":${JSON.stringify(table)}}\n\n`,
        `event: final_answer\ndata: {"answer":${JSON.stringify(table)}}\n\n`,
      ]),
    );
    render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "tabulate it");

    // The finalized answer body: a real <table> (semantic markup, not a div-grid),
    // mounted inside the shared `.tableWrapper` scroll box rather than bare
    // user-agent defaults.
    await waitFor(() => {
      const wrapper = document.querySelector(`.${chatStyles.tableWrapper}`);
      expect(wrapper).not.toBeNull();
      expect(wrapper!.querySelector("table")).not.toBeNull();
    });

    // The SAME renderer draws the Activity step result's table on the OTHER
    // surface — scoped to the fold, which a native <details> keeps its children
    // mounted even while collapsed, so querying the whole document would pass
    // without the table ever reaching this surface.
    const fold = screen.getByText("Activity").closest("details")!;
    await user.click(screen.getByText("Activity"));
    expect(fold.open).toBe(true);
    const foldWrapper = fold.querySelector(`.${chatStyles.tableWrapper}`);
    expect(foldWrapper).not.toBeNull();
    expect(foldWrapper!.querySelector("table")).not.toBeNull();
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
    // message is preserved — a replace, never an append (FR-UI-20). No conversation
    // was adopted here, so there was nothing to cut in the store (S-624's cut is
    // pinned in the S-624 suite below).
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

describe("ChatView — a silent stream (S-626, FR-UI-50)", () => {
  // The notice text is pinned literally, not through the constant, so a change to
  // the wording or the threshold fails here.
  const NOTICE = "No response from the server for 45 s. The connection may be lost.";
  const KEEPALIVE = ": keepalive\n\n";

  afterEach(() => {
    vi.useRealTimers();
  });

  /** Mount a consented chat with the fake clock installed BEFORE any send, so every
   *  turn's watchdog is a fake timer. */
  async function mountWithFakeClock() {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    mockFetchConfig.mockResolvedValue(configuredModel());
    render(<ChatView />);
    await acceptConsent(user);
    return user;
  }

  /** Send a turn whose stream opens with a plan and then stays open. */
  async function openSilentTurn() {
    const user = await mountWithFakeClock();
    const pending = pendingSseResponse([
      'event: plan\ndata: {"round":0,"steps":[{"role":"graph_navigator","instruction":"map callers"}]}\n\n',
    ]);
    mockStreamTurn.mockResolvedValueOnce(pending.response);
    await ask(user, "q");
    expect(await screen.findByText("Working…")).toBeInTheDocument();
    return { user, pending };
  }

  it("never raises the notice once a turn has ended — answered or refused at the start", async () => {
    const user = await mountWithFakeClock();
    mockStreamTurn.mockResolvedValueOnce(sseResponse(['event: final_answer\ndata: {"answer":"done"}\n\n']));
    await ask(user, "q");
    expect(await screen.findByText("done")).toBeInTheDocument();

    mockStreamTurn.mockResolvedValueOnce({ ok: false, status: 503, body: null } as unknown as Response);
    await ask(user, "again");
    expect(await screen.findByText("The chat turn could not start (status 503).")).toBeInTheDocument();

    // Both turns are over, so their watchdogs are disarmed: a minute of silence is
    // not a stalled stream.
    await vi.advanceTimersByTimeAsync(60_000);
    expect(screen.queryByText(NOTICE)).not.toBeInTheDocument();
  });

  it("never shows the notice while keepalive comments arrive every 15 s for 120 s", async () => {
    const { pending } = await openSilentTurn();
    for (let elapsed = 0; elapsed < 120_000; elapsed += 15_000) {
      await vi.advanceTimersByTimeAsync(15_000);
      pending.push(KEEPALIVE);
    }
    await vi.advanceTimersByTimeAsync(30_000); // 30 s past the last comment: still inside 45 s
    expect(screen.queryByText(NOTICE)).not.toBeInTheDocument();
    expect(screen.getByText("Working…")).toBeInTheDocument();
    pending.close();
  });

  it("shows the notice with Retry and Stop after 45 s with no byte, and a later byte clears it", async () => {
    const { pending } = await openSilentTurn();
    await vi.advanceTimersByTimeAsync(44_000);
    expect(screen.queryByText(NOTICE)).not.toBeInTheDocument();

    await vi.advanceTimersByTimeAsync(1_000);
    const notice = screen.getByRole("alert");
    expect(notice).toHaveTextContent(NOTICE);
    expect(within(notice).getByRole("button", { name: "Retry" })).toBeInTheDocument();
    expect(within(notice).getByRole("button", { name: "Stop" })).toBeInTheDocument();
    // "Working…" would be the dishonest state the notice replaces.
    expect(screen.queryByText("Working…")).not.toBeInTheDocument();

    // A keepalive is a byte, not an event — it clears the notice all the same.
    pending.push(KEEPALIVE);
    await waitFor(() => expect(screen.queryByText(NOTICE)).not.toBeInTheDocument());
    expect(screen.getByText("Working…")).toBeInTheDocument();
    pending.close();
  });

  it("counts the response head as liveness: the 45 s run from the last byte, head included", async () => {
    const user = await mountWithFakeClock();
    const pending = pendingSseResponse([]);
    // The head arrives 40 s after the send, and the body then carries nothing.
    mockStreamTurn.mockImplementationOnce(
      () => new Promise<Response>((resolve) => setTimeout(() => resolve(pending.response), 40_000)),
    );
    await ask(user, "q");
    await vi.advanceTimersByTimeAsync(40_000);
    // 10 s after the head (50 s after the send): the head reset the clock.
    await vi.advanceTimersByTimeAsync(10_000);
    expect(screen.queryByText(NOTICE)).not.toBeInTheDocument();
    // 45 s after the head: silence since the last byte trips it.
    await vi.advanceTimersByTimeAsync(35_000);
    expect(screen.getByRole("alert")).toHaveTextContent(NOTICE);
    pending.close();
  });

  it("marks only the stalled turn, never an earlier finished one", async () => {
    const user = await mountWithFakeClock();
    mockStreamTurn.mockResolvedValueOnce(sseResponse(['event: final_answer\ndata: {"answer":"first"}\n\n']));
    await ask(user, "one");
    expect(await screen.findByText("first")).toBeInTheDocument();

    const pending = pendingSseResponse([]);
    mockStreamTurn.mockResolvedValueOnce(pending.response);
    await ask(user, "two");
    await vi.advanceTimersByTimeAsync(45_000);

    // One notice, on the silent turn — the finished first answer carries none.
    expect(screen.getAllByRole("alert")).toHaveLength(1);
    pending.close();
  });

  it("Retry re-runs the turn in place once the server answers again", async () => {
    const { user, pending } = await openSilentTurn();
    await vi.advanceTimersByTimeAsync(45_000);
    const notice = screen.getByRole("alert");

    mockStreamTurn.mockResolvedValueOnce(
      sseResponse(['event: final_answer\ndata: {"answer":"back again"}\n\n']),
    );
    await user.click(within(notice).getByRole("button", { name: "Retry" }));

    expect(await screen.findByText("back again")).toBeInTheDocument();
    expect(screen.queryByText(NOTICE)).not.toBeInTheDocument();
    // The same question, re-run once; the stalled turn was aborted and replaced,
    // never duplicated.
    expect(mockStreamTurn).toHaveBeenCalledTimes(2);
    expect(mockStreamTurn.mock.calls[1][1]).toBe("q");
    expect((mockStreamTurn.mock.calls[0][4] as AbortSignal).aborted).toBe(true);
    expect(screen.getAllByText("q")).toHaveLength(1);
    pending.close();
  });

  it("Retry of a first send adopts the conversation it creates, so the next send appends to it", async () => {
    const { user, pending } = await openSilentTurn();
    await vi.advanceTimersByTimeAsync(45_000);

    // The stalled first send created thread 1 server-side but never settled, so no
    // conversation was adopted; the retried turn creates thread 2 and answers there.
    mockFetchThreads.mockResolvedValue([thread(2, "q", 20), thread(1, "q", 10)]);
    mockStreamTurn.mockResolvedValueOnce(
      sseResponse(['event: final_answer\ndata: {"answer":"back again"}\n\n']),
    );
    await user.click(within(screen.getByRole("alert")).getByRole("button", { name: "Retry" }));
    expect(await screen.findByText("back again")).toBeInTheDocument();

    mockStreamTurn.mockResolvedValueOnce(
      sseResponse(['event: final_answer\ndata: {"answer":"follow-up answer"}\n\n']),
    );
    await screen.findByRole("button", { name: "Send" }); // the retried turn has settled
    await ask(user, "and then?");
    expect(await screen.findByText("follow-up answer")).toBeInTheDocument();
    // The follow-up carries the retried turn's conversation — not null, which would
    // fork a third thread.
    expect(mockStreamTurn.mock.calls[2][2]).toBe(2);
    pending.close();
  });

  it("Stop ends the stalled turn", async () => {
    const { user, pending } = await openSilentTurn();
    await vi.advanceTimersByTimeAsync(45_000);
    const notice = screen.getByRole("alert");

    await user.click(within(notice).getByRole("button", { name: "Stop" }));

    expect(await screen.findByRole("button", { name: "Send" })).toBeInTheDocument();
    await waitFor(() => expect(screen.queryByText(NOTICE)).not.toBeInTheDocument());
    // A stopped turn claims no progress: "Working…" does not come back in its place.
    expect(screen.queryByText("Working…")).not.toBeInTheDocument();
    expect((mockStreamTurn.mock.calls[0][4] as AbortSignal).aborted).toBe(true);
    expect(mockStreamTurn).toHaveBeenCalledTimes(1);
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
    expect(mockDeleteThread).toHaveBeenCalledWith(MEMBER_CHAT_ROUTES, 3);
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
    await waitFor(() => expect(window.localStorage.getItem("logos.chat.activeThread:single")).toBeNull());
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
    expect(window.localStorage.getItem("logos.chat.activeThread:single")).toBe("5");
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
    expect(mockDeleteThread).toHaveBeenCalledWith(MEMBER_CHAT_ROUTES, 5);
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
    await waitFor(() => expect(window.localStorage.getItem("logos.chat.activeThread:single")).toBeNull());
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
    await user.click(screen.getByRole("button", { name: "New chat" }));

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
    const [, question] = mockStreamTurn.mock.calls[0];
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
    expect(mockFetchMessages).toHaveBeenCalledWith(MEMBER_CHAT_ROUTES, 5);
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

    await user.click(screen.getByRole("button", { name: "New chat" }));
    // The log is cleared to a fresh composer…
    await waitFor(() => expect(screen.queryByText("old answer")).not.toBeInTheDocument());
    expect(screen.getByRole("button", { name: "Send" })).toBeInTheDocument();
    // …and no turn was streamed and no new thread was created (no empty rows): the
    // rail still shows exactly the one server-side conversation.
    expect(mockStreamTurn).not.toHaveBeenCalled();
    expect(screen.getAllByRole("button", { name: "Conv A" })).toHaveLength(1);
    // The stored selection is cleared, so a reload lands on a fresh composer.
    await waitFor(() => expect(window.localStorage.getItem("logos.chat.activeThread:single")).toBeNull());
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
    expect(mockStreamTurn.mock.calls[0][2]).toBeNull();
    // The just-created conversation is adopted and appears in the rail.
    expect(await screen.findByRole("button", { name: "first question" })).toBeInTheDocument();

    await ask(user, "second turn");
    await screen.findByText("second answer");
    // The follow-up turn continues the SAME server thread (id 9), not a new one.
    expect(mockStreamTurn.mock.calls[1][2]).toBe(9);
  });

  it("restores the last-open conversation across a reload (persisted selection)", async () => {
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockFetchThreads.mockResolvedValue([thread(7, "Persisted conv", 500)]);
    mockFetchMessages.mockResolvedValue([
      persisted("user", "remembered question", 1),
      persisted("assistant", "remembered answer", 2),
    ]);
    // A prior session left thread 7 open.
    window.localStorage.setItem("logos.chat.activeThread:single", "7");
    render(<ChatView />);
    // On mount the runtime re-hydrates the stored selection with no user action.
    expect(await screen.findByText("remembered answer")).toBeInTheDocument();
    expect(screen.getByText("remembered question")).toBeInTheDocument();
    expect(mockFetchMessages).toHaveBeenCalledWith(MEMBER_CHAT_ROUTES, 7);
  });

  it("falls back to a fresh composer when the stored selection was deleted (404)", async () => {
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockFetchThreads.mockResolvedValue([]);
    mockFetchMessages.mockRejectedValue(new ApiError("chat/threads/7", 404));
    window.localStorage.setItem("logos.chat.activeThread:single", "7");
    render(<ChatView />);
    // No crash, no restored history — an honest empty composer.
    expect(await screen.findByRole("button", { name: "Send" })).toBeInTheDocument();
    // The stale stored selection is cleared.
    await waitFor(() => expect(window.localStorage.getItem("logos.chat.activeThread:single")).toBeNull());
  });

  // The toggle's own behaviour — narrow-start-collapsed (AC-3) included — moved to
  // the S-619 suite below, which shows it at every width and remembers it per scope.
});

describe("ChatView — rail honest error paths (S-210)", () => {
  it("shows an honest note when the conversation list fails to load", async () => {
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockFetchThreads.mockRejectedValue(new ApiError("chat/threads", 500));
    render(<ChatView />);
    // The rail degrades to an honest note (not a silently empty list), and the chat
    // surface still mounts.
    expect(await screen.findByText(/Could not load your conversations/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "New chat" })).toBeInTheDocument();
  });

  it("keeps the open conversation and notes a fault when a restore faults (non-404)", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockFetchThreads.mockResolvedValue([thread(5, "Conv A", 300), thread(3, "Conv B", 100)]);
    mockFetchMessages.mockImplementation((_routes, id: number) =>
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
    expect(mockStreamTurn.mock.calls[0][2]).toBeNull();
    expect(mockStreamTurn.mock.calls[1][2]).toBe(9);
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
    await user.click(screen.getByRole("button", { name: "New chat" }));
    pending.close();

    // The fresh session must NOT be rebound to thread 9 by the aborted turn's
    // reconcile: the next send creates a new conversation (thread id null).
    mockStreamTurn.mockResolvedValueOnce(sseResponse(['event: final_answer\ndata: {"answer":"fresh"}\n\n']));
    await ask(user, "brand new");
    await screen.findByText("fresh");
    expect(mockStreamTurn.mock.calls[mockStreamTurn.mock.calls.length - 1][2]).toBeNull();
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
    expect(mockStreamTurn).toHaveBeenCalledWith(MEMBER_CHAT_ROUTES, "still works?", null, "default", expect.anything());
  });
});

describe("ChatView — transcript column structure (S-300, FR-UI-31)", () => {
  it("nests the user turn's text in a bubble element inside the column row", async () => {
    // S-300 made the user MessagePrimitive.Root the full-width transcript row and
    // moved the bubble treatment onto an inner element, so the bubble hugs the
    // TRANSCRIPT's right edge rather than the viewport's. That separation is a DOM
    // fact jsdom can see; the declared CSS itself is asserted in
    // `web/tests/spa_design_system.rs` (under `css: false` a CSS-module import is
    // a proxy that names ANY key, so only class PLACEMENT is visible here, never a
    // rule's definition). Without this guard, deleting the wrapper would break the
    // layout in every theme and no test would notice.
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

describe("ChatView — the two-pane card grammar (S-308, FR-UI-33)", () => {
  // Class PLACEMENT is what these assert: under `css: false` each CSS-module
  // import is a proxy that returns a generated name for any key, so `classList`
  // carries real tokens. Whether a class is DEFINED, and what it declares, is the
  // Rust `spa_design_system` suite's job.

  it("renders the transcript and the composer inside the view's ONE Card, which wraps no turn", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockStreamTurn.mockResolvedValue(sseResponse(['event: final_answer\ndata: {"answer":"inside"}\n\n']));
    const { container } = render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "where is the card?");
    const answer = await screen.findByText("inside");

    // Exactly one card on the view — the edge appears once per view, never once
    // per answer — and it holds BOTH turns and the composer.
    const cards = container.querySelectorAll(`.${cardStyles.card}`);
    expect(cards).toHaveLength(1);
    const card = cards[0]!;
    expect(card.contains(answer)).toBe(true);
    expect(card.contains(screen.getByText("where is the card?"))).toBe(true);
    expect(card.contains(screen.getByRole("textbox", { name: "Your message" }))).toBe(true);

    // The card's content is the thread, and the transcript inside it is the log
    // viewport — the bounded scroll box assistant-ui drives.
    const log = container.querySelector(`.${chatStyles.log}`);
    expect(log).not.toBeNull();
    expect(card.contains(log)).toBe(true);
    expect(log!.contains(answer)).toBe(true);

    // The rail is NOT in the card: the card is the conversation pane only.
    expect(card.contains(screen.getByRole("navigation", { name: "Conversations" }))).toBe(false);
  });

  it("gives the rail a full-size iconed New chat pinned outside the scrolling list", async () => {
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockFetchThreads.mockResolvedValue([thread(2, "Conv B", 200), thread(1, "Conv A", 100)]);
    render(<ChatView />);
    await screen.findByRole("button", { name: "Conv B" });

    const newChat = screen.getByRole("button", { name: "New chat" });
    // The shared Button at `md` — the `sm` control measured 23px tall.
    expect(newChat.classList).toContain(buttonStyles.md);
    expect(newChat.classList).not.toContain(buttonStyles.sm);
    expect(newChat.classList).toContain(chatStyles.newChat);
    // A LEADING, decorative icon: the first child is the aria-hidden glyph, so the
    // accessible name stays the text.
    const icon = newChat.firstElementChild;
    expect(icon?.tagName.toLowerCase()).toBe("svg");
    expect(icon).toHaveAttribute("aria-hidden", "true");

    // Pinned: the action is a direct child of the rail, OUTSIDE the list that
    // scrolls — so the list can scroll without taking the action with it.
    const rail = screen.getByRole("navigation", { name: "Conversations" });
    const list = screen.getByRole("list");
    expect(list.classList).toContain(chatStyles.threadList);
    expect(newChat.parentElement).toBe(rail);
    expect(list.contains(newChat)).toBe(false);
    expect(rail.firstElementChild).toBe(newChat);
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

  it("omits the plan caption when the fold renders but the plan is step-less", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    // The no-fold gate short-circuits on plan AND chips, so it can never reach
    // PlanList's own guard. Here a step DID run, so the fold renders — and
    // PlanList's guard is the only thing standing between a malformed plan frame
    // (reduced to a PRESENT plan with zero steps) and a "Plan" caption sitting
    // over an empty list inside an otherwise legitimate fold.
    mockStreamTurn.mockResolvedValue(
      sseResponse([
        'event: step_started\ndata: {"index":0,"role":"source_reader","instruction":"read it"}\n\n',
        'event: step_observed\ndata: {"index":0,"role":"source_reader","summary":"read it all"}\n\n',
        'event: plan\ndata: {"round":0,"steps":"not-an-array"}\n\n',
        'event: final_answer\ndata: {"answer":"answered"}\n\n',
      ]),
    );
    const { container } = render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "q");
    expect(await screen.findByText("answered")).toBeInTheDocument();

    const activity = fold(container)!;
    expect(activity).not.toBeNull();
    // The step is there…
    expect(activity.textContent).toContain("Source-Reader");
    expect(activity.textContent).toContain("read it all");
    // …and the plan section is absent entirely, caption included.
    expect(activity.textContent).not.toContain("Plan");
  });

  it("pairs each status glyph with a text equivalent, so state never rides on colour", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    // STREAMING_FRAMES leaves step 0 observed and step 1 running, so both states
    // are on screen at once.
    const pending = pendingSseResponse(STREAMING_FRAMES);
    mockStreamTurn.mockResolvedValue(pending.response);
    const { container } = render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "who calls it?");
    await waitFor(() => expect(container.textContent).toContain("Synthesizer"));

    const activity = fold(container)!;
    // The glyph differs in SHAPE, not just hue — and it is decorative, so it is
    // hidden from assistive tech…
    const glyphs = [...activity.querySelectorAll('[aria-hidden="true"]')].map((g) => g.textContent);
    expect(glyphs).toEqual(["✓", "▸"]);
    // …with the state spoken as words instead ([NFR-CC-04]: an honest state, never
    // one carried by colour alone).
    const spoken = [...activity.querySelectorAll(".sr-only")].map((s) => s.textContent);
    expect(spoken).toEqual(["done:", "running:"]);
    pending.close();
  });

  it("renders the flat, ungrouped layout for a single round — no grouping wrapper (AC4, CR-089 baseline, review-fix S-303)", async () => {
    // A text-content assertion alone cannot tell "the flat layout" apart from "the
    // grouped-by-round layout rendering identical text for one round" — both would
    // pass every OTHER test in this describe block. This one checks the STRUCTURE:
    // with `css: false` hiding class names, the flat layout nests the plan's `<ol>`
    // two `<div>`s below the fold (`.activityBody` → `.plan`); the grouped layout
    // (ActivityByRound) would nest it FOUR deep (adding `.activityRounds` and
    // `.activityRound`). Counting div ancestors between the `<ol>` and the fold is
    // therefore a structural proof that no grouping chrome renders for one round,
    // not just that the same words happen to appear either way.
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    const pending = pendingSseResponse(STREAMING_FRAMES);
    mockStreamTurn.mockResolvedValue(pending.response);
    const { container } = render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "who calls it?");
    await waitFor(() => expect(container.textContent).toContain("Synthesizer"));

    const activity = fold(container)!;
    const planOl = activity.querySelector("ol")!; // the plan's <ol>, rendered before the steps' <ol>
    let divAncestors = 0;
    for (let el = planOl.parentElement; el && el !== activity; el = el.parentElement) {
      if (el.tagName === "DIV") divAncestors++;
    }
    expect(divAncestors).toBe(2);
    pending.close();
  });

  it("shows a round's plan even when replanned before any of its steps started, and never drops it from the grouped fold (S-303, CR-090)", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    // Round 0 is planned and then immediately superseded — no step of round 0 ever
    // starts, so round 0 has a real (non-empty) plan but ZERO chips. Only round 1
    // actually runs.
    mockStreamTurn.mockResolvedValue(
      sseResponse([
        'event: plan\ndata: {"round":0,"steps":[{"role":"graph_navigator","instruction":"map callers"}]}\n\n',
        'event: plan\ndata: {"round":1,"steps":[{"role":"source_reader","instruction":"read file"}]}\n\n',
        'event: step_started\ndata: {"index":0,"role":"source_reader","instruction":"read file"}\n\n',
        'event: step_observed\ndata: {"index":0,"role":"source_reader","summary":"read"}\n\n',
        'event: final_answer\ndata: {"answer":"done"}\n\n',
      ]),
    );
    const { container } = render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "q");
    expect(await screen.findByText("done")).toBeInTheDocument();

    const activity = fold(container)!;
    // Round 0's plan renders on the strength of `turn.plans` ALONE — it has no
    // chip to contribute its round via `turn.chips`. Dropping the `plans` half of
    // `activityRounds()`'s round union would silently drop this whole group.
    expect(activity.textContent).toContain("Plan");
    expect(activity.textContent).toContain("Graph-Navigator");
    expect(activity.textContent).toContain("map callers");
    expect(activity.textContent).toContain("Revised plan");
  });

  it("keeps round 0's plan beside its steps when the replan carries zero steps (S-303, sprint-78 review)", async () => {
    // `activityRounds()` drops a zero-step round, so this turn has ONE renderable
    // round and takes the flat branch — which must render THAT round's plan, not
    // `turn.plan` (the latest, empty replan). Before the fix the fold showed round
    // 0's step with no plan above it.
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockStreamTurn.mockResolvedValue(
      sseResponse([
        'event: plan\ndata: {"round":0,"steps":[{"role":"graph_navigator","instruction":"map callers"}]}\n\n',
        'event: step_started\ndata: {"index":0,"role":"graph_navigator","instruction":"map callers"}\n\n',
        'event: step_observed\ndata: {"index":0,"role":"graph_navigator","summary":"found callers"}\n\n',
        'event: plan\ndata: {"round":1,"steps":[]}\n\n',
        'event: final_answer\ndata: {"answer":"done"}\n\n',
      ]),
    );
    const { container } = render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "q");
    expect(await screen.findByText("done")).toBeInTheDocument();

    const activity = fold(container)!;
    expect(activity.querySelectorAll("ol")).toHaveLength(2); // the plan's list and the steps' list
    expect(activity.textContent).toContain("Plan");
    expect(activity.textContent).not.toContain("Revised plan");
    expect(activity.textContent).toContain("found callers");
  });

  it("still shows round 0's plan when it is replanned to zero steps before any step started (S-303, sprint-78 review)", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockStreamTurn.mockResolvedValue(
      sseResponse([
        'event: plan\ndata: {"round":0,"steps":[{"role":"graph_navigator","instruction":"map callers"}]}\n\n',
        'event: plan\ndata: {"round":1,"steps":[]}\n\n',
        'event: final_answer\ndata: {"answer":"done"}\n\n',
      ]),
    );
    const { container } = render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "q");
    expect(await screen.findByText("done")).toBeInTheDocument();

    const activity = fold(container);
    expect(activity).not.toBeNull();
    expect(activity!.textContent).toContain("map callers");
  });

  it("groups a replanned turn's fold by round, keeps colliding indices apart, and renders no duplicate-key warning (S-303, CR-090)", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    // React's duplicate-key warning goes to console.error — silence it (jsdom would
    // otherwise print it as test noise) while still asserting it never fired.
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    // Round 0 plans one step (index 0) and starts it, but a replan supersedes it
    // BEFORE it is observed. Round 1 restarts `index` at 0 — the orchestrator's
    // per-round indexing (CR-090) — plans and starts ITS OWN index-0 step, and
    // that one alone is observed.
    mockStreamTurn.mockResolvedValue(
      sseResponse([
        'event: plan\ndata: {"round":0,"steps":[{"role":"graph_navigator","instruction":"map callers"}]}\n\n',
        'event: step_started\ndata: {"index":0,"role":"graph_navigator","instruction":"map callers"}\n\n',
        'event: plan\ndata: {"round":1,"steps":[{"role":"source_reader","instruction":"read file"}]}\n\n',
        'event: step_started\ndata: {"index":0,"role":"source_reader","instruction":"read file"}\n\n',
        'event: step_observed\ndata: {"index":0,"role":"source_reader","summary":"read file contents"}\n\n',
        'event: final_answer\ndata: {"answer":"done"}\n\n',
      ]),
    );
    const { container } = render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "q");
    expect(await screen.findByText("done")).toBeInTheDocument();

    const activity = fold(container)!;
    // Each round's OWN plan renders — grouped, not flattened into one list: round
    // 0's caption ("Plan") and round 1's ("Revised plan") both appear.
    expect(activity.textContent).toContain("Plan");
    expect(activity.textContent).toContain("Revised plan");
    expect(activity.textContent).toContain("Graph-Navigator");
    expect(activity.textContent).toContain("Source-Reader");

    // Round 0's step was never observed — it stays running, and round 1's
    // observation of the SAME index does not mark it done or lend it a summary.
    const glyphs = [...activity.querySelectorAll('[aria-hidden="true"]')].map((g) => g.textContent);
    expect(glyphs).toEqual(["▸", "✓"]);
    // The observed summary is attributed to round 1's step alone — exactly one
    // rendering of it, not two (which a round-blind match would produce by
    // updating both colliding-index chips).
    expect(activity.textContent!.split("read file contents")).toHaveLength(2);

    // No React "two children with the same key" warning — the (round, index)
    // keying keeps round 0's and round 1's index-0 chips distinct.
    const keyWarning = errorSpy.mock.calls.some((call) =>
      call.some((arg) => typeof arg === "string" && arg.includes("same key")),
    );
    expect(keyWarning).toBe(false);
    errorSpy.mockRestore();
  });
});
describe("ChatView — extra read roots (sprint-79 HF-1)", () => {
  function withReadRoots(model: ChatConfigReadModel, roots: string[]): ChatConfigReadModel {
    model.effective_chat.policy = { ...model.effective_chat.policy, read_roots: roots };
    return model;
  }

  it("names nothing extra when no read roots are declared (the default)", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    render(<ChatView />);
    const banner = (await screen.findByText(/source and graph excerpts/)).closest("section");
    expect(banner?.textContent).not.toMatch(/read roots/);
    await acceptConsent(user);
    const band = (await screen.findByText("CHAT")).closest("section");
    expect(band?.textContent).not.toMatch(/read roots/);
  });

  it("names every declared read root in the consent banner, before anything is sent", async () => {
    mockFetchConfig.mockResolvedValue(withReadRoots(configuredModel(), ["../logos-docs", "/srv/specs"]));
    render(<ChatView />);
    const banner = (await screen.findByText(/source and graph excerpts/)).closest("section");
    expect(banner).toHaveTextContent(
      "Extra read roots: ../logos-docs, /srv/specs — files under them, reached through this project's symlinks, can be sent too.",
    );
    expect(banner?.textContent).not.toMatch(/relative to the workspace root/);
  });

  it("asks again when read roots appear after an earlier consent, before anything is sent", async () => {
    const user = userEvent.setup();
    // Consented on a policy with no read roots…
    window.localStorage.setItem("logos.chat.consent:single", "1");
    mockFetchConfig.mockResolvedValue(withReadRoots(configuredModel(), ["../logos-docs"]));
    const first = render(<ChatView />);
    // …so the banner naming the new root is back, and the composer is gated.
    const banner = (await screen.findByText(/source and graph excerpts/)).closest("section");
    expect(banner).toHaveTextContent("Extra read roots: ../logos-docs");
    expect(screen.getByRole("textbox", { name: "Your message" })).toBeDisabled();
    await acceptConsent(user);
    first.unmount();

    // The same set is now covered: no banner on the next visit.
    render(<ChatView />);
    await screen.findByText("CHAT");
    expect(screen.queryByRole("button", { name: "Start chatting" })).not.toBeInTheDocument();
  });

  it("keeps naming them in the status band, and says an inherited table's roots are the workspace's", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(
      withReadRoots(configuredModel("openai", { policy: "workspace", credential: "workspace" }), ["../logos-docs"]),
    );
    render(<ChatView />);
    await acceptConsent(user);
    const band = (await screen.findByText("CHAT")).closest("section");
    expect(band).toHaveTextContent("Extra read roots: ../logos-docs (relative to the workspace root)");
  });
});

// ── S-485: the Workspace Chat, and scope-keyed client state ───────────────────

/** The workspace tier's chat slice: `member` = declared AT the workspace root. */
function workspaceTier(
  policy: ChatOrigin = "member",
  credential: ChatOrigin = "member",
): WorkspaceChatConfigReadModel {
  return { ...configuredModel("openai", { policy, credential }), config: { error: null }, chat_key_error: null };
}

/** A shell-like harness: the real provider over the stubbed two-member roster
 *  (`shop`: `api` default, `web`), a member switch control, and `children`. */
function MemberSwitch() {
  const { selectMember } = useWorkspace();
  return (
    <button type="button" onClick={() => selectMember("web")}>
      switch to web
    </button>
  );
}

/** The member chat mounted exactly as `App.tsx` mounts a member-scoped view: not
 *  until the probe settles, and keyed on the shell's cache key. */
function ShellKeyedChat() {
  const { cacheKey, mode } = useWorkspace();
  return mode === "loading" ? null : <ChatView key={cacheKey} />;
}

describe("WorkspaceChatView (S-485, FR-WS-34, frontend-design §4.22)", () => {
  beforeEach(() => {
    stubApi();
    window.history.replaceState({}, "", "/workspace-chat");
    mockFetchWorkspaceConfig.mockResolvedValue(workspaceTier());
    mockFetchReadRoots.mockResolvedValue([]);
  });
  afterEach(() => {
    vi.unstubAllGlobals();
    setScopedMember(null);
    window.history.replaceState({}, "", "/");
  });

  function renderWorkspaceChat() {
    return render(
      <WorkspaceProvider>
        <MemberSwitch />
        <WorkspaceChatView />
      </WorkspaceProvider>,
    );
  }

  it("names the workspace and its member count in its heading", async () => {
    renderWorkspaceChat();
    expect(
      await screen.findByRole("heading", { level: 1, name: "Workspace chat · shop · 2 members" }),
    ).toBeInTheDocument();
  });

  it("talks to the workspace chat's routes and reads no member's config", async () => {
    const user = userEvent.setup();
    mockStreamTurn.mockResolvedValue(sseResponse(['event: final_answer\ndata: {"answer":"ok"}\n\n']));
    renderWorkspaceChat();
    await acceptConsent(user);
    await ask(user, "which services call billing?");
    await screen.findByText("ok");
    expect(mockFetchThreads).toHaveBeenCalledWith(WORKSPACE_CHAT_ROUTES);
    expect(mockStreamTurn).toHaveBeenCalledWith(
      WORKSPACE_CHAT_ROUTES,
      "which services call billing?",
      null,
      "default",
      expect.anything(),
    );
    // The member chat's config read (`GET /api/v1/config?repo=…`) is never made:
    // it would start a member engine, and no member's [chat] drives this chat.
    expect(mockFetchConfig).not.toHaveBeenCalled();
    expect(mockStreamTurn.mock.calls.some((c) => c[0] === MEMBER_CHAT_ROUTES)).toBe(false);
  });

  it("does not remount or re-read on a member switch", async () => {
    const user = userEvent.setup();
    renderWorkspaceChat();
    const heading = await screen.findByRole("heading", { level: 1 });
    await screen.findByRole("button", { name: "Start chatting" });
    await user.click(screen.getByRole("button", { name: "switch to web" }));
    await waitFor(() => expect(window.location.search).toBe("?repo=web"));
    // Same element, same single read of each workspace resource.
    expect(screen.getByRole("heading", { level: 1 })).toBe(heading);
    expect(heading).toHaveTextContent("Workspace chat · shop · 2 members");
    expect(mockFetchWorkspaceConfig).toHaveBeenCalledTimes(1);
    expect(mockFetchReadRoots).toHaveBeenCalledTimes(1);
    expect(mockFetchThreads).toHaveBeenCalledTimes(1);
  });

  it("configure-first names the workspace root and links Workspace Config — never a member's Config", async () => {
    mockFetchWorkspaceConfig.mockResolvedValue(workspaceTier("member", "unset"));
    renderWorkspaceChat();
    const advisory = (await screen.findByText(/The workspace chat is not configured yet/)).closest("section");
    expect(advisory).toHaveTextContent(
      "The workspace chat is not configured yet for the workspace root of shop — no API key is declared there.",
    );
    expect(advisory).toHaveTextContent("A member's own [chat] does not configure the workspace chat.");
    expect(advisory).toHaveTextContent("<workspace-root>/.logos/secrets.toml");
    const link = screen.getByRole("link", { name: "Workspace Config" });
    expect(link.getAttribute("href")?.split("?")[0]).toBe("/workspace-config");
    expect(screen.queryByRole("link", { name: "Config" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Send" })).not.toBeInTheDocument();
  });

  it("names an unparsable tier as a fault, not as an unconfigured chat", async () => {
    mockFetchWorkspaceConfig.mockResolvedValue({
      effective_chat: null,
      config: { error: "config.toml line 2: expected `]`" },
      chat_key_error: null,
    });
    renderWorkspaceChat();
    expect(await screen.findByText(/could not be read: config\.toml line 2/)).toBeInTheDocument();
    expect(screen.queryByText(/not configured yet/)).toBeNull();
    expect(screen.getByRole("link", { name: "Workspace Config" })).toBeInTheDocument();
  });

  it("names an unreadable workspace secrets.toml on the page", async () => {
    mockFetchWorkspaceConfig.mockResolvedValue({
      effective_chat: null,
      config: { error: null },
      chat_key_error: "secrets.toml line 1: invalid key",
    });
    renderWorkspaceChat();
    expect(await screen.findByText(/could not be read: secrets\.toml line 1: invalid key/)).toBeInTheDocument();
    expect(screen.queryByText(/not configured yet/)).toBeNull();
  });

  it("says, on an empty rail, that member conversations live in a --standalone serve", async () => {
    renderWorkspaceChat();
    const rail = await screen.findByRole("navigation", { name: "Conversations" });
    await waitFor(() =>
      expect(rail).toHaveTextContent(
        "No workspace conversations yet. Member conversations are not listed here — each is available in a --standalone serve of that member.",
      ),
    );
    expect(within(rail).getByText("--standalone").tagName).toBe("CODE");
  });

  it("names every member's effective read roots in the consent banner, from the engine-free read-model", async () => {
    const roots: MemberChatReadRoots[] = [
      { name: "api", policy_origin: "member", declared_by: "member", read_roots: ["../api-docs"] },
      { name: "web", policy_origin: "workspace", declared_by: "workspace", read_roots: ["shared-docs"] },
    ];
    mockFetchReadRoots.mockResolvedValue(roots);
    renderWorkspaceChat();
    const banner = (await screen.findByText(/source and graph excerpts/)).closest("section");
    expect(banner).toHaveTextContent("from this workspace's members");
    expect(banner).toHaveTextContent("(the openai endpoint declared at the workspace root)");
    expect(banner).toHaveTextContent(
      "Extra read roots: shared-docs (relative to the workspace root; read by web); ../api-docs (relative to api) — files under them, reached through each member's symlinks, can be sent too.",
    );
  });

  it("asks again when a member's read roots change after the workspace consent (NFR-SE-07)", async () => {
    const user = userEvent.setup();
    const shared: MemberChatReadRoots = {
      name: "web",
      policy_origin: "workspace",
      declared_by: "workspace",
      read_roots: ["shared-docs"],
    };
    mockFetchReadRoots.mockResolvedValue([shared]);
    const first = renderWorkspaceChat();
    await acceptConsent(user);
    await screen.findByText("CHAT");
    first.unmount();

    // The same set: the consent covers it, no banner.
    const same = renderWorkspaceChat();
    await screen.findByText("CHAT");
    expect(screen.queryByRole("button", { name: "Start chatting" })).toBeNull();
    same.unmount();

    // A new root on a member: the banner is back, naming it, before anything is sent.
    mockFetchReadRoots.mockResolvedValue([
      shared,
      { name: "api", policy_origin: "member", declared_by: "member", read_roots: ["../api-docs"] },
    ]);
    const grown = renderWorkspaceChat();
    expect(await screen.findByRole("button", { name: "Start chatting" })).toBeInTheDocument();
    expect(screen.getByText(/source and graph excerpts/).closest("section")).toHaveTextContent(
      "../api-docs (relative to api)",
    );
    grown.unmount();

    // The same entry moving to another declaring root asks again too.
    mockFetchReadRoots.mockResolvedValue([{ ...shared, policy_origin: "member", declared_by: "member" }]);
    renderWorkspaceChat();
    expect(await screen.findByRole("button", { name: "Start chatting" })).toBeInTheDocument();
  });

  it("deletes a conversation from the workspace's own store, never a member's", async () => {
    const user = userEvent.setup();
    mockFetchThreads.mockResolvedValue([thread(4, "Workspace four", 100)]);
    renderWorkspaceChat();
    await user.click(await screen.findByRole("button", { name: "Delete conversation “Workspace four”" }));
    await user.click(screen.getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(mockDeleteThread).toHaveBeenCalledTimes(1));
    expect(mockDeleteThread).toHaveBeenCalledWith(WORKSPACE_CHAT_ROUTES, 4);
  });

  it("names a member whose chat config cannot be read, with or without other roots (NFR-CC-04)", async () => {
    const broken: MemberChatReadRoots = { name: "broken", policy_origin: null, declared_by: null, read_roots: [] };
    mockFetchReadRoots.mockResolvedValue([broken]);
    const alone = renderWorkspaceChat();
    let banner = (await screen.findByText(/source and graph excerpts/)).closest("section");
    expect(banner).toHaveTextContent(
      "The chat configuration of broken could not be read, so its read roots are not listed here; a source call on that member fails until it is repaired.",
    );
    expect(banner?.textContent).not.toMatch(/Extra read roots/);
    alone.unmount();

    mockFetchReadRoots.mockResolvedValue([
      broken,
      { name: "also-broken", policy_origin: null, declared_by: null, read_roots: [] },
      { name: "api", policy_origin: "member", declared_by: "member", read_roots: ["../api-docs"] },
    ]);
    renderWorkspaceChat();
    banner = (await screen.findByText(/source and graph excerpts/)).closest("section");
    expect(banner).toHaveTextContent("Extra read roots: ../api-docs (relative to api)");
    expect(banner).toHaveTextContent(
      "The chat configuration of broken, also-broken could not be read, so their read roots are not listed here; a source call on those members fails until it is repaired.",
    );
  });

  it("names an absolute read root as written, never as relative to its member", async () => {
    mockFetchReadRoots.mockResolvedValue([
      { name: "api", policy_origin: "member", declared_by: "member", read_roots: ["../api-docs", "/abs/docs"] },
    ]);
    renderWorkspaceChat();
    const banner = (await screen.findByText(/source and graph excerpts/)).closest("section");
    expect(banner).toHaveTextContent(
      "Extra read roots: ../api-docs (relative to api); /abs/docs (read by api) — files under them",
    );
  });
});

describe("scope-keyed client state (S-485, FR-UI-26, NFR-SE-07)", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    setScopedMember(null);
    window.history.replaceState({}, "", "/");
  });

  it("switching member on the member chat never reopens the other member's same-numbered thread", async () => {
    // Each member's chat is its own store, so thread 3 of `api` and thread 3 of
    // `web` are unrelated conversations that happen to share a rowid.
    const user = userEvent.setup();
    stubApi();
    window.history.replaceState({}, "", "/chat?repo=api");
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockFetchThreads.mockResolvedValue([thread(3, "Conversation three", 100)]);
    mockFetchMessages.mockResolvedValue([
      persisted("user", "api's question", 1),
      persisted("assistant", "api's answer", 2),
    ]);
    render(
      <WorkspaceProvider>
        <MemberSwitch />
        <ShellKeyedChat />
      </WorkspaceProvider>,
    );

    // On `api`: open thread 3, which is remembered under `api`'s scope alone.
    await user.click(await screen.findByRole("button", { name: "Conversation three" }));
    await screen.findByText("api's answer");
    expect(window.localStorage.getItem("logos.chat.activeThread:member:api")).toBe("3");
    mockFetchMessages.mockClear();

    // Switch to `web` — the shell's key remounts the member chat for it.
    await user.click(screen.getByRole("button", { name: "switch to web" }));
    await waitFor(() => expect(mockFetchThreads).toHaveBeenCalledTimes(2));
    expect(await screen.findByText(/What would you like to know about this project\?/)).toBeInTheDocument();
    // `web` reopened nothing: thread 3 was never fetched for it, nothing is open.
    expect(mockFetchMessages).not.toHaveBeenCalled();
    expect(screen.queryByText("api's answer")).toBeNull();
    expect(window.localStorage.getItem("logos.chat.activeThread:member:web")).toBeNull();
    // …and `api`'s own remembered thread survives the switch.
    expect(window.localStorage.getItem("logos.chat.activeThread:member:api")).toBe("3");
  });

  it("the workspace chat never reopens a member's or a single root's thread 3", async () => {
    stubApi();
    window.localStorage.setItem("logos.chat.activeThread:member:api", "3");
    window.localStorage.setItem("logos.chat.activeThread:single", "3");
    window.localStorage.setItem("logos.chat.activeThread", "3");
    mockFetchWorkspaceConfig.mockResolvedValue(workspaceTier());
    mockFetchReadRoots.mockResolvedValue([]);
    mockFetchThreads.mockResolvedValue([thread(3, "Workspace three", 100)]);
    render(
      <WorkspaceProvider>
        <WorkspaceChatView />
      </WorkspaceProvider>,
    );
    await screen.findByRole("button", { name: "Workspace three" });
    await waitFor(() => expect(mockFetchThreads).toHaveBeenCalledTimes(1));
    expect(mockFetchMessages).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Workspace three" })).not.toHaveAttribute("aria-current");
  });

  it("…while the workspace chat DOES reopen its own remembered thread (the control)", async () => {
    stubApi();
    window.localStorage.setItem("logos.chat.activeThread:workspace", "3");
    mockFetchWorkspaceConfig.mockResolvedValue(workspaceTier());
    mockFetchReadRoots.mockResolvedValue([]);
    mockFetchThreads.mockResolvedValue([thread(3, "Workspace three", 100)]);
    mockFetchMessages.mockResolvedValue([
      persisted("user", "workspace question", 1),
      persisted("assistant", "workspace answer", 2),
    ]);
    render(
      <WorkspaceProvider>
        <WorkspaceChatView />
      </WorkspaceProvider>,
    );
    expect(await screen.findByText("workspace answer")).toBeInTheDocument();
    expect(mockFetchMessages).toHaveBeenCalledWith(WORKSPACE_CHAT_ROUTES, 3);
  });

  it("a member chat's consent does not carry over to the workspace chat", async () => {
    stubApi();
    window.localStorage.setItem("logos.chat.consent:member:api", "1");
    window.localStorage.setItem("logos.chat.consent:single", "1");
    mockFetchWorkspaceConfig.mockResolvedValue(workspaceTier());
    mockFetchReadRoots.mockResolvedValue([]);
    render(
      <WorkspaceProvider>
        <WorkspaceChatView />
      </WorkspaceProvider>,
    );
    expect(await screen.findByRole("button", { name: "Start chatting" })).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Your message" })).toBeDisabled();
  });
});

// ── S-617 (CR-203, FR-UI-39/40): the conversation is a registered tool panel ──

describe("the chat renders its conversation as a registered tool panel (S-617)", () => {
  it("one stack — the lead callout and the panes — with the conversation under chatConversation", async () => {
    mockFetchConfig.mockResolvedValue(configuredModel());
    const { container } = render(<ChatView />);
    const banner = (await screen.findByText(/source and graph excerpts/)).closest("section")!;
    const stacks = container.querySelectorAll("[data-widget-stack]");
    expect(stacks).toHaveLength(1);
    expect(banner.parentElement).toBe(stacks[0]);
    const panel = screen.getByRole("heading", { name: "Conversation" }).closest("section")!;
    expectToolPanel(panel, "chatConversation");
  });
});

// ── S-619 (CR-204 L2/L3/L6/C8, FR-UI-45): the full-height card's pill composer,
// the cardless rail, its remembered collapse toggle and the consent chip ──────
//
// Class PLACEMENT and DOM structure only, as everywhere in this file: under
// `css: false` a module is a proxy, so whether the pill is round, the page does
// not scroll or the trash is hidden is `spa_design_system`'s and the
// `chat-layout` Playwright spec's to prove.

/** The rail's collapse toggle, by either of its state-named labels. */
function railToggle() {
  return screen.getByRole("button", { name: /^(Show|Hide) conversations$/ });
}

describe("ChatView — the pill composer (S-619, FR-UI-45)", () => {
  it("holds the + button, the input and Send in ONE pill, the + a closed menu button", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    render(<ChatView />);
    await acceptConsent(user);

    const input = screen.getByRole("textbox", { name: "Your message" });
    const pill = input.closest("form")!;
    expect(pill.classList).toContain(chatStyles.composer);
    const plus = screen.getByRole("button", { name: "Composer options" });
    const send = screen.getByRole("button", { name: "Send" });
    expect(pill.contains(plus)).toBe(true);
    expect(pill.contains(send)).toBe(true);
    // Leading "+", trailing Send — the input between them.
    expect(plus.compareDocumentPosition(input) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(input.compareDocumentPosition(send) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    // S-625 fills the slot: the + is the preset menu's button, closed until pressed
    // (its behaviour is `presets.test.tsx`'s).
    expect(plus).toHaveAttribute("aria-haspopup", "menu");
    expect(plus).toHaveAttribute("aria-expanded", "false");
    expect(plus).toBeEnabled();
    expect(plus.querySelector("svg")).toHaveAttribute("aria-hidden", "true");
  });

  it("makes Send a round icon button named Send, disabled (neutral) while the input is empty", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    render(<ChatView />);
    await acceptConsent(user);

    const send = screen.getByRole("button", { name: "Send" });
    expect(send.classList).toContain(chatStyles.send);
    // Icon-only: the name is the label, the glyph is decorative, no visible word.
    expect(send.textContent?.trim()).toBe("");
    expect(send.querySelector("svg")).toHaveAttribute("aria-hidden", "true");
    expect(send).toBeDisabled();
    await user.type(screen.getByRole("textbox", { name: "Your message" }), "x");
    expect(send).toBeEnabled();
  });

  it("swaps Send for a round Stop inside the same pill while a turn runs", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    const pending = pendingSseResponse([]);
    mockStreamTurn.mockResolvedValue(pending.response);
    render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "still running?");

    const stop = await screen.findByRole("button", { name: "Stop" });
    const pill = screen.getByRole("textbox", { name: "Your message" }).closest("form")!;
    expect(pill.contains(stop)).toBe(true);
    expect(stop.classList).toContain(chatStyles.stop);
    expect(stop.textContent?.trim()).toBe("");
    expect(stop.querySelector("svg")).toHaveAttribute("aria-hidden", "true");
    expect(screen.queryByRole("button", { name: "Send" })).not.toBeInTheDocument();
    await user.click(stop);
    expect(await screen.findByRole("button", { name: "Send" })).toBeInTheDocument();
    pending.close();
  });

  it("renders the composer's own input, no longer the fixed three-row box", async () => {
    mockFetchConfig.mockResolvedValue(configuredModel());
    render(<ChatView />);
    const input = await screen.findByRole("textbox", { name: "Your message" });
    expect(input.classList).toContain(chatStyles.input);
    // The old fixed `rows={3}` is gone. The one-line start and the cap are growth
    // bounds on the primitive (`minRows`/`maxRows`), which jsdom does not lay out:
    // `spa_design_system`'s composer guard pins them, and the `chat-layout` spec
    // measures the cap live.
    expect(input).not.toHaveAttribute("rows", "3");
  });
});

describe("ChatView — the cardless rail (S-619, FR-UI-45, FR-UI-26)", () => {
  it("marks the active row's line, not just its title, and gives every row a trash icon", async () => {
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockFetchThreads.mockResolvedValue([thread(2, "Conv B", 200), thread(1, "Conv A", 100)]);
    window.localStorage.setItem("logos.chat.activeThread:single", "2");
    mockFetchMessages.mockResolvedValue([persisted("user", "hi")]);
    render(<ChatView />);

    const active = await screen.findByRole("button", { name: "Conv B" });
    await waitFor(() => expect(active).toHaveAttribute("aria-current", "true"));
    // The filled pill and the red leading marker ride the whole LINE (title + trash).
    expect(active.parentElement!.classList).toContain(chatStyles.threadLineActive);
    const idle = screen.getByRole("button", { name: "Conv A" });
    expect(idle.parentElement!.classList).toContain(chatStyles.threadLine);
    expect(idle.parentElement!.classList).not.toContain(chatStyles.threadLineActive);

    // The delete affordance is a trash icon, not a ✕ character, and keeps its name.
    for (const title of ["Conv A", "Conv B"]) {
      const trash = deleteButton(title);
      expect(trash.textContent?.trim()).toBe("");
      expect(trash.querySelector("svg")).toHaveAttribute("aria-hidden", "true");
    }
  });

  it("keeps the inline Delete / Cancel confirmation behind the trash icon", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockFetchThreads.mockResolvedValue([thread(1, "Conv A", 100)]);
    render(<ChatView />);
    await screen.findByRole("button", { name: "Conv A" });

    await user.click(deleteButton("Conv A"));
    const confirm = screen.getByRole("group", { name: "Confirm deleting “Conv A”" });
    expect(within(confirm).getByRole("button", { name: "Delete" })).toBeInTheDocument();
    await user.click(within(confirm).getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("group", { name: "Confirm deleting “Conv A”" })).toBeNull();
    expect(mockDeleteThread).not.toHaveBeenCalled();
  });
});

describe("ChatView — the rail's collapse toggle, remembered per scope (S-619, FR-UI-45)", () => {
  it("is shown at desktop width, starts open, and hides the rail when pressed", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockFetchThreads.mockResolvedValue([thread(5, "Conv A", 300)]);
    const { container } = render(<ChatView />);
    await screen.findByRole("button", { name: "Conv A" });

    const toggle = railToggle();
    expect(toggle).toHaveAccessibleName("Hide conversations");
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    expect(toggle).toHaveAttribute("aria-controls", "chat-rail");
    expect(toggle.querySelector("svg")).toHaveAttribute("aria-hidden", "true");

    await user.click(toggle);
    expect(toggle).toHaveAccessibleName("Show conversations");
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(container.querySelector("#chat-rail")).toHaveAttribute("hidden");
    expect(screen.queryByRole("navigation", { name: "Conversations" })).toBeNull();
    // The grid gives the conversation the freed column.
    expect(container.querySelector(`.${chatStyles.layout}`)!.classList).toContain(chatStyles.layoutCollapsed);
  });

  it("remembers a collapse for this scope across a remount, and only for this scope", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    const first = render(<ChatView />);
    await screen.findByRole("button", { name: "New chat" });
    await user.click(railToggle());
    expect(window.localStorage.getItem("logos.chat.railOpen:single")).toBe("0");
    first.unmount();

    render(<ChatView />);
    await screen.findByRole("button", { name: "Show conversations" });
    expect(railToggle()).toHaveAttribute("aria-expanded", "false");
    // Another scope's key is untouched: the Workspace Chat keeps its own choice.
    expect(window.localStorage.getItem("logos.chat.railOpen:workspace")).toBeNull();

    await user.click(railToggle());
    expect(window.localStorage.getItem("logos.chat.railOpen:single")).toBe("1");
    expect(screen.getByRole("navigation", { name: "Conversations" })).toBeInTheDocument();
  });

  /** A viewport the test can move across the 1023px breakpoint: `matchMedia` answers
   *  from `narrow`, and `resize` flips it and fires the change listeners the rail
   *  subscribed, as a real window crossing the breakpoint does. */
  function mockViewport(startNarrow: boolean) {
    let narrow = startNarrow;
    const listeners = new Set<() => void>();
    vi.spyOn(window, "matchMedia").mockImplementation(
      (query: string) =>
        ({
          get matches() {
            return query.includes("max-width: 1023px") ? narrow : false;
          },
          media: query,
          onchange: null,
          addEventListener: (_: string, fn: () => void) => listeners.add(fn),
          removeEventListener: (_: string, fn: () => void) => listeners.delete(fn),
          addListener: () => {},
          removeListener: () => {},
          dispatchEvent: () => false,
        }) as unknown as MediaQueryList,
    );
    return {
      resize(toNarrow: boolean) {
        narrow = toNarrow;
        act(() => listeners.forEach((fn) => fn()));
      },
    };
  }

  it("starts collapsed above a narrow conversation, and a pick there closes it, remembering nothing", async () => {
    const user = userEvent.setup();
    mockViewport(true);
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockFetchThreads.mockResolvedValue([thread(5, "Conv A", 300)]);
    render(<ChatView />);
    const toggle = await screen.findByRole("button", { name: "Show conversations" });
    expect(toggle).toHaveAttribute("aria-expanded", "false");

    await user.click(toggle);
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    await user.click(await screen.findByRole("button", { name: "Conv A" }));
    // The pick closes the stacked rail so the restored thread is in view…
    await waitFor(() => expect(railToggle()).toHaveAttribute("aria-expanded", "false"));
    // …and nothing narrow is a preference: the stacked rail behaves as before S-619.
    expect(window.localStorage.getItem("logos.chat.railOpen:single")).toBeNull();
  });

  it("starts collapsed on a narrow viewport even when the desktop choice is open", async () => {
    mockViewport(true);
    window.localStorage.setItem("logos.chat.railOpen:single", "1");
    mockFetchConfig.mockResolvedValue(configuredModel());
    render(<ChatView />);
    expect(await screen.findByRole("button", { name: "Show conversations" })).toHaveAttribute("aria-expanded", "false");
  });

  it("shows each width's own state when the window crosses the breakpoint", async () => {
    const user = userEvent.setup();
    const viewport = mockViewport(true);
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockFetchThreads.mockResolvedValue([thread(5, "Conv A", 300)]);
    render(<ChatView />);
    await user.click(await screen.findByRole("button", { name: "Show conversations" }));
    await user.click(await screen.findByRole("button", { name: "Conv A" }));
    await waitFor(() => expect(railToggle()).toHaveAttribute("aria-expanded", "false"));

    // Widened, the rail sits beside the conversation again — a narrow pick never
    // hides it there.
    viewport.resize(false);
    expect(railToggle()).toHaveAttribute("aria-expanded", "true");
    // A desktop collapse is remembered, and does not leak back to the narrow rail…
    await user.click(railToggle());
    expect(window.localStorage.getItem("logos.chat.railOpen:single")).toBe("0");
    viewport.resize(true);
    expect(railToggle()).toHaveAttribute("aria-expanded", "false");
    // …which still opens on its own toggle without touching the desktop choice.
    await user.click(railToggle());
    expect(railToggle()).toHaveAttribute("aria-expanded", "true");
    expect(window.localStorage.getItem("logos.chat.railOpen:single")).toBe("0");
    viewport.resize(false);
    expect(railToggle()).toHaveAttribute("aria-expanded", "false");
  });

  it("keeps the rail open on a desktop pick", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockFetchThreads.mockResolvedValue([thread(5, "Conv A", 300)]);
    render(<ChatView />);
    await user.click(await screen.findByRole("button", { name: "Conv A" }));
    await waitFor(() => expect(mockFetchMessages).toHaveBeenCalled());
    expect(railToggle()).toHaveAttribute("aria-expanded", "true");
  });

  it("survives storage that throws, falling back to the width default", async () => {
    // The whole `localStorage` is replaced, not a prototype method: the test setup's
    // shim (used where the runtime has no Storage) is a plain object, which a
    // `Storage.prototype` spy never reaches.
    const denied = () => {
      throw new Error("denied");
    };
    vi.stubGlobal("localStorage", { getItem: denied, setItem: denied, removeItem: denied, clear: denied });
    // A throw escaping a handler or a render surfaces as a window `error` event.
    const escaped: unknown[] = [];
    const onError = (event: ErrorEvent) => {
      escaped.push(event.error);
      event.preventDefault();
    };
    window.addEventListener("error", onError);
    try {
      expect(() => window.localStorage.setItem("probe", "1"), "the stub is the storage the page reads").toThrow("denied");
      const user = userEvent.setup();
      mockFetchConfig.mockResolvedValue(configuredModel());
      render(<ChatView />);
      const toggle = await screen.findByRole("button", { name: "Hide conversations" });
      await user.click(toggle);
      expect(toggle).toHaveAttribute("aria-expanded", "false");
      expect(escaped, "a storage failure never escapes the rail").toEqual([]);
    } finally {
      cleanup();
      window.removeEventListener("error", onError);
      vi.unstubAllGlobals();
    }
  });
});

describe("ChatView — the consent chip (S-619, CR-204 C8, FR-UI-45)", () => {
  it("collapses the post-consent lead to a one-line chip naming provider, host, model and budget bounds", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    render(<ChatView />);
    await acceptConsent(user);

    const chip = await screen.findByRole("button", { name: /openrouter\/some-model/ });
    expect(chip).toHaveAttribute("aria-expanded", "false");
    expect(chip).toHaveTextContent("CHAT");
    expect(chip).toHaveTextContent("openai · openrouter.ai · openrouter/some-model");
    expect(chip).toHaveTextContent("budget 24/8/3");
    expect(chip.classList).toContain(chatStyles.statusChip);
    // Collapsed, the full band's sentence is not shown.
    expect(screen.getByText(/Budget tree:/)).not.toBeVisible();
    // The masked key is not on the chip either (NFR-SE-07).
    expect(chip.textContent).not.toContain(MASKED_LAST4);
  });

  it("expands on click to the full band, budget bounds included, and collapses again", async () => {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    render(<ChatView />);
    await acceptConsent(user);

    const chip = await screen.findByRole("button", { name: /openrouter\/some-model/ });
    await user.click(chip);
    expect(chip).toHaveAttribute("aria-expanded", "true");
    const band = document.getElementById(chip.getAttribute("aria-controls")!)!;
    expect(band).toBeVisible();
    expect(band).toHaveTextContent("Budget tree: 24 tool calls, 8 per subagent, 3 replans.");
    await user.click(chip);
    expect(chip).toHaveAttribute("aria-expanded", "false");
    expect(band).not.toBeVisible();
  });

  it("says on the chip that extra read roots are disclosed inside", async () => {
    const user = userEvent.setup();
    const model = configuredModel();
    model.effective_chat.policy = { ...model.effective_chat.policy, read_roots: ["../docs"] };
    mockFetchConfig.mockResolvedValue(model);
    render(<ChatView />);
    await acceptConsent(user);
    const chip = await screen.findByRole("button", { name: /openrouter\/some-model/ });
    expect(chip).toHaveTextContent("extra read roots");
    expect(screen.getByText(/Extra read roots:/)).not.toBeVisible();
    await user.click(chip);
    expect(screen.getByText(/Extra read roots:/)).toBeVisible();
  });

  it("leaves the pre-consent disclosure as it was: the full callout, no chip", async () => {
    mockFetchConfig.mockResolvedValue(configuredModel());
    render(<ChatView />);
    const banner = (await screen.findByText(/source and graph excerpts/)).closest("section")!;
    expect(banner).toHaveTextContent("BEFORE YOU START");
    expect(screen.queryByRole("button", { name: /openrouter\/some-model/ })).toBeNull();
  });
});

describe("WorkspaceChatView — the S-619 surface (FR-WS-34, FR-UI-45)", () => {
  beforeEach(() => {
    stubApi();
    window.history.replaceState({}, "", "/workspace-chat");
    mockFetchWorkspaceConfig.mockResolvedValue(workspaceTier());
    mockFetchReadRoots.mockResolvedValue([]);
  });
  afterEach(() => {
    vi.unstubAllGlobals();
    setScopedMember(null);
    window.history.replaceState({}, "", "/");
  });

  it("renders the same pill, chip and collapse toggle, remembered under the workspace scope", async () => {
    const user = userEvent.setup();
    render(
      <WorkspaceProvider>
        <WorkspaceChatView />
      </WorkspaceProvider>,
    );
    await acceptConsent(user);

    const input = screen.getByRole("textbox", { name: "Your message" });
    expect(input.closest("form")!.classList).toContain(chatStyles.composer);
    expect(screen.getByRole("button", { name: "Composer options" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /budget 24\/8\/3/ })).toHaveAttribute("aria-expanded", "false");

    await user.click(railToggle());
    expect(window.localStorage.getItem("logos.chat.railOpen:workspace")).toBe("0");
    expect(window.localStorage.getItem("logos.chat.railOpen:single")).toBeNull();
  });

  it("flags read roots on the chip when the only disclosure is a member whose roots could not be read", async () => {
    // No member declares a root, but one member's chat configuration is unreadable:
    // the band says so, so the collapsed chip must say there is more inside.
    const broken: MemberChatReadRoots = { name: "broken", policy_origin: null, declared_by: null, read_roots: [] };
    mockFetchReadRoots.mockResolvedValue([broken]);
    const user = userEvent.setup();
    render(
      <WorkspaceProvider>
        <WorkspaceChatView />
      </WorkspaceProvider>,
    );
    await acceptConsent(user);
    const chip = await screen.findByRole("button", { name: /budget 24\/8\/3/ });
    expect(chip).toHaveTextContent("extra read roots");
    await user.click(chip);
    expect(screen.getByText(/The chat configuration of broken could not be read/)).toBeVisible();
  });

  it("renders its conversation as the registered chatConversation tool panel (S-617)", async () => {
    render(
      <WorkspaceProvider>
        <WorkspaceChatView />
      </WorkspaceProvider>,
    );
    const panel = (await screen.findByRole("heading", { name: "Conversation" })).closest("section")!;
    expectToolPanel(panel, "chatConversation");
  });
});

describe("ChatView — the icon action bar (S-620, CR-204 C10, FR-UI-45)", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  async function answered(user: ReturnType<typeof userEvent.setup>, answer = "the answer") {
    mockFetchConfig.mockResolvedValue(configuredModel());
    mockStreamTurn.mockResolvedValue(
      sseResponse([`event: final_answer\ndata: ${JSON.stringify({ answer })}\n\n`]),
    );
    render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "q");
    await screen.findByRole("button", { name: "Copy answer" });
  }

  it("renders Copy and Regenerate as icon-only buttons with an accessible name and a tooltip", async () => {
    const user = userEvent.setup();
    await answered(user);
    for (const name of ["Copy answer", "Regenerate answer"]) {
      const button = screen.getByRole("button", { name });
      expect(button.classList).toContain(chatStyles.actionIcon);
      // Icon-only: no visible word, a decorative glyph, the tooltip is the name.
      expect(button.textContent?.trim()).toBe("");
      expect(button.querySelector("svg")).toHaveAttribute("aria-hidden", "true");
      expect(button).toHaveAttribute("title", name);
    }
  });

  it("copies the answer, shows a check for exactly 1.5 s, then the copy glyph again", async () => {
    const user = userEvent.setup();
    const writeText = vi.spyOn(navigator.clipboard, "writeText").mockResolvedValue();
    await answered(user, "copy me");
    const copy = screen.getByRole("button", { name: "Copy answer" });
    const idleGlyph = copy.innerHTML;
    expect(copy).not.toHaveAttribute("data-copied");

    // The fake clock goes in only now, so the 1.5 s is measured to the millisecond.
    // A live region only announces a CHANGE: it is empty until the copy lands.
    const status = screen.getByRole("status", { hidden: true });
    expect(status).toBeEmptyDOMElement();
    vi.useFakeTimers();
    await act(async () => {
      fireEvent.click(copy);
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(writeText).toHaveBeenCalledWith("copy me");
    expect(copy).toHaveAttribute("data-copied", "true");
    expect(copy.innerHTML).not.toBe(idleGlyph);
    // The acknowledgement is also said once for a screen reader.
    expect(status).toHaveTextContent("Copied");

    // The window is measured from the LATEST copy: a second click at 1000 ms restarts it.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1000);
      fireEvent.click(copy);
      await vi.advanceTimersByTimeAsync(0);
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1499);
    });
    expect(copy).toHaveAttribute("data-copied", "true");
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1);
    });
    expect(copy).not.toHaveAttribute("data-copied");
    expect(copy.innerHTML).toBe(idleGlyph);
    expect(status).toBeEmptyDOMElement();
  });

  it("disables Copy on a turn that has no answer text to copy", async () => {
    const user = userEvent.setup();
    const writeText = vi.spyOn(navigator.clipboard, "writeText").mockResolvedValue();
    mockFetchConfig.mockResolvedValue(configuredModel());
    const pending = pendingSseResponse([]);
    mockStreamTurn.mockResolvedValue(pending.response);
    render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "q");
    await user.click(await screen.findByRole("button", { name: "Stop" }));

    const copy = await screen.findByRole("button", { name: "Copy answer" });
    expect(copy).toBeDisabled();
    await user.click(copy);
    expect(writeText).not.toHaveBeenCalled();
    pending.close();
  });

  it("leaves a code block's Copy as the text button it was", async () => {
    const user = userEvent.setup();
    vi.spyOn(navigator.clipboard, "writeText").mockResolvedValue();
    await answered(user, "```ts\nconst x = 1;\n```");
    const code = await screen.findByRole("button", { name: "Copy code" });
    expect(code.classList).toContain(chatStyles.codeCopy);
    expect(code).toHaveTextContent("Copy");
  });
});

describe("ChatView — the scroll-to-bottom control (S-620, FR-UI-46)", () => {
  /** Give the transcript a scroll geometry jsdom does not have, and a `scrollTo` that
   *  moves it. Returns a function that scrolls the transcript to `top`. */
  function geometry(viewport: HTMLElement) {
    let top = 0;
    Object.defineProperty(viewport, "scrollHeight", { configurable: true, value: 1000 });
    Object.defineProperty(viewport, "clientHeight", { configurable: true, value: 400 });
    Object.defineProperty(viewport, "scrollTop", {
      configurable: true,
      get: () => top,
      set: (v: number) => {
        top = v;
      },
    });
    const scrollTo = vi.fn((arg: ScrollToOptions) => {
      // A real scroller clamps to its end: scrollHeight − clientHeight.
      top = Math.min(arg.top ?? 0, 1000 - 400);
      viewport.dispatchEvent(new Event("scroll"));
    });
    viewport.scrollTo = scrollTo as unknown as typeof viewport.scrollTo;
    return {
      scrollTo,
      scrollTo_: (to: number) => {
        top = to;
        viewport.dispatchEvent(new Event("scroll"));
      },
    };
  }

  async function mounted() {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    render(<ChatView />);
    await acceptConsent(user);
    const viewport = document.querySelector<HTMLElement>(`.${chatStyles.log}`)!;
    return { user, viewport };
  }

  it("is absent from the accessibility tree at the bottom", async () => {
    const { viewport } = await mounted();
    const g = geometry(viewport);
    act(() => g.scrollTo_(600)); // 1000 − 600 − 400 = 0: at the bottom
    expect(screen.queryByRole("button", { name: "Scroll to bottom" })).not.toBeInTheDocument();
    // Not merely hidden: nothing of it is in the DOM either.
    expect(document.querySelector(`.${chatStyles.scrollToBottom}`)).toBeNull();
  });

  it("appears once the reader scrolls away, returns to the bottom when activated, then goes again", async () => {
    const { user, viewport } = await mounted();
    const g = geometry(viewport);
    act(() => g.scrollTo_(600)); // the bottom…
    act(() => g.scrollTo_(100)); // …then up: 500px above it

    const down = await screen.findByRole("button", { name: "Scroll to bottom" });
    expect(down).toHaveAttribute("title", "Scroll to bottom");
    expect(down.querySelector("svg")).toHaveAttribute("aria-hidden", "true");
    expect(down).toBeEnabled();

    await user.click(down);
    // It asked the transcript for its end…
    expect(g.scrollTo).toHaveBeenCalledWith(expect.objectContaining({ top: 1000 }));
    // …and, now followed again, the control is gone from the tree.
    await waitFor(() =>
      expect(screen.queryByRole("button", { name: "Scroll to bottom" })).not.toBeInTheDocument(),
    );
  });

  it("is operable from the keyboard, and hands focus to the composer as it leaves the tree", async () => {
    const { user, viewport } = await mounted();
    const g = geometry(viewport);
    act(() => g.scrollTo_(600));
    act(() => g.scrollTo_(100));
    const down = await screen.findByRole("button", { name: "Scroll to bottom" });

    // Reached by Tab, not by a scripted focus: it is in the tab order, just before the
    // pill's leading "+" (S-625's preset menu).
    const input = screen.getByRole("textbox", { name: "Your message" });
    input.focus();
    await user.tab({ shift: true });
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Composer options" }));
    await user.tab({ shift: true });
    expect(document.activeElement).toBe(down);

    await user.keyboard("{Enter}");
    expect(g.scrollTo).toHaveBeenCalledWith(expect.objectContaining({ top: 1000 }));
    await waitFor(() =>
      expect(screen.queryByRole("button", { name: "Scroll to bottom" })).not.toBeInTheDocument(),
    );
    // The control was focused when it unmounted; focus must not fall to <body>.
    expect(document.activeElement).toBe(input);
  });

  it("rides the transcript's own end — after its last turn, inside the scroller — and leaves the composer's neighbour the transcript", async () => {
    const { viewport } = await mounted();
    const g = geometry(viewport);
    act(() => g.scrollTo_(600));
    act(() => g.scrollTo_(0));
    const down = await screen.findByRole("button", { name: "Scroll to bottom" });
    expect(viewport.contains(down)).toBe(true);
    expect(viewport.lastElementChild!.contains(down)).toBe(true);
    // The pill's previous sibling stays the transcript (the layout specs read it so).
    const pill = screen.getByRole("textbox", { name: "Your message" }).closest("form")!;
    expect(pill.previousElementSibling).toBe(viewport);
  });
});

describe("ChatView — keyboard shortcuts (S-620, FR-UI-46)", () => {
  async function mounted() {
    const user = userEvent.setup();
    mockFetchConfig.mockResolvedValue(configuredModel());
    render(<ChatView />);
    await acceptConsent(user);
    return user;
  }

  /** A text field that belongs to some OTHER view (or the shell): outside the chat. */
  function foreignField() {
    const input = document.createElement("input");
    input.setAttribute("aria-label", "another view's field");
    document.body.appendChild(input);
    return input;
  }

  /** Fire a key on `target` and report whether the chat claimed (prevented) it. */
  function press(target: Element, init: KeyboardEventInit): boolean {
    const event = new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init });
    act(() => {
      target.dispatchEvent(event);
    });
    return event.defaultPrevented;
  }

  afterEach(() => {
    document.querySelectorAll("input[aria-label=\"another view's field\"]").forEach((n) => n.remove());
    vi.unstubAllGlobals();
  });

  it("sends on Enter and inserts a newline on Shift+Enter", async () => {
    const user = await mounted();
    mockStreamTurn.mockResolvedValue(sseResponse(['event: final_answer\ndata: {"answer":"ok"}\n\n']));
    const input = screen.getByRole("textbox", { name: "Your message" });

    await user.type(input, "line one{Shift>}{Enter}{/Shift}line two");
    expect(input).toHaveValue("line one\nline two");
    expect(mockStreamTurn).not.toHaveBeenCalled();

    await user.keyboard("{Enter}");
    await waitFor(() => expect(mockStreamTurn).toHaveBeenCalledTimes(1));
    expect(await screen.findByText("ok")).toBeInTheDocument();
  });

  it("names Enter and Shift+Enter in a hint under the composer, which describes the input", async () => {
    await mounted();
    const input = screen.getByRole("textbox", { name: "Your message" });
    const hint = document.getElementById(input.getAttribute("aria-describedby")!)!;
    expect(hint.classList).toContain(chatStyles.composerHint);
    expect(hint.textContent).toMatch(/Enter.*to send/);
    expect(hint.textContent).toMatch(/Shift.*\+.*Enter.*new line/);
    // Under the pill, not inside it.
    const pill = input.closest("form")!;
    expect(pill.contains(hint)).toBe(false);
    expect(pill.compareDocumentPosition(hint) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("stops a running turn on Esc, as the pill's Stop does", async () => {
    const user = await mounted();
    const pending = pendingSseResponse(['event: answer_delta\ndata: {"delta":"thinking"}\n\n']);
    mockStreamTurn.mockResolvedValue(pending.response);
    await ask(user, "q");
    await screen.findByRole("button", { name: "Stop" });

    await user.keyboard("{Escape}");
    // Exactly what the Stop button leaves behind: Send is back and the turn is
    // finalized over what streamed, so its actions are offered.
    expect(await screen.findByRole("button", { name: "Send" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Stop" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Copy answer" })).toBeInTheDocument();
    expect(screen.getByText("thinking")).toBeInTheDocument();
    pending.close();
  });

  it("hears Esc from the page body too, but only claims it while a turn runs", async () => {
    const user = await mounted();
    expect(press(document.body, { key: "Escape" })).toBe(false); // idle: nothing to stop

    const pending = pendingSseResponse(['event: answer_delta\ndata: {"delta":"x"}\n\n']);
    mockStreamTurn.mockResolvedValue(pending.response);
    await ask(user, "q");
    await screen.findByRole("button", { name: "Stop" });
    (document.activeElement as HTMLElement | null)?.blur();

    expect(press(document.body, { key: "Escape" })).toBe(true);
    expect(await screen.findByRole("button", { name: "Send" })).toBeInTheDocument();
    pending.close();
  });

  it("does not stop a turn on Esc typed in a text field outside the conversation", async () => {
    const user = await mounted();
    const pending = pendingSseResponse(['event: answer_delta\ndata: {"delta":"x"}\n\n']);
    mockStreamTurn.mockResolvedValue(pending.response);
    await ask(user, "q");
    await screen.findByRole("button", { name: "Stop" });

    expect(press(foreignField(), { key: "Escape" })).toBe(false);
    expect(screen.getByRole("button", { name: "Stop" })).toBeInTheDocument();
    pending.close();
  });

  describe("⌘K / Ctrl+K starts a new chat", () => {
    async function withConversation() {
      const user = userEvent.setup();
      mockFetchConfig.mockResolvedValue(configuredModel());
      mockFetchThreads.mockResolvedValue([thread(5, "Conv A", 300)]);
      mockFetchMessages.mockResolvedValue([
        persisted("user", "old question", 1),
        persisted("assistant", "old answer", 2),
      ]);
      render(<ChatView />);
      await user.click(await screen.findByRole("button", { name: "Conv A" }));
      expect(await screen.findByText("old answer")).toBeInTheDocument();
      return user;
    }

    it("uses Ctrl+K off macOS and claims the key from the browser", async () => {
      await withConversation();
      // Meta+K is not this platform's chord.
      expect(press(document.body, { key: "k", metaKey: true })).toBe(false);
      expect(screen.getByText("old answer")).toBeInTheDocument();

      expect(press(document.body, { key: "k", ctrlKey: true })).toBe(true);
      await waitFor(() => expect(screen.queryByText("old answer")).not.toBeInTheDocument());
      expect(screen.getByRole("button", { name: "Send" })).toBeInTheDocument();
      // The same fresh composer "+ New chat" leaves: no turn, no new row.
      expect(mockStreamTurn).not.toHaveBeenCalled();
      expect(screen.getAllByRole("button", { name: "Conv A" })).toHaveLength(1);
    });

    it("uses ⌘K on macOS", async () => {
      vi.stubGlobal("navigator", Object.create(navigator, { platform: { value: "MacIntel" } }));
      await withConversation();
      expect(press(document.body, { key: "k", ctrlKey: true })).toBe(false);
      expect(screen.getByText("old answer")).toBeInTheDocument();

      expect(press(document.body, { key: "k", metaKey: true })).toBe(true);
      await waitFor(() => expect(screen.queryByText("old answer")).not.toBeInTheDocument());
    });

    it("fires from the composer's own field but NOT from a text field of another view", async () => {
      await withConversation();
      expect(press(foreignField(), { key: "k", ctrlKey: true })).toBe(false);
      expect(screen.getByText("old answer")).toBeInTheDocument();

      expect(press(screen.getByRole("textbox", { name: "Your message" }), { key: "k", ctrlKey: true })).toBe(true);
      await waitFor(() => expect(screen.queryByText("old answer")).not.toBeInTheDocument());
    });

    it("ignores a chord with an extra modifier, a held key and an IME composition", async () => {
      await withConversation();
      expect(press(document.body, { key: "k", ctrlKey: true, shiftKey: true })).toBe(false);
      expect(press(document.body, { key: "k", ctrlKey: true, altKey: true })).toBe(false);
      expect(press(document.body, { key: "k", ctrlKey: true, repeat: true })).toBe(false);
      expect(press(document.body, { key: "k", ctrlKey: true, isComposing: true })).toBe(false);
      expect(screen.getByText("old answer")).toBeInTheDocument();
    });

    it("stops listening when the chat unmounts — even with a turn running", async () => {
      const user = await mounted_running();
      cleanup();
      expect(press(document.body, { key: "k", ctrlKey: true })).toBe(false);
      expect(press(document.body, { key: "Escape" })).toBe(false);
      void user;
    });
  });

  /** A consented chat with a turn that stays open. */
  async function mounted_running() {
    const user = await mounted();
    const pending = pendingSseResponse(['event: answer_delta\ndata: {"delta":"thinking"}\n\n']);
    mockStreamTurn.mockResolvedValue(pending.response);
    await ask(user, "q");
    await screen.findByRole("button", { name: "Stop" });
    (document.activeElement as HTMLElement | null)?.blur();
    return user;
  }

  it("starts a new chat on Ctrl+K while a turn is running, ending that turn", async () => {
    await mounted_running();
    expect(press(document.body, { key: "k", ctrlKey: true })).toBe(true);
    await waitFor(() => expect(screen.queryByText("thinking")).not.toBeInTheDocument());
    expect(screen.getByRole("button", { name: "Send" })).toBeInTheDocument();
    expect(screen.queryByText("q")).not.toBeInTheDocument();
  });

  it("stops on a bare Esc only — not with Ctrl, Shift, Alt or Meta held", async () => {
    await mounted_running();
    for (const held of [{ ctrlKey: true }, { shiftKey: true }, { altKey: true }, { metaKey: true }]) {
      expect(press(document.body, { key: "Escape", ...held }), JSON.stringify(held)).toBe(false);
    }
    expect(screen.getByRole("button", { name: "Stop" })).toBeInTheDocument();
    expect(press(document.body, { key: "Escape" })).toBe(true);
  });

  it("leaves a key some other handler already claimed alone", async () => {
    await mounted_running();
    // Another handler (here: one on the window, in the capture phase, ahead of the
    // document listener) claims the key first.
    const claim = (e: KeyboardEvent) => e.preventDefault();
    window.addEventListener("keydown", claim, true);
    try {
      expect(press(document.body, { key: "Escape" })).toBe(true); // claimed — by the other handler
      // Give a (wrongly) acting handler time to land its state change before looking.
      await act(async () => {
        await new Promise((r) => setTimeout(r, 50));
      });
      expect(screen.getByRole("button", { name: "Stop" })).toBeInTheDocument();
    } finally {
      window.removeEventListener("keydown", claim, true);
    }
  });

  it("treats a foreign textarea or select like a foreign input, and Caps Lock's K like k", async () => {
    await mounted_running();
    for (const tag of ["textarea", "select"]) {
      const el = document.createElement(tag);
      document.body.appendChild(el);
      expect(press(el, { key: "Escape" }), `Esc in a foreign ${tag}`).toBe(false);
      expect(press(el, { key: "k", ctrlKey: true }), `Ctrl+K in a foreign ${tag}`).toBe(false);
      el.remove();
    }
    expect(screen.getByRole("button", { name: "Stop" })).toBeInTheDocument();
    expect(press(document.body, { key: "K", ctrlKey: true })).toBe(true); // Caps Lock
  });
});

describe("WorkspaceChatView — the S-620 surface (FR-WS-34, FR-UI-46)", () => {
  beforeEach(() => {
    stubApi();
    window.history.replaceState({}, "", "/workspace-chat");
    mockFetchWorkspaceConfig.mockResolvedValue(workspaceTier());
    mockFetchReadRoots.mockResolvedValue([]);
  });
  afterEach(() => {
    vi.unstubAllGlobals();
    setScopedMember(null);
    window.history.replaceState({}, "", "/");
  });

  it("carries the same hint, icon actions and Ctrl+K", async () => {
    const user = userEvent.setup();
    mockFetchThreads.mockResolvedValue([thread(7, "WS conv", 300)]);
    mockFetchMessages.mockResolvedValue([persisted("user", "wq", 1), persisted("assistant", "wa", 2)]);
    render(
      <WorkspaceProvider>
        <WorkspaceChatView />
      </WorkspaceProvider>,
    );
    await acceptConsent(user);
    const input = screen.getByRole("textbox", { name: "Your message" });
    expect(document.getElementById(input.getAttribute("aria-describedby")!)!.textContent).toMatch(
      /Enter.*to send.*Shift.*Enter.*new line/,
    );

    await user.click(await screen.findByRole("button", { name: "WS conv" }));
    expect(await screen.findByText("wa")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Copy answer" }).classList).toContain(chatStyles.actionIcon);
    expect(screen.getByRole("button", { name: "Regenerate answer" }).classList).toContain(chatStyles.actionIcon);

    const event = new KeyboardEvent("keydown", { key: "k", ctrlKey: true, bubbles: true, cancelable: true });
    act(() => {
      document.body.dispatchEvent(event);
    });
    expect(event.defaultPrevented).toBe(true);
    await waitFor(() => expect(screen.queryByText("wa")).not.toBeInTheDocument());
  });
});

describe("ChatView — edit and resend, and a Regenerate that truncates in the store (S-624, FR-UI-46)", () => {
  type Row = { role: "user" | "assistant"; content: string };

  beforeEach(() => mockFetchConfig.mockResolvedValue(configuredModel()));

  /**
   * A conversation store behind the mocked client, holding thread 7: a turn appends
   * its question and then `answer to <question>`, a truncate cuts back to before the
   * named stored question, and a restore reads what is left. What a reload shows is
   * what this holds — so a re-run that skipped the cut would leave both answers here.
   */
  function fakeStore(rows: Row[], routes: ChatRoutes = MEMBER_CHAT_ROUTES) {
    mockFetchThreads.mockResolvedValue([thread(7, "Conversation", 100)]);
    mockFetchMessages.mockImplementation(async () =>
      rows.map((r, i) => persisted(r.role, r.content, i + 1)),
    );
    mockTruncate.mockImplementation(async (r, id, userTurn) => {
      expect([r, id]).toEqual([routes, 7]);
      const at = rows.flatMap((row, i) => (row.role === "user" ? [i] : []))[userTurn];
      if (at !== undefined) rows.splice(at);
      return { ok: true, status: 204 } as unknown as Response;
    });
    mockStreamTurn.mockImplementation(async (_r, question) => {
      rows.push({ role: "user", content: question }, { role: "assistant", content: `answer to ${question}` });
      return sseResponse([`event: final_answer\ndata: {"answer":"answer to ${question}"}\n\n`]);
    });
    return rows;
  }

  /** `n` answered turns: `q0 a0 q1 a1 …`. */
  function turns(n: number): Row[] {
    return Array.from({ length: n }, (_, i): Row[] => [
      { role: "user", content: `q${i}` },
      { role: "assistant", content: `a${i}` },
    ]).flat();
  }

  /** Open thread 7 from the rail and wait for its last row. */
  async function open(user: ReturnType<typeof userEvent.setup>, last: string) {
    await user.click(await screen.findByRole("button", { name: "Conversation" }));
    await screen.findByText(last);
  }

  /** The transcript's turn texts, in order. */
  function transcript(): string[] {
    return Array.from(document.querySelectorAll(`.${chatStyles.userBubble}, .${chatStyles.final}`)).map(
      (el) => el.textContent ?? "",
    );
  }

  /** Reload the view: a new mount restores the remembered conversation from the store. */
  async function reload(last: string) {
    cleanup();
    render(<ChatView />);
    await screen.findByText(last);
  }

  async function edit(user: ReturnType<typeof userEvent.setup>, index: number, text: string) {
    await user.click(screen.getAllByRole("button", { name: "Edit message" })[index]);
    const field = screen.getByRole("textbox", { name: "Edit your message" });
    await user.clear(field);
    await user.type(field, text);
    await user.click(screen.getByRole("button", { name: "Resend" }));
  }

  it("editing the second of four questions leaves the first two messages plus the re-run turn, after a reload too", async () => {
    const user = userEvent.setup();
    const rows = fakeStore(turns(4));
    render(<ChatView />);
    await acceptConsent(user);
    await open(user, "a3");

    await edit(user, 1, "q1, sharper");

    await screen.findByText("answer to q1, sharper");
    const kept = ["q0", "a0", "q1, sharper", "answer to q1, sharper"];
    await waitFor(() => expect(transcript()).toEqual(kept));
    expect(mockTruncate).toHaveBeenCalledTimes(1);
    expect(mockTruncate.mock.calls[0][2]).toBe(1);
    // The resend appends to the SAME conversation, cut first.
    expect(mockStreamTurn).toHaveBeenCalledWith(MEMBER_CHAT_ROUTES, "q1, sharper", 7, "default", expect.anything());
    expect(mockTruncate.mock.invocationCallOrder[0]).toBeLessThan(mockStreamTurn.mock.invocationCallOrder[0]);
    expect(rows.map((r) => r.content)).toEqual(kept);

    await reload("answer to q1, sharper");
    expect(transcript()).toEqual(kept);
  });

  it("after a Regenerate and a reload, the turn shows one answer", async () => {
    const user = userEvent.setup();
    const rows = fakeStore([{ role: "user", content: "q" }, { role: "assistant", content: "first answer" }]);
    render(<ChatView />);
    await acceptConsent(user);
    await open(user, "first answer");
    await user.click(screen.getByRole("button", { name: "Regenerate answer" }));
    await screen.findByText("answer to q");
    expect(mockTruncate).toHaveBeenCalledWith(MEMBER_CHAT_ROUTES, 7, 0);
    expect(rows.map((r) => r.content)).toEqual(["q", "answer to q"]);

    await reload("answer to q");
    expect(transcript()).toEqual(["q", "answer to q"]);
    expect(screen.queryByText("first answer")).not.toBeInTheDocument();
  });

  it("hides Edit while a turn runs, and shows it again when the turn ends", async () => {
    const user = userEvent.setup();
    const pending = pendingSseResponse(['event: answer_delta\ndata: {"delta":"thinking"}\n\n']);
    mockStreamTurn.mockResolvedValue(pending.response);
    render(<ChatView />);
    await acceptConsent(user);
    await ask(user, "q");
    await screen.findByText("thinking");
    expect(screen.queryByRole("button", { name: "Edit message" })).not.toBeInTheDocument();

    pending.push('event: final_answer\ndata: {"answer":"done"}\n\n');
    pending.close();
    expect(await screen.findByRole("button", { name: "Edit message" })).toBeInTheDocument();
  });

  it("Esc cancels an open edit: the question stands and nothing is cut or sent", async () => {
    const user = userEvent.setup();
    fakeStore(turns(2));
    render(<ChatView />);
    await acceptConsent(user);
    await open(user, "a1");

    await user.click(screen.getAllByRole("button", { name: "Edit message" })[0]);
    const field = screen.getByRole("textbox", { name: "Edit your message" });
    expect(field).toHaveValue("q0");
    expect(field).toHaveFocus();
    await user.type(field, " and more");
    await user.keyboard("{Escape}");

    expect(screen.queryByRole("textbox", { name: "Edit your message" })).not.toBeInTheDocument();
    expect(transcript()).toEqual(["q0", "a0", "q1", "a1"]);
    expect(mockTruncate).not.toHaveBeenCalled();
    expect(mockStreamTurn).not.toHaveBeenCalled();
  });

  it("Esc in an open edit never reaches the chat's Esc-stops-the-turn shortcut", async () => {
    const user = userEvent.setup();
    fakeStore(turns(1));
    const pending = pendingSseResponse(['event: answer_delta\ndata: {"delta":"thinking"}\n\n']);
    mockStreamTurn.mockResolvedValue(pending.response);
    render(<ChatView />);
    await acceptConsent(user);
    await open(user, "a0");

    // An edit opened before a turn starts stays open while it runs.
    await user.click(screen.getByRole("button", { name: "Edit message" }));
    await ask(user, "next");
    await screen.findByText("thinking");
    await user.click(screen.getByRole("textbox", { name: "Edit your message" }));
    await user.keyboard("{Escape}");

    expect(screen.queryByRole("textbox", { name: "Edit your message" })).not.toBeInTheDocument();
    const signal = mockStreamTurn.mock.calls[0][4] as AbortSignal;
    expect(signal.aborted).toBe(false);
    expect(screen.getByRole("button", { name: "Stop" })).toBeInTheDocument();
    pending.close();
  });

  it("does not count a question the server refused when it names the cut", async () => {
    const user = userEvent.setup();
    const rows = fakeStore(turns(1));
    render(<ChatView />);
    await acceptConsent(user);
    await open(user, "a0");
    // Refused before the server stored anything (a 4xx start).
    mockStreamTurn.mockResolvedValueOnce(sseResponse([], 400));
    await ask(user, "refused");
    await screen.findByText(/could not start \(status 400\)/);
    await ask(user, "q1");
    await screen.findByText("answer to q1");

    await edit(user, 2, "q1, again");

    await screen.findByText("answer to q1, again");
    // q0 is the only stored question before q1: the cut is the store's second question.
    expect(mockTruncate.mock.calls[0][2]).toBe(1);
    expect(rows.map((r) => r.content)).toEqual(["q0", "a0", "q1, again", "answer to q1, again"]);
  });

  it("re-runs nothing, and says so, when the cut fails", async () => {
    const user = userEvent.setup();
    fakeStore(turns(1));
    render(<ChatView />);
    await acceptConsent(user);
    await open(user, "a0");

    mockTruncate.mockResolvedValueOnce({ ok: false, status: 500 } as unknown as Response);
    await user.click(screen.getByRole("button", { name: "Regenerate answer" }));
    expect(await screen.findByText(/not re-run: the conversation could not be cut back \(status 500\)/)).toBeInTheDocument();

    mockTruncate.mockResolvedValueOnce({ ok: false, status: 404 } as unknown as Response);
    await user.click(screen.getByRole("button", { name: "Regenerate answer" }));
    expect(await screen.findByText(/not re-run: this conversation no longer exists/)).toBeInTheDocument();
    expect(mockStreamTurn).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Send" })).toBeInTheDocument();
  });

  it("a failed cut leaves the conversation as the store still holds it", async () => {
    const user = userEvent.setup();
    const rows = fakeStore(turns(3));
    render(<ChatView />);
    await acceptConsent(user);
    await open(user, "a2");

    mockTruncate.mockResolvedValueOnce({ ok: false, status: 500 } as unknown as Response);
    await edit(user, 1, "q1, sharper");
    await screen.findByText(/not re-run: the conversation could not be cut back \(status 500\)/);
    const whole = ["q0", "a0", "q1", "a1", "q2", "a2"];
    expect(transcript()).toEqual(whole);
    expect(rows.map((r) => r.content)).toEqual(whole);

    // The surface and the store still agree, so the next re-run names the right question.
    await ask(user, "q3");
    await screen.findByText("answer to q3");
    await edit(user, 3, "q3, again");
    await screen.findByText("answer to q3, again");
    expect(mockTruncate.mock.calls[1][2]).toBe(3);
    expect(rows.map((r) => r.content)).toEqual([...whole, "q3, again", "answer to q3, again"]);
  });

  it("a cut that lands after Stop no longer counts the question it removed", async () => {
    const user = userEvent.setup();
    const rows = fakeStore(turns(2));
    // Hold the first cut until the reader has stopped the re-run.
    let land: () => void = () => {};
    const landed = new Promise<void>((resolve) => (land = resolve));
    const cut = mockTruncate.getMockImplementation()!;
    mockTruncate.mockImplementationOnce(async (r, id, userTurn) => {
      await landed;
      return cut(r, id, userTurn);
    });
    render(<ChatView />);
    await acceptConsent(user);
    await open(user, "a1");

    await user.click(screen.getAllByRole("button", { name: "Regenerate answer" })[1]);
    await user.click(await screen.findByRole("button", { name: "Stop" }));
    land();
    await waitFor(() => expect(rows.map((r) => r.content)).toEqual(["q0", "a0"]));
    expect(mockStreamTurn).not.toHaveBeenCalled();

    await ask(user, "q2");
    await screen.findByText("answer to q2");
    const regenerate = screen.getAllByRole("button", { name: "Regenerate answer" });
    await user.click(regenerate[regenerate.length - 1]);
    await waitFor(() => expect(mockStreamTurn).toHaveBeenCalledTimes(2));
    // q1 left the store when the cut landed: q2 is the store's second question.
    expect(mockTruncate.mock.calls[1][2]).toBe(1);
    await waitFor(() => expect(rows.map((r) => r.content)).toEqual(["q0", "a0", "q2", "answer to q2"]));
  });

  it("Stop during a cut holds the composer until the cut lands, so a question sent next is never cut with it", async () => {
    const user = userEvent.setup();
    const rows = fakeStore(turns(2));
    let land: () => void = () => {};
    const landed = new Promise<void>((resolve) => (land = resolve));
    const cut = mockTruncate.getMockImplementation()!;
    mockTruncate.mockImplementationOnce(async (r, id, userTurn) => {
      await landed;
      return cut(r, id, userTurn);
    });
    render(<ChatView />);
    await acceptConsent(user);
    await open(user, "a1");

    await user.click(screen.getAllByRole("button", { name: "Regenerate answer" })[1]);
    await user.click(await screen.findByRole("button", { name: "Stop" }));
    // The server has not run the cut yet: nothing can be sent into the thread.
    expect(screen.queryByRole("button", { name: "Send" })).not.toBeInTheDocument();

    land();
    await screen.findByRole("button", { name: "Send" });
    await ask(user, "q2");
    await screen.findByText("answer to q2");
    expect(rows.map((r) => r.content)).toEqual(["q0", "a0", "q2", "answer to q2"]);
    expect(mockTruncate).toHaveBeenCalledTimes(1);
  });

  it("a refused question re-run successfully counts as stored again", async () => {
    const user = userEvent.setup();
    const rows = fakeStore(turns(1));
    render(<ChatView />);
    await acceptConsent(user);
    await open(user, "a0");
    mockStreamTurn.mockResolvedValueOnce(sseResponse([], 400));
    await ask(user, "refused");
    await screen.findByText(/could not start \(status 400\)/);

    // Regenerate the refused turn: this time the store takes it.
    await user.click(screen.getAllByRole("button", { name: "Regenerate answer" })[1]);
    await screen.findByText("answer to refused");
    await ask(user, "q2");
    await screen.findByText("answer to q2");
    await edit(user, 2, "q2, again");
    await screen.findByText("answer to q2, again");

    // q0 and "refused" are both stored now, so q2 is the store's third question.
    expect(mockTruncate.mock.calls.at(-1)?.[2]).toBe(2);
    expect(rows.map((r) => r.content)).toEqual([
      "q0", "a0", "refused", "answer to refused", "q2, again", "answer to q2, again",
    ]);
  });

  it("a cut that never reached the server re-runs nothing, and says so", async () => {
    const user = userEvent.setup();
    const rows = fakeStore(turns(1));
    render(<ChatView />);
    await acceptConsent(user);
    await open(user, "a0");

    mockTruncate.mockRejectedValueOnce(new TypeError("Failed to fetch"));
    await user.click(screen.getByRole("button", { name: "Regenerate answer" }));
    expect(await screen.findByText(/not re-run: the conversation could not be cut back \(Failed to fetch\)/)).toBeInTheDocument();
    expect(mockStreamTurn).not.toHaveBeenCalled();
    expect(transcript()).toEqual(["q0", "a0"]);
    expect(rows.map((r) => r.content)).toEqual(["q0", "a0"]);
  });

  describe("in the Workspace Chat", () => {
    beforeEach(() => {
      stubApi();
      window.history.replaceState({}, "", "/workspace-chat");
      mockFetchWorkspaceConfig.mockResolvedValue(workspaceTier());
      mockFetchReadRoots.mockResolvedValue([]);
    });
    afterEach(() => {
      vi.unstubAllGlobals();
      setScopedMember(null);
      window.history.replaceState({}, "", "/");
    });

    it("edits, regenerates and cuts through the workspace chat's own routes", async () => {
      const user = userEvent.setup();
      const rows = fakeStore(turns(2), WORKSPACE_CHAT_ROUTES);
      render(
        <WorkspaceProvider>
          <WorkspaceChatView />
        </WorkspaceProvider>,
      );
      await acceptConsent(user);
      await open(user, "a1");

      await edit(user, 1, "q1, sharper");
      await screen.findByText("answer to q1, sharper");
      await user.click(screen.getAllByRole("button", { name: "Regenerate answer" })[0]);
      await waitFor(() => expect(rows.map((r) => r.content)).toEqual(["q0", "answer to q0"]));

      expect(mockTruncate.mock.calls.map((c) => [c[0], c[2]])).toEqual([
        [WORKSPACE_CHAT_ROUTES, 1],
        [WORKSPACE_CHAT_ROUTES, 0],
      ]);
      expect(mockStreamTurn.mock.calls.every((c) => c[0] === WORKSPACE_CHAT_ROUTES)).toBe(true);
    });
  });
});
