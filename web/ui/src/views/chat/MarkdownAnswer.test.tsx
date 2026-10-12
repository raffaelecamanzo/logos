/*
 * MarkdownAnswer — syntax highlighting and verified code-reference links (S-622,
 * FR-UI-47, NFR-CC-04, NFR-SE-06).
 *
 * The code-reference cases render the answer INSIDE a real assistant-ui message
 * (an external-store runtime over the chat runtime's own `convertMessage`), because
 * that is where `MarkdownAnswer` learns the turn finalized: from the message's
 * folded turn, exactly as `ChatView`'s `AssistantMessage` renders it.
 */

import {
  AssistantRuntimeProvider,
  ThreadPrimitive,
  useExternalStoreRuntime,
  useMessage,
} from "@assistant-ui/react";
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { useMemo } from "react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { CodeRefResolution } from "../../api/types.ts";
import { setScopedMember } from "../../workspace/scope.ts";
import { ROSTER } from "../../workspace/testFixtures.ts";
import { useWorkspace, WorkspaceProvider } from "../../workspace/WorkspaceContext.tsx";
import { initialTurn, type TurnState } from "./chatModel.ts";
import { clearCodeRefResolutions } from "./codeRefs.ts";
import { convertMessage, type ChatMessage } from "./chatRuntime.tsx";
import { MarkdownAnswer } from "./MarkdownAnswer.tsx";

// The four-span answer of the acceptance criterion: an existing file, a unique
// symbol, an ambiguous symbol and a nonexistent path — plus fenced blocks whose
// contents must never be collected (one is a single path-shaped token, which the
// client's pre-filter would let through), and a command that is not a candidate.
const ANSWER = [
  "The registry lives in `src/registry.rs:3`, where `register_all()` calls each arm.",
  "Every arm implements `extract`; there is no `src/missing.rs`.",
  "Run `cargo test` to check.",
  "",
  "```rust",
  "fn main() { register_all(); }",
  "```",
  "",
  "```text",
  "src/fenced.rs",
  "```",
].join("\n");

const RESOLUTION: CodeRefResolution = {
  cap: 24,
  over_cap: 0,
  refs: [
    { text: "src/registry.rs:3", outcome: "file", seed: "src/registry.rs" },
    { text: "register_all()", outcome: "symbol", seed: "logos . . . src/`registry.rs`/register_all()." },
    { text: "extract", outcome: "ambiguous" },
    { text: "src/missing.rs", outcome: "unresolved" },
  ],
};

/** Route `/api/v1/code-refs` to {@link RESOLUTION}; record every URL fetched. */
function stubCodeRefs(resolution: CodeRefResolution = RESOLUTION) {
  const urls: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((url: string) => {
      urls.push(url);
      return Promise.resolve({ ok: true, json: () => Promise.resolve(resolution) } as Response);
    }),
  );
  return urls;
}

const codeRefRequests = (urls: string[]) => urls.filter((u) => u.startsWith("/api/v1/code-refs"));

/** One assistant message rendering its answer the way `AssistantMessage` does,
 *  plus an Activity-fold step result through the same renderer. */
function TurnBody({ summary }: { summary?: string }) {
  const turn = useMessage((m) => m.metadata.custom.turn as TurnState);
  return (
    <div>
      <section aria-label="answer">
        <MarkdownAnswer text={turn.answer} />
      </section>
      {summary !== undefined && (
        <section aria-label="activity">
          <MarkdownAnswer text={summary} />
        </section>
      )}
    </div>
  );
}

function Thread({ turn, summary }: { turn: TurnState; summary?: string }) {
  // Stable across re-renders, as the chat runtime's own message array is between
  // frames: a fresh array would make assistant-ui remount the message.
  const messages = useMemo<ChatMessage[]>(() => [{ kind: "assistant", id: 1, parentId: 0, turn }], [turn]);
  // Stable component types, as ChatView's are: an inline arrow is a new type on
  // every render, and React would remount the message each time.
  const components = useMemo(
    () => ({ UserMessage: () => null, AssistantMessage: () => <TurnBody summary={summary} /> }),
    [summary],
  );
  const runtime = useExternalStoreRuntime<ChatMessage>({
    messages,
    convertMessage,
    isRunning: !turn.finalized,
    onNew: async () => {},
  });
  return (
    <AssistantRuntimeProvider runtime={runtime}>
      <ThreadPrimitive.Messages components={components} />
    </AssistantRuntimeProvider>
  );
}

const finalized = (answer: string): TurnState => ({ ...initialTurn(), answer, finalized: true });
const streaming = (answer: string): TurnState => ({ ...initialTurn(), answer, streaming: true });

beforeEach(() => {
  clearCodeRefResolutions();
  setScopedMember(null);
  window.history.replaceState({}, "", "/chat");
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  setScopedMember(null);
});

describe("MarkdownAnswer — syntax highlighting (FR-UI-47)", () => {
  it("renders a fenced rust block as token-class spans", () => {
    const { container } = render(<MarkdownAnswer text={"```rust\nfn main() { let x = 1; }\n```\n"} />);
    const code = container.querySelector("pre code")!;
    expect(code.querySelector(".hljs-keyword")?.textContent).toBe("fn");
    expect(code.querySelector(".hljs-number")?.textContent).toBe("1");
    expect(code.textContent).toBe("fn main() { let x = 1; }");
  });

  it("renders nested token spans with their text: a TS template literal stays verbatim", () => {
    const { container } = render(<MarkdownAnswer text={"```ts\nconst s = `a ${b} c`;\n```\n"} />);
    const code = container.querySelector("pre code")!;
    expect(code.textContent).toBe("const s = `a ${b} c`;");
    expect(code.querySelector(".hljs-string .hljs-subst")?.textContent).toBe("${b}");
  });

  it("highlights a `c#` or `c++` fence with its own grammar, not C's, under its full label", () => {
    const { container } = render(
      <MarkdownAnswer text={"```c#\nvar xs = new List<int>();\n```\n\n```c++\nnamespace app { class Box {}; }\n```\n"} />,
    );
    const [csharp, cpp] = Array.from(container.querySelectorAll("pre code"));
    // `var` and `new` are C# keywords the C grammar does not know.
    expect(Array.from(csharp.querySelectorAll(".hljs-keyword")).map((k) => k.textContent)).toEqual(
      expect.arrayContaining(["var", "new"]),
    );
    expect(Array.from(cpp.querySelectorAll(".hljs-keyword")).map((k) => k.textContent)).toEqual(
      expect.arrayContaining(["namespace", "class"]),
    );
    expect(screen.getByText("c#")).toBeInTheDocument();
    expect(screen.getByText("c++")).toBeInTheDocument();
  });

  it("renders an unknown language plain, its source verbatim", () => {
    const { container } = render(<MarkdownAnswer text={"```brainfuck\n+[->+<]\n```\n"} />);
    const code = container.querySelector("pre code")!;
    expect(code.querySelectorAll("span")).toHaveLength(0);
    expect(code.textContent).toBe("+[->+<]");
    expect(screen.getByText("brainfuck")).toBeInTheDocument(); // the label stays
  });

  it("puts no `style` attribute on any element of a highlighted, linked answer", async () => {
    stubCodeRefs();
    const { container } = render(<Thread turn={finalized(ANSWER)} />);
    await waitFor(() => expect(container.querySelectorAll("a")).toHaveLength(2));
    expect(container.querySelectorAll(".hljs-keyword").length).toBeGreaterThan(0);
    expect(container.querySelectorAll("[style]")).toHaveLength(0);
  });
});

describe("MarkdownAnswer — verified code references (FR-UI-47, NFR-CC-04)", () => {
  it("links exactly the existing file and the unique symbol, to the Graph view", async () => {
    const urls = stubCodeRefs();
    render(<Thread turn={finalized(ANSWER)} />);

    const file = await screen.findByRole("link", { name: "src/registry.rs:3" });
    expect(file).toHaveAttribute("href", "/graph?seed=src%2Fregistry.rs&granularity=file");
    const symbol = screen.getByRole("link", { name: "register_all()" });
    const target = new URL(symbol.getAttribute("href")!, "http://127.0.0.1");
    expect(target.pathname).toBe("/graph");
    expect(target.searchParams.get("seed")).toBe("logos . . . src/`registry.rs`/register_all().");
    expect(target.searchParams.has("granularity")).toBe(false);
    // The ambiguous symbol and the missing path stay plain code.
    expect(screen.getAllByRole("link")).toHaveLength(2);
    expect(screen.getByText("extract").closest("a")).toBeNull();
    expect(screen.getByText("src/missing.rs").closest("a")).toBeNull();

    // ONE request, carrying the inline spans in order — not the fenced blocks'
    // code, and not a command line.
    const sent = codeRefRequests(urls);
    expect(sent).toHaveLength(1);
    expect(new URLSearchParams(sent[0].split("?")[1]).getAll("ref")).toEqual([
      "src/registry.rs:3",
      "register_all()",
      "extract",
      "src/missing.rs",
    ]);
  });

  it("issues at most one request per finalized turn, across re-renders and a remount", async () => {
    const urls = stubCodeRefs();
    const view = render(<Thread turn={finalized(ANSWER)} />);
    await screen.findByRole("link", { name: "register_all()" });
    view.rerender(<Thread turn={finalized(ANSWER)} />);
    view.unmount();
    render(<Thread turn={finalized(ANSWER)} />);
    await screen.findByRole("link", { name: "register_all()" });
    expect(codeRefRequests(urls)).toHaveLength(1);
  });

  it("issues nothing while the answer streams, and resolves once it finalizes", async () => {
    const urls = stubCodeRefs();
    const view = render(<Thread turn={streaming(ANSWER)} />);
    await act(async () => {});
    expect(codeRefRequests(urls)).toHaveLength(0);
    expect(screen.queryAllByRole("link")).toHaveLength(0);

    view.rerender(<Thread turn={finalized(ANSWER)} />);
    await screen.findByRole("link", { name: "register_all()" });
    expect(codeRefRequests(urls)).toHaveLength(1);
  });

  it("highlights an Activity-fold step result's code but never resolves its spans", async () => {
    const urls = stubCodeRefs();
    const summary = "Found `register_all()` in `src/registry.rs:3`.\n\n```rust\nfn register_all() {}\n```\n";
    render(<Thread turn={finalized("Done.")} summary={summary} />);
    await act(async () => {});
    expect(codeRefRequests(urls)).toHaveLength(0);
    expect(screen.queryAllByRole("link")).toHaveLength(0);
    // FR-UI-47: step results are highlighted like the answer body.
    const activity = screen.getByRole("region", { name: "activity" });
    expect(activity.querySelector("pre code .hljs-keyword")?.textContent).toBe("fn");
  });

  it("never resolves a bare render outside a chat message", async () => {
    const urls = stubCodeRefs();
    render(<MarkdownAnswer text={ANSWER} />);
    await act(async () => {});
    expect(codeRefRequests(urls)).toHaveLength(0);
  });

  it("caps the spans one turn sends", async () => {
    const urls = stubCodeRefs({ cap: 24, over_cap: 0, refs: [] });
    const many = Array.from({ length: 30 }, (_, i) => `\`name_${i}\``).join(" ");
    render(<Thread turn={finalized(many)} />);
    await waitFor(() => expect(codeRefRequests(urls)).toHaveLength(1));
    const refs = new URLSearchParams(codeRefRequests(urls)[0].split("?")[1]).getAll("ref");
    expect(refs).toHaveLength(24);
    expect(refs[23]).toBe("name_23");
  });

  it("does not nest a link inside a markdown link", async () => {
    stubCodeRefs();
    render(<Thread turn={finalized("See [`register_all()`](https://example.com) and `register_all()`.")} />);
    await waitFor(() => expect(screen.getAllByRole("link", { name: "register_all()" })).toHaveLength(2));
    const links = screen.getAllByRole("link", { name: "register_all()" });
    expect(links[0]).toHaveAttribute("href", "https://example.com");
    expect(links[0].querySelector("a")).toBeNull();
    expect(links[1].getAttribute("href")).toMatch(/^\/graph\?seed=/);
  });

  it("addresses the workspace member the shell scopes reads to", async () => {
    setScopedMember("web");
    const urls = stubCodeRefs();
    render(<Thread turn={finalized(ANSWER)} />);
    const symbol = await screen.findByRole("link", { name: "register_all()" });
    const sent = codeRefRequests(urls);
    expect(sent).toHaveLength(1);
    // One query string: the spans join the member's `?repo=` with `&`. A second
    // `?` would make the server read the member as `web?ref=…` — an unknown one.
    expect(sent[0].split("?")).toHaveLength(2);
    const query = new URLSearchParams(sent[0].split("?")[1]);
    expect(query.get("repo")).toBe("web");
    expect(query.getAll("ref")).toEqual(["src/registry.rs:3", "register_all()", "extract", "src/missing.rs"]);
    // A new-tab open lands on the same member's graph.
    expect(new URLSearchParams(symbol.getAttribute("href")!.split("?")[1]).get("repo")).toBe("web");
  });

  it("never keeps one member's links after a shell switch: it re-resolves for the selected member, once", async () => {
    // A real WorkspaceProvider over a two-member roster, as the app-scoped Workspace
    // Chat sees it: a member switch re-renders the transcript but does not remount it.
    const urls: string[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string) => {
        urls.push(url);
        const body = url.startsWith("/api/v1/workspace/roster")
          ? ROSTER
          : url.includes("repo=web")
            ? { cap: 24, over_cap: 0, refs: [] } // `web`'s graph confirms none of the spans
            : RESOLUTION;
        return Promise.resolve({ ok: true, json: () => Promise.resolve(body) } as Response);
      }),
    );
    let select: (member: string) => void = () => {};
    const turn = finalized(ANSWER);
    // Like the shell, mount the view only once the probe has settled the member.
    function Shell() {
      const workspace = useWorkspace();
      select = workspace.selectMember;
      return workspace.mode === "workspace" ? <Thread turn={turn} /> : null;
    }
    render(
      <WorkspaceProvider>
        <Shell />
      </WorkspaceProvider>,
    );
    await screen.findByRole("link", { name: "register_all()" });
    expect(new URLSearchParams(codeRefRequests(urls)[0].split("?")[1]).get("repo")).toBe("api");

    // Switch to `web`: `api`'s links go, and the turn is asked of `web` once.
    act(() => select("web"));
    await waitFor(() => expect(codeRefRequests(urls)).toHaveLength(2));
    expect(new URLSearchParams(codeRefRequests(urls)[1].split("?")[1]).get("repo")).toBe("web");
    await act(async () => {});
    expect(screen.queryAllByRole("link")).toHaveLength(0);

    // Back to `api`: its links return from the memo, with no further request.
    act(() => select("api"));
    expect(await screen.findByRole("link", { name: "register_all()" })).toBeInTheDocument();
    expect(codeRefRequests(urls)).toHaveLength(2);
  });

  it("opens the Graph view in place on a plain click", async () => {
    stubCodeRefs();
    render(<Thread turn={finalized(ANSWER)} />);
    await userEvent.click(await screen.findByRole("link", { name: "src/registry.rs:3" }));
    expect(window.location.pathname).toBe("/graph");
    expect(window.location.search).toBe("?seed=src%2Fregistry.rs&granularity=file");
  });

  it("leaves every span plain when the resolver is unreachable", async () => {
    vi.stubGlobal("fetch", vi.fn(() => Promise.resolve({ ok: false, status: 500 } as Response)));
    render(<Thread turn={finalized(ANSWER)} />);
    await act(async () => {});
    expect(screen.queryAllByRole("link")).toHaveLength(0);
  });
});
