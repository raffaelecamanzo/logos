# Contributing to Logos

Thank you for helping. This page explains how to file an issue, how an accepted
issue becomes planned work, how to send a change, and how a change is reviewed.

## Filing an issue

Open an issue from the **Issues** tab and pick a form:

- **Bug report**: something does not work as documented. Give the exact
  commands, the output and exit code, the output of `logos --version`, and your
  platform. Remove secrets and private source code from anything you paste.
- **Feature request**: describe the problem first, then the behaviour you would
  like, and how we would know it works.

Search the existing issues before filing. A report that can be reproduced from
its text alone is picked up soonest.

## How an accepted issue becomes planned work

A maintainer reads each new issue. What happens next:

1. **Accepted.** The maintainer brings the issue into the project's plan as a
   change request. The issue you filed is kept, so its history and discussion
   stay in one place. It is retitled `[CR-NNN] <title>`, gets the
   `change-request` label, and receives one comment, "Accepted as CR-NNN.". Your
   own text and labels are left as you wrote them.
2. **Split into stories.** The change request is broken into stories. Each
   story appears as a sub-issue titled `[S-NNN] <title>`, with a description and
   acceptance criteria.
3. **Tracked by label.** A story's state follows the plan:

   | Label or state | Meaning |
   |---|---|
   | `status:backlog` | accepted, not yet scheduled |
   | `status:planned`, milestone `Sprint N` | scheduled into sprint N |
   | `status:in-progress` | work on it has started |
   | `status:deferred` | accepted, parked for later |
   | closed (completed) | delivered, with a link to the pull request that carried it |
   | closed (not planned) | replaced by another story, named in a comment |

   The change request closes when all of its stories have closed.
4. **Taken as a hotfix.** A small, self-contained fix may skip the story split
   and be developed on its own branch, outside any sprint.
5. **Not accepted.** A maintainer closes the issue with a reason.

Please do not relabel, retitle or close these issues by hand. The planning side
is the source of truth: the issue state follows the plan, and a manual change is
put back at the next update.

## Sending a change

1. Fork the repository, or create a branch if you have write access, from the
   latest `main`. Name the branch for the change, for example
   `fix/short-description`.
2. Make one focused change. Add or update tests, and update the documentation
   under `docs/howto/` when behaviour changes.
3. Before pushing, run the core of what the CI runs:

   ```bash
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   bash scripts/check-public-terms.sh
   ```

   The CI also runs `cargo deny check`, the repository's script suites and the
   web UI tests. `bash scripts/gate.sh fast` is an optional, quicker subset
   scoped to your branch. Its architecture check runs `logos check`, which needs
   a `logos` binary on your `PATH` and the maintainers' local index and rules,
   which a clone does not carry. On a clone that check is recorded as skipped or
   failed and the script exits 1, so read its per-check lines rather than its
   exit status.
4. Open a pull request **against `main`**. Say what changed and why, and
   reference the issue (`Refs #N`); the maintainers close issues as the plan
   completes. Contributor pull requests always target `main`. Planned work
   arrives separately, as one pull request per sprint.

## Review

- CI must be green. The checks are `clippy · fmt · test · deny`,
  `cross-link` (one job per release target) and `golden-signal`.
- A maintainer reviews every pull request and may ask for changes. Push
  follow-up commits to the same branch.
- `main` keeps a linear history and pull requests are merged with **Rebase and
  merge**, so each commit on your branch lands as it is. Keep the commits tidy
  and each message meaningful before asking for the merge. Only a maintainer
  merges.

More detail for maintainers is in
[docs/howto/contributing.md](docs/howto/contributing.md).
