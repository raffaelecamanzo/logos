/*
 * The tool-panel register (S-617, CR-203 §3.2 A, FR-UI-39). A tool panel is a
 * search box, a query form, a configuration editor, the chat or a wiki page
 * body: it presents no figure, so it is exempt from why, action and where. It
 * still takes the layout standard and states in one line what it is for: a view
 * renders it as `<Widget panel="key" title=…>`, and the frame reads the line
 * from here.
 *
 * Each panel is listed by name, with the reason it is a panel rather than a
 * figure widget, so the exemption stays explicit and reviewable. A key no view
 * renders, or a view naming a key that is not here, fails the widget source scan
 * (`views/widgetScan.test.ts`).
 */

import type { CopyText } from "./types.ts";

export interface ToolPanel {
  /** The panel as a reader knows it. */
  readonly name: string;
  /** One line: what the panel is for. Rendered as the panel's explanation. */
  readonly what: CopyText;
  /** Why it is a tool panel rather than a figure widget. */
  readonly reason: string;
}

export const TOOL_PANELS = {
  // ── Graph ────────────────────────────────────────────────────────────────
  graphQuery: {
    name: "Query the whole graph",
    what: "Finds nodes anywhere in the graph by name, kind, layer or file, or lists the callers, callees or impact of one symbol.",
    reason: "A query form: it shows nothing until you ask it something.",
  },
  graphTable: {
    name: "Graph nodes & edges",
    what: "The nodes and edges the canvas draws, as tables you can read with a keyboard or a screen reader.",
    reason: "The accessible twin of the canvas: it restates what the canvas draws and adds no figure of its own.",
  },
  decisions: {
    name: "Decisions & docs",
    what: "The requirements, architecture decisions and stories that trace to the symbol you lock on the canvas.",
    reason: "A lookup driven by the canvas selection: it shows nothing until a symbol is locked.",
  },

  // ── Wiki ─────────────────────────────────────────────────────────────────
  wikiStartHere: {
    name: "Start here",
    what: "The first page to read in this wiki.",
    reason: "Navigation into the wiki, with no figure.",
  },
  wikiWelcome: {
    name: "Welcome to the wiki",
    what: "How the wiki is organised, and how many agent-written pages it holds.",
    reason: "An introduction to the wiki's sections; its one count is the page total, not a finding.",
  },
  wikiSearch: {
    name: "Search",
    what: "Searches the titles and bodies of every wiki page as you type.",
    reason: "A search box: it shows nothing until you type.",
  },
  wikiPage: {
    name: "Wiki page",
    what: "One wiki page, as an agent wrote it about this codebase.",
    reason: "A wiki page body: prose to read, with no figure.",
  },
  wikiAnchors: {
    name: "Anchors",
    what: "The code entities this page documents, each marked fresh or stale against the current code.",
    reason: "Part of the page reader: the page's own list of what it documents, read beside its body.",
  },

  // ── Config ───────────────────────────────────────────────────────────────
  policyFile: {
    name: "Policy file editor",
    what: "Edits one policy file in place; Save checks the whole file and writes it, and Apply puts it into effect.",
    reason: "A configuration editor: its fields are settings you change, not figures.",
  },
  chatKey: {
    name: "Chat API key",
    what: "Sets the API key the chat uses; the key is stored as a secret and only its last four characters are ever shown.",
    reason: "A configuration editor for one secret.",
  },
  graphConsistency: {
    name: "Graph consistency check",
    what: "Re-indexes the project into a throwaway copy and compares it with the live graph, when you ask.",
    reason: "An on-demand check: it shows nothing until you run it, and its report is the answer to that run.",
  },
  workspaceConfigGroup: {
    name: "Workspace configuration group",
    what: "Edits one of the workspace's own files, named beside the title; each group saves on its own.",
    reason: "A configuration editor for a workspace-root file.",
  },

  // ── Chat ─────────────────────────────────────────────────────────────────
  chatConversation: {
    name: "Conversation",
    what: "Ask about this code; each answer is built from the graph and the source.",
    reason: "The chat: a conversation, with no figure.",
  },

  // ── Workspace ────────────────────────────────────────────────────────────
  buildLayerTable: {
    name: "Build dependencies (map)",
    what: "The build dependencies the map draws, as a table you can read with a keyboard or a screen reader.",
    reason: "The accessible twin of the map's build layer; the Cross-service coverage tab holds the figure widget for it.",
  },
  impactResult: {
    name: "Cross-service impact result",
    what: "The callers that reach the symbol you traced, in one service.",
    reason: "The answer to a query form (the Cross-service impact tab, hidden from the web UI): it shows nothing until a symbol is traced.",
  },
} as const satisfies Record<string, ToolPanel>;

export type ToolPanelKey = keyof typeof TOOL_PANELS;

/** Whether `key` names a registered tool panel (an own key, never an inherited one). */
export function isToolPanelKey(key: string): key is ToolPanelKey {
  return Object.hasOwn(TOOL_PANELS, key);
}
