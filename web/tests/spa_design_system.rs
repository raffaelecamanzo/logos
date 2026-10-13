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

// ── The shell header's progressive disclosure (S-317, FR-UI-34, CR-089/CR-092) ──

/// The one rung of the disclosure ladder still standing, in CSS px: the complement
/// of the tier frontend-design §7 documents (tablet ≥768px), and the shell's
/// existing collapse point (`AppShell.module.css`).
///
/// Pinned as a constant because a rung is the one value in this story that can
/// drift in silence. Every rendered measurement in the S-317 record was taken by
/// hand, once, against this number; move it to 419px and the readout is back on
/// the row at the exact viewport CR-089 measured, with nothing to say so.
///
/// HF-4 removed the second rung this ladder used to carry: the `code
/// intelligence` brand subtitle (`.brandSub`, dropped at 767px) no longer exists,
/// so there is nothing left to drop at that width — see
/// `header_never_reintroduces_the_brand_subtitle` below.
const READOUT_RUNG_PX: f64 = 1023.0;

/// Every declaration that takes an element off the page. `display: none` is the one
/// the header uses; the other two are here because an element hidden by any of them
/// is just as lost to the reader, and a survivor guard that names one spelling
/// guards exactly one spelling.
const HIDING_DECLARATIONS: [(&str, &str); 3] =
    [("display", "none"), ("visibility", "hidden"), ("opacity", "0")];

/// Every `(selector, rung px)` the header stylesheet hides inside a width rung.
///
/// A rung whose width cannot be read as a length **panics** rather than being
/// skipped. The first draft of this walk skipped it, and an `em`-expressed rung — or
/// a reformatted `max-width :`, which is valid CSS — then carried whatever it hid
/// past every assertion below without a word.
/// Every stylesheet whose classes render into the header row.
///
/// The survivor guard below is a claim about what the reader can still SEE in
/// that row, and the row is not one file: S-317 gave the member selector its own
/// width concessions in its own module, in the same commit that wrote the guard.
/// A survivor's stylesheet has to be inside the walk, not exempted by a sentence
/// next to it.
///
/// S-425 moved the member SELECTOR off this row into the sidebar's Service-section
/// header, and `MemberSelector.module.css` left the walk with it. What stayed on the
/// row is the workspace-probe fault badge, whose rules moved to
/// `WorkspaceFault.module.css` in the same change — so the row still has two
/// stylesheets and the guard still covers every survivor on it. Dropping the
/// selector's module WITHOUT adding the badge's would have left this walk reading the
/// header's own file alone, which is the exact shape the doc comment above records as
/// having been wrong once.
const HEADER_ROW_STYLESHEETS: [&str; 2] = [
    "src/shell/Header.module.css",
    "src/shell/WorkspaceFault.module.css",
];

fn header_disclosure_ladder(stylesheet: &str) -> Vec<(String, f64)> {
    let css = strip_comments(&read(stylesheet));
    let mut out = Vec::new();
    for (at_rule, body) in top_level_rules(&css) {
        if !at_rule.starts_with("@media") || !at_rule.to_ascii_lowercase().contains("max-width") {
            continue;
        }
        let px = max_width_px(&at_rule).unwrap_or_else(|| {
            panic!(
                "`{at_rule}` names a max-width this suite cannot read as a length. \
                 Skipping it would hide whatever the rung hides from every assertion \
                 in this section — express the rung in px or rem, or teach \
                 `length_px` the unit",
            )
        });
        for (selector, decls) in top_level_rules(&body) {
            if hides_element(&decls) {
                out.push((selector, px));
            }
        }
    }
    out
}

/// The WIDEST rung at which `selector` is dropped — as the viewport narrows it is
/// the rung reached first, so it is the one that fixes the disclosure order.
fn header_drops_at(selector: &str) -> Option<f64> {
    HEADER_ROW_STYLESHEETS
        .iter()
        .flat_map(|sheet| header_disclosure_ladder(sheet))
        .filter(|(sel, _)| sel == selector)
        .map(|(_, px)| px)
        .reduce(f64::max)
}

/// The rung itself. Nothing else asserted about the ladder pins the absolute
/// width — the surrounding tests only say the readout is the one thing dropped —
/// so the rung could slide off the documented tier while every other test here
/// stayed green.
#[test]
fn header_disclosure_rung_is_the_documented_tier() {
    assert_eq!(
        header_drops_at(".status"),
        Some(READOUT_RUNG_PX),
        "the graph-state readout gives way at the tablet tier the AppShell sidebar \
         already collapses at (frontend-design §7)",
    );
}

/// Nothing in the header is hidden UNCONDITIONALLY. Every other assertion in this
/// section looks inside a rung, so a `display: none` at the top level — the readout
/// gone at 1600px as surely as at 420px — was invisible to all of them, and the SPA
/// suite cannot see it either (`css: false`, so jsdom evaluates no stylesheet).
#[test]
fn header_hides_nothing_unconditionally() {
    let css = strip_comments(&read("src/shell/Header.module.css"));
    for (selector, body) in top_level_rules(&css) {
        if selector.starts_with('@') {
            continue;
        }
        assert!(
            !hides_element(&body),
            "`{selector}` is hidden at EVERY width rather than inside a rung — the \
             readout is required PRESENT at desktop widths (FR-UI-34), and the \
             acceptance grid records it rendered at 1024px and 1600px",
        );
    }
}

/// The priority the design contract states (frontend-design §3, re-baselined by
/// CR-097, amended by HF-4): the readout gives way FIRST and is now the ONLY
/// thing that gives way, so the brand lockup, the workspace-probe fault badge and
/// the theme toggle survive to the narrowest supported viewport. (The member
/// selector was on this row until S-425 moved it into the sidebar's
/// Service-section header. The brand subtitle that used to give way second was
/// removed outright by HF-4, not relocated — there is no second rung to reach.)
/// `.status` is the readout's whole slot — every one of its four states renders
/// into it (`Header.test.tsx` binds that end).
#[test]
fn header_drops_the_readout_and_only_the_readout() {
    header_drops_at(".status")
        .expect("the graph-state readout (`.status`) must be dropped at some rung");
}

/// A dropped readout is ABSENT, not truncated. An ellipsis would present a clipped
/// `rev 3,9…` as a figure, which is the reporting failure NFR-CC-04 forbids.
///
/// The clip ban is scoped to the readout's own rules, deliberately. Banning it over
/// the whole stylesheet also blocks the **project root path** that frontend-design
/// §3 records as a known-unbuilt header element — a path is the canonical ellipsis
/// case, and the story that builds it would fail a test whose message talks about
/// the readout.
#[test]
fn header_drops_the_readout_absent_never_truncated() {
    let css = strip_comments(&read("src/shell/Header.module.css"));
    assert!(
        header_drops_at(".status").is_some(),
        "the readout must be removed, not merely narrowed",
    );
    for selector in [".status", ".readout"] {
        let body = rule_body(&css, selector);
        for needle in ["text-overflow", "ellipsis"] {
            assert!(
                !body.contains(needle),
                "`{selector}` declares `{needle}` — a clipped readout presents a \
                 truncation as a measurement (NFR-CC-04, NFR-RA-05)",
            );
        }
    }
}

/// …and nothing ELSE gives way. The brand lockup, the workspace-probe fault badge and
/// the theme toggle survive to the narrowest supported viewport (FR-UI-29,
/// NFR-RA-05): a workspace whose roster could not be read must say so at every width.
///
/// The ladder counts every mechanism in `HIDING_DECLARATIONS`, not `display` alone —
/// a `visibility: hidden` on the brand lockup loses the home link just as completely
/// and once slipped past this assertion.
///
/// It walks every stylesheet in `HEADER_ROW_STYLESHEETS`, not the header's own.
/// This guard previously read `Header.module.css` alone while its own message
/// named the member selector as the survivor that mattered — and that selector's
/// rules lived in a module of their own, which the same story had created width
/// concessions in. (S-425 moved the selector off this row; the survivor whose rules
/// sit outside the header's file is now the fault badge, in
/// `WorkspaceFault.module.css`. The lesson is unchanged, and so is the walk.)
/// Appending
///
/// ```css
/// @media (max-width: 420px) { .select { display: none } }
/// ```
///
/// to that module removed the selector at the narrowest supported viewport and
/// left this suite green at 25/25. A survivor guarded by a sentence is not
/// guarded.
#[test]
fn header_hides_nothing_but_the_readout() {
    let mut hidden: Vec<String> = HEADER_ROW_STYLESHEETS
        .iter()
        .flat_map(|sheet| header_disclosure_ladder(sheet))
        .map(|(sel, _)| sel)
        .collect();
    hidden.sort();
    hidden.dedup();
    assert_eq!(
        hidden,
        vec![".status".to_string()],
        "only the readout gives way, across every stylesheet that renders into the \
         header row; anything else here is a survivor the narrow viewport has lost \
         (FR-UI-29, FR-UI-34)",
    );
}

/// Whether `needle` occurs in `haystack` as a whole selector/class token, not
/// merely as a substring. `.brandSub` is a literal prefix of `.brandSubtle` or
/// `.brandSubtitleWrap`, so a plain `contains` would reject an unrelated future
/// class that happens to start the same way; this requires the character right
/// after the match to not continue a CSS identifier (alphanumeric, `_` or `-`).
fn contains_selector_token(haystack: &str, needle: &str) -> bool {
    let is_ident_continuation = |c: char| c.is_alphanumeric() || c == '_' || c == '-';
    let mut start = 0;
    while let Some(pos) = haystack[start..].find(needle) {
        let end = start + pos + needle.len();
        if haystack[end..].chars().next().is_none_or(|c| !is_ident_continuation(c)) {
            return true;
        }
        start = end;
    }
    false
}

/// HF-4 removed the `code intelligence` brand subtitle outright — the span in
/// `Header.tsx` and the `.brandSub` rule (and its 767px rung) in
/// `Header.module.css` alike — rather than relocating it, so nothing should ever
/// reintroduce either half. The media-query case is already covered by
/// `header_hides_nothing_but_the_readout` above — a `.brandSub` hiding rule
/// coming back inside `@media` would show up in its hidden-set assertion and
/// fail it there, so re-asserting it here would only duplicate that test. This
/// guard's own job is the case that ladder can't see: an unconditional
/// (non-`@media`) `.brandSub` rule, which `header_disclosure_ladder` only walks
/// INSIDE `@media` blocks and so would not see at all.
#[test]
fn header_never_reintroduces_the_brand_subtitle() {
    for sheet in HEADER_ROW_STYLESHEETS {
        let css = strip_comments(&read(sheet));
        assert!(
            !contains_selector_token(&css, ".brandSub"),
            "`{sheet}` still names `.brandSub` — HF-4 removed the brand subtitle \
             span from `Header.tsx`, so no rule may reference that class at all",
        );
    }
}

/// The theme toggle is a `Button`, so its module is the third way off the row —
/// and it is a SHARED component module, where a width rung would be an app-wide
/// change rather than a header disclosure. Asserted separately, and precisely:
/// the toggle cannot be dropped from there because nothing there is width-rung'd
/// at all. If that ever changes, this fails with its own message rather than
/// surfacing as a confusing failure in the header ladder above.
#[test]
fn the_theme_toggles_module_declares_no_width_rung() {
    let css = strip_comments(&read("src/components/Button.module.css"));
    let rungs: Vec<String> = top_level_rules(&css)
        .into_iter()
        .map(|(at_rule, _)| at_rule)
        .filter(|at_rule| {
            at_rule.starts_with("@media") && at_rule.to_ascii_lowercase().contains("width")
        })
        .collect();
    assert!(
        rungs.is_empty(),
        "the theme toggle survives to the narrowest supported viewport (FR-UI-29); \
         `Button.module.css` now declares a width rung, so check whether the toggle \
         can be dropped from it: {rungs:?}",
    );
}

/// The sidebar's scope labels — "Workspace" and "Service" — are what tell two views
/// registered under the same name apart (ADR-66), so they must be readable at every
/// supported width, 420px included ([FR-UI-35], [NFR-CC-04]). The markup half (that
/// they are text, in a heading, in the rendered DOM) is asserted in
/// `web/ui/src/shell/Sidebar.test.tsx`; this is the stylesheet half, which that
/// suite cannot see because it runs with `css: false`.
///
/// Asserted as "this module declares no width rung at all", the same shape as the
/// theme toggle's guard above and for the same reason: it is a fact about the file
/// rather than about one spelling of hiding, so a rung that hid the label with
/// `visibility`, `opacity`, `content-visibility` or a negative `text-indent` fails
/// here too. A rung added for some unrelated sidebar tweak fails this with its own
/// message, which is the moment to check whether the labels survive it.
#[test]
fn sidebar_scope_label_survives_every_breakpoint() {
    let css = strip_comments(&read("src/shell/Sidebar.module.css"));

    // The label rule exists and is the text treatment, not a decoration: if it is
    // gone, an empty rung list below would otherwise report success over nothing.
    let label = rule_body(&css, ".sectionLabel");
    assert!(
        !label.trim().is_empty(),
        "`.sectionLabel` is the scope label's only styling hook; this suite's rung \
         assertion is worthless if the rule has been renamed or removed",
    );
    for (prop, value) in HIDING_DECLARATIONS {
        assert!(
            !declarations_of(&label).iter().any(|(n, v)| n == prop && v == value),
            "`.sectionLabel` declares `{prop}: {value}` — the scope label is the only \
             thing distinguishing two same-named views (ADR-66), so it is never taken \
             off the page",
        );
    }

    let rungs: Vec<String> = top_level_rules(&css)
        .into_iter()
        .map(|(at_rule, _)| at_rule)
        .filter(|at_rule| {
            at_rule.starts_with("@media") && at_rule.to_ascii_lowercase().contains("width")
        })
        .collect();
    assert!(
        rungs.is_empty(),
        "the sidebar's scope labels survive to the narrowest supported viewport \
         (FR-UI-35, NFR-CC-04); `Sidebar.module.css` now declares a width rung, so \
         check whether the labels can be dropped from it: {rungs:?}",
    );
}

/// The member selector sits in the sidebar's Service-section header (S-425); it
/// shared the app header's row with the graph-state readout until then. The header
/// sits in a 232px column at desktop width (≥1024px; below that the sidebar takes
/// the full viewport), and a `<select>` sizes itself to its LONGEST option — on the
/// reference workspace a 42-character member name, measured at 467px, which
/// overflowed a 420px viewport by 242px even after the header had dropped both of
/// its own elements, and which overflows the narrower column by more. So the header
/// must be able to give width back, the label must yield by ellipsis rather than
/// overflow or vanish, and the control must stop yielding while it is still a
/// control. All three are asserted, because in the shared row each one alone was a
/// defect: no `min-width: 0` on the label was an overflow, no floor on the control
/// was one measured at 24px, and no ellipsis on the label was a clipped name.
///
/// Since CR-145 the header is a COLUMN — the label on its own row, the control at
/// full width beneath it (frontend-design §3) — because the shared row it replaced
/// gave width to a long member name and truncated the label to `Se…`. The label no
/// longer competes with the control for width, so its yield rungs are a BACKSTOP
/// rather than the row's width ordering: the ellipsis rungs engage only for a label
/// longer than the column, and the two `min-width: 0` rungs are inert in the column
/// and guard a return to a row. They are still declared, and this test asserts them
/// unchanged — so its body's own row-era wording ("yields BEFORE the control") is
/// kept byte-identical by the same rule. The stacking itself is
/// verified against rendered state at review, not here: this suite reads
/// declarations, and no declaration check can tell a column that fits from one
/// that does not.
///
/// It reads TWO stylesheets, because the header is two components: the header and
/// its label belong to the sidebar, the control to its own module. An earlier
/// version of this test read the selector's module alone, back when that module
/// owned a `.selector` wrapper and a `.label`; both were deleted in S-425 when the
/// section heading became the control's label, and a test that had kept reading them
/// would have failed on absence rather than on the property.
#[test]
fn the_service_section_header_gives_width_back_but_stays_a_control() {
    let sidebar = strip_comments(&read("src/shell/Sidebar.module.css"));
    let selector = strip_comments(&read("src/shell/MemberSelector.module.css"));
    let decl = |css: &str, sel: &str, prop: &str| -> Option<String> {
        declarations_of(&rule_body(css, sel))
            .into_iter()
            .find(|(name, _)| name == prop)
            .map(|(_, value)| value)
    };

    assert_eq!(
        decl(&sidebar, ".sectionHeader", "min-width").as_deref(),
        Some("0"),
        "`.sectionHeader` must lift its automatic minimum size, or the longest \
         member name pins the row at that width and the column overflows (FR-UI-34)",
    );

    // The label yields BEFORE the control — and by ellipsis, never by removal: it is
    // the `<select>`'s accessible name as well as the section's (ADR-66), so taking
    // it off the page trades a layout defect for an accessibility one. That it is
    // never width-rung'd away is asserted in `sidebar_scope_label_survives_every_breakpoint`.
    for (prop, value) in [("min-width", "0"), ("overflow", "hidden"), ("text-overflow", "ellipsis")]
    {
        assert_eq!(
            decl(&sidebar, ".sectionLabel", prop).as_deref(),
            Some(value),
            "`.sectionLabel` must yield BEFORE the control does; without `{prop}: \
             {value}` the label cannot shrink and puts the select straight onto its \
             floor",
        );
    }

    let floor = decl(&selector, ".select", "min-width")
        .unwrap_or_else(|| panic!("`.select` declares no `min-width` floor"));
    let floor_px = length_px(&floor).unwrap_or_else(|| {
        panic!(
            "`.select` min-width is `{floor}`, which is not a length — `auto` and \
             `min-content` are the intrinsic sizings `min-width: 0` on the row exists \
             to defeat, so they are not floors",
        )
    });
    assert!(
        floor_px > 0.0,
        "`.select` must keep a NON-ZERO `min-width` floor — a control squeezed to a \
         sliver is present but not reachable (FR-UI-29); found `{floor}`",
    );

    // And the cap must be the container, not a fixed length wider than it: 18rem
    // (288px) exceeded the 232px column outright, which is what the former header
    // row could afford and this one cannot.
    assert_eq!(
        decl(&selector, ".select", "max-width").as_deref(),
        Some("100%"),
        "`.select`'s cap must be its container — a fixed cap wider than the 232px \
         sidebar column overflows it at every viewport width",
    );

    // The deleted rules stay deleted: a `.selector` wrapper or a `.label` reappearing
    // here means the control has grown a second name beside the section heading, and
    // the assertions above would then be guarding the wrong element.
    let declared: Vec<String> =
        top_level_rules(&selector).into_iter().map(|(sel, _)| sel).collect();
    for gone in [".selector", ".label"] {
        assert!(
            !declared.iter().any(|sel| sel == gone),
            "`{gone}` is back in `MemberSelector.module.css`; S-425 made the section \
             heading the control's label, so a second label element is a duplicate \
             name on the row (FR-UI-35, NFR-CC-04). Declared: {declared:?}",
        );
    }
}

/// The Workspace section reads as ONE list (S-454, [FR-UI-35], [FR-UI-37]). Its two
/// CR-042 groups stay two `<ul>`s — the group says what a tab answers, and the
/// app-scoped Statistics tab shares its member-scoped twin's group on purpose — but
/// the hairline between them left that Statistics entry stranded between the
/// hairline and the Workspace/Service boundary, reading as belonging to neither
/// scope. So the inter-group hairline and gap are suppressed inside that section
/// only, by a section-scoped rule.
///
/// Four facts, because each alone is a half-applied fix that looks like a working
/// one: the scoped rule zeroes the border and both inter-group paddings; the
/// unscoped `.group` border and `.group + .group` gap are still declared (the
/// Service section's hairlines and gaps are unchanged); the section boundary still
/// declares its own; and the scoped rule, declared once, comes AFTER every rule it
/// ties with. `.appSection .group` is two classes, exactly
/// as `.group:last-child` and `.group + .group` are, so source order decides — placed
/// above `.group + .group`, the second Workspace group would keep its top gap.
///
/// The markup half — that the Workspace region still renders two lists and the
/// Service region three, and that the Workspace section and only it carries
/// `appSection` — is asserted in `web/ui/src/shell/Sidebar.test.tsx`; this is the
/// stylesheet half, which that suite cannot see because it runs with `css: false`.
#[test]
fn the_workspace_section_renders_its_groups_as_one_list() {
    const SCOPED: &str = ".appSection .group";
    let css = strip_comments(&read("src/shell/Sidebar.module.css"));
    // The LAST declaration of a property, because that is the one the cascade
    // applies: `padding-top: 0; padding-top: var(--space-3)` renders the gap.
    let decl = |sel: &str, prop: &str| -> Option<String> {
        declarations_of(&rule_body(&css, sel))
            .into_iter()
            .rfind(|(name, _)| name == prop)
            .map(|(_, value)| value)
    };
    // `none` zeroes a border but is INVALID for padding — the browser drops the
    // declaration and the unscoped padding applies — so it counts only for borders.
    let zero = |prop: &str, v: &str| {
        length_px(v) == Some(0.0) || (prop.starts_with("border") && v == "none")
    };

    // The scoped rule zeroes the hairline and the gap on both sides of it. Read as
    // longhands, so a shorthand in the same body is refused by name: appended after
    // the longhands it would win, and this read would never see it.
    let scoped = declarations_of(&rule_body(&css, SCOPED));
    for shorthand in ["padding", "border", "border-width", "border-style"] {
        assert!(
            !scoped.iter().any(|(name, _)| name == shorthand),
            "`{SCOPED}` declares the shorthand `{shorthand}`; this guard reads longhands, \
             so spell the zeroed edges as `border-bottom` / `padding-top` / `padding-bottom`",
        );
    }
    for prop in ["border-bottom", "padding-top", "padding-bottom"] {
        let value = decl(SCOPED, prop)
            .unwrap_or_else(|| panic!("`{SCOPED}` declares no `{prop}` (as a longhand)"));
        assert!(
            zero(prop, &value),
            "`{SCOPED}` must zero `{prop}` — the Workspace section renders as one list \
             (FR-UI-35); found `{value}`",
        );
    }

    // The unscoped rules still declare the hairline and the top gap the scoped one
    // suppresses: the Service section keeps both.
    for (sel, prop) in [(".group", "border-bottom"), (".group + .group", "padding-top")] {
        let value = decl(sel, prop).unwrap_or_else(|| panic!("`{sel}` declares no `{prop}`"));
        assert!(
            !zero(prop, &value),
            "`{sel}` must still declare a non-zero `{prop}` — the suppression is scoped \
             to the Workspace section, and the Service section's groups keep it; found \
             `{value}`",
        );
    }

    // The Workspace/Service boundary is its own rule and keeps its own hairline.
    let boundary = decl(".section + .section", "border-top")
        .unwrap_or_else(|| panic!("`.section + .section` declares no `border-top`"));
    assert!(
        !zero("border-top", &boundary),
        "the Workspace/Service boundary must keep its hairline; found `{boundary}`",
    );

    // Source order: the scoped rule wins the specificity tie only by coming later.
    let order: Vec<String> = top_level_rules(&css).into_iter().map(|(sel, _)| sel).collect();
    // Declared once: a second `{SCOPED}` later in the file would override this one,
    // and `rule_body` reads only the first.
    assert_eq!(
        order.iter().filter(|s| *s == SCOPED).count(),
        1,
        "`{SCOPED}` must be declared exactly once",
    );
    let at = |sel: &str| {
        order
            .iter()
            .position(|s| s == sel)
            .unwrap_or_else(|| panic!("rule `{sel}` not found in the stylesheet"))
    };
    for tied in [".group:last-child", ".group + .group"] {
        assert!(
            at(SCOPED) > at(tied),
            "`{SCOPED}` ties with `{tied}` on specificity (two classes each), so it must \
             come AFTER it in source order to win; it comes before",
        );
    }
}

// ── helpers ──────────────────────────────────────────────────────────────────

/// A CSS length in px (`rem` resolved at the 16px root), or `None` when the value is
/// not a length this suite understands — `auto`, `min-content`, a percentage. Unit
/// keywords are ASCII case-insensitive in CSS, so the comparison is too.
fn length_px(value: &str) -> Option<f64> {
    let v = value.trim().to_ascii_lowercase();
    let digits: String = v.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    let n: f64 = digits.parse().ok()?;
    match v[digits.len()..].trim() {
        "px" => Some(n),
        "rem" => Some(n * 16.0),
        // A bare `0` is a valid CSS length and needs no unit.
        "" if n == 0.0 => Some(0.0),
        _ => None,
    }
}

/// The `max-width` a media query names, in CSS px, or `None` when it names none.
/// Lowercased and whitespace-normalised first, so `max-width : 1023PX` — valid CSS,
/// and what a reformat can produce — is read rather than silently missed.
fn max_width_px(query: &str) -> Option<f64> {
    let q = query.to_ascii_lowercase().split_whitespace().collect::<Vec<_>>().join(" ");
    let at = q.find("max-width")? + "max-width".len();
    let rest = q[at..].trim_start().strip_prefix(':')?;
    length_px(rest.split(')').next()?)
}

/// The `property: value` declarations of a rule body, property names lowercased and
/// values whitespace-collapsed. Split per declaration rather than scanned as text,
/// so a custom property (`--display: none`) is never mistaken for the property it is
/// named after, and a declaration broken across lines still reads.
fn declarations_of(body: &str) -> Vec<(String, String)> {
    body.split(';')
        .filter_map(|decl| decl.split_once(':'))
        .map(|(name, value)| {
            (
                name.split_whitespace().collect::<String>().to_ascii_lowercase(),
                value.split_whitespace().collect::<Vec<_>>().join(" ").to_ascii_lowercase(),
            )
        })
        .collect()
}

/// Whether a rule body takes its element off the page, by any mechanism in
/// `HIDING_DECLARATIONS`.
fn hides_element(body: &str) -> bool {
    let declared = declarations_of(body);
    HIDING_DECLARATIONS
        .iter()
        .any(|(prop, value)| declared.iter().any(|(n, v)| n == prop && v == value))
}

/// Every top-level `selector { body }` pair in a stylesheet, selectors normalised
/// to single-spaced form (`".empty,\n.user"` → `".empty, .user"`). Comments must
/// already be stripped. At-rule bodies (`@media`, `@keyframes`) are returned under
/// the at-rule's own "selector" and are not descended into BY THIS CALL; a caller
/// that wants the rules inside a rung re-invokes it on the at-rule body, which is
/// what the header-disclosure section above does.
fn top_level_rules(css: &str) -> Vec<(String, String)> {
    let bytes = css.as_bytes();
    let mut out = Vec::new();
    let (mut i, mut sel_start) = (0usize, 0usize);
    while i < bytes.len() {
        if bytes[i] != b'{' {
            i += 1;
            continue;
        }
        let selector = css[sel_start..i].split_whitespace().collect::<Vec<_>>().join(" ");
        let body_start = i + 1;
        let mut depth = 0i32;
        let mut j = i;
        while j < bytes.len() {
            match bytes[j] {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            j += 1;
        }
        assert!(
            j < bytes.len(),
            "unterminated block for selector `{selector}` — a malformed stylesheet must fail \
             loudly here, not be absorbed into one giant trailing rule (cf. `block_after`)",
        );
        out.push((selector, css[body_start..j].to_string()));
        i = j + 1;
        sel_start = i;
    }
    out
}

/// The declaration body of the rule whose FULL selector list is exactly `selector`.
/// Exact-matching (not substring) is what keeps `.user` off `.userBubble` and the
/// standalone `.assistant` rule off the grouped `.empty, .user, .assistant` one.
fn rule_body(css: &str, selector: &str) -> String {
    top_level_rules(css)
        .into_iter()
        .find(|(sel, _)| sel == selector)
        .map(|(_, body)| body)
        .unwrap_or_else(|| panic!("rule `{selector}` not found in the stylesheet"))
}

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

// ── 14. A view never names a CSS-Module class its stylesheet does not define ───
//
// Added by S-429, which made `StatisticsView.module.css` serve TWO views. The
// hazard is specific to this toolchain and invisible to the SPA suite: `css: false`
// in `vitest.config.ts` makes every CSS Module resolve to `{}`, so `styles.gone` is
// `undefined`, React renders no class at all, and all 800-plus specs stay green. A
// rule deleted from a shared stylesheet as "only its original view uses this" would
// therefore break the other consumer in total silence.
//
// It is checked HERE, not in Vitest, because for a `*.module.css` Vite's CSS-modules
// plugin wins over the `?raw` query — the glob hands back the empty proxy rather than
// the stylesheet text. This file reads stylesheets off disk, which is the same reason
// it already owns every other stylesheet contract in the project.
//
// Both scanners below were written by hand (no regex crate here) and both were WRONG
// in their first form — a review agent broke each one with a mutation the guard
// passed. The two defects are named at their fixes, because a hand-rolled scanner
// that admits a non-definition is worse than no guard: it reports safety it does not
// provide.

/// The stylesheet each `import <binding> from "….module.css"` binds, as
/// `(binding, relative path)`.
///
/// Matched on the SPECIFIER, not on the binding name. The first version of this
/// scanner looked for the literal `import styles from "` and so skipped, in silence,
/// any file that binds its stylesheet to another name — and skipped every import
/// after the first, because it used `split_once`.
fn module_css_imports(source: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for line in source.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("import ") else { continue };
        let Some((binding, after)) = rest.split_once(" from ") else { continue };
        let binding = binding.trim();
        if binding.is_empty() || !binding.chars().all(|c| c.is_alphanumeric() || c == '_') {
            continue; // a named/namespace import, never a CSS-Module default
        }
        let spec = after.trim().trim_end_matches(';').trim_matches('"').trim_matches('\'');
        if spec.ends_with(".module.css") {
            found.push((binding.to_string(), spec.to_string()));
        }
    }
    found
}

/// Every class key `binding` is used with in `source` — both `binding.key` and
/// `binding["key"]`.
///
/// The bracket form was missed entirely by the first version. It type-checks against
/// the CSS-Module declaration and is what any codemod or dynamic-key refactor emits,
/// so a view could name a nonexistent class through it and this guard would pass —
/// proven by mutation before this arm existed.
fn module_style_keys(source: &str, binding: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let bytes = source.as_bytes();
    let dotted = format!("{binding}.");
    let bracketed = format!("{binding}[");
    let mut i = 0;
    while i < bytes.len() {
        // These sources are UTF-8 with plenty of em dashes in their comments, so a
        // byte cursor must not be used to slice: `source[i..]` panics mid-character.
        // (The scan stays byte-driven because every token it looks for is ASCII.)
        if !source.is_char_boundary(i) {
            i += 1;
            continue;
        }
        // The byte before must not be able to continue an identifier, so a longer
        // name ending in the binding (`myStyles.foo`) is not a match.
        let boundary_ok = i == 0 || {
            let p = bytes[i - 1];
            !(p.is_ascii_alphanumeric() || p == b'_' || p == b'$' || p == b'.')
        };
        if !boundary_ok {
            i += 1;
            continue;
        }
        if source[i..].starts_with(&dotted) {
            let mut j = i + dotted.len();
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                j += 1;
            }
            if j > i + dotted.len() {
                keys.push(source[i + dotted.len()..j].to_string());
            }
            i = j.max(i + 1);
            continue;
        }
        if source[i..].starts_with(&bracketed) {
            let after = &source[i + bracketed.len()..];
            // A literal key only. A computed key cannot be checked statically, and
            // saying so is better than pretending the walk covered it.
            let quote = after.chars().next().filter(|c| *c == '"' || *c == '\'' || *c == '`');
            if let Some(q) = quote {
                if let Some(end) = after[1..].find(q) {
                    let key = &after[1..1 + end];
                    if !key.is_empty() && key.chars().all(|c| c.is_alphanumeric() || c == '_') {
                        keys.push(key.to_string());
                    }
                }
            }
            i += bracketed.len();
            continue;
        }
        i += 1;
    }
    keys
}

/// The SELECTOR PRELUDES of `css` — the text that actually creates rules: everything
/// between a `}` (or the start of the file) and the next `{`, with at-rule preludes,
/// functional-pseudo argument lists (`:not(…)`, `:is(…)`, `:where(…)`, `:has(…)`)
/// and quoted strings removed.
///
/// Scanning the whole stylesheet was the second defect. `(`, `)` and `"` are all
/// identifier boundaries, so `.capNoteZ:not(.capNote) { … }` and
/// `content: ".capNote"` both counted `.capNote` as DEFINED — while no rule defines
/// it and every view using it renders no class. That is the exact failure this guard
/// exists to catch, so it read green on its own subject.
fn selector_preludes(css: &str) -> String {
    let mut out = String::new();
    let mut segment = String::new();
    let mut quote: Option<char> = None;
    let mut paren_depth = 0usize;
    for c in css.chars() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            continue; // drop the whole string body
        }
        match c {
            '"' | '\'' => quote = Some(c),
            '(' => paren_depth += 1,
            ')' => paren_depth = paren_depth.saturating_sub(1),
            _ if paren_depth > 0 => {}
            '{' => {
                let trimmed = segment.trim();
                if !trimmed.starts_with('@') {
                    out.push_str(trimmed);
                    out.push('\n');
                }
                segment.clear();
            }
            '}' => segment.clear(),
            _ => segment.push(c),
        }
    }
    out
}

/// Does `preludes` define `.key` as a class selector?
fn defines_class(preludes: &str, key: &str) -> bool {
    let pat = format!(".{key}");
    let mut rest = preludes;
    while let Some(at) = rest.find(&pat) {
        let after = &rest[at + pat.len()..];
        let next_ok = after
            .chars()
            .next()
            .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'));
        let before_ok = rest[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'));
        if next_ok && before_ok {
            return true;
        }
        rest = &rest[at + 1..];
    }
    false
}

fn collect_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_sources(&path, out);
        } else if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| (n.ends_with(".ts") || n.ends_with(".tsx")) && !n.contains(".test."))
        {
            out.push(path);
        }
    }
}

#[test]
fn every_module_style_key_a_view_uses_is_defined_in_the_stylesheet_it_imports() {
    // The ONE pre-existing violation this guard found when it was written, named
    // rather than excluded by narrowing the walk. `Tabs.tsx` sets `styles.tabs` and
    // `Tabs.module.css` defines `.tablist` / `.tab` / `.active` / `.panel` but no
    // `.tabs`, so that wrapper has silently carried no class. It is a real defect and
    // it is OUTSIDE S-429's scope — that component is in no diff here, and guessing
    // at the intended rule would be a change nobody asked for.
    const ALLOWED_PREEXISTING: [(&str, &str); 1] = [("components/Tabs.tsx", "tabs")];

    let src = ui_dir().join("src");
    let mut sources = Vec::new();
    collect_sources(&src, &mut sources);
    // The walk's own denominator: a partial walk would make every assertion below
    // vacuously true.
    assert!(
        sources.len() > 60,
        "the source walk found only {} modules — it is not walking the tree",
        sources.len(),
    );

    let mut pairs = 0usize;
    let mut checked = 0usize;
    let mut missing: Vec<String> = Vec::new();
    let mut hit_exemptions: std::collections::BTreeSet<(String, String)> =
        std::collections::BTreeSet::new();
    for path in &sources {
        let source = std::fs::read_to_string(path).unwrap();
        let rel_src = path.strip_prefix(&src).unwrap().display().to_string();
        for (binding, spec) in module_css_imports(&source) {
            let stylesheet = path.parent().unwrap().join(&spec);
            let Ok(css_raw) = std::fs::read_to_string(&stylesheet) else {
                panic!("{rel_src} imports {spec}, which does not exist");
            };
            let preludes = selector_preludes(&strip_comments(&css_raw));
            pairs += 1;
            for key in module_style_keys(&source, &binding) {
                checked += 1;
                if defines_class(&preludes, &key) {
                    continue;
                }
                let pair = (rel_src.clone(), key.clone());
                if ALLOWED_PREEXISTING.contains(&(pair.0.as_str(), pair.1.as_str())) {
                    hit_exemptions.insert(pair);
                    continue;
                }
                missing.push(format!("{rel_src} uses {binding}.{key}, undefined in {spec}"));
            }
        }
    }

    assert!(pairs >= 10, "found only {pairs} view/stylesheet pairs — the import scan is broken");
    assert!(checked >= 50, "checked only {checked} class keys — the key scan is broken");
    assert!(
        missing.is_empty(),
        "a view names {} CSS-Module class(es) its stylesheet does not define. \
         Under `css: false` this renders as NO class and no Vitest spec can see it:\n  {}",
        missing.len(),
        missing.join("\n  "),
    );

    // A stale exemption is a lie about the codebase, so it fails too. Compared as a
    // SET of pairs, not as a count of occurrences: the first version counted
    // occurrences against the list's length, so a second use of an allow-listed key
    // made the counts disagree and the diagnostic underflowed a `usize` — the test
    // still failed, but with "attempt to subtract with overflow" instead of its
    // message. Found by mutation.
    let declared: std::collections::BTreeSet<(String, String)> = ALLOWED_PREEXISTING
        .iter()
        .map(|(f, k)| ((*f).to_string(), (*k).to_string()))
        .collect();
    let stale: Vec<_> = declared.difference(&hit_exemptions).collect();
    assert!(
        stale.is_empty(),
        "{} allow-listed pre-existing violation(s) no longer occur — delete the stale \
         entr{} from ALLOWED_PREEXISTING: {stale:?}",
        stale.len(),
        if stale.len() == 1 { "y" } else { "ies" },
    );
}

/// Most contrast guards in this file measure two theme blocks — `:root` (the dark
/// default) and `:root[data-theme="light"]` — while `tokens.css` declares four: the
/// explicit `[data-theme="dark"]` and the first-visit `@media (prefers-color-scheme:
/// light) :root:not([data-theme])` too. That is sound only while the other two are
/// copies: the first-visit block IS the light block, and the explicit dark block
/// overrides exactly the light block's tokens with `:root`'s own values. Pinned here,
/// once, so a value that drifts in one block cannot pass every two-block measurement.
#[test]
fn the_four_theme_blocks_agree_so_two_block_measurements_cover_all_four() {
    let tokens = strip_comments(&read("src/styles/tokens.css"));
    let base = declarations(&block_after(&tokens, ":root"));
    let dark = declarations(&block_after(&tokens, ":root[data-theme=\"dark\"]"));
    let light = declarations(&block_after(&tokens, ":root[data-theme=\"light\"]"));
    let first_visit = declarations(&block_after(&tokens, ":root:not([data-theme])"));
    assert!(!light.is_empty(), "the light block declares its overrides");
    assert_eq!(first_visit, light, "the first-visit light block is the explicit light block, token for token");
    assert_eq!(
        dark.keys().collect::<Vec<_>>(),
        light.keys().collect::<Vec<_>>(),
        "the explicit dark block overrides exactly the tokens the light block does",
    );
    for (name, value) in &dark {
        assert_eq!(base.get(name), Some(value), "`{name}` in the explicit dark block equals `:root`'s dark default");
    }
}
