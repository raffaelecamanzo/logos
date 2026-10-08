// Every `logos …` command a catalogue names is a documented command (S-617,
// FR-UI-39, CR-203 §8). The reference is `docs/howto/commands.md` — tracked in the
// public repository, so this runs in public CI; nothing here reads docs/specs or
// docs/planning, which public CI does not have.
import { describe, expect, it } from "vitest";

import reference from "../../../../docs/howto/commands.md?raw";
import { commandsIn, documentedCommands, isDocumented } from "../test/commandCheck.ts";

// The catalogues, and the tool-panel register whose lines a reader also sees.
const sources = import.meta.glob<string>(["/src/copy/**/*.copy.ts", "/src/copy/toolPanels.ts"], {
  query: "?raw",
  import: "default",
  eager: true,
});

const documented = documentedCommands(reference);

describe("the commands the catalogues name", () => {
  const named = Object.entries(sources).flatMap(([file, source]) => commandsIn(file, source));

  it("are every one documented in docs/howto/commands.md", () => {
    expect(named.filter((cmd) => !isDocumented(cmd, documented)).map((cmd) => `logos ${cmd.words.join(" ")} — ${cmd.in}`)).toEqual([]);
    // A finding, not a floor; only a check over nothing is refused.
    expect(named.length, "the catalogues name commands").toBeGreaterThan(0);
    const distinct = new Set(named.map((c) => c.words[0]));
    console.info(
      `catalogue commands: ${named.length} mentions of ${distinct.size} commands across ${Object.keys(sources).length} catalogue modules, checked against ${documented.size} documented commands`,
    );
  });

  it("reads the reference's command index, subcommands included", () => {
    for (const cmd of ["init", "index", "scan", "gate", "hotspots", "workspace status", "coverage ingest", "wiki write", "xservice"]) {
      expect(documented, cmd).toContain(cmd);
    }
    // A group is listed only by its subcommands.
    expect(documented).not.toContain("coverage");
  });
});

describe("the check itself (falsifiable)", () => {
  const check = (text: string) =>
    commandsIn("/src/copy/x.copy.ts", `export const x = ${JSON.stringify(text)};`).map((cmd) => isDocumented(cmd, documented));

  it("fails a catalogue naming a command that does not exist", () => {
    expect(check("Run logos frobnicate to fix it.")).toEqual([false]);
    expect(check("Run logos coverage frobnicate.")).toEqual([false]);
    expect(check("Run logos workspace frobnicate.")).toEqual([false]);
  });

  it("passes documented commands, with their arguments", () => {
    expect(check("Run logos index in that member.")).toEqual([true]);
    expect(check("Then logos gate --save, and logos node <symbol>.")).toEqual([true, true]);
    expect(check("Run logos workspace status, then logos coverage ingest <report>.")).toEqual([true, true]);
  });

  it("reads string literals only: a command in a comment, or the file logos.workspace.toml, is not one", () => {
    const src = `// logos frobnicate\nexport const x = "Edit logos.workspace.toml; Use Logos from your agent.";\n`;
    expect(commandsIn("/src/copy/x.copy.ts", src)).toEqual([]);
  });
});
