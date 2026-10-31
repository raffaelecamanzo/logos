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
logos security status                     # each scanner: fresh, stale, never run, not applicable, not installed
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
these things reach the network, each a separate program that Logos starts, and
none without your consent:

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
3. **A coverage run, `logos coverage install` and a coverage dependency addition.**
   A coverage run builds your project and runs its tests, which executes the
   project's own code and may fetch dependencies. `logos coverage install` runs
   your system `curl` to download the pinned `cargo-llvm-cov`, verified by its
   SHA-256, and a dependency addition runs your own package manager. Each has a
   consent of its own, recorded outside the repository:
   [coverage.md](coverage.md#the-execution-consent).

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
| `osv-scanner` | 1.9 | 2.7.0 | Known-vulnerable dependencies | whole manifest only |
| `bandit` | 1.7.10 | 1.7.10 | Insecure Python code | yes |
| `gosec` | 2.21 | 2.29.0 | Insecure Go code | yes |
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

osv-scanner is pinned at 2.7.0 while its *Tested* version stays 1.9, the last
one the harness ran. Its command line now uses the 2.x spelling of the scanner's
output option, which 1.9.x does not know, so a 1.9.x copy reports `failed` with
the scanner's own error until it is replaced: a managed 1.9.2 copy shows 2.7.0 as
its update (`logos security install <scanner> --update --consent`, naming
osv-scanner), and a copy of your own on `PATH` or named in your user-level config
has to be upgraded or removed by you, because Logos does not replace a copy that
meets the minimum. Findings stored from 1.9.2 keep their identity across the
move: none reads as new, in the ratchet baseline or in an ignore entry.

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
- A *not installed* row that Logos can install offers **Install**. The **Version**
  cell says which copy runs: `managed` (installed by Logos), `path` (your own
  copy on `PATH`) or `override` (your user-level `path` setting). A `managed` copy
  older than this release's pin reads `update available` and offers **Update**,
  which installs the pinned version and removes the older copy.
- **One consent dialog** comes first while anything is awaiting consent. It names
  each tool with its version, the host it is downloaded from and its size, and any
  support install a tool needs (`uv` and the managed Python for Semgrep, Bandit and
  Checkov). For Install & scan it also names the scanners whose network consent
  this project has not given. Accepting records both consents exactly as
  `--consent` does; declining downloads and records nothing.
- **Nothing runs before you answer.** The page decides what to ask from what is
  installed, without starting any scanner, not even to read its version. A copy
  of your own on `PATH`, with no managed copy beside it, is therefore left as it
  is, whatever its version; to put the pinned version beside an older one, run
  `logos security install <scanner>`. Where both copies are there, **Update**
  asks for the pinned version, and if your own copy meets the minimum the install
  keeps it and downloads nothing.
- **Dependency-Check and CodeQL are opt-in lines, unticked.** Dependency-Check's
  line states the Java runtime it brings and its size. CodeQL's line shows its
  licence terms, and accepting them is a checkbox of its own: CodeQL does not
  install until it is ticked. Left unticked, neither is installed.
- One live line above the Scanners table counts the tools installed, then the
  scanners finished, and the row of the tool installing reads `installing`.
  **Cancel install** stops the download in progress: no partial tool is left in
  the tools directory, no further tool starts, and the line names the tools that
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
| Scan | The scanners named in it may reach the network to refresh rules or databases | `security-consent.toml`, user-level, keyed by the project's root path | A scanner is installed or enabled later, or a `not_applicable` one gets something to scan: it is asked for alone |
| Install | Downloading and installing each tool at its pinned version, in every project | `security-install-consent.toml`, user-level | A release pins a new version of a tool |

Without consent the command exits 2, names what is awaiting it and the flag that
records it, and starts nothing, not even a version probe. `logos security status`
reports the consent state: the scanners consented to and the ones the next scan
will ask for. To withdraw scan consent, delete the project's table from
`security-consent.toml` (or the file). Where the files live and the exact wording
are in [configuration.md](configuration.md#scan-consent).

Coverage runs have a consent of their own. A coverage run builds your project and
runs its tests, so it executes the project's own code, which a scan does not. It is
recorded in a separate user-level file, per project, and it pins the exact text of a
configured `refresh_cmd`, so that text changing in a repository asks again. Neither
consent stands in for the other. What the coverage consent covers and what it does
not is in [coverage.md](coverage.md#the-execution-consent).

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

**One scan or install at a time per project.** Every scan and every scanner
install of a project holds the project's lock, `.logos/security/run.lock`,
whether it was started from the CLI, an MCP client or the Security page, and in
whichever process. While one runs, another is refused before it starts anything:
the CLI exits 3 and the MCP tool answers an error, each naming what runs, for
example `a security scan is already running on this project: job 4, started
from the web UI by Logos process 81234 12 s ago`. The Security page answers
**Run scan** by joining a scan it is already running, and refuses a start while a
scan or install started elsewhere runs, with the same message naming it. An
install and a scan of the same project never run together. The lock is the operating system's, so it is released however its
holder ends, a killed process included. Each scanner and installer runs in a
process group of its own, out of reach of the terminal's Ctrl-C, so a holder that
stops kills them first: a `logos security scan` or `install` stopped by Ctrl-C
(SIGINT) or SIGTERM — the plugin's Stop hook stops an overrunning scan with
SIGTERM — kills the scanners it started and exits 130 or 143, and a stopping
`serve` kills those of a scan an MCP client started. SIGKILL cannot be caught: a
Logos process killed by it leaves its scanner running, and the freed lock then
admits a scan beside it. A `logos security install` run where no
scan can run — a directory with no `.logos/`, or one whose `.logos` is a symbolic
link, which every scan refuses — takes no lock and writes nothing there.

**A scanner with nothing to scan is not run.** gosec reads only Go files (`.go`)
and Bandit only Python files (`.py`, `.pyw`). In a project where no file the
project keeps has one of those extensions, the scan reports that scanner
`not_applicable`, with a `reason` naming the files it looked for, and starts
nothing for it, not even its version check. Files in directories the project
ignores do not count, so a vendored `third_party/` you ignore does not make
gosec run. CodeQL is `not_applicable` when no file is in a language it analyses.
Every other scanner reads many languages, manifests or any file, so it always
runs.

**A failure names its cause.** A `failed` scanner's entry carries `cause`: one
line taken from its output (the same text as its log), with anything that looks
like a secret replaced by `[masked]`. Logos picks the first line that reports an
error, meaning one with the word `error`, `fatal` or `panic` (`0 errors` does
not count). Without such a line it takes the last line, and if the scanner
printed nothing, it says so. The line is cut to 240 characters. A Dependency-Check
run that could not update its NVD data reads, for example,
`[ERROR] Error updating the NVD Data`. The whole output stays in the log.

**A missing NVD API key is named, with its remedy.** Dependency-Check 13 needs
an NVD API key to update its data, and when it has none the NVD API refuses it
(`failed`, exit 13). Its first error line says only that the update failed, so
when the log shows the NVD client rejecting the API key (an `NvdApiException`
naming it) the `cause` is this sentence instead: *NVD API key missing or rejected: set NVD_API_KEY where logos or `logos serve` runs, or
[security_scan.dependency-check] extra_args = ["--nvdApiKey", "<key>"] in the user config;
free key: https://nvd.nist.gov/developers/request-an-api-key*.
Request the key at that address and give it to Logos in one of two ways
([Dependency-Check's NVD API key](#dependency-checks-nvd-api-key) below). The
cause never repeats a key value. Any other NVD failure keeps its own first error
line as the cause.

#### Dependency-Check's NVD API key

**Export `NVD_API_KEY`.** Logos reads the key from the environment of the
process that runs the scan: your shell for `logos security scan`, and the
environment `logos serve` was started with for the MCP tools and the web page.
Export it before you start `serve`; a `serve` already running does not see a
variable exported later. Nothing else can supply it: no repository file, no
`.env`, no `.logos/config.toml` key. An empty `NVD_API_KEY` sets no key.

```bash
export NVD_API_KEY=<key>
logos security scan --scanner dependency-check
logos serve --ui          # MCP and the Security page scan with the key
```

Dependency-Check 13 reads no environment variable itself, so Logos hands the key
over in a properties file (`nvd.api.key`) of its own:
`.logos/security/nvd-api-key.properties`, created new with mode 0600 (readable by
you alone) and passed as `--propertyfile <path>`. The key is never on the command
line, so the process list shows only the path. The file exists only while
Dependency-Check runs and is deleted when the run ends, whether it ran, failed or
was cancelled; one a crash left behind is deleted at the next scan's start. The
key appears in no log, cause, job event, payload or stored row: should
Dependency-Check print it, Logos replaces it with `[masked]` before writing the
failure log.

**Or your own `extra_args`.** In your user-level `config.toml`
([configuration.md](configuration.md#user-level-config--settings-no-repository-can-choose)),
never `.logos/config.toml`, which may only set `enabled`:

```toml
[security_scan.dependency-check]
extra_args = ["--nvdApiKey", "<key>"]
```

When your `extra_args` carry `--nvdApiKey` or a properties file of your own
(`--propertyfile`, `-P`), they win: Logos adds no file. This route passes the key
on Dependency-Check's command line, where other users of the machine can see it
in the process list while a scan runs, and keeps it in the user config: do not
commit that file.

**Whether a key is set.** `security status` carries `nvd_api_key: set` or
`not_set` on the Dependency-Check row only, the same on the CLI, MCP and the web,
and never the key itself. It reads `set` when `NVD_API_KEY` is set where Logos
runs or your `extra_args` hand Dependency-Check a key. On the Security page the
Dependency-Check row of the **Scanners** table reads "NVD key: set" or "NVD key:
not set".

**What a scan covers.** A scan reads the project, not what it ignores. Logos finds
the top-most directories the project's own ignore files exclude: the `.gitignore`
and `.ignore` files at the root and below it, and `.git/info/exclude`, but not
your global git excludes or an ignore file above the root. Then it keeps each
scanner out of them in the way that scanner allows:

- **Bandit, Trivy, Checkov and Dependency-Check** get the directories through
  their own exclusion flag, besides `.logos/`. So they do not scan a build
  directory (`target/`, `node_modules/`, `dist/`), a notes folder or a vendored
  copy you ignore, and Trivy no longer spends its whole timeout walking `target/`.
  Their flags were read from each scanner's source, but the hand-run harness
  has not run them, so nothing yet proves them anchored on the pinned
  versions: these four still say `scanned`, with the note `exclusion flag not
  proven by the harness`, until it does.
- **Semgrep and OSV-Scanner** get every path outside them as their targets,
  instead of `.`: beside an ignored `target/`, that is `.gitignore app go.mod src
  targets`. A directory that holds an ignored one is named by its entries, so an
  ignored `web/dist/` makes the list `… web/.gitignore web/src …`. A symbolic
  link is never named: neither scanner follows one when it walks `.` itself, and
  Semgrep refuses one given as a target. This works the
  same in a git checkout, a linked worktree (`git worktree add`) and a copy that
  is not a git repository. Left to themselves, Semgrep reads ignored directories
  in a copy that is not a git repository, and OSV-Scanner reads them in a linked
  worktree too: it cannot read the worktree's git ignores. Semgrep's own
  exclude option would also skip `src/target/` when you ignore `/target/`. The
  hand-run harness proved the target list on osv-scanner 1.9.2; its re-run on the
  pinned 2.7.0 is the maintainer's, and until its rows are committed the evidence
  is the 1.9.2 run.
- **gosec** gets `-exclude-dir '^target(/|$)'` for each directory: an
  expression anchored at the project root that covers the directory and every
  directory below it. The hand-run harness proved it on gosec 2.21.4, in a git
  checkout, a linked worktree and a copy that is not a git repository (the
  committed rows are still that run; the pinned release is 2.29.0): it skips
  `target/` and `target/nested/`, and keeps the near misses `src/target/` and
  `targets/`. A test of gosec's matching rule also
  keeps `pkg/mytarget/`. gosec's simpler spellings would not do: `target` also
  drops those near misses, and `^target$` misses `target/nested/`.
- **Gitleaks** reads committed history, so it reads a file committed inside an
  ignored directory. **CodeQL** still reads ignored directories. A config with
  `paths-ignore` might keep it out, but the harness has never run CodeQL, so
  nothing proves that on the pinned version: Logos gives CodeQL no exclusion
  and writes no config for one.

Every whole-project scan says what each scanner did. Each scanner that ran reads
`ignored_paths: excluded` (a mechanism the hand-run harness proved kept it out
of all of them, which today means Semgrep, OSV-Scanner and gosec, or the project
ignores none) or `ignored_paths: scanned` with a one-line `coverage_note`
saying why and a `coverage_kind` naming how: `unproven` (no exclusion is proven
for the scanner), `history` (Gitleaks reads the committed history), `whole_project`
(Semgrep or OSV-Scanner could not be given the list of files outside the ignored
directories) or `partial` (some ignored directories were left out of the exclusion).
The kind is the same on the CLI, MCP and web, and is present exactly when the note
is. The
fact is in each scanner's entry of `security scan` (CLI and MCP), in each
installed scanner's row of `security status`, and in the **Ignored dirs**
column of the Security page, which shows a marker chosen from the kind
(`skipped`, `not proven †`, `reads history ‡`, `whole project §`, `partly skipped ¶`
or `—`) with the full note as its tooltip, and lists each marker shown in a legend
under the table. In `security status` and on the page it is a
forecast: what a whole-project scan of the tree as it is now would do, beside a
stored report that may come from an older tree or from CI. A scanner the scan
would not start (disabled, or refused because of a `.checkov.yaml`) states
none there; CodeQL states its forecast even when the scan would find no
language for it to analyse.

Per scanner, when the project ignores something:

| Scanner | How Logos keeps it out | Proven by the harness | `ignored_paths` |
|---|---|---|---|
| Semgrep, OSV-Scanner | every path outside the ignored directories as a target | yes | `excluded`, or `scanned` when the list cannot be passed |
| gosec | one anchored `-exclude-dir` expression per directory | yes | `excluded`, or `scanned` when a directory is left out of the list |
| Bandit, Trivy, Checkov, Dependency-Check | their own exclusion flag | no | `scanned`, note `exclusion flag not proven by the harness` |
| Gitleaks | none: it reads committed history | — | `scanned` |
| CodeQL | none: no exclusion is proven for it | — | `scanned` |

The edges:

- A file you ignore *by name* (`*.log`, `.env`) in a directory that is not
  ignored is still read by Bandit, Trivy, Checkov, Dependency-Check and gosec,
  which are given whole directories only. Semgrep and OSV-Scanner leave it out where
  Logos names a directory's entries, and otherwise apply their own rules.
- Your global git excludes are not the project's, so a file only they ignore
  (`.DS_Store`) is named to Semgrep and OSV-Scanner when it sits beside an
  ignored directory. In a git checkout Semgrep would otherwise have skipped it.
- Semgrep scans a file it is given by name whatever its own filters say: your
  `.semgrepignore`, an exclude flag in your `extra_args` and its 1 MB size limit
  do not apply to the files Logos names (those beside an ignored directory, at
  the root for an ignored `target/`). Directories it is given are filtered as
  usual. A large generated file at that level can slow Semgrep down; ignore it
  in `.gitignore` or `.ignore` to keep it out.
- A file committed inside an ignored directory (added with `git add -f`) is
  skipped with the rest of that directory by every scanner but Gitleaks, which
  reads it in history, and CodeQL.
- An ignored directory whose name holds a comma, a double quote, a glob
  character, a backslash or a control character, or starts with `-`, is not
  given to an exclusion flag, which could misread it. Bandit, Trivy, Checkov,
  Dependency-Check and gosec read it, and gosec then says `scanned`, with how
  many it was not given. Semgrep and OSV-Scanner still keep out of it.
- The list a scanner is given is capped at 64 KiB of arguments, so that every
  scanner still starts when a project has thousands of ignored directories,
  such as a `__pycache__` in every package. Past the cap, an exclusion flag
  leaves out the deepest directories, which are read (the log says how many,
  and gosec says `scanned` with the count); and Semgrep and OSV-Scanner are
  given `.` as before, and say `scanned`, with the note saying why. Semgrep and OSV-Scanner are also given `.`, and say
  `scanned`, when a path outside the ignored directories starts with `-` (they
  would read it as a flag) or is not UTF-8, or when no path lies outside the
  ignored directories at all.

A re-scan of named paths (`--path`, next section) reads exactly those paths, even
inside an ignored directory, and states no coverage for the scanners that read
only them. The scanners that cannot be narrowed to paths run over the whole
project there too: they leave out the same directories and state their
coverage.

`security status` gives each scanner one state:

| State | Meaning |
|---|---|
| `not_installed` | No binary was found and no report is stored |
| `disabled` | Switched off in `[security_scan]` |
| `never_run` | Installed, and no report is stored |
| `not_applicable` | Installed, no fresh report is stored, and nothing it reads is in the project (gosec without Go files, Bandit without Python files; CodeQL without a language it analyses still reads `never_run` here); `reason` says which files it looked for. No readout starts it, not even for its version: its `installed` copy says `"unprobed": "nothing_to_scan"`. No scan is missing, so neither `check`, the `[security]` table nor the workspace roll-up counts it as one |
| `stale` | A report is stored but HEAD has moved (`head_moved`), a file it anchors changed (`files_changed`), or only re-scans of some paths stand behind it (`scoped_only`) |
| `fresh` | A report is stored at this HEAD over unchanged files; a clean scan is `fresh` with `findings: 0` and nothing left out. `findings` counts fresh, unsuppressed findings in production code; `suppressed`, `not_fresh`, `test`, `dev` and `needs_review` state the rest ([test code](#findings-in-test-code), [dev-only dependencies](#findings-on-a-dev-only-dependency), [needs review](#findings-that-need-review)), and `merged` the findings counted on another scanner's row ([one advisory, several scanners](#one-advisory-several-scanners)) |

A scanner that has not run has looked at nothing, so its silence is never a clean
result: `never_run` carries no count, and every list says what it was drawn from.
A fresh stored report outranks `not_applicable`: a gosec report ingested from CI
at this HEAD reads `fresh` whatever the files here are. A stale one does not,
since no scan here could refresh it: once the last `.go` file is gone, gosec reads
`not_applicable`, never a `stale` scan whose remedy is a scan that starts
nothing.

Whatever the state, a scanner whose last scan failed also carries
`last_failure`, the log, and `last_failure_cause`, the
[cause line](#running-scans-and-reading-the-results) read back from that log.
Both stay until the scanner next runs cleanly. On the Security page the row's
**Findings** cell shows a red `LAST SCAN FAILED` label that expands to the cause
and the log path.

Dependency-Check's row alone also carries `nvd_api_key`, `set` or `not_set`:
whether it has an NVD API key, never the key
([Dependency-Check's NVD API key](#dependency-checks-nvd-api-key)).

`security findings` returns the stored findings with their OWASP Top 10 categories
(2025, or 2021 with `--edition`, reclassified without a re-ingest), the filters
`--severity`, `--scanner`, `--category`, `--path` and `--suppression`, and a
`categories` view that marks a category `not_covered`, never `0`, when no scanner
whose report is stored can detect it. A covered category states two counts:
`state.findings`, over every stored finding under it in every code scope, and
`production_findings`, the part of it in production code. Suppressed findings are
in both, whatever `--scope` and the other filters say, and the difference is the
findings left out of production: those in test code ([below](#findings-in-test-code)),
on a [dev-only dependency](#findings-on-a-dev-only-dependency) (`dev_findings`) and
that [need review](#findings-that-need-review) (`needs_review_findings`). The Security page's OWASP
list under Production states `production_findings`, so the page and the CLI and MCP
payloads give the same figure per category. In a workspace, `--repo <MEMBER>` on
`status` and `findings` answers for one member.

Each finding also carries its place in the graph, described next. The payloads and
every flag are in [commands.md](commands.md#security-findings).

### Findings in test code

A fake key in a test, or a Dockerfile in a test fixture, is still reported by the
scanners. Logos tells such findings apart from the code you ship. Each finding
reads `code_scope: "test"` or `"production"`, decided each time it is read and
never stored:

- **`test_symbol`**: the symbol around its line is test code in the graph, such
  as a `#[test]` function in a `src/` file's test module;
- **`test_path`**: its path is a test path (a `tests/`, `test/`, `spec/` or
  `__tests__/` directory, or a test file name such as `*_test.go`), or it sits in
  a `fixtures/`, `testdata/` or `__fixtures__/` directory. A report that names
  the file by its absolute path under the project reads the same. The fixture
  directories mark test code for security only: metrics and hotspots still count
  `src/fixtures/x.rs` as production code.

A finding with no location, or one outside the project, is production. A secret
found only in an earlier commit, and a finding whose file changed since the scan,
has no current symbol, so only its path decides: a secret that sat in a test
module inside `src/` reads `production`.

**Lists and counts leave test code out and say so.** `security findings` and the
MCP `security_findings` list production code unless you pass `--scope test` or
`--scope all` (MCP: `scope`), and state the number hidden as `hidden_by_scope`.
`security status`, the Security page's figure, the workspace roll-up and the
findings column of Files & Risk and `hotspots` count fresh, unsuppressed findings
in production code. `status`, the page and the roll-up state the rest by why:
`suppressed`, `not_fresh`, `test` ("N more in test code"), `dev` ("N more in
dev-only dependencies", [below](#findings-on-a-dev-only-dependency)) and
`needs_review` ([below](#findings-that-need-review)). Each finding falls in the first of those
that holds, so there the counted and the excluded findings add up to every stored
finding. The per-file cell states `stale`, `test`, `dev` and `needs_review` (the page
shows `+N dev` and `+N review`) beside its `count`; a suppressed finding, and one bound to no file, joins no cell.

**The gate keeps every finding.** The `[security]` ratchet and caps, `gate --save`,
`security bless` and the branch delta of `review --base` treat a finding in test
code like any other: a real secret committed in a test is still a leak. Their
output does not change. To list what earlier releases listed by default, pass
`--scope all`.

### Findings that need review

A scanner that states how sure it is of a result is believed about it. Each
finding reads `confidence`: `low`, `medium`, `high` or `unstated`, taken from its
report by one rule per scanner and stored with the finding (`history.db`
migration 12):

| Scanner | Where the report states it | Reads as |
|---------|----------------------------|----------|
| Semgrep | the rule tag `LOW CONFIDENCE`, `MEDIUM CONFIDENCE` or `HIGH CONFIDENCE` | the tag |
| CodeQL | the rule's `properties.precision` | `low`, `medium`; `high` and `very-high` read `high` |
| Bandit | the result's `properties.issue_confidence` | `LOW`, `MEDIUM`, `HIGH` |
| gosec | the rule's `properties.precision`, which is its own confidence | `low`, `medium`, `high` |
| Trivy, Gitleaks, OSV-Scanner, Checkov, dependency-check | nothing | `unstated` |

A value a scanner writes on every result is never read as confidence. Semgrep's
`precision` is `very-high` on every rule, and so is Trivy's: a Semgrep result whose
rule is tagged `MEDIUM CONFIDENCE` reads `medium` beside `precision: very-high`,
and Semgrep's tag decides. Which field carries confidence was recorded from each
scanner's SARIF in `logos-core/tests/fixtures/sarif/README.md`; where that file
says a fixture was reconstructed from the scanner's writer rather than captured,
the field is as the writer defines it and has not been checked against a report
from a scanner run here. A scanner that states nothing reads `unstated`, and so does
every finding stored before migration 12 until the next ingest of its scanner
(re-ingesting a stored report decides it anew).

A `low` finding **needs review**, on every surface:

- `security findings` and the MCP `security_findings` list production code that does
  not need review by default, and state the number left out as
  `hidden_needs_review`, "N more need review". `--scope all` lists it, with
  `confidence: "low"` and `needs_review: true`; `--scope test` lists it only when it
  sits in test code, where it is test code first.
- `security status`, the Security page, the Dashboard's Security widget and the
  workspace roll-up leave it out of the count and state the rest as `needs_review`
  beside `test`: "N more need review". Each finding falls in the first of
  `suppressed`, `not_fresh`, `test`, `dev` and `needs_review` that holds. The per-file
  cell of Files & Risk reads `+N review`, and an OWASP category's
  `needs_review_findings` is its production part that needs review, so production,
  dev-only, needs review and test code add up to the category's `findings`.
- On the Security page the finding carries a muted **REVIEW** tag under its location,
  with "Needs review" and the scanner's word on hover and in the text a screen
  reader gets, under Code scope **All**.

**The gate keeps every finding.** The `[security]` ratchet and caps, `gate --save`,
`security bless` and the branch delta of `review --base` count a finding that needs
review like any other, as they do one in test code: a `max_high = 0` cap fails on a
low-confidence high finding. Their output does not change; to see what a list left
out, pass `--scope all`.

### Findings on a dev-only dependency

A lockfile finding carries a **dependency scope**, read each time the finding is
read and never stored. It reads `dev` when the lockfile itself marks the package
dev-only, and `production` otherwise:

| Lockfile | Marks a package dev-only by |
|----------|------------------------------|
| `package-lock.json`, `npm-shrinkwrap.json` | `"dev": true` on every entry of that name and version (lockfile v1, v2 and v3; `devOptional` is not `dev`) |
| `pnpm-lock.yaml` | `dev: true` on every `packages` entry of that name and version |

The scope never guesses. A `go.mod`, `Cargo.lock`, `pom.xml`, `yarn.lock` or Python
finding reads `production`, as does a finding whose package the lockfile does not
hold, a package it holds once without the flag (a hoisted copy something ships
depends on), a lockfile that cannot be parsed, and a finding whose message names no
package (OSV-Scanner's `Package 'name@version'` and Trivy's `Package:` /
`Installed Version:` are read). A dev package's finding in a fixtures lockfile reads
`test` first. Go, Cargo, Maven and Python record dev scope in the manifest or not at
all; covering them is a later change.

A `dev` finding is labelled apart on every surface, as one in test code is:

- `security findings` and the MCP `security_findings` list it with
  `code_scope: "dev"` and `code_scope_reason: "dev_dependency"` under `--scope all`
  alone, and state the number the default scope left out as `hidden_dev`, "N more in
  dev-only dependencies" (`--scope test` counts it in `hidden_by_scope`).
- `security status`, the Security page, the Dashboard's Security widget and the
  workspace roll-up leave it out of the count and state it as `dev` beside `test`,
  after it: each finding falls in the first of `suppressed`, `not_fresh`, `test`,
  `dev` and `needs_review` that holds. The per-file cell of Files & Risk reads
  `+N dev`, and an OWASP category's `dev_findings` is its part on a dev-only
  dependency, apart from `production_findings`.
- On the Security page the finding carries a muted **DEV** tag under its location,
  with its reason on hover and in the text a screen reader gets, under Code scope
  **All**.

**The gate keeps every finding.** The `[security]` ratchet and caps, `gate --save`,
`security bless` and the branch delta of `review --base` count a finding on a
dev-only dependency like any other: a `max_severity = "high"` cap fails on a high
advisory in a dev tool. Their output does not change; to see what a list left out,
pass `--scope all`.

### One advisory, several scanners

OSV-Scanner, Trivy and Dependency-Check overlap: run together, they report most
lockfile advisories twice or three times. Each report is stored as it is, per
scanner, but every surface reads one advisory on one package as **one finding**.
Two dependency findings are one when they share:

- **the lockfile**, by its path in the project: OSV-Scanner's absolute path and
  Trivy's relative one name one file;
- **the package name and version**, compared in one spelling: Trivy's Go version
  `v2.11.0` is OSV-Scanner's `2.11.0`, Dependency-Check's purl
  `pkg:maven/org.apache/log4j-core` is `org.apache:log4j-core`, and
  `pkg:npm/%40vitest/mocker` is `@vitest/mocker`; letter case is ignored;
- **an advisory id**: the rule id, or an id OSV-Scanner lists in its message as
  `also known as` (a CVE, a GHSA, a `GO-` or `PYSEC-` id). OSV-Scanner reporting
  `PYSEC-2018-28`, also known as `CVE-2018-18074`, and Trivy reporting
  `CVE-2018-18074` are one advisory.

Two advisories on one package whose ids never meet stay two, and so does one
advisory on two packages, two versions or two lockfiles.

The merged finding:

- names every scanner that reported it in `reporters`, each with its version,
  its own `fingerprint`, the `rule_id` it used and the `severity` it gave; a
  finding one scanner alone reported has no `reporters`;
- reads the highest severity a reporter gave, and an `unrated` reporter never
  lowers a rated one: OSV-Scanner's unrated beside Trivy's `low` reads `low`;
- counts once in `security findings`, the MCP tools, the Security page, the
  Dashboard's Security widget, Files & Risk and the workspace roll-up.
  `--scanner` lists it under each of its reporters. In `security status` it is
  counted on one row, its first reporter's, and each other reporter's row states it
  as `merged`, so every row still accounts for each finding of its report;
- counts once for a `[security]` cap and the ratchet, and is evaluated while any
  reporter's scan is fresh. A baseline that recorded any reporter's fingerprint
  knows it, so a baseline written before findings were merged still matches;
  `gate --save` and `security bless` record every reporter's fingerprint;
- is one entry of the branch delta, which names its `reporters`: it is
  `introduced` only when no reporter's merge-base report held it, and `resolved`
  only when no reporter still does;
- is **suppressed** by an ignore entry naming any reporter's fingerprint, the one
  the Security page writes included. An entry that names a rule or a scanner (a
  rule-wide entry, or a path entry narrowed to one) covers only the reporters it
  names: the finding reads suppressed only when every reporter is.

A Dependency-Check report stored before this (history.db migration 14) does not
record the package, which its message never names: its findings join the others
once Dependency-Check scans again, or its report is ingested again.

### Severities, and `unrated`

A finding's severity comes from its report. A CVSS score in `security-severity`
decides first: 9.0 and above is `critical`, 7.0 `high`, 4.0 `medium`, above 0
`low`, and 0 `info`. Without a score, the SARIF `level` decides: `error` is `high`,
`warning` `medium`, `note` `low`, `none` `info`.

A report that gives neither a score nor a level of its own gives no rating, and
Logos does not make one up. The finding reads **`unrated`**, and the reason
`the scanner gave none` goes with it (`severity_reason` in the JSON, a line in
the Security page's finding detail). There are two cases:

- the result has no `level` and its rule sets none, so the level is only SARIF's
  default `warning`. Gitleaks writes no level and no score at all, so every
  Gitleaks finding reads `unrated`;
- the scanner writes the same level whatever the finding is. osv-scanner does
  this: each result reads `warning` whatever the advisory says. osv-scanner 1.x
  never gives a score, so a 1.9.2 report you ingest from CI reads `unrated`
  throughout. osv-scanner 2.0.0 and later put the advisory's CVSS score on the
  rule, where the advisory has one: the pinned 2.7.0, or a 2.x report you ingest,
  reads the advisory's own rating, and only the advisories with no score read
  `unrated`. Checkov does it too (`error` on a failed check that has no
  severity, `warning` on a skipped one) and dependency-check writes `warning` on
  every result: a result of either with no `security-severity` reads `unrated`,
  and one that carries it reads that rating. The other scanners' levels follow
  the finding, so they rate it.

`unrated` is listed with the other findings and counted on its own, never inside
`medium`. `--severity unrated` lists the unrated findings alone. A rated
`--severity` compares an unrated finding as `medium`: `--severity medium` lists it
and `--severity high` does not. The Security page's **Least severity** filter
works the same way (its *unrated only* option), and the workspace roll-up gives
`unrated` its own column. Findings stored before this release that read `medium`
from an osv-scanner 1.x or a Gitleaks report read `unrated` after the upgrade,
without a new scan. Re-ingest any other report to have its severities decided
again: Checkov and dependency-check findings stored before this change keep
their old severity until a report with new content is ingested.

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
  still in history and stays listed. A secret the scoped run finds that is a
  recorded one has that finding's fingerprint (below), so `check`, `gate` and the
  ignore file still recognise it; only a new secret reads as new.
- **One secret is one Gitleaks finding.** Its fingerprint folds its rule, its
  file and a keyed digest of the secret, never its position or its commit. The
  digest is computed in memory from the file, or from the commit that holds the
  secret; neither the secret nor its digest is stored. So a secret found in
  several commits is one finding that lists them in `commits`, newest first, and
  it keeps its fingerprint when its line moves, when it is committed and when a
  scoped run reads it. The key is derived from the repository's first commit, so
  every full clone keys a secret alike; a shallow clone keys it differently.
- **A secret found only in history is `historical`.** A full scan's finding is
  bound to today's file and line only where that line still holds the secret
  (an edit elsewhere on the line keeps it). If its file is gone, or the line
  holds other code now, the finding's `binding` is `historical`: it has no
  `path`, no enclosing symbol and no blast radius, and its `anchor` names the
  commit and the path it was found at, with `short_commit` — the seven digits
  the CLI, MCP and the Security page all show. When the secret is still on the
  line some older commit put it, the finding sits there instead. A
  `historical` finding still counts, in the ratchet and in every cap: the secret
  is still in the repository's history, and a `check` or `gate` violation names
  its place `(historical, commit <short_commit>)`. A secret still on its line in
  a file the index does not hold keeps its `unmatched` binding. No author,
  email or commit message is stored.
- **A secret found only in history is known by its own path.** Whether it is
  still on its line is read at the path its report gives at its commit, made
  relative to the project (in a workspace member below the git root, the
  member's directory is taken off), and its fingerprint is keyed by that path —
  never by a file of today's tree that shares its name. A root `client.py`
  holding the same text never binds a deleted `legacy/client.py` secret, and
  adding or removing such a file never re-keys it, so its suppression and the
  ratchet's baseline keep knowing it. A history path outside the project is
  `historical`. Stores written before this identity (history.db migration 11)
  are re-keyed at the next Gitleaks scan or ingest; each row keeps its old key,
  which an ignore entry, the ratchet's baseline, a checked-in baseline and the
  branch delta still honour. Each stored row is matched by its place (rule,
  report path, commit and exact region), so this holds whatever files exist
  today. In a fresh clone with no stored rows, an old key is known only through
  the binding today's index gives: a checked-in baseline or ignore entry
  written while an unrelated same-named file existed, since removed, names it
  again only once re-blessed (`logos security bless`) or rewritten with the
  finding's fingerprint.
- **One package on one advisory is one dependency finding.** An OSV-Scanner,
  Trivy or Dependency-Check finding is keyed by its rule, its path (the lockfile,
  or the file Dependency-Check scanned) and the package's name and version, read
  from the result (OSV-Scanner's `Package
  'name@version'`, Trivy's `Package:` / `Installed Version:`, Dependency-Check's
  purl), never by its position. A result a scanner writes twice in one report is
  one finding: OSV-Scanner writes a package once per advisory record that matched
  it, so a Go module with both a `GO-` and a `GHSA-` record was listed twice. Two
  packages sharing one advisory in one lockfile stay two. Every other finding
  keeps its key. Stores written before this identity (history.db migration 13)
  are re-keyed at each of those scanners' next scan or ingest, a repeat's two rows
  becoming one; each row keeps its old keys, which an ignore entry, the ratchet's
  baseline, a checked-in baseline and the branch delta still honour.
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
another symbol), its line is in no code symbol, or it is a secret found only in
history (`historical`, anchored to its commit instead).

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
| `required_scanners` | none | A scanner named here has no fresh scan at HEAD: it never ran, is stale, is not installed or is disabled, or its name matches no scanner (`unknown`). Exit 5. A `not_applicable` one, with nothing to scan here, is not missing |
| `ratchet_min_severity` | `high` | An unsuppressed finding at or above it is **new**: its fingerprint is not in the ratchet's baseline — the checked-in `.logos/security-baseline.json` when present, else the set `logos gate --save` recorded. Rule `security:ratchet` |
| `max_severity` | unset | Any unsuppressed finding at or above it exists, recorded or not. Rule `security:max_severity` |
| `fail_on_owasp` | none | Any unsuppressed finding is in one of these OWASP Top 10 categories. `A04` is the 2025 edition; `A01:2021` names the older one. Rule `security:fail_on_owasp:<id>` |

**How the ratchet and the cap treat an `unrated` finding.** Both gate it as
`medium`, the rank its default `warning` level gave it before it read `unrated`
([Severities, and `unrated`](#severities-and-unrated)). So
`ratchet_min_severity = "medium"` or `max_severity = "medium"` (or a lower
threshold) fails on it, and `"high"` or `"critical"` does not. No verdict changed
when the spelling did. The message names it as `unrated (gated as medium)`.
Neither key accepts `"unrated"` as a value: it is not a rank, and naming it is
a usage error (exit 2).

**The ratchet adopts the gate on a codebase that already has findings.** Scan, then
record what is there; after that only a finding that is new since the save fails:

```bash
logos security scan --consent   # fresh scans of the required scanners at HEAD
logos gate --save               # records the metric baseline and every current finding's fingerprint
logos check                     # now fails only on what is new, or over a cap
```

Until a save is recorded, every finding at or above `ratchet_min_severity` is new,
and the report says to run the save. The recorded set lives in `.logos/history.db`
with the findings; `session_start` records it before the edits when none is
saved and none is checked in, and never moves it after. A suppressed finding is not
recorded, so when its suppression expires or is removed it is new again. The
**caps** ignore the recorded set: `max_severity = "critical"` fails on any
unsuppressed critical finding, however old.

To gate in CI, check the baseline in: `logos security bless` writes the findings
— fingerprint, rule id and path, no message or secret — to
`.logos/security-baseline.json`, which the ratchet then reads instead of the set in
`history.db`, in every checkout
([ci-integration.md](ci-integration.md#gating-on-security-findings-in-ci-a-checked-in-baseline)).
A finding reported by an absolute path under the project root that binds to no
indexed file (OSV-Scanner on a lockfile) is keyed by its project-relative path, so
a baseline blessed in one checkout holds it in a clone at another path.

**One OSV-Scanner identity in any clone and at any scanner version.** An
OSV-Scanner finding's fingerprint is its rule and project-relative location (plus
region when the report gives one), never the report's own `partialFingerprints`:
osv-scanner 2.x writes them where 1.x did not, so a scanner upgrade would otherwise
read every finding as new. The same advisory in `Cargo.lock` and in
`sub/Cargo.lock` is two findings with two keys. Stores written before this
identity (history.db migration 10) are re-keyed at the next OSV-Scanner scan or
ingest, from osv-scanner 1.x or 2.x alike: every stored row of a finding that
report still holds, in every stored scan, and its `gate --save` baseline entry
move to the new key, and each row keeps its old key, which every surface still
honours. A finding the report no longer holds keeps its old key: it is fixed. An ignore entry,
the ratchet's baseline and the branch delta written under the old key therefore
still know the finding; the checked-in `.logos/security-baseline.json` is never
rewritten, and the next `logos security bless` writes the new keys. A checked-in
file blessed before migration 10 also matches such a finding by its rule id and
project-relative path; that match is kept for one release, so re-bless before it
goes.

A path **outside** the project root keeps its report spelling in the key. A CI
report ingested locally whose paths name the CI checkout (`/home/ci/project/…`)
therefore gives keys that depend on that root: they match a later ingest of a
report written under the same root, never a local scan. Ingest reports produced
in the checkout they describe, or bless from them alone.

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
- A suppressed finding is **never hidden** by its suppression: `security findings`
  still returns it with its reason (`--suppression active` filters it out), under
  its [code scope](#findings-in-test-code) like any other — one in test code is
  listed with `--scope test` or `--scope all` and otherwise counted in
  `hidden_by_scope`. It is excluded from every `[security]` rule and from the
  counts of `status`, `hotspots` and the workspace roll-up.
- From the day after `expires` the entry stops suppressing and is listed as
  `expired`; an entry that matches no stored finding is listed as `orphaned`.
- An entry written under a fingerprint a finding had before it was re-keyed (a
  Gitleaks finding's earlier, per-commit fingerprint; an OSV-Scanner finding's
  fingerprint from before history.db migration 10) still suppresses that finding.
  Logos never rewrites the file: the entry carries `should_name`, the fingerprint
  its finding has now, for you to put in its place.

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

### One entry for a whole path

A fixtures directory full of planted secrets would need one fingerprint entry per
finding. One entry can name a **`path`** glob instead, optionally narrowed to one
`rule` and one `scanner`:

```toml
[[ignore]]
path = "tests/fixtures"
scanner = "gitleaks"
reason = "planted test secrets, never deployed"
```

- **What it matches.** The glob is matched against each finding's location
  relative to the project root. `*` stays within one path segment and `**`
  crosses segments. A glob that matches a directory covers everything under it,
  as a `.gitignore` line does. So `tests/fixtures` and `tests/fixtures/**` cover
  the same findings, `*.lock` matches only a lockfile at the root, and
  `**/fixtures` matches a `fixtures` directory at any depth. A finding a scanner
  reported by an absolute path, such as osv-scanner's `Cargo.lock`, is matched
  after the root is taken off, so the same entry holds in every checkout, as the
  finding's fingerprint now does too.
- **Narrowing.** `rule` is the finding's rule id, exactly. `scanner` is the
  scanner name findings show (`gitleaks`, `osv-scanner`, `Semgrep OSS`), in
  either case. Leave a key out to cover every rule or every scanner.
- **Each entry is counted.** `logos security status` and `security findings` list
  every entry under `suppressions` with `suppressed`, the number of findings it
  suppresses now. Each finding a path entry covers is listed as `suppressed`,
  with the entry's reason and, under `by`, the entry that covers it. An entry
  that names no finding, such as a glob matching nothing, is `orphaned`. When
  several entries name a finding, the first unexpired one labels it and counts
  it, fingerprint entries before path entries; an expired fingerprint entry
  gives way to a live path entry.
- **Refused, with exit 2 and the line.** A glob that covers the whole project
  (`*`, `**`, `.`, any glob made only of those, or one that matches every name
  at the root, such as `?*` or `{*}`), a glob that escapes the root
  (`..`, an absolute path) or does not compile, an entry with both `fingerprint`
  and `path`, and `rule` or `scanner` beside a fingerprint each reject the file.
- **Canonical spelling.** A path is written without a leading `./` or a
  trailing `/`, with `/` separators. Path entries follow the fingerprint entries,
  sorted by path, then rule, then scanner. A file spelled differently still
  loads, but triage refuses to rewrite it, as with any hand-spelled file.

On the Security page, the Suppress dialog offers **Everything under this path**.
It starts from the finding's directory, which you can edit, and can be narrowed
to the finding's rule or scanner. **Unsuppress** on any finding the entry covers
removes that entry, and so restores everything under it. As with a fingerprint
entry, adding and then removing it restores the file byte for byte. The
**Suppressions** panel lists every entry with its count, flags an orphaned or
expired one, and removes an entry directly.

### One entry for a rule, in every file

A scanner rule you have decided does not apply to the project (a Checkov check
the deploy tool makes moot, say) would need an entry per file, or a `path` glob
that cannot name "everywhere". An entry with a **`rule` and its `scanner`** and no
`path` and no `fingerprint` suppresses that rule's findings for that scanner in
every file:

```toml
[[ignore]]
rule = "CKV_K8S_21"
scanner = "checkov"
reason = "the namespace is set by the deploy tool, not the manifest"
expires = 2027-06-30
```

- **What it matches.** A finding whose rule id is `CKV_K8S_21`, exactly, and whose
  scanner is `checkov`, in either case, wherever it sits, including a finding
  that names no file. Another rule of the same scanner, and the same rule of
  another scanner, stay listed active.
- **Labelled, never silent.** Each finding it covers is listed as `suppressed`
  with the entry's reason and, under `by`, `{"rule": …, "scanner": …}`: no `path`
  and no `fingerprint` in `by` is how a rule-wide entry reads on a finding in
  `security findings` and the web API. `security status` lists the entry itself,
  under `suppressions`, as `{rule, scanner, reason, expires}` with its count. On the Security page the finding's
  detail says "suppressed by a rule-wide entry" with the reason and the expiry,
  and the **Suppressions** panel lists the entry with its count. An entry that
  covers no stored finding is `orphaned`.
- **Expiry.** `expires` works as for any entry: it suppresses through that day,
  and from the next day the findings return, labelled `expired` with the reason.
- **The gate.** The `[security]` rules (`check`, `gate`) leave a suppressed
  finding out as they do any suppression, so the rule-wide entry holds the rule
  out of the gate; once it expires the findings count again.
- **Refused, with exit 2 and the line.** A `rule` without a `scanner`, or a
  `scanner` without a `rule`, when the entry has no `path` and no `fingerprint`:
  the message names the key that is missing. (Beside a `path`, `rule` and
  `scanner` stay what they were: a narrowing.) A whole-project `path` glob
  (`*`, `**`, `.`) is still refused; a rule-wide entry is the way to say "every
  file", and it must name both the rule and the scanner. A blank `rule` or
  `scanner`, a `fingerprint` beside either, and the same rule and scanner twice
  (spelled alike) are refused as well; the same rule of two scanners is two entries.
- **Canonical spelling.** Rule-wide entries follow the path entries, sorted by
  rule and then scanner, written `rule` then `scanner` then `reason`. When
  several entries name a finding, the first unexpired one labels it and counts
  it: fingerprint, then path, then rule-wide.

The web API's `POST /api/v1/security/suppress` takes the same selector, a `rule`
and a `scanner` with no `path` or `fingerprint`, and `unsuppress` removes the entry
by the `by` a finding carries. The Suppress dialog does not offer it yet: write the
entry in the file.

### Recording what a verdict rests on: `evidence` and `reconfirm`

A suppression's reason is often a claim about other code: "the cookie is Secure
because `DefaultCookieConfig` sets it". The scanner's fingerprint lapses when the
matched lines change, but not when that code does. An entry can name the symbols
its reason rests on, each with a hash of its source as it was when you made the
verdict:

```toml
[[ignore]]
fingerprint = "<the finding's 64-character fingerprint>"
reason = "the cookie is Secure: DefaultCookieConfig sets it"
evidence = [
    { symbol = "internal/config/`cookie.go`/DefaultCookieConfig().", hash = "<64 hex characters>" },
]
```

- **It suppresses only while every evidence symbol holds.** The hash covers the
  exact bytes of the symbol's lines, literals, comments and formatting included,
  so changing `Secure: true` to `Secure: false` is noticed. (The `fingerprint`
  the graph keeps on a function is *not* used: it is a normalised shape that
  ignores literals, and cannot tell that change.) Deleting the symbol is noticed
  too.
- **A verdict that stopped holding re-opens.** The finding reads **verdict out of
  date: re-review**, naming the symbol (`` `DefaultCookieConfig` changed ``, `` no
  longer exists ``, or `` could not be checked `` with the reason): in
  `security findings` its `suppression` is `{"state": "out_of_date", "reason": …,
  "changed": [{"symbol": …, "name": …, "change": "changed"}]}`, and the entry
  under `status`' `suppressions` lists the same under `out_of_date`. It is
  **counted** like any unsuppressed finding: in `status` and the page's headline,
  in `hotspots`, and by the `[security]` gate, its ratchet and every cap, until you
  re-confirm it. Evidence Logos cannot check never holds: if the index is behind
  the file (run `logos sync <path>`; a bare `sync` re-reads nothing) or cannot be read, the symbol reads `could not be
  checked`.
- **Re-confirm it once you have re-read the code.**
  `logos security reconfirm <fingerprint>` re-records the hash of every evidence
  symbol as the code is now, keeps the reason and the expiry, and the finding is
  suppressed again. The fingerprint is the finding's, or the entry's own. A symbol
  that no longer exists cannot be re-confirmed: name the symbols the verdict now
  rests on with `--evidence <symbol>` (repeatable; a symbol identity, a canonical
  symbol, or a bare name that names exactly one symbol, else the refusal lists
  the candidates), which records exactly those, plus, for a fingerprint entry, the finding's own enclosing
  symbol. An expired entry is refused: it suppresses nothing, so extend or remove it.
  The same flag gives an entry that records no evidence its first. The
  MCP twin is `logos:security_reconfirm` (`fingerprint`, optional `evidence`).
  Both write `.logos/security-ignore.toml`, so the [plugin guard](plugin.md) asks
  before an agent runs either, as it does before an agent edits the file.
- **On the Security page**, **Suppress…** offers *Record the enclosing symbol as
  evidence* (off by default). A finding whose verdict is out of date shows the
  badge and the symbol that changed, with **Re-confirm** beside **Unsuppress**.
- **An entry without evidence is as before**, and reads **no evidence
  recorded** (its `suppression` carries no `evidence` key): it suppresses exactly as it did, is never checked against the code,
  and `reconfirm` refuses it unless you name `--evidence`. A person's suppression
  may omit evidence.
- **Evidence is part of the canonical spelling:** after `reason` and `expires`,
  symbols sorted, one per line. A `hash` that is not 64 lowercase hex characters,
  an item without `symbol` and `hash`, or the same symbol twice rejects the file
  with exit 2, naming the line. Record hashes with `reconfirm --evidence` or the
  page rather than by hand.

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

- **The verdict first**: the finding count by severity, with what it leaves out
  ([one definition](#one-definition-of-a-counted-finding)), and, when `rules.toml`
  has a `[security]` table, the gate band with the required scanners and the
  findings its rules fail on.
- **Scanners**: the state of each of the nine, with the command that fixes a missing
  one (`logos security install …`, `logos security scan --consent`, or
  `logos security ingest <report>`). Its **Install** and **Update** buttons, and
  **Install & scan**, install from the page itself
  ([From the Security page](#from-the-security-page)).
  The **Version** cell says which copy runs. The page reads a scanner's version
  only once the project's scan consent names it, and then once per `logos serve`
  for each binary (again after a reinstall). Until then the row's `installed`
  says `"unprobed": "not_consented"` and its version is unknown: the Version
  column reads `—` unless a stored report gives one. Without that version the
  page cannot tell whether a `PATH` copy meets the minimum, so where a managed
  copy is installed too, the Version cell names the managed one until consent
  lets the page read the `PATH` copy's version. Opening the page, from any
  site, starts no scanner process you have not consented to.
- **Run scan** sits on the Scanners widget's title row and starts a scan in the
  background. While it runs, the running scanner's row reads `running` (an
  installing tool's row reads `installing`) and one live line above the table counts
  the scanners finished; when it ends the line states the outcome and the page
  reloads. **Cancel scan** stops every scanner process at once and keeps the
  previous results. A click while a scan runs joins it; while a scan or install
  started elsewhere runs (`logos security scan`, an MCP client), Run scan is
  refused with a message naming it. The first scan asks consent
  in a dialog that names the scanners, and records it exactly as
  `security scan --consent` does.
- **Findings**, filtered by severity, scanner, category and path, in the columns
  **Severity** · **Rule** · **Location** · **Symbol** · **Reachable** · **Scanner** ·
  **OWASP** · **Show** · **Triage**. No row carries the finding's message: a clipped
  rule id or path shows in full as its tooltip, **Reachable** reads `yes`, `no` or
  `unknown` (never `no` for unknown), and the message (or why it is redacted) and a
  suppressed or expired finding's state, reason and expiry are in the detail. Selecting one
  opens its graph detail (enclosing symbol, route, blast radius, each with its
  confidence) and the triage actions above. **Show** brings the detail, below the
  table, into view and moves focus to its heading; with reduced motion set in the
  system, the page jumps instead of scrolling. An `unrated` finding's badge carries
  its reason, and its detail states it; *unrated only* in **Least severity**
  lists those findings alone ([Severities, and `unrated`](#severities-and-unrated)).
  One advisory several scanners report is one row whose **Scanner** cell names each
  of them, and its detail lists every reporter with the id and severity it gave
  ([One advisory, several scanners](#one-advisory-several-scanners)). A scanner's
  row in **Scanners** states the findings it shares with another scanner as
  `counted under another scanner`.
- **The code scope** control above the table reads **Production**, **Test code** or
  **All**, and starts on Production, as `security findings` does. It is kept in the
  page's address (`?scope=test`, `?scope=all`; Production names none), so a
  reload or a shared link opens on it, and the Files & Risk `+N test` link opens
  the file's test-code findings. Under **All** or **Test code** a finding in test
  code carries a muted `TEST` tag whose hover says why (`test_symbol` or
  `test_path`, [Findings in test code](#findings-in-test-code)). The figure row
  states `· N in test code` beside the count, the same N as `security status`.
  When the scope hides every finding the other filters keep, the empty table says
  so and how many.
- **A redacted finding** reads *Redacted: the scanner's message would contain the
  secret itself*, followed by its location, in the finding detail. The CLI and MCP
  keep printing the stored text.
- **OWASP Top 10**, 2025 or 2021, with each category `covered` or `not covered`.
  Each count is of the findings in the code scope, Production by default, and the
  figure states how many findings sit in test code. The counts are the ones
  `security findings` states per category: Production reads `production_findings`,
  All reads `state.findings` and Test code is what is left of it after
  `production_findings`, `dev_findings` and `needs_review_findings`. A suppressed finding
  counts here, as in those two, though the Findings figure leaves it out.

In a workspace, a **Security** page at `/workspace-security` rolls up every member:
each member's fresh scanners and its fresh, unsuppressed findings by severity,
linked to that member's own page, and the OWASP view summed over the members read.
The OWASP figure counts fresh, unsuppressed findings in production code, so a
member's figure there can be below its own page's OWASP list under Production,
which also counts suppressed findings and findings in changed files.
Every total says how many members it counts, and `N of M reports fresh` says how
many of the stored reports are themselves fresh at HEAD. A member never scanned
reads `never run`, not `0`; a finding in a changed file, a suppressed one or one in
test code is left out and counted as left out, by
[the one definition](#one-definition-of-a-counted-finding); the figure row states the
test-code remainder summed over the members. The page starts no scan;
open a member's page, or run `logos security scan` in that member.

Elsewhere, `logos hotspots --with-findings` ranks files with a fresh finding first
and the Files & Risk Findings column links each file to its findings on the
Security page, with `+N test` beside the count for the findings in test code
([commands.md](commands.md#hotspots)); both count findings by
[the one definition](#one-definition-of-a-counted-finding).

### One definition of a counted finding

**One definition of a counted finding.** `security status`, the Security page,
Files & Risk and the workspace roll-up count a finding the same way: it counts
unless a current entry in `.logos/security-ignore.toml` suppresses it, its own file
changed since the scan, or it sits in [test code](#findings-in-test-code); a
production finding its scanner marks low-confidence ([needs
review](#findings-that-need-review)) is left out too, and so is a finding on a
[dev-only dependency](#findings-on-a-dev-only-dependency). A
finding left out is left out for the first of those that holds, in that order.
Freshness is the finding's, never its report's, so a project whose only report is a
`--path` scan (its scanner reads `stale` with `scoped_only`) reads the same count on
all of them. A finding in an edited file is left out and shown apart as `stale`
(`not_fresh` in `status`, on the page and in the roll-up); a suppressed one is left
out and counted apart; one in test code is left out and counted apart as `test`, one
on a dev-only dependency as `dev` and one its scanner marks low-confidence as
`needs_review`; a finding bound to no file has none to compare, so it counts on the page and in the
roll-up, while Files & Risk, which is per file, has no row for it.

Whether the *report* is fresh is a separate figure that changes no count: the
roll-up states it as its own field, `N of M reports fresh` (a disabled scanner's
stored report is in `M` and never in `N`). A scanner you disable keeps its stored
findings in every count until you re-scan without it. A file a `--path` scan never
read reads `0` in Files & Risk, which is the absence of a finding and not proof of a
scan: the scanner's `scoped_only` state says which. The OWASP table below the
findings is the one exception: it still counts every stored finding it classifies,
suppressed and changed-file ones included.

## From an agent: the MCP tools

`logos:security_scan`, `logos:security_ingest`, `logos:security_status`,
`logos:security_findings` and `logos:security_reconfirm` return the same payloads
as their commands.
`logos:security_scan` takes `scanners` and `paths` for a re-scan, and refuses a
`consent` argument: an agent that meets the consent refusal must ask you to run
`logos security scan --consent`. No MCP tool installs a scanner; `security install`
is a command you run. `logos:review` returns the branch delta with its security
section ([Reviewing what a branch introduced](#reviewing-what-a-branch-introduced)).

## When something goes wrong

| You see | Why |
|---|---|
| `security scan` exits 2 naming `--consent` | No consent is recorded for a scanner it would start. Nothing ran |
| `security scan` or `security install` exits 3: `… is already running on this project` | Another scan or install of this project runs, named in the message. Nothing was started. Wait for it, or cancel it on the Security page if it is a web job, then run it again |
| `security install` exits 2 listing tools, versions and sources | The install consent is missing; run it again with `--consent` |
| `security install` exits 1 | Every tool you asked for failed or has no release for this platform: none ended `installed`, `already_present` or kept. Read each `outcome` and `reason` in the output; one tool in place is exit 0 |
| `check` or `gate` exits 5 | A required scanner has no fresh scan at HEAD; run the command it names |
| A scanner reads `stale` after you re-scanned a file | Only a full scan at HEAD makes it `fresh` (`scoped_only`, `head_moved`) |
| A scanner is `failed` | Its `cause` names why in one line; the whole bounded output is in its `log` (`.logos/security/<scanner>.failed.log`): past the bound (64 KiB of stdout, 16 KiB of stderr) it keeps the beginning and the end of each stream, with one line, `[… N bytes omitted …]`, between them, so a scanner's closing error survives. One failure never stops the others |
| Dependency-Check is `failed`, cause `NVD API key missing or rejected …` | Dependency-Check 13 needs an NVD API key and none was supplied (or the one given was refused). Get a free key at https://nvd.nist.gov/developers/request-an-api-key and export `NVD_API_KEY` where Logos runs: in your shell for the CLI, before starting `logos serve` for MCP and the web. Or set `extra_args = ["--nvdApiKey", "<key>"]` under `[security_scan.dependency-check]` in your **user-level** `config.toml`; the repository config may not set `extra_args`. `security status` reads `nvd_api_key: set` once a key is there ([Dependency-Check's NVD API key](#dependency-checks-nvd-api-key)) |
| gosec or Bandit is `not_applicable` | No Go (or Python) file outside the ignored paths: there is nothing for it to scan, and nothing is missing |
| gosec is `failed`, cause `gosec cannot type the standard library: gosec <version> with the local go<version> …` | gosec could not type a standard-library package the project imports (its log holds `internal error: package "…" without types was imported`) and wrote no report. The managed gosec before 2.29.0 does this under Go 1.27. Run `security install gosec --update` for the pinned 2.29.0, built for Go 1.27, or use a gosec release built for your Go, or a Go toolchain gosec supports (`go version` shows yours). Until then ingest gosec's SARIF from CI |
| Exit 2 on a `path` or `extra_args` key in `.logos/config.toml` | A repository may only set `enabled`; set the rest in your user-level config |

The troubleshooting table in [error-handling.md](error-handling.md#troubleshooting-common-errors)
has the full list.
