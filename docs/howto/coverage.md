# Coverage runs

Logos can run your project's own coverage tools, ingest what they write, and show
the result on the Coverage page. This page is the whole story: which tools Logos
knows, what a run executes and what you consent to, the commands, the Coverage
page and its workspace roll-up, and the user-level settings that decide what runs.

Coverage is an advisory evidence tier. A run never moves the quality signal, the
quality gate never reads `history.db`, and a run starts only when you or an agent
you have set up asks for one: the `serve` watcher and the timers never start one.
Ingesting a report you produced yourself (`logos coverage ingest <report>`) needs
no consent and runs nothing; see [commands.md](commands.md#coverage-ingest-report---format-fmt).

- [The registry: seven tools](#the-registry-seven-tools)
- [The execution consent](#the-execution-consent)
- [Running coverage: `coverage run` and `coverage refresh`](#running-coverage-coverage-run-and-coverage-refresh)
- [What a tool run ends as](#what-a-tool-run-ends-as)
- [Reading the state: `coverage status`](#reading-the-state-coverage-status)
- [Installing `cargo-llvm-cov` and adding a missing dependency](#installing-cargo-llvm-cov-and-adding-a-missing-dependency)
- [The Coverage page](#the-coverage-page)
- [The Workspace Coverage page](#the-workspace-coverage-page)
- [User-level overrides](#user-level-overrides)
- [From an agent: MCP](#from-an-agent-mcp)

## The registry: seven tools

Logos has a built-in registry of seven coverage tools. Each has a **marker** file at the
project root that selects it, a fixed command line, and a report it writes. A
repository can only switch a tool off; it can never change a command, a path or a
flag ([configuration.md](configuration.md#coverage-tools--which-may-run-and-as-what)).

| Tool | Marker (selects it) | Invocation (program first, then its arguments) | Report | Tested |
|---|---|---|---|---|
| `cargo-llvm-cov` | `Cargo.toml` | `cargo-llvm-cov llvm-cov --workspace --cobertura --output-path <rust.xml> [extra_args]`, with `CARGO_LLVM_COV_SETUP=no` | Cobertura, `.logos/coverage/rust.xml` | 0.6 (managed copy: 0.9.1) |
| `vitest` | `package.json` declaring `vitest` | `vitest run --coverage.enabled --coverage.reporter=cobertura --coverage.reportsDirectory=<dir> --coverage.reportOnFailure [extra_args]` | Cobertura, `.logos/coverage/vitest/cobertura-coverage.xml` | 3.2 |
| `jest` | `package.json` declaring `jest` | `jest --ci --coverage --coverageReporters=cobertura --coverageDirectory=<dir> [extra_args]` | Cobertura, `.logos/coverage/jest/cobertura-coverage.xml` | 29 |
| `pytest-cov` | `pyproject.toml`, `setup.cfg` or `pytest.ini` | `pytest --cov --cov-report=xml:<python.xml> [extra_args]` | Cobertura, `.logos/coverage/python.xml` | 5.0 (pytest 8) |
| `maven` | `pom.xml` | `mvn --batch-mode [extra_args] org.jacoco:jacoco-maven-plugin:0.8.13:prepare-agent test org.jacoco:jacoco-maven-plugin:0.8.13:report` | JaCoCo XML, each module's `target/site/jacoco/jacoco.xml` | Maven 3.9 |
| `gradle` | `build.gradle` or `build.gradle.kts` | `gradle --no-daemon --init-script <jacoco-init.gradle> [extra_args] test jacocoTestReport` | JaCoCo XML, each project's `build/reports/jacoco/test/jacocoTestReport.xml` | Gradle 8 (7 and later) |
| `go` | `go.mod` | `go test -coverprofile=<go.out> [extra_args] ./...` | Go coverprofile, `.logos/coverage/go.out` | Go 1.23 |

How to read the table:

- **Marker.** A tool applies when its marker is at the project root. A `package.json` declares a
  tool only in its `dependencies` or `devDependencies`. A project with no marker for a tool reads
  that tool as `not_applicable`.
- **Invocation.** Every tool is started from an argument list in a process group of its own,
  never through a shell, with the project root as its working directory. `<…>` is a path Logos
  chooses under `.logos/coverage/`; `[extra_args]` is your own user-level list, inserted at that
  fixed place ([User-level overrides](#user-level-overrides)). Nothing in the repository fills any
  of it. The one command that does run through a shell is the custom `refresh_cmd`, covered below.
- **Where the program comes from.** `./mvnw` and `./gradlew` are preferred over `mvn` and
  `gradle` on `PATH`. `vitest` and `jest` run only from `node_modules/.bin`, never through `npx`, so
  a run downloads no package; `vitest` also needs a coverage provider
  (`@vitest/coverage-v8` or `@vitest/coverage-istanbul`) installed beside it. `pytest-cov` runs the
  project's `.venv` pytest, else `pytest` on `PATH`, and only when the `pytest-cov` plugin is installed
  with it. `cargo-llvm-cov` is your user-level path override, else a `PATH` copy at 0.6.0 or later,
  else the copy `logos coverage install` manages (a `PATH` copy below 0.6.0 runs only while there is
  no managed copy). `go` is `go` on `PATH`, probed with
  `GOTOOLCHAIN=local` so that finding its version fetches no toolchain.
- **Detection starts no copy your project supplies.** A binary outside the project is asked for its
  version with a command that reaches no network; for `vitest` and `jest` the version is read from
  `node_modules/<package>/package.json`, for a wrapper from its properties file, and for the
  `.venv`'s pytest from the installed `pytest_cov` metadata.
- **What each template adds.** `vitest` gets `--coverage.reportOnFailure`, so a failing suite
  still writes a report. `jest` gets `--ci`, so a test meeting a new snapshot fails instead of writing
  one into your project. `cargo-llvm-cov` runs with `CARGO_LLVM_COV_SETUP=no`, so a missing
  `llvm-tools-preview` fails the run, naming the command, instead of changing your toolchain.
- **JaCoCo.** Maven gets the JaCoCo plugin **0.8.13** on its command line, pinned in this
  Logos release. Gradle gets JaCoCo from an init script that Logos writes to
  `.logos/coverage/jacoco-init.gradle` before each run and passes with `--init-script`; the run
  uses `--no-daemon`, so no Gradle daemon outlives it. **Neither build file is edited.**
  A failing test stops a Maven or Gradle build before its report task, so a failing Maven or
  Gradle suite writes no report and reads *failed*, with its stderr.
- **Tested versions.** The command lines were written against the versions in the last column and
  are pinned by tests that run stub executables and compare their argument lists; no real tool is
  run by that suite. A tool whose flags have changed since reports *failed* with its own
  stderr, never a wrong figure.

The ingest side reads Cobertura, JaCoCo XML and Go coverprofile (and LCOV), and rejects a
malformed report before it writes anything. A path a report names that binds to no indexed file, or
to more than one, is counted `unmatched`, never guessed.

## The execution consent

**A coverage run executes your project's own code.** Building a Rust project runs its build
scripts and procedural macros; `cargo-llvm-cov`, `vitest`, `jest`, `pytest-cov`, Maven, Gradle and `go test`
all compile and run your tests, and so run whatever those tests, your test configuration,
your build files and your dependencies' install and build hooks contain. A run may also reach the
network to fetch dependencies. That is why no run starts in a project until you have recorded an
**execution consent** for it.

The consent:

- **states** that the run builds the project and runs its tests, which executes the project's
  own code, and that it may reach the network to fetch dependencies;
- **names the tools** that will run, by registry name;
- when `[coverage_ingest].refresh_cmd` is set, **shows that command's exact text** and records the
  SHA-256 of it. Any control or bidirectional-formatting character in the text is shown as an
  escape (`\u{1b}`), so what you read is what runs.

Without it, `logos coverage run`, `logos coverage refresh` and an MCP run exit 2 (or refuse) naming
`--consent` and start **nothing**, not even a version probe. `--consent` records exactly what was
missing and then runs:

```text
error: invalid value for consent: no coverage execution consent is recorded in this project for custom — a coverage run builds this project and runs its tests, which executes the project's own code, and it may reach the network to fetch dependencies. The custom command it runs is, verbatim:
echo hi
 Run `logos coverage refresh --consent` in a terminal to record it (an MCP client cannot); later runs will not ask again until another tool is enabled or the command changes
```

**When it asks again:**

- **A tool the consent does not name is enabled or newly detected.** It is asked for alone; the
  tools already named are not asked about again.
- **The `refresh_cmd` text changes, by one character.** The run shows the new text verbatim and
  asks again. The command lives in the repository's `.logos/config.toml`, so pulling a change that
  edits it is exactly the case this exists for.

**Where it is kept.** In `coverage-consent.toml` in the [user-level config
directory](configuration.md#user-level-config--settings-no-repository-can-choose), keyed by the
project's canonical root path, so each checkout and each worktree has its own consent. It is never
written inside the repository, and it is separate from the security scan consent. To withdraw it,
delete the project's table from the file, or the file. `logos coverage status` reports it.

**What the consent protects against:**

- a **repository that changes what runs.** A committed `[coverage_ingest].tools` can only switch a
  tool on or off: a path, a command or a flag there fails the load with exit 2 before anything
  starts.
  The binary path, extra arguments and timeout come from your user-level config alone;
- a **committed command that changes unnoticed.** The `refresh_cmd` is pinned by the hash of
  its text, so a clone or a pull cannot swap it for another one without the prompt;
- an **agent that approves itself.** An MCP client cannot pass `consent`; it relays the
  instruction. The Coverage page asks you in a dialog;
- a run that **starts without you.** The watcher, the timers and the Dashboard never start one;
  the Dashboard reads stored evidence and probes no tool.

**What it does not protect against.** The consent is permission to run, not a review of what
runs, and it is not a sandbox:

- it does **not** inspect or vet your tests, build files, build scripts, wrappers (`./mvnw`,
  `./gradlew`) or dependencies. They run with your user's permissions, as they do when you run
  your own tests;
- a tool's consent names the tool, not its behaviour: editing a test, a build script or a
  dependency does **not** ask again; only a new tool or a changed `refresh_cmd` does;
- it does not limit the network a build uses, and it does not cap what the build writes;
- consenting to a repository you do not trust is as risky as running its test suite by hand. Read
  what you are about to run first, or run it somewhere you would run untrusted code.

## Running coverage: `coverage run` and `coverage refresh`

```bash
logos coverage run                          # every enabled, installed tool, then the custom command
logos coverage run --consent                # record the consent the run lacks, then run
logos coverage run --tool vitest            # only this tool (or `custom`)
logos coverage run --consent --dependency-consent   # also add a missing coverage dev dependency
logos coverage refresh --consent            # `coverage run --tool custom`
logos coverage status                       # what ran, what is installed, the consent
logos coverage install --consent            # the managed cargo-llvm-cov and llvm-tools-preview
```

`coverage run` runs every enabled, installed registry tool in registry order, then the custom
entry when `[coverage_ingest].refresh_cmd` is configured, and ingests each tool's report into the
snapshot at HEAD as soon as that tool ends. A polyglot project ends with one merged snapshot. One
tool failing never stops the others. `--tool <id>` runs only the named tool: a registry name
(`cargo-llvm-cov`, `vitest`, `jest`, `pytest-cov`, `maven`, `gradle`, `go`) or `custom`.

**The custom command.** `coverage refresh` is `coverage run --tool custom`: it runs the
configured `[coverage_ingest].refresh_cmd` through `sh -c` in the project root, then ingests the
newest conventional or configured coverage artifact the command wrote or rewrote; one an earlier
run left is never credited. It is the one command Logos starts through a shell, because the
consent pins its exact text and you approved that text. With no `refresh_cmd` configured it fails
with exit 3 and starts nothing. Ctrl-C (or SIGTERM) on a run kills the tool it started, with its whole process
group.

**Exit codes.** `coverage run` and `coverage refresh` exit 0 when every tool the run started ran
cleanly, 1 otherwise (a failing suite whose report was ingested included), 2 when the consent is
missing, a config key is rejected or `--tool` names an unregistered tool, and 3 when `--tool
custom` (or `coverage refresh`) finds no `refresh_cmd`. Naming `--tool` makes a tool that is not applicable,
disabled or not installed an exit 1; without it, those are not failures. With
`--dependency-consent`, a coverage dependency that was refused or failed to add also exits 1.

**Output.** One outcome per tool, with its version, the reports it ingested and, for a failure, a
masked excerpt of the end of its output: the last lines of its stdout, where a test runner lists
what failed, and of its stderr. The whole bounded output of a failed tool goes to
`.logos/coverage/<tool>.failed.log`: for each stream, the first quarter of the bound and the
rest from the end, with one line, `[… N bytes omitted …]`, saying how much lies between them
(64 KiB for stdout and 16 KiB for stderr; a stream that fits is kept whole). A failing test's
name, its panic message and the runner's `failures:` summary therefore survive a run that
printed far more than the bound. The payload and every log line Logos writes carry the masked
excerpt only. `--json` prints the same as machine-readable JSON. The run records each
started tool's end in `.logos/coverage/last-run.json`, which `coverage status` reads back.

**Timeouts.** A tool that runs past its timeout, 30 minutes by default, has its whole process
group killed and reads `timed_out`.

## What a tool run ends as

Each tool reports exactly one state. No state reads as 0 % or as clean.

| State | Meaning |
|---|---|
| `ran` | Exited 0; each report it wrote was ingested |
| `tests_failed` | *Ran, tests failed*: exited non-zero but wrote a report, which was ingested |
| `failed` | Wrote no report, could not start, or a report did not ingest; a masked stderr excerpt, and the full output in `<tool>.failed.log` |
| `timed_out` | Its timeout passed and its process group was killed |
| `cancelled` | The run was cancelled; nothing it wrote is credited, and every installed tool after it reads `cancelled` without starting |
| `not_applicable` | Its marker is not at the project root |
| `disabled` | Switched off in `[coverage_ingest].tools` |
| `not_installed` | Not found; where Logos can say how to fix it (a JavaScript project without `node_modules`, a missing `cargo-llvm-cov`), the entry carries a hint |
| `never_run` | Detected, with no run yet (`coverage status` only) |

## Reading the state: `coverage status`

```bash
logos coverage status [--repo <MEMBER>]
logos coverage status --json
```

`coverage status` reports per-file freshness and the overall fresh-coverage fraction
([commands.md](commands.md#coverage-status)), plus a `runner` block: each tool's state (its
detection, else its last run, else `never_run`), its last run, and the consent, with the tools a
next run would ask for under `pending` and whether the custom command's text has `command_changed`.
It reads stored evidence and records, and starts no run; the only process it may start is the
version command of a binary outside your project.

## Installing `cargo-llvm-cov` and adding a missing dependency

A project that cannot yet be measured can often be set up by Logos, each step under a consent of
its own. Neither is ever done by a plain run.

- **`logos coverage install [--from <dir>] [--consent]`** installs the pinned, SHA-256-verified
  `cargo-llvm-cov` into the user-level tools directory and adds `llvm-tools-preview` with your own
  `rustup`. A copy of your own at the tested minimum or above is used and never downloaded beside. The install consent is per
  user and covers every project; without `--consent` the command exits 2, naming each line, and
  changes nothing. The full account is in
  [installation.md](installation.md#coverage-runs-on-a-rust-project).
- **`logos coverage run --dependency-consent`** adds a missing coverage dev dependency through the
  project's own package manager, read from its lockfile: `@vitest/coverage-v8` at the vitest
  major the project uses for a vitest without a coverage provider, or `pytest-cov` for the
  project's `.venv` pytest. The run reports it under `dependencies` first: the package, the
  manager (npm, pnpm, yarn or bun; uv or poetry; pip in the `.venv` when `requirements.txt` is the
  only Python lock), the **exact command** and the **files it changes**. The **dependency
  consent** records that exact command for this project, in `coverage-dependency-consent.toml` in
  the user-level directory, never in the repository; a different command, such as one after a vitest
  major upgrade, asks again. It says what you are accepting: the package manager may **run
  dependency lifecycle scripts** and may reach the network. Logos never commits; the change
  stays in your working tree for you to review. Without the flag nothing is installed and the entry
  carries the consent statement.
- **Refused, with the command to run by hand:** no lockfile, a lockfile of a manager Logos does not
  run, lockfiles of two managers, a vitest major that cannot be read, or a manager missing from
  `PATH`.
- **Cancelled or failed additions undo themselves.** A cancelled, failed or timed-out addition
  puts the manifest and lockfile back byte for byte, and removes the added package's own entries it
  had just installed; the run's `restored` field names them.

An MCP client can give neither consent, and MCP has no install tool.

## The Coverage page

The Coverage page leads with a **Coverage tools** widget: one row per tool whose marker is at the
project root, plus the custom command, with its state in the words above, its last run with the
commit's short hash and the files its report matched, and its version when that can be read
without starting a tool (a copy outside the project reads `—`; the command line shows it). A missing tool or a
failing suite never reads as 0 %.

- **Run coverage** on the widget's title row asks the execution consent the first time, in a
  dialog that says the run builds the project and runs its tests, names each tool and shows the
  custom command as the terminal consent shows it. **Allow** sends back exactly what the dialog
  showed.
- The run is a `coverage-run` job on the same job runner as the security scan: one per project at a
  time, so a second click joins the running one, and a run and a security scan can go side by side.
  The button becomes **Cancel**, which kills each tool's whole process group, a forked test JVM or a
  build daemon included. Cancel stops this job alone: an MCP `coverage_refresh` is not a job, so
  Cancel does not stop it ([From an agent: MCP](#from-an-agent-mcp)).
- A per-tool progress list follows the job over Server-Sent Events: queued, running with its
  elapsed time, then each outcome, with a failed tool's error output in a disclosure and one
  polite live line announcing each end. A page opened while a run goes joins it, and when a
  followed run ends the page reloads its figures.
- **Install** (a not-installed `cargo-llvm-cov`) and **Add dependency** (a vitest without a coverage
  provider, or a `.venv` pytest without `pytest-cov`) open the one consent dialog described in
  [usage.md](usage.md#setting-up-a-coverage-tool-from-the-coverage-page).
- With nothing ingested, the page offers **Run coverage** beside the ingest command, or names the
  tools that would apply when none is installed.

The page's start and cancel are intent-guarded `POST`s; a cross-origin or intent-less request is
refused. A start answers `409` with what the consent lacks until the request accepts all of it.

## The Workspace Coverage page

In a workspace, the app-scoped **Workspace Coverage** page (`/workspace-coverage`, the sidebar's
Workspace section) shows one row per member, from `GET /api/v1/workspace/coverage`:

- the member's overall line coverage, over its **fresh** files (unchanged since its last
  report), and how many files are fresh out of those covered;
- how many of its applicable tools ran, and how many failed;
- when a tool last ran, linked to that member's own Coverage page.

A member that was never measured reads **never run**, never 0 %. The figure row states its
denominator, "Coverage from *m* of *N* members; *k* members never run", where *m* counts the
members with fresh coverage. Each percentage is its member's
own: none is summed or averaged across members. A member that cannot be opened is a row that
says so and hides no other. The page reads stored evidence only: it starts no tool and writes
nothing, and has **no Run button**. A run is started on a member's own Coverage page, under that
member's consent.

## User-level overrides

What a tool runs as is set in the [user-level
config](configuration.md#user-level-config--settings-no-repository-can-choose), never in a
repository:

```toml
[coverage_ingest.tools.maven]
path = "/opt/maven/bin/mvn"    # absolute; the only copy used while it is set
extra_args = ["-Pcoverage"]    # one argument per entry, no shell splitting
timeout_secs = 3600            # default 1800
```

`extra_args` are inserted at the fixed place shown in the [registry](#the-registry-seven-tools).
A relative `path`, an empty `extra_args` entry, a `timeout_secs` of 0 and a tool name that is not
registered all fail with exit 2. The repository switches a tool on or off in
`.logos/config.toml`:

```toml
[coverage_ingest]
tools = { gradle = false, jest = false }
```

## From an agent: MCP

`coverage_run` starts the run as a job, or joins the one already running, and returns at once with
the job's id; `coverage_status` carries the job, each tool's events and, once it completed, the
run's outcomes. `coverage_refresh` runs the `custom` tool alone. Under a workspace those two act on
the default member only. Both refuse without the consent, and neither can record it: an agent relays
the `--consent` instruction to you. `coverage_status` only reads: it takes `repo` and reports the
consent, and it never refuses.

`coverage_refresh` is the exception to the job: it is synchronous. It runs the configured
`refresh_cmd` inside the request and answers with the outcome when the command ends, as `logos
coverage refresh` does in a terminal, and not as a `coverage-run` job. So:

- It is not listed as a job: `coverage_status`'s `job` and the page's progress list never show it.
- The Coverage page's **Cancel** does not stop it. Cancel stops the page's own run only.
- A page run and a `coverage_refresh` can run at the same time, and run `refresh_cmd` twice at
  once: the page's runs are single-flight against each other, but `coverage_refresh` takes no job
  slot and no lock, so nothing joins the two. Use `coverage_run` when you want the one job-tracked,
  cancellable run that a page click joins.
- It still requires the recorded consent, which pins the command's exact text: without it, or after
  the text changed, it spawns nothing and returns the `--consent` instruction.
