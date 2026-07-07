//! SPA design-system conformance tests (S-193, CR-050, FR-UI-23, ADR-44).
//!
//! These assert the machine-checkable half of the design-system contract over the
//! AUTHORED source (the token + base stylesheets, the theme bootstrap, the
//! component CSS Modules) — the source of truth a Vite build extracts to external
//! hashed CSS. The CSP-cleanliness of the *built* bundle is guarded separately by
//! `tests/spa_bundle.rs` (over the embedded bytes); the legacy server-rendered
//! design system is guarded by `tests/design_system.rs` (over `assets/logos.css`).
//! This file is the SPA-design-system analog: tokens, dark-first theming, the
//! signal-only-red invariant, WCAG 2.1 AA contrast in BOTH themes, the
//! :focus-visible ring, reduced-motion, and the no-flash theme bootstrap.
//!
//! The contrast checks model the WCAG 2.1 relative-luminance formula over the
//! resolved semantic-token pairs in each theme — there is no headless browser in
//! CI, so the token values are resolved and compared directly, the same pattern
//! the legacy `design_system.rs` busy-overlay cascade guard uses.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// `<root>/web/ui` — the SPA project root (CARGO_MANIFEST_DIR is `<root>/web`).
fn ui_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("ui")
}

fn read(rel: &str) -> String {
    let path = ui_dir().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Remove `/* … */` CSS comments so they can't pollute selector/value scans.
fn strip_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(open) = rest.find("/*") {
        out.push_str(&rest[..open]);
        match rest[open + 2..].find("*/") {
            Some(close) => rest = &rest[open + 2 + close + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// The `{ … }` block body immediately following the first occurrence of `needle`,
/// brace-matched. `needle` must select the rule (e.g. `:root`, or a full attribute
/// selector). Comments must already be stripped.
fn block_after(css: &str, needle: &str) -> String {
    let i = css.find(needle).unwrap_or_else(|| panic!("selector `{needle}` not found"));
    let rest = &css[i..];
    let open = rest.find('{').expect("rule has an opening brace");
    let bytes = rest.as_bytes();
    let mut depth = 0i32;
    let start = open + 1;
    let mut j = open;
    while j < bytes.len() {
        match bytes[j] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return rest[start..j].to_string();
                }
            }
            _ => {}
        }
        j += 1;
    }
    panic!("unterminated block for `{needle}`");
}

/// Parse `--name: value;` custom-property declarations from a block body into a map.
fn declarations(body: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for decl in body.split(';') {
        let Some((name, value)) = decl.split_once(':') else { continue };
        let name = name.trim();
        if !name.starts_with("--") {
            continue;
        }
        map.insert(name.to_string(), value.trim().to_string());
    }
    map
}

/// Resolve a token value to a concrete value, following `var(--x)` references
/// through `map` (with `base` as the fallback scope for primitives). Returns the
/// resolved string (a hex/rgb/keyword literal).
fn resolve(value: &str, map: &BTreeMap<String, String>, base: &BTreeMap<String, String>) -> String {
    let mut v = value.trim().to_string();
    for _ in 0..16 {
        let Some(inner) = v.strip_prefix("var(").and_then(|s| s.strip_suffix(')')) else {
            return v;
        };
        // var(--x) or var(--x, fallback) — take the first name.
        let name = inner.split(',').next().unwrap_or("").trim();
        v = map
            .get(name)
            .or_else(|| base.get(name))
            .cloned()
            .unwrap_or_else(|| panic!("unresolved var `{name}`"));
    }
    panic!("var resolution did not terminate for `{value}`");
}

/// Parse a `#rrggbb` (or `#rgb`) literal into linear-ready 0–255 channels.
fn hex_rgb(hex: &str) -> (u8, u8, u8) {
    let h = hex.trim().trim_start_matches('#');
    let full = match h.len() {
        3 => h.chars().flat_map(|c| [c, c]).collect::<String>(),
        6 => h.to_string(),
        _ => panic!("not a hex colour: `{hex}`"),
    };
    let n = u32::from_str_radix(&full, 16).unwrap_or_else(|_| panic!("bad hex `{hex}`"));
    (((n >> 16) & 0xff) as u8, ((n >> 8) & 0xff) as u8, (n & 0xff) as u8)
}

/// WCAG relative luminance of an sRGB colour.
fn luminance((r, g, b): (u8, u8, u8)) -> f64 {
    fn lin(c: u8) -> f64 {
        let s = c as f64 / 255.0;
        if s <= 0.03928 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

/// WCAG contrast ratio between two colours (1.0–21.0).
fn contrast(a: (u8, u8, u8), b: (u8, u8, u8)) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// Build the effective token map for a theme: the base `:root` map, with the
/// theme's overrides applied (dark = base as-is; light = base + light block).
fn theme_map(base: &BTreeMap<String, String>, overrides: Option<&BTreeMap<String, String>>) -> BTreeMap<String, String> {
    let mut m = base.clone();
    if let Some(o) = overrides {
        for (k, v) in o {
            m.insert(k.clone(), v.clone());
        }
    }
    m
}

/// Resolve a semantic token to its RGB in a given theme map.
fn token_rgb(name: &str, map: &BTreeMap<String, String>, base: &BTreeMap<String, String>) -> (u8, u8, u8) {
    let raw = map.get(name).or_else(|| base.get(name)).unwrap_or_else(|| panic!("token `{name}` missing"));
    hex_rgb(&resolve(raw, map, base))
}

// ── 1. The authoritative primitive palette is present and exact ──────────────

#[test]
fn primitive_palette_carries_the_authoritative_sourcesense_values() {
    let css = strip_comments(&read("src/styles/tokens.css"));
    let base = declarations(&block_after(&css, ":root"));
    for (name, hex) in [
        ("--so-red", "#da291c"),
        ("--so-orange", "#e35205"),
        ("--so-merlin", "#3d3935"),
        ("--so-merlin-50", "#f4f4f2"),
        ("--so-muted", "#716b5d"),
        ("--so-green", "#16a34a"),
    ] {
        assert_eq!(
            base.get(name).map(String::as_str),
            Some(hex),
            "authoritative SOURCESENSE primitive {name} must be {hex} (frontend-design §1.2)",
        );
    }
}

// ── 2. Dark-first theming via data-theme + prefers-color-scheme ──────────────

#[test]
fn dark_is_the_canonical_default_on_root() {
    let css = strip_comments(&read("src/styles/tokens.css"));
    let base_body = block_after(&css, ":root");
    let base = declarations(&base_body);
    // The :root default is dark: color-scheme dark, and the page surface resolves
    // to the dark neutral, not the brand off-white.
    assert!(base_body.contains("color-scheme: dark"), ":root declares color-scheme: dark");
    let surface0 = token_rgb("--surface-0", &base, &base);
    assert_eq!(surface0, hex_rgb("#0f1216"), "the default page surface is the dark neutral");
}

#[test]
fn light_is_a_first_class_opt_in_via_data_theme() {
    let css = strip_comments(&read("src/styles/tokens.css"));
    let base = declarations(&block_after(&css, ":root"));
    let light = declarations(&block_after(&css, ":root[data-theme=\"light\"]"));
    let light_map = theme_map(&base, Some(&light));
    // The explicit light theme remaps the page surface back to the brand off-white
    // and primary text to merlin — proving a theme is a token remap.
    assert_eq!(token_rgb("--surface-0", &light_map, &base), hex_rgb("#f4f4f2"));
    assert_eq!(token_rgb("--text-1", &light_map, &base), hex_rgb("#3d3935"));
    // An explicit dark theme block also exists (a dark choice survives a light OS).
    let _dark = block_after(&css, ":root[data-theme=\"dark\"]");
}

#[test]
fn first_visit_honors_prefers_color_scheme_without_an_explicit_choice() {
    let css = strip_comments(&read("src/styles/tokens.css"));
    // A light-OS first-visit user (no data-theme set yet) gets light via the media
    // query, gated on :not([data-theme]) so a persisted choice always wins.
    assert!(css.contains("@media (prefers-color-scheme: light)"), "first-visit media query present");
    let mq = &css[css.find("@media (prefers-color-scheme: light)").unwrap()..];
    assert!(
        mq.contains(":root:not([data-theme])"),
        "the first-visit flip is gated on :root:not([data-theme]) so an explicit choice wins",
    );
}

// ── 3. --so-red is signal-only in both themes (never a large background fill) ─

#[test]
fn so_red_is_signal_only_never_a_surface_fill() {
    let css = strip_comments(&read("src/styles/tokens.css"));
    let base = declarations(&block_after(&css, ":root"));
    let light = declarations(&block_after(&css, ":root[data-theme=\"light\"]"));
    let red = hex_rgb("#da291c");
    for (theme, map) in [
        ("dark", theme_map(&base, None)),
        ("light", theme_map(&base, Some(&light))),
    ] {
        // No page/card/raised surface is ever the signal red.
        for surface in ["--surface-0", "--surface-1", "--surface-2"] {
            assert_ne!(
                token_rgb(surface, &map, &base),
                red,
                "{surface} must never be the signal red in the {theme} theme (red is signal-only)",
            );
        }
        // The accent token IS the signal red in both themes (verdicts/active/focus).
        assert_eq!(
            token_rgb("--color-accent", &map, &base),
            red,
            "--color-accent stays the signal red in the {theme} theme",
        );
        assert_eq!(
            token_rgb("--focus-ring", &map, &base),
            red,
            "the focus ring is the signal red in the {theme} theme",
        );
    }
}

// ── 4. WCAG 2.1 AA contrast in BOTH themes ───────────────────────────────────

#[test]
fn text_and_signal_contrast_meets_wcag_aa_in_both_themes() {
    let css = strip_comments(&read("src/styles/tokens.css"));
    let base = declarations(&block_after(&css, ":root"));
    let light = declarations(&block_after(&css, ":root[data-theme=\"light\"]"));

    for (theme, map) in [
        ("dark", theme_map(&base, None)),
        ("light", theme_map(&base, Some(&light))),
    ] {
        let s0 = token_rgb("--surface-0", &map, &base);
        let s1 = token_rgb("--surface-1", &map, &base);
        let t1 = token_rgb("--text-1", &map, &base);
        let t2 = token_rgb("--text-2", &map, &base);
        let accent = token_rgb("--color-accent", &map, &base);
        let focus = token_rgb("--focus-ring", &map, &base);

        // Body text (normal): ≥ 4.5:1 on both the page and card surfaces.
        for (label, text) in [("text-1", t1), ("text-2", t2)] {
            for (sl, surf) in [("surface-0", s0), ("surface-1", s1)] {
                let c = contrast(text, surf);
                assert!(
                    c >= 4.5,
                    "{theme}: {label} on {sl} is {c:.2}:1, below the 4.5:1 AA body minimum",
                );
            }
        }
        // Signal red + focus ring are UI/graphic affordances: ≥ 3:1 on the page.
        assert!(
            contrast(accent, s0) >= 3.0,
            "{theme}: the signal red on the page surface is below 3:1 (UI minimum)",
        );
        assert!(
            contrast(focus, s0) >= 3.0,
            "{theme}: the focus ring on the page surface is below 3:1 (UI minimum)",
        );
    }
}

#[test]
fn badge_ink_meets_wcag_aa_on_the_signal_hues() {
    let css = strip_comments(&read("src/styles/tokens.css"));
    let base = declarations(&block_after(&css, ":root"));
    // Badge hues are theme-independent signals, so the legible ink is too.
    let red = token_rgb("--so-red", &base, &base);
    let warm = token_rgb("--so-orange", &base, &base);
    let green = token_rgb("--so-green", &base, &base);
    let ink_red = token_rgb("--ink-on-red", &base, &base);
    let ink_warm = token_rgb("--ink-on-warm", &base, &base);
    for (label, ink, bg) in [
        ("white-on-red", ink_red, red),
        ("ink-on-orange", ink_warm, warm),
        ("ink-on-green", ink_warm, green),
    ] {
        let c = contrast(ink, bg);
        assert!(c >= 4.5, "badge {label} contrast is {c:.2}:1, below the 4.5:1 AA minimum");
    }
}

// ── 5. :focus-visible ring + reduced motion (theme-independent a11y) ──────────

#[test]
fn base_layer_has_focus_visible_ring_and_reduced_motion() {
    let base_css = read("src/styles/base.css");
    // A 2px signal-red ring on keyboard focus only (frontend-design §1.2/§7).
    assert!(base_css.contains(":focus-visible"), "a :focus-visible rule exists");
    assert!(
        base_css.contains("outline: 2px solid var(--focus-ring)"),
        "the global focus ring is 2px solid var(--focus-ring)",
    );
    // Reduced motion collapses non-essential animation/transition.
    assert!(
        base_css.contains("@media (prefers-reduced-motion: reduce)"),
        "a prefers-reduced-motion rule disables non-essential motion",
    );
}

// ── 6. Components are token-driven (a theme is a remap, no component change) ──

#[test]
fn component_modules_use_tokens_not_raw_hex_colours() {
    // Every component stylesheet must reference semantic tokens (var(--…)), never a
    // raw hex literal — so switching the theme remaps token VALUES with no
    // component change (FR-UI-23, ADR-44). Scrims use rgba(0,0,0,…) intentionally
    // (a fixed black overlay, not a themed colour); those carry no `#`.
    let dir = ui_dir().join("src");
    let mut module_files = Vec::new();
    collect_module_css(&dir, &mut module_files);
    assert!(module_files.len() >= 10, "the component library has CSS Modules: {}", module_files.len());
    let hex = regex_hex();
    for path in module_files {
        let css = strip_comments(&std::fs::read_to_string(&path).unwrap());
        for line in css.lines() {
            assert!(
                !hex(line),
                "{} contains a raw hex colour (`{}`) — components must use design tokens only",
                path.display(),
                line.trim(),
            );
        }
    }
}

// ── 7. The no-flash theme bootstrap is CSP-clean and consistent ──────────────

#[test]
fn theme_bootstrap_is_an_external_classic_head_script() {
    let index = read("index.html");
    // The served shell references the bootstrap as an EXTERNAL classic script
    // (carries src → not an inline script the self-only CSP forbids), in <head>.
    assert!(
        index.contains("<script src=\"/theme-init.js\"></script>"),
        "index.html loads /theme-init.js as an external classic head script",
    );
    let head_close = index.find("</head>").expect("index.html has a </head>");
    let script_at = index.find("theme-init.js").expect("theme-init referenced");
    assert!(script_at < head_close, "the theme bootstrap is inside <head> (runs before paint)");

    let js = read("public/theme-init.js");
    // It applies a persisted choice and is self-contained: no external origin, no eval.
    assert!(js.contains("data-theme"), "the bootstrap sets the data-theme attribute");
    assert!(js.contains("logos-theme"), "it reads the persisted choice key (mirrors theme.ts)");
    assert!(!js.contains("http://") && !js.contains("https://"), "names no external origin");
    assert!(!js.contains("eval("), "uses no eval (CSP)");
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn collect_module_css(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_module_css(&path, out);
        } else if path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with(".module.css")) {
            out.push(path);
        }
    }
}

/// A tiny `#rrggbb`/`#rgb` hex-colour detector (no regex crate dependency): true
/// when a line contains a `#` followed by exactly 3 or 6 hex digits at a boundary.
fn regex_hex() -> impl Fn(&str) -> bool {
    |line: &str| {
        let bytes = line.as_bytes();
        for (i, &b) in bytes.iter().enumerate() {
            if b != b'#' {
                continue;
            }
            let run = bytes[i + 1..]
                .iter()
                .take_while(|c| c.is_ascii_hexdigit())
                .count();
            // A trailing non-hex boundary (or EOL) distinguishes #fff/#ffffff from
            // longer alnum tokens (e.g. an id fragment).
            let boundary = bytes
                .get(i + 1 + run)
                .map(|c| !c.is_ascii_alphanumeric())
                .unwrap_or(true);
            if (run == 3 || run == 6) && boundary {
                return true;
            }
        }
        false
    }
}
