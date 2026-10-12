/*
 * The welcome chips (S-621, FR-UI-46, CR-204 C1) — the fixed templates an empty chat
 * thread offers, and the facts they are filled with.
 *
 * A chip is a FIXED template. Its one slot is filled with a fact an existing
 * `/api/v1` read-model returned (CRA-03): the top-ranked hotspot, the riskiest
 * untested file and the indexed file count for a member; the workspace name and
 * roster size, the busiest broker topic and the largest member for the workspace.
 * Where a fact was not returned — the read failed, the board is empty, the graph is
 * not indexed — that template's slot becomes its GENERIC form, which names nothing
 * and counts nothing (NFR-CC-04). No model is consulted to write a chip (NFR-SE-07):
 * this module is a pure function of the read-models it is handed, which is also why
 * it can be tested without a provider.
 *
 * The Workspace Chat reads only what the shell already holds (the roster). It is
 * built engine-free (NFR-PE-10) and `workspace/status` — a fan-out that opens every
 * member's store — is the service map's and the Workspace tab's read, not a welcome
 * decoration's; so no topic ranking or member size is offered either, and no
 * superlative is ever claimed over a population that may be only partly measured.
 *
 * Not served, so not offered (CRA-03): a "largest module". The Architecture bundle's
 * module rows carry a name and a layer, never a size, and the structure read behind
 * it is a whole-graph computation rather than a stored figure. Adding a projection
 * for it is out of scope; the template is left out instead.
 */

import type { FilesModel } from "../../api/types.ts";

/** One suggestion: the text a click sends, and whether it names a returned fact. */
export interface SuggestionChip {
  /** Stable identity of the template (not of its fill). */
  key: string;
  /** The words shown on the chip — and, verbatim, the first message when sent. */
  text: string;
  /** `true` when the text names a fact a read-model returned; `false` when generic. */
  grounded: boolean;
}

// ── Inputs: only the slice of each read-model a template reads ──────────────────────

/** The slice of `GET /api/v1/files` the member chips read. {@link FilesModel} is
 *  assignable; the narrow shape lets a caller (and a test) hand over only this. */
export interface FilesReadModel {
  status?: Pick<FilesModel["status"], "indexed" | "file_count">;
  hotspots?: { files?: readonly { path?: string }[] };
}

// ── Facts ──────────────────────────────────────────────────────────────────────────

/** What the member read-models returned; `null` is "not returned", never a default. */
export interface MemberChipFacts {
  /** The top-ranked hotspot file. */
  topHotspot: string | null;
  /** The top-ranked file lacking fresh positive coverage. */
  riskiestUntested: string | null;
  /** Indexed file count, from an indexed graph only. */
  fileCount: number | null;
}

/** What the roster returned; `null` / `0` is "not returned". */
export interface WorkspaceChipFacts {
  workspace: string | null;
  /** The roster's size. */
  memberCount: number;
  /** The shell's selected member, when the roster names it. */
  member: string | null;
}

/** A name is quoted inside a code span, so it must be a non-blank single line with no
 *  backtick — anything else could not be shown verbatim, and a chip that altered a
 *  returned name would name a fact the read-model did not return. */
function quotable(name: unknown): name is string {
  return typeof name === "string" && name.trim() !== "" && !/[`\r\n]/.test(name);
}

/** The first ranked path of a hotspot board, or `null` (empty, degraded, absent). */
function firstPath(model: FilesReadModel | null | undefined): string | null {
  const path = model?.hotspots?.files?.[0]?.path;
  return quotable(path) ? path : null;
}

/** Read the member chips' facts from the two `/files` boards: the plain one (top
 *  hotspot, indexed file count) and the `?untested` one (riskiest untested file). */
export function memberChipFacts(
  board: FilesReadModel | null | undefined,
  untested: FilesReadModel | null | undefined,
): MemberChipFacts {
  const status = board?.status;
  const count = status?.indexed === true ? status.file_count : undefined;
  return {
    topHotspot: firstPath(board),
    riskiestUntested: firstPath(untested),
    fileCount: typeof count === "number" && count > 0 ? count : null,
  };
}

/** Read the workspace chips' facts from the shell's own roster read: the workspace
 *  name, the members, and the selected member (only if the roster lists it). */
export function workspaceChipFacts(
  workspace: string | null,
  members: readonly string[],
  member: string | null,
): WorkspaceChipFacts {
  return {
    workspace: quotable(workspace) ? workspace : null,
    memberCount: members.length,
    member: quotable(member) && members.includes(member) ? member : null,
  };
}

// ── Templates ──────────────────────────────────────────────────────────────────────

const code = (name: string) => `\`${name}\``;

/** The member chat's chips: a grounded form per fact, the generic form otherwise. */
export function memberChips(facts: MemberChipFacts): SuggestionChip[] {
  const { topHotspot, riskiestUntested, fileCount } = facts;
  return [
    topHotspot !== null
      ? {
          key: "hotspot",
          grounded: true,
          text: `Why is ${code(topHotspot)} the top hotspot, and what depends on it?`,
        }
      : { key: "hotspot", grounded: false, text: "Which files are the riskiest to change?" },
    riskiestUntested !== null
      ? {
          key: "untested",
          grounded: true,
          text: `What would it take to cover ${code(riskiestUntested)}, the riskiest untested file?`,
        }
      : { key: "untested", grounded: false, text: "Where is the riskiest untested code?" },
    fileCount !== null
      ? {
          key: "tour",
          grounded: true,
          text: `Give me a tour of this project's ${fileCount.toLocaleString("en-US")} indexed files.`,
        }
      : { key: "tour", grounded: false, text: "Give me a tour of this project's structure." },
  ];
}

/** The Workspace Chat's chips — keyed apart from the member chat's, since the two
 *  answer for different scopes. The broker chip is a fixed question with no slot. */
export function workspaceChips(facts: WorkspaceChipFacts): SuggestionChip[] {
  const { workspace, memberCount, member } = facts;
  return [
    memberCount >= 2
      ? {
          key: "ws-members",
          grounded: true,
          text: `How do the ${memberCount} members of ${workspace !== null ? code(workspace) : "this workspace"} depend on each other?`,
        }
      : {
          key: "ws-members",
          grounded: false,
          text: "How do this workspace's services depend on each other?",
        },
    {
      key: "ws-broker",
      grounded: false,
      text: "Which services talk to each other over a message broker?",
    },
    memberCount >= 2 && member !== null
      ? {
          key: "ws-member",
          grounded: true,
          text: `What does ${code(member)} do, and which other members depend on it?`,
        }
      : { key: "ws-member", grounded: false, text: "Give me a tour of this workspace's services." },
  ];
}
