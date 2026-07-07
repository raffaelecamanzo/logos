//! `scip` containment fitness function (NFR-MA-07, ADR-06).
//!
//! ADR-06 confines the `scip` rust-protobuf codec behind the private
//! `model::convert` seam so no protobuf type ever crosses a public boundary
//! (NFR-MA-07). That invariant was previously enforced only by grep + review —
//! a stray `use scip::…` in a new module would compile and pass `cargo test`.
//!
//! This test enforces it *structurally*, mirroring the `no_network_deps.rs`
//! fitness function: it walks `logos-core/src`, skips the one sanctioned seam
//! file (`model/convert.rs`), and fails the build if any other source file
//! names a `scip::` path. A regression is caught at `cargo test` time rather
//! than slipping through review.

use std::fs;
use std::path::{Path, PathBuf};

/// The single source file allowed to name `scip::` — the ADR-06 seam.
const SEAM: &str = "model/convert.rs";

/// The `scip::` path token a leak would contain.
const NEEDLE: &str = "scip::";

/// Collect every `.rs` file under `dir`, recursively.
fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("src dir must be readable") {
        let path = entry.expect("readable dir entry").path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn scip_is_confined_to_the_convert_seam() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let seam = src.join(SEAM);

    let mut files = Vec::new();
    rust_sources(&src, &mut files);
    assert!(seam.is_file(), "the {SEAM} seam file must exist");

    let leaks: Vec<String> = files
        .iter()
        .filter(|file| **file != seam)
        .filter(|file| {
            fs::read_to_string(file)
                .expect("source file must be readable")
                .contains(NEEDLE)
        })
        .map(|file| {
            file.strip_prefix(&src)
                .unwrap_or(file)
                .display()
                .to_string()
        })
        .collect();

    assert!(
        leaks.is_empty(),
        "scip containment violated (NFR-MA-07, ADR-06): {NEEDLE:?} appears \
         outside the sanctioned {SEAM} seam in {leaks:?}. The scip/protobuf \
         codec must stay confined to model::convert so no protobuf type crosses \
         a public boundary."
    );
}
