# Installation

Logos ships as a **single static binary with zero runtime dependencies** —
no network access of its own, no external services, no system libraries. SQLite
and the bundled tree-sitter grammars are compiled in. The one download is the
optional `logos security install`, which runs the system `curl` (and `uv`) under
your consent ([Installing scanners](security.md#installing-scanners)), and its
coverage twin `logos coverage install`
([Coverage runs on a Rust project](#coverage-runs-on-a-rust-project)).

## Supported platforms & minimum OS baseline

| Target | Minimum baseline |
|--------|------------------|
| macOS Apple Silicon (`aarch64-apple-darwin`) | macOS 11 Big Sur |
| macOS Intel (`x86_64-apple-darwin`) | macOS 10.12 Sierra |
| Linux x86_64 (`x86_64-unknown-linux-musl`) | any distro, kernel ≥ 3.2 (fully static musl — no glibc required) |
| Linux ARM64 (`aarch64-unknown-linux-musl`) | any distro, kernel ≥ 3.2 (fully static musl — no glibc required) |

Linux builds are **musl-only by design**: there is no glibc variant and no
install-time libc detection — the same artifact runs on Alpine, Debian,
NixOS, or a scratch container. Windows is not supported in v1.

## Which release the installers deliver

Homebrew, the shell installer and a manual download all fetch the newest release
on the [Releases page](https://github.com/raffaelecamanzo/logos/releases): the
default build for your target, with the web UI embedded. For now those releases
are built by this repository's own release workflow, from its release tags (see
[CI integration → the release train](ci-integration.md#the-release-train-what-a-tag-publishes)),
until the first release built outside this repository takes over the same names.
The formula, the installer URL and the archive names do not change when that
happens, so `brew upgrade` or re-running the installer moves to it.

## Install with Homebrew (macOS)

```bash
brew install raffaelecamanzo/tap/logos
```

## Install with the shell installer (macOS / Linux)

```bash
curl -fsSL https://github.com/raffaelecamanzo/logos/releases/latest/download/logos-installer.sh | sh
```

The installer detects your platform, downloads the matching artifact, and
installs to `~/.cargo/bin` when present, else `~/.local/bin`.

## Manual download

Grab the archive for your target from the
[Releases page](https://github.com/raffaelecamanzo/logos/releases), unpack,
and put `logos` on your `PATH`. Every archive ships the binary together with
`THIRD-PARTY-NOTICES.md` (the aggregated third-party attributions; Logos
itself is Apache-2.0-licensed).

## Verify the installation

```bash
logos --version          # prints the version
logos languages --json   # lists the compiled-in grammars; "skipped" must be []
```

The default binary carries all twelve code-language grammars: **Rust, Python,
TypeScript/JavaScript (+ TSX/JSX), Go, Java, C, C++, C#, Kotlin, Scala, Ruby,
PHP** — the five v1 languages plus the seven added in CR-009 — alongside the
`markdown` documentation grammar and the config/infra artifact grammars.

## First five minutes

From your project root:

```bash
logos init -i   # .logos/ policy files + MCP host wiring + managed CLAUDE.md
logos index     # build the code graph into .logos/logos.db
logos status    # confirm a populated, fresh index
```

`init -i` writes the policy files, injects the `logos` MCP server block into the
project's `.mcp.json`, writes the managed `CLAUDE.md` usage block and the
session-start readout hook, and on a terminal asks whether to install git hooks
(the default is no; `logos init --hooks` installs the hooks that keep the graph
synced on commit, checkout and merge and gate a push). Plain `logos init` writes
only the policy files and the store inside `.logos/`. Every step is idempotent and
non-clobbering — re-running `init` never overwrites your edits. Restart your
agent afterwards: the `logos:*` tools appear automatically. `logos init
--uninstall` removes what `init` wrote outside `.logos/`.

### Claude Code: install the plugin

On Claude Code, the plugin carries the MCP server and the session-start readout,
and adds the review loop (a Stop check, the `/logos:review` skill and a
guardrail). It needs `logos` 1.20.0 or newer on `PATH`:

```bash
claude plugin marketplace add raffaelecamanzo/logos
claude plugin install logos@logos
logos init -i   # with the plugin enabled: enrols it, no .mcp.json entry
logos index
```

See [The Claude Code plugin and the review loop](plugin.md) for what each part
does, how to migrate a project that ran `init -i` before the plugin existed, and
the off switches.

## Coverage runs on a Rust project

`logos coverage run` measures a Rust project with `cargo-llvm-cov`, which needs
the `llvm-tools-preview` component on your Rust toolchain. If you have neither,
`logos coverage status` reports `cargo-llvm-cov` as `not_installed` and names
the command that installs both:

```bash
logos coverage install            # until you consent: names what it would download and run; changes nothing
logos coverage install --consent  # records your consent, then installs
logos coverage install --from <dir> --consent   # offline, from pre-downloaded archives
```

Until your consent is recorded, the first command refuses (exit 2) and lists
each line it needs your consent for: the pinned `cargo-llvm-cov` release with its source and size, and
`rustup component add llvm-tools-preview` on your active Rust toolchain. Nothing
is downloaded and no `rustup` command runs until you consent. With `--consent`:

- `cargo-llvm-cov` is downloaded with the system `curl`, its SHA-256 is checked
  against the value pinned in this Logos release **before** it is unpacked or
  run, and it is moved into the user-level tools directory
  (`$LOGOS_DATA_HOME`, else `$XDG_DATA_HOME/logos/tools`, else
  `~/.local/share/logos/tools`). A file that fails the check is deleted, and
  nothing from it runs.
- `llvm-tools-preview` is added by **your own** `rustup`, run from the
  filesystem root so that no project's `rust-toolchain.toml` picks the
  toolchain. Logos never downloads the component itself. A project that pins
  another toolchain needs the component there too: the run then fails naming
  `rustup component add llvm-tools-preview --toolchain <name>`.

The consent covers that `cargo-llvm-cov` version, and the `rustup` command, in
every project; a later Logos that pins a newer version asks again. It is recorded
beside the scan consent, in `security-install-consent.toml` and
`toolchain-consent.toml`.

**Your own copy wins.** A user-level path override, or a `cargo-llvm-cov` on
`PATH` at 0.6.0 or later, is used and never downloaded; `coverage install`
reports it as kept, and `coverage status` names the copy in use: `override`,
`path` or `managed`. A `PATH` copy below 0.6.0 is used only while there is no
managed copy: `coverage install` installs one beside it, which runs from then on.

**Without `rustup`** (a distribution's Rust packages, for example), the install
reports `llvm-tools-preview` as `no_rustup` with the command to run by hand, and
changes nothing on your toolchain. Install LLVM's tools your toolchain's own way,
or point `cargo-llvm-cov` at them with `LLVM_COV` and `LLVM_PROFDATA`.

A coverage run never adds the component on its own: `cargo-llvm-cov` runs with
`CARGO_LLVM_COV_SETUP=no`, so a missing component fails the run with the
command to run, instead of changing your toolchain without asking. MCP has no
install tool: an agent cannot grant itself the consent, so `coverage_status`
names the command for you to run.

## Build from source (alternative)

Requires only a recent stable Rust toolchain:

```bash
git clone https://github.com/raffaelecamanzo/logos.git
cd logos
cargo build -p logos --release
```

The binary lands at `target/release/logos` (`cargo install --path cli`
installs it to `~/.cargo/bin/logos`).

### Building the web UI (`serve --ui`)

The web interface (`logos serve --ui`) is a client-side **React single-page
app** embedded into the binary at build time. It is served **same-origin from
the same loopback binary** — there is **no Node runtime, no network, and no
external service at serve time**; the single-binary and offline-by-default
guarantees are unchanged. Node is needed **only at build time**, and only when
you want the rich SPA compiled into your own build.

- **Slim build — no Node required.** `cargo build -p logos --release
  --no-default-features --features lang-all` builds the CLI and the MCP server
  with no web surface and no Node toolchain.
- **Default build without a built SPA — still Node-free.** The `ui` feature is on
  by default, and a committed placeholder shell keeps `cargo build -p logos
  --release` compiling with no Node present; `serve --ui` then serves a minimal
  placeholder shell (enough to verify wiring, not the full interface).
- **The rich SPA — build it first, then embed.** Compile the SPA bundle with a
  Node toolchain, then build with the `ui` feature (a default) to embed it:

  ```bash
  cd web/ui
  npm ci          # restore the pinned build-time toolchain (never shipped)
  npm run build   # emit the hashed bundle into web/ui/dist/
  cd ../..
  cargo build -p logos --release --features ui
  ```

Binaries installed through Homebrew, the installer or the archives embed the
real SPA, so an installed `logos` needs nothing from you.

### Browser layout tests (contributors)

The web UI's layout rules — equal gaps between stacked widgets, left-aligned
widget text, one body font size — are checked in a real browser, on every view
of both sidebars, by Playwright specs under `web/ui/e2e/`. They need a Chromium build, installed **once** per
machine:

```bash
cd web/ui
npm ci
npx playwright install chromium   # one-time download into Playwright's cache
```

`bash scripts/gate.sh full` runs them as its `ui-e2e` leg: it builds the SPA,
builds `logos` from the tree, serves two checked-in fixtures (a single
repository and a two-member workspace) and prints how many specs passed. The
fast tier never runs them. The leg never passes by running nothing; when it
fails, its verdict says why:

| Verdict | It means | Do this |
|---|---|---|
| `missing_browser` | Playwright found no Chromium | `npx playwright install chromium` in `web/ui` |
| `killed` | The run passed its time limit (`UI_E2E_TIMEOUT`, default 900 s) and was stopped with every process it started | Look for a spec or fixture server that hangs; raise `UI_E2E_TIMEOUT` only for a genuinely slow machine |
| `skipped` | A spec was listed but did not run, so a layout check went unchecked | Remove the `test.skip` (or the skip condition) that caused it |
| `no_binaries` | There was no usable report, or nothing ran | Read the leg's log; usually the SPA or the `logos` build failed first |

To run the specs alone after building the SPA and the binary, use
`npm run test:e2e` in `web/ui`.

### Build modes

Two build modes ship from this repository: the **default** build (the CLI, the MCP
server and the web dashboard) and the **slim** headless build. The command for each
and what each links are in the
[build matrix](usage.md#build-matrix--dashboard-or-slim-headless).

### Slim builds (optional)

Grammar support is feature-gated. For a smaller, faster-to-compile binary
restricted to Rust-only analysis:

```bash
cargo build -p logos --release --no-default-features --features lang-rust
```

`logos languages` on such a build lists only the compiled-in grammars. The
full set returns with the default `lang-all` feature. A `--no-default-features`
build also leaves out the web dashboard (`ui`); add it back by name —
`--features lang-rust,ui` keeps `logos serve --ui` in a Rust-only build.

### A build without the dashboard (contributors)

The slim headless build keeps the CLI and the MCP server and drops the web
surface:

```bash
cargo build -p logos --release --no-default-features --features lang-all
```

`logos serve --ui` is not an option there (clap exits `2`).

`cargo test -p logos` checks the default build's command, route and tool surface
against the committed snapshots under `cli/tests/surface_snapshots/` (and
`cli/tests/no_extension_surface.rs`). `bash scripts/gate.sh full` runs it with the
rest of the suite.

### The local gate and CI (contributors)

`bash scripts/gate.sh fast` and `full` test every package that declares a
`lang-all` feature without enabling it by default (today `logos-core` and `web`)
with `--features lang-all`, in the `test` leg. The gate reads that from
`cargo metadata`, not from a package name. It matches what
`cargo test --workspace` in CI turns on for those packages through the `cli`
crate's defaults, so tests gated on `lang-all` run locally too. Each test unit's
evidence records the exact `cargo` command it ran, and the leg prints a
`reproduce:` line for each package whose flags differ from its leg's.
`bash scripts/tests/test-gate-pkg-features.sh` checks the rule against fixture
metadata.

Both tiers also run a `public-terms` leg, `scripts/check-public-terms.sh`. It
reads every text file `git ls-files` lists, plus the untracked files `git add -A`
would take, and fails naming `file:line:term` when a line holds a listed word or
pair of adjacent words. Matching is whole-word and case-insensitive. Punctuation
and camelCase humps separate words.

- **The list.** It is stored only as SHA-256 digests, in
  `scripts/public-terms.sha256`. To add a term, append
  `printf '%s' 'word' | shasum -a 256`.
- **Exceptions.** `scripts/public-terms.allow` admits reviewed lines by path and a
  digest of the whole line, so an entry lapses when its line changes.
  `--emit-allow` prints candidate entries for the current hits.
- **Failures.** A walk that reads no file fails.

The leg prints the files it scanned. `scripts/verify-evidence.sh` refuses fast-
and full-tier evidence without it, and CI runs it first on the checkout.

The shell scripts' own suites (`scripts/tests/test-*.sh`) run in a
`script-suites` leg, one evidence unit per suite (a suite that exits non-zero is
a failed unit). `gate.sh full` always runs it. `gate.sh fast` runs it when
`git diff --name-only` against the gate base touches `scripts/`, `plugins/` or
`.claude-plugin/` (the Claude Code plugin's scripts are tested by
`scripts/tests/test-plugin-*.sh`), and prints a note otherwise. A glob that
matches no suite fails the leg. The leg prints each suite with its counts.
`scripts/verify-evidence.sh --tier full` refuses evidence without the leg or
with a failed suite. `--tier fast` validates it when present. CI runs every
suite right after checkout. To run them by hand:

```bash
for t in scripts/tests/test-*.sh; do bash "$t" || echo "FAILED: $t"; done
```

## Smoke test

```bash
cd /path/to/any/project
logos index     # exit 0; creates ./.logos/logos.db
logos status    # index health summary
```

That's the entire setup — there is no daemon, no account, no configuration
required to start. Configuration is optional and covered in
[configuration.md](configuration.md).
