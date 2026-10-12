/*
 * The four built-in, prompt-only chat presets (S-625, FR-UI-49, CR-204 C7).
 *
 * A preset steers a turn's planner and Synthesizer preambles and nothing else: the
 * tools, the subagent roster, the budget tree and the sandbox are the same under
 * every preset (ADR-41, BR-36). The server holds the preamble text
 * (`chat-agent/src/orchestrator/preset.rs`); the SPA names the preset on the turn
 * POST by its wire id, which the server validates against the same four values.
 *
 * The choice is the CONVERSATION's: it is remembered per conversation under the
 * chat's storage scope (`chatStorageKey`, like the open-conversation key), so the
 * member chat and the Workspace Chat never share a choice, and two conversations of
 * one chat each keep their own.
 */

import { chatStorageKey, type ChatStorageScope } from "./chatModel.ts";

/** A preset's wire id — the value of the turn POST's `preset` field. */
export type ChatPresetId = "default" | "impact_analysis" | "onboarding" | "troubleshooting";

/** One preset as the composer menu lists it. */
export interface ChatPreset {
  id: ChatPresetId;
  label: string;
  /** The one-line description the menu shows under the label. */
  description: string;
}

/** The preset a conversation runs under until the reader picks another. */
export const DEFAULT_PRESET: ChatPresetId = "default";

/** Every preset, Default first — the server's `Preset::ALL` order. */
export const CHAT_PRESETS: readonly ChatPreset[] = [
  {
    id: "default",
    label: "Default",
    description: "Answers the question as asked.",
  },
  {
    id: "impact_analysis",
    label: "Impact analysis",
    description: "Leads with what a change would reach and what to re-test.",
  },
  {
    id: "onboarding",
    label: "Onboarding",
    description: "Explains the code for someone new to it.",
  },
  {
    id: "troubleshooting",
    label: "Troubleshooting",
    description: "Diagnoses a fault and names the next check to run.",
  },
];

/** Whether `value` is one of the four wire ids — exactly, never by prefix or case. */
export function isChatPresetId(value: unknown): value is ChatPresetId {
  return CHAT_PRESETS.some((p) => p.id === value);
}

/** The menu entry for `id`. */
export function presetOf(id: ChatPresetId): ChatPreset {
  return CHAT_PRESETS.find((p) => p.id === id) ?? CHAT_PRESETS[0];
}

/** The localStorage key BASE holding each conversation's non-Default preset, as one
 *  JSON object from thread id to preset id — qualified by the chat's storage scope
 *  ({@link chatStorageKey}), because a thread id names a row in ONE chat's store. */
export const PRESETS_KEY = "logos.chat.presets";

/** Each conversation's remembered preset under `scope`. Only well-formed entries are
 *  kept — an integer thread id naming a non-Default preset — so a hand-edited or
 *  stale value can never send a preset the server would refuse. Empty when nothing
 *  is stored, the value is not an object, or storage is blocked. */
export function readStoredPresets(scope: ChatStorageScope): Map<number, ChatPresetId> {
  return loadStoredPresets(scope) ?? new Map();
}

/** {@link readStoredPresets}, but `null` when storage itself cannot be read — so a
 *  writer can tell "nothing is stored" (the stored map is authoritative) from
 *  "storage is blocked" (keep what this session holds). */
export function loadStoredPresets(scope: ChatStorageScope): Map<number, ChatPresetId> | null {
  let raw: string | null;
  try {
    raw = window.localStorage.getItem(chatStorageKey(PRESETS_KEY, scope));
  } catch {
    return null;
  }
  const presets = new Map<number, ChatPresetId>();
  try {
    const parsed: unknown = raw == null ? null : JSON.parse(raw);
    if (parsed == null || typeof parsed !== "object" || Array.isArray(parsed)) return presets;
    for (const [key, value] of Object.entries(parsed)) {
      const id = Number(key);
      if (Number.isSafeInteger(id) && String(id) === key && isChatPresetId(value) && value !== DEFAULT_PRESET) {
        presets.set(id, value);
      }
    }
  } catch {
    /* malformed storage: every conversation reads as Default */
  }
  return presets;
}

/** Remember `presets` under `scope` — best-effort: blocked storage only means the
 *  choice is not remembered across a reload. */
export function storePresets(scope: ChatStorageScope, presets: ReadonlyMap<number, ChatPresetId>): void {
  try {
    const key = chatStorageKey(PRESETS_KEY, scope);
    if (presets.size === 0) window.localStorage.removeItem(key);
    else window.localStorage.setItem(key, JSON.stringify(Object.fromEntries(presets)));
  } catch {
    /* non-fatal: the preset still applies to this session's turns */
  }
}

/** `presets` with `threadId` set to `preset` — removed when it is Default, so only
 *  steered conversations take storage. */
export function withPreset(
  presets: ReadonlyMap<number, ChatPresetId>,
  threadId: number,
  preset: ChatPresetId,
): Map<number, ChatPresetId> {
  const next = new Map(presets);
  if (preset === DEFAULT_PRESET) next.delete(threadId);
  else next.set(threadId, preset);
  return next;
}
