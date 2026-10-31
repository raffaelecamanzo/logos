# Contributing — collaboration flow and maintainer checklist

This page is for the repository owner and maintainers. Outside contributors
start at [CONTRIBUTING.md](../../CONTRIBUTING.md): how to file an issue, how an
accepted issue becomes planned work, and how a pull request is reviewed.

## Two kinds of pull request

| Source | Branch | Target | Opened by |
|---|---|---|---|
| Planned work | `sprint/N`, published as one commit | `main` | the owner, with `scripts/publish-sprint.sh` |
| Contributor change | the contributor's own branch or fork | `main` | the contributor |

Both are merged on GitHub with **Rebase and merge**, which keeps `main` linear
and keeps the commit message unchanged. A fix that is outside any sprint is
developed on `hotfix/HF-N`, branched from `main`.

## The per-sprint pull request

An approved sprint reaches the public repository as **one pull request carrying
one commit**. The commit's parent is `origin/main` and its tree is the tree of
local `main` after the sprint was merged into it, so publishing can never drop
a change that is already public.

1. On `main`, bring local `main` up to date with the public one, then merge the
   approved sprint into it (the script refuses, exit 2, while `sprint/N` is not
   merged into `main`). The sprint coordinator makes this merge commit when the
   sprint is approved; if it already has, the second command reports "Already up
   to date":

   ```bash
   git fetch origin && git merge origin/main
   git merge --no-ff sprint/N
   ```

2. Run the Linux leg on that `main`. `--push` publishes only a tree that passed it
   (step 4), so this comes before it; [The Linux leg before a
   publish](#the-linux-leg-before-a-publish) says what it runs:

   ```bash
   bash scripts/gate.sh ci
   ```

3. Do a dry run. It writes one unreferenced commit object and nothing else: no
   branch, no tag, no remote call.

   ```bash
   bash scripts/publish-sprint.sh --sprint N
   ```

   It prints the commit id, the commit message, and the exact `git push` and
   `gh pr create` commands it would run, and whether the evidence of step 2 is
   present for `main`'s tree (`gate evidence: present` or `absent`, with what is
   missing). `--help` prints every option (and exits 2, like any usage message).

4. Publish. Only the owner runs this:

   ```bash
   bash scripts/publish-sprint.sh --sprint N --push
   ```

   It pushes the commit to `refs/heads/sprint/N` (never forced) and opens the
   pull request against `main`, titled with the commit's subject and carrying
   its body — read from the commit itself, so the pull request says exactly
   what the term gate scanned. A re-run after a
   push that already landed goes straight to the pull request.

   It exits 2, naming `bash scripts/gate.sh ci` and pushing nothing, unless
   passing tier `ci` evidence exists for local `main`'s tree. Evidence for any
   other tree counts as absent: a commit after the run, or the run on another
   branch, means running step 2 again. `scripts/publish-public.sh --tag-only`
   (a release tag) is not held to it.

5. Wait for CI, then merge the pull request on GitHub with **Rebase and merge**.

6. Restore the local ancestry (the merge is trivial, as the trees match):

   ```bash
   git fetch origin && git merge origin/main
   ```

The preconditions the script checks before it writes anything, the term gate it
runs on the message and the tree, and its exit codes are described once, in
[CI integration](ci-integration.md#publishing-a-sprint-one-pull-request). The
plan files it reads for the commit message are not part of a public checkout;
without them the commit gets a one-line summary and the output says so.

## The Linux leg before a publish

A test can pass on a fast machine with a warm cargo cache and fail on CI: a
timing band that CI's slower host overruns, or a call that needs every platform's
crates offline when CI's cache holds only its own. Run the tests the way CI does,
once per publish, on the `main` you are about to publish (after step 1 above, so
the evidence is for the tree that is pushed), before step 4:

```bash
bash scripts/gate.sh ci
```

It starts a native arm64 Linux container and runs, from the tree it certifies (the
tracked and untracked files, minus the ignored ones), the steps of the CI test job
that a developer machine can hide a failure in: every `scripts/tests/test-*.sh`
suite, the web UI build (`npm ci && npm run build`), and
`cargo test --workspace --no-fail-fast` with `LOGOS_PERF_TOLERANCE=3`, CI's value.
The legs run as an unprivileged user and every one of them runs even after another
fails, so one run names every red leg. The script that does the work in the
container is `scripts/ci-container.sh`; the counting is `scripts/gate.sh`'s, from
the logs the container leaves.

What makes it a different run from the full tier:

- **The cargo home is empty at every start.** The container is removed with its
  cargo home (`docker run --rm`, nothing mounted over it), so every dependency is
  downloaded for the platform being built, as on CI. Expect the download on every
  run.
- **Only the build directory persists**, in the named Docker volume
  `logos-ci-target` (`GATE_CI_VOLUME` renames it), so the compile is not repeated.
  It grows to tens of GB. Current versions of the sprint-coordinator's
  `disk-check.sh` report its size and print the command that reclaims it, which is:

  ```bash
  docker volume rm logos-ci-target
  ```

- **The image is pinned by digest**, in `CI_IMAGE` at the top of `scripts/gate.sh`,
  and its Rust must be the local toolchain's: every run compares the two and exits
  2 on a mismatch. When the local toolchain moves, resolve the digest of the new
  version's `rust:<version>-bookworm` image and replace the line.
- **Node is the exact release pinned in `scripts/ci-container.sh`**, with its
  checksum; CI installs the latest 22.x.

The run reports, for the workspace tests, the binaries expected, started and
finished and the tests passed and failed, and writes tier `ci` evidence for the
tree (`ci-ui-build`, `ci-script-suites` and `ci-test` under the worktree's
`gate-evidence` directory). Check it:

```bash
bash scripts/verify-evidence.sh --strict --tier ci
```

That accepts tier `ci` evidence for the tree as it is now and refuses the fast and
full tiers' evidence in its place, as they refuse it in theirs. `publish-sprint.sh
--push` asks the same question of local `main`'s tree
(`--tier ci --tree <that tree>`), so the answer does not depend on which branch is
checked out where you publish; run `gate.sh ci` where `main`'s tree is the one
being certified.

It exits 2, and never reports a pass it did not run, when there is nothing to run
it on, and says which: Docker is not installed, the Docker daemon is not running,
the engine is not a native arm64 one (the leg does not emulate), the image cannot
be pulled or run, or the image's Rust is not the local toolchain's. Any Docker runtime with a native arm64 engine will
do; start it before the run. The compile is bounded to two jobs
(`GATE_CI_JOBS`) and the tests run on four threads (`GATE_CI_TEST_THREADS`), CI's
core count; give the Docker VM enough memory for the former.

It does not replace the full tier, and CI still runs on x86_64, which this leg does
not: it catches what a cold cargo home and Linux show, not what only that
architecture does.

## Contributor pull requests during a sprint

After the owner merges a contributor pull request on GitHub, it enters the work
in progress at the next iteration boundary. Two merges on two branches, in this
order:

```bash
git checkout main && git fetch origin && git merge origin/main
git checkout sprint/N && git merge main
```

Then run the integration check on `sprint/N`. The sprint coordinator runs these
merges itself at sprint start, at every iteration boundary and before the sprint
merges into `main`; run them by hand only outside a coordinated sprint.

## Status labels

Planned work is mirrored one way into GitHub Issues: each change request is an
issue titled `[CR-NNN] <title>` and each of its stories a sub-issue titled
`[S-NNN] <title>`. The plan is the source of truth. A change made on GitHub to
one of these issues is not carried back, and the next update restores the
plan's state.

| Plan state | Issue |
|---|---|
| change request with at least one open story | open, label `change-request` |
| change request whose stories are all closed | closed (completed) |
| story not started | open, `status:backlog`, no milestone |
| story planned into sprint N | open, `status:planned`, milestone `Sprint N` |
| story with work in progress | open, `status:in-progress`, milestone `Sprint N` |
| story delivered | closed (completed), with a comment linking the sprint pull request |
| story deferred | open, `status:deferred`, no milestone |
| story superseded | closed (not planned), with a comment naming the successor |

Labels and milestones are created when they are first needed. A story that
concerns only a private component, or whose text would fail the public-terms
gate, is not posted at all.

An issue filed by an outside contributor keeps its author's text and labels when
it is accepted. It is retitled `[CR-NNN] <title>`, receives the `change-request`
label and one comment, "Accepted as CR-NNN.", and is the change request's issue
from then on.

## Owner checklist: GitHub settings

Apply these once in the repository settings on GitHub. Nothing in the repository
can set them.

**Settings → Branches → branch protection rule for `main`**

- [ ] Require a pull request before merging.
- [ ] Require status checks to pass before merging, with these checks required:
  `clippy · fmt · test · deny`, every `cross-link (…)` job and every
  `golden-signal (…)` job (one per matrix leg; the legs are listed in
  `.github/workflows/ci.yml`, so re-check this list when a leg is added).
- [ ] Require linear history.
- [ ] Block force pushes and deletion of `main`.

**Settings → General → Pull Requests**

- [ ] Allow rebase merging: **on**.
- [ ] Allow merge commits: **off**.
- [ ] Allow squash merging: **off**.

Rebase and merge is then the only merge method, which is what linear history
and the one-commit sprint pull request rely on. Optionally turn on
"Automatically delete head branches".

**Check afterwards**

- [ ] Open a throwaway pull request and confirm that CI runs on it and that only
  Rebase and merge is offered.
- [ ] Open the **New issue** page and confirm that the Bug report and Feature
  request forms are listed.

## CI actions are pinned

Every `uses:` in `.github/workflows/ci.yml` names a 40-hex commit, with the
release it was resolved from in a trailing `# vX.Y.Z` comment, and the workflow
holds a read-only token (`permissions: contents: read`). A moved or hijacked tag
cannot change what CI runs, and no job can write to the repository.
`scripts/tests/test-ci-pins.sh` fails the CI script-suites step on an unpinned
`uses:`, a missing version comment, a `dtolnay/rust-toolchain` step without
`toolchain: stable`, a `setup-node` older than v5 (the first major off Node 20) and
a missing or widened top-level `permissions:` block. `.github/dependabot.yml`
opens weekly pull requests for the `github-actions` ecosystem only; a bump rewrites
the commit and its comment together; the suite confirms both are still present
and well formed, but not that the comment names the release the commit belongs to,
which is what the `git ls-remote` check below is for.

To add or move a pin, resolve the tag to its commit and write both:

```bash
git ls-remote --tags https://github.com/actions/checkout 'refs/tags/v4.4.0*'
```

An annotated tag lists a second line ending in `^{}`: that line is the commit.
`dtolnay/rust-toolchain` publishes no release tag but `v1`, so it is pinned to that
tag's commit with `# v1`, and takes its toolchain from the `toolchain: stable`
input.

**The release workflow is pinned by hand.** `.github/workflows/release.yml` started
as cargo-dist output and is customised (the `build-spa` job, the tag guard, the
release-text term gate, the `workflow_dispatch` release of an existing tag), so
`dist-workspace.toml` carries `allow-dirty = ["ci"]`. Never run `dist init` or
`dist generate` to refresh it. Its actions are pinned to commits with a trailing
`# vX.Y.Z` comment exactly as `ci.yml`'s are, and `scripts/tests/test-ci-pins.sh`
checks them in the same way (pins, comments, `setup-node` v5 or later, one `uses:`
spelling the check reads). The one rule that differs is the token: the workflow
publishes the release, so its top-level `permissions:` block grants `contents:
write` and nothing wider than `read` beside it, where `ci.yml` grants `read` only.
Checked against the pinned cargo-dist 0.32.0:

- cargo-dist can pin action commits itself, with `[dist.github-action-commits]` in
  `dist-workspace.toml`, but only when it regenerates the workflow, and `dist
  generate --mode ci` refuses to run while `allow-dirty` names `ci`. With that flag
  dropped, a regenerate in a scratch copy replaced the whole file: `build-spa`, the
  tag guard, `release-dist-config.sh`, the term gate and `workflow_dispatch` were all
  gone, and the actions it pinned carried no version comment. Nothing here sets
  `github-action-commits`.
- The same commits are written into the file by hand, so a bump is a hand edit made
  as above (resolve the tag with `git ls-remote`, write the commit and the comment).
  Dependabot reads this file too and rewrites both together; check its pull request
  against the recipe above, since the suite cannot tell a comment that names the
  wrong release.
- dist's own step for installing itself pipes a downloaded installer into `sh`.
  `scripts/install-dist.sh` replaces it in `plan` and in `build-local-artifacts`: it
  downloads the installer to a file and runs it only if its sha256 is the one pinned
  in the script (`scripts/tests/test-install-dist.sh`). When `cargo-dist-version`
  moves, download the new installer, read it, and change the version and the digest
  in the script together.
- dist generated a step that pipes rustup's installer into a shell for a matrix entry
  that runs in a container. `dist plan` gives none of the four targets a container, so
  the step never ran; it is now a refusal, so a container entry added later stops the
  run rather than building without Rust.

The change that would let dist own the file again is to move the customisations out of
it (cargo-dist's `github-build-setup`, or separate workflows the release workflow
calls) and then regenerate; it is a change to the release pipeline, not to CI, and it
is not made here.

`.github/dependabot.yml` also sets a seven-day `cooldown`, so a release is proposed a
week after it is published and a hijacked one that is pulled quickly is never offered.
