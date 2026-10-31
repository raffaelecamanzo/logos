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

`single.security.sarif` is a Semgrep report over the `single` copy's
`src/main.rs`: `serve-fixture.sh` ingests it after indexing, so the Security
page (S-670) renders findings in graph context, a redacted secret-class one
among them. It sits beside the fixture rather than in it, so the indexed tree
— and every other view's figures — stay as they were.

Its third result has a rule id and a path of 120 characters each (the path is the
report's own; no such file exists in the copy), so `security.project.spec.ts`
(S-793) can show the Findings table fits the page at 1024 px.

Its fourth result is in `tests/fixtures/payments.rs`, a path the security path rule
reads as test code, so `security.project.spec.ts` (S-796) can show the page lists
production code by default and the test-code finding under the code scope control.

Its fifth result is in `src/main.rs` under a rule tagged `LOW CONFIDENCE` beside
`precision: very-high`, so `security.project.spec.ts` (S-817) can show the page leave a
finding its scanner marks low-confidence out of the default list and state it as
"N more need review", and list it, tagged, under All.

`single.dev.security.sarif` is an OSV-Scanner report over `single/web/package-lock.json`,
whose one package is `"dev": true`: `serve-fixture.sh` ingests it after the Semgrep
report, so `security.project.spec.ts` (S-810) can show the page leave that finding out of
the default list, state it as "N more in dev-only dependencies", and list it, tagged DEV,
under All. The lockfile is not indexed, so no other view's figures move. The same report
also holds an advisory on the lockfile's production package `left-pad`, which its Trivy
run reports too under the CVE OSV-Scanner lists as an alias, so `security.project.spec.ts`
(S-809) can show one row naming both scanners. Both runs are in one report because a
second OSV-Scanner report would replace the first's snapshot.

`workspace.security.sarif` is a Semgrep report over the `workspace` copy's
`api/src/main.rs`: `serve-fixture.sh` ingests it into `api` alone, so Workspace
security (S-674) renders a scanned member beside a never-scanned one (`web`).
