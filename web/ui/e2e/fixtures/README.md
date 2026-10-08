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

`single.history.sh` gives the `single` copy its later commits: a second author
and a fix commit on the two files under the long `src/analysis/…/scoped/` path.
`serve-fixture.sh` runs it after the first commit, then indexes and runs
`logos hotspots`, so Files & Risk has churn to rank, ownership to disperse and a
path long enough to abbreviate (S-616). Those two files are reached from
`main.rs` through nested inline modules.
