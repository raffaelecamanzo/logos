import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { THREAD_TITLE_MAX, type ThreadSummary } from "./chatModel.ts";
import { ThreadList } from "./ThreadList.tsx";

// The rail's inline rename (S-623, FR-UI-48): a rename icon or a double-click opens a
// field on the row; Enter saves, Esc cancels, an empty name is refused.

const THREADS: ThreadSummary[] = [
  { id: 5, title: "Old name", updated_at: 200 },
  { id: 4, title: "Another conversation", updated_at: 100 },
];

/** Render the rail; `withRename: false` leaves the rename handler out. */
function renderRail({ withRename = true } = {}) {
  const onRename = withRename ? vi.fn() : undefined;
  render(
    <ThreadList
      threads={THREADS}
      activeThreadId={5}
      onSelect={vi.fn()}
      onNewChat={vi.fn()}
      onDelete={vi.fn()}
      onRename={onRename}
    />,
  );
  return onRename;
}

function renameField(title = "Old name") {
  return screen.getByRole("textbox", { name: `Rename conversation “${title}”` });
}

afterEach(cleanup);

describe("ThreadList inline rename", () => {
  it("the rename icon opens a field holding the current name, and Enter saves the normalized name", async () => {
    const user = userEvent.setup();
    const onRename = renderRail();
    await user.click(screen.getByRole("button", { name: "Rename conversation “Old name”" }));
    const field = renameField();
    expect(field).toHaveValue("Old name");
    expect(field).toHaveFocus();
    expect(field).toHaveAttribute("maxLength", String(THREAD_TITLE_MAX));

    await user.clear(field);
    await user.type(field, "  Binder   map {Enter}");
    expect(onRename).toHaveBeenCalledTimes(1);
    expect(onRename).toHaveBeenCalledWith(5, "Binder map");
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
  });

  it("a double-click on the title opens the same field", async () => {
    const user = userEvent.setup();
    renderRail();
    await user.dblClick(screen.getByRole("button", { name: "Another conversation" }));
    expect(renameField("Another conversation")).toHaveValue("Another conversation");
  });

  it("Esc cancels: nothing is saved, the title is back, and the key goes no further", async () => {
    const user = userEvent.setup();
    const onRename = renderRail();
    const viewWide = vi.fn();
    window.addEventListener("keydown", viewWide);
    try {
      await user.click(screen.getByRole("button", { name: "Rename conversation “Old name”" }));
      await user.type(renameField(), " edited");
      await user.keyboard("{Escape}");
    } finally {
      window.removeEventListener("keydown", viewWide);
    }
    expect(onRename).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Old name" })).toBeInTheDocument();
    const escapes = viewWide.mock.calls.filter(([e]) => (e as KeyboardEvent).key === "Escape");
    expect(escapes, "a rename's Esc never reaches a view-wide handler").toHaveLength(0);
  });

  it("an empty name is refused: the field stays open, says so, and nothing is sent", async () => {
    const user = userEvent.setup();
    const onRename = renderRail();
    await user.click(screen.getByRole("button", { name: "Rename conversation “Old name”" }));
    const field = renameField();
    await user.clear(field);
    await user.type(field, "   {Enter}");
    expect(onRename).not.toHaveBeenCalled();
    expect(field).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByText("A conversation needs a name.")).toBeInTheDocument();

    // Typing again clears the refusal; a real name then saves.
    await user.type(field, "Kept{Enter}");
    expect(onRename).toHaveBeenCalledTimes(1);
    expect(onRename).toHaveBeenCalledWith(5, "Kept");
  });

  it("an Enter or Esc that belongs to an IME composition neither saves nor cancels", () => {
    const onRename = renderRail();
    fireEvent.click(screen.getByRole("button", { name: "Rename conversation “Old name”" }));
    fireEvent.change(renameField(), { target: { value: "にほ" } });
    fireEvent.keyDown(renameField(), { key: "Enter", keyCode: 229, isComposing: true });
    fireEvent.keyDown(renameField(), { key: "Escape", keyCode: 229, isComposing: true });
    expect(onRename).not.toHaveBeenCalled();
    expect(renameField()).toHaveValue("にほ");

    // The composition ends; the next Enter is the user's and saves.
    fireEvent.keyDown(renameField(), { key: "Enter" });
    expect(onRename).toHaveBeenCalledWith(5, "にほ");
  });

  it("an unchanged name closes the field without a write", async () => {
    const user = userEvent.setup();
    const onRename = renderRail();
    await user.click(screen.getByRole("button", { name: "Rename conversation “Old name”" }));
    await user.keyboard("{Enter}");
    expect(onRename).not.toHaveBeenCalled();
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
  });

  it("leaving the field cancels the rename", async () => {
    const onRename = renderRail();
    fireEvent.click(screen.getByRole("button", { name: "Rename conversation “Old name”" }));
    fireEvent.change(renameField(), { target: { value: "Half typed" } });
    fireEvent.blur(renameField());
    expect(onRename).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Old name" })).toBeInTheDocument();
  });

  it("opening a rename disarms a pending delete confirm", async () => {
    const user = userEvent.setup();
    renderRail();
    await user.click(screen.getByRole("button", { name: "Delete conversation “Old name”" }));
    expect(screen.getByRole("group", { name: "Confirm deleting “Old name”" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Rename conversation “Another conversation”" }));
    expect(screen.queryByRole("group")).not.toBeInTheDocument();
  });

  it("without a rename handler the rail offers no rename", () => {
    renderRail({ withRename: false });
    expect(screen.queryByRole("button", { name: /^Rename conversation/ })).not.toBeInTheDocument();
    fireEvent.doubleClick(screen.getByRole("button", { name: "Old name" }));
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
  });
});
