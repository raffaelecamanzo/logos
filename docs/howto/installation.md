# Installation

Logos ships as a **single static binary with zero runtime dependencies** —
no network access, no external services, no system libraries. SQLite and the
bundled tree-sitter grammars are compiled in.

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

`init -i` interactively offers to inject the `logos` MCP server block into
the project's `.mcp.json`, write the managed `CLAUDE.md` usage block, and
(optionally, also available as `logos init --hooks`) install git hooks that
keep the graph synced on commit/checkout/merge. Every step is idempotent and
non-clobbering — re-running `init` never overwrites your edits. Restart your
agent afterwards: the `logos:*` tools appear automatically.

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

- **Plain build — no Node required.** `cargo build -p logos --release` (no `ui`
  feature) builds the CLI with no web surface and no Node toolchain.
- **`--features ui` without a built SPA — still Node-free.** A committed
  placeholder shell keeps `cargo build -p logos --release --features ui`
  compiling with no Node present; `serve --ui` then serves a minimal placeholder
  shell (enough to verify wiring, not the full interface).
- **The rich SPA — build it first, then embed.** Compile the SPA bundle with a
  Node toolchain, then build with `--features ui` to embed it:

  ```bash
  cd web/ui
  npm ci          # restore the pinned build-time toolchain (never shipped)
  npm run build   # emit the hashed bundle into web/ui/dist/
  cd ../..
  cargo build -p logos --release --features ui
  ```

Released binaries (Homebrew, the installer, the archives) always embed the
real SPA — CI runs the `npm run build` step once and embeds the bundle into
every target, so an installed `logos` needs nothing from you.

### Slim builds (optional)

Grammar support is feature-gated. For a smaller, faster-to-compile binary
restricted to Rust-only analysis:

```bash
cargo build -p logos --release --no-default-features --features lang-rust
```

`logos languages` on such a build lists only the compiled-in grammars. The
full set returns with the default `lang-all` feature.

## Smoke test

```bash
cd /path/to/any/project
logos index     # exit 0; creates ./.logos/logos.db
logos status    # index health summary
```

That's the entire setup — there is no daemon, no account, no configuration
required to start. Configuration is optional and covered in
[configuration.md](configuration.md).
