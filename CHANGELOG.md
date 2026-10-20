# Changelog

All notable changes to Logos are recorded here. This project adheres to
[Semantic Versioning](https://semver.org/).

The dogfood point releases between 0.2.0 and 0.7.6 advanced the self-dogfood pin
without a capability change and were recorded only in the commit history;
0.8.0 is the next notable, capability-bearing release.

Releases 1.1.0 through 1.4.13 shipped without changelog entries. The entries for
them below are one-line summaries reconstructed afterwards from the release commits
and sprint records. 1.4.2 and 1.4.4 were never released.

## [Unreleased]

## [1.20.4] — 2026-10-21

### Changed

- **The README opens with what Logos is for:** guidance on governance and
  compliance, security and test coverage, for agents and humans.

### Fixed

- **A Dependency-Check run refused for want of an NVD API key says so.**
  Dependency-Check 13 needs an NVD API key to update its data, and its first
  error line (`Error updating the NVD Data`) did not say that. When the log
  shows the NVD client rejecting the key, the scan's and `security status`'s
  `cause` is now one sentence: the key is missing or rejected, where to get a
  free one, and the user-level `config.toml` setting that supplies it
  (`[security_scan.dependency-check] extra_args = ["--nvdApiKey", "<key>"]`;
  a repository config may not set `extra_args`). A rejected key's value is never
  repeated, and any other NVD failure keeps its first error line.
- **The Security page's Show brings the Finding detail into view.** The detail
  sits below every other widget, so Show used to change nothing the reader could
  see. It now scrolls the detail into view (smoothly, unless the user prefers
  reduced motion) and moves focus to its heading, including when Show is
  pressed on the finding already shown; a reload does not scroll. Inside the
  detail the blast-radius lists, and the sentence shown when one is empty, are
  spaced apart.

## [1.20.3] — 2026-10-20

### Added

- **A `rules.toml [review]` table gates the branch, per symbol.** Under it,
  `gate` and `session-end` fail on a `new` or `worsened` offender against the
  merge-base with the default branch in a gating dimension (by default nesting,
  conciseness, cohesion, focus and uniqueness), named with its value, threshold
  and trust label under `gating_entries`. The aggregate becomes a drift budget:
  `drift_budget = "report"`, the default, reports the fall below the baseline
  and never fails on it; a number fails a fall past it. Equality, Modularity,
  Acyclicity and Depth never gate per symbol. `review` marks each gating entry,
  and the Stop hook then blocks only on one. A distributional or unknown
  dimension, or an unknown key, exits 2. Without the table nothing changes.
  The Health page's verdict judges the drift budget as `gate` does; per-symbol
  entries are judged by `gate` and `session-end` only.
- **Every change of the signal is attributed, and a re-bless records why.**
  `review` carries `attribution`: the signal's change from the merge-base split
  per dimension, with the offenders that drove each share. Under the `[review]`
  table `gate`, `session-end` and the Health page's verdict carry it too, from the
  baseline, and the Health page shows it as a Signal change card. `gate --save`
  stores the attribution of what it blesses and an optional `--reason`, reported
  as `baseline_reason` while that baseline stands; under the table a save past a
  numeric `drift_budget` without `--reason` exits 2 and leaves the baseline as it
  was. The store moves to schema 39.
- **A distributional dimension in decline raises a trend warning.** Under the
  `[review]` table, when Equality, Modularity, Acyclicity or Depth fell in each
  of the last `trend_window` recorded snapshots (default 2), `scan`, `gate`,
  `session-end` and `review` carry a warning naming the dimension, its values
  and the window. It never changes a verdict or an exit code. A `trend_window`
  under 1 exits 2.
- **`review --branches` projects shared budgets before a merge.** `logos
  review --base main --branches b1 b2 …` sums each branch's delta against the
  base (its tip minus its merge-base) and lists each `[constraints]` budget
  the combined merge would exceed — per module, function, file or
  project-wide count — and a numeric `[review] drift_budget`, with each
  branch's share. The drift is judged from this checkout's comparable gate
  baseline, as the gate after the merge would judge it, else from the base
  commit's signal; `drift.reference` says which. `max_clone_ratio` and `no_god_containers` are not
  projected, and the answer names them. A branch it cannot measure is named
  and left out of the sum. It is report only and exits 0 whenever it answers.
  The MCP twin is the `review` tool with `branches`.
- **A checked-in security baseline for CI: `logos security bless`.** It writes
  every unsuppressed stored finding to `.logos/security-baseline.json` as its
  fingerprint, rule id and project-relative path only — no message, snippet or
  secret — sorted, so an unchanged state blesses to the same bytes. When the
  file exists the `[security]` ratchet reads it instead of the set `gate --save`
  records in the gitignored `history.db`, so a fresh CI clone reads only new
  findings as new; `gate --save` and `session-start` then record no set beside
  it. Under a table whose required scan is missing it writes nothing and exits
  5. The plugin guard asks before an agent runs it or writes the file. A
  finding reported by an absolute path outside the index (OSV-Scanner on a
  lockfile) still carries the checkout's directory in its fingerprint, so the
  checked-in file also matches it by its rule id and project-relative path and
  holds in any clone; re-keying it waits for the osv-scanner 2.x identity
  change. A Gitleaks secret blessed under the
  key it had before the store's re-key stays known after the next scan re-keys it.

### Fixed

- **One scan or install at a time per project, on every surface.** Under
  `serve --mcp --ui`, an MCP `security_scan` ran beside the Security page's scan,
  and two Semgrep runs wrote one report at once. Every scan and scanner install
  now holds the project's lock, `.logos/security/run.lock`, from the CLI, MCP or
  the web UI, in any process. A second start is refused before it starts
  anything, naming what runs: the CLI exits 3 and the MCP tool answers an error.
  The Security page's Run scan still joins its own running scan, and an install
  and a scan of one project never run together. `logos security install` run
  where no scan can run, with no `.logos/` directory, takes no lock and writes
  nothing there. Its scanners and installers run in process groups of their own,
  out of reach of Ctrl-C, so a `logos security scan` or `install` stopped by
  Ctrl-C (SIGINT) or SIGTERM — as the plugin's Stop hook stops an overrunning
  scan — kills them before the lock goes with it, and exits 130 or 143. A Logos
  process killed by SIGKILL still leaves its scanner running beside the next
  scan.
- **A "where to start" unit keeps its id across a re-index.** `improvements`,
  the MCP `improvements` tool and the Health card's `scan.where_to_start` used
  to carry, as `node_id`, the storage row id a unit was named by. A re-index
  renumbers those rows, so the same unit came back under another number. Each
  unit's and outlier label's `node_id` is now a string. A class's or function's
  is its symbol id without the repository coordinate, the identity an
  `offenders` entry carries. A duplicate or near-clone group's is
  `duplicate_group:` or `clone_group:` and a digest of its sorted member
  identities, the same while its member set is. Ties in the ranking break by
  it. `node`, `impact`, `callers` and `callees` resolve such a coordinate-free
  id exactly. The store moves to schema 40; a snapshot recorded before it reads
  `node_id: null`.
- **The Security page's install starts nothing before you consent.** Install,
  Update and Install & scan used to run each offered scanner's version command
  before the consent dialog was answered. They now decide from what is
  installed alone. The dialog's lines are unchanged except in two cases where
  only a version could tell. A copy of your own on `PATH`, with no managed copy
  beside it, is kept whatever its version: `logos security install <scanner>`
  still installs the pinned version beside an older one. An Update beside such a
  copy may ask for the pinned version, and if your copy meets the minimum the
  install then keeps it and downloads nothing.
- **`session_start` no longer moves an existing baseline.** It used to re-save
  the gate baseline, and under a `[security]` table the security ratchet's,
  every time it ran, so an agent's session could bless its own regression or
  findings without the guardrail asking. It now decides each on its own, before
  the edits: it saves a missing gate baseline, records a missing ratchet
  baseline (none checked in and none in `history.db`), and never overwrites one
  that exists: only `gate --save` moves them, and `session_end` compares against
  the baseline last blessed. A baseline that no longer compares, after a change of
  metric semantics or thresholds, is reset before the edits with the usual
  `baseline reset` notice.
- **The re-baseline notice shows what it replaced.** `baseline reset: metric
  semantics changed` now names the replaced baseline's metric version and signal
  and the run's (`… (v7 8292 → v8 8295)`), says `metric thresholds changed` when
  only the thresholds moved, and `gate`, `session-end` and `session-start` carry
  it in `--json` as `baseline_reset`.
- **A scanner with nothing to scan is `not applicable`, and a failure names
  its cause.** gosec read `failed` with an empty log on a project with no Go
  code, and Dependency-Check read `failed` while the cause, a failed NVD data
  update, stayed in its log. gosec, on a project with no `.go` file outside the
  ignored paths, and Bandit, with no `.py` or `.pyw` file, are now reported
  `not_applicable` with a `reason`, and are not started, not even for a version
  check. `security status` reads them `not_applicable`, and neither `check`'s
  `[security]` missing-scan exit nor the workspace roll-up counts that as a gap.
  A stored report that no scan here could refresh — gosec's, once the last `.go`
  file is gone — reads `not_applicable` too; a fresh one, such as a report
  ingested from CI at this HEAD, keeps its state. A scan asks no consent for a
  scanner it will not start, so a `not_applicable` one is asked for once it has
  something to scan. A `failed` scanner's outcome carries `cause`, one masked
  line of its log, and `security status` carries it as `last_failure_cause`, on
  the CLI, MCP and the Security page.
- **Stopping `serve` stops its scans and installs.** `serve --ui` now handles
  Ctrl-C (SIGINT) and SIGTERM, and on any normal stop it cancels each running
  Security page scan or install, waiting up to 10 seconds in all for them to
  end. Before, `curl` went on downloading after `serve` stopped and its partial
  download stayed in staging, and a scanner ran on to its own end. A `serve`
  killed by SIGKILL or any other signal (SIGHUP when its terminal closes) still
  runs nothing, and the next install of any scanner now discards the staging it
  left. Before, only the next install of that same
  scanner did. Staging that a running Logos process holds is never touched.
  `serve --mcp` alone now stops on Ctrl-C and SIGTERM too, and however `serve`
  stops, it also kills the scanners of a scan an MCP client started.

- **One secret is one Gitleaks finding, whatever the scan mode or commit.**
  Gitleaks' history mode reports a secret once per commit that wrote it, each
  with its own fingerprint, at the place it had in that commit, so one secret
  read as several findings, and a secret first recorded by a `--path` scan read
  as new, and lost its suppression, once committed. A Gitleaks finding's
  fingerprint now folds its rule, its file and a keyed digest of the secret,
  computed in memory from the file or the commit and never stored; the key
  comes from the repository's first commit, so every full clone agrees. One
  secret is one finding listing its `commits`, and it keeps its fingerprint
  when its line moves, when it is committed and in a `--path` scan. The
  evidence store migrates (history.db 7): the next full Gitleaks scan re-keys
  the stored findings and the ratchet baseline; until then a `--path` scan
  keeps a stored finding at its old fingerprint. `.logos/security-ignore.toml` is
  never rewritten: an entry written under a per-commit fingerprint is listed
  `orphaned`, with `should_name` giving the fingerprint to put in its place.
- **A secret found only in history is anchored to its commit.** A full
  Gitleaks scan bound each secret to today's file and line, labelled `fresh`,
  even when the file was deleted or the line held other code since. A
  history-mode finding is now bound only where its line still holds the
  secret; otherwise its `binding` is `historical`, with no path, enclosing
  symbol or blast radius, and its `anchor` names the commit and the path it was
  found at. The CLI, MCP and the Security page show the label and the same
  seven-digit `short_commit`. A `historical` finding still counts in the
  `[security]` ratchet and caps. No author, email or commit message is stored.
  The evidence store migrates (history.db 8): stored history-mode findings read
  `historical` until the next full Gitleaks scan checks each line again.
- **Semgrep and OSV-Scanner stay out of ignored directories in any checkout,
  and every scan says what each scanner did with them.** Both relied on their
  own git-based ignore handling, so Semgrep read ignored directories in a copy
  that is not a git repository, and OSV-Scanner read them there and in a linked
  worktree, where it cannot read the git ignores. A whole-project scan now gives
  both every path outside the ignored directories as their targets instead of
  `.`, the one mechanism the hand-run harness showed keeps them out without
  dropping a near miss (`src/target/` beside an ignored `target/`). A symbolic
  link is never named, and Semgrep scans the files it is named whatever its own
  `.semgrepignore` and size limit say. Each
  scanner's whole-project outcome carries `ignored_paths`: `excluded` only where
  the harness proved the mechanism, which means Semgrep, OSV-Scanner and gosec
  (next entry), or `scanned` with a one-line `coverage_note` saying why. Bandit, Trivy,
  Checkov and Dependency-Check keep their exclusion flags but read `scanned`
  (`exclusion flag not proven by the harness`) until the harness runs them;
  Gitleaks and CodeQL read `scanned` too. It is in `logos security scan` and `status`, the
  MCP `security_scan` and `security_status`, and a new **Ignored paths** column
  of the Security page's scanner table. A re-scan of named paths still reads
  exactly those paths and states no coverage for them.
- **gosec stays out of ignored directories.** A whole-project scan gives gosec
  one `-exclude-dir '^target(/|$)'` per ignored directory: anchored at the
  project root, and covering every directory below it. The hand-run harness,
  re-run with a package in `target/nested/`, shows this skips both on the
  pinned gosec 2.21.4 in a git checkout, a linked worktree and a non-git copy,
  and keeps the near misses `src/target/` and `targets/`; `^target$` would
  still read `target/nested/`. gosec now reads `excluded`, or `scanned` with
  the count of directories it was not given (a name a flag could misread, or
  past the 64 KiB budget). CodeQL, which the harness has never run, gets no
  exclusion and no config, and still reads `scanned`.
- **A severity is never made up.** A finding whose report gave no rating (no
  `security-severity`, and only a default level) reads `unrated`, with
  `severity_reason: "the scanner gave none"`, where it used to read `medium`.
  A default level is one the report left out, or one the scanner writes on
  every result. osv-scanner writes `warning` on every result, so all of the
  pinned 1.9.2's findings read `unrated`. Gitleaks writes no level or score at
  all, so its findings read `unrated` too. From 2.0.0 osv-scanner carries the
  advisory's CVSS score, and a 2.x report reads that rating. The pin stays at
  1.9.2. `unrated` has its own count on the Security page and on the
  workspace Security page, where it also gets its own column. `--severity unrated`
  and the page's *unrated only* option list unrated findings alone. A rated
  least severity, the `[security]` ratchet and `max_severity` all compare an
  unrated finding as `medium`, so no gate verdict changes. Neither threshold
  accepts `"unrated"` (exit 2). The upgrade rewrites a stored osv-scanner 1.x
  or Gitleaks `medium` to `unrated` without a new scan (`history.db` migration 9).

## [1.20.2] — 2026-10-18

### Fixed

- **Bandit, Trivy, Checkov and Dependency-Check stay out of the directories the
  project ignores.** Every scanner excluded only `.logos/`, so Bandit reported
  findings in ignored notes and tool directories, and Trivy walked a Rust
  `target/` until its five-minute timeout and failed. That means any project
  with a build directory lost Trivy on its first scan. A scan now finds the
  top-most directories that the project's own `.gitignore`, `.ignore` and
  `.git/info/exclude` exclude. It passes them to Bandit (`--exclude`), Trivy
  (`--skip-dirs`), Checkov (`--skip-path`, as an anchored expression) and
  Dependency-Check (`--exclude`), after `.logos` and before your `extra_args`.
  Semgrep and OSV-Scanner already skip what git ignores, Gitleaks reads
  committed history, and gosec and CodeQL are unchanged. This covers `logos
  security scan`, the Security page and the MCP `security_scan`. A re-scan of
  named paths still reads exactly those paths with every scanner that takes
  paths. `docs/howto/security.md` lists the edges: a file ignored by name, and a
  file committed inside an ignored directory.

## [1.20.1] — 2026-10-18

### Fixed

- **The plugin guardrail now asks before `logos security ingest`.** Ingesting an
  empty SARIF report clears a failing `[security]` gate, required scanners
  included, and the `PreToolUse` guard let it through. It now asks, naming the
  action, for the shell command (in every spelling it already catches for
  `gate --save`: by path, behind `env` or an assignment, in `bash -c`, through
  `$(which logos)` or a variable) and for the MCP tool `security_ingest` under
  both the plugin's and a project `.mcp.json`'s prefix. `security findings`,
  `status` and `scan` still pass.
- **The secret masker masks a value that follows a few words after a credential
  key.** `password for db: Tr0ub4dorNeedle&3` was stored and shown as
  `password [masked] db: Tr0ub4dorNeedle&3`: the masker hid the first word
  after the key and kept the value. A `:`, `=`, `=>`, `:=` or `==` up to three
  words after a credential key, on the key's line and before a `.`, `,` or `;`
  ends a word, now has its value masked as well
  (`password [masked] db: [masked]`). The word right after the key stays
  masked, since that is where a scanner echoes a value
  (`password [masked] in app.py:[masked]`). This applies to every stored
  finding message (and so to the Security page, the branch review's security
  delta and the Stop hook message) and to a failed scanner's excerpt. Text with
  no credential key in reach is unchanged.
- **The Security page starts no scanner the project has not consented to.** Each
  load of the page, and of the workspace security roll-up, ran every installed
  scanner's version command, a cross-site page's request included. It now runs the
  version command only for a scanner the project's scan consent names, and only
  once per `logos serve` for each binary: a reinstalled copy is probed again. Any
  other installed scanner reads installed, with its version unknown and the reason
  `not_consented`. `check` over a `[security]` table, which the Dashboard runs on
  every load, now starts no scanner at all: it reads only each scanner's state.
  `logos security status`, the MCP `security_status` tool and `check` answer as
  before.
- **`logos security install` exits 1 when nothing it was asked to install got
  installed.** It exited 0 when every requested tool failed (for instance
  `install gosec --consent` with no network). It now exits 1 when none of the
  requested tools ended `installed`, `already_present` or kept as your own copy,
  and still exits 0 when at least one did; the per-tool outcomes are printed
  either way and the exit-2 refusals are unchanged. The CI guide now says that
  the `[security]` ratchet's baseline lives in the gitignored `.logos/history.db`,
  so a fresh CI clone reads every finding at or above `ratchet_min_severity` as
  new, and names the two ways to gate today.

## [1.20.0] — 2026-10-18

### Added

- **`/logos:sec-review`: the plugin's `logos-sec-review` skill.** It reads the
  security section of the branch delta and gives every introduced or touched
  finding one verdict: `fix`, `escalate-secret`, `escalate-dependency`,
  `false-positive` or `untrusted`. The gate sets all but `fix` and
  `false-positive` from the finding itself (redacted, a dependency scanner or
  advisory id, reachability confidence, freshness); the agent may only confirm a
  proposed `fix` or downgrade it to `false-positive`. Only `fix` code is edited,
  and a fix is claimed only when a re-scan by the same scanner over the changed
  files (`security scan --scanner --path`) no longer lists its fingerprint, lists
  no new finding at or above its severity, and the project's tests pass with a
  count. A secret is reported for rotation, never closed by deleting the value
  (it stays in git history). A dependency finding gets a proposal with its
  changelog risk and blast radius. A false positive gets an ignore-entry draft
  whose write is held by the plugin guardrail's `ask`. The report never says the
  code is secure: it names the scanners, their versions and the paths, and lists
  the OWASP categories no enabled scanner covers. The deterministic steps are
  scripts in the skill's `scripts/` (delta read, triage, targeted re-scan, proof,
  coverage, draft, report check), each printing a denominator; they source the
  `logos-review` skill's helpers in place.
- **The Stop hook scans the files a turn changed for secrets.** With recorded
  scan consent, the plugin's Stop hook adds a security leg after `check` and
  `review`: it runs Gitleaks, plus Semgrep when it is installed and consented, on
  only the files the turn changed (`security scan --scanner … --path …`), inside
  the same budget; a turn that changed only documentation or media runs this leg
  alone. A new secret, or a new finding at `high` or above, blocks the
  stop once; the reason names the rule, file, line and the stored redacted message,
  and points to `/logos:sec-review`. A finding the scan recognises as one it
  already recorded is not new, and suppressed findings are never listed. Without
  consent, without Gitleaks, or over budget, nothing is scanned, nothing blocks,
  and the hook says what it did not measure ("security not measured: no
  consent"). Consent is the scan's own check: without it, the scan refuses before
  any scanner process starts, its version probe included.
- **The manual documents the Security section.** `docs/howto/security.md` is
  the guide to it: installing the scanners with `logos security install` (your own
  copy wins), the tested version of each of the nine scanners beside the version
  Logos installs, scan and install consent, what consent lets a scanner do and
  that Logos itself makes no network connection, `security scan`, `ingest`,
  `status` and `findings`, the scoped re-scan with its `stale` reasons and
  Gitleaks's content mode, what reachability confidence means (including that
  `unknown` is never `unreachable`, and what `unreachable` cannot see), the
  `[security]` gate with its ratchet, caps and exit 5, the ignore file and the
  Security page's triage. Tests hold it to the binary: every command and flag it
  prints exists, its `[security]` example passes `check` on the fixture and is
  evaluated, and its nine tested versions are the registry's and the install
  manifest's. `README.md`, `usage.md`, `commands.md`, `configuration.md` and
  `error-handling.md` link to it, and two stale claims are corrected: that Logos
  never installs a scanner, and that security findings never gate.
- **The manual documents the security review loop.** `docs/howto/security.md`
  gains "Reviewing what a branch introduced": how to read `sections.security` of
  `logos review --base <ref>` (the classes `introduced`, `touched`, `resolved`,
  `stale` and `pre-existing`, the `fingerprint` and `hunk` methods, suppressed
  findings listed apart, `measured: false` for a project never scanned), the five
  verdicts of `/logos:sec-review` in a table that a test holds to the skill and,
  where the specification is present, to the requirement, what an agent may close (a `fix`,
  only by a targeted re-scan with the fingerprint absent, no new finding at or above
  its severity, and passing tests) and what it may not (a secret, which is reported
  for rotation and never closed; a dependency; an untrusted finding; a suppression),
  and that a clean review is not a claim that the code is secure. `plugin.md`
  documents the skill, the Stop hook's security leg and its consent requirement
  (the leg never asks for consent and never records it: the scan's own refusal
  turns the leg into "security not measured: no consent"), and the Stop check's
  three legs. `commands.md` documents `sections.security` and no longer says
  `sections` is `{}`. The plugin page now joins the span walk that holds every
  command and tool a page names to the shipped binary.

- **A security re-scan of one scanner and a few paths.** `logos security scan
  --scanner <name> --path <path>` (both repeatable; MCP `security_scan` takes
  `scanners` and `paths`; a scanner is named by registry name, `semgrep`, or by
  its report's tool name, `Semgrep OSS`, as the branch review gives it) runs
  Semgrep, Gitleaks, Bandit, gosec and Checkov on
  those files or directories only, so a fix is proved in seconds. Each path must
  resolve inside the project root: one outside it, through `..`, an absolute path
  or a symbolic link, is refused with exit 2 before any scanner starts, and each
  reaches the scanner as one argument in its command's fixed path position. The
  ingest replaces that scanner's findings only in the scanned paths; every other
  file's findings and freshness are unchanged. Gitleaks scans the files as they
  are (`--no-git`) while its full scan reads git history, so its history
  findings in the scanned paths stay, and a secret it finds again keeps its
  recorded fingerprint: only a newly added secret reads as new. Trivy, OSV-Scanner and
  Dependency-Check report `whole-manifest only` (CodeQL `whole-project only`) and
  run unscoped when named. A scanner whose stored report rests on `--path` scans
  alone reads `stale` (`scoped_only`) in `security status` and to the security
  gate, never fresh. `history.db` migrates to schema 6, which records the full
  scan behind a scoped snapshot and the paths re-scanned since.

- **`logos review --base <ref>`: the branch delta, per symbol.** It classifies
  every offender of the nine offender dimensions and every `check` violation as
  `new`, `worsened`, `improved`, `resolved` or `unchanged` against the merge-base of
  `HEAD` and the base (default: the repository's default branch). Each entry
  the branch changed is listed with its value before and after, its threshold
  and a trust label (unchanged entries are counted, not listed): `resolved`,
  `stale` for a file edited since the last sync, or the tag of a language the
  graph binds within files only. The answer also lists the near-clone groups the
  branch introduced. On a module graph too thin to measure on either side (the
  graph dimensions are out of the signal), Acyclicity and Depth offenders are not
  classified, and the answer says so. The before-state is the snapshot recorded at the merge-base,
  or else a shadow index of that tree, cached by commit under
  `.logos/review-cache/`. That directory is in the generated `.logos/.gitignore`.
  No other commit is ever compared. The MCP twin is `review`. The command is
  advisory: it exits 1 only when it could not measure, writes no snapshot, and
  never moves a `check` or `gate` exit code. Reruns are byte-identical.
- **`logos review --base` lists the security findings the branch caused.** Under
  `sections.security`, every finding of each scanner's latest stored report is
  `introduced`, `touched` (a merge-base finding inside a hunk the branch changed)
  or `pre-existing`, and every merge-base finding the latest re-scan no longer
  reports is `resolved`, or `stale` when its file was edited after that re-scan.
  A scanner whose report was recorded at the merge-base is compared by
  fingerprint; any other by the hunks the branch changed, uncommitted and
  untracked files included, and so is one whose merge-base report a `--path`
  re-scan rewrote (it holds the working tree of that run, not the commit). Each entry names its method, freshness, severity,
  OWASP categories and reachability confidence. Suppressed findings are listed
  apart with their reason and expiry, messages stay redacted, and the merge-base
  tree is never scanned. A project never scanned reads `measured: false`.
- **Hotspots and Files & Risk show security findings.** Once a full scan is stored,
  every `logos hotspots` row, every row of the MCP `hotspots` tool and the web
  Files & Risk table carries a findings cell: the count of the file's
  unsuppressed findings and the highest severity among them, each web cell linking
  to the Security page filtered to that file. A finding in a file that changed since
  the scan is labelled `stale` and is never counted; a suppressed finding is not
  counted. A project that was never scanned shows no column, not a column of zeros.
  The new `--with-findings` flag (MCP `with_findings`, web `?with_findings`) ranks
  the files with a fresh finding first, highest severity first, and drops none. The
  score and the default order are unchanged.
- **Offenders carry a symbol id, a value and a threshold, and list uncapped.**
  Every worst-offender entry now records `node_id` (the symbol id without its
  repository coordinate, the identity `check --bless` uses), the `measure` its
  `value` counts, the `value`, and the `threshold` it met, so a review can show
  that one symbol improved. `value ≥ threshold` holds wherever the dimension has
  a threshold. New `logos offenders [--path <glob>] [--dimension <d>] --json`,
  with the MCP twin `offenders`, lists every offender the last snapshot
  recorded, past the top-10 cap, in the order the capped lists use. It reads
  the snapshot and writes nothing. The capped lists, the signal, the gate and
  the metric semantics are unchanged. The store moves to schema 37; a
  snapshot written before it reads its new fields as not recorded (`null` under
  `values_recorded: false`), never as 0.
- **Every snapshot records where to start.** Each `scan`, `gate`,
  `session_start` and `session_end` snapshot now persists an advisory
  "where to start" list: up to ten fix units — a class, a function, an
  exact-duplicate group or a near-clone group — each with the signal points
  fixing it would recover. A unit's points are an estimate: the signal
  recomputed with that one unit no longer offending, minus the current signal.
  Units are ordered by points, then dimension, then node id, and a unit that
  recovers no whole point is not listed. Equality and Depth never carry points,
  because splitting a function or a module has no single result. Beside the
  list, seven per-unit measures (LCOM4, method count, span, complexity, nesting
  depth and the two group masses) label outliers against the member's own
  distribution: above `Q3 + 1.5 × IQR` and at or above the detection threshold,
  and never with fewer than 8 units. Both are read back unchanged as
  `where_to_start` in `scan --json` and the Health payload. They never enter the
  signal, the gate or any `check` exit code, and they leave the metric
  semantics unchanged. The store moves to schema 36; snapshots written before it read as not
  recorded.
- **Ratchet constraints: adopt a budget on an existing codebase and fail only on
  new debt.** Every budget whose violations name a symbol — `max_cc`,
  `max_fn_lines`, `no_god_files`, `max_fan_in`, `max_fan_out`,
  `max_nesting_depth`, `no_god_containers`, `max_brain_methods`, and `max_dead`
  and `max_duplicates` as symbol sets — accepts `{ max = N, ratchet = true }`
  (`no_god_containers = { ratchet = true }`). `logos check --bless`, with the MCP
  twin `bless_rules`, records every current offender in the checked-in
  `.logos/rules-baseline.json`, sorted and byte-identical on re-bless; `check`
  and `scan` only read it, and `gate` never touches it. `check` then reports a blessed offender at or
  below its blessed value as a `warning`, a new or worsened one as an `error`,
  and a blessed one that no longer violates under `ratchet.fixed`. With no
  blessed file every ratchet offender is an error and the report names the bless
  command. `max_cycles` and `max_clone_ratio` take `max_dead`'s count-delta form,
  `{ baseline = N, delta = 0 }`; `max_dead`'s own delta form is unchanged, and a
  contract with no ratchet budget reports exactly as before. The generated
  `.logos/.gitignore` leaves the blessed set tracked, and `ci-integration.md`
  shows the adoption recipe.
- **Security findings in graph context.** Every finding `security findings`
  (and MCP `security_findings`) lists now carries a `context`: the innermost
  code symbol enclosing its line, whether a framework route reaches that symbol
  over call edges — `reachable` naming the nearest route and its hop count,
  `unreachable`, or `unknown` — and its blast radius, the sizes of `impact` on
  the same symbol. Each claim is labelled `resolved`, `unresolved` or `unbound`,
  read from the per-language resolution denominator, whose `calls` row rides
  along: a language whose calls do not resolve across files is `unresolved`,
  and an unresolved claim is never `unreachable`. `unreachable` is claimed only
  when the graph could have seen every way in; otherwise the reachability names
  what it could not see — no route at all, a route with no proven handler, a
  member behind an interface or trait, or a callable handed off by value. A finding with no enclosing
  symbol is `unbound` and says why (no location, not an indexed file, changed
  since the scan, or in no symbol); a lockfile finding keeps its manifest line.
- **A Claude Code plugin, published from this repository's marketplace.**
  `.claude-plugin/marketplace.json` lists one plugin, `plugins/logos/`, version
  1.20.0, which needs a `logos` binary of 1.20.0 or newer (`metadata.minBinaryVersion`
  in its `plugin.json`). Install it with `claude plugin marketplace add
  <this repository>` and `claude plugin install logos@logos`. It declares the
  `logos` MCP server and the session-start quality readout, and does nothing in a
  directory without a `.logos/` store: there, every hook exits 0 with no output, and
  the server starts no `logos` process and offers no tool, so it creates no store.
  At session start the readout compares `logos --version` with that minimum. An older binary
  is named with both versions and the readout is skipped. A missing binary or a
  version it cannot read is said so. Neither blocks the session.
  `LOGOS_QUALITY_REPORT_DISABLE=1` silences the readout, as it does the hook
  `init -i` writes. The plugin's scripts are bash (plus `git`, and `python3` in
  the review skills). Each has a
  fixture suite (`scripts/tests/test-plugin-*.sh`), and one suite asserts that the
  tree names no extension that is not part of this repository. `gate.sh fast` now
  runs the script suites when `plugins/` or `.claude-plugin/` changed, as well as
  `scripts/`.
- **A managed tools directory for scanner binaries.** Logos can
  now install Trivy, Gitleaks, OSV-Scanner and gosec itself, into one user-level
  tools directory shared by every project and never inside a repository:
  `$LOGOS_DATA_HOME`, else `$XDG_DATA_HOME/logos/tools`, else
  `~/.local/share/logos/tools`. Each release compiles in a manifest that pins
  every tool's version, URL and SHA-256 for macOS arm64/x86_64 and Linux
  x86_64/arm64. Downloads go through the system `curl` with a fixed, HTTPS-only
  argv that ignores `.curlrc`, so Logos still links no HTTP client. Logos checks
  the SHA-256 itself before anything is unpacked: a tampered or truncated
  download is deleted, never run, and its tool fails with the checksum reason.
  Each tool is unpacked in a staging directory and moved into place with one
  rename, so an install that is interrupted or killed leaves no partial tool,
  and the next run completes it. Security detection now names which copy of a
  scanner it found (`override`, `path` or `managed`) and falls back to the
  managed copy of the pinned version. A missing `curl` reports `curl not found`
  and names the package; Windows reports `managed install not available on this
  platform`. `logos security install` runs it (below).
- **Security findings can gate a merge, and a missing scan exits 5.** A
  `[security]` table in `rules.toml` makes security findings gate input.
  `required_scanners` names the scanners that must have a fresh scan at HEAD.
  The ratchet fails on an unsuppressed finding at or above
  `ratchet_min_severity` (default `high`) that `gate --save` did not record.
  The caps `max_severity` and `fail_on_owasp` fail on any unsuppressed finding
  they match, recorded or not. `gate --save` and `session_start` record the
  fingerprint of every unsuppressed stored finding; the set lives in
  `history.db`, which moves to schema 5. A finding suppressed in
  `.logos/security-ignore.toml` is excluded from every rule. When a required
  scanner never ran, is stale, is not installed or is disabled, `check` and
  `gate` exit 5 naming it. Exit 4 takes precedence, and a scanner the table does
  not require never causes exit 5. `check --json` gains `missing_scans`;
  `gate --json` gains `security_violations` and `missing_scans`. Without the
  table nothing reads the evidence and the output is unchanged. It moves
  neither the signal nor the metric semantics.
- **The plugin asks before an agent relaxes the quality contract.** A
  PreToolUse hook (`plugins/logos/hooks/guard.sh`) returns `ask` when an agent
  would write, edit or delete `.logos/rules.toml`, `.logos/rules-baseline.json`
  or `.logos/security-ignore.toml` (any `.logos/`, so a workspace member's too),
  or run `logos gate --save` or `logos check --bless`, whether as a shell command
  or through the `bless_rules` MCP tool under either the plugin's or a project
  `.mcp.json`'s prefix. The reason names the file or the command, and the human
  decides. Reads pass: a Read of a guarded file, and a shell command that only
  reads one (`cat .logos/rules.toml`). A shell command that names a guarded file,
  as a word of its own or glued to an option (`dd of=…`, `--output=…`), asks
  unless its command is a known reader and the file is not a redirection target;
  a command that is not a known reader also asks when given the `.logos`
  directory itself, a glob under it that can match a guarded name, or a bare
  guarded name, glob or brace list beside a mention of `.logos`
  (`cd .logos && rm *`). Edits to other `.logos/` files and unrelated commands
  pass untouched.
  A payload the hook cannot read asks rather than allows. It is a guardrail
  against an agent loosening the contract by mistake, not a sandbox: a path
  assembled at run time is not seen.
- **`logos security install`, `uninstall` and `scan --install`, under a
  per-user consent.** `logos security install [<scanner>…|--all]` installs the
  scanners that have no usable copy. A user-level `path` override, or a copy on
  `PATH` at or above the minimum version Logos was tested with, is kept and
  never downloaded; a `PATH` copy below that minimum no longer wins over a
  managed copy. Installing needs the user's consent for each tool's pinned
  version, recorded once per user in `security-install-consent.toml` beside the
  scan consent and covering every project. Without it the command exits 2
  naming each tool, version, source host and download size, and downloads
  nothing; `--consent` records it. `--update` replaces a managed copy older than
  the release's pin, after consenting to the new version, and removes the old
  one; nothing updates by itself. `--from <dir>` installs offline from
  pre-downloaded artifacts with the same SHA-256 check. `logos security
  uninstall` removes managed copies only. `logos security scan --install`
  installs what is missing and not switched off, then scans. `security status`
  (and its MCP twin) now carries, per installed scanner, the copy in use, its
  version and `update_available`, and for a missing one the install command;
  reading the versions runs each installed scanner's version command. MCP has
  no install tool: an agent cannot consent to downloading a binary.
- **The web UI has a Security page.** A new **Security** entry in the
  sidebar, beside Rule findings and Coverage, shows each scanner's state, every
  stored finding with its enclosing symbol and route reachability, the findings
  grouped by OWASP Top 10 with a 2025/2021 switch, and one finding's detail with
  its blast radius. Each absent state names the command that fixes it: no scan
  yet, a scanner that is not installed or has never run, and an OWASP category
  no scanner that ran can detect, which reads "not covered" rather than zero.
  The findings table filters by severity, scanner, category, path and
  suppression, and a `?path=` link opens it narrowed to a file or directory.
  Switching edition re-classifies the same findings without a new request. No
  source snippet is shown, and a secret-class finding's message reads
  `[redacted]`. With a `[security]` table in `.logos/rules.toml`, the page
  also shows the verdict `logos check` reaches on it: the missing required
  scans and the findings its rules fail on. The page reads one new endpoint,
  `GET /api/v1/security`, which runs no scan and writes nothing.
- **A scan runs from the Security page.** A **Run scan** button starts
  `logos security scan` as a background job, so every other page keeps
  answering while it runs, and the panel shows each scanner as it starts and
  finishes. Clicking again while a scan runs joins it rather than starting a
  second one. **Cancel scan** stops every scanner process, including any a
  scanner started itself, and starts none after it; a cancelled scanner leaves
  no failure log, and the previous results are kept. Each scanner's captured
  output stays bounded. The first scan in a project opens a dialog that names
  the scanners that may reach the network; accepting records the consent in the
  user-level Logos config, never in the repository, and later scans do not
  ask. The new endpoints are `GET /api/v1/security/scan` (the job and its
  progress) and `POST /api/v1/security/scan` and
  `POST /api/v1/security/scan/cancel`, which accept a request only from the UI's
  own origin carrying the session's intent token.
- **Findings are triaged from the Security page.** Each finding row has a
  **Suppress…** action that opens a dialog. The dialog requires a reason, takes
  an optional expiry date, and names the file it writes:
  `.logos/security-ignore.toml`, which is committed with the repository. An empty
  reason is refused and nothing is sent. A suppressed finding stays listed,
  labelled with its reason and expiry. **Unsuppress** removes the entry. Both
  write through the ignore file's sorted serialisation, so adding an entry then
  removing it restores the file byte for byte, and removing the last entry
  removes the file. A file that is not in that canonical spelling, for example
  one with comments, is refused rather than rewritten, so nothing a person wrote
  by hand is lost. An expiry in the past or an already-suppressed finding is
  also refused with the reason. The new endpoints, `POST /api/v1/security/suppress`
  and `POST /api/v1/security/unsuppress`, accept a request only from the UI's own
  origin carrying the session's intent token.
- **A workspace rolls security up per member.** In a workspace, the sidebar's
  Workspace section gains **Security**: one row per member with how many of its
  scanners have a fresh scan, and its findings by severity, each row linking to
  that member's Security page. Totals count only fresh, unsuppressed findings,
  say so, and state the members they were summed over and the stale and
  suppressed findings they left out. A member never scanned reads "never run",
  never 0. A member that cannot be opened is a row with its reason, and the
  others still answer. The OWASP Top 10 table sums the members read, with a
  2025/2021 switch, and says in how many members a fresh scan covers each
  category; a category no member covers reads "not covered", and the findings
  under no category are counted apart, as on a member's own page. The page starts
  no scan. It reads one new endpoint, `GET /api/v1/workspace/security`, which
  runs no scan and writes nothing; a security read over a project that never
  stored a report no longer creates `.logos/history.db`.
- **The `logos-review` skill, the `logos-reviewer` subagent and `/logos:review`.**
  The plugin now reviews a branch symbol by symbol and gives every entry of the
  `logos review` delta one verdict. `untrusted` is an entry labelled `stale`,
  `same-file-only` or anything but `resolved`: reported, never edited.
  `test-first` is a trusted entry no test reaches: the skill names the
  characterisation test and stops. `no-action-warranted` always carries a reason.
  `fix` is a trusted, covered entry worth the change, and only `fix` symbols are
  edited. A fix is claimed only when a re-run of the delta shows its entry
  `resolved` or `improved` and the tests pass, with their count. An entry the
  branch added and then fixed has no merge-base state to resolve, so it is
  reported as `cleared`, never as `resolved`. The aggregate quality number is
  never cited. A clean branch reports "no entries" and edits nothing. Every
  deterministic step is a script that states its denominator: the merge-base, the
  delta run, the test-reach lookup, the trust gate, the edit-scope check, the
  re-measurement and a check of the report itself. On request, debt mode reads
  the "where to start" list instead of the branch. Test reach is by test-marked
  files, so a unit test inside the offender's own file is not seen and the entry
  reads `test-first`.
- **The plugin checks each editing turn when the agent stops.** A Stop hook
  (`plugins/logos/hooks/stop.sh`) acts only when tracked source changed since its
  previous run (documentation and media files do not count), and then runs
  `logos check` and `logos review` within a budget of 15 s (at most 25 s, inside
  the hook's 30-second timeout). A clean turn prints
  nothing. On a regression it blocks the stop once: the reason lists each new or
  worsened entry with its symbol, value, threshold and trust label, and each
  failing rule, and names `/logos:review`. An entry the graph could not fully
  resolve, `stale` among them, is listed but never alone blocks. The host does
  not block once by itself, so the hook reads `stop_hook_active` and never blocks
  again in the same chain. Over budget, or when a leg cannot run, that leg never
  blocks: the hook says it was not measured (a regression another leg did find
  still blocks, with the unmeasured leg named). The merge-base before-state is not
  built inside the budget: unless it was built here under the same logos version,
  contract and configuration (or, with no rule to evaluate, a snapshot is recorded
  at the merge-base), the hook starts the build detached, one at a time, says "not
  measured", and measures the next turn. A build that failed is not retried until
  the contract, the configuration or logos change. `LOGOS_STOP_HOOK_BUDGET_MS` sets the budget and
  `LOGOS_STOP_HOOK_DISABLE=1` turns the hook off; a directory without `.logos/`
  is left alone. The script runs its checks as a list of legs behind that one
  budget, so a further check is one function and one word in the list. Unlike the
  plugin's other scripts it needs `git` on `PATH` (it measures a working tree),
  and exits 0 silently without it.
- **A manual page for the plugin and the review loop.**
  `docs/howto/plugin.md` documents installing the plugin, migrating a project
  that ran `init -i` before it, `init --uninstall`, `offenders` and
  `review --base`, the Stop check with its 15 s budget and its off switch, the
  guardrail, the review skill's four verdicts and what proves a fix, and a table
  of the trust labels `review` puts on each entry. The page is held to the binary
  by tests: every `logos` command and flag it shows, inline or fenced, must exist in the shipped CLI,
  every `LOGOS_*` variable it names must be read by a shipped script or source file,
  and every variable a plugin script reads must be named in it.
  `docs/howto/configuration.md` lists the three plugin variables and the two
  directories (`.logos/review-cache/`, `.logos/stop-hook/`) the loop keeps.
- **Dependency-Check and CodeQL install only when named.** `logos security
  install dependency-check` installs Dependency-Check 13.0.0 with a pinned,
  SHA-256-verified Java runtime (Eclipse Temurin 21.0.12.1) that only it uses,
  so no system Java is needed; the managed copy always runs on that runtime,
  whatever `JAVA_HOME` says, and the consent names the runtime as a line of its
  own. `logos security install codeql --accept-codeql-licence` installs the
  CodeQL 2.27.2 bundle; without the flag it exits 2 showing the CodeQL licence
  terms (open-source codebases and academic research only) and downloads
  nothing. The acknowledgement is recorded per user and per version in
  `security-install-consent.toml`, with the install consent, and `security
  status` (and its MCP twin) shows it on CodeQL's row as `licence`; CodeQL's
  `install` hint names the flag until it is accepted. Neither is installed by
  `--all` or by `security scan --install`, and their status rows carry
  `opt_in: true`, so the Security page no longer suggests `scan --install` for
  them. `uninstall` removes the Java runtime once no installed scanner needs it,
  and `uninstall --all` covers both. A Dependency-Check on `PATH` below 13.0.0
  now counts as below the tested minimum: the managed copy runs in its place
  once installed.
- **Scanners install from the Security page, and Install & scan runs a full scan
  in one action.** While any of the seven default scanners is missing,
  **Install & scan** replaces **Run scan**: one background job installs the
  missing scanners, then runs the full scan, with no terminal. A *not installed*
  row offers **Install**; the new **Copy** column reads `managed`, `path` or
  `override`, and a managed copy older than the pin reads `update available` and
  offers **Update**, which replaces it. One consent dialog comes first: each tool
  with its version, source host and size, the support installs it needs, and
  for Install & scan the scanners still awaiting this project's network consent.
  Dependency-Check (with its Java runtime's size) and CodeQL (with its licence
  terms, the licence a checkbox of its own) are opt-in lines, unticked, and are
  never installed unless ticked. The panel lists each tool as it installs, then
  each scanner as it runs; **Cancel install** stops the download, leaves no
  partial tool, starts no further tool and names the tools installed before
  the cancel. A scan and an install, or two installs, never run side by side for
  one project: the second start is refused. The
  new endpoints are `GET /api/v1/security/install` (the job and its progress)
  and `POST /api/v1/security/install` and `POST /api/v1/security/install/cancel`,
  which accept a request only from the UI's own origin carrying the session's
  intent token; a start with a line still unanswered gets `409` with the plan
  and downloads nothing.
- **Hook and skill calls carry an invoker, and `stats --json` counts by it.** A
  process that declares itself in `LOGOS_INVOKER` stamps every telemetry event
  it records with that name. The plugin's hooks export `hook:stop`,
  `hook:pre-tool-use` and `hook:session-start`, and its skill scripts
  `skill:logos-review` and `skill:logos-sec-review` (a `logos-review` script run
  by `logos-sec-review` keeps the latter). Only `hook:<name>` or `skill:<name>`
  is taken; a direct call, or any other value, records none. `stats --json`
  adds `calls_by_tool_invoker`, per-tool counts by invoker with `null` for none,
  and the workspace statistics sum it across members. `attribution_coverage`
  states its limits: raw events only, like the origin cross-tab, and
  `null_invoker_includes_pre_stamp_rows` — events written before the stamp are
  never given an invoker. `telemetry.db` migrates forward to version 5 (a
  nullable `invoker` column); a reader of an older store counts every call as
  having none.

### Changed

- **The signal drops the graph dimensions it cannot measure, and Cohesion counts
  only languages that capture member access (metric semantics v8).** Modularity,
  Acyclicity and Depth are now not applicable when the production graph has
  fewer than 5 cross-module edges — edges between two different directories,
  both in an indexed file. Before, Modularity alone dropped out, and only below
  5 edges of any kind, so intra-module edges kept a graph with nothing between
  its modules looking measured. Each dimension keeps its computed values beside
  a false flag and leaves the mean and the zero short-circuit; `scan --json`
  and `quality-report --json` carry `acyclicity_not_applicable`,
  `depth_not_applicable` and `cross_module_edge_count` beside
  `modularity_not_applicable`, and the Health page names the reason on each row.
  Cohesion now scores a class only in a language whose plugin declares the new
  `member_access` capability — Java, TypeScript, TSX and PHP; a class of any
  other language, C++, Ruby and JavaScript included, is not applicable rather
  than scored from its calls alone. A plugin can carve extensions out of its
  declaration with `member_access_excluded_extensions`; the TypeScript grammars
  carve out `js`, `mjs`, `cjs` and `jsx`, whose constructor-assigned fields no
  query captures. `scan`, `gate`, `session-start`, the
  MCP `scan` and the session-start readout carry one coverage warning per
  language that is at least 10% of the production callables and binds no call
  or import across files, naming the language and its share; it never changes an
  exit code. The first gate after upgrading re-baselines once, announced. The
  store moves to schema 38; a snapshot written before it reads Acyclicity and
  Depth as applicable and keeps the Modularity reason it was decided on.
- **Semgrep, Bandit and Checkov install on a machine with no Python.**
  `logos security install` (and `scan --install`, and `--all`) now covers the
  three Python scanners: Semgrep 1.85.0, Bandit 1.7.10 (with its SARIF extra)
  and Checkov 3.2.533. Logos installs a pinned, SHA-256-verified `uv` 0.12.21,
  which provisions a managed CPython 3.12.14 and one environment per scanner.
  Each environment comes from requirements compiled into the release, every
  package pinned with its hashes and installed with `--require-hashes` from
  wheels only. A package whose hash does not match fails that scanner with the
  checksum reason and leaves no environment. Everything lives under the tools
  directory (`uv/`, `python/`, `uv-cache/` and `<scanner>/<version>/`). `uv`
  runs with a cleared environment, no config file and its home inside the
  install's staging directory, so nothing is written to your own Python, your
  `PATH` or your home directory; only the proxy and CA-bundle variables pass
  through. The three are detected as `managed` and run through their usual
  fixed command lines. The consent names `uv` and the Python runtime as lines
  of their own, beside each scanner's size from `pypi.org`. For an offline
  install, `--from <dir>` also reads `<dir>/uv-cache`: a copy of a connected
  machine's `<tools dir>/uv-cache` after it installed the three, the Python
  runtime included, with the `uv` archive beside it. That machine must have the
  same operating system and processor: the cache holds only its platform's
  packages. Its packages are trusted the way `uv` trusts its own cache, without
  re-hashing their files, so use a cache you produced yourself. Uninstalling the last of
  the three also removes `uv`, the Python runtime and the cache.
- **`init -i` adopts the Claude Code plugin, and `init --uninstall` exists.**
  When the plugin `logos@logos` is enabled for the project or the user (its
  `enabledPlugins` entry in `.claude/settings.local.json`,
  `.claude/settings.json` or the user's `settings.json` under
  `$CLAUDE_CONFIG_DIR`, else `~/.claude`; the most specific one decides),
  `init -i` enrols it in the committed `.claude/settings.json`
  (`extraKnownMarketplaces.logos` and `enabledPlugins`), so every teammate is
  prompted to install it. It also removes what an earlier `init -i` wrote and
  the plugin now carries: the `logos` `.mcp.json` entry, the session-start
  readout entry and its script, and a materialized skill copy with its
  `.claude/skills` pointer. With the plugin absent, `init -i` prints and writes
  exactly what it did before. `logos init --uninstall` (also spelled
  `init --hooks --uninstall`) removes everything Logos wrote: the `CLAUDE.md`
  block, the MCP, readout and plugin entries, the scripts and skill copies, and
  the managed git hooks, unsetting `core.hooksPath` only if Logos set it. It
  removes an entry only while it holds exactly what `init` writes, leaves
  foreign content and `.logos/` in place, and a second run changes nothing.
  `logos doctor` gains `plugin_warnings`: a `logos` server declared both by
  `.mcp.json` and by the enabled plugin (every tool would be listed twice), and
  a binary older than an installed plugin's `minBinaryVersion`. Both are
  advisory and do not change the exit code.
- **Health, MCP and the CLI answer "what should I fix first?".** Health gains a
  **Where to start** card above the per-dimension widgets: the ranked fix units
  of the displayed snapshot with Rank, Unit, Dimension, Est. points and Outlier
  columns, captioned as an advisory estimate that never gates. An empty recorded
  list reads "no single unit recovers a whole point", never "clean", and Health
  names `logos scan` when the snapshot recorded no ranking. Acyclicity's widget
  links to the Architecture view (its Cycles while that list is shown, the
  dependency matrix otherwise) whenever that view is not hidden; in this build
  the view is hidden, so no link appears and the cycles are listed inline. New
  `logos improvements --json`, with the MCP twin `improvements`, prints the same
  list `GET /api/v1/health` carries as `scan.where_to_start`; the three agree
  field for field on one snapshot, and a rerun is byte-identical. It reads the
  last snapshot and writes nothing. `docs/howto/metrics.md` explains what an
  estimate means. The chat agent's use of the list is not part of the public
  build.
- **The manual stops saying `init -i` installs git hooks, and describes plain
  `init` as it is.** `init -i` writes `.mcp.json`, `CLAUDE.md` and the readout hook,
  or the plugin's enrolment when the plugin is enabled, and installs no git hook:
  on a terminal it asks and the default is no, and `init --hooks` is the way to
  install them. Plain `init` writes `.logos/config.toml`, `.logos/rules.toml` and
  `.logos/.gitignore` beside the store, where `index` alone creates the store and
  no policy file. The MCP section of `usage.md` now counts the 33 tools the server
  registers (42 in a workspace) and lists `bless_rules`, `offenders`, `review`,
  `improvements` and the four `security_*` tools it had left out.

### Fixed

- **`/logos:review` sees a symbol's tests against the real binary.** Its test-reach
  step asked `logos impact` for each entry's symbol id, which carries no
  repository coordinate, and the binary resolves no such id, so every entry read
  test reach `unknown` and went to `test-first`: the skill could never propose a
  fix. It now asks the bare name the id ends in when the id does not resolve, and
  credits the answer only when it lands in the entry's own file.

## [1.19.0] — 2026-10-16

### Added

- **`logos security scan|ingest|status|findings`, with MCP twins.** The security
  evidence tier gets its commands and the matching `security_scan`,
  `security_ingest`, `security_status` and `security_findings` tools, each over
  one engine method, so both surfaces return the same payload. `scan` runs every
  installed, enabled scanner, writes its SARIF to `.logos/security/` and ingests
  it as it finishes; each registered scanner gets one outcome (`ran`,
  `not_installed`, `disabled`, `requires_build`, `not_applicable` or `failed`),
  and one failing never stops the others. A failed scanner's whole output goes to
  `.logos/security/<scanner>.failed.log`; the payload carries its exit code, the
  log path and the last lines with anything that looks like a secret masked.
  The first scan in a project needs `--consent`, because a scanner may reach the
  network to refresh its rules or database: without it the command exits 2
  naming the flag and the scanners, and nothing starts. Consent is the user's to
  give on the command line: MCP `security_scan` never records it, refuses a
  `consent` argument, and until it is given fails naming the CLI command. Consent
  is recorded per project in `security-consent.toml` in the user-level config
  directory, never in the repository; a scanner installed or enabled later is
  asked for alone. `status` gives each scanner exactly one state — not
  installed, disabled, never run, stale (and why) or fresh with its unsuppressed
  finding count — plus the consent state, so never run and fresh with 0
  findings are always distinct. `findings` lists the stored findings with their
  OWASP Top 10 categories and suppressions, filtered by severity, scanner,
  category, path and suppression (filters intersect), and states what it was
  drawn from so an empty list never reads as clean. A running watcher ingests a
  SARIF report written into `.logos/security/` and never starts a scanner.
  Documented in `docs/howto/commands.md` and `docs/howto/configuration.md`.
- **Security findings are stored as anchored evidence.** The evidence store
  (`.logos/history.db`) gains security findings beside coverage, through a
  forward-only migration, read and written through `logos security ingest`
  and its twins (above). A SARIF 2.1.0 report from any tool is parsed and
  validated in full before anything is written. A malformed, truncated or
  non-SARIF report changes nothing, and the error names the defect: the JSON
  position, or the SARIF path of the missing or wrong field, without quoting
  any value from the report. A run that did not
  complete (a failed invocation, or no `results` at all) is rejected too, so it
  never replaces real findings with an empty scan. A result of kind `pass` or
  `notApplicable` is not a finding. Each snapshot is
  anchored to HEAD and to the scanner's name and version. A rescan at the same
  HEAD replaces only that scanner's snapshot. Re-ingesting a byte-identical
  report at the same HEAD is a no-op reported `already_ingested`, unless a
  newer HEAD's snapshot has since become current, in which case the report is
  written again and becomes current. Each finding is
  keyed by a fingerprint: a hash of its rule, its path and the report's own
  fingerprint when it carries one, otherwise its location; no report value is
  stored. Report paths bind to indexed files
  through the binder coverage already uses (longest unique suffix). An ambiguous
  path is listed unmatched with its candidate count, never guessed. A result
  with no location is kept and listed as unbound. Freshness is decided per file
  by content hash at read time, so a file edited after ingest reads `stale`. No
  source snippet, code flow or rule text is ever read from the report. The
  message of a secret-class finding is replaced before the write by a fixed text
  naming only the rule and location. Secret-class covers Gitleaks, Trivy secret
  scanning, rules tagged or named as secrets, and hard-coded-credential CWEs.
  Every other message is stored with anything shaped like a secret masked.
  Committed SARIF fixtures cover all nine registered scanners.
- **Findings are classified by OWASP Top 10, 2025 and 2021.** A compiled-in,
  versioned table maps CWE identifiers to the categories of each edition, 2025
  being the default, and names the registered scanners that can detect each
  category. A finding is classified from the CWE tags of its rule, so a CWE-89
  finding is Injection in both editions. A finding with no CWE tag is
  `unclassified`, as is one whose CWEs the edition does not list, and the two
  are told apart. Classification is computed on read from the stored findings,
  so switching edition re-classifies them without a re-ingest. A category that
  no enabled scanner with a stored report can detect reports `not covered`,
  never a count of zero; a count appears only where such a scanner can see the
  category. `logos security findings` (above) shows the classification.
- **Logos can find and run the security scanners you have installed.** A
  built-in registry covers Semgrep, Trivy, Gitleaks, OSV-Scanner, Bandit, gosec,
  Checkov, CodeQL and OWASP Dependency-Check. Logos detects each scanner
  (installed with its version, not installed, or disabled) by running only its
  version command, and `logos security scan` (above) runs them to SARIF under
  `.logos/security/`. One scanner failing never
  stops the others. Every command line is fixed in Logos and started without a
  shell. `PATH` is reduced to its absolute entries outside the project, both to
  find a scanner and for the scanner's own process, and everything a scanner
  started is killed when it exits. A repository's
  `.logos/config.toml` may only switch scanners on or off
  (`[security_scan.<scanner>] enabled`); any other key there fails the load
  with exit 2 before anything runs. A scanner's binary path and extra flags are
  read only from a new **user-level config file** (`$LOGOS_CONFIG_HOME`, else
  `$XDG_CONFIG_HOME/logos`, else `~/.config/logos`, file `config.toml`). CodeQL
  runs only with `--build-mode=none` and only on JavaScript/TypeScript, Python
  and Ruby, merging every language's runs into one `codeql.sarif`; every other
  language is reported "requires a build — ingest a CI report" and nothing is
  started for it. `.logos/security/` joins the managed
  `.logos/.gitignore`.
  A repository's scanner config cannot make a scanner run code: Checkov is not
  started while `.checkov.yaml` is at the root, Trivy reads an empty config Logos
  writes, Dependency-Check's package-manager analyzers are off, and gosec runs
  with `GOTOOLCHAIN=local`.
- **Security findings can be suppressed from a committed file, with a reason and
  an expiry.** `.logos/security-ignore.toml` lists `[[ignore]]` entries keyed by a
  finding's fingerprint, each with a required `reason` and an optional `expires`
  date. `logos security findings` and `status` (above) label findings against
  it as of today. A file that does not parse, an entry with no `reason` or a blank one, a
  fingerprint that is not 64 lowercase hex characters, an `expires` that is not a
  calendar date, a repeated fingerprint and an unknown key are all rejected with
  exit 2 naming the line. An absent file means no suppression. An entry
  suppresses through its `expires` date inclusive; from the next day it no longer
  suppresses and the finding comes back labelled as having an expired entry. A
  suppressed finding is still listed, with its reason. An entry whose fingerprint
  matches no finding is reported as orphaned. The serialised file is sorted by
  fingerprint, so a file in that spelling reads and writes back byte for byte.

## [1.18.0] — 2026-10-15

### Added

- **A repository carries a stable project identity.** `.logos/config.toml` takes
  a `[project] id`, the coordinate that names the repository. Without the key it
  is derived from the normalised `origin` remote (`git@h:o/r.git`,
  `https://h/o/r`, `https://user@h/o/r.git` and `ssh://git@h/o/r` all give
  `h/o/r`). With no remote it is the main worktree's directory name, so a linked
  worktree, a clone at another path and CI resolve the same value. `logos init`
  writes the key when it is absent and never rewrites it. When
  `.logos/config.toml` is git-ignored, `init` says the id will not travel and
  names the default other checkouts derive. A malformed id (empty, a space, a
  disallowed character, or a bare `.`, which in a symbol marks an empty field)
  exits 2 naming `project.id`. The coordinate in force is recorded by `init` and
  every full `index`, and `logos status --json` reports it as `project_id`.

- **Workspace members have distinct identities.** `workspace status` (human,
  `--json` and `GET /api/v1/workspace/status`) lists each member's `coordinate`,
  the repository coordinate its symbols carry. Two members that resolve to one
  coordinate (two clones of one remote, or a copied checkout) are never folded:
  the later one in manifest order is reported `degraded` with the new
  `degraded_cause` `duplicate-coordinate`, whose reason names both paths and the
  `[project] id` override, and it is excluded from every fan-out, the bridge and
  coverage like a member that cannot be opened, so the command exits 1. The first
  member keeps answering. A distinct `[project] id` in the later member, then
  re-running the command, clears it. A workspace whose members all resolve
  distinct coordinates differs only by the added `coordinate` field. A fitness
  test unions the symbol sets of every member store of a workspace whose members
  share every repo-relative path, and fails when the coordinate is forced empty.

### Changed

- **Every node symbol names its repository.** The coordinate fills the SCIP
  package slot of every node symbol, so the same declaration in two repositories
  has two symbols: `logos . . . src/`lib.rs`/run().` becomes
  `logos . github.com/acme/svc-a . src/`lib.rs`/run().`. Broker topics carry it
  too, so two repositories' `orders` topics are two nodes. Policy nodes take the
  shape `logos policy <coordinate> . layer/domain/`, which replaces
  `logos policy rules . layer/domain/`. A full `index` builds every symbol under
  the coordinate in force. An incremental sync keeps the coordinate the store's
  symbols already carry, so one store never holds two; a sync that writes the
  first file into a store takes the project's coordinate, as an index would.
- **A duplicate symbol is refused in every build profile.** Before this change,
  a second node with a symbol that another node already held was folded into the
  existing node. Now the file that declares it fails to persist, and the reason
  is recorded in `persist_failures`, naming the symbol. The node that already
  holds the symbol is not touched, and every other file persists.
- **An existing store moves to the repository coordinate when it is opened.** A
  store built by an earlier release, or one whose `[project] id` or `origin`
  remote has changed since it was built, is migrated in place before anything
  syncs into it: every symbol in `symbols`, the reference ledger and the
  declared types takes the new coordinate, symbol rows that no node references
  any more are removed, and the store records the coordinate under both
  `symbol_coordinate` and `project_id`. Every file is marked for re-reading, so
  the next reconcile re-extracts it. Node and edge counts, snapshot history and
  the gate baseline are unchanged. The migration runs in one transaction: if it
  is interrupted, the store stays as it was and the next open runs it again. A
  store already at the coordinate is opened without a write. A linked worktree
  seeded from an unmigrated main store migrates its copy before its first sync.
  If the configuration does not load, or no `[project] id` is set and the derived
  value is only a fallback (git cannot answer for the repository, or the local
  checkout `origin` names is away), a store keeps the coordinate it records.
- **Symbols saved before the coordinate keep resolving.** A symbol argument whose
  package slot is empty (`logos . . . src/`lib.rs`/run().`) resolves under the
  coordinate the store's symbols carry, on `node`, `callers`, `callees`,
  `impact`, `explore`, `precedent`, `impact-intersection`, `implements`,
  `referencing-docs`, the web graph's `?seed=` and `xservice callers`/`impact`
  (each member under its own coordinate). The answer is the canonical
  symbol's, with a one-line note in `warnings`; the graph echoes the canonical
  seed. A `[[links]]` endpoint written as a full symbol without a coordinate
  stays valid. A canonical symbol answers as before. A symbol naming another
  repository's coordinate is not rewritten and resolves as unknown.
- **A `no_cross_service_callers` glob matches the descriptor tail.** The glob
  matches the provider symbol's path and scope, without the repository
  coordinate, and `member` names the repository. A glob written against the
  whole symbol before the coordinate (`logos . . . …`) matches the same bindings
  as before. A glob written as a path (`src/routes.ts/legacy*`), which matched
  nothing before because the symbol began with `logos`, now matches by its tail
  and can report violations. A glob that matches no provider in its scope is listed under
  `unmatched_rules` in the `workspace check` report and on stderr, and the
  report no longer reads as passing.

## [1.17.0] — 2026-10-15

### Added

- **`[workspace] exclude` keeps a directory out of a workspace for good.** A glob
  list in `logos.workspace.toml`, matched against the member name, removes
  matching directories from autodiscovery and from `init --workspace`'s
  candidates, so an excluded fork no longer returns on the next `init` and no
  longer makes real cross-service bindings ambiguous. `init --workspace --exclude
  <glob>` now writes its globs into the key. An explicit `members` entry that
  matches a glob is kept and warned about, and `workspace status` lists the
  directories it excluded. A malformed glob exits 2 naming `workspace.exclude`;
  a manifest without the key is unchanged byte for byte.
- **A cross-service answer matches the member and the symbol.** `xservice callers`
  and `xservice impact` (CLI, MCP and `/api/v1/workspace/{callers,impact}`) match a
  bridge edge on the queried `(member, symbol)` pair, never on the symbol string
  alone, so two members that declare the same handler no longer receive each
  other's cross-service callers or impact. `--repo` names which member's symbol is
  meant: it scopes the provider end (`callers`) or the near end (`impact`) of each
  edge, the per-member fan-out and the residue, while the other side stays in any
  member. Without `--repo`, a symbol two or more members hold is answered per
  member — `resolved_by_member` and an `ambiguity_warning` naming the members —
  and the web Impact panel shows both. A type reference whose owner is no member
  is listed under `type_reference_unattributed`, never dropped. A symbol one
  member holds answers byte for byte as before.
- **`[[links]]` entries are validated.** `workspace status` (human and `--json`)
  lists every declared link with what became of it: `valid` (one bridge edge with
  intake `asserted`), `invalid` with its reason (`unknown-member`,
  `member-degraded`, `no-match`, `ambiguous`, `same-member`), or `redundant`,
  naming the measured edge it repeats. An asserted edge never enters the bridge's
  exactly-one matcher, so adding a link never makes an evidence binding ambiguous.
- **A declared `[[links]]` entry is used in the answers, labelled `asserted`.** A
  valid link joins `xservice callers` and `xservice impact` (CLI, MCP and web)
  under its own `via_asserted_link` key, each row carrying `intake: "asserted"`
  and the `(asserted)` marker, matched on the queried `(member, symbol)` pair. In
  `workspace reachability` a link declaring an `http_call` or `grpc_call` keeps
  its target live under the distinct verdict `live-via-asserted-link`, never
  `live-via-cross-service`; a `publishes` link roots nothing. `workspace status`
  counts the links beside the coverage headline (`coverage.asserted_edges`), and
  a residue row a valid link covers keeps its reason and points at the link.
  The web service map draws asserted links dashed, with a legend entry. A link
  naming the handler of a route a measured edge already binds is `redundant`.
  `resolved_cross_service_edges`, egress resolution and `spec_conformance_ratio`
  do not change with or without `[[links]]`, and a workspace without them
  answers byte for byte as before.
- **Read-only tools answer for any workspace member.** Under a workspace the
  read-only MCP tools (`search`, `context`, `explore`, `node`, `callers`,
  `callees`, `impact`, `impact_intersection`, `precedent`, `branch_overlap`,
  `status`, `health`, `hotspots`, `dsm`, `evolution`, `coverage_status`) take an
  optional `repo`, and their CLI twins (and `query`) take `--repo <member>`.
  Without it the default member answers (`[workspace] default`, else the first
  member). Every answer under a workspace carries `member`; a single-root answer
  is unchanged. A name that is no member, or `--repo` with no workspace, is
  refused (exit 2) in the same words on both surfaces; a member whose store will
  not open is reported with its reason. The write and gate tools take no `repo`,
  refuse one, and say they act on the default member; an extension's MCP tool
  does the same unless it is registered with `McpTool::read_only`.

### Changed

- **`logos serve --mcp` in a workspace serves the workspace** (the build with the
  web UI): the read-only tools take `repo` and the `xservice_*` / `workspace_*`
  tools are listed. `--standalone` keeps the single-repository server.
- **A read-only command run at a workspace root answers for the default member**,
  with `member` first in its `--json`, where it used to refuse with `no Logos
  index found` (exit 3). Run inside a member's own directory or in a plain
  repository, a command answers exactly as before.
- **A `[[links]]` `relation` other than `http_call`, `grpc_call` or `publishes` is
  a parse error (exit 2)**; any value used to be read and ignored.

## [1.16.4] — 2026-10-14

The first binary release since 1.15.8: Homebrew, the shell installer and the
release archives deliver it again, and it carries every change listed under
1.16.0 to 1.16.3 below.

### Added

- **The contributor documents are checked in the gate and in CI.**
  `scripts/tests/test-contributing-docs.sh` checks `CONTRIBUTING.md`,
  `docs/howto/contributing.md` and the issue forms: every quoted command, path,
  flag, link and status label must exist, and the forms must be valid.

## [1.16.3] — 2026-10-14

### Added

- **Release tags publish this repository's binaries again (S-732).** Until the
  first release built outside this repository takes over the release names, a
  pushed release tag runs `.github/workflows/release.yml` once more: four archives
  (`aarch64`/`x86_64` × Apple Darwin / Linux musl, the default build with the real
  web UI), their checksums, the shell installer and the Homebrew formula in the
  `raffaelecamanzo/homebrew-tap` tap, under the names they had before 1.16.0. The
  workflow, `dist-workspace.toml`, the `logos` package's `[package.metadata.dist]`
  and the `[profile.dist]` profile are restored as they were before 1.16.0; the
  commit that removes them again lands in the tag that first outside release is
  built from. New in the workflow:
  - a manual dispatch releases an **existing** tag, named by its `tag` input, so a
    tag cut while the pipeline was gone (`v1.16.2`) is released without a new
    tag. Every job builds the tag's commit, never the dispatching branch, and
    `scripts/release-dist-config.sh` supplies the dist configuration a tag cut
    without it lacks, changing nothing on a tag that has it;
  - `scripts/release-tag-guard.sh` refuses, before anything is built and again
    before the GitHub Release is created, a tag that is not a version tag, is not
    the checkout, or already has a GitHub Release (drafts included); an unreadable
    release listing is a refusal too, and so is a tag moved during the run;
  - `scripts/check-release-text.sh` runs the term gate over the release title, the
    notes, the formula, the installer and the published `dist-manifest.json`
    before the Release is created or the formula pushed, and fails the release on
    a hit. Reviewed lines of generated text are admitted through
    `scripts/release-text.allow`; its one entry is a comment in the installer
    template `dist` ships.

  The suites are `scripts/tests/test-release-tag-guard.sh`,
  `scripts/tests/test-release-dist-config.sh`, `scripts/tests/test-release-workflow.sh`
  (the workflow's structure: step order, checkouts, refusal steps that cannot be
  bypassed) and the release-text cases of `scripts/tests/test-check-public-terms.sh`. `docs/howto/ci-integration.md`
  describes the release train; `docs/howto/installation.md` says which release
  the installers deliver.
- **An extension contributes typed fields to the Config forms (S-730).** The SPA
  extension registry (`web/ui/src/extensions/registry.ts`) takes config field
  groups per Config view (member, workspace), keyed to one of the extension's
  manifest config panels and limited to that panel's table (or a sub-table of it):
  kinds `int`, `float`, `bool`, `list`, `str` and `choice`, each with a label,
  help, placeholder and default hint, and an optional read-only group note. The member Config view
  renders the mounted extensions' groups after its core groups in the
  `config.toml` form, saved by its Save; the workspace Config view renders them in
  a group over the workspace root's `.logos/config.toml`, saved with the
  fingerprint its read returned (`409` on a stale one), and offered only while an
  extension contributes workspace fields. A field reads its value from the raw
  candidate and patches its one key back into it (`readValue` beside `patch` in
  `web/ui/src/views/config/toml.ts`), so no new write path or route exists. An
  empty field removes its key; a value the reader cannot type is shown read-only
  and left untouched; a `choice` value outside its options is shown as written
  and never rewritten. A build that composes no extension renders no extension
  field. `docs/howto/configuration.md` describes the fields.

## [1.16.2] — 2026-10-14

## [1.16.1] — 2026-10-13

## [1.16.0] — 2026-10-13

### Removed

- **Chat and the source wiki leave this repository (S-651).** The build this
  repository ships is the open-source build: it composes no extension, so it
  serves no chat or wiki surface on any channel, and nothing stands in their
  place. Gone with them:
  - the crates `logos-wiki`, `agent-core`, `chat-agent`, `wiki-agent` and
    `logos-chat`, and the `wiki` and `agents` features of the `logos` package —
    the default build is `lang-all` + `ui`, and no build of this repository links
    an outbound HTTP client;
  - the `logos wiki` command and its hidden `logos wiki hook --emit` alias
    (`logos hook --emit` is the one spelling), the five `wiki_*` MCP tools and the
    `/api/v1/wiki*` routes;
  - the SPA's Chat, Workspace Chat and Wiki views, the chat-key, chat-tier and
    wiki config panels, the Dashboard's Project Overview card, their tests, and
    the dependencies only they used (`@assistant-ui/react`, `react-markdown`,
    `remark-gfm`, `highlight.js`). The SPA's `virtual:logos-extensions` entry
    registers no extension.

  `GET /api/v1/extensions` answers `{"extensions":[]}`. `logos status` and
  `logos serve --ui` still start on a project whose `config.toml` has `[chat]` or
  `[wiki]`, reporting the ignored `[wiki]` table once, and an existing
  `.logos/wiki.db` is never opened. `logos init -i` asks about materializing
  skills only when a composed extension contributes an init step, so this build
  no longer asks.
- **This repository no longer builds or publishes binaries (S-652).** A version
  tag here is a source release: it marks the single commit
  `scripts/publish-public.sh` publishes, and no workflow reacts to it. Gone:
  `.github/workflows/release.yml`, `dist-workspace.toml`, the cargo-dist metadata
  of the `logos` package and the `dist` build profile. CI builds, tests and
  cross-links, and uploads, releases and publishes nothing. Release archives, the
  shell installer and the Homebrew formula are produced outside this repository,
  from a pin of the tag.

### Changed

- **The no-extension fitness suite runs on the default build (S-651).**
  `cli/tests/no_extension_surface.rs` holds the default build to the rule that
  nothing on it names chat or the wiki; the committed surface snapshots were
  regenerated to that build — the earlier default snapshots less exactly the five
  wiki routes, the `wiki` command, the alias note on `hook`, the five `wiki_*`
  tools, and the `[wiki]` values the build now ignores. The `agents` snapshots
  and the dependency test over the removed crates are gone. CI drops its separate
  no-extension step, `scripts/gate.sh` its `agents` and no-extension test legs
  (the full tier has one test leg), and `scripts/verify-evidence.sh` requires
  neither. `scripts/swe-build.sh` builds the default features, and
  `THIRD-PARTY-NOTICES.md` is regenerated over the smaller dependency set.
- **The code behind logos-core's `agents` marker is tested with the marker on
  (S-652).** No crate of the workspace enables the marker, so
  `observability::Surface::Chat`, its wire value and the test that pins it were
  compiled out of every leg. `scripts/agents-marker.sh` runs
  `cargo test -p logos-core --lib --features agents observability::` and refuses a
  run in which the gated test never printed a result line. CI runs it as a step;
  the full tier of `scripts/gate.sh` runs it as the `agents-marker` leg with its
  own evidence unit (the gated tests expected against those that reported), which
  `scripts/verify-evidence.sh` requires at that tier only. The marker itself is
  unchanged.

## [1.15.9] — 2026-10-12

## [1.15.8] — 2026-10-12

### Added

- **A gate keeps a reviewed list of terms out of the repository's text (S-716).**
  `scripts/check-public-terms.sh` reads every text file `git ls-files` lists, plus
  the untracked files `git add -A` would take, and fails naming `file:line:term`
  when one holds a listed word or pair of adjacent words. Matching is whole-word
  and case-insensitive; punctuation (hyphens, underscores, dashes and curly
  quotes included) and camelCase humps separate words. The list is stored only
  as SHA-256 digests (`scripts/public-terms.sha256`), and
  `scripts/public-terms.allow` admits reviewed lines by path and line digest. A
  walk that reads no text file fails. `gate.sh` runs it at both tiers as the
  `public-terms` gate and prints the files scanned; `verify-evidence.sh` refuses
  fast- and full-tier evidence without it; CI runs it first on the checkout.
  About 7 s on this repository.

### Changed

- **The extension host gates extensions through one neutral feature gate
  (S-714).** `logos-ext` defines a single trait, `FeatureGate`, which reports the
  enabled features and is asked again on every evaluation: what a surface reads per
  request (the extensions list, the overview fields) follows a change at run time,
  while web routes, MCP tools and CLI subcommands keep the answer they were
  composed with at start-up. An extension names the feature it needs with
  `Extension::required_feature()`, and a binary supplies a gate with
  `ExtensionHost::with_gate`; with no gate, only extensions that name no feature
  are active. `GET /api/v1/extensions` answers `{"extensions": [...]}`: the active
  extensions and their UI manifests, and nothing else.

## [1.15.7] — 2026-10-11

### Added

- **Fitness tests for a build without extensions (CR-213, S-645).** The build
  that composes no extension (`--no-default-features --features lang-all,ui`) is
  now tested. Its route census, MCP `tools/list`, `logos --help`, `GET
  /api/v1/extensions` and SPA navigation name no chat or wiki entry. Each is the
  default build's committed surface less exactly the chat and wiki entries.
  `status` and `serve --ui` start on a project with `[chat]`, `[wiki]` and a chat
  key. The few chat-adjacent items that stay are each listed with the decision
  behind it. A dependency test reads `cargo metadata` and fails, naming the edge,
  if `logos-core`, `logos-ext`, `mcp`, `web` or the CLI library depends on
  `logos-wiki`, `logos-chat`, `chat-agent`, `wiki-agent` or `agent-core`. The
  no-network check now also covers the no-extension tree, and the `agents` tree
  less `agent-core`. `gate.sh full` (a new `test-noext` leg) and CI run the
  no-extension build. In that build, `logos --help` no longer names the `logos
  wiki hook --emit` alias, which does not exist there. The default and release
  help are unchanged.

### Changed

- **Chat is a registered extension (CR-213, S-643).** The chat turn and
  conversation-history routes, the workspace chat, the `[chat]` table, the
  `chat.api_key` secret, the Chat and Workspace Chat views and the chat config
  panels moved out of the `web` crate into a new `chat` extension crate,
  `logos-chat`, which the `logos` binary's `main` registers under `agents`. `web`
  no longer depends on `chat-agent` and has no `agents` arm that names chat; the
  extension depends on `web`, never the reverse. The release (`agents`) binary's
  surface is unchanged — route census, `POST` allow-list, config read-model, MCP
  tools, CLI help and SPA navigation all equal the committed snapshots. A build
  without `agents` serves no chat turn or conversation-history route and offers no
  Chat entry, view, `[chat]` fields or key panel; an existing `[chat]` config still
  loads there, read and validated as before. Three chat-adjacent core routes stay
  in every build because the default build's committed surface carries them: the
  chat-key writes (`POST /config/secret`, `POST /api/v1/workspace/config/secret`)
  and the Workspace Chat's read-roots read (`GET
  /api/v1/workspace/config/read-roots`). With `agents`, the extension validates `[chat]` and its key,
  and a malformed table is reported as its error, naming the table and the file
  and never echoing what the file holds (the line and column of the old parse
  error are not carried). An extension's web
  route can now be a `WebRoute::shell` — a `GET` answered with the SPA shell — so a
  view path that also takes a `POST` (`/chat`) still serves the app on a browser
  navigation. The Playwright leg of `gate.sh full` builds the `agents` binary,
  which is the surface its specs drive.
- **The source wiki is its own crate (CR-213, S-644).** `logos-core/src/wiki/` and
  the fourteen `Engine::wiki_*` methods moved into a new `logos-wiki` crate.
  It works over `&Engine` through a `WikiEngine` trait the engine implements, so
  a caller that imports the trait keeps writing `engine.wiki_read(slug)`. The
  core no longer holds a wiki engine and cannot depend on the crate. The `init
  -i` wiki-skill step is now a contributed init step that the CLI supplies. As a
  result its row in the `init` report comes after the session-start hook's row
  instead of before it; the rows themselves are unchanged. Nothing else changes
  for a user: the MCP `tools/list`, `logos wiki` and its subcommands, the web
  wiki routes, `wiki.db` and the telemetry tool names are byte-identical. This
  prepares the wiki to register as an extension.
- **The wiki is a registered extension (CR-213, S-644).** `logos-wiki` is now the
  `wiki` extension, compiled under a new default-on `wiki` cargo feature and
  registered by the `logos` binary's `main`; the in-app generator joins it under
  `agents` (`wiki-agent` hands the extension its configured run service). Its five
  MCP tools are served as dynamic routes with the same names, descriptions and input
  schemas; `logos wiki` is the extension's subcommand, listed in `logos --help` where
  it always was; the wiki's web read routes, the Wiki-tab generation trigger, the
  `init -i` skill step, the `[wiki]` table, the fourteen `wiki_*` telemetry names,
  the Dashboard's Project Overview card and the Wiki view all register through the
  host. `mcp`, `web` and the CLI library no longer depend on `logos-wiki`,
  `wiki-agent` or `agent-core`, and `web` has no `agents` feature left. The default
  and release binaries' surfaces are unchanged — every committed surface snapshot
  holds. A build without the `wiki` feature (the slim
  `--no-default-features --features lang-all` build, for one) has no `logos wiki`,
  no `wiki_*` tool, no wiki route, Wiki view or Project Overview card, and never
  opens, migrates or deletes an existing `.logos/wiki.db`; a `[wiki]` table there is
  ignored and reported, as an unclaimed extension table is. With the extension, a
  malformed `[wiki]` is reported as its error, naming the table and the file. The
  Dashboard's overview read-model now carries `overview_page` only while the wiki is
  composed, and an extension can add a field to it and a card to the Dashboard.

## [1.15.6] — 2026-10-10

### Added

- **An extension host composes optional capabilities at one composition root
  (CR-213, S-639).** A new crate, `logos-ext`, defines `Extension` — with every
  contribution point: config tables, secret keys, MCP tools, web routes and their
  `POST` allow-list entries, a CLI subcommand, init steps, a UI manifest and
  telemetry tools — plus `ExtensionHost`. The CLI is now a library,
  `logos_cli::run(ExtensionHost)`, behind a thin `main` that registers the
  extensions its build includes. Every surface takes the host: `serve --ui` mounts
  the active extensions' routes inside the same loopback, CSP, method and intent
  guards as the core's; `serve --mcp` serves their tools beside the core roster as
  dynamic routes; `logos --help` lists their subcommands; `logos init` runs their
  steps after the core's; `logos stats` classifies their telemetry tools. An
  extension can never shadow a core tool, route or command, and one that is
  unregistered, or names a feature the build does not enable, contributes nothing.
  Nothing changes for a user: chat and the wiki are not moved yet, and the default
  and `agents` builds expose exactly the surface they did — frozen as committed
  snapshots (web route census and `POST` allow-list, MCP `tools/list`, `logos
  --help` and `logos wiki --help`, the config read-model, the SPA navigation) taken
  before the change.
- **The server states its active extensions, and the SPA bundle is an injected asset
  provider (CR-213, S-641).** `GET /api/v1/extensions` answers which extensions the
  running server composes, each with the UI manifest it contributes (an empty list
  when the build registers none). It is `GET`-only, loopback-only and same-origin
  like every read route, reaches no engine, and is the one added row in the
  committed web route census. The router no longer names the embedded bundle: it
  serves a `SpaAssets` provider, and the default (`EmbeddedSpa`) serves the same
  embedded `web/ui/dist/` bytes, so a binary that composes an extension with its own
  SPA bundle can hand the router that bundle (`hosted_router_with_assets`,
  `serve_surfaces_with_assets`). Nothing changes for a user: the shell, the assets
  and the history fallback behave as before.

- **Extensions own their config tables and secret keys, and an unclaimed
  `[chat]` or `[wiki]` table still parses (CR-213, S-640).** Each top-level
  table of `config.toml` and each `secrets.toml` key goes to the core or to the
  extension that claims it. The extension validates its own tables on every load
  and before every Config editor save, and its secrets are masked in the config
  read-model the way `chat.api_key` is. `[chat]` and `[wiki]` (and the
  `chat.api_key` secret) are reserved: in a build with no extension that owns
  them they are accepted, ignored, reported once on stderr, and kept in the file
  when the editors save. Nothing changes for a user of the default or `agents`
  binary: the core still owns `[chat]` and `[wiki]`, and they resolve exactly as
  before. An unknown table or key still fails with the same error.

- **The SPA mounts navigation, views and config panels from registered
  extensions (CR-213, S-642).** The navigation list, the view registry, the
  sidebar's icon map and both Config views' panel lists are now registrable. Extension
  UI code enters through a build-time module, `virtual:logos-extensions`, which this
  build aliases to an entry that registers nothing; the SPA reads
  `GET /api/v1/extensions` once at boot and mounts an entry only when the server
  reports its extension active and lists it in that extension's manifest. A visit
  to the route of an extension that is not active lands on the Dashboard, like a
  hidden view. Nothing changes for a user: no extension registers UI yet, so the
  navigation, views and config panels render exactly as before — the committed SPA
  navigation snapshot is unchanged.

### Changed

- **The session-start quality-report hook is a core command, outside the wiki
  (S-638).** `logos hook --emit [--force]` installs the Claude Code hook — the
  same script and the same non-clobbering `.claude/settings.json` merge as
  before, byte for byte — and `logos init -i` installs it exactly as it did. Its
  code moved from the wiki module to the core, so it survives a build that leaves the
  wiki out. `logos wiki hook --emit` stays as a hidden alias with identical
  output and exit codes; it no longer appears in `logos wiki --help`, and
  `logos --help` now lists `hook`. The telemetry name of the install is unchanged,
  so recorded history stays attributed.

## [1.15.5] — 2026-10-10

### Changed

- **The chat's automatic code-refs lookup counts as app chrome, not as graph
  navigation (HF-1, S-622).** `GET /api/v1/code-refs` runs on every finalized chat
  answer and makes up to 24 traced `node` / `graph_elements` reads, although nobody
  asked a graph question. It was recorded on the `web` surface, so those reads counted
  as a developer's graph navigation in `logos stats` and the dogfood usage figures. It
  now names the `shell` surface, like the app header's `status` readout: the events are
  still recorded in `telemetry.db`, attributed to `shell`, and no usage figure counts
  them. The response and its cap are unchanged. The guard test that pins which
  handlers name the shell is now a closed set of exactly `status` and `code_refs`; a
  third handler naming it fails.

## [1.15.4] — 2026-10-10

### Added

- **A chat message can be edited and resent, and Regenerate replaces the answer in
  the store (CR-204 C6, S-624).** Your own messages carry a pencil (hidden while a
  turn runs) that opens the text in place: Resend or Enter asks the edited
  question, Cancel or Esc leaves it — Esc there never stops a running turn. There
  is no branching. Edit and Regenerate both first cut the conversation back to
  before the question in `.logos/chat.db`, then ask it, through the same send path
  as any turn, so it carries the conversation's preset. The cut deletes the
  question, every later message, and the scratchpad and working memory written
  after it, in one transaction. A reloaded conversation now shows one answer per
  regenerated turn; before, the store kept the replaced answer and a reload showed
  both. The cut is a new intent-guarded `POST …/chat/threads/{id}/truncate` with a
  `user_turn` form field (the 0-based index among the stored questions) on the
  member and the Workspace Chat alike; it is on the enumerated allow-list, a
  missing or non-numeric `user_turn` is `400`, and an unknown conversation `404`.
  `chat.db` migrates to version 3: each memory row records the point in its
  conversation it was written at, and existing rows are backfilled from their
  timestamps, at worst one message late, so a cut can only delete such a row
  early, never keep it.

- **An empty chat greets the reader with suggestion chips filled from the graph
  (CR-204 C1, S-621).** In place of the one-line hint, an empty conversation shows a
  greeting and three chips built from fixed templates. The member chat fills them
  from `GET /api/v1/files` — the top hotspot, the riskiest untested file (the
  `?untested` board) and the indexed file count. The Workspace Chat fills them from
  the roster the shell already holds — the member count and the selected member —
  and makes no request of its own, so opening it never starts a member's engine. It
  does not read `workspace/status`, which opens every member's store. Both are
  existing reads; no core query or projection was added. A "largest module" chip was
  left out because no read-model serves a module size. A fact that was not returned
  turns its chip generic, and a chip never names anything the read-models did not
  return. No model writes a chip and no provider request is made before a send.
  Before consent the chips are disabled; after consent a chip sends its text through
  the composer's own send path, so it carries whatever the conversation's turn
  carries, and the chips disappear.

- **A chat turn runs under a prompt-only preset chosen in the composer (CR-204 C7,
  S-625).** The composer's **+** button, a placeholder until now, opens a menu of
  four built-in presets: Default, Impact analysis, Onboarding and Troubleshooting.
  Each has a one-line description. A preset adds one clause to the planner's
  instructions and one to the Synthesizer's, on the member chat and the Workspace
  Chat alike. Tests pin that the tools, the subagent roster and the budget-tree
  bounds are Default's under every preset, on both rosters. Default adds nothing, so
  an unsteered turn's instructions are unchanged. The member and workspace turn
  routes take an optional `preset` form field. An absent field means Default, and
  any value other than the four is a `400` that starts no turn and calls no
  provider. The choice belongs to the conversation: each conversation keeps its own
  across a reload, per chat. Every send in it uses the preset, including Regenerate
  and Retry. A non-Default preset shows as a chip in the composer that clears back
  to Default. A first turn under any preset is still titled.

- **Chat code is syntax-highlighted, and the code references the graph confirms
  link to it (CR-204 C4/C5, S-622).** A fenced block in a chat answer or an
  Activity step result is highlighted when it is tagged with a language Logos
  indexes or a common config, shell or patch format; any other block renders
  plain, as before. Highlighting uses a bundled `highlight.js` core with an explicit
  language list. It emits CSS classes only, with no wasm and no inline style, so
  the self-only CSP is unchanged. The colours are seven new `--syntax-*` tokens for
  both themes, each at least 4.5:1 against the code-block background. Once an
  answer is final, its backticked file paths and symbol names are checked against
  the graph in one `GET /api/v1/code-refs` request of at most 24 spans. Only an
  indexed source file, or a name exactly one node carries, becomes a link, and the
  link opens the Graph view focused on it. Ambiguous names and missing paths stay
  plain code. In the Workspace Chat the check runs against the member selected in
  the shell. The Graph view now honours `?seed=` (and `?granularity=`) when it
  opens, so the Architecture view's cycle links open focused on the module too. The
  SPA bundle grows by 117,676 bytes, to 4.66 MiB of its 8 MiB budget.
  `highlight.js` is BSD-3-Clause.

- **A silent chat stream is reported, with Retry and Stop (CR-204 C9, S-626).**
  While a chat turn is open, Logos already sent an empty SSE comment every 15 s on
  the member and the Workspace Chat turn routes. That comment now reads
  `: keepalive`. It is still a comment, not an event, so no SSE consumer sees a
  new event. The Chat tab now counts any received byte, keepalive included, as
  proof the connection is alive. After 45 s with nothing received, the turn's
  "Working…" line is replaced by "No response from the server for 45 s. The
  connection may be lost." with **Retry** (re-runs the question in place) and
  **Stop**. A byte arriving later clears the notice. A slow subagent step never
  trips it, because the keepalive keeps arriving. Both intervals are fixed and
  documented in `docs/howto/usage.md`; neither is a setting. A server that exits
  closes the connection instead, and that is still reported at once as a dropped
  connection. Retrying the first turn of a new chat continues in the conversation
  the retry creates; the stalled attempt's question stays in the history rail as
  its own unanswered conversation.

- **A new conversation is titled by its first answer, and a conversation can be
  renamed (CR-204 C3, S-623).** On the turn that starts a conversation, the
  Synthesizer is asked to end its answer with one line, `TITLE: <title>`, of at
  most six words. The line never reaches the answer — not even for a moment when
  it arrives split across two streamed chunks — and the title replaces the rail's
  first-message title as the answer finishes. It rides the answer's own request,
  so a turn makes no extra provider call. A missing, empty or over-long title
  leaves the first-message title, which is now cleaned: whitespace collapsed, the
  first letter capitalised, and cut at a word boundary within 60 characters. A
  later turn never retitles. The stream carries the title as a new
  `thread_title` event (`thread_id`, `title`); the seven existing events are
  unchanged. Rename a conversation from the pencil on its row, or by
  double-clicking its title: Enter saves, Esc cancels, and an empty name is
  refused. Rename is an intent-guarded `POST …/chat/threads/{id}/rename` with a
  `title` form field, on the member and the Workspace Chat alike; a blank,
  missing or over-long title is `400` and an unknown conversation `404`. Renaming
  does not move a conversation in the rail.

### Changed

- **Chat prose reads on its own 16px scale, and the red is kept for small signals
  (CR-204, S-618).** Answer and user-turn text in the Chat and the Workspace Chat
  now render at 16px on a 28px line, and answer h2/h3/h4 take weight 600 with
  −0.025em tracking (1.875, 1.5 and 1.25rem). The app-wide 15px base is unchanged.
  Six tokens land in `tokens.css` for the rest of the chat refresh to use:
  `--radius-3`, `--elevation-composer` (themed), `--measure-prose`, `--text-chat`,
  `--leading-chat` and `--tracking-heading`. "Start chatting", Send and the
  conversation-delete confirm no longer fill with the signal red; Send carries a
  small red dot while there is something to send. No inline style or script is
  added, and the served CSP is byte-identical.
- **The chat fills the window, with one scroller, a pill composer, a cardless rail
  and a consent chip (CR-204, S-619).** At desktop widths (1024px and up) the
  Chat and the Workspace Chat now fill the height below the header. The page no
  longer scrolls: the transcript is the only scroller, the composer stays in view,
  and a streaming answer is still followed to the bottom. Answer prose is held to
  a left-aligned reading width (`--measure-prose`). The composer is now one rounded
  pill with a focus ring around the whole pill. The input starts at one line and
  grows to eight before it scrolls. A round Send button sits inside the pill. It is
  neutral while the input is empty, red once there is something to send, and
  becomes Stop while a turn runs. This replaces the red dot added above. A "+"
  button at the start of the pill is a disabled placeholder for now. The history
  rail is no longer a card. The open conversation and the hovered one are filled
  rows, and the open one has a small red marker on its left edge instead of a red
  outline. Each row's delete control is a trash icon, shown on hover, keyboard
  focus and the open row, with the same Delete / Cancel confirmation. A toggle at
  every width hides or shows the rail. On a wide window each chat remembers that
  choice in the browser; on a narrow one the rail still starts collapsed. After consent, the provider/model status band becomes a one-line chip
  (`CHAT openai · host · model · budget 24/8/3`) that expands to the full band.
  The consent disclosure before it is unchanged. Below 1024px the panes still
  stack, and the transcript keeps its own bounded height.

- **Chat turns get rounded bubbles, icon actions, a scroll-to-bottom button and four
  keyboard shortcuts (CR-204, S-620).** Your own turns are bubbles with a 24px
  radius (`--radius-3`) and a 10px × 20px inset; the assistant turn stays flat. Copy
  and Regenerate under an answer are now 24px icon buttons with a tooltip and an
  accessible name, filled with the hover tint on hover and on keyboard focus; Copy
  shows a check for 1.5 s once it has copied. When you scroll away from the bottom
  of a long conversation a round ↓ button floats above the composer; it returns to
  the bottom and the conversation follows the stream again, and at the bottom the
  button is not rendered. **Enter** sends, **Shift+Enter** inserts a new line,
  **Esc** stops a running turn through the same Stop the pill uses, and **⌘K**
  (**Ctrl+K** off macOS) starts a new chat. ⌘K and Esc ignore a key typed in a text
  field outside the conversation. A muted hint under the composer names Enter and
  Shift+Enter, and `docs/howto/usage.md` lists all four. The Workspace Chat behaves
  identically. No Rust, route or wire change; the served CSP is unchanged.

### Fixed

- **The first Chat or Files load on a fresh project no longer fails one of its
  requests.** Opening the Chat view on a project whose history or chat store did
  not exist yet makes several requests at once, and each one creates the store.
  Two of them could both apply the same schema step ("table schema_versions
  already exists") or collide switching the new file to WAL ("database is
  locked"), and that request answered `500`. A pending step is now applied under
  the write lock after re-reading the version, and a busy switch to WAL is
  retried for up to about a second. Found while testing Sprint 95: 5 of 12
  concurrent first reads failed before, 0 of 20 after.

- **A chat turn stopped before any answer no longer reads "Working…" (S-626).**
  Stop finalised the turn, but its progress line stayed up indefinitely.

## [1.15.3] — 2026-10-09

### Added

- **Health names the worst items of Acyclicity, Depth, Equality and Redundancy
  (CR-209, S-632).** `logos scan` records a top-10 list for four more dimensions,
  beside the five it already kept:
  - Acyclicity: each cross-directory cycle, largest first;
  - Depth: the longest directory chain, then the other chain heads;
  - Equality: the functions above the mean cyclomatic complexity, highest first;
  - Redundancy: dead or duplicate production functions, longest first.

  Each list has a fixed tie-break, so a re-scan of an unchanged tree records the
  same rows. A function is listed as dead only where its language's reachability
  analysis gives a verdict. The lists ride the existing `worst_offenders` payload
  of `logos scan --json`, MCP `scan` and `GET /api/v1/health`, with the same row
  shape and no schema migration. Health renders them with the same badge and
  table as the other five, so nine of the ten dimensions now say which code drove
  their score. Modularity alone stays unlisted. No dimension's score changes, and
  the five existing lists are byte-identical. A snapshot recorded before this
  release names the four lists it lacks in `worst_offenders.unrecorded`, and
  Health shows them as "not recorded" until the next `logos scan`.

### Changed

- **Grammar entries are declared once (CR-211, S-634).** In
  `logos-core/src/plugin/grammars.rs` each embedded query is one
  `query!("<language>", "<name>")` and each grammar one `entry!(…)`, which build
  the relative path, the label and the `include_str!` source from the language
  directory and the query name, instead of a hand-written five-line block per
  query file per language. Nothing observable changes: `compiled()` returns the
  same entries in the same order (pinned by a test written first, over labels,
  paths and the byte length and hash of every manifest and query), and
  `logos languages --json` is byte-identical.

### Fixed

- **The watcher and incremental sync honour nested `.gitignore` and `.ignore`
  files (CR-210, S-633).** `logos index` and `scan` always did; the `serve`
  watcher, git hooks and `logos sync <paths>` read only the root's ignore
  files. So a file ignored only by a nested `.gitignore` was indexed when it
  was written while `serve` ran. It then counted in rule findings and metrics
  until the next full reconcile, and came back on its next write. The
  incremental path now reads the same ignore files as the walk, with git's
  precedence: a deeper rule overrides a shallower one, and a `!negation`
  re-includes. Each directory's ignore files are read once and cached. An edit
  to one applies to the next watcher batch with no restart, and removes
  already-indexed files the new rule excludes in that same batch.
  `logos doctor` now reports a stored file that a nested ignore excludes as
  admission drift. The global gitignore (`core.excludesFile`) is still not
  read, on either path.

## [1.15.2] — 2026-10-09

### Changed

- **The Architecture view and Declared contracts are hidden; review polish
  (CR-208, S-631).** The sidebar no longer lists **Architecture**: `/architecture`
  and the retired `/dsm` bookmark land on Health, and `logos dsm`, MCP `dsm` and
  `GET /api/v1/architecture` answer unchanged. On Workspace → Service map the
  **Declared contracts** widget is hidden too, while the map keeps its declared
  layer and legend; its data stays on `GET /api/v1/workspace/status`,
  `logos workspace status`, `logos xservice route-providers` and their MCP twins.
  Both are entries in the hidden-widget register, so removing an entry brings the
  widget back — for the view, its sidebar entry and route together. The
  Dashboard's **Project Overview** shows its title and wiki snippet with no
  explanation, the one widget typed to have none. A glossed table header keeps
  its underlined term outside the sort button, so the header is one sort control
  and clicking the term no longer sorts (Members, and every other table). The
  service map's bindings table heads its count column **Calls**; *n of m bindings
  shown* is unchanged. The bindings filter's three inputs sit on one line, with
  one height, though only the first has a hint. Health's Acyclicity and Depth
  point at `logos dsm` for the module-to-module dependencies instead of the
  hidden matrix. The root `.gitignore` names the web UI's Playwright output
  directories, so `logos serve` stops indexing the e2e harness build while a gate
  runs (a stop-gap until the watcher reads nested `.gitignore` files, CR-210).

## [1.15.1] — 2026-10-08

### Changed

- **Widgets drop the "What you can do" line; an absent figure names the command
  that fills it (CR-206, S-629).** Every web widget now reads in three parts —
  title and figure, explanation (what it shows, why it matters), evidence — with
  no action line and no where chip; the catalogue entry is `{ what, why }`, and
  an entry that carries an action is a type error. Where a widget has no figure
  yet, its absence sentence names the command that fills it: `logos index` and
  `logos scan` on Health (and in its not-current notes), `logos scan` for
  offenders not recorded and for an empty Signal trend, `logos stats` when no
  telemetry is recorded, `logos coverage ingest <report>` when no coverage is
  ingested, `logos hotspots` when nothing is ranked, `.logos/rules.toml` and
  `logos check` when no architecture rules exist, and `[[governance.boundaries]]`
  in `logos.workspace.toml` when no workspace rules are declared. A failing Gate
  names its lowest-scoring dimension under the figure. **Resolved cross-service
  edges** below 100% shows its not-resolved reasons as an evidence table of
  reason and count, largest first, summing to the unresolved figure, without
  remedies. (The per-row action columns this entry left in **Members** and
  **Binding evidence** are removed too; see the next entry.)
- **Members and Binding evidence drop the "What you can do" column (CR-207,
  S-630).** On the Workspace Dashboard, **Members** now ends at *Unused across
  the workspace*: a member that could not be opened is still shown by its red
  *degraded* State badge and its reason, and no row reads *Nothing to do —
  informational.* any more. On Workspace → Service map, **Binding evidence**
  ends at *Calls*: a refusal is still stated in words in *Committed value* (no
  committed source defines it, a placeholder value, not committed by the
  repository, or a newer refusal as the server named it), and *Defining sources*
  still names the files. Nothing those columns said moves elsewhere. No
  "What you can do" text renders anywhere in the web UI, and the widget test
  helper, the widget source scan and the browser layout check now refuse it in
  tables too.

### Fixed

- **Builds clean on Rust 1.99.** The agent call budget charges through
  `AtomicUsize::try_update`, the 1.99 name for `fetch_update`, so
  `cargo clippy -- -D warnings` passes on the current stable toolchain again
  (CI had been failing on the deprecation since 1.8.3). Behaviour is unchanged.

## [1.15.0] — 2026-10-08

### Added

- **A shared widget frame and a browser layout check for the web UI (CR-203,
  S-611).** A new `Widget` component lays every widget out in one order — title
  row, figure row, explanation, action line ("What you can do" and, when there
  is something to do, where: source code, documentation, configuration or a
  command), evidence — and states an absence as a left-aligned sentence in the
  figure row rather than a centred empty state. `WidgetStack` spaces widgets one
  token apart, and `Term` glosses internal vocabulary (a `<dfn>` whose
  explanation shows on hover and keyboard focus) from one glossary: the
  fourteen terms FR-UI-39 names, plus the terms later catalogues add. A widget's words come from a typed catalogue
  entry `{ what, why, action(state) }`, so a catalogue missing a part does not
  type-check, and the test helper `expectWidgetCopy` asserts the four parts on
  a rendered widget. Every view renders through it (below).
- **Browser tests in the full gate (S-611).** `web/ui` gains Playwright
  (Chromium only): `npm run test:e2e` drives a `logos serve --ui` built from the
  tree over a checked-in single-repository fixture and a two-member workspace
  fixture, and asserts layout in computed style — equal gaps between stacked
  widgets, left-aligned widget text, one font size for explanation and action.
  The frame's own checks run on a harness page built from the same components
  and served from the same server; every view of both sidebars is checked on
  the real pages, and the app shell on both fixtures. `scripts/gate.sh full` runs it as a
  new `ui-e2e` leg and prints its pass count; `gate.sh fast` never runs it. A
  missing browser, an empty run, a skipped spec or a run past its time limit is
  a failed leg (a timed-out run is killed with all its processes), and
  `scripts/verify-evidence.sh` now requires the leg for a full-tier handoff.
  Install the browser once with `npx playwright install chromium` in `web/ui`.

### Changed

- **Four low-insight widgets are hidden from the web UI; their data is still
  served everywhere else (CR-203, S-612).** The per-arm coverage board (Workspace
  Dashboard and the Cross-service coverage panel), the Cross-service impact tab
  (the Workspace tab now has two tabs), the Health page's Non-gated tier callout,
  and the Architecture page's CYCLES band and cycle list no longer render. The
  dependency matrix keeps its `↺` back-edge cells, and the sidebar entry reads
  **Architecture**. `GET /api/v1/workspace/status`, `GET /api/v1/workspace/impact`
  and `GET /api/v1/architecture`, `logos workspace status`, `logos xservice
  impact`, `logos dsm`, `logos hotspots` and the MCP tools `workspace_status`,
  `xservice_impact` and `dsm` answer unchanged. Every hidden widget is listed in
  one register, `web/ui/src/views/hiddenWidgets.ts`, with its reason and the
  surfaces that still serve it; deleting an entry brings the widget back. The
  usage guide lists them under *Hidden widgets*.
- **Files & Risk abbreviates long paths, and Files & Risk and Statistics explain
  their figures (CR-203, S-616).** In "Files ranked by risk" and "Ownership
  dispersion", a path longer than 40 characters reads as its first segment, an
  ellipsis and its last two segments (`logos-core/…/resolve/binder.rs`). Two rows
  that would read the same keep more segments until they differ. Hovering shows
  the full path, keyboard focus shows it as a tip, a screen reader reads it, and
  the File column sorts by it. Both widgets say what they rank and why, and
  what to do: add tests to or split the top files; run `logos coverage ingest`
  when Coverage reads n/a; run `logos hotspots` when nothing is ranked yet; name
  an owner in `CODEOWNERS` when files have several authors. A single-author
  history needs nothing. The top hotspot is now this widget's figure, not a
  separate HOTSPOT callout. The Co-change and Defect headers explain themselves
  on hover and focus. Every Statistics widget states that it is informational.
  With no telemetry yet, the page says so in the Estimated value widget and
  names `logos stats`. Tool attribution by class is now one table with a Class
  column, ordered by class and then by calls, with "Answered" explained. The
  read-model's own coverage notes stay verbatim, in the widget's explanation.
  No HTTP, CLI or MCP answer changes.
- **Health explains its gate and its signal, and every quality dimension has its
  own widget (CR-203, S-615).** The page now renders through the shared widget
  frame. The Gate reads "PASS/FAIL · signal *s* vs baseline *b*; passes at ≥
  *b − ε*", with ε taken from the gate result; on FAIL it names the
  lowest-scoring dimension to start with and `logos gate --save` for an
  intended drop, and on PASS it says there is nothing to do. A pass the gate
  reached without comparing — no baseline, or one recorded under other
  thresholds or metric semantics — says so instead of showing a pass floor it
  never applied. The stale and absent states keep their classification and
  commands; the command now sits on the widget's action line, and when the
  snapshot is not current the dimension widgets say so and name the same
  command. The Quality signal reads
  "*n* / 10000, geometric mean of the *k* applicable dimensions", with the
  production functions scored and test functions excluded beneath it and a
  disclosure explaining the thresholds fingerprint: it changes when
  `[metric_thresholds]` in `.logos/rules.toml` changes, and the next `logos
  gate` then saves the new score as the baseline by itself. The separate
  Aggregate scope card is gone. All ten
  dimensions — Modularity through Uniqueness, in the table's order — now have a
  widget with their plain question, score, raw value with its unit, any
  not-applicable reason, and what to do and where. Nesting, Conciseness,
  Cohesion, Focus and Uniqueness keep their offender lists and their three
  states; the other five say no list is recorded and point to where their units
  are found (the Architecture dependency matrix and `logos dsm`, Files & Risk
  and `logos hotspots`, `logos node`). "Brain method", "god container" and
  "near-clone" join the glossary. `GET /api/v1/health` is unchanged.
- **Every remaining web widget says what it shows, why it matters and what to
  do, and every view keeps one layout (CR-203, S-617).** The member Dashboard,
  Coverage, Rule findings, Architecture, Workspace Health's other four widgets
  and Workspace Statistics render through the shared widget frame, with their
  words in catalogues (`web/ui/src/copy/dashboard.copy.ts`,
  `coverageView.copy.ts`, `ruleFindings.copy.ts`, `architecture.copy.ts`,
  `workspaceHealth.copy.ts`, `workspaceStatistics.copy.ts`):
  - The **Dashboard** is one column of widgets instead of equal-width pairs. A
    missing figure is stated in its widget with the command that produces it
    (`logos scan`, `logos coverage ingest <report>`, `logos index`,
    `logos stats`, `logos wiki write overview/project-overview`).
  - **Rule findings** is one set of words on the Dashboard and the Rule findings
    view. A `.logos/rules.toml` that declares no rule now reads as nothing
    checked on the Rule findings view too, never as a clean result.
  - **Coverage**, with no report ingested, states that in each widget and names
    the ingest command; stale files name it too.
  - **Workspace Health**: *Promoted broker topics* is now **Broker topics**, and
    *Members answering*, *Members* and *Warm state* name `logos index` or
    `logos workspace status` for a member that did not open or index.
  - **Workspace Statistics** states an empty window in the Estimated value
    widget. Where a member's telemetry store could not be read, it names that
    store (`.logos/telemetry.db`) as the place to look, never `logos stats`.
  - Search boxes, query forms, editors, the chat and wiki pages are **tool
    panels**: the same frame, a title and one line saying what each is for, each
    listed with its reason in `web/ui/src/copy/toolPanels.ts`.
  Three checks keep the claim "every widget" true: a source scan of every view
  finds no `Card` rendered directly and every widget naming a catalogue entry or
  a registered tool panel; a test looks up every `logos …` command a catalogue
  names in `docs/howto/commands.md`; and the browser layout check now runs on
  every view of both sidebars. `docs/howto/usage.md` describes how to read a
  widget. A figure's unit or qualifier ("files ranked", "of lines covered", a
  scope or not-current line) is set at one body size, in muted ink, on every
  view; it had rendered at three sizes. No HTTP, CLI or MCP answer changes.
- **The Workspace Dashboard, the Cross-service coverage tab and Workspace rules
  say what each widget shows, why it matters and what to do, in one layout
  (CR-203, S-613).** Eight widgets now render through the shared widget frame
  with their words in catalogues (`web/ui/src/copy/coverage.copy.ts`,
  `workspaceDashboard.copy.ts`, `workspaceHealth.copy.ts`):
  - **Resolved cross-service edges** leads with "r of s outbound call sites
    resolved". Below 100% its action lists why the rest did not resolve, across
    every binding kind and largest first, each reason with its remedy and where
    to apply it. The counts add up to the unresolved figure.
  - **Cross-service reachability** leads with how many callables are unused in
    their own service but called from another, to keep. It reads "at least"
    when coverage is partial, and states an empty answer in place of a centred
    empty state.
  - **Members** glosses its figure headers and gives each row an action: run
    `logos index` in a degraded member, or review its callables unused across
    the workspace for deletion.
  - **Workspace rules** carries an *Advisory* badge and the figure "r rules
    checked over b bindings · v findings". With no rules it says nothing was
    checked and names `[[governance.boundaries]]` in `logos.workspace.toml`.
  - **Spec conformance**, **Coverage by intake** (with *intake* glossed),
    **Declared contracts and named externals** and **Build dependencies** take
    the same four parts. A workspace with no build manifest now sees that
    absence stated in the Build dependencies widget, where the widget used to
    be left out; Declared contracts still renders only when a member vendors a
    spec.

  Each workspace view stacks its widgets at one spacing, so the coverage tab's
  last three widgets no longer touch. A Playwright spec checks the equal gaps,
  left alignment and single body size on the real views. No figure, endpoint,
  CLI command or MCP tool changed.
- **The service map's bindings can be filtered, and its evidence states each
  fact once (CR-203, S-614).** On the Workspace tab's Service map:
  - **Cross-service bindings** gains a filter: text matching the consumer or the
    provider, a binding kind (HTTP, gRPC, broker) and — only when the Provenance
    column exists — a provenance kind. Its figure reads "n of m bindings shown".
    The filter narrows this table and Binding evidence; the map itself always
    draws every binding. A map with no binding resolved states that in the
    widget rather than as a centred empty state.
  - **Binding evidence** merges rows identical in end, member, key, value or
    refusal, profiles and defining sources into one row with a **Calls** count,
    so five calls sharing one key and value read as one row, "Calls 5". Each row
    says what to do: define the key, or replace the placeholder, in the member's
    configuration; nothing to fix in the repository when the value arrives at
    runtime; otherwise, if the value is wrong, correct it in the file named
    under Defining sources.
  - **Declared contracts** replaces its one disclosure per link with one
    Documents table and one Bound calls table, each row naming its member and
    counterparty.
  - **Cross-context model hint** carries a *Review hint* badge and points at the
    member's build manifest (`pom.xml` / `build.gradle`); *bounded context* joins
    the glossary.

  The four widgets take the shared frame with words in
  `web/ui/src/copy/serviceMap.copy.ts`, and the map and its widgets sit in one
  stack. A literal-only workspace still shows no Provenance column, filter or
  evidence; no vendored spec, no Declared contracts; and the build layer stays
  off until asked for. No figure, endpoint, CLI command or MCP tool changed.

### Fixed

- **The retired `/dsm` bookmark redirects to `/architecture` again (S-612).**
  The documented redirect had been lost when the SPA replaced the server-rendered
  views, so `/dsm` rendered no view. It is now a client-side redirect beside
  `/overview`, carrying the query and fragment across.

## [1.14.0] — 2026-10-08

### Added

- **Rust impl blocks, trait signatures and self calls record what one
  associated-item lookup needs (CR-202, S-606).** Every Rust `impl` block — an
  empty `impl Greet for X {}` included — records its header: its self type as
  written for any shape (a bare, module-relative `a::X`, `crate::` or external
  path with its generics stripped; `()`, `str` or a slice as written; `&T` and
  `&mut T` as `T`, flagged a reference), its trait, and an `impl Deref`'s
  `type Target`. Each `impl` function records its receiver mode (by value,
  `&self`, `&mut self`, typed `self: X`, or none), each `use` whether it is a
  re-export (`pub use`, any `pub(…)`), and each enum its variant names. A
  trait's required signature (`fn m(&self);`) is now a bodyless `Method` node
  of its trait, with its parameter range and whether it takes `self`; it binds
  no call yet and is never reported dead. A `self.m()` call's `Self::m` row now
  records the `self` shape, which a written `Self::m()` does not, and a fully
  qualified `<T as Tr>::m()` records `<T as Tr>::m` instead of the bare `m`.
  The facts come from new `@item.*`, `@ref.use.exported` and
  `@ref.call.qualified.*` captures in the Rust queries, so a droppable query
  override can tune them. Recording them moves no `Calls` edge: on a
  full-indexed export of this repository all 19,589 nodes and 46,791 edges are
  byte-identical, the 69 signature nodes and their 69 `Contains` edges are the
  only additions, 504 `Self::` rows gain the `self` shape, one bare `custom`
  row becomes `<toml::de::Error as serde::de::Error>::custom` (unbound before
  and after), non-Rust graphs are byte-identical and the quality signal is
  unchanged. Store migration 34 adds the columns and an `impl_blocks` table and
  clears every content hash, so the first `logos scan` (or `logos index`) after
  upgrading re-reads every file. A bare `logos sync` reads no file.
- **A Java or Kotlin call reaches an implemented interface's `default` body
  (CR-202, S-609).** When no class of a class's `extends` chain has an
  applicable method, a call on the instance (`m(1)`, `this.m(1)`) or on a
  proven receiver (`c.m(1)`) now goes on to the interfaces that chain
  implements, then their super-interfaces, nearest first. It binds a Java
  `default` method or a Kotlin interface `fun` with a body. A superclass
  method still beats an interface default. Abstract, `static` and `private`
  interface members are never bound this way. Two unrelated defaults of one
  name bind nothing (`overload-ambiguous`), arity applies, and a chain that
  crosses a superclass the graph does not hold reaches no interface. Plugins
  opt in with the new `inherits_interface_bodies` key (Java and Kotlin
  declare it; C#, whose default interface member is not inherited, does not)
  and mark `static`/`private` interface members (and a Kotlin interface's
  `companion object` functions) with `@item.uninherited`; a plugin declaring
  the key whose `symbols` query (an on-disk override included) has no such
  capture is refused at load.
  On the pec-services estate (60 Java members, 48,073 Java call rows), bound
  Java calls go from 7,128 to 7,568: 440 gained, 0 lost. Among them are the
  7 `MailboxControllerV1` → `verifyUserRetailOrPix` calls 1.13.0 left
  `no-applicable-overload`. Rust, Go, PHP, C# and Scala graphs are
  byte-identical. Store migration 35 adds a `nodes.uninherited` column and
  clears every content hash, so the first `logos scan` (or `logos index`)
  after upgrading re-reads every file. A bare `logos sync` reads no file.

### Changed

- **A Rust call to a type's method resolves through one associated-item
  lookup (CR-202, S-607).** `self.m()`, `Self::m()`, a method call on a proven
  receiver and a written `T::m()` now all bind among the functions of every
  `impl` block whose self type resolves to `T` — from any crate, through named
  and glob `pub use` re-exports, and for module-relative headers such as
  `impl a::X`. Syntax decides the filters: a method call never reaches an
  associated function (`fn new()`), and a path call to a `self`-taking function
  counts its receiver as an argument. An inherent function beats a trait's in
  any module; a trait method binds only where its trait is in scope (declared
  in the caller's module, imported by name, `as _` or a glob, or named by the
  caller's own `impl` header); an inherent `&self` method beside a trait's
  by-value `self` one binds nothing. A written `T::m()` no longer binds any
  same-named function of `T`'s module: `CallersResult::default()` stops binding
  `ResolutionDenominator::default`. Every unbound Rust call now carries a
  reason, with two new ones in `status`'s `call_residue` (CLI, MCP, HTTP):
  `not-a-callable` (an enum variant, a tuple-struct constructor) and
  `name-not-in-scope` (a prelude function, a closure, an item the file does not
  import); `unclassified` reads 0 on a fresh index. Java, Kotlin, C#, Scala
  and PHP rows list both reasons at 0. On a full-indexed export of this
  repository, 1.13.0 vs this release: `Calls` edges 23,916 → 26,121 (2,251
  added, 46 removed — every removal a call bound to another type's method);
  non-Rust graphs byte-identical; the Rust row's unclassified 16,360 → 0. The
  quality signal reads 8168 → 8128 on that export, from the newly bound
  cross-module calls (dependency depth 14 → 15, raw modularity 0.733 → 0.726). Plugins
  opt in with `impl_block_lookup = true` in `plugin.toml` (Rust declares it).
  Run `logos scan` (or `logos index`) to re-bind an existing graph; a bare
  `logos sync` re-binds only the rows its change touches.
- **Trait defaults, `Deref` targets, trait-typed receivers and qualified paths
  join the Rust lookup (CR-202, S-608).** A trait's default body is now a
  candidate for every type whose impl of the trait does not override it — an
  empty `impl Greet for X {}` included — so `x.hello()`, `X::hello(x)` and
  `self.hello()` bind it; an override beats the default, an inherent method
  beats both, the trait must be in scope, and two traits in scope that each
  supply the method bind nothing. A method call that finds nothing on its type
  retries on the type's `Deref` target, each type once and at most 8 hops; a
  path call never does. `self.m()` / `Self::m()` in a trait's default body, a
  receiver typed `impl Tr` or by a generic parameter bounded by `Tr` (inline,
  in a `where` clause, or on the enclosing `impl`), and a written `Tr::m(x)`
  now fan out as a `&dyn Tr` call does (a trait-typed receiver also inside a
  macro's arguments) to every impl of the method plus the default body. Two
  bounds that each supply the method bind nothing, and a
  method only a bound outside the repository (`Clone`, `Iterator`) supplies
  reads `external-type`. `<T as Tr>::m()` binds `T`'s impl of `Tr::m`, or
  `Tr`'s default. A sync now also re-binds the calls an `impl` block's header
  can move — an empty impl or a `Deref` impl added or removed — as a fresh
  index would. On a full-indexed export of this repository, the previous
  build vs this one: `Calls` edges 26,681 → 26,717 (36 added, none removed:
  `&impl EventSink`, `Arc<E: MemberEngine>` and three `GraphStore` default
  bodies fanning out to their impls), every delta checked against the source;
  non-Rust graphs byte-identical; 66 bare receiver calls become
  trait-qualified (62 now `external-type`); Rust `unclassified` stays 0; cold
  index time unchanged within noise. The quality signal reads 8126 → 8162 on
  that export (dependency depth 15 → 14). A bound is read as the caller names
  it, so an imported `std::io::Write` never reaches a repository trait called
  `Write`. Run `logos scan` (or
  `logos index`) to re-bind an existing graph.

### Fixed

- **A Rust call inside a macro records its turbofish path and its receiver's
  proof (CR-202, S-610).** A turbofish call inside a macro's token tree
  (`vec![Vec::<u8>::new()]`, `assert!(T::make::<u8>())`, `f::<T>()`) used to
  record a bare name (`new`, a path call that could bind any `new` in scope) or,
  past the name, nothing at all; it now records the path the same call records
  outside a macro (`Vec::new`, `T::make`, `f`), with its argument count, and
  never scans a turbofish's type arguments for calls (`Box::<dyn Fn(u8)>::new`
  calls no `Fn`). A method call on a plain name or an own field (`x.f()`,
  `self.x.f()`) inside a macro is handed to the same receiver proof as one
  outside it, so `format!("{}", m.f())` with `m: &M` in scope records `M::f`;
  an unproven receiver (a chain, a path, a shadowed name, a generic one no
  trait bounds) stays the `other` row it was, and a trait-typed one records
  the same trait-qualified row as outside a macro (S-608, above). A turbofish method call (`x.f::<T>()`) still records no
  row, inside or outside a macro. A name a pattern inside the macro binds (a
  closure parameter, `let`, `for`, a match arm or `matches!` guard) is not the
  caller's: its receiver stays `other`. A binding form of a user macro is not
  seen.
  On a full-indexed `git archive` export of this repository 195 `Calls` edges
  are added and none removed (47,025 → 47,220, all from Rust test and
  production code inside `assert!`, `format!`, `write!` and `params!`, each
  re-judged against its source: a typed parameter, a typed or constructed
  `let`, or an own field proves the receiver), 22 bare `new` rows become
  `Vec::new` and 10 `serde_json::from_*::<T>` rows are newly recorded (both
  unresolved, no edge), 606 `other` method rows become type-qualified, non-Rust
  graphs are byte-identical and the quality signal moves 8,167 → 8,168. A
  `logos sync` of one edited macro body equals a full re-index. No migration:
  the extractor change is picked up by the content-hash clear migration 34
  already makes, so a store indexed before it needs the same `logos scan` (or
  `logos index`) after upgrading; a bare `logos sync` re-reads no file.

## [1.13.0] — 2026-10-07

### Added

- **A callable records its parameter range and a call records its argument
  count (CR-190, CR-200, S-591).** Every `Function`/`Method` node of Rust, Go,
  Java, Kotlin, Scala, C#, C, C++, Python, PHP, Ruby, TypeScript and TSX now
  records the argument counts it admits: required parameters set the minimum,
  a defaulted one raises only the maximum, a variadic one (`...`, `params`,
  `vararg`, `*args`, `**kw`) makes it unbounded, and a receiver parameter
  (Rust `self`, Python `self`/`cls`, Go's receiver, Java's `C this`) is not
  counted. A PHP function's maximum is unbounded, since PHP passes surplus
  arguments through. Every call row records how many arguments it passes: a
  Kotlin trailing lambda counts as one, a Scala call counts its first argument
  list, and a spread (`*xs`, `...xs`, `xs: _*`) records unknown, as does any
  form a plugin cannot count (a Scala `using` list, a C# extension method,
  C's `()`, a C++ definition outside its class whose defaults may sit on a
  separate declaration, a Python method under an `if`/`try` of its class, a
  Ruby call with a bare `key: value` run).
  Every Rust `impl` function also records whether it takes `self`. The facts
  come from new `@arity.*` captures in each plugin's queries, so a droppable
  query override can tune them. The ranges and counts bind from S-592 and the
  takes-`self` fact from S-604 (both below); recording them alone moved
  nothing: on this repository
  symbols and edges are byte-identical, every node is unchanged apart from the
  new facts, and the quality signal is unchanged. A call's count joins its ledger row's identity, so `f(a)` and
  `f(a, b)` from one caller are now two rows; on this repository 268 `Calls`
  rows appear (95,154 → 95,422), which moves the `status` call figures (Rust
  references 90,125 → 90,234, `no-receiver-evidence` 47,404 → 47,509; Python,
  TypeScript and TSX likewise) without adding or removing an edge. Store
  migration 33 adds the columns and clears every content hash, so the first
  `logos scan` (or `logos index`) after upgrading re-reads every file. A bare
  `logos sync` reads no file.

### Changed

- **`status` computes the call residue once per graph revision (CR-201,
  S-605).** A long-lived engine keeps each language's `call_residue` until the
  graph revision, the schema version or the `[resolution]` section changes, so
  the web dashboard's header and page models stop re-walking every unbound call
  on each navigation. On this repository (release build) a repeated
  `GET /api/v1/status` falls from about 0.41 s to about 0.09 s; the first call
  still walks. The figures are unchanged on the CLI, MCP and HTTP alike, and a
  config or graph read that fails still states no residue and caches nothing.

### Fixed

- **A call binds only a callable whose arity admits it (CR-190, S-592).** The
  `self`, `super` and typed `T::m` receiver walks, Rust's proven receivers and
  a bare call in a language that overloads by name (Java, Kotlin, Scala, C#,
  C++) first drop every candidate whose parameter range excludes the call's
  argument count. A level left with none is passed over to the base class's
  overload, where the language records its classes' bases (not yet Scala, C++
  or TypeScript, whose call then stays unbound); a Kotlin unqualified call
  whose members all mismatch goes on to the top-level function or import of
  that name, unless the class has a base or interface the graph cannot see. Two candidates the count both admits stay
  `overload-ambiguous` (no argument type is read), and a call nothing admits is
  the new `status` `call_residue` reason `no-applicable-overload`. Defaults and
  varargs widen a range, and an unknown range or count filters nothing;
  JavaScript (`.js`, `.mjs`, `.cjs`, `.jsx`) is never filtered, `.ts`/`.tsx`
  are. New descriptor keys `overloaded_calls`, `arity_unchecked_extensions`,
  `implicit_call_falls_through` and `implicit_root_members` declare this per
  language. Three miscounts were fixed in the queries: a
  Kotlin/Scala `override` (whose defaults are inherited) and a Python
  `cls.m(…)` or PHP `f(...)` record unknown, and a comment opening a Python
  parameter list no longer hides `self`. Measured against the previous build
  on the 2026-10-03 inspection repositories: 8 of the 17 Sprint 87 name-only
  self-loops and the gitbucket `post` edge are gone (the rest, and eShop's
  `OnPropertyChanged`, are same-arity type mismatches or C++ members whose
  range is unknown); `Calls` edges move only in C# (−4/+307, mostly
  different-arity overloads that were ambiguous), Scala (−11/+28), Kotlin
  (−7/+14), C++ (−1/+24) and Java (+3), and a random sample re-judged against
  source was correct for 80 of 83. This repository's graph, and Go's on zap
  and ollama, are byte-identical; the quality signal is unchanged (8166).
  No migration.

- **A Rust method call never binds an associated function without `self`
  (CR-200, S-604).** On a receiver whose type is proven, `x.name()` used to
  bind an inherent `fn name()` over a trait impl's `fn name(&self)`, because an
  inherent candidate outranked a trait impl's whether or not it could be
  called that way. rustc calls the trait method. A candidate recorded as not
  taking `self` is now dropped before that rank, so the call binds the trait
  impl's `name`; two genuine methods still bind the inherent one, and a type
  whose only `m` takes no `self` leaves `x.m()` unbound with
  `supertype-unreached`. A candidate whose fact is unknown is kept. `Self::m()`
  calls are unchanged: they may name an associated function. On this
  repository nodes, symbols and edges are byte-identical and the quality
  signal is unchanged, as no type here pairs such a function with a method of
  the same name. The fact is the one migration 33 records, so a store needs
  the same one `logos scan` (or `logos index`) after upgrading.

- **A config-narrowing purge advances the graph revision (CR-201, S-605).**
  When an `exclude` edit narrowed the configuration, two paths removed the
  newly excluded files without advancing the graph revision if nothing else
  changed: the reconcile an evaluation tool runs first (`logos scan`, `gate`
  and the others), and the navigation prologue, which runs once at an engine's
  first navigation. A reader keyed on the revision — the native wiki tier, and
  now the call residue — kept serving the purged graph until the next
  graph-changing `sync`. Both purges now advance it once they have committed.

## [1.12.0] — 2026-10-06

### Added

- **A Rust call records its receiver's type where the file proves it (CR-188,
  S-587).** A call `x.f()` is now recorded as `T::f` when exactly one binding
  of `x` in scope at the call proves `T`: a typed parameter, a typed `let`, a
  constructor `let x = T::new(…)` when every `new` the caller's module declares
  on `T` returns `Self` or `T` (any associated function, by the same rule), a
  one-segment struct literal `let x = T { … }`, or `self.field` declared on the
  caller's own struct in the caller's module. `&`, `&mut`, `Box`, `Arc` and `Rc` are peeled to `T`,
  and the row records which wrappers it peeled; `Option`, `Vec`, `Mutex` and
  every other wrapper are the receiver's type themselves. A shadowed, re-bound
  or two-typed name, a generic parameter, `impl Trait` and a chained call keep
  the `other` row they had. On this repository 5,855 of the 52,526 Rust
  receiver-call rows are retyped; the next entry binds them. Store migration 32 adds the ledger's
  `peeled` column to its identity and clears every content hash, so the first
  `logos scan` (or `logos index`) after upgrading re-reads every file. A bare
  `logos sync` reads no file.

- **A Rust call on a proven receiver binds among its type's methods (CR-188,
  S-588).** A call `x.f()` recorded as `T::f` now binds `T`'s one method `f`.
  `T` is read through the calling file's `use` declarations (a `pub use`
  re-export is followed to the declaration it names) to exactly one type
  declared in this repository, in the caller's crate or another one, so a
  `cli` crate's `engine.runtime()` reaches `logos_core`'s `Engine::runtime`. An
  inherent method outranks a trait impl's of the same name. Nothing binds when
  `T` is not declared in the repository (`String`, `Vec`, `str`,
  `std::io::Error`, an external crate's type), when a peeled `Arc`, `Rc` or
  `Box` provides the method itself (`x.clone()` on an `Arc<T>` is
  `Arc::clone`; the list is the Rust plugin's new `[wrapper_methods]` table),
  when a same-named type is one the file never imports, when the file
  re-exporting `T` also imports its name elsewhere (a top-level `use` beside an
  inline `mod`'s `pub use`), or when the type has no such method or two of one
  rank. On this repository, against the same tree,
  1,077 `Calls` edges are added (292 across crates), every one from such a
  row; nodes and symbols are unchanged, and 153 functions reported dead are
  now reached. Run `logos scan` or `logos index` to bind them.

- **`status` says why Rust calls stay unbound (CR-188, S-589).** The Rust row of
  `resolution_by_language` now carries `call_residue`, as the Java row does, in
  `status --json` and the MCP `status` tool: `unbound` (the row's
  `calls.references − calls.bound`), the count per reason and `unclassified`.
  `external-type` counts a proven receiver whose type the repository does not
  declare, or a method the peeled `Arc`/`Rc`/`Box` provides itself;
  `no-receiver-evidence` counts a method call whose receiver is not proven. A
  Rust path call (other than `Self::m` inside an `impl`) or bare call that does
  not bind takes no receiver walk, so it is counted in `unclassified` instead of
  being given a reason. Capture-before-delete
  rows are left out, as in every other figure. On this repository the Rust row
  reads 68,279 unbound: 47,278 `no-receiver-evidence`, 4,774 `external-type`,
  146 `supertype-unreached` and 16,081 `unclassified`.

### Fixed

- **A bare call never binds a method in a language where it cannot reach one
  (CR-189, S-590).** In Go, Rust, Python, PHP, JavaScript and TypeScript a
  method is reached only through a receiver (`self.`, `$this->`, `this.`,
  `s.`). A bare `f()` with no free `f` in scope still bound a same-named
  method, often the caller itself, so `fwrite()` inside a PHP method `fwrite`,
  `deepcopy(x)` inside a Python method `deepcopy` or `performWebSearch()` beside
  a Go method of that name drew a self-loop instead of staying unresolved. Now,
  in a plugin that explicitly declares `implicit_receiver = "none"`, a
  single-segment bare call never binds a member of a class-like container or a
  callable with a recorded self type (a Go or Rust method of a named type). It
  binds the free or imported function when there is one, a nested function
  still binds, and otherwise the call stays unresolved. A Rust `use` or glob
  import of a free function now binds it even when its module also has an
  associated function of that name. A TypeScript or JavaScript class method
  named `f` no longer hides the file's `import { f }`, so a bare `f()` in that
  file binds the imported function. The Go and Rust plugins now declare the
  key; Python, PHP, TypeScript and TSX already did. Java declares nothing, so
  its bare in-class calls bind as before. C#, Kotlin, Scala, C++ and Ruby are
  unchanged. Expect fewer `Calls` edges in those six languages after the next
  `logos scan` or `logos index`, and possibly some methods newly reported dead
  that were reached only through such a call.

- **Near-clone clustering no longer counts every pair of a shingle posting
  (CR-198, S-601).** Clustering counted the shared shingles of every pair of
  functions that carry a common shingle, so its memory and time grew with the
  square of how many functions share the commonest shingles. On this
  repository, every one-file `sync` peaked at about 2.4 GB, and a long-running
  `serve --mcp` paid that on every watcher-triggered sync. Clustering now orders
  shingles rarest-first and indexes only a short prefix of each function's
  shingles. It keeps only pairs whose sizes can reach the threshold and checks
  each remaining pair's similarity exactly. A shingle carried by thousands of
  functions sorts last and drops out of almost every prefix. The thresholds,
  the eligibility floor and the group identifiers are unchanged, so every
  `clone_group` verdict is byte-identical to before. The prefix bound uses the
  verdict's own comparison, so no rounding can drop a pair.

## [1.11.1] — 2026-10-06

### Fixed

- **An incremental re-bind retracts the edges it no longer produces, for every
  language (CR-193, S-596).** When a sync re-selected a row that was bound and
  the row came back unbound, ambiguous or bound to another target, the row's
  flag flipped but its old edge stayed, so the synced graph kept a call, import
  or supertype edge that a fresh index of the same tree does not have. This
  happened, for example, when a rival `helper` or a second Kotlin `class Bar`
  arrived, when a second glob import supplied the called name, when `x.rs`
  arrived beside `x/mod.rs`, or when a Java class dropped the supertype a call
  climbed through. Python import-root files were already covered (S-519).
  Now every row of the re-selected row's source is re-bound, in every language,
  and each reference-bound edge out of that source that no row produces any
  more is deleted in the same transaction. A capture-before-delete row no longer
  restores an edge that its source's own re-bound rows do not produce. The unit
  is the source symbol, not its whole file: a whole-file sweep re-bound about
  40% of this repository's ledger on every one-file sync. A full index is
  unchanged.
- **Sync leaves no stale rows in the reference ledger (CR-187, S-586).** A sync
  saves each cross-file edge into a re-extracted file as a capture-before-delete
  row so the edge survives the delete. That row stayed in the ledger after it
  re-bound, so `status` counted refs a fresh index of the same tree does not
  have. Now the resolution pass deletes a capture row once it re-binds, once
  its source's own rows are re-bound and decide its edges, or once its source
  is gone, in the same transaction that restores the edge. The edge stays. A
  capture whose target was renamed away, and whose edge no other row carries,
  stays unresolved as before. In a store synced by an earlier version, the
  stale resolved captures go on the next `sync`, even one that changes
  nothing; an unresolved one goes when its source is next re-bound, or on
  `logos index`. No migration. After any sequence of syncs the ledger now
  equals a fresh index's row for row, so `refs_total` and `refs_resolved` agree.
  On nlohmann/json, an edit → sync → revert → sync cycle left 47 extra resolved
  doc→code rows; it now leaves none.
- **Two imports of one simple name are ambiguous in the package rungs (CR-196,
  S-599).** `import a.Helper; import b.Helper;` then `Helper.util()` bound whichever
  import the file listed first, because the qualified-head rung read a first-wins alias
  map. It now resolves the rest of the path under every distinct import of the head's
  name, binds only where they reach one declaration, and records `type-ambiguous`
  where they reach two — the same rival rule Python imports already followed. A single
  import and a verbatim repeat bind as before, and a member type in lexical scope still
  wins. An import of a name no in-repository type carries is skipped rather than
  shadowing a rival that does, so that rival now binds in either import order. Rival
  heads met inside one another's expansions bind nothing, which keeps the work flat as
  imports multiply (a re-entrancy guard, as for globs). The same head in an
  `extends` clause (`Helper.Inner`) binds nothing under rival imports.
- **`status` resolution figures exclude capture-before-delete rows (CR-195, S-598).**
  Syncing a file writes a capture-before-delete row for each edge pointing into it,
  stored under the *target* file and duplicating a reference its source's own row
  already records. `status` counted both, so a synced store read higher than a cold
  reindex of the same tree: a two-file Go module read calls and imports 1/1, then 2/2
  after `sync a/a.go`, then 1/1 again after a reindex. Those rows are now left out of
  every figure counted over the ledger — the per-language `references` and `bound`, the
  global `refs_total`/`refs_resolved` ratio and the per-relation-class coverage, the
  call residue's `unbound`, per-reason and `unclassified` counts, and the global ratio a
  `sync` itself reports — on the CLI, `--json`, MCP and HTTP alike. This rule changes
  no ledger row: the rows are still written and re-bound, and the sync deletes each
  one once it is spent (CR-187, above). The rows it keeps out of the figures are the
  ones that stay: a capture awaiting a target that was renamed away, and the captures
  a store synced by an earlier version still holds. `same_file_edges` and
  `cross_file_edges` were never affected. On a freshly indexed store there are no such
  rows, so every figure is byte-identical to the last release.
  **Measured** on monolog at `d7059e4c` after `sync src/Monolog/Logger.php`: PHP
  imports 448/585 → 431/568 and the global ratio 2,678/7,228 → 2,650/7,200, equal to a
  cold reindex's (PHP calls 1,034/5,189 throughout).

### Changed

- **Two local names of one import target are two reference-ledger rows (CR-194,
  S-597).** The ledger's identity is now `(source_symbol, target, form, kind, payload,
  receiver, alias)`, with a missing alias normalised so a row without one dedups as
  before. The unique index, the writer's `ON CONFLICT` target and the extractor's
  deduplication key all name those seven components. Until now the alias was not part
  of it, so `from pkg.m import X as A` and `… as B` in one scope, or `import numpy`
  beside `import numpy as np`, kept one row and dropped the other at insert: the second
  local name never reached the ledger and never bound. Python, Kotlin, Scala and every
  other language that records an import alias keep both: a Kotlin `class Aliased :
  KBase()` beside `import p.Base` and `import p.Base as KBase` now extends `Base`
  (it was left unbound). Aliasless rows are unchanged.
  Two rows of one target are still one edge (an edge is `(source, target, kind)`), so
  the gain is the second name's binding, not a second edge.
- **Measured** on `594452f6` of werkzeug and on this repository at `9d401b9f`, indexed
  by 1.11.0 and by this build over identical trees: werkzeug 9,748 → 9,748 ledger rows
  and 4,843 → 4,843 edges; this repository 103,025 → 103,026 rows and 43,641 → 43,641
  edges. The one added row is an alias twin — `use crate::model::BridgeRole as Role`
  beside the `BridgeRole` import already kept — and every edge is byte-identical, Rust
  edges included. Upgrading a populated v30 store in place gives the same ledger and
  edges as a cold index.
- **Plugin queries compile on first use of their language (CR-197, S-600).** Engine
  start no longer compiles every compiled-in language's tree-sitter queries — most of
  a cold start (~620 of ~710 ms in a debug build), and what failed the NFR-PE-05
  budget tests in `scripts/gate.sh full`. Each language compiles all its queries, once per process,
  on the first extraction that needs it, so a cold start pays only for the languages
  a repository uses; concurrent first uses of one language compile it once. An
  on-disk override under `.logos/plugins/<lang>/` still compiles its whole language
  at load and still fails the load naming its file. A broken *embedded* query no
  longer fails the load: a test compiles every embedded query and names the file of
  a broken one, so it never ships. Each language's first-use compile is reported once
  per process as an `info` event (`RUST_LOG=info`) with its duration. Extraction
  output is unchanged.
- **A C# namespace sees the types of its enclosing namespaces (CR-192, S-595).**
  A type in `A.B.C` that names `Base` declared in `A` was left unbound without a
  `using A;`, though C# resolves it by walking out through `A.B` and `A`. A plugin now
  declares `enclosing_namespaces = true` under `[module_model]` (the `namespace` model's
  key, refused under any other), and the C# plugin does. For such a language a simple
  type name, or the head of a qualified one, that the source's own namespace does not
  supply is read in each enclosing namespace, nearest first, before any `using`
  namespace. One type decides a level; two at one level bind nothing (`type-ambiguous`)
  and the walk never falls through to an outer level; none passes outward. Only the
  source's own interop family is read, and the global namespace is never a level. A
  single-type `using` stays final for the name it imports. No language id is named in the
  resolver; PHP, Kotlin, Scala and Java bind exactly as before. Two ceilings, both
  shared with the same-namespace rung: types are indexed by name, so a generic `Result<T>`
  in an enclosing namespace is taken for a non-generic `Result`; and a `using` written
  inside a namespace block is read after the enclosing namespaces, where C# reads it
  before them. A qualified head (`Result.Inner`) whose enclosing type lacks the member
  still reaches the `using` that has it.
- **Measured** on Newtonsoft.Json at `52fa3aef`, indexed by the parent commit's build and
  by this build over identical trees: C# `Extends`/`Implements` ledger rows bound 263 / 886 → 290
  / 886 (+22 `Extends` edges, +5 `Implements` edges, none removed), and incoming `Extends`
  on `JsonReader` 2 → 10 of its 10 subclasses. All 27 added type edges were re-judged
  against the source and are correct: each source sits in a namespace under
  `Newtonsoft.Json`, and each target is the one type of its name in the repository. The
  newly bound supertypes also let 54 `base.X()` calls climb to their inherited method
  (cross-file C# `Calls` edges 56 → 110); 81 ledger rows flipped to resolved in all, and
  none flipped back. The Java, PHP, Kotlin and Scala fixtures and this repository (147,528
  edge and ledger rows) are byte-identical before and after, and so is the C# fixture.
  The C# `[reach]` table is unchanged: the measured relation classes did not change.

### Upgrade

- A store that was synced reads **lower** after upgrading: the capture-before-delete rows it
  already holds stop counting (CR-195). That is the figure a cold reindex reports, not a
  loss of references; no migration or re-index is needed for it.
- One forward-only store migration, **31** (the alias joins the ledger identity index).
  It changes no row, id or column and clears every file's content hash, so run **one
  `logos scan` or `logos index`** after upgrading — a bare `logos sync` re-reads nothing
  and the rows the old identity dropped would not return. Migration 30 is not edited: a
  store already at 30 receives 31 as its own step.
- **This ships after 1.11.0, in the next release.** A store that already applied 1.11.0's
  migration 30 re-extracts a second time on its first index after upgrading; a store
  upgrading straight from 1.10.x or earlier to that release pays one re-extraction for
  30 and 31 together. Earlier `logos` versions refuse an upgraded store.

## [1.11.0] — 2026-10-05

### Changed

- **Inheritance binds for Python, PHP, C# and Kotlin (CR-170, S-522).** Their
  `references` queries capture supertypes — Python `class A(B)`, PHP `extends`,
  `implements` and trait `use`, C#'s `base_list`, Kotlin's supertype list — and an
  `Extends` or `Implements` from a type binds through the module model its language
  declares, not only from a package-shaped source. It binds the one in-repository
  type the file's scope names, never a workspace name guess under any policy; a
  library base stays unbound. `Implements` may target an interface or a trait (a PHP
  class's `use`d trait). C# and Kotlin declare the new `plugin.toml` key
  `supertype_kind_follows_target`: their supertype list does not say which entry is
  the base class, so each entry's edge is `Extends` to a class and `Implements` to an
  interface or a trait. PHP's leading `\` and C#'s `global::` read a name from the
  global namespace only; a PHP `namespace\X` is not read, and a class never names
  itself (`class TestCase(TestCase)` names the import). A proven `Extends` is what a
  call on the current instance climbs: `self.m()`, `$this->m()`, `this.M()` and an
  unqualified Kotlin/C# call now reach an inherited method, and `super().m()`,
  `parent::m()`, `base.M()` and `super.m()` the nearest base that declares it. A
  Python class with several bases (whose MRO the walk does not read) and a PHP class
  that uses a trait (whose methods outrank the inherited ones) are never climbed
  through; Python's `super(A, self)` and Kotlin's `super<T>` stay unbound. Rust's
  `Implements` is unchanged. Java's type relations are unchanged on a Java-only
  repository; a Java class may now implement a Scala trait, and a Java file outside
  every source root binds its `extends` through its scope.
- **An import's alias is the name it binds locally (CR-170, S-520).** A `references`
  query can now mark that name with `@ref.import.alias`, and the extractor records it
  as the import row's alias instead of the path's last segment. Python, PHP, C# and Go
  switch it on: `import typing as t` records `t`, `use A\B as C` records `C`,
  `using Test = Xunit.FactAttribute` records `Test` (one row naming the type; the
  alias is not an import row of its own) and Go's `internalcloud "…/internal/cloud"`
  records `internalcloud`. Kotlin's `import a.b.C as D` records `D`, and Scala's
  `import a.b.{C => D}` and `import a.b.C as D` record `D`: beside `import other.C as
  D`, a `class X : C()` or a `C()` reaches the same-package `C`, never the imported
  one. A Python `as` import used to record no alias at all; a call
  through the alias (`from .helpers import open as open_resource`, then
  `open_resource(p)`) now binds, and a call to the original name still does not. Rust's
  import rows are unchanged (3,459 rows byte-identical on this repository).
- **Measured on Newtonsoft.Json**: the 654 junk import rows an alias directive used to
  record as its own name (before S-518) are 0, and all 657 alias directives are one row
  each aliased by their local name.
- **Calling a class instantiates it, and calling a C macro binds it (CR-170, S-521).**
  Two new `plugin.toml` keys widen what a call may bind. `class_call_instantiates`
  (Python, Kotlin, Scala): a call whose one candidate is a class records an
  `Instantiates` edge to it, so `Check(project=p)` after `from hc.api.models import
  Check` reaches `Check`. `macros_callable` (C): a call whose one candidate is a
  function-like macro records `Calls` to that Macro node. The exactly-one rule is
  unchanged: two classes of one name, a function and a class (or macro) of one name
  in one scope, or no candidate leave the call unbound. Every other language binds
  its calls as before. The declared reach of Python, Kotlin and Scala adds
  `type_relations`: a free `Foo()` instantiates a class declared in another file.
  A Kotlin or Scala `Foo()` inside a class body is a call on the current instance,
  and still binds among that type's own members only.
- **Measured on libuv** (`49b1c064`): C calls bound to a macro go from 0 to 342, and
  all C calls bound from 1,210 to 1,550 of 13,308. The calls in `src/fs-poll.c` to
  `uv__make_close_pending`, which that file defines as a macro, now bind. Two calls
  that bound a function no longer bind: their file defines a macro of the same name
  under another `#ifdef` branch (`uv__cpu_count`, `uv__random_getrandom_init`), so
  the name has two candidates. On this repository every edge and ledger row is
  identical before and after.

- **A Rust `mod x;` declaration no longer blocks binding through it (CR-186, S-585).**
  The declaration is a node of the declaring file keyed exactly where `x.rs` /
  `x/mod.rs` is, and whichever node was numbered first answered that key — usually
  the empty declaration, so `crate::x::…`, `self::…` and `super::…` paths through it
  bound nothing. The key now answers with the one file the declaration names; a
  `#[path]` declaration, a missing file, or two files (`x.rs` beside `x/mod.rs`) stay
  unresolved. No node or symbol changes. On this repository `crate::` imports bound
  rise from 359 to 616 of 1,050 and `crate::` calls from 97 to 288 of 386 (all Rust
  imports 1,655 → 2,312 of 6,359; calls 18,761 → 19,535 of 85,827); 42 imports of a
  declared module now name its file instead of the declaration. Nearly all remaining
  `crate::` misses go through a `pub use` re-export. The scan signal moves 8402 →
  8319 on a full index, from the newly bound cross-directory edges (modularity
  0.874 → 0.833, depth 9 → 10; redundancy improves as 18 functions stop reading
  dead). The Rust reach fixture regains its `mod` lines.
- **Python imports bind: packages, import roots and relative levels (CR-170, S-519).**
  The path module model gains plugin-declared data on `[module_model]`:
  `package_stems` (a package file names its directory — Python `__init__`, Rust
  `mod`/`lib`/`main`, moved out of the core with every Rust module key and symbol id
  unchanged) and `import_roots` (Python is keyed under `src/` when a package sits
  beneath it, else the repository root). Every directory under an import root is a
  module, so a namespace package descends; a relative import keeps its level
  (`from .rules import Rule`, `from .._internal import x`); `from a import b, c`
  records one import row per name; and an import of a name a package's `__init__.py`
  re-exports binds to that package. `.logos/config.toml` gains
  `[resolution.import_roots] python = [...]`, which replaces the detection. Python's
  declared reach rises from `same-file` to `partial` (calls, imports).
- **Measured on the inspection repositories**, Python import rows bound before → after
  (the denominator grew because each imported name is now its own row): werkzeug
  0 of 1,072 → 732 of 1,520, healthchecks 0 of 2,343 → 1,123 of 3,171. Of the internal
  imports, werkzeug binds 716 of 760 and healthchecks 1,123 of 1,167; the rest name
  module-level variables, which are not declarations. Cross-file Python calls appear
  for the first time (werkzeug 310, healthchecks 270), and healthchecks' Django routes
  bound to their views rise from 88 to 136. A name imported from two places (a
  `try`/`except` compat import) binds only where both imports agree, and an import
  repeated verbatim is one import, not a rival (werkzeug's functions each re-run
  `import warnings`, 13 times in `wrappers/request.py`), so the binding adds no
  measurable indexing time: on debug builds made the same way, a cold index of
  werkzeug takes 3.3 s and healthchecks 5.9 s, against 3.4 s and 5.7 s for 1.10.0.
- **A JavaScript, TypeScript, Go or C `main`, `lib` or `mod` file is named after
  itself**, not its folder: those stems are Rust's, now declared by the Rust plugin
  alone. On this repository 12 documentation tokens that bound `ui` to
  `web/ui/src/main.tsx` only because of the old name no longer bind.
- **An interop `family` on `[module_model]` keeps binding inside languages that can
  name each other's types.** Java, Kotlin and Scala declare `jvm`; a plugin declaring
  none is its own family. The fully-qualified type index and the namespace index are
  partitioned by it, so a C# `using App.Models;` never binds a PHP `namespace
  App\Models;`, while Java↔Kotlin binding is unchanged (koin: identical edges).
- **An import never names a framework-promoted `route` or `component`.** A Django model
  or an Axum state type is both a class and a promoted component of the same name, and
  every import of it read as ambiguous.
- **The workspace suffix match compares a module by its parent key**, so `pkg.mod`
  reaches `pkg/mod.py`. On this repository one Rust import newly binds
  (`observability/mod.rs` → `stats::stats`).

- **PHP, C#, Kotlin and Scala files take their identity from the namespace or package
  they declare (CR-170, S-518).** A plugin now names its module model in one
  `plugin.toml` table, `[module_model] kind = "path" | "package" | "namespace"`; PHP,
  C#, Kotlin and Scala declare `namespace`, Java `package` (unchanged). A file's
  `namespace` / `package` declaration — not its directory — names its types, so a
  PSR-4 tree, a C# project whose namespaces differ from its folders and a Kotlin
  Multiplatform `commonMain` source set all bind their imports. A single-type import
  binds the type it names and is final for that name; a type of the file's own
  namespace is visible without an import; C#'s `using N;`, Kotlin's `a.b.*` and Scala's
  `a.b._` are wildcards that bind to every other file declaring the namespace; C#'s
  `global using` applies to every C# file under its file's directory; `using static`
  and aliases bind the type they name. Composer maps and `.csproj` files are not read.
  Scala imports are captured for the first time.
- **Measured on the inspection repositories**, import rows bound before → after:
  monolog 0 → 429 of 568, mantisbt 0 → 201 of 224, koel 0 → 4,555 of 7,705, eShop
  0 → 411 of 929, Newtonsoft 1 → 947 of 5,038, koin (Kotlin) 25 → 893 of 3,249. What
  stays unbound is library code (PSR, `System`, Illuminate, PHPUnit) and imports of
  functions rather than types. C# rows fell because an alias `using X = Y;` no longer
  also records `X` as an import. Coupling metrics move for these languages: a C#
  `using` binds every file of its namespace (Newtonsoft: 48,604 `Imports` edges).
- **Kotlin's `src/{main,test}/kotlin` source-root keying is replaced** by its `package`
  header. Its declared-type facts are named by the package and are no longer refused
  when the package differs from the directory.
- **PHP, C# and Scala now declare `partial` reach** (`imports` bound across files);
  `logos languages` and the manual's table say so.

### Upgrade

- One forward-only store migration, **30** (`files.namespace`). It clears every file's
  content hash, so run **one `logos scan` or `logos index`** after upgrading — a bare
  `logos sync` re-reads nothing and the new bindings would not appear. Until then each
  such file keeps the path key it had.
- **The signal moves on the first index after upgrading.** The newly bound edges (Rust
  paths through `mod x;`, and cross-file imports and supertypes in PHP, C#, Kotlin, Scala
  and Python) usually lower modularity. On this repository it went 8402 → 8319. Re-establish
  the baseline with `logos gate --save` after that first index. See
  [metrics.md](docs/howto/metrics.md).

## [1.10.0] — 2026-10-04

### Changed

- **A call on another object never binds to the caller's own method (CR-169).** Every
  method-form call now records its receiver's shape — `self`, `super` or `other` — and
  binds by it instead of walking scope outward from the caller. `this.m()` / `self.m()`
  binds to the caller's own class; `super.m()` only through a proven base class; a call
  on any other receiver (`x.m()`) binds nowhere and is reported `no-receiver-evidence`.
  All twelve code languages declare their receiver forms (TS/TSX/JS, Python, PHP, Ruby,
  C#, Kotlin, Scala, C++, Go, Rust; Java maps its existing receiver typing onto the
  same shape, byte-identically). No binding policy widens a receiver call any more.
- **This trades recall for never fabricating, most visibly on Rust.** On this
  repository the release removes 2,105 `Calls` edges and adds 88 (self-loops 202 → 60),
  and 191 callables reached only through `x.m()` chains now read dead (11 the other
  way); the quality signal moves 8446 → 8397. A sample of removed Rust edges was 15/19
  correct, 4/19 fabricated — Rust receiver typing is what would restore the correct ones.
  On Go (zap, ollama) the re-bound own-receiver calls judged 40/40 correct.
- **Rust `self.helper()` and `Self::helper()` bind through the enclosing impl (CR-159).**
  Each impl method records its self type, so a self call binds to its own impl's
  method, never a sibling impl's or another crate's same-named type.
- **Go methods record their receiver's base type** (`func (s *Svc[T]) Work()` → `Svc`),
  and a call on the method's own receiver binds to that type's method — never to a free
  `func` of the same name.
- **A bare call to a nested `def` or local function binds the local callable**, never the
  class member it shadows (Scala, Kotlin, C#). In a Kotlin chain `a.b.c()` only `c` is a
  call, and a Scala auxiliary-constructor delegation `this(…)` is no call.

### Upgrade

- Two forward-only store migrations: **28** (`nodes.self_type`) and **29**
  (`unresolved_refs.receiver`, plus the ledger identity index). Both clear every file's
  content hash, so run **one `logos scan` or `logos index`** after upgrading — a bare
  `logos sync` re-reads nothing and the new bindings would not appear.

## [1.9.2] — 2026-10-04

### Fixed

- **A production source root overrides the test file-name conventions too — for JVM
  file types only.** A `*Test` / `*Tests` / `*_test` / `*_spec` / `*.test.*` file with a
  `.java`, `.kt`, `.kts`, `.scala` or `.groovy` extension beneath `src/main/…`
  or a Gradle `*Main` source set is production (any other extension, e.g. `foo.test.ts`,
  keeps its filename rule): koin's `KoinTest.kt` (`commonMain`) and
  `AutoCloseKoinTest.kt` (`src/main`) were still `is_test`, so all 128 nodes under
  `org/koin/test/` are now production (114 before). Files outside a production root,
  and `src/it/` / Gradle `src/*Test/` source sets, classify as before.

### Changed

- **The bare-name preference reaches `callers`, `callees`, `impact` and `explore`.**
  Each now resolves a bare name like `node` does — a code type, else a callable, else
  another code declaration, else a module, else a configuration artifact, else a
  documentation node — and lists what it passed over as `alternatives` (CLI and MCP;
  absent when nothing was passed over). `callers Utils` on a PSR-4 PHP tree now reports
  the callers of the `Utils` class rather than of its `Utils.md` doc section or file
  module. `impact-intersection` and `precedent` keep lowest-id resolution and their
  ambiguity warning.
- **C and C++ declare their reach as `same-file`.** Both bind calls within a file and
  nothing across files, so `logos languages`, the manual and the README now list them
  with Python, PHP, C#, Ruby and Scala. No shipped language declares `symbols`.

## [1.9.1] — 2026-10-04

### Fixed

- **An index never silently empties.** Same-named declarations of different kinds —
  a Go function and method `F`, a TypeScript `interface X` and `class X`, a Scala trait
  and its companion `object`, a C `typedef struct S S` — collided on one symbol and
  aborted the whole index while `index` exited `0` with 0 files. They now get distinct
  symbols (siblings are numbered per descriptor family), and a declaration whose name
  the parser could not recover emits nothing. zap, ollama, preact, gitbucket, ox,
  ccache, nlohmann/json, redis and libuv now index every admitted file. Symbols of
  Rust and Java code are unchanged.
- **One file that cannot be persisted fails alone.** Each file persists under its own
  savepoint: a failing file is rolled back alone, listed in `files_failed` with its
  reason (`persist_failures`), and named in a warning, while every other file is
  stored. On `sync` and in the `serve` watcher a failing file keeps its last good facts
  and is marked stale until it persists. `status` and `scan` report
  `persistence: {failed_to_persist, stale_files}` when anything remains. An
  `index`/`sync` that reached files and persisted none exits `1`; an index that
  admits no file still exits `0`.
- **No declaration takes a parse-error region's span.** Error recovery no longer lifts
  a declaration into an enclosing ERROR node (ccache's `parse_umask` spanned a whole
  627-line file at complexity 117; it now spans its own declarator), and a C++ class
  head stranded in an ERROR is recovered (nlohmann/json's `class exception`). The
  file's partial-extraction warning counts what was affected:
  `N declaration(s) truncated and M skipped at a parse error`.
- **Test code is classified by where it lives.** A `test`/`tests`/`spec` directory
  under a production source root (`src/main/…`, a Gradle `*Main` source set) is
  production; `src/it/` and Gradle `src/*Test/` source sets are test code; a
  `*.test.*` tag needs a three-part file name (a bare `test.py` is not a test); a PHP
  `test*` method is a test only in a `*TestCase` subclass or a test file; and a
  module node is no longer re-judged by the test-name markers.

### Changed

- **Every language states its cross-file reach.** `logos languages` (and `--json`)
  carries `reach.level` and `reach.cross_file` for each code language: `resolved`
  (Rust, Java), `partial` (Go, TypeScript/TSX, Kotlin — imports only), `same-file`
  (Python, PHP, C#, Ruby, Scala) and `symbols` (C, C++). The manual's table and the
  README are generated from the same declarations, and a fixture per language fails
  the build if a declaration over- or under-claims.
- **A bare-name `node` lookup prefers code.** When a bare name matches several nodes,
  `node` resolves to a code type, else a callable, else another code declaration, else
  a module, else a configuration artifact, else a documentation node, and lists what it
  passed over as `alternatives` (CLI and MCP). Qualified and SCIP lookups are
  unchanged. `callers`, `callees`, `impact` and `explore` still resolve a bare name by
  lowest node id.

### Upgrade notes

- **Store migration 27** (forward-only) adds the `persist_failures` table. The first
  run of this release on an existing project migrates `.logos/logos.db`; Logos 1.9.0
  cannot open the migrated store afterwards (exit 3). Back up `.logos/` first if you
  may need to downgrade.

## [1.9.0] — 2026-10-03

### Added

- **Health shows the worst offenders its snapshot computed.** Every `scan`, `gate`,
  `session_start` and `session_end` now persists its per-dimension worst-offender lists
  with the snapshot (migration 26), and the Health page reads them from the same single
  snapshot read as the signal. Each drill-down renders one of three states: the offender
  table, *No offenders flagged within thresholds* (only for a recorded-empty result), or
  *Offenders were not recorded for this snapshot — run `logos scan`* for a snapshot
  written before this release. `scan --json`'s `worst_offenders` carries a `recorded` flag.
- **The Workspace Chat has its own view.** In a workspace serve the Workspace section
  lists Chat at `/workspace-chat` and the Service section lists none; `/chat` redirects
  there with its query. Chat state is kept per scope, so a thread from one chat is never
  reopened in another. `GET /api/v1/workspace/config/read-roots` lists each member's
  effective read roots without starting an engine.
- **`[metric_thresholds] duplicate_min_tokens`** (default 50): a function counts as an
  exact duplicate only when it has a body and at least that many normalized tokens.

### Changed

- **Metric semantics v7: declarative code stops counting.** Extraction records whether a
  callable has a body (migration 25). Cohesion (LCOM4) and Focus count only bodied
  methods; bodyless declarations never count as duplicates; the Uniqueness list ranks
  clone groups by mass (members × mean lines). The first `gate` after upgrading re-baselines
  once (`baseline reset: metric semantics changed`). Signals rise on declarative code: on a
  real 84-member Spring estate, no member's signal fell.
- **Upgrade:** opening a store applies migrations 25 and 26, and the next `logos scan` (or
  `logos index`) re-extracts every file once. Until then callables count as bodied. A bare
  `logos sync` re-reads no file. Earlier `logos` versions refuse an upgraded store.

## [1.8.3] — 2026-10-02

### Changed

- **The workspace chat checks its read roots before a turn starts.** The workspace
  tier's `[chat] read_roots` and every member's effective ones are checked when a
  `POST /workspace/chat` turn starts — config reads only, no member engine is started.
  A bad read-root entry fails the turn up front, by name (the declaring root, the entry
  and why), before anything is recorded, as the member chat already does. A member whose
  `[chat]` cannot be read, or whose root is gone, is skipped by that check and keeps its
  fault on its own addressed source calls.

## [1.8.2] — 2026-10-02

### Added

- **The workspace chat is its own service, route and store** (`agents` builds, workspace
  serve only). `POST /workspace/chat` runs a turn with the same intent guard, host guard,
  consent disclosure and SSE/buffered contract as `POST /chat`; its conversations are at
  `GET /api/v1/workspace/chat/threads[/{id}]` and `POST …/{id}/delete`. It ignores `?repo=`
  and answers `404` in a single-root or `--standalone` serve. Its policy and credential come
  from the workspace tier alone — a member's own `[chat]` does not configure it, and the
  configure-first state names the workspace root and the missing half. Turns are stored in
  `<workspace root>/.logos/chat.db`, which the generated workspace-root ignore entry now
  covers; no member's `chat.db` is created or changed. There is no in-app view yet.
- **A workspace-centred chat roster.** The workspace chat's planner and Synthesizer are told
  the workspace's members and their kinds; a Workspace-Analyst holds the workspace
  read-model tools (`workspace_status`, `workspace_reachability`, `workspace_check`,
  `xservice_build_deps`, `workspace_roster`) and the four `xservice_*` tools under its own
  cap; Graph-Navigator, Governance-Analyst and Source-Reader take a required `repo` and open
  only the member they address, reading through that member's own sandbox and its
  *effective* `[chat] read_roots`. The Synthesizer ranks members by their own named signals
  and never states a composite workspace score.
- **Follow-up chat turns see prior turns.** The planner and the Synthesizer of both chats
  receive a bounded window of the thread's earlier turns, oldest first, bounded by two new
  `[chat]` keys: `history_max_turns` (default 6, in `[1, 50]`) and `history_max_chars`
  (default 16000, in `[1, 200000]`). When turns are dropped the prompt says how many. The
  first turn of a thread renders exactly as before.
- **Cross-service answers name the members they read.** `xservice callers`, `impact` and
  `route-providers` — CLI `--json`, MCP and `/api/v1/workspace/*` — carry a `member_reads`
  field: `read`, and `unread` with each member's reason.

### Changed

- **The member chat in a workspace serve is single-backing.** `/chat?repo=<member>` no
  longer carries the `xservice_*` tools or the federated addenda; cross-service questions
  belong to the workspace chat.
- **A warm cross-service bridge reads only the members a query needs** instead of every
  member's sync stamp on each query; results are unchanged.

## [1.8.1] — 2026-10-01

### Changed

- **The minified-JavaScript exclusion notice is an advisory note, not a warning.** The
  "`<N>` minified JavaScript file(s) excluded from indexing by the `**/*.min.js` exclude
  glob" line moved from `warnings` to the advisory `notes` channel that `index` already
  uses. It is a permanent, benign statement about a default working as intended, yet it
  repeated on every `index` and every reconcile-backed readout and could trip a CI parser
  that scans `warnings`. It now appears under `notes` on `index` (human and `--json`) and
  on `scan`, `check`, `gate`, `dsm`, `doc_gaps` and the config-apply result, and no longer
  under `warnings`; each read-model's `notes` key is omitted when empty, so output with
  nothing to note is byte-identical to before. The oversize-file notice stays a warning.

### Fixed

- **`logos sync --help` no longer claims `sync` defaults to all changed files.** With no
  path `sync` re-reads no file, as `docs/howto/commands.md` says; the help now says the
  same and points at `logos scan` / `logos index` for folding in every change. Help text
  only — `sync`'s behaviour is unchanged.

## [1.8.0] — 2026-10-01

### Added

- **Members record the types they declare.** An index records every top-level Java/Kotlin
  class, interface, enum and record under its package-aware fully-qualified name, with its
  node's symbol and whether it sits in the main or the test tree, and every Avro `.avsc`
  record and enum (nested named types included) under its namespace, with the schema's path.
  A source file whose `package` statement disagrees with its directory is recorded refused
  with both named; a malformed schema is recorded with its reason and yields no name. Store
  migration 24 (two new tables); a store upgraded from 1.7.0 fills them on its first full
  reconcile, which re-extracts the member's Java/Kotlin files once. A member with no
  Java/Kotlin/Avro file gains only the empty tables and one marker row.
- **An import of another member's type binds, as an advisory type reference.** In a
  workspace, a still-unresolved Java/Kotlin import naming a type exactly one other member
  declares (main-tree source or an Avro schema) is matched to it, with the importing file and
  line and the declaring file or schema. It is admitted only where the build relation relates
  the two members, or the importer references a colliding artifact the owner produces (the
  artifact named);
  every other match is listed `type-only`, and a type several members declare stays unbound
  as `ambiguous-owner`, owners named. `workspace status` gains a `type_reference` section:
  `type_reference_pairs` beside every row considered, by bucket, and the members read. It is
  never a bridge edge, a coupling or a gate input: the coverage figures, the build headline
  and every member's `scan`/`gate` are unchanged. A workspace with no Java/Kotlin/Avro member
  shows no new section.
- **`logos xservice type-refs [--repo]` lists the type references, and `callers`/`impact`
  follow them.** Per provider member, `type-refs` lists the types other members import, each
  with its declaration and every importer's file and line, under the `type_reference`
  headline beside its denominators; `--repo` scopes the listing to one provider, and a name
  that is not a member read answers a `scope_note` instead of an error. The MCP twin is
  `xservice_type_refs`. `xservice callers` and `xservice impact` on a type — its node, or its
  dotted name, the only handle an Avro type has — add a `via_type_reference` section, apart
  from the bridge-reached results and never merged with them: each entry is tagged
  `via type reference` with the reference it was reached through, and for `impact` carries the
  files depending on the importing file in its member, or that member's error when it will not
  open. A member whose declared types could not be read is named, with its reason, in
  `type_reference_unread`. A symbol no type reference names answers exactly as before. Advisory, never a
  coupling. As with every `xservice_*` tool, `logos serve --mcp` does not yet route the twin:
  it serves the single-root engine even inside a workspace.

### Changed

- **Kotlin files are keyed by their package, as Java files are.** The Kotlin plugin now
  declares `[package_modules]` under `src/main/kotlin` and `src/test/kotlin`, so a Kotlin
  file there is `com.x.Svc` rather than `main::kotlin::com::x::Svc`: Kotlin imports of
  in-repository types can bind, and a type declared under one name in both trees stays
  unbound. Kotlin projects will see more resolved imports after their first full reconcile or
  re-index, and `status` reports the Kotlin row's `call_residue` as it does Java's. Kotlin type
  relations (`Extends`/`Implements`/`TypeUses`) are still not captured.
- **TypeScript and TSX class fields are `Field` nodes, so `this.x` binds.** A declared field, a
  `#private` field and a constructor parameter property (`private readonly http`) of a
  `class` declaration are now captured as `Field`, and a `this.#x` read is an own-field access like
  `this.x`. The exactly-one `Accesses` binding finds them, so LCOM4 sees the fields methods share.
  A parameter property is owned by the class, not by its constructor. A getter, a method, an
  inherited member and an absent name still stay unresolved, and so does a field of an abstract
  class or a class expression (neither is a captured class, so such a field is not created at
  all). A field initialised with an arrow function is a field, and refs inside an initialiser are
  attributed to it. On the reference estate's TypeScript, first-party own-field accesses bound
  0 → 334 of 507; vendored JavaScript stays 0 of 6,805 (no class fields there). A new field can
  make a bare name ambiguous where it used to bind by coincidence: it then stays unresolved.
- **Minified JavaScript is no longer indexed by default.** The default code `exclude` gains
  `**/*.min.js` (any depth), so minified third-party bundles stop contributing nodes and
  unresolved references; `index` reports how many such files it skipped. An existing graph
  purges them on the next `index` or reconcile, because the admission fingerprint changes. A
  `config.toml` that sets its own `exclude` replaces the default and re-admits them.

## [1.7.0] — 2026-09-30

### Added

- **A vendored spec is a declared contract.** An OpenAPI document a member holds but does not
  implement declares `declares-contract(A → C)`: to the member whose own spec holds ≥ 90 % of its
  operations (document identity, score and matched document named), or else to a **named
  external** grouped across copies. Names are not unique, so an external is identified by the
  `member:path` of its first copy. A `kind = "mock"` member is a stand-in provider; a
  `documentation` member's copies stay out. Built in memory on the first cross-service query; no
  migration. Published as `coverage.declared_contracts`, headline `declared_contract_pairs` over
  every spec document read. On the reference estate: 6 pairs (1 by identity, 5 to named
  externals) from 7 vendored of 41 spec documents.
- **A no-provider call binds to the external its member vendors.** A `no-provider-in-workspace`
  REST call whose path, under a base path its member's committed sources prove (application
  configuration, or agreeing committed deploy overlays), equals exactly one operation of an
  external that member declares is reported in `coverage.bound_external`, with the file, key and
  matched operation. Every other case is refused with its reason. On the reference estate: 21 of
  32 rows bound.
- **Declared contracts and named externals on every surface.** `workspace status`,
  `xservice route-providers` (CLI `--json` and MCP; `--repo` adds `declared_scope_note`) and the
  web service map — a dotted declared-contract edge class with its legend section, named-external
  nodes, and a declared-relations card on the Coverage tab. A workspace with no vendored spec and
  no `kind` renders every surface unchanged.

### Changed

- **Modularity drops out of a graph too small to have community structure.** Below 5
  module-rollup edges Modularity is *not applicable*: its computed pair is kept, it leaves both
  the geometric mean and the zero short-circuit, and `quality-report --json`, `scan --json` and
  the dashboard say why (`n of 5`). Metric semantics 5 → 6, so every project re-baselines once
  (`baseline reset: metric semantics changed`). Graphs with 5 or more edges score exactly as
  before; on the reference estate 7 small members rise (e.g. `notification-kafka-models`
  0 → 9725) and all 63 others are byte-identical.
- **Forward-only store migration 23** (`metric_snapshots.modularity_applicable`). Once 1.7.0 opens
  a store, 1.6.x can no longer open it.
- **Declared figures stand beside the runtime ones, never inside.** `resolved_cross_service_edges`,
  `egress_resolution`, `no_provider_in_workspace` and the bridge edge set are unchanged by either
  new relation. `workspace status --json` grows about 3 % on the reference estate and
  `route-providers --json` about 29 %.

## [1.6.0] — 2026-09-29

### Added

- **A plugin can declare a package-shaped module path.** A `[package_modules]` table in
  `plugin.toml` lists `source_roots` (for example `src/main/java`). A file under one is keyed by
  the path after the root, so `src/main/java/com/x/Svc.java` is `com.x.Svc`, the name its
  imports spell. The table is off by default; Java opts in, and every other language keeps its
  module keys.
- **Java type relations are edges.** A class's superclass and an interface's super-interfaces
  bind as `Extends`, a class's interfaces as `Implements`, `new T(…)` as `Instantiates`, and a
  declared field, parameter, local or return type (type arguments included) as `TypeUses`. Each
  binds only to the one in-repository type of the right kind; JDK, library, generated and
  ambiguous types stay in `unresolved_refs`. The four kinds are kept in the full symbol graph
  and fenced out of the dependency views the metrics run on, so they do not change the signal
  by themselves.

### Changed

- **Java imports bind to the type they name.** A single-type import binds to the class and a
  static import to the member; a wildcard import brings its package's (or type's) members into
  scope; and a reference to a type in the file's own package binds without an import. A type declared under one name in both `src/main` and
  `src/test` stays unbound. On the reference estate Java imports went from 18 to 4,628 bound
  of 25,132; the rest are external, generated or in another member.
- **Spring routes built from string constants are promoted, or counted.** A mapping path
  written as `"/users/{" + USER_ID + "}"`, with `USER_ID` a `static final String` of the same
  type, of an interface, or of a type in the same member reached through `import static` or
  `Type.NAME`, folds to one path and becomes a `route` node like a written one. A path that
  cannot be folded (a method call, a non-final field, a wildcard static import, a constant in
  another member or a library) promotes no route and is counted in `routes_not_composed`
  instead of being dropped silently. Kotlin non-literal paths are not counted yet. On the
  reference estate all 16 concatenated sites are promoted, and `resolved_cross_service_edges`
  rose from 65 to 70.
- **A typed Java call binds through its receiver's class and that class's superclasses.**
  `service.send()` on a receiver the file declares binds to that class's `send()`. If the
  class does not declare it, the call binds to the one `send()` of its nearest superclass in
  the repository. `super.m()` and an inherited `m()` bind the same way. None of these binds:
  a method overloaded on one class, a JDK or library type, a type another member declares,
  or a superclass outside the repository. The nearest class that declares the name decides,
  without reading arity or visibility. An overload split across a class and its superclass
  therefore binds the nearer one. A nested type a class inherits does not yet hide a
  same-package type of the same name, so a receiver typed with that name binds the
  same-package type's method.
- **`status` states why the rest stays unbound.** The Java row of `resolution_by_language`
  carries `call_residue`: the unbound calls, and how many stay unbound for each reason
  (`no-receiver-evidence`, `external-type`, `type-in-another-member`, `overload-ambiguous`,
  `type-ambiguous`, `supertype-unreached`). Outside a workspace, another member's type counts
  as `external-type`.
- **Java figures move on the first re-index, and the move is a correction.** A Java method
  that was called only through a typed or inherited call gains its first inbound `Calls` edge,
  so:
  - fan-in and fan-out rise;
  - `callers`, `callees` and `impact` answers grow;
  - class coupling rises;
  - methods reported dead only because their callers never bound stop being dead;
  - the signal score of a Java repository moves with all of these.

  Some figures fall instead. A typed call used to fall through to a same-named method in lexical
  scope, often the calling method itself: `delegate.write()` inside `write()` bound to that
  `write()`. Those edges were fabrications and are gone (355 on the reference estate: 324
  self-edges, 31 to a sibling method). A method "called" only through one loses fan-in, drops
  out of `callers` and `impact` answers, and can newly be reported dead.

  Rust, and every other language, is unchanged: this repository's own index is byte-identical.
- **Known limitation: a sync can leave a stale Java call edge until the next full `index`.**
  `logos sync` re-binds the calls a hierarchy edit moves, but it does not remove an edge the
  call bound before. This happens when a class in the middle of a hierarchy gains an override
  (the call then has an edge to both methods), drops its `extends`, or is deleted. A full
  `logos index` has no stale edge. The same holds for a method that gains an overload.

## [1.5.0] — 2026-09-28

### Added

- **Members that build against each other.** Indexing reads each member's Maven `pom.xml` and
  Gradle `build.gradle` / `build.gradle.kts` into member-local facts: the artifacts it produces
  and the ones it references, by kind (`parent`, `dependency`, `managed`, `bom-import`) and
  scope. An unresolved `${…}` coordinate is refused with a reason, never guessed; Gradle
  support is read but reported unexercised.
- **A `builds-against` relation across workspace members**, with its own headline
  `build_dependency_pairs` split by kind beside its denominator. An artifact produced by two
  members resolves to neither, and the collision is reported. A build dependency is never a
  runtime coupling: it never enters `resolved_cross_service_edges`, egress resolution, a
  provider bucket or a bridge edge.
- **`logos xservice build-deps`** (`--json`, `--repo <member>`) and its MCP twin list what each
  member builds against and what builds against it. `workspace status` gains a
  `build_dependency` section. The web service map gains a `build` edge layer behind a legend
  toggle that is off by default, with platform members collapsed, and a cross-context model
  hint that is never drawn as an edge.
- **A member can declare its kind** in `logos.workspace.toml`: `kind = "documentation"`,
  `"mock"` or `"platform"`. Documentation and mock members leave the contract-surface headline
  and `spec_conformance_ratio`, and their rows are reported apart with their count. A platform
  member's inbound build edges are counted apart. `workspace status` lists candidates for each
  and never classifies a member itself.

### Changed

- **Graph store schema version 22** (forward-only) adds the build-manifest facts. After
  upgrading, each member with build manifests reads *unread — build facts not yet extracted*
  until `logos index` or `logos health` runs in it; it is never reported as having no
  manifests. An older binary cannot open a store at version 22.

## [1.4.27] — 2026-09-27

### Added

- **A project README** with install, quick-start and configuration guides.
- **Homebrew install:** `brew install raffaelecamanzo/tap/logos`. The release pipeline now
  publishes the formula to the `raffaelecamanzo/homebrew-tap` tap on every tagged release.

### Changed

- **The Logos mark and wordmark in the web header are 1.4 times larger** (mark 28px →
  39px, wordmark `--text-lg` × 1.4).

## [1.4.26] — 2026-09-27

Sprint 79, hotfix round 2.

### Changed

- **The web header shows the Logos mark and wordmark only.** The "code intelligence"
  subtitle after the wordmark is removed.

## [1.4.25] — 2026-09-27

Sprint 79, hotfix round 1.

### Added

- **The chat can read documentation kept outside the project.** A new optional
  `[chat] read_roots` key (for example `read_roots = ["../logos-docs"]`) lets the
  Source-Reader read files reached through symlinks inside the project that point
  into the listed folders. `grep` and `glob` follow only those links. It is empty by
  default, so nothing changes unless a project opts in. The consent banner and the
  CHAT band name the extra folders, and changing them asks for consent again. A
  listed folder that does not exist fails the turn with an error naming it
  (NFR-SE-04 amended).

### Fixed

- **Mermaid shapes are no longer drawn solid black.** The strict content-security
  policy blocked Mermaid's own styles, so cylinders, class boxes and most
  sequence-diagram parts fell back to black, and self-calls drew as filled blobs.
  Those styles are now applied in a way the policy allows, without loosening it, in
  both the Chat and Wiki tabs.
- **Sequence diagrams with a `;` in a message or note render.** Mermaid treats `;` as
  the end of a statement and rejected the whole diagram, so the chat showed its
  source. The text is repaired to Mermaid's `#59;` escape before rendering; the
  Source view still shows exactly what the model wrote, and the chat's answer writer
  is told to avoid a bare `;` in diagram text.

## [1.4.24] — 2026-09-27

Sprint 79.

### Changed

- **The Chat tab uses the application's two-pane layout.** The conversation now sits
  inside the shared card with its red top edge, beside a history rail that runs the full
  height of the view, on the same rail track and column gap as the Wiki tab. The centred
  reading column that left a 172px gap beside the rail at 1600px (652px at 2560px) is gone.
  A full-size, iconed **+ New chat** stays pinned above the scrolling conversation list
  (CR-092).

### Added

- **A persistent CHAT status band.** After you acknowledge the consent banner, the band
  takes its place at the top of the view. It names the provider, endpoint host and model,
  and the turn's budget tree (tool calls, per-subagent cap, replans), and stays visible
  after the first message. A value you have not configured is shown as not configured,
  and the key never appears (CR-092, FR-UI-33).
- **Chat answer tables render as bordered blocks.** A table in an answer or an Activity
  step result uses the code block's border and radius, with an uppercase header over a
  stronger underline, hairline row rules and a row hover tint. A table wider than the
  column scrolls sideways inside its block instead of breaking cells mid-word (CR-094).

### Fixed

- **The Statistics "By surface" caption no longer claims dashboard activity is
  excluded.** It states the rule the figures apply: the tab's own `stats` request and the
  shell's `status` readout are excluded per event, and a graph query issued through the
  dashboard counts (CR-146).

## [1.4.23] — 2026-09-26

Sprint 78.

### Added

- **Telemetry records what a call answered, not only that it ran.** `callers`, `impact`,
  `precedent` and `affected` now record an outcome (`answered`, `empty`, `unresolved` or
  `failed`), and every usage cell in `logos stats --json` carries `answered_calls` and
  `classified_calls` beside `calls`. The payload computes no rate: divide `answered_calls` by
  `classified_calls`, never by `calls`. A cell with no classified call carries
  `outcome_absence: "none recorded"`. The workspace aggregate carries the same fields. The
  store migrates to schema v4 on first use; older events stay unclassified and are never
  back-filled, and 1.4.22 still reads a v4 store (CR-144, FR-OB-14).
- **Statistics tab: tool attribution by class.** A new card shows per-tool usage split by
  dev/`main` origin, grouped by tool class, with each cell's `N of M answered` or
  `none recorded`. The coverage limits (raw events only, retention, pre-origin-stamp data)
  are printed beside the figures (CR-091).
- **A census fails the build if a traced tool is neither classified nor excluded with a
  reason**, so a new tool cannot silently shrink the answered denominator.

### Fixed

- **The Wiki progress banner never overshoots.** When an auto-continued run surfaces work it
  did not start with, the denominator grows to the real scope, never decreases, and the
  banner says so (`scope grew from 5 as new work surfaced during the run`), instead of
  reporting `7/5` (CR-093).
- **The chat Activity fold is correct on replanned turns.** A later round's observation no
  longer marks an earlier round's step done, the fold groups each round's plan with its own
  steps, and a replan to zero steps keeps the earlier round visible (CR-090).
- **The Statistics notes no longer claim pre-origin-stamp data is CLI+MCP-only.** It came
  from every surface running at the time, including web and the watcher.

## [1.4.22] — 2026-09-25

Sprint 77, second hotfix round.

### Performance

- **A workspace's first load is about 5× faster.** Compiled tree-sitter queries are now shared
  across member engines in one process instead of being recompiled at every member start. On
  the 84-member reference workspace, the Workspace Dashboard's first load after `logos serve`
  starts dropped from ~46 s to ~9.4 s, with identical answers. A per-root
  `.logos/plugins/<language>/` override still compiles its own copy (ADR-04 amended).

## [1.4.21] — 2026-09-25

Sprint 77, hotfix round.

### Changed

- **A workspace `[wiki].model` is inherited.** Wiki generation resolves its model in this
  order: the member's own `[wiki].model`; else the workspace root's `[wiki].model`, but only
  while the member inherits the workspace `[chat]` policy; else the effective chat model. A
  member that declares its own `[chat] model` owns its endpoint and never receives the
  workspace wiki model. The Wiki tab and the workspace Config view state the same rule, and
  the config read-model carries it as `effective_wiki` (CR-145, ADR-67, FR-CF-07).
- **The manifest routes emit telemetry.** `GET /api/v1/workspace/manifest` and
  `POST /api/v1/workspace/manifest/save` book `config_read` / `config_write` events into the
  serve's telemetry store, like the workspace config routes.
- **A broken workspace config file is one click from its repair.** In workspace mode the
  member Chat and Config tabs' error state links the workspace Config view and names both
  roots the failure can come from.

Sprint 77, hotfix round.

### Changed

- **A workspace `[wiki].model` is inherited — by a member that inherits the workspace
  `[chat]` table.** Wiki generation now takes the member's own `[wiki].model`, else the
  workspace root's when the member declares no `[chat] model` and the workspace root
  declares one, else the effective chat model. A member that declares its own
  `[chat] model` owns its endpoint, where a model named for the workspace endpoint may not
  exist, so it never receives the workspace wiki model. Provider, endpoint and key are
  still the effective chat resolution's. This replaces 1.4.20's "documented as not read":
  the workspace Config view now states the condition instead of *Not inherited*, and the
  config read-model (`GET /api/v1/config`) carries an `effective_wiki` slice beside
  `effective_chat`, which the Wiki tab's readiness and consent disclosure read, so the tab
  names the model the run uses (CR-145, FR-CF-07, ADR-67).
- **The workspace manifest routes are counted in `logos stats`.** `GET` and
  `POST /api/v1/workspace/manifest[/save]` each book one `config_read` / `config_write`
  event, as the workspace config routes already did (FR-UI-38).

## [1.4.20] — 2026-09-25

Sprint 77: the workspace is configured from the app, and the chat answers cross-service
questions.

### Added

- **The workspace Config view.** In workspace mode the sidebar's Workspace section
  gains **Config** (`/workspace-config`), which edits `logos.workspace.toml` in the
  same typed-fields + raw-TOML grammar as the member Config tab: `[workspace]`,
  `[workspace.warm]` and the full `[governance]` family, shown beside the advisory
  findings. It says at the point of editing that workspace governance never moves a
  member's gated signal. A save is validated before anything is written. A document
  identical to disk writes nothing, and fields the manifest does not declare are never
  added. A manifest changed on disk since the view loaded it is **not overwritten
  silently**: the view shows the disk copy and lets you load it or overwrite it, and
  says which happened. New routes: `GET /api/v1/workspace/manifest` and
  `POST /api/v1/workspace/manifest/save` (CR-137, FR-UI-38).
- **Workspace chat settings in the same view.** A second group edits the workspace
  `[chat]` policy and the masked, write-only chat key at the workspace root, names both
  files, and states how members inherit each half (CR-145).
- **Cross-service tools in the chat.** In a workspace the chat's Graph-Navigator can call
  `xservice_route_providers`, `xservice_callers`, `xservice_impact` and
  `xservice_search`. Answers are qualified by member, and an empty answer over an
  unresolved residue is reported as unresolved, never as "none". A plain single repo's
  tool list is unchanged (CR-137, FR-WS-29).

### Changed

- **The workspace chat tier's read is the repair path, and its save is clobber-safe.**
  `GET /api/v1/workspace/config` now returns the literal `config.toml`, a load
  fingerprint and `parsed: null` when the file is broken (a `200`, not a `500`). A fault
  in either file is named by file and line/column only, and nothing from
  `secrets.toml` is ever echoed. `POST /api/v1/workspace/config/save` requires that
  fingerprint (`400` without it) and answers `409` with the disk copy when the file
  changed since the read.
- **The Chat tab's configure-first state links the workspace Config view** for the
  workspace half, instead of naming the file as text.
- **A `[wiki].model` at the workspace root is documented as not read** by any member;
  the workspace Config view labels it *Not inherited*.

## [1.4.19] — 2026-09-24

Sprint 76, hotfix round.

### Changed

- **A member's own API key is never sent to a workspace endpoint.** When a
  member inherits the workspace `[chat]` policy (it declares no `model`), the key
  now comes from the workspace root or not at all; the member's own key is
  withheld (`effective_chat.member_key_withheld`). The Chat tab, the member Config
  tab and a refused chat request say so, and setting a `[chat] model` on the
  member makes it use its own key. A workspace key still reaches an endpoint a
  member declares itself (CR-145, FR-WS-30, ADR-67).


## [1.4.18] — 2026-09-24

Sprint 76 — the chat works in a workspace, and its not-ready state says why.

### Added

- **Workspace-level chat configuration.** Declare `[chat]` (and `[wiki].model`)
  in `<workspace-root>/.logos/config.toml` and the chat API key in
  `<workspace-root>/.logos/secrets.toml` once, and every member that declares
  none inherits them. Inheritance is **per half** — the policy table and the key
  resolve independently — and the member wins wherever it declares; the policy
  half is atomic on `model`, so a member that sets its own model uses its own
  table entire. The Chat tab, the chat request path and wiki generation all read
  one resolution, so they cannot disagree (CR-145, FR-WS-30).
- **`GET /api/v1/workspace/config`, `POST /api/v1/workspace/config/save`,
  `POST /api/v1/workspace/config/secret`** — read and write the workspace tier
  with the existing validated, atomic, `0600` writers. Intent-guarded, `404`
  under a single-root server, no apply/reindex action, and no engine is started
  at the workspace root.
- The config read-model (`GET /api/v1/config`) carries an `effective_chat` slice
  — the resolved policy, the masked key and the origin of each half (`member`,
  `workspace`, `unset`) — **beside** the member's literal document, which is
  unchanged byte for byte. The member Config tab shows inherited values as a
  read-only note and never writes them back into the member's `config.toml`.

### Fixed

- **The Chat tab's not-ready state now says what it checked**: the root it
  inspected (the member by name in a workspace, "this repository" otherwise),
  which half is missing (model, key or both), and where any present half came
  from. Previously it said only that a provider was needed — on the reference
  workspace it was reporting on a member other than the one being configured. A
  refused chat request names the same facts.
- The Wiki tab's readiness and consent disclosure read the effective resolution,
  so a member inheriting the workspace key is offered generation instead of a
  configure-first state.
- The sidebar Service header is now a column, so its label no longer truncates
  to `Se…` beside a long member name; the Workspace section renders as one list.


## [1.4.17] — 2026-09-23

### Fixed

- **The cross-file call relation is no longer Rust-only.** A module specifier is
  now canonicalised as a *path* rather than through the member-path separator, so
  TypeScript, TSX, JavaScript and Go stop producing zero `Imports` edges, and the
  **imported** rung of the scope hierarchy now binds a call through an imported
  binding. `logos callers navItemsFor` returned a confident `total: 0` for a
  function called at `web/ui/src/shell/Sidebar.tsx:172`; it now returns its real
  callers. Capitalised JSX elements (`<RuleFindingsCard />`) are captured as
  calls, without admitting `<div>` or `<Nav.Item>`. Rust resolution is unchanged,
  asserted by hashing the sorted edge set rather than by comparing counts
  (CR-142).

### Added

- **`status` reports resolution coverage per language, with its denominator**
  (`resolution_by_language`). A language that binds nothing across a file
  boundary renders as a named state — `same-file-only`, `no-resolved-edges` or
  `no-references-recorded` — never as a bare `0` that would read as a
  measurement. A single global coverage figure cannot express a per-language
  zero, which is how the defect above survived (FR-RS-09).
- **Every relational answer states the resolution denominator it was computed
  over** (`resolution_denominator`), on `callers`, `callees`, `impact`,
  `impact-intersection`, `branch-overlap`, `affected` and `precedent` — present
  on empty *and* non-empty answers, as a typed field rather than prose. This
  separates *nothing depends on this* from *nothing could be resolved here*
  (FR-NV-14, CR-143).
- **A structural arm to the absence audit**: the relational result types are
  enumerated from their definitions, so a new answer type that omits the
  denominator fails the suite rather than shipping silently.
- The shipped guidance in the managed `CLAUDE.md` block, the MCP server
  instructions and `docs/howto/README.md` now state the language scope on which
  their relational claims hold, keyed on the denominator rather than on a
  hardcoded language list.

### Upgrade note

Existing stores need a full `logos index`, not a `sync` — a sync does not
re-extract unchanged files, so the new `Imports`/`Calls` edges would not appear.

## [1.4.16] — 2026-09-21

Sprint 74 — the workspace stops being one tab and becomes a scope the
application declares.

### Added

- **Every navigation entry declares the scope it answers for.** `scope`
  (`app` | `member`) is now a required field, replacing a path-prefix test over a
  hard-coded one-item list that let the member selector silently govern eleven
  views and not the twelfth. The sidebar renders a **Workspace** section and a
  **Service** section whose header carries the member selector, so the control
  sits inside the boundary it governs. Single-root rendering is unchanged.
- **Two shipped capabilities gain the HTTP surface they never had.**
  `GET /api/v1/workspace/reachability` and `GET /api/v1/workspace/check`
  serialise the existing `federation::reach` and `federation::governance`
  read-models — reachable from the CLI alone since they shipped. A CLI/HTTP
  field-for-field agreement test pins them to one read-model rather than to two
  fixtures that could drift.
- **The selected member is in the URL.** `?repo=<member>` scopes the SPA on first
  paint with no unscoped pre-pass, switching writes through history, and a
  `?repo=` naming a member the workspace does not have renders a state naming the
  members it does have — while **no** view renders any member's figures. The
  normalisation is one table compiled into both the server and the client, so the
  two spellings cannot drift.
- **Workspace Dashboard and Workspace Health**, two `app`-scoped views. Every
  ratio carries its denominator and its exclusion; a zero denominator renders the
  ratio absent rather than as a `0%` that reads like a measurement. No
  workspace-level score is invented and no per-member signal is aggregated.
- **Workspace Statistics**, summed over members and read **engine-free** — the
  view load leaves the resident-engine count unchanged. It states its member
  denominator and names every member whose telemetry could not be read, reserving
  "a lower bound" for members that *failed* a read and never for one that simply
  has no store yet. It omits `artifact_bindings` and latency percentiles by
  construction: both are `Engine`-bound, and the ceiling is the point.

### Fixed

- **The Dashboard Rule findings card no longer passes a vacuous check.** A
  contract authoring zero rules now renders the onboarding empty state instead of
  a green `PASS` badge over a check that evaluated nothing — the fourth and last
  surface still doing so. Violations still win: a finding raised by an always-on
  fold-in renders red even on a zero-rule contract.

## [1.4.15] — 2026-09-21

Sprint 73 — every readout carries the denominator that makes it checkable, and
the absence vocabulary is stated once.

### Fixed

- **The session-start readout no longer calls a vacuous run a clean check.** The
  `check_run` marker recorded a violation count and nothing about what produced
  it, so a run that evaluated **zero rules** — no contract (exit `4`), or `logos
  init`'s default contract authoring none (exit `0`, the ordinary state of a
  fresh project) — rendered as ``clean `logos check` ``. Migration 21 widens the
  marker with the evaluated set (`checked_rules`, `rules_present`) and the
  operation that wrote it; the readout now names four distinct states, states a
  clean result only beside its denominator (`0 of 12 rule(s) evaluated — clean`),
  renders a pre-migration marker as *evaluated set unknown*, and names no
  command. Additive, in-place, no re-index (S-437, CR-140).
- **`quality-report` no longer reports a populated graph as empty.** A store
  whose every function is a test computes a legitimately empty *production*
  scope; it was reported as `signal n/a (empty graph)` while `logos status` said
  `indexed: true` in the same breath. The readout now distinguishes the two and
  carries the figure that establishes it (S-432, CR-138).
- **The dashboard no longer renders an age it cannot establish.** A timestamp
  ahead of now was clamped and shown as `just now`; since `status.last_sync_at`
  is a file mtime, that is routine on copied trees and restored backups. Both
  degradations now use the CLI readout's own wording (S-433, CR-138).
- **The Health page no longer labels a superseded snapshot `current`.** A
  snapshot persisted before the graph was last re-indexed now drops the word and
  carries its own date. Detects disagreement, not currency — it over-reports by
  design, and an indeterminate ordering renders neither (S-436, CR-135 §3.2).
- **Logos's own wiki generation is no longer counted as a developer browsing the
  dashboard.** `POST /wiki/generate` inherited the `serve --ui` process surface;
  it now has a distinct override-only `wikigen` telemetry surface, separable from
  `web`, `mcp` and `chat` (S-435, CR-139, FR-OB-13).

### Changed

- **One absence taxonomy across the CLI, governance and SPA surfaces**, stated
  once in code and referenced rather than restated. Audited from source: 3
  non-conformant occurrences of 68, in 1 file, at 2026-09-20 — the other 65 were
  already conformant (S-434, CR-138).
- `logos stats`'s surface vocabulary is documented in full: four process
  surfaces (`cli`, `mcp`, `web`, `chat`) and three override-only ones
  (`watcher`, `shell`, `wikigen`).

## [1.4.14] — 2026-09-20

### Added
- **The governance readout dates its violations and can state a recorded clean check**
  (S-314, CR-096, FR-IN-07). `quality-report` reports the check run's age and `HEAD`, says
  when a finding was measured against a *different* tree, and states a clean check only
  from the recorded marker — absent a marker it says **no check has run**, never
  "0 violations". Two unusable clock readings degrade explicitly rather than being
  smoothed into "just now".
- **The Health page states staleness instead of implying currency** (S-422, CR-135,
  FR-UI-04). Its gate verdict and quality grid are built from **one** read of the last
  persisted snapshot, so a concurrent `scan` can no longer render two cards describing
  different generations. With a signal present over a graph that is no longer indexed,
  both cards render their figures **labelled**, dated and naming `logos index`.
- **The app header survives a narrow viewport** (S-317, CR-097, FR-UI-34). `Header.module.css`
  gains its first media queries and a progressive-disclosure order: the graph-state readout
  is dropped **whole** before the brand subtitle, never truncated, and the brand lockup,
  member selector and theme toggle survive to the narrowest supported width.

### Changed
- **The LOC roll-up distinguishes three cases instead of writing unconditionally**
  (S-421, CR-134, FR-IX-12). Write on persist, clear over an empty store, and **leave
  untouched while a previous graph survives** — so an index in which every candidate fails
  to load no longer reports `total_line_count: 0` beside a correct file count.
- **The shell's own status read is classified as non-usage telemetry** (S-316, CR-097,
  FR-OB-09). The app header's `status` request is registered as a self-referential
  read-model request at the adapter boundary, so rendering chrome stops counting as
  someone using the tool. `surface` is now required on both `spawn_blocking` hops, making
  an unclassified handler a build failure.
- **The promoted topic inventory keys on the committed configured value** (S-424, CR-136,
  FR-WS-27), closing the last tier of the ADR-52 one-classifier drift: one identify
  function is now called from both the intra-repo promotion pass and the federation
  bridge, so a publisher and a subscriber that spell one property differently meet on one
  topic and the coupling draws as a `publisher → topic → subscriber` hop.
- **The rust client-call candidacy gate is receiver-grained** (S-423, CR-128, FR-WS-08),
  following Java's and Go's rule shape: a verb-named call on a non-client receiver no
  longer promotes a reference merely because its file imports a client crate. Six of the
  ten arms declaring `http_client_detectors` now scope their receivers.

### Fixed
- **Reading the Health page could have destroyed the last recorded `check` result.** The
  write-free contract was guarded per-path — S-314's path on the violations table and
  marker, S-422's on `metric_snapshots` — and neither covered the union, which is the
  actual contract. Found at sprint review by restoring the defect: an inserted write left
  the suite green while every page load would have truncated `violations` and stamped a
  fabricated recorded-clean marker over it.


### Changed — BREAKING (exit code)
- **`logos workspace status` / `reachability` / `check` now exit 1 when a member
  could not be opened (CR-100, S-326, FR-WS-16, FR-CL-01).** These commands
  previously returned **0** no matter how many members failed to open: on a
  72-member workspace under the stock 256-descriptor macOS limit, 63 members
  failed and the command still succeeded, so the run passed in CI over a payload
  that was three-quarters missing. An unopenable member now names itself and
  moves the exit code.

  **Scripts that tolerated the old exit 0 will begin failing** — which is the
  point, but it is a behaviour change to plan for. What does *not* move the exit
  code: a member with no index yet (`warm_state: deferred` — it indexes lazily on
  first query), a member the command never needed to open (`open_state:
  not-attempted`), and a member whose engine was **evicted** to stay inside the
  workspace connection budget (`open_state: opened` — eviction reclaims a
  success, not a failure). A workspace-governance violation still never moves the
  exit code either; `workspace check` remains advisory.

### Changed — BREAKING (payload)
- **`bound_ratio` is retired from the coverage payload; the headline is
  `resolved_cross_service_edges` with `egress_resolution` beside it (CR-120,
  S-376, FR-WS-05, ADR-53, BR-51).** Re-measured on the 84-member reference
  workspace the bound-ratio read `0.287 (81 of 282 measured)` over a workspace
  whose caller→callee edge count was **zero**: every one of those 81 bound rows
  was an OpenAPI operation matched to a controller route, not a resolved call. It
  also moved *downward* (0.355 → 0.287) as broker instrumentation improved, which
  is the wrong direction for a headline. CR-111 had already made the figure
  legible; legibility was not the problem.

  **`bound_ratio`, `bound_ratio_measured` and `bound_ratio_summary` are no longer
  emitted** on any surface — `logos workspace status` human output and `--json`,
  the `workspace_status` and `workspace_reachability` MCP tools, the
  `/api/v1/workspace/status` endpoint and the web coverage view. A reader of the
  old key now gets a missing field and fails loudly rather than silently reading a
  figure that no longer means what it did. The three keys are accepted for one
  release as **deserialization aliases** (`logos_core::federation::SpecConformanceReading`),
  so a stored pre-change capture still parses. **That window closes with the
  release after the one carrying this entry** — the aliases, and the type holding
  them, are removed then. Anchored here rather than left as "one release", because
  an unanchored deprecation becomes permanent vocabulary.

  What replaces them:
  - `resolved_cross_service_edges` — cross-service edges resolved from a captured
    **invocation** (a caller→callee HTTP client call, a producer→consumer broker
    publish, a gRPC stub call). It counts *edges*, not sites: under the broker
    fan-out one publish binds every cross-member subscriber and the bridge emits
    one edge per subscriber.
  - `egress_resolution` — the rate at which captured egress *sites* resolve at
    all, over the invocation population's own denominator, with
    `egress_resolution_measured` beside it. **Absent** (`null`) when no egress
    site was captured, never a fabricated `1.0`.
  - `resolved_edges_summary` — both figures as one composed line, so no rendering
    can show the count without the rate (BR-51).
  - `spec_conformance_ratio` / `_measured` / `_summary` — the retired ratio's
    formula, unchanged, under the name of what it always measured: how far this
    workspace's *declarations* line up with its controllers. The composed line's
    wording is byte-identical to the old one, so a pre-change capture and a
    post-change one compare directly. CR-100's absent-on-zero-denominator
    guarantee and CR-111's denominator-disclosure duty carry over intact.

  Measured on the reference workspace 2026-09-09 and recorded as a durable
  baseline in `logos-core/tests/coverage_headline_baseline/`: **0 resolved
  cross-service edges, egress resolution 0.000 over 54 egress sites**, beside a
  pooled `bound: 81`. That baseline is labelled with the index generation that
  produced it (logos 1.4.7, pre-S-374) and carries the refresh procedure for the
  re-index that will move it.

### Changed
- **Cross-service coverage reports its `intake` on every row, and splits the
  classification counts by it (CR-120, S-377, FR-WS-05).** The headline
  `bound`/`ambiguous`/`unbound`/`no-provider` counters add two different claims
  together: a `contract-surface` reference is a *declared* endpoint matched to a
  controller, an `invocation` reference is a captured *call site*. On the
  84-member reference workspace the split is **81 contract-surface and 0
  invocation** bound rows — no outbound call site in the whole estate resolves —
  and a bare `bound: 81` reads as a healthy workspace.

  `intake` was previously serialized **only on a bound row**, so the invocation
  population's non-bound rows carried no population marker at all and the split
  was not recoverable from the payload. It is now present on **every** row, in
  every state, and is no longer optional. Read `bucket`/`state` for "did this row
  bind" — the presence of `intake` never meant that and now cannot be mistaken
  for it.

  New: `coverage.by_intake.contract_surface` and `coverage.by_intake.invocation`,
  each carrying the same four counts. The two **sum to** the four top-level
  counters, which the server now derives from the split — so a figure and its
  decomposition cannot disagree. Present on all four surfaces:
  `workspace status` human output and `--json`, the `workspace_status` MCP tool,
  and the web coverage view, which gains a *Coverage by intake* board beside its
  per-arm one (the arm axis cannot separate the two — an OpenAPI operation and an
  HTTP client call are both the `route` arm).

  No existing field changed meaning or value, and the unbound-reason taxonomy is
  untouched. Payload cost, measured on the reference workspace's shape: **+7.2%**
  — 311 202 → 333 619 bytes, `intake` on the 794 non-bound rows plus the summary
  block.

- **The HTTP client-call arm records the call sites it declines, so coverage
  reports them instead of losing them (CR-120, S-374, FR-WS-08 AC2).** The arm's
  normalizer returned a refusal reason and the caller discarded it with `.ok()`,
  so a declined call site left **no reference, no ledger row and no coverage
  entry** — an estate whose client paths are all composed at runtime read exactly
  like one with no outbound calls at all. FR-WS-08's second acceptance criterion
  already required such a path to "appear under a runtime-composition coverage
  reason", so this was a conformance failure, not a gap.

  A call whose path is not a static absolute literal now leaves one **keyless**
  `unresolved_refs` row — empty target, so no template is fabricated — which
  `workspace status` reports under `base-url-runtime`. That reason previously had
  **no production producer** anywhere in the tree; it now has one, and
  `impl From<ClientCallRefusal> for UnboundReason` is no longer dead. The row is
  inert to binding by construction: it promotes no node, keys no topic, and
  `route_key` refuses an empty `"METHOD /template"`, so it can never become an
  edge. One row per **declaration** (the ledger's own identity ignores `line`),
  stable across re-syncs. The recorder is the one the broker arm has used since
  S-370, generalized rather than copied — the two arms now share
  `extract::config::refs::record_refusals`.

  **This makes the reported numbers worse, and that is the correction working.**
  Measured read-only over the 84-member reference workspace: **115 production
  rows** (94 Java, 20 Go, 1 Python) and 16 test-tree rows, **131 in total**,
  against a prior count of zero. All 94 Java sites refuse; the workspace-wide
  reference count stays 1. Full reconciliation, including why 115 agreeing with
  CR-120's ~111 is two different measurements rather than one confirmed twice, in
  `logos-core/tests/operand_resolvability/client_call_refusal_finding.txt`.

  Two populations stay invisible and are stated rather than implied: a call the
  language's `invocations` query never matched (every stated capture ceiling — a
  verb-suffixed `RestTemplate` method, OpenFeign, a receiver the S-375 rule
  declines) is refused before a call site exists, so it carries no reason and
  only a query change can reach it; and `path-not-composed`, which needs a
  non-keyless row and is out of this increment's scope — zero on Java by S-355's
  recorded evidence, and bounded rather than measured workspace-wide.

- **The NFR-PE-05 cold-start budget is re-derived 500 → 600 ms and now enumerates
  all six phases (CR-116, S-368, S-369).** The requirement enumerated three
  phases — embedded `plugin.toml` parse, `LanguageRegistry` construction, query
  compilation — while both tests citing it measured something else, and neither
  measured its enumeration. Per-phase instrumentation over eight fresh-process
  cold starts settled it: the three enumerated phases came in at mean 440.1 /
  **max 457.5 ms**, inside the old 500 ms budget in every sample, so **this was
  never a performance breach**. The ~67 ms excess was store open, schema
  migration, pool startup and incidental cold-path work the requirement never
  claimed to bound.

  A user waiting for a ready engine waits for the store too, so the requirement
  now enumerates all six phases and bounds their **total** wall time — the whole
  path to a ready engine — at **≤ 600 ms**, re-derived from the measured full
  total: p90 528.4 / max 539.8 ms, leaving 71.6 ms and 60.2 ms of margin (11.9%
  and 10.0% of the budget), rather than from the old target plus a margin. `LOGOS_PERF_TOLERANCE`'s default stays **1.0**; no cost
  regressed, and no per-phase sub-budgets exist, so no subset of the six may be
  gated as NFR-PE-05 conformance. The 200 ms `Runtime::open` guard, which
  asserted over exactly the phases the old requirement *excluded*, keeps its band
  but drops the citation and is renamed
  `runtime_open_stays_within_its_store_and_pool_regression_band`.

- **`Engine::start` runs one `git rev-parse --git-common-dir` on the DB-less
  worktree path instead of two (CR-116 §9.5, S-369).** The graph-store seed and
  the governance-contract seed each resolved the primary checkout with their own
  identical subprocess. They now share one resolution, via the new
  `workspace::seed_source_from_primary`. Waste removal on a budgeted cold path,
  not budget-chasing: the saving is headroom, and the 600 ms figure above is
  derived from the measurement *before* it. Measured effect on the phase that
  absorbs it (`ColdStartPhases::other`), medians over 10 fresh-process samples
  per run — medians because the documented cross-process ramp-up effect puts
  the occasional 1 s outlier in the mean: **32.1 / 32.1 ms before** (2 runs) →
  **24.2 / 24.2 / 22.5 / 24.4 / 23.5 ms after** (5 runs), a saving of ~8 ms,
  ~25% of the phase. The **full** cold start is *not* measurably faster: query
  compilation dominates it at ~440 ms and drifts ±25 ms between runs on the same
  host, which swamps an 8 ms saving. That is the expected outcome — the point was
  to stop paying twice for one answer.

### Added
- **Named, caused, degraded-member reporting in `workspace status` (S-326,
  FR-WS-16).** Each member row gains an `open_state` — `opened` /
  `not-attempted` / `degraded` — on an axis **separate** from the existing
  `warm_state`: the first is about whether the store could be opened, the second
  about whether it holds an index, and an un-indexed member that opens perfectly
  well is `deferred` / `opened`. A degraded row carries `degraded_reason` and,
  where the diagnostic identifies one, `degraded_cause`:
  `host-resource-limit` (the store is present and intact; the process ran out of
  file descriptors — raise `ulimit -n`, do **not** re-index) or
  `store-obstructed` (something that is not a regular file occupies
  `.logos/logos.db` — clear that path, then re-index). The previous message named
  only SQLite's "unable to open database file", which reads as a corrupt store
  and sent readers to the wrong remedy. A failure whose store file is merely
  *missing* deliberately claims **no** cause: the store is created on open, so an
  absent file at failure time is equally consistent with descriptor exhaustion
  during creation, and a guessed "go re-index" would relocate the very
  misdiagnosis this removes. `degraded_diagnostic` always carries the verbatim
  engine error beside the classified sentence, so classifying never destroys the
  evidence it read.
  The payload gains a `degraded_rollup` naming every unopenable member, folded
  into the same member table as the warm roll-up rather than a competing one, and
  a human-readable warning naming them goes to stderr so `--json` stdout stays
  machine-clean. The existing per-member `error` field is unchanged.

### Fixed — BREAKING (payload shape)
- **`coverage.bound_ratio` is now reported ABSENT instead of a fabricated `1.0`
  when nothing was measured (CR-100, S-326, FR-WS-05, NFR-CC-04).** A zero
  `bound + ambiguous + unbound` denominator used to serialise as a perfect
  score, so the observed partial workspace reported `bound: 0` beside
  `bound_ratio: 1.0` — a confident measurement invented from no evidence. The
  field is now omitted (`null` in `--json`) in that case, in `workspace status`,
  `workspace reachability`'s coverage rider, and the `/api/v1/workspace/*` web
  payloads alike. A consumer reading it as a number must handle absence; the web
  UI renders "bound ratio not measured" rather than an empty or full bar.
- **The coverage summary states how much of the workspace it covers (S-326,
  FR-WS-16, NFR-CC-04).** `coverage.members_read` / `members_total` /
  `covers_all_members` are new: a summary computed over a partially-opened
  workspace is now marked as covering fewer than all members instead of reading
  as a complete picture.

## [1.4.13] — 2026-09-18

Sprint 71: the last cross-service arm agrees with itself, and three readouts start reporting state they can establish (CR-132, CR-133).

## [1.4.12] — 2026-09-17

Sprint 70: Kafka Streams coupling becomes visible, and the two gates that decide the config-declared intake are measured (CR-131).

## [1.4.11] — 2026-09-14

Sprint 69: the capture reaches the shapes real estates write, and every cross-service figure agrees with its own payload (CR-123..CR-127).

## [1.4.10] — 2026-09-13

Sprint 68: committed-configuration evidence is wired into the production extract pass, and the workspace surface stops repeating itself.

## [1.4.9] — 2026-09-12

Sprint 67: committed configuration becomes cross-service evidence, and both blocking gates return a verdict (CR-121).

## [1.4.8] — 2026-09-09

Sprint 66: the invocation arms report their own refusals, and the headline stops flattering them (CR-120).

## [1.4.7] — 2026-09-07

Sprint 65: the gating measurements for the blocked change requests, plus the capture work not waiting on them.

## [1.4.6] — 2026-09-06

Sprint 64: `logos check` over an absent rules contract exits 4 instead of passing vacuously, seeded worktrees carry the contract, and the graph is offered at scoping time (CR-112..CR-114).

## [1.4.5] — 2026-09-05

Sprints 61–63: bounded workspace index/warm resources, JVM Spring route binding, a truthful multi-repo workspace from first contact to steady state, and visible cross-service REST coupling (CR-098..CR-102, CR-108..CR-111). 1.4.4 was a dev-only build.

## [1.4.3] — 2026-08-06

The session-start quality readout and the write-free `logos quality-report` (CR-095); four RustSec advisories cleared. 1.4.2 was never released.

## [1.4.1] — 2026-07-26

Sprint 60: chat UI refresh — assistant-ui column, foldable activity, Mermaid viewer (CR-089).

## [1.4.0] — 2026-07-25

Sprint 59: multi-thread chat history, streaming answers, and metric-semantics corrections (CR-053).

## [1.3.0] — 2026-07-23

Sprint 58: hardened CR-061 app-wide reachability and broker coupling, and dashboard LOC figures (CR-084).

## [1.2.3] — 2026-07-12

Sprint 57: CR-061 completion — broker topics, reachability, governance, workspace UI.

## [1.2.2] — 2026-07-11

Sprint 56: CR-061 cross-service invocation arms.

## [1.2.1] — 2026-07-11

Sprint 55: CR-061 federation surface.

## [1.2.0] — 2026-07-11

Sprint 54: CR-061 multi-repo workspace federation foundation.

## [1.1.1] — 2026-07-10

Sprint 53: the Quadrant view and static test-gaps tool are dropped, and Rule findings are promoted (CR-079).

## [1.1.0] — 2026-07-10

Sprint 52: the web dashboard joins the default build, with the agents egress carve-out (CR-078).

## [1.0.7] — 2026-07-09

**Sub-second `serve` cold start (CR-077).** The `serve --mcp` filesystem watcher
no longer pre-walks build-output trees at registration time, restoring the
NFR-PE-05 cold-start budget. No schema, reconcile-contract, or public-interface
change; the quality signal is unchanged (delta 0).

### Fixed
- **Watcher registration prunes ignored directories (CR-077, S-285, NFR-PE-05,
  FR-SY-04, FR-SY-11, ADR-48).** `notify-debouncer-full`'s file-ID cache used to
  seed itself by walking the *entire physical tree* on `.watch()` — `target/`
  included (measured ~1.2M entries) — before the server could answer its first MCP
  request, a ~58–93 s stall on every `serve` boot. A custom `PrunedFileIdCache` now
  routes that registration-time seed walk through the same `AdmissionAuthority`
  predicate the full `index` walk and event-time `classify` already share, so it
  visits only admitted source directories. Cold start returns to sub-second
  (measured ~1.7 s cold / ~0.5 s warm on the ~221k-LOC dogfood repo, was ~58–93 s).
  Directory descent is name-pruned (`target`, `node_modules`, `dist`, `build`,
  `vendor`, `.git`, the feedback-loop set, and `[semantics].ignored_dirs`); leaf
  files are gated through the admission authority for full-walk parity; the walk
  degrades to a name-only prune when no authority is available. Rename tracking for
  admitted source paths and event-time `classify` are unchanged — the watcher stays
  best-effort and non-load-bearing for correctness (FR-SY-06, ADR-11).

## [1.0.6] — 2026-07-09

**Reachability & metric-scope precision (CR-073 + CR-074 + CR-075 + CR-076).**
Four independent graph-precision fixes closing standing CRs, all monotonic and
never-fabricate. No schema or reconcile-contract change; the signal-vs-baseline
gate shifts by design (recovered coupling + production-scoped metric values) and is
re-blessed at release via `logos gate --save`.

### Added
- **Optional production-scope filter for the hotspot surface (CR-076, FR-GH-06,
  FR-CV-07).** `logos hotspots --production-scope` (and the MCP `hotspots` tool's
  `production_scope` arg + the web Files & Risk view's "Production files only"
  toggle) drops whole test files (`tests.rs`/`*_tests.rs`/`tests/`) from the
  candidate set *before* ranking, so the `--untested` board surfaces production
  files instead of test files. Opt-in and gate-immune (BR-26): the default board is
  byte-identical and toggling never moves a gated signal. CLI/MCP/web return
  identical rankings for the same state (NFR-CC-01).

### Fixed
- **Trait-object dynamic-dispatch reachability (CR-073 / CR-068 Part C, FR-RS-08,
  ADR-39).** A `&dyn Trait` receiver-method call now fans out to the trait's default
  body ∪ its concrete workspace impls via real `Calls` edges when the receiver is a
  *provable* trait object (explicit `&dyn T`/`Box<dyn T>`/`Arc<dyn T + …>` binding),
  and the dispatch live-rooting pass now roots trait-default bodies. The six
  `LanguagePlugin` trait methods reached only through `dyn` dispatch stop reading as
  dead (dead-callable census down); the CR-066 receiver-method guard (FR-RS-06) is
  not loosened and never-fabricate (NFR-RA-05) is preserved — a non-provable receiver
  is an honest miss, not a guess. A new (previously-unemitted) `Implements` edge is
  fenced out of every metric subgraph, so only the intended dead-code recovery moves.
- **Redundancy budgets production-scoped (CR-074, FR-QM-08).** `check_redundancy`'s
  `max_dead`/`max_duplicates` budgets now exclude `is_test` functions via the shared
  `test_node_ids` set, matching the Redundancy metric (FR-QM-05) and the sibling
  fan/structural budgets. `max_dead` is behaviour-preserved; `max_duplicates` drops
  by the excluded test-duplicate count.
- **Plural Rust test-file conventions recognized in `is_test` detection (CR-075,
  FR-AN-05).** The shared `is_test_path` helper now matches bare `tests.rs` and the
  snake_case `*_tests.rs` suffix (it already matched CamelCase `*Tests`). Non-`#[test]`
  helpers in those files are now correctly `is_test=true`, removing test-code
  contamination from every production-scoped metric via the single shared column; all
  consumers correct in lock-step with no per-consumer edits.

## [1.0.5] — 2026-07-08

**Graph-precision & config-surfacing cleanup (CR-071 + CR-068 Part B + CR-067).**
Three independent precision/UX fixes closing standing CRs, all additive/read-model.
No schema, reconcile-contract, or gate-baseline change.

### Added
- **Config parameter defaults surfaced in the web Config editor (CR-067, FR-UI-12).**
  `GET /api/v1/config` gained a read-model `defaults` projection — code-sourced and
  computed independently of the live documents. The Config editor now renders a
  `Default: …` hint beside every `config.toml`/`[metric_thresholds]` field and
  `unset → not enforced · Recommended: …` beside every `[constraints]` field. The
  save/apply/validate path is byte-identical, and the chat API key is never present
  in the projection (NFR-SE-07).

### Fixed
- **Sanctioned external-docs symlink now followed on git-ignoring checkouts (CR-071,
  FR-IX-10, ADR-59).** Resolves the CR-069/1.0.4 known limitation: the discovery walk
  built its `WalkBuilder` with git-ignore filtering upstream of the `.swe-skills`
  follow-branch, so a repo whose `docs/{specs,planning,requests}` symlinks are
  git-ignored indexed **zero** doc nodes. A dedicated git-ignore-bypassing detection
  pass — confined to the documentation subtree, one-hop, containment-gated, sanctioned
  root only — now follows the symlink and indexes the docs behind it. Admission parity
  (`admits_path`, ADR-48) mirrors the carve-out so `doctor`'s admission tripwire does
  not flag the freshly-indexed docs as drift. Source-code symlinks are still skipped
  wholesale; an escaping/unsanctioned target is refused, not followed.
- **Associated-function `Method` kinding and bare-path binder tie-break (CR-068 Part B,
  FR-EX-05, FR-RS-07, ADR-39).** Rust `impl`-nested associated functions are now kinded
  `NodeKind::Method` (distinct from free `NodeKind::Function`) at emission — symbol IDs
  and ordinals byte-identical (NFR-RA-06). In the binder, a single-segment bare-path
  call now prefers a free function over same-named associated methods (a monotonic
  tie-break, never a drop; the full callable set stands when no free function exists).
  On the Rust dogfood this recovers the `graph_store/mod.rs` `insert_node`/`insert_edge`/
  `upsert_symbol` cluster: resolved `Calls` edges **3694 → 3716 (+22)** with **0 lost**
  and **0** live callable turned dead (BR-38 monotonic). Receiver-method calls and the
  CR-066 unique-name fallback are untouched.

### Added (diagnostics)
- **`doctor --json` `doc_symlink_warnings` (CR-071, FR-IX-11).** A new advisory array
  naming any documentation directory-symlink that exists under the doc-include set but
  ended up unindexed (no sanctioned root, or target escapes containment). Purely
  diagnostic — it never flips `ok` or changes the exit status. `index`/`sync` fold the
  same drops into their warnings.

## [1.0.4] — 2026-07-08

**Binding precision & discovery fidelity (CR-068 + CR-069 + CR-070).** Continues the
measurement-precision arc into *binding & discovery* precision, with the same
never-fabricate, monotonic, honest discipline. No schema, reconcile-contract, or
gate-baseline change.

### Added
- **Function-pointer handoffs recognized as live roots (CR-068 Part A, FR-AN-01,
  ADR-39).** The framework-dispatch pass now recognizes axum function-pointer
  handoffs — `.fallback(fn)`, `middleware::from_fn(fn)` / `from_fn_with_state(_, fn)`,
  and every method-router handler inside `route(path, get(fn)|post(fn)|…)` including
  chained setters (`get(a).post(b)`) — as live roots via the existing `RoutesTo`
  self-marker. A handler name is bound only to the one same-file callable of that
  name (exactly-one-or-nothing), so nothing is fabricated (NFR-RA-05). On the Rust
  dogfood this drops false-positive dead functions **61 → 49** (12 `web/src/lib.rs`
  handlers/guards/fallbacks recovered) with **zero** previously-resolved `Calls`
  edges lost and **no** live function turned dead (BR-38 monotonic); a full re-index
  is required to observe the new liveness (an incremental `sync` over unchanged files
  keeps the prior markers). The `name_matcher` is untouched (the CR-066 fallback is
  not loosened).

### Removed
- **PostToolUse wiki-augmentation hook retired (CR-070, FR-WK-14).** The advisory
  augmentation hook — which surfaced the `wiki generate` work-list to the connected
  agent on every tool call — is deleted from the binary (`AUGMENT_SPEC`, the augment
  script/constants, and the augment `materialize()` entry point are gone). `logos
  init -i` and `logos wiki hook --emit [--force]` now install/emit only the SessionEnd
  quality-report hook (FR-IN-07); `wiki hook --emit --json` consequently returns a
  single summary object rather than a two-element array. The deterministic `wiki
  generate` queue and the `ui`-gated in-process generator (FR-WK-18) are unchanged.

### Known limitations
- **External docs-root symlink following (CR-069, FR-IX-10) landed but is inert on
  checkouts that git-ignore the doc symlinks.** The discovery carve-out that follows
  a `.swe-skills`-sanctioned docs symlink is correct in isolation, but the walk's
  git-ignore/exclude filtering runs upstream of it, so on a repo whose `docs/{specs,
  requests}` symlinks are git-ignored the doc nodes are not indexed. Resolving whether
  a sanctioned docs root should override its own git-ignore is a discovery-contract
  decision deferred to a follow-up CR (amending FR-IX-10 / ADR-59 / NFR-SE-04).

## [1.0.3] — 2026-07-07

**Graph measurement precision (CR-065 + CR-066).** Two core graph measurements
are re-based to reflect genuine structure rather than measurement artifacts, with
no change to the tool surface.

### Changed
- **Module-grain, production-scoped coupling budgets (CR-065, FR-GV-11, FR-QM-08).**
  `check_coupling`'s `max_fan_in` / `max_fan_out` now count each *module's* distinct
  neighbouring modules over the canonical dependency view (reusing the module rollup
  that backs `logos dsm`), excluding `is_test` nodes before the rollup. A shared
  helper called from many symbols in one module counts that module once, not once
  per call site — so the budget flags genuine cross-module coupling, not the name
  popularity of standard-library method names. Deterministic, module-key-ordered
  output (NFR-RA-06); the coupling budget stays a `check_rules`-only budget,
  orthogonal to the quality-metric gate signal (ADR-21).
- **Receiver-unqualified method binding no longer fabricates `Calls` edges (CR-066,
  FR-RS-06, FR-RS-03, NFR-RA-05).** A bare `x.f()` method call now resolves only on
  genuine lexical/module scope evidence; the workspace unique-name and suffix
  fallbacks are gated off for the receiver-method form, so a `.map()`/`.collect()`/
  `.join()`/`.path()` with no in-scope target stays in `unresolved_refs` and retries
  on sync instead of binding to a same-named callable in an unrelated module. Removes
  the ~29.5% of `Calls` edges that funnelled into ~15 std-method-named targets
  (~1065 edges / −21.3% on the dogfood self-graph). Path-qualified and typed calls,
  and free-call resolution across C++/Java/Kotlin/Scala/C#, are unchanged. The
  downstream dead-code/cycles signal shift is re-blessed deliberately at release per
  FR-GV-16, with CR-066 as the accepting authority.

## [1.0.1] — 2026-07-06

**Trustworthy wiki reframe (CR-062).** The source wiki gains a third tier: the
binary now *presents* the project's authored `docs/specs/**` and `docs/howto/**`
verbatim instead of paraphrasing them with a model, reserving LLM inference for
the Summary/Overview tier alone. This retires the per-file wiki pages that added
volume without traceable provenance, so the corpus converges on three canonical
tiers (extracted / presented / generated) — see [ADR-57].

### Added
- **Deterministic presented tier — `logos wiki materialize` (FR-WK-20, ADR-57).**
  A pure, offline, idempotent command assembles one wiki page per Design/Specs
  category (section-per-source-file) directly from the authored SRS sources,
  labelled `generator = logos:doc-present` ("Presented verbatim … Not
  model-generated") — copied verbatim, never paraphrased. No LLM, no network.
  `materialize` has a payload-identical MCP twin (`wiki_materialize`), bringing
  the wiki tool set to five (`write`/`read`/`search`/`status`/`materialize`).
- **User Guide tier (FR-WK-23).** `wiki materialize` also presents one
  `guide/<name>` page per `docs/howto/*.md` (e.g. `guide/overview` ← `README.md`),
  so the rendered manual is copied verbatim from source rather than regenerated.
- **SRS-mode bimodal generation gate (FR-WK-21).** When the project ships an SRS
  (`docs/specs/architecture.md` + a requirement), the Design/Specs and User
  Guide pages are presented and the connected agent generates only the
  Summary/Overview tier; otherwise it infers the full set from the code graph.

### Changed
- **User-needs-aware Overview generation (FR-WK-24).** The generated
  Summary/Overview pages are grounded in `README.md` + `docs/howto/**` and
  prompted to read as user-facing (goals/workflows), not a symbol tour.

### Removed
- **Per-file wiki pages retired (FR-WK-22).** The `files/*` pages are excluded
  from the generation work-list and from the status page count, and a
  reconciliation sweep purges unreachable orphan pages so a previously bloated
  corpus self-prunes to the three canonical tiers.

## [1.0.0] — 2026-07-05

**First stable release.** Logos reaches 1.0.0 with a trustworthy source-wiki
generation pipeline (Sprint 44 — CR-059) on top of the structural code-graph,
architecture-quality gate, agentic chat, and single-binary web UI shipped across
the 0.x line. The command surface, `.logos/` store layout, MCP tool set, and CLI
JSON contracts are now considered stable under [Semantic
Versioning](https://semver.org/).

### Added
- **Grounded wiki generation (FR-WK-18, ADR-51).** Each queue item's grounding
  content — the referenced `docs/` source read, or a token-bounded code-graph
  digest as fallback — is resolved in-binary and injected into the synthesis
  prompt, so the tool-less generator writes page prose only from supplied
  context instead of hallucinating or emitting planning noise. Generation stays
  offline; no network egress on the grounding path (NFR-SE-01).
- **Write-path content-validity guard (FR-WK-19, NFR-CC-04).** The shared
  `wiki write` façade — CLI stdin/`--body-file` and the in-process generator
  alike — rejects a body that is agent-noise rather than page prose (a
  `<tool_call>` token, an `Error:`/`cmd:` transcript, a first-person
  planning/refusal preamble, no Markdown heading, or below a minimum length). A
  rejected write leaves the store byte-identical and is reported as an honest
  per-page failure, never a fabricated page. The two noise-content signatures
  are scanned with fenced code blocks stripped, so a page that legitimately
  *quotes* one of those patterns inside a ` ``` `/`~~~` fence is accepted.
- **Wiki run-state legibility (S-239).** The web Wiki tab shows cumulative
  "N of M" progress and a visible per-page synthesis-timeout hint; a halted run
  reads "Generation halted" rather than a stale "Generating…" or a false
  "complete", and a drained work-list launches no redundant run.

### Changed
- **Corpus regeneration under the fixed pipeline (S-238).** Regeneration drives
  grounding + guard together and purges orphaned pages, so a previously
  corrupted corpus is replaced with grounded, guard-checked prose.
- **Statistics "Dev vs main" card aggregates worktree branches (FR-OB-08).** The
  origin split (`calls_by_origin`, surfaced in the web Statistics tab and `logos stats`)
  now collapses every non-`main` origin into a single cumulative `"dev"` bucket, so the
  card is a two-way comparison of all development-increment work versus `main` — instead
  of one bar per (often stale) worktree branch, which added noise over a wide window. The
  stored per-event `origin` is unchanged; only the read-model aggregation changed.

## [0.14.0] — 2026-07-04

**Recoverable-fault degradation across the agent substrate (Sprint 43 — CR-060).**
A single recoverable subagent fault (a transient provider hiccup, a missing-path tool
error) no longer kills the whole chat turn. The runtime now recovers at three layers —
transparent provider retry, tool-errors-as-observations, and cross-step degradation —
so the turn continues and returns a best-effort grounded answer.

### Added
- **Provider-call retry with backoff (S-240).** A transparent `RetryingModel` decorator
  over rig's `CompletionModel`, wired into both provider constructors, retries retryable
  faults (transport, 429, 5xx, deserialization hiccups) with exponential backoff + jitter;
  `Auth` and other terminal errors are never retried, and exhaustion returns the original
  error. Two new `[chat]` keys — `max_provider_retries` (default `2`, `0` disables) and
  `provider_retry_base_ms` (default `200`, `0` rejected at load) — are inherited by the
  wiki-agent.
- **Tool errors as self-correcting observations (S-241).** In `run_tool_subagent`, tool
  errors and out-of-domain requests become model-visible `tool_result` observations so a
  subagent adapts instead of dying, bounded by a consecutive-error soft-close cap
  (`CloseReason::ToolErrors`) that closes well-formed with a `[bounded — …consecutive tool
  errors…]` marker; a success resets the streak.
- **Cross-step fault degradation (S-242).** A new `StepError::Unavailable` reclassifies the
  three recoverable roster fault sites; a step that stays unavailable after retries degrades
  to a `[unavailable — …]` observation and the turn continues, answering best-effort when any
  usable observation exists (and halting honestly when none do). Synthesizer/structural faults
  stay turn-fatal; sustained outage terminates via `max_replans`.

### Changed
- Recoverable subagent faults are now degradation events, not turn-fatal errors — extending the
  bounded-degradation posture established in 0.13.0 (CR-048) from budget exhaustion to provider
  and tool faults.

## [0.13.0] — 2026-07-04

**Bounded, graceful degradation of generative work (Sprint 42 — CR-048 + CR-044).**
The agent budget tree stops being a hard tripwire and becomes a soft, self-summarizing
bound; subagent preambles are budget-aware; wiki regeneration cadence is dampened.

### Changed
- **Soft per-subagent budget cap (S-181).** A subagent that reaches its per-subagent
  tool-call cap no longer hard-halts the turn — it closes well-formed, summarizes its
  findings tool-free, and returns a marked `[bounded — …cap…]` observation. Only the
  global `max_tool_calls` ceiling and `max_replans` hard-halt; on a hard halt the
  orchestrator returns a best-effort grounded answer over the scratchpad (or an honest
  bare halt when nothing was gathered) instead of an error.
- **Budget-aware subagent preambles (S-182).** Each tool-bearing subagent's preamble
  names its cap and running "calls remaining", steering it to prefer the
  breadth-efficient `context` tool; the preamble is rebuilt from the live budget at
  every model round, including the soft-close step.

### Added
- **`[wiki].revision_stale_threshold` (S-164).** A new config key (default `5`, min `1`,
  `0` rejected at load) dampens the re-queue cadence of anchorless prose wiki pages.
  `revision_stale_count` stays truthful; the regeneration queue stays a pure offline read.

## [0.12.1] — 2026-07-04

**Web-UI polish (dogfood fixes).** Three rendering/aesthetic fixes surfaced while
dogfooding 0.12.0; no behavior or API change.

### Fixed
- **Files & Risk section spacing.** `FilesView` returned bare fragments, so the
  hotspot Callout and the risk/ownership Cards stacked flush; it now uses the shared
  `.view` section-stack (`gap: var(--space-5)`) like the Coverage/Quadrant tabs.
- **Wiki search form.** The hand-rolled raw `<input>` (TOC-heading label voice, no
  label↔control spacing, browser-default ~20-char width) is replaced by the shared
  `TextField`: a real form label, `var(--space-2)` spacing, and a full-width input.
- **Mermaid diagram legibility.** Under the self-only CSP, Mermaid's injected style
  is stripped, leaving arrowhead markers on the SVG default black fill and edges at
  an uncontrolled weight. The external CSS now sets a light edge `stroke-width` and
  reliably re-colors the arrowhead markers to `--text-2`, so diagrams read cleanly.

## [0.12.0] — 2026-07-03

**Wiki generation, made usable end-to-end (CR-056).** The in-process wiki
generation run no longer floods its work-list with per-file pages, survives a
dropped SPA connection, and reports honest cumulative progress on re-attach.

### Changed
- **Pruned generation work-list (S-221).** `status()` / `structured_sections()` /
  `generation_queue()` no longer seed per-file `objectives` pages or unanchored
  File/Module entities — the cold-start queue collapses from ~1600 to dozens on
  this repo. Existing `wiki.db` pages are still served and refreshed on drift;
  determinism (NFR-RA-06) preserved.
- **Connection-resilient, auto-continuing run (S-222).** The run's lifetime is
  owned in app state rather than the SSE response body, so dropping the stream no
  longer aborts generation — it auto-continues across budget chunks until the
  work-list drains, bounded by a hard safety ceiling. A per-page synthesis timeout
  prevents a hung provider from orphaning the single-run lock.
- **Wiki-tab re-attach + cumulative progress (S-223).** Reopening the tab mid-run
  subscribes to the live run (exactly-once delivery) and shows a whole-run
  cumulative "N of M", not a per-chunk reset; reopening after completion reads
  "up to date" and starts no second run.

### Added
- **Typed `[wiki].model` field in the Config tab (S-224).** The wiki synthesis
  model is editable in the UI; a valid save round-trips through an atomic
  write-back, an invalid document is rejected inline with no partial write, and an
  absent `[wiki]` section renders blank rather than crashing.

## [0.11.0] — 2026-07-03

**Durable telemetry + Statistics tab (CR-058).** Logos tool usage is recorded in a
store that survives worktree teardown, and the dashboard surfaces it.

### Added
- **Durable, shared-primary telemetry store.** Tool invocations are persisted to a
  telemetry store rooted in the git common directory (surviving worktree teardown),
  stamped with an `origin`, and queryable via `logos stats`.
- **Statistics dashboard tab.** The web UI exposes recorded tool usage.

### Fixed
- **HF-1:** web-UI-originated activity is excluded from the Statistics read-model so
  the dashboard reflects agent/CLI usage rather than its own rendering.

## [0.10.0] — 2026-07-03

**Cold-index performance — measure-first de-serialization of the write path (CR-057).**
A full cold index is materially faster with byte-identical graph output. Nothing
about the produced graph changes; only how fast it is built.

### Added
- **Per-phase index instrumentation (S-225).** `logos --json index` now carries a
  `phases` object with per-phase wall-clock durations (`discover`, `load`, `extract`,
  `persist`, `resolve`, `framework`, `dispatch`, `annotate`), derived from the same
  `tracing` seam as the logs and reconciling to `total_ms`. A repeatable cold-index
  benchmark (`cold_index_phase_baseline`) records total + per-phase + peak RSS.

### Changed
- **Parallel annotation compute (S-229).** Near-clone clustering (the measured ~83%
  of the annotate phase) and the per-node verdict loop now run on the shared worker
  pool via keyspace-sharded pair counting — **annotate −54% (2.2×)**, byte-identical
  across every worker count, peak RSS held under the 1 GB ceiling.
- **Chunked Pass-1 persistence (S-226).** The per-file commit storm collapses into
  bounded write batches (≈958 → ≈4 transactions on this repo), single-writer
  invariant preserved.
- **Parallel file-load and discovery walk (S-228).** Read+hash and the directory
  walk fan out on the shared pool; order-deterministic, byte-identical.
- **Writer bulk-load pragmas (S-227).** The writer connection sets `cache_size` /
  `mmap_size` / `temp_store` for the index write window; the reader pool is untouched.

Overall cold-index total is down ~25% on a real repo, driven by S-229 and S-226.
Full analysis: `docs/perf/cold-index-0.10.0.md`.

## [0.9.8] — 2026-07-02

**Web-UI packaging hardening — no more silent white page (CR-049 follow-up).** A
`--features ui` binary built without a matching `npm run build` no longer serves a
blank page. This is a build/packaging robustness fix; the offline default binary is
unaffected.

### Fixed
- **Hash-free committed placeholder (`web/ui/dist/index.html`).** An earlier revision
  committed a real Vite build's `index.html` as the "placeholder", so its frozen
  `/assets/index-<hash>.js` reference never matched any fresh build — embedding it
  produced a `200` shell whose JS bundle `404`'d (a blank page). The placeholder is
  now a genuine hash-free stub that references no build artifacts and shows a visible
  "UI bundle not built — run `npm run build`" message, so it can never create the
  hash mismatch. This also fixes `web/tests/spa_shell.rs`'s
  `every_shell_referenced_asset_resolves_through_the_router`, which was red at HEAD.

### Added
- **Serve-time SPA consistency guard (`web`).** When the embedded shell references an
  `/assets/*.{js,css}` the binary does not embed, `/` (and the history fallback) now
  return a `503` self-describing diagnostic naming the missing assets and the rebuild
  recipe, instead of the silent white page. Detection is a pure ref-extractor over
  the embedded `index.html`; the happy path (consistent bundle) is unchanged.

## [0.9.7] — 2026-07-02

**Internalized wiki generation (CR-047).** Source-wiki prose generation moves
**in-process** onto the shared `rig` agent substrate the Chat tab already uses,
replacing the out-of-process headless `claude -p` SessionEnd autogen hook. The
default (offline) binary is unchanged and remains provably offline — the wiki
generator, like chat, compiles only under `--features ui`.

### Added
- **Dedicated `[wiki].model` config (S-176, FR-CF-07).** An optional `[wiki]`
  section selects a wiki-generation model distinct from `[chat].model`, inheriting
  `provider` / `base_url` / the `secrets.toml` key from `[chat]` (no separate wiki
  provider, endpoint, or secret). Omit it to fall back to the chat model.
- **In-process wiki-agent (S-177, FR-WK-18).** A new `ui`-gated `wiki-agent` crate:
  a single-purpose `rig` agent on the shared `agent-core` substrate that loops the
  deterministic `wiki generate` queue, uses the embedded `logos-wiki` skill body as
  its system prompt, and persists pages via the unchanged `wiki write` contract. No
  planner/subagent roster; a per-run budget bounds the pass.
- **Wiki-tab generation trigger (S-178, FR-WK-18, NFR-SE-07).** Opening the Wiki tab
  launches a background generation run under a single-run lock, renders existing
  pages immediately, and streams per-page refreshes over a same-origin SSE endpoint
  (`POST /wiki/generate`). The first outbound call in a session is gated by a
  first-use consent disclosure naming the configured endpoint; an unconfigured
  provider shows a configure-first state, not an error.

### Changed
- **Retired the headless `claude -p` wiki-autogen hook (S-179, FR-WK-16 → CR-047).**
  `logos init -i` no longer installs the SessionEnd autogen hook or its
  `.claude/settings.local.json` materialization. The embedded `logos-wiki` skill is
  now the wiki-agent's system prompt (single source of guidance) and is still
  materialized via `logos wiki skill --emit` for manual regeneration. The advisory
  PostToolUse augmentation hook is unchanged.

### Verified
- **Offline carve-out regression + UAT (S-180, UAT-WK-06, NFR-SE-01).** The default
  build links no HTTP client (no-networking-crate fitness test byte-identical); the
  full configured→open→consent→regenerate→stream→dual-axis-fresh flow is exercised
  end-to-end via the mock `CompletionModel` with zero real egress and a loopback bind.

## [0.9.6] — 2026-07-02

**Standalone quality integration (CR-055).** The full quality loop —
**freshen / enforce / report / bless** — is now a first-class capability any
adopter gets from `logos init`, decoupled from the swe-skills harness (ADR-49).
The existing `check` / `scan` / `gate` commands are unchanged; two new
`init`-installed triggers plus a documented CI recipe wire them into everyday
workflow.

### Added
- **Enforcing `pre-push` git gate (S-218, FR-IN-06).** `logos init --hooks` now
  installs a fourth, **blocking** git hook alongside the exit-0 freshness hooks:
  a `pre-push` gate that runs `logos check` and **propagates its non-zero exit**,
  so a rule / structural / admission / dead-code regression makes `git push` fail
  (exit 1) and names the offending contract. It bails **open** (exit 0) when the
  `logos` binary is absent — never a false block — carries the managed marker, and
  is bypassed by `git push --no-verify`.
- **Harness-agnostic Claude Code SessionEnd quality-report hook (S-219,
  FR-IN-07).** `logos init -i` now registers a non-blocking SessionEnd hook that
  runs `logos check` + `logos scan`, prints the current signal, the baseline
  signal, and any violations to the terminal, and **always exits 0** (never blocks
  session teardown). Disable it without uninstalling via the
  `LOGOS_QUALITY_REPORT_DISABLE=1` off-switch (mirroring the wiki-autogen hook).
- **CI recipe and workflow-integration docs (S-220).** New
  [`docs/howto/ci-integration.md`](docs/howto/ci-integration.md) documents the
  freshen / enforce / report / bless model and ships a copy-pasteable CI recipe —
  `logos check` as the enforcing build step, `logos scan --json` as the
  non-blocking signal report, and `logos gate --save` blessing a new baseline **at
  release only, never in the PR path**. Cross-linked from the how-to README,
  usage, and the `init` command reference; `error-handling.md` documents the
  `pre-push` exit-1 contract.

## [0.8.0] — 2026-06-27

**Agentic Chat.** A new `ui`-gated Chat tab brings an orchestrated LLM agent over
the code graph to the localhost dashboard (Sprint 30, CR-045 + CR-046).

### Added
- **Agentic chat orchestrator** — an LLM planner runs a plan→act→observe→replan
  loop over a fixed roster of four specialized subagents (Graph-Navigator,
  Governance-Analyst, Source-Reader, and a tool-less Synthesizer), each driven at
  the completion-model level through a least-privilege bounded tool dispatcher, all
  bounded by a budget tree (global tool-call ceiling / per-subagent cap / max
  replans) that halts honestly rather than fabricating an answer.
- **Chat tab UI** — a consent-gated composer that streams the plan, live
  subagent-activity chips, and the final answer over Server-Sent Events; the SSE
  rides the intent-guarded `POST /chat` (a `GET` `EventSource` cannot carry the
  CSRF intent header) under the unchanged self-only CSP, with a no-JS buffered
  fallback and Clear-history.
- **Token-by-token answer streaming** — the Synthesizer's answer now types out live,
  token by token (`answer_delta` SSE events), reconciling to the authoritative final
  answer when the turn completes.
- **`[chat]` configuration** — provider (Anthropic native / OpenAI-compatible,
  default OpenRouter), model, budget-tree params, and sampling, with the API key in
  a `0600` `secrets.toml` that is masked and never echoed. The Config tab gives the
  provider, model, and base_url their own typed controls (a provider select + a
  model input + a base_url input, patched into the validated raw-TOML candidate
  like every other typed field, in a full-width `[chat]` fieldset), so the settings
  that gate whether Chat is usable — and the endpoint it talks to — are discoverable
  rather than hidden in the raw pane; the remaining `[chat]` keys stay in the raw pane.
- **Multi-step agent memory** — per-thread scratchpad + working memory in
  `.logos/chat.db`, with the Synthesizer grounded on the persisted scratchpad.

### Security / carve-out
- The entire chat stack is gated behind the non-default `ui` feature: the default
  binary links **no** networking or LLM crate and stays byte-identical to 0.7.6
  (the `no_network_deps` fitness function and three further carve-out guards hold).
  The first and only outbound egress is the explicit, consent-gated chat turn.

## [0.2.0] — 2026-06-14

Language-breadth release — the first 0.x increment to carry new capability
rather than a pure dogfood re-pin. The default binary now indexes **twelve**
programming languages out of the box (up from five), and three resolution/config
correctness fixes land. The self-dogfood pin advances to this version.

- **Feature — seven more languages in the default build (CR-009).** Kotlin, C,
  C#, C++, Ruby, PHP, and Scala join the out-of-the-box code-language set, taking
  it from five to twelve (`logos languages` now lists 24 grammar rows including
  artifacts). Each ships as pure plugin data — one grammar crate, one feature
  line, one `plugins/<lang>/` descriptor — riding the existing plugin substrate
  and the load-time ABI assertion (ADR-09) without touching core extraction logic
  (NFR-MA-01). The stripped default binary stays within the NFR-PC-04 ≤ 50 MB
  budget. The NFR-PE-05 cold-start budget is revised 200 → 500 ms to reflect the
  larger grammar set compiled at registry load.

- **Fix — reconcile-purge demotes inbound references (CR-017 Defect A).** When a
  config change narrows the indexed set and purges a file, references from
  still-indexed code that resolved to the purged file are now correctly returned
  to unresolved instead of keeping a stale resolved row (NFR-RA-05 honesty).
  Surfaced by 0.1.2's incremental resolution, whose change-delta did not see the
  out-of-band purge.

- **Fix — OpenAPI operations bind to their framework routes (CR-017 Defect B).**
  An `ApiOperation` reference now resolves to the route node the framework pass
  promotes, producing the operation → route edges that previously came out empty
  on a no-op sync. Same incremental-resolution interaction as Defect A: the route
  is promoted after the resolve pass, so a focused re-resolve over the newly
  promoted names is now run.

- **Fix — the `languages` config field gates indexing (CR-017 Defect C).**
  Previously inert (it fed only the admission fingerprint), `languages` now
  restricts which code grammars are admitted: omitted or empty means all
  compiled-in languages (preserving the twelve-language default), a non-empty
  list narrows, and narrowing purges the dropped languages' files.

## [0.1.2] — 2026-06-13

Dogfooding bugfix release. Re-pins the self-dogfood binary. Two resolution-engine
performance fixes that, together with 0.1.1's watcher-exclusion fix, eliminate the
CPU melt that had forced dev-pane Logos off.

- **Fix — `serve --mcp` whole-ledger re-resolution under churn (CR-015).** The
  watcher-fired `Engine::sync` re-bound the *entire* reference ledger (~40k rows)
  on every sync via an all-core parallel pass, even when nothing relevant changed
  — N concurrent panes saturated the machine. Resolution is now **incremental**:
  a sync re-binds only the change-affected rows (the changed files' rows plus the
  untouched rows whose target token a change moved), and the watcher coalesces
  bursts via a settle window + rate-limit floor + staleness cap. A no-op sync on
  the self-graph drops ~152 s → ~1.6 s. Guarded by a sync≡reindex equivalence net.

- **Fix — cold `logos index` exponential glob resolution (CR-016).** The binder's
  `through_globs` resolved each glob import's own module path by re-entering
  `through_globs`, so a file with `G` glob imports did `O(G^8)` work per reference;
  a file with 14 `use super::*` imports drove a single bind to ~1.5e9 operations
  (~150 s) and, in parallel, pegged every core. A re-entrancy guard collapses this
  to `O(G)`. Cold self-index ~7 min → **3.8 s**, restoring the NFR-PE-02 budget.
  Edge output is byte-identical (verified by the equivalence net + exhaustive glob
  classification).

## [0.1.1] — 2026-06-12

Dogfooding bugfix release. Re-pins the self-dogfood binary.

- **Fix — `serve --mcp` watcher CPU storm.** The hosted filesystem watcher
  excluded only `.logos`/`.git`, so build-output churn under indexer-ignored
  directories (`target/`, `node_modules/`, `dist/`, `build/`, `vendor/`) flooded
  the debounced sync worker — a single `cargo build` drove `Engine::sync` across
  every core, and N concurrent dev panes saturated the machine. The watcher now
  drops events under the same `ignored_dirs` set the indexer prunes (unioned with
  the always-excluded internal dirs, so feedback-loop containment is not
  configurable away). The watched set now matches what indexing admits; real
  source edits still sync. Regression-tested in `logos-core/src/watch`.

## [0.1.0] — 2026-06-11

First tagged release and the baseline pin for self-dogfooding.

Logos at 0.1.0 is a single static binary providing structural code intelligence
for AI-assisted development: deterministic, offline, never-fabricating. Headless
CLI plus a stdio MCP server (`logos serve --mcp`).

Capabilities in this release:

- **Code graph** — multi-language extraction (Rust, Python, TypeScript, Go,
  Java, Kotlin, Swift, Ruby, PHP, C#, C/C++, Scala, …), SCIP-conformant data
  model, resolution, annotation, and the eight navigation tools
  (`search`/`query`/`context`/`callers`/`callees`/`impact`/`affected`/`explore`).
- **Quality metrics & governance** — full architecture-quality `scan`, the
  versioned-baseline `gate` (CI regression check), `check` against `rules.toml`,
  test-aware and production-scope metrics.
- **Extended analytics** — structural metrics and near-clone detection,
  git-history temporal metrics and `hotspots`, external `coverage` ingestion,
  `dsm`, `evolution`, `test-gaps`, `doc-gaps`.
- **Documentation graph** — markdown doc nodes, doc↔code link resolution, and
  doc-aware traceability (`implements`, `referencing-docs`).
- **Config & artifact graph layer** — an `artifact = true` plugin class over 10
  artifact grammars (YAML/JSON/TOML, Dockerfile/Makefile/Shell,
  Protobuf/GraphQL, Terraform/SQL) with content-sniffed OpenAPI promotion;
  metric-neutral by construction.
- **Setup & integration** — `logos init` (self-contained: `.logos/` config +
  rules, optional git hooks for automatic freshness, `.mcp.json` for any MCP
  host).

[0.2.0]: https://github.com/ — local tag `v0.2.0`
[0.1.2]: https://github.com/ — local tag `v0.1.2`
[0.1.1]: https://github.com/ — local tag `v0.1.1`
[0.1.0]: https://github.com/ — local tag `v0.1.0`
