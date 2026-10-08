# Playwright fixture projects (S-611)

Checked-in sources the browser harness serves through a tree-built
`logos serve --ui` (`web/ui/playwright.config.ts`):

- `single/` — one repository: the project views render against it.
- `workspace/` — a parent of two member repositories (`api`, `web`): the
  workspace views render against it.

`e2e/serve-fixture.sh` copies a fixture into `e2e/.run/<kind>/` (gitignored),
makes each repository a git repository there, and serves the copy, so the
checked-in tree is never indexed in place and no `.logos/` store is written
under it. Keep the sources small, and keep every function reachable from an
entry point: these files are part of this repository too.
