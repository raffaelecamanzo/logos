/*
 * The `chat` extension's SPA code (S-643, FR-UI-51, CR-213, ADR-74) — what the bundle
 * mounts while the server reports the extension active (`GET /api/v1/extensions`):
 * the member Chat and the Workspace Chat, their sidebar icon, and the chat config
 * panels of both Config views. Registered through `virtual:logos-extensions`
 * (`extensions/composed.ts`), and keyed by the ids and routes the server's manifest
 * uses (`logos-chat/src/lib.rs`, `ChatExtension::ui_manifest`) — the only join
 * between the two halves, proven end to end over the body that server serves
 * (`extensions/fixtures/chat.real.json`, mounted by `test/chat.ts` in the navigation
 * snapshot and the composed-entry suites).
 *
 * Nothing here mounts on its own: a build whose server registers no `chat` extension
 * (a `logos` binary without `agents`) leaves all of it inert, so the shell offers no
 * Chat entry, view or panel.
 */

import { IconChat } from "../../components/icons.tsx";
import type { ExtensionUi } from "../../extensions/registry.ts";
import { ChatKeyPanel } from "../config/ConfigView.tsx";
import { WorkspaceChatTierPanel } from "../workspace/WorkspaceConfigView.tsx";
import { ChatView } from "./ChatView.tsx";
import { WorkspaceChatView } from "./WorkspaceChatView.tsx";

/** The extension's id — the server's `Extension::id`. */
export const CHAT_EXTENSION_ID = "chat";

export const CHAT_EXTENSION_UI: ExtensionUi = {
  id: CHAT_EXTENSION_ID,
  // The Workspace Chat answers the member Chat's question for the whole workspace —
  // the member chat it replaces there (S-485) — so it carries the same icon.
  icons: { chat: IconChat, "workspace-chat": IconChat },
  views: {
    // S-190 — a React SSE client over the intent-guarded `POST /chat`.
    "/chat": ChatView,
    // S-485 — the Workspace Chat (FR-WS-34, ADR-71) over the S-482 workspace chat
    // routes. It lives beside the member chat because it reuses that surface whole.
    "/workspace-chat": WorkspaceChatView,
  },
  configPanels: {
    // The key card beneath the member's `[chat]` fields (S-452), and the whole
    // workspace chat tier — policy and credential (S-451) — in the workspace's.
    chat: { member: ChatKeyPanel, workspace: WorkspaceChatTierPanel },
  },
  // Where the built-in sidebar had them: the member Chat after Graph, and the
  // Workspace Chat after Workspace, ahead of the cross-service tabs that follow it.
  navAfter: { chat: "graph", "workspace-chat": "workspace" },
};
