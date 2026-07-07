//! Surface line-count budget guard for the MCP adapter (NFR-MA-02, ADR-01).
//!
//! Extended in S-017 (per the original note) to sum non-blank Rust lines
//! across ALL of `mcp/src/` — lib root, server module, and the test-harness
//! bin — so the thin-surface invariant covers the whole crate.
//!
//! The `server-instructions` text (src/instructions.md) is data, not code:
//! it is excluded by construction (only `.rs` files are counted).

use std::path::Path;

/// Recursively count non-blank lines of every `.rs` file under `dir`.
fn non_blank_rust_lines(dir: &Path) -> usize {
    let mut total = 0;
    for entry in std::fs::read_dir(dir).expect("read src dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            total += non_blank_rust_lines(&path);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            let source = std::fs::read_to_string(&path).expect("read source file");
            total += source.lines().filter(|l| !l.trim().is_empty()).count();
        }
    }
    total
}

/// Budget: ≤ 690 non-blank lines of Rust across the whole MCP adapter
/// (NFR-MA-02 thick-core/thin-surface invariant).
///
/// Derivation (combined S-020, S-022, S-048, S-051, S-053 re-base): 28 `#[tool]`
/// registrations (8 navigation, 11 quality, 1 temporal `hotspots`, 3 coverage per
/// CR-007/CR-036, 5 wiki per CR-008/CR-062), each a mechanical attribute, signature,
/// and ONE Engine call — cost ~15 non-blank lines each ≈420; the typed parameter
/// structs (6 navigation, 4 quality, 1 temporal, 1 coverage, 3 wiki) ≈135; the
/// fixed scaffolding (serve entrypoint, the shared ADR-03 bridge with its ADR-14
/// error mapping, two `invalid_params` token parsers, ServerHandler info, bin)
/// ≈165; hosting the core-owned watcher handle for the serve loop's lifetime
/// (S-022, zero policy per ADR-03) ≈10. Any growth beyond this means logic is
/// leaking into the surface — move it to logos-core instead of raising the budget.
/// (S-053 raised 575→660 for the four wiki twins; S-140/CR-036 raised 660→675 for
/// the `coverage_refresh` twin, a single `Engine::coverage_refresh` delegation;
/// S-204/S-205/CR-052 raised 675→680 for the `doctor` and `verify` quality twins,
/// each a single `Engine::doctor`/`Engine::verify` delegation; S-263/CR-062 raised
/// 680→690 for the `wiki_materialize` twin, a single `Engine::wiki_materialize`
/// delegation — logic lives in logos-core governance/wiki, not the surface.)
#[test]
fn mcp_surface_line_budget() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let non_blank = non_blank_rust_lines(&src);

    assert!(
        non_blank <= 690,
        "mcp adapter exceeds the 690 non-blank LOC budget (NFR-MA-02): \
         found {non_blank} lines — move logic to logos-core"
    );
}
