# Security

Logos does not find vulnerabilities itself. It runs the security scanners you
already trust (or ingests their reports), stores what they find next to the code
graph, and then answers the questions a scanner cannot: which function contains
each finding, whether a route can reach it, how much depends on it, and whether a
merge should be blocked. This page is the guide to that section; the command and
configuration references it links to hold every flag and payload.

- [What you get](#what-you-get)
- [Logos makes no network connection](#logos-makes-no-network-connection)
- [The nine scanners](#the-nine-scanners)
- [Installing scanners](#installing-scanners)
- [Consent](#consent)
- [Running scans and reading the results](#running-scans-and-reading-the-results)
- [Re-scanning a few files](#re-scanning-a-few-files)
- [What reachability confidence means](#what-reachability-confidence-means)
- [Gating a merge: the `[security]` table](#gating-a-merge-the-security-table)
- [Accepting a risk: the ignore file and UI triage](#accepting-a-risk-the-ignore-file-and-ui-triage)
- [Reviewing what a branch introduced](#reviewing-what-a-branch-introduced)
- [The Security page](#the-security-page)
- [From an agent: the MCP tools](#from-an-agent-the-mcp-tools)
- [When something goes wrong](#when-something-goes-wrong)

## What you get

```bash
logos security scan --install --consent   # install what is missing, record consent, scan
logos security findings --severity high   # what was found, with OWASP categories
logos security status                     # each scanner: fresh, stale, never run, not installed
```

Findings are evidence in `.logos/history.db`, anchored to the commit (HEAD), the
scanner and the file the scanner named. They are **advisory** until you add a
[`[security]` table](#gating-a-merge-the-security-table) to `.logos/rules.toml`;
without it `check` and `gate` print exactly what they printed before, whether
findings are stored, absent or deleted. Either way the 0–10000 quality signal
never moves: security is not a metric dimension. No source snippet is stored, and
a secret-class finding's message is replaced before it is written.

## Logos makes no network connection

Logos itself makes no network connection. The default build links no HTTP client
(a fitness check over its dependency tree keeps it so), and the slim build links
no socket stack at all. `serve --ui` listens on `127.0.0.1` and never dials. Only
two things reach the network, each a separate program that Logos starts, and
neither without your consent:

1. **A scanner you consented to** may reach the network to refresh its rules or
   vulnerability data: Semgrep's registry rules, the Trivy and OSV-Scanner
   databases, Dependency-Check's NVD feed. Consent names the scanners and is kept
   per project, outside the repository.
2. **`logos security install`** runs your system `curl` with a fixed argument list
   against the sources pinned in the Logos release, and verifies each download's
   SHA-256 before it is used. The Python scanners install through a managed `uv`
   that fetches pinned, hash-checked packages from `pypi.org` and the Python
   runtime itself from `github.com`. Consent is per user
   and per version, and names each source and download size first.

Nothing downloads in the background, and no scan starts except through a command,
the Security page's **Run scan** or an MCP tool call, each after consent; the
`serve` watcher only reads a `.sarif` file written into `.logos/security/`.

**What scan consent allows.** It lets the scanners it names, started by Logos with
the command line fixed in the release, reach the network to refresh their rules or
vulnerability databases. It does not let a repository choose a command, a binary
or a flag (those come from the built-in registry and your user-level config only),
and Logos itself sends nothing anywhere. For Semgrep and Trivy, Logos sets
their update-check opt-outs and Semgrep's metrics opt-out (and passes
`--metrics off` to Semgrep); the other scanners get no opt-out from Logos. What a
scanner transmits is the scanner's own behaviour, which Logos can neither see nor
limit. If that matters to you, do not consent: run the scanner in your own
CI and `logos security ingest` its SARIF report, which reaches no network.

## The nine scanners

Logos registers nine scanners, each with a fixed SARIF command line. A scanner
runs only if it is installed and not switched off with `enabled = false` under
`[security_scan.<scanner>]` in `.logos/config.toml`; that is the only key a
repository may set there ([configuration.md](configuration.md#security_scan--which-scanners-may-run)).

<!-- doc-test: tested-versions -->

| Scanner | Tested | Installed by Logos | Looks for | Re-scan of some paths |
|---|---|---|---|---|
| `semgrep` | 1.85 | 1.85.0 | Insecure code patterns | yes |
| `trivy` | 0.74 | 0.74.0 | Vulnerable dependencies, secrets, misconfiguration | whole manifest only |
| `gitleaks` | 8.18 | 8.18.4 | Secrets, in files and in git history | yes, in content mode |
| `osv-scanner` | 1.9 | 1.9.2 | Known-vulnerable dependencies | whole manifest only |
| `bandit` | 1.7.10 | 1.7.10 | Insecure Python code | yes |
| `gosec` | 2.21 | 2.21.4 | Insecure Go code | yes |
| `checkov` | 3.2 | 3.2.533 | Infrastructure-as-code misconfiguration | yes |
| `codeql` | 2.19 | 2.27.2, opt-in | Semantic code analysis | whole project only |
| `dependency-check` | 13.0 | 13.0.0, opt-in | Known-vulnerable dependencies (NVD) | whole manifest only |

*Tested* is the version each command line was written or last verified against
(Dependency-Check's was written against 10.0 and verified at 13.0.0, which is
also the oldest Logos accepts); a release whose flags have since changed reports
`failed` with the scanner's own error rather than guessing. *Installed by Logos*
is the version this Logos release pins and `security install` fetches. *opt-in*
means Logos installs that scanner only when you name it
([Installing scanners](#installing-scanners)); until then, put it on your `PATH`
or name it in your user-level config and Logos uses it. A scanner that is
already on your `PATH` at or above the tested version is used as it is. CodeQL
runs only on JavaScript/TypeScript, Python and Ruby and never builds your
project; for every other language ingest a report from CI ([configuration.md](configuration.md#security_scan--which-scanners-may-run)
has the fixed command lines and the config files Logos refuses to let a repository
use to run code).

## Installing scanners

Install the scanners Logos can install with one command, or let a scan do it:

```bash
logos security install --all                  # exit 2: lists what would be installed and downloads nothing
logos security install --all --consent        # record the consent, then install
logos security install trivy gosec --consent  # only these
logos security install trivy --update --consent   # replace an older managed copy
logos security install --all --from ~/Downloads/logos-tools --consent   # offline, from pre-downloaded artifacts
logos security uninstall --all                # remove the managed copies, never your own
logos security scan --install --consent       # install what is missing, then scan
logos security install dependency-check --consent                 # opt-in, with its own Java runtime
logos security install codeql --accept-codeql-licence --consent   # opt-in, once you accept its licence
```

Everything goes into the user-level tools directory (`$LOGOS_DATA_HOME`, else
`$XDG_DATA_HOME/logos/tools`, else `~/.local/share/logos/tools`), never inside a
repository.

- **Your own copy wins.** A scanner is installed only when none is usable: a
  user-level `path` override, or a `PATH` copy at or above the tested version, is
  kept and never downloaded. A `PATH` copy below the tested version is installed
  over for Logos's use, and your copy is never removed. `logos security uninstall`
  removes only what Logos installed.
- **Dependency-Check and CodeQL install only on explicit opt-in.** `--all` and
  `security scan --install` never install them; name them. Dependency-Check
  brings a pinned Java runtime of its own. CodeQL installs only once you accept
  GitHub's CodeQL terms with `--accept-codeql-licence`, recorded for the pinned
  version; until you have, the command exits 2 showing the terms, and a release
  that pins a new version asks again.
- **Nothing updates by itself.** When this release pins a newer version than your
  managed copy, `security status` shows `update_available` and the older copy
  keeps running until you run `security install <scanner> --update`.
- **Where a missing scanner is named.** `security status` carries, for each scanner
  Logos can install, the command that installs it, and so does the Security page
  beside the scanner. The MCP `security_status` names it too, but no MCP tool
  installs.
- **Windows** has no managed installs yet: put the scanners on your `PATH`.

### From the Security page

Under `logos serve --ui`, the [Security page](#the-security-page) installs the
scanners without a terminal:

- **Install & scan** takes the place of **Run scan** while any of the seven
  default scanners is missing. One click installs the missing ones and then runs
  the full scan, in one background job: what `logos security scan --install --consent`
  does on the command line.
- A *not installed* row that Logos can install offers **Install**. The **Copy**
  column says which copy runs: `managed` (installed by Logos), `path` (your own
  copy on `PATH`) or `override` (your user-level `path` setting). A `managed` copy
  older than this release's pin reads `update available` and offers **Update**,
  which installs the pinned version and removes the older copy.
- **One consent dialog** comes first while anything is awaiting consent. It names
  each tool with its version, the host it is downloaded from and its size, and any
  support install a tool needs (`uv` and the managed Python for Semgrep, Bandit and
  Checkov). For Install & scan it also names the scanners whose network consent
  this project has not given. Accepting records both consents exactly as
  `--consent` does; declining downloads and records nothing.
- **Dependency-Check and CodeQL are opt-in lines, unticked.** Dependency-Check's
  line states the Java runtime it brings and its size. CodeQL's line shows its
  licence terms, and accepting them is a checkbox of its own: CodeQL does not
  install until it is ticked. Left unticked, neither is installed.
- The panel lists each tool as it installs, then each scanner as it runs.
  **Cancel install** stops the download in progress: no partial tool is left in
  the tools directory, no further tool starts, and the panel names the tools that
  were installed before the cancel. While an install runs for the project, a
  second install or a scan is refused rather than joined, and an install does
  not start while a scan runs.

The page installs through the same pins, checksums and tools directory as the
command line, and its two routes (`/api/v1/security/install` and
`/api/v1/security/install/cancel`) are intent-guarded `POST`s like Run scan's: a
cross-origin page can neither install a scanner nor stop an install.

The full flags, the payload and the failure stages (`download`, `checksum`,
`unpack`, `install`) are in [commands.md](commands.md#security-install-scanner--all---update---from-dir---consent---accept-codeql-licence).

## Consent

There are two consents, and each is yours to give; an MCP client cannot give
either. Both can be given on the command line or in the Security page's consent
dialog.

| Consent | Covers | Kept in | Asked again when |
|---|---|---|---|
| Scan | The scanners named in it may reach the network to refresh rules or databases | `security-consent.toml`, user-level, keyed by the project's root path | A scanner is installed or enabled later: it is asked for alone |
| Install | Downloading and installing each tool at its pinned version, in every project | `security-install-consent.toml`, user-level | A release pins a new version of a tool |

Without consent the command exits 2, names what is awaiting it and the flag that
records it, and starts nothing, not even a version probe. `logos security status`
reports the consent state: the scanners consented to and the ones the next scan
will ask for. To withdraw scan consent, delete the project's table from
`security-consent.toml` (or the file). Where the files live and the exact wording
are in [configuration.md](configuration.md#scan-consent).

## Running scans and reading the results

```bash
logos security scan --consent                  # the first scan in a project, then
logos security scan                            # every later one
logos security ingest ci/codeql.sarif          # a SARIF 2.1.0 report from any tool, run anywhere
logos security status                          # one state per scanner
logos security findings --severity high --suppression active
logos security findings --scanner semgrep --path src/api   # the filters intersect
logos security findings --category A05 --edition 2021      # OWASP Top 10:2021 numbering
```

`security scan` runs every installed, enabled scanner, writes each SARIF report to
`.logos/security/` (gitignored: a report can quote a secret) and ingests it as it
finishes. One scanner failing never stops the others, and a failed scanner's
previous results are kept. `security ingest` validates the whole report before it
writes anything, binds each path to an indexed file by longest unique suffix, and
lists a path it cannot bind as `unmatched` rather than guessing; ingesting the same
bytes at the same HEAD again writes nothing.

**What a scan covers.** A scan reads the project, not what it ignores, for the
scanners that walk its directories. Logos finds the top-most directories the
project's own ignore files exclude: the `.gitignore` and `.ignore` files at the
root and below it, and `.git/info/exclude`, but not your global git excludes or
an ignore file above the root. It then gives them to Bandit, Trivy, Checkov and
Dependency-Check through each one's exclusion flag, besides `.logos/`. So those
four do not scan a build directory (`target/`, `node_modules/`, `dist/`), a notes
folder or a vendored copy you ignore, and Trivy no longer spends its whole
timeout walking `target/`. The others work by their own rules:

- Semgrep and OSV-Scanner skip what *git* ignores, by themselves, in a git
  repository. A directory only `.ignore` names is still scanned by both, and
  so is everything in a project that is not a git repository. Semgrep also
  honours your global git excludes.
- Gitleaks reads committed history, which an ignored file never enters.
- gosec and CodeQL still read ignored directories: neither has an exclusion
  flag that would skip exactly those directories. CodeQL only analyses
  languages found in files the project does not ignore.

The edges for the four scanners Logos gives the list to:

- A file you ignore *by name* in a directory that is not ignored (`*.log`,
  `.env`) is still read by the directory-walking scanners. Only whole
  directories are passed.
- A file committed inside an ignored directory (added with `git add -f`) is
  skipped by Bandit, Trivy, Checkov and Dependency-Check with the rest of that
  directory. Semgrep still scans it, because it is tracked, and Gitleaks reads
  it in history.
- An ignored directory whose name holds a comma, a double quote, a glob
  character, a backslash or a control character, or starts with `-`, is scanned
  as before, because an exclusion flag could misread it.
- The list a scanner is given is capped at 64 KiB of arguments, so that every
  scanner still starts when a project has thousands of ignored directories,
  such as a `__pycache__` in every package. Past the cap, the deepest
  directories are scanned as before, and the log says how many.

A re-scan of named paths (`--path`, next section) reads exactly those paths, even
inside an ignored directory. The scanners that cannot be narrowed to paths run
over the whole project there too, and they leave out the same directories.

`security status` gives each scanner one state:

| State | Meaning |
|---|---|
| `not_installed` | No binary was found and no report is stored |
| `disabled` | Switched off in `[security_scan]` |
| `never_run` | Installed, and no report is stored |
| `stale` | A report is stored but HEAD has moved (`head_moved`), a file it anchors changed (`files_changed`), or only re-scans of some paths stand behind it (`scoped_only`) |
| `fresh` | A report is stored at this HEAD over unchanged files; a clean scan is `fresh` with `findings: 0` |

A scanner that has not run has looked at nothing, so its silence is never a clean
result: `never_run` carries no count, and every list says what it was drawn from.
`security findings` returns the stored findings with their OWASP Top 10 categories
(2025, or 2021 with `--edition`, reclassified without a re-ingest), the filters
`--severity`, `--scanner`, `--category`, `--path` and `--suppression`, and a
`categories` view that marks a category `not_covered`, never `0`, when no scanner
whose report is stored can detect it. In a workspace, `--repo <MEMBER>` on
`status` and `findings` answers for one member.

Each finding also carries its place in the graph, described next. The payloads and
every flag are in [commands.md](commands.md#security-findings).

## Re-scanning a few files

A full scan can take minutes; proving that a fix worked should not. Narrow a scan
to one scanner and some paths:

```bash
logos security scan --scanner semgrep --path src/a.py             # one scanner, one file
logos security scan --scanner semgrep --scanner bandit --path src/api --path src/util.py
```

- `--scanner <NAME>` (repeatable) runs only the named scanners; the rest read
  `not_selected`. `--path <PATH>` (repeatable) is a file or directory inside the
  project, literal (no globs). A path outside the root, one that does not exist,
  the root itself, or one starting with `-` is refused with exit 2 before anything
  starts. `--install` takes neither flag.
- **Semgrep, Gitleaks, Bandit, gosec and Checkov** run on the paths only, and the
  ingest replaces that scanner's findings **in those paths only** (Gitleaks keeps
  them, below): every other file's findings and freshness stay as they were.
- **Trivy, OSV-Scanner and Dependency-Check** read whole manifests and **CodeQL** a
  whole database, so none can be narrowed. Each reports its `scope` as
  `whole-manifest only` (`whole-project only` for CodeQL); named with `--scanner`
  it runs on the whole project, and otherwise a `--path` scan leaves it out.
- **Gitleaks reads content, not history, in a scoped run.** A full scan reads the
  git history of the repository; a scoped run reads the named files as they are
  now (`--no-git`). Those are different evidence, so a scoped Gitleaks run does not
  replace the full scan's findings in those paths: a secret removed from a file is
  still in history and stays listed. A secret the scoped run finds that matches a
  recorded one (same file, same rule, same position, or the same column span when
  only its lines moved and no other finding shares it) keeps that finding's
  fingerprint, so `check`, `gate` and the ignore file still recognise it; only a
  secret with no match reads as new.
- **A re-scan never makes a scanner read `fresh` for the rest.** A scanner whose
  only report is from `--path` scans reads `stale` with `scoped_only`, and one
  whose full scan is from an earlier commit reads `stale` with `head_moved`, so a
  required scan is satisfied only by a full scan at HEAD.

## What reachability confidence means

For each finding Logos names the **enclosing symbol** (the innermost function or
type whose span holds the finding's line), whether a **route** reaches it over
call edges, and its **blast radius** (the sizes of `logos impact` on that symbol:
how many symbols depend on it, how many it depends on, how many documentation
sections reference it). These sit in the `context` of each finding in
`logos security findings --json`, and in the Security page's finding detail.

Every claim carries a confidence, so a reader can tell what the graph knows from
what it could not see:

| Confidence | Means |
|---|---|
| `resolved` | The graph binds calls across files for the enclosing file's language, so the claim rests on resolved edges |
| `unresolved` | The language has no cross-file call resolution (or none was recorded), so the claim is the most the graph can say, not a measurement |
| `unbound` | There is no enclosing symbol to make the claim about |

An `unbound` finding says why: it has no location, its file is not indexed (a
lockfile, which is excluded by default, keeps its manifest line), its file changed
since the scan (a stale finding is never bound, because its line may now point at
another symbol), or its line is in no code symbol.

The reachability **verdict** is `reachable`, `unreachable` or `unknown`:

- `reachable` names the nearest route and how many calls away the symbol is.
- `unreachable` is claimed only when the graph could have seen every way in: its
  language binds calls across files, a route exists to walk from, and nothing
  reaches the symbol other than by a call.
- `unknown` means none was found and none could be ruled out, and it says which
  way in the graph could not see: the file's language binds no call across files;
  the graph holds no route at all (a framework it does not recognise); a route
  names no handler the graph proved; the symbol is a member of a type reached
  through its supertype's dispatch; or the symbol is referenced other than by a
  call (passed as a value, or as a function pointer). **`unknown` is never
  rendered as unreachable**, on the CLI, in the MCP payload or on the page.

**A limit to read `unreachable` against.** The walk follows calls and handoffs the
graph records. A function passed by value with no handoff marker, for example
Rust's `iter().map(db::render)`, is a way in the graph cannot see, so a symbol that
is only reached like that can read `unreachable`. Take `unreachable` as "no route
reaches it by any call the graph resolved", the strongest claim the graph makes
and not a proof, and check a high finding that reads it before you rank it last.

The blast radius has its own confidence: a symbol behind dispatch is `unresolved`,
because its callers through the supertype are counted against the supertype's
member. The `resolution` row on each finding is the per-language denominator the
labels were read from, so you can see how much of that language's calls bind.

## Gating a merge: the `[security]` table

Add a `[security]` table to `.logos/rules.toml` and `check`, `gate` and
`session-end` judge security findings as they judge a broken rule. This example
requires two fresh scans, ratchets new high findings, and caps critical findings
and two OWASP categories:

<!-- doc-test: security-example -->

```toml
[security]
required_scanners = ["semgrep", "gitleaks"]
ratchet_min_severity = "high"
max_severity = "critical"
fail_on_owasp = ["A04", "A01:2021"]
```

| Key | Default | Fails when |
|---|---|---|
| `required_scanners` | none | A scanner named here has no fresh scan at HEAD: it never ran, is stale, is not installed or is disabled, or its name matches no scanner (`unknown`). Exit 5 |
| `ratchet_min_severity` | `high` | An unsuppressed finding at or above it is **new**: its fingerprint is not in the set `logos gate --save` recorded. Rule `security:ratchet` |
| `max_severity` | unset | Any unsuppressed finding at or above it exists, recorded or not. Rule `security:max_severity` |
| `fail_on_owasp` | none | Any unsuppressed finding is in one of these OWASP Top 10 categories. `A04` is the 2025 edition; `A01:2021` names the older one. Rule `security:fail_on_owasp:<id>` |

**The ratchet adopts the gate on a codebase that already has findings.** Scan, then
record what is there; after that only a finding that is new since the save fails:

```bash
logos security scan --consent   # fresh scans of the required scanners at HEAD
logos gate --save               # records the metric baseline and every current finding's fingerprint
logos check                     # now fails only on what is new, or over a cap
```

Until a save is recorded, every finding at or above `ratchet_min_severity` is new,
and the report says to run the save. The recorded set lives in `.logos/history.db`
with the findings; `session_start` records it too. A suppressed finding is not
recorded, so when its suppression expires or is removed it is new again. The
**caps** ignore the recorded set: `max_severity = "critical"` fails on any
unsuppressed critical finding, however old.

**Exit 5 means the evidence is missing, not that a rule failed.** `check`, `gate`
and `session-end` exit **5** when a scanner in `required_scanners` has no fresh scan
at HEAD, naming the scanner and the command that produces the scan. The gate never
runs a scanner itself. Exit 4 (no rules contract) takes precedence over 5, and 5
outranks 1: the JSON still lists any violations. A scanner the table does not name
never causes exit 5; if its stored scan is stale it is simply not evaluated, and a
note says so. A `gate --save` that exits 5 saves the metric baseline but not the
security one. An invalid table or an invalid ignore file is a usage error (exit 2).

Adding, changing or removing the table moves no metric and triggers no
re-baseline. The key reference is
[configuration.md](configuration.md#security--the-security-gate), and the
exit-code contract is in [error-handling.md](error-handling.md#how-it-surfaces-on-the-cli).

## Accepting a risk: the ignore file and UI triage

Accepted risk is recorded in `.logos/security-ignore.toml`, a file you commit, one
entry per finding:

```toml
[[ignore]]
fingerprint = "<the finding's 64-character fingerprint>"
reason = "test fixture, never deployed"
expires = 2026-12-31          # optional; suppresses through this day inclusive
```

- The fingerprint is the one `logos security findings --json` shows for the
  finding. **`reason` is required**; a missing reason, a malformed fingerprint or
  date, or a repeated fingerprint rejects the whole file with exit 2, naming the
  line. A suppression that silently did nothing would be worse than none.
- A suppressed finding is **never hidden**: `security findings` still returns it
  with its reason (`--suppression active` filters it out), and it is excluded from
  every `[security]` rule and from the counts of `status`, `hotspots` and the
  workspace roll-up.
- From the day after `expires` the entry stops suppressing and is listed as
  `expired`; an entry that matches no stored finding is listed as `orphaned`.

You do not have to edit the file by hand. On the Security page, **Suppress…** on a
finding opens a dialog that asks for the reason (required) and an expiry date
(optional) and writes the entry to `.logos/security-ignore.toml`; **Unsuppress**
removes it. Because the file is committed, the acceptance is reviewed like code.
The page writes the file in one canonical spelling (one `[[ignore]]` table per
entry, sorted by fingerprint, no comments), so a removal restores it byte for
byte. A file you spelled by hand differently, with a comment like the `expires`
one above for instance, is refused rather than rewritten, because rewriting it
would drop your comments: edit that file by hand, or remove the comments first.
The reference is [configuration.md](configuration.md#logossecurity-ignoretoml--suppressions).

## Reviewing what a branch introduced

A scan lists every finding in the tree. A review asks which of them this branch
put there, and what to do about each. Three commands carry it, and you can run
each one yourself; the plugin's `/logos:sec-review` skill
([plugin.md](plugin.md#the-security-review-skill)) runs them for an agent:

```bash
logos security scan --consent                           # a full scan: the evidence the delta reads
logos review --base main --json                         # the delta; the findings are under sections.security
logos security scan --scanner semgrep --path src/a.py   # prove a fix: the same scanner, the changed file
```

### The security section of `review --base`

`logos review --base <ref>` compares the working tree with the merge-base of
`HEAD` and the base ([commands.md](commands.md#review---base-ref)). Its
`sections.security` classifies every unsuppressed finding of each scanner's latest
stored report:

<!-- doc-test: review-classes -->

| Class | Means |
|---|---|
| `introduced` | The branch added it: its fingerprint is not in the scanner's report recorded at the merge-base (method `fingerprint`), or, when no such report exists, it lies inside a hunk the branch changed (method `hunk`) |
| `touched` | In the merge-base report, and inside a hunk the branch changed. Only the `fingerprint` method can tell |
| `resolved` | In the merge-base report, absent from the latest one, on a file that report saw as it stands now |
| `stale` | As `resolved`, but the file was edited after the latest report. It is never read as resolved |
| `pre-existing` | Anything else |

- **The merge-base tree is never scanned.** The before-state is a report a scan
  recorded at the merge-base commit, and nothing else. A scanner with none is
  compared by hunks, which include uncommitted and untracked files, so it cannot
  report a `touched`, a `resolved` or a `stale` entry. Each scanner's method is
  listed under `scanners`. When `HEAD` is the merge-base, a scan run now replaces
  the report recorded there; commit before re-scanning, or the comparison is only
  as old as the last scan. A `--path` re-scan there (the Stop hook's, before the
  branch's first commit) records what the working tree held, not the commit, so
  that report is no before-state: its scanner is compared by hunks, and `notes`
  says so.
- **A finding whose line moved is not new.** A report with no stable fingerprint
  keys a finding by its region, so an edit above it moves the fingerprint. An
  entry outside every changed hunk with a new fingerprint is paired with the
  vanished merge-base finding of the same scanner, rule and file, reads
  `pre-existing`, and a note says so.
- **Each entry** carries its class, its method, the scanner, the fingerprint, the
  rule, the severity, the OWASP categories, the file and line, its freshness, and
  the reachability verdict and confidence of
  [What reachability confidence means](#what-reachability-confidence-means). A
  secret-class finding's message is the redacted text stored at ingest, and no
  source snippet exists to print.
- **Suppressed findings are listed apart**, under `suppressed`, with the ignore
  entry's reason and expiry, and counted apart. They are never judged.
- **A project never scanned reads `measured: false`** with `not_measured_because`,
  and its lists are empty and mean nothing. The quality half of the review is
  unaffected, and so is `review`'s exit code: the section is advisory and gates
  nothing. The MCP `logos:review` tool returns the same bytes.

### The five verdicts

The skill gives every `introduced` or `touched` entry exactly one verdict. The
others are counted and not judged: findings the branch did not touch, `resolved`
and `stale` ones, and suppressed ones.

<!-- doc-test: review-verdicts -->

| Verdict | The finding | The agent | Edited? |
|---|---|---|---|
| `fix` | A true positive with `resolved` reachability confidence that is neither secret-class nor a dependency finding | Edits it, then proves the edit by re-scan | yes |
| `escalate-secret` | Secret-class | Reports "rotate the credential, and the value remains in git history". The entry is never closed as fixed | no |
| `escalate-dependency` | A dependency finding | Proposes the version change with its changelog risk and blast radius, and you approve | no |
| `false-positive` | Judged not exploitable in this occurrence | Drafts an ignore-file entry with a reason; writing it is held by the guardrail | no |
| `untrusted` | Reachability confidence `unresolved` or `unbound` (also: no bound file, or a file edited since the scan) | Reports it | never |

What a verdict may be rests on the finding, not on the agent's opinion of the
code. A script sets `escalate-secret`, `escalate-dependency` and `untrusted` from
the finding's own class and confidence, and the agent may not change them; it
confirms a proposed `fix` or downgrades it to `false-positive`. It may never move
anything else to `fix`: a secret, a dependency or an untrusted finding does not
become editable because the code looks wrong.

### What an agent may close, and what it may not

- **It may close a `fix`, and only by proof.** After a targeted re-scan of the
  same scanner over the changed paths ([Re-scanning a few files](#re-scanning-a-few-files)),
  all three hold: the fingerprint is absent, no new finding stands at or above the
  original severity, and the tests pass, with their count. A scanner that cannot
  be narrowed (Trivy, OSV-Scanner, Dependency-Check, CodeQL) runs on the whole
  project instead. Otherwise the report says `UNPROVEN` and the edit is undone.
  Deleting the code that held a finding is not a fix.
- **It may not close a secret.** The value stays in every earlier commit, and a
  Gitleaks scan of the file alone cannot show that. The agent does not edit the
  file under this skill; removing the value is a change you ask for, and it still
  closes nothing. A secret is never re-scanned as proof and never reported fixed.
- **It may not decide a dependency or an untrusted finding.** It proposes or
  reports, and edits nothing.
- **It may not suppress.** It drafts the ignore entry and stops at the guardrail's
  question; the file is yours to accept ([Accepting a risk](#accepting-a-risk-the-ignore-file-and-ui-triage)).
- **It never commits, pushes or blesses a baseline, records scan consent, or
  installs a scanner.** If a scan needs consent, it asks you to run
  `logos security scan --consent`.

**A clean review is not a claim that the code is secure.** The report names the
scanners that ran, their versions and the paths, and lists the OWASP categories
that no enabled scanner with a stored report covers, so none could have found anything in them. It
never says the code is secure, whatever the findings look like: an empty delta, a
clean re-scan and a list of proven fixes still leave the uncovered categories
uncovered and everything a scanner cannot see unseen. A script refuses any report
that does.

### In the Stop hook

With the plugin installed, the Stop hook adds a security leg after the quality
legs ([plugin.md](plugin.md#the-stop-check)). It scans only the files the turn
changed, with Gitleaks and, when installed and consented, Semgrep, and blocks the
stop once on a new secret or a new finding at `high` or above, pointing to
`/logos:sec-review`. **It needs scan consent already recorded for the project.**
It never asks for consent and never records it: without it the hook scans nothing,
blocks nothing and says "security not measured: no consent". Give the consent
yourself with `logos security scan --consent`.

## The Security page

`logos serve --ui` adds a **Security** page (`/security`) in the *analyse* group,
beside Files & Risk, Rule findings and Coverage. It answers for the selected member; every read is
a `GET`, and the actions are the intent-guarded `POST`s the dashboard already
allows for scans and suppression.

- **The verdict first**: the finding count, and, when `rules.toml` has a
  `[security]` table, the gate band with the required scanners and the findings its
  rules fail on.
- **Scanners**: the state of each of the nine, with the command that fixes a missing
  one (`logos security install …`, `logos security scan --consent`, or
  `logos security ingest <report>`). Its **Install** and **Update** buttons, and
  **Install & scan**, install from the page itself
  ([From the Security page](#from-the-security-page)).
  The **Copy** column says which copy runs. The page reads a scanner's version
  only once the project's scan consent names it, and then once per `logos serve`
  for each binary (again after a reinstall). Until then the row's `installed`
  says `"unprobed": "not_consented"` and its version is unknown: the Version
  column reads `—` unless a stored report gives one. Without that version the
  page cannot tell whether a `PATH` copy meets the minimum, so where a managed
  copy is installed too, the Copy column names the managed one until consent
  lets the page read the `PATH` copy's version. Opening the page, from any
  site, starts no scanner process you have not consented to.
- **Run scan** starts a scan in the background, shows each scanner as it starts and
  finishes, and **Cancel scan** stops every scanner process at once and keeps the
  previous results. A click while a scan runs joins it. The first scan asks consent
  in a dialog that names the scanners, and records it exactly as
  `security scan --consent` does.
- **Findings**, filtered by severity, scanner, category and path; selecting one
  opens its graph detail (enclosing symbol, route, blast radius, each with its
  confidence) and the triage actions above.
- **OWASP Top 10**, 2025 or 2021, with each category `covered` or `not covered`.

In a workspace, a **Security** page at `/workspace-security` rolls up every member:
each member's fresh scanners and its fresh, unsuppressed findings by severity,
linked to that member's own page, and the OWASP view summed over the members read.
Every total says how many members it counts: a member never scanned reads
`never run`, not `0`, and a stale scan or a suppressed finding is left out and
counted as left out. The page starts no scan; open a member's page, or run
`logos security scan` in that member.

Elsewhere, `logos hotspots --with-findings` ranks files with a fresh finding first
and the Files & Risk Findings column links each file to its findings on the
Security page ([commands.md](commands.md#hotspots)).

## From an agent: the MCP tools

`logos:security_scan`, `logos:security_ingest`, `logos:security_status` and
`logos:security_findings` return the same payloads as their commands.
`logos:security_scan` takes `scanners` and `paths` for a re-scan, and refuses a
`consent` argument: an agent that meets the consent refusal must ask you to run
`logos security scan --consent`. No MCP tool installs a scanner; `security install`
is a command you run. `logos:review` returns the branch delta with its security
section ([Reviewing what a branch introduced](#reviewing-what-a-branch-introduced)).

## When something goes wrong

| You see | Why |
|---|---|
| `security scan` exits 2 naming `--consent` | No consent is recorded for a scanner it would start. Nothing ran |
| `security install` exits 2 listing tools, versions and sources | The install consent is missing; run it again with `--consent` |
| `security install` exits 1 | Every tool you asked for failed or has no release for this platform: none ended `installed`, `already_present` or kept. Read each `outcome` and `reason` in the output; one tool in place is exit 0 |
| `check` or `gate` exits 5 | A required scanner has no fresh scan at HEAD; run the command it names |
| A scanner reads `stale` after you re-scanned a file | Only a full scan at HEAD makes it `fresh` (`scoped_only`, `head_moved`) |
| A scanner is `failed` | Read its `log` (`.logos/security/<scanner>.failed.log`); one failure never stops the others |
| Exit 2 on a `path` or `extra_args` key in `.logos/config.toml` | A repository may only set `enabled`; set the rest in your user-level config |

The troubleshooting table in [error-handling.md](error-handling.md#troubleshooting-common-errors)
has the full list.
