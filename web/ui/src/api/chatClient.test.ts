import { afterEach, describe, expect, it, vi } from "vitest";

// Mock the transport seams so the test asserts the request CONTRACT chatClient
// builds (route, body encoding, headers, intent plumbing) without a live fetch or
// the module-load intent token.
vi.mock("../intent.ts", () => ({
  apiMutate: vi.fn(() => Promise.resolve({ ok: true, status: 200 } as Response)),
}));
vi.mock("./client.ts", () => ({
  apiFetch: vi.fn(() => Promise.resolve({})),
}));

import { apiMutate } from "../intent.ts";
import { apiFetch } from "./client.ts";
import * as chatClient from "./chatClient.ts";
import {
  CHAT_ROUTE,
  chatThreadDeleteRoute,
  deleteChatThread,
  fetchChatConfig,
  fetchThreadMessages,
  fetchThreads,
  fetchWorkspaceChatConfig,
  fetchWorkspaceChatReadRoots,
  MEMBER_CHAT_ROUTES,
  streamChatTurn,
  WORKSPACE_CHAT_ROUTE,
  WORKSPACE_CHAT_ROUTES,
} from "./chatClient.ts";
import { setScopedMember } from "../workspace/scope.ts";

const mockMutate = vi.mocked(apiMutate);
const mockFetch = vi.mocked(apiFetch);

afterEach(() => {
  vi.clearAllMocks();
  setScopedMember(null);
});

describe("streamChatTurn", () => {
  it("POSTs the form-encoded question with the SSE Accept header over the intent seam", async () => {
    const ctrl = new AbortController();
    await streamChatTurn(MEMBER_CHAT_ROUTES, "what is risky?", null, ctrl.signal);
    expect(mockMutate).toHaveBeenCalledWith(CHAT_ROUTE, {
      headers: { "Content-Type": "application/x-www-form-urlencoded", Accept: "text/event-stream" },
      body: "q=what%20is%20risky%3F",
      signal: ctrl.signal,
    });
  });

  it("omits the thread field for a fresh conversation (byte-identical single-thread body)", async () => {
    await streamChatTurn(MEMBER_CHAT_ROUTES, "hi", null);
    expect(mockMutate.mock.calls[0][1]?.body).toBe("q=hi");
  });

  it("appends the active thread id when continuing a conversation", async () => {
    await streamChatTurn(MEMBER_CHAT_ROUTES, "more", 7);
    expect(mockMutate.mock.calls[0][1]?.body).toBe("q=more&thread=7");
  });
});

describe("fetchThreads", () => {
  it("GETs the same-origin thread list (S-209 read API, no re-added route)", async () => {
    await fetchThreads(MEMBER_CHAT_ROUTES);
    expect(mockFetch).toHaveBeenCalledWith("chat/threads");
  });
});

describe("fetchThreadMessages", () => {
  it("GETs one thread's transcript by id", async () => {
    await fetchThreadMessages(MEMBER_CHAT_ROUTES, 42);
    expect(mockFetch).toHaveBeenCalledWith("chat/threads/42");
  });
});

describe("deleteChatThread (S-211, FR-UI-26)", () => {
  it("POSTs the per-thread delete route over the intent seam", async () => {
    await deleteChatThread(MEMBER_CHAT_ROUTES, 42);
    expect(mockMutate).toHaveBeenCalledWith("/api/v1/chat/threads/42/delete", {});
  });

  it("addresses exactly the named conversation (never a global wipe)", () => {
    expect(chatThreadDeleteRoute(MEMBER_CHAT_ROUTES, 7)).toBe("/api/v1/chat/threads/7/delete");
    expect(chatThreadDeleteRoute(MEMBER_CHAT_ROUTES, 8)).toBe("/api/v1/chat/threads/8/delete");
  });

  it("returns the raw response so 204 / 404 / fault stay distinguishable", async () => {
    mockMutate.mockResolvedValueOnce({ ok: false, status: 404 } as Response);
    await expect(deleteChatThread(MEMBER_CHAT_ROUTES, 9)).resolves.toMatchObject({ status: 404 });
  });
});

describe("the retired global clear (S-211, ADR-47)", () => {
  it("exports no clear-all helper or route — per-conversation delete is the only path", () => {
    const surface = chatClient as unknown as Record<string, unknown>;
    expect(surface.clearChatHistory).toBeUndefined();
    expect(surface.CHAT_CLEAR_ROUTE).toBeUndefined();
  });
});

describe("fetchChatConfig", () => {
  it("GETs the same-origin config read-model", async () => {
    await fetchChatConfig();
    expect(mockFetch).toHaveBeenCalledWith("config");
  });
});

// ── S-485: the workspace chat over the same helpers ───────────────────────────

describe("the workspace chat's routes (S-485, S-482)", () => {
  it("streams the turn to the workspace route, carrying no member even when one is active", async () => {
    setScopedMember("api");
    await streamChatTurn(WORKSPACE_CHAT_ROUTES, "q", 3);
    expect(mockMutate).toHaveBeenCalledWith(WORKSPACE_CHAT_ROUTE, expect.anything());
    expect(mockMutate.mock.calls[0][1]?.body).toBe("q=q&thread=3");
  });

  it("scopes the MEMBER chat's turn and delete to the active member — the contrast", async () => {
    setScopedMember("api");
    await streamChatTurn(MEMBER_CHAT_ROUTES, "q", null);
    await deleteChatThread(MEMBER_CHAT_ROUTES, 4);
    expect(mockMutate.mock.calls.map((c) => c[0])).toEqual([
      "/chat?repo=api",
      "/api/v1/chat/threads/4/delete?repo=api",
    ]);
  });

  it("lists, opens and deletes the workspace's own conversations", async () => {
    setScopedMember("api");
    await fetchThreads(WORKSPACE_CHAT_ROUTES);
    await fetchThreadMessages(WORKSPACE_CHAT_ROUTES, 5);
    await deleteChatThread(WORKSPACE_CHAT_ROUTES, 5);
    // `apiFetch` itself scopes no `workspace/*` read (client.ts), so the endpoint
    // handed to it is what reaches the wire.
    expect(mockFetch.mock.calls.map((c) => c[0])).toEqual([
      "workspace/chat/threads",
      "workspace/chat/threads/5",
    ]);
    expect(mockMutate).toHaveBeenCalledWith("/api/v1/workspace/chat/threads/5/delete", {});
  });

  it("reads the workspace tier and the read roots off the two engine-free workspace routes", async () => {
    await fetchWorkspaceChatConfig();
    await fetchWorkspaceChatReadRoots();
    expect(mockFetch.mock.calls.map((c) => c[0])).toEqual([
      "workspace/config",
      "workspace/config/read-roots",
    ]);
  });
});
