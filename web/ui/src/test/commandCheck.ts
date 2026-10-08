/*
 * The catalogue-command check (S-617, CR-203 §8 risk "an action names a command
 * that does not exist", FR-UI-39). Every `logos …` command a catalogue names to
 * the reader — since CR-206, in an absence or not-current sentence — is looked
 * up in the command reference, `docs/howto/commands.md`.
 * `docs/howto/` is tracked in the public repository, so the check runs in public
 * CI too; it reads nothing under `docs/specs` or `docs/planning`, which public CI
 * does not have.
 *
 * Pure functions over text: the test hands them the catalogue sources and the
 * reference, both as `?raw` imports.
 */

import * as ts from "typescript";

/** The commands the reference documents: each row of its index table names one
 *  (`init`, `workspace status`, `coverage ingest`, `wiki skill --emit` → `wiki skill`). */
export function documentedCommands(reference: string): Set<string> {
  const commands = new Set<string>();
  for (const [, cell] of reference.matchAll(/^\|\s*\[`([^`]+)`\]\(#[^)]*\)\s*\|/gm)) {
    const words = cell.split(/\s+/).filter((w) => /^[a-z][a-z-]*$/.test(w));
    if (words.length > 0) commands.add(words.join(" "));
  }
  return commands;
}

/** One `logos …` command as a catalogue names it: its first one or two words. */
export interface NamedCommand {
  readonly words: readonly string[];
  /** The text it was found in, for the failure message. */
  readonly in: string;
}

const COMMAND = /\blogos ([a-z][a-z-]*)(?: ([a-z][a-z-]*))?/g;

/** Every `logos …` command in the string literals of a TypeScript source — the
 *  words a reader can see, never a comment. */
export function commandsIn(file: string, source: string): NamedCommand[] {
  const sf = ts.createSourceFile(file, source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
  const found: NamedCommand[] = [];
  const visit = (node: ts.Node) => {
    if (ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node) || ts.isTemplateLiteralToken(node)) {
      for (const m of node.text.matchAll(COMMAND)) {
        found.push({ words: m[2] === undefined ? [m[1]] : [m[1], m[2]], in: `${file}: "${node.text}"` });
      }
    }
    ts.forEachChild(node, visit);
  };
  visit(sf);
  return found;
}

/**
 * Whether the reference documents `cmd`: the pair of words is a listed
 * subcommand (`workspace status`), or the first word is itself a listed command
 * and the second is its argument (`logos node <symbol>`, `logos xservice impact`).
 * A group the reference lists only by its subcommands (`coverage`, `wiki`) is
 * not a command on its own, so `logos coverage <anything unlisted>` fails.
 */
export function isDocumented(cmd: NamedCommand, documented: ReadonlySet<string>): boolean {
  const [first, second] = cmd.words;
  return (second !== undefined && documented.has(`${first} ${second}`)) || documented.has(first);
}
