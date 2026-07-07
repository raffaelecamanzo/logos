//! `logos-mcp` — bare stdio entrypoint for the MCP surface (S-017).
//!
//! Exists so the mcp crate's process-level acceptance tests (UAT-MC-02
//! stdout cleanliness, UAT-MC-04 disconnect cleanup) can spawn a real stdio
//! server without depending on the `cli` crate — which would invert the
//! adapter dependency direction (ADR-01). The shipped user entrypoint is
//! `logos serve --mcp` (cli crate, S-016), which calls the same
//! [`mcp::serve_stdio`].

use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    // Root = argv[1] or cwd — mirrors `serve --mcp --project <root>`
    // (FR-WT-04: the server is rooted at the worktree the agent is in).
    let root = match std::env::args_os().nth(1) {
        Some(arg) => PathBuf::from(arg),
        None => std::env::current_dir()?,
    };
    mcp::serve_stdio(root)
}
