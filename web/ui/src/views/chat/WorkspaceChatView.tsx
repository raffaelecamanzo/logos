/*
 * The Workspace Chat (S-485, FR-WS-34, FR-UI-35, ADR-71, frontend-design §4.22) —
 * the workspace chat Sprint 84 built (S-482: `POST /workspace/chat`, the
 * `/api/v1/workspace/chat/threads` tree, `<workspace root>/.logos/chat.db`), made
 * reachable from the Workspace section at its own route.
 *
 * It reuses the member chat's surface WHOLE (`ChatConfigured`): transcript, Activity
 * disclosure, composer, history rail and status band, with the chat it serves as a
 * parameter. What is its own is what it answers for:
 *
 *   - the heading names its scope in words — the workspace and its member count —
 *     and reads no member selection, so a screenshot says what it answered for;
 *   - the verdict is the workspace tier's (`workspaceChatReadiness`), the
 *     resolution the workspace turn dials; no member's `[chat]` configures it, so
 *     configure-first names the workspace root and links Workspace Config, never a
 *     member's Config;
 *   - the consent disclosure names every member's effective read roots, from the
 *     engine-free read-roots read-model — never from `GET /api/v1/config?repo=<m>`,
 *     whose extractor would start each member's engine (NFR-PE-10);
 *   - its client state is keyed by the `workspace` scope, so neither a member
 *     chat's remembered thread nor its consent carries over;
 *   - its rail lists the workspace's conversations only, and an empty rail says
 *     where member conversations live (a `--standalone` serve of that member).
 *
 * App-scoped (`nav.ts`): every read here is the same for every member, so the shell
 * does not remount it on a member switch, and its reads name no member.
 */

import { fetchWorkspaceChatConfig, fetchWorkspaceChatReadRoots } from "../../api/chatClient.ts";
import { AsyncResource, useApiResource } from "../../api/hooks.tsx";
import { Callout, EmptyState, ErrorPanel, LoadingState } from "../../components/index.ts";
import { urlWithMember } from "../../workspace/scope.ts";
import { useWorkspace } from "../../workspace/WorkspaceContext.tsx";
import { ChatConfigured } from "./ChatView.tsx";
import {
  WORKSPACE_CHAT_SCOPE,
  WORKSPACE_CONFIG_HREF,
  workspaceChatReadiness,
  workspaceConfigureFirstCopy,
  workspaceReadRootsDisclosure,
  type MemberChatReadRoots,
  type WorkspaceChatConfigReadModel,
  type WorkspaceConfigureFirst,
  type WorkspaceTierUnreadable,
} from "./chatModel.ts";
import styles from "./Chat.module.css";

/** The heading's words: the workspace and how many members it answers across. */
export function workspaceChatHeading(workspace: string, members: number): string {
  return `Workspace chat · ${workspace} · ${members} ${members === 1 ? "member" : "members"}`;
}

export function WorkspaceChatView() {
  const { mode, error, workspace, members } = useWorkspace();
  // `[]` is the WHOLE dependency array — the app-scoped contract (FR-UI-35, ADR-66):
  // the tier and every member's read roots are one answer for every member, so a
  // member switch must not re-issue them. The shell's `scope: "app"` declaration is
  // the other half: it does not remount this view on a switch.
  const reads = useApiResource<[WorkspaceChatConfigReadModel, MemberChatReadRoots[]]>(
    () => Promise.all([fetchWorkspaceChatConfig(), fetchWorkspaceChatReadRoots()]),
    [],
  );

  if (mode === "loading") {
    return (
      <div className={styles.view}>
        <LoadingState label="Reading the workspace…" />
      </div>
    );
  }
  if (error) {
    return (
      <div className={styles.view}>
        <ErrorPanel>The workspace could not be read: {error.message}</ErrorPanel>
      </div>
    );
  }
  if (mode !== "workspace" || workspace === null) {
    return (
      <div className={styles.view}>
        <EmptyState message="Not a workspace — this serve has a single repository root. Its chat is the Chat view." />
      </div>
    );
  }

  return (
    <div className={styles.view}>
      <h1 className={styles.title}>{workspaceChatHeading(workspace, members.length)}</h1>
      <AsyncResource resource={reads} loadingLabel="Loading the workspace chat…">
        {([config, readRoots]) => {
          const verdict = workspaceChatReadiness(config);
          if (verdict.ready) {
            return (
              <ChatConfigured
                ready={verdict}
                surface="workspace"
                storageScope={WORKSPACE_CHAT_SCOPE}
                disclosure={workspaceReadRootsDisclosure(readRoots)}
                emptyRailNote={<EmptyRailNote />}
              />
            );
          }
          return verdict.unreadable ? (
            <TierUnreadable state={verdict} />
          ) : (
            <WorkspaceConfigureFirst state={verdict} workspace={workspace} />
          );
        }}
      </AsyncResource>
    </div>
  );
}

/** The Workspace Config view's href, carrying the active member so returning to a
 *  member-scoped view reopens the same member — as every other link to it does. */
function useWorkspaceConfigHref(): string {
  const { member } = useWorkspace();
  return urlWithMember(WORKSPACE_CONFIG_HREF, member);
}

/**
 * The configure-first state (FR-WS-34, NFR-CC-04): a muted advisory — not an error,
 * and no composer — naming the workspace root, the missing half and any present
 * one, saying that a member's `[chat]` does not configure this chat, and linking
 * Workspace Config (§4.21). It never links a member's Config: no member's `[chat]`
 * drives this chat.
 */
function WorkspaceConfigureFirst({
  state,
  workspace,
}: {
  state: WorkspaceConfigureFirst;
  workspace: string;
}) {
  const copy = workspaceConfigureFirstCopy(state, workspace);
  const href = useWorkspaceConfigHref();
  return (
    <Callout label="CONFIGURE" tone="muted">
      <p>{copy.summary}</p>
      {copy.present && <p>{copy.present}</p>}
      <p>{copy.memberNote}</p>
      <p>
        {copy.action} in{" "}
        {state.workspaceFiles.map((file, i) => (
          <span key={file}>
            {i > 0 && " and "}
            <code>{file}</code>
          </span>
        ))}{" "}
        — the <a href={href}>Workspace Config</a> view edits{" "}
        {state.workspaceFiles.length === 1 ? "that file" : "both files"} — then return here to
        start chatting. Until then no outbound call is possible.
      </p>
    </Callout>
  );
}

/** A workspace tier file that does not parse: the verdict cannot be read, so the
 *  fault is named (by file and position only) and Workspace Config — its repair
 *  path — is linked, rather than passing it off as an unconfigured chat. */
function TierUnreadable({ state }: { state: WorkspaceTierUnreadable }) {
  const href = useWorkspaceConfigHref();
  return (
    <ErrorPanel>
      The workspace chat&apos;s configuration at the workspace root could not be read
      {state.faults.length > 0 && <>: {state.faults.join("; ")}</>}. Repair it in the{" "}
      <a href={href}>Workspace Config</a> view.
    </ErrorPanel>
  );
}

/** The empty rail (frontend-design §4.22): this rail lists the workspace's own
 *  conversations, so it says where a member's live instead of implying there are
 *  none. */
function EmptyRailNote() {
  return (
    <>
      No workspace conversations yet. Member conversations are not listed here — each is
      available in a <code>--standalone</code> serve of that member.
    </>
  );
}
