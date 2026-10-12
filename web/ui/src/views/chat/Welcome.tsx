/*
 * The empty thread's welcome (S-621, FR-UI-46, CR-204 C1): a centred greeting and
 * two to four suggestion chips, in place of the one-line hint the empty thread used
 * to carry.
 *
 * The chips are the fixed templates of `suggestionChips.ts`, filled from read-models
 * that already exist: the member chat reads the member's `/files` boards when it
 * mounts; the Workspace Chat reads nothing at all — it uses the roster and the
 * selected member the shell already holds, because the workspace's other read
 * (`workspace/status`) opens every member's store and the Workspace Chat is built
 * engine-free (NFR-PE-10). Those are plain GETs: no chip is produced by a model and
 * no provider request is made before a send (NFR-SE-07). A read that fails or answers
 * an empty shape is not an error here — it leaves its fact unreturned, so its
 * template shows the generic form (NFR-CC-04); the member chat's chips stay hidden
 * until both reads settle, so a chip is never replaced under the pointer.
 *
 * A chip sends through the runtime's EXISTING path — the thread's `append`, which is
 * the composer's own route into `onNew` — never a direct `streamChatTurn` call. The
 * consent gate is therefore the runtime's (`onNew` is a hard no-op before consent),
 * and whatever a turn is made to carry there (the conversation's preset, S-625) a
 * chip's turn carries too. Before consent the chips are disabled as well, so nothing
 * looks sendable that is not.
 */

import { useAui } from "@assistant-ui/react";

import { fetchFiles } from "../../api/client.ts";
import { useApiResource } from "../../api/hooks.tsx";
import { useWorkspace } from "../../workspace/WorkspaceContext.tsx";
import {
  memberChipFacts,
  memberChips,
  workspaceChipFacts,
  workspaceChips,
  type SuggestionChip,
} from "./suggestionChips.ts";
import styles from "./Chat.module.css";

/** A read that may fail: its failure is "the fact was not returned", never a throw —
 *  including the synchronous throw of an environment with no `fetch`. */
function attempt<T>(read: () => Promise<T>): Promise<T | null> {
  return Promise.resolve()
    .then(read)
    .catch(() => null);
}

export function Welcome({
  consented,
  surface,
  restoring,
}: {
  consented: boolean;
  surface: "member" | "workspace";
  /** A stored conversation is open but its messages have not arrived: the thread
   *  only LOOKS empty. A chip sent now would run in that conversation and the
   *  restore would then overwrite its transcript, so the chips wait. */
  restoring: boolean;
}) {
  const live = consented && !restoring;
  return surface === "workspace" ? (
    <WorkspaceWelcome consented={consented} live={live} />
  ) : (
    <MemberWelcome consented={consented} live={live} />
  );
}

function MemberWelcome({ consented, live }: { consented: boolean; live: boolean }) {
  // Scoped to the active member by `apiFetch` itself; the shell remounts the chat on
  // a member switch, so `[]` is the whole dependency array (as the config read's).
  const chips = useApiResource<SuggestionChip[]>(async () => {
    const [board, untested] = await Promise.all([
      attempt(() => fetchFiles()),
      attempt(() => fetchFiles(true)),
    ]);
    return memberChips(memberChipFacts(board, untested));
  }, []);
  return (
    <WelcomeBody
      subject="this project"
      chips={chips.data ?? null}
      consented={consented}
      live={live}
    />
  );
}

function WorkspaceWelcome({ consented, live }: { consented: boolean; live: boolean }) {
  // Engine-free (NFR-PE-10): only what the shell already read — the roster and the
  // selected member — so there is nothing to wait for and nothing to fetch. The
  // `workspace/status` fan-out opens every member's store; it is not a welcome's to
  // issue.
  const { workspace, members, member } = useWorkspace();
  const chips = workspaceChips(workspaceChipFacts(workspace, members, member));
  return <WelcomeBody subject="this workspace" chips={chips} consented={consented} live={live} />;
}

/** The greeting and the chip row. `chips` is `null` while the reads are in flight. */
function WelcomeBody({
  subject,
  chips,
  consented,
  live,
}: {
  subject: string;
  chips: SuggestionChip[] | null;
  consented: boolean;
  /** Whether a chip may be activated: consented, and no restore pending. */
  live: boolean;
}) {
  const aui = useAui();
  // Before consent the chips are `disabled`, and the runtime's `onNew` is the gate.
  const send = (text: string) => aui.thread().append(text);
  return (
    <div className={styles.welcome}>
      <p className={styles.welcomeTitle}>What would you like to know about {subject}?</p>
      <p className={`${styles.empty} ${styles.welcomeLead}`}>
        {consented
          ? "Start from a suggestion, or ask your own question below."
          : "Accept the notice above to start from a suggestion or ask your own question."}
      </p>
      {chips !== null && (
        <div className={styles.chips} role="group" aria-label="Suggested questions">
          {chips.map((chip) => (
            <button
              key={chip.key}
              type="button"
              className={styles.chip}
              disabled={!live}
              onClick={() => send(chip.text)}
            >
              {chip.text}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
