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

// ── 8. The chat view: flat turns inside the view's one card (S-300, S-308) ───
//
// [FR-UI-31] flattened the turn to the base assistant-ui grammar; [FR-UI-33]
// (S-308) put the transcript inside the application's own two-pane card grammar.
// The invariants below are declared CSS, and the SPA's Vitest run disables CSS
// processing (`css: false` — a CSS-module import is then a proxy that names ANY
// key, so class PLACEMENT reaches the jsdom DOM but a rule's DEFINITION never
// does), which makes the authored stylesheet the only place the contract can be
// checked without a headless browser — the same reasoning that puts the
// token/contrast checks above in this file.

#[test]
fn chat_assistant_turn_carries_no_card_chrome() {
    let css = strip_comments(&read("src/views/chat/Chat.module.css"));
    let assistant = rule_body(&css, ".assistant");
    for banned in ["box-shadow", "border-top", "background", "--card-accent"] {
        assert!(
            !assistant.contains(banned),
            "the assistant turn must not re-introduce `{banned}`: it is a flat, \
             left-aligned block in the assistant-ui column grammar, not a card \
             (FR-UI-31)",
        );
    }
}

/// The chat Mermaid viewer's zoom ladder is CSS, not an inline transform (S-302,
/// [FR-UI-32], NFR-SE-06): the served `default-src 'self'` policy has no `style-src`
/// escape hatch, so a `style="transform: scale(…)"` attribute would be blocked
/// exactly as Mermaid's own injected `<style>` is.
///
/// This lives here rather than in Vitest because the SPA tests run with `css: false`,
/// where the CSS-module proxy fabricates ANY key it is asked for — so a component
/// asking for a `.zoomNN` class that does not exist is unfalsifiable there. Reading
/// the stylesheet is the only way to prove each rung actually carries a scale.
#[test]
fn chat_mermaid_zoom_ladder_is_declared_as_css_classes() {
    let css = strip_comments(&read("src/views/chat/Chat.module.css"));
    // Every rung the component can select must exist and scale.
    for step in [50, 67, 80, 100, 125, 150, 200, 250, 300] {
        let body = rule_body(&css, &format!(".zoom{step}"));
        assert!(
            body.contains("transform:") && body.contains("scale("),
            ".zoom{step} must carry a `transform: scale(...)` — the zoom ladder is \
             CSS-only because an inline style attribute is CSP-blocked (NFR-SE-06)",
        );
    }
    // Distinct rungs must scale DIFFERENTLY, or zoom ships dead while every test
    // that only reads the percent label stays green.
    let scales: std::collections::BTreeSet<String> = [50, 67, 80, 100, 125, 150, 200, 250, 300]
        .iter()
        .map(|s| rule_body(&css, &format!(".zoom{s}")).replace(char::is_whitespace, ""))
        .collect();
    assert_eq!(scales.len(), 9, "each zoom rung must declare its own distinct scale");
}

/// The chat Mermaid fallback must carry the label-centring rule, not just colours
/// (S-302, CR-034/S-196). Under the self-only CSP Mermaid's injected `<style>` is
/// stripped, so `getBBox()` collapses during measurement and the centring translate
/// is never applied — labels stay left-anchored and overflow their node boxes. The
/// wiki reader learned this the hard way; the chat viewer runs the same bundle under
/// the same policy, so it needs the same rule.
#[test]
fn chat_mermaid_fallback_centers_node_labels_like_the_wiki() {
    let chat = strip_comments(&read("src/views/chat/Chat.module.css"));
    // Exact-match the RULE, the way the zoom-ladder guard above does: two
    // independent `contains` checks over the whole file would also pass with the
    // centring on some unrelated selector and the node-label selectors carrying
    // something else — which is the very drift this guard exists to catch.
    let body = rule_body(
        &chat,
        ".mermaidScale :global(.mermaid .node text), .mermaidScale :global(.mermaid .node tspan)",
    );
    assert!(
        body.contains("text-anchor: middle"),
        "the chat Mermaid fallback must set `text-anchor: middle` on the node-label \
         selectors themselves (mirrors WikiView.module.css) — without it, CSP-stripped \
         measurement leaves labels overflowing their boxes",
    );
}

/// The chat answer table's container chrome must be JOINED to the fenced code
/// block's, not copied (S-311, [FR-UI-31]): `.tableWrapper` is the THIRD selector
/// on the existing `.codeBlock, .mermaidBlock` rule, so all three share border,
/// radius and background from ONE declaration. Matching the exact joined selector
/// string (not merely each property individually) is what catches the admission
/// mutation this guard is named for: a copied, non-joined `.tableWrapper` rule
/// with identical property VALUES reads the same to every per-property assertion
/// below, but fails this lookup outright because the selector it copied from no
/// longer names all three container kinds in one rule.
#[test]
fn chat_answer_table_shares_the_code_block_chrome_and_carries_the_house_table_voice() {
    let css = strip_comments(&read("src/views/chat/Chat.module.css"));

    // (1) One declaration, not a copy: the exact three-selector join, in the
    // documented order (codeBlock, mermaidBlock, THEN tableWrapper — FR-UI-31).
    let container = rule_body(&css, ".codeBlock, .mermaidBlock, .tableWrapper");
    for (prop, value) in [
        ("border-radius", "var(--radius-2)"),
        ("overflow", "hidden"),
        ("background", "var(--surface-2)"),
    ] {
        assert_eq!(
            declared(&container, prop).as_deref(),
            Some(value),
            "`.tableWrapper`'s shared container rule declares `{prop}: {value}`",
        );
    }

    // (2) The wrapper's OWN rule: `min-width: 0` is what lets `overflow-x` actually
    // engage as a flex child of `.markdown` (computed-state behaviour, not just the
    // declaration — verified live at review); `overflow-x: auto` is the scroll axis
    // itself. Both must survive independently: this is the guard the AC names for
    // "removing the wrapper's overflow-x".
    let wrapper = rule_body(&css, ".tableWrapper");
    assert_eq!(
        declared(&wrapper, "min-width").as_deref(),
        Some("0"),
        "`.tableWrapper` declares `min-width: 0` so `overflow-x` can engage",
    );
    assert_eq!(
        declared(&wrapper, "overflow-x").as_deref(),
        Some("auto"),
        "`.tableWrapper` declares `overflow-x: auto` — the table's own scroll axis",
    );

    // (2b) `overflow-x` only engages if the table can be wider than the wrapper. An
    // inherited `overflow-wrap: anywhere` or `word-break: break-all` (the answer body's
    // `.final` sets the first) counts in min-content sizing, so it lets every column
    // shrink to one character. The table then never overflows and its cells break
    // mid-word in the answer body, while the Activity fold, which has no such
    // ancestor, scrolls the same table. So the table resets every such property that
    // any chat rule declares. The list comes from the stylesheet, so a wrap rule
    // added later is caught here too.
    let table = rule_body(&css, ".markdown table");
    for (selector, body) in all_style_rules(&css) {
        for (prop, value) in declarations_of(&body) {
            let shrinks_min_content = (prop == "overflow-wrap" && value == "anywhere")
                || (prop == "word-break" && matches!(value.as_str(), "break-all" | "break-word"));
            if shrinks_min_content {
                assert_eq!(
                    declared(&table, &prop).as_deref(),
                    Some("normal"),
                    "`{selector}` declares `{prop}: {value}`. A table that inherits it shrinks \
                     its columns to one character, so the wrapper never scrolls — \
                     `.markdown table` must reset `{prop}: normal` (FR-UI-31)",
                );
            }
        }
    }

    // (3) The header: uppercase tracked `--text-xs` label in `--text-2`, over a
    // `--border-strong` UNDERLINE — never a fill. Named by the AC as "the header
    // underline"; its removal is the guard's second required mutation.
    let th = rule_body(&css, ".markdown th");
    for (prop, value) in [
        ("text-transform", "uppercase"),
        ("letter-spacing", "var(--tracking-label)"),
        ("font-size", "var(--text-xs)"),
        ("color", "var(--text-2)"),
        ("border-bottom", "1px solid var(--border-strong)"),
    ] {
        assert_eq!(
            declared(&th, prop).as_deref(),
            Some(value),
            "`.markdown th` declares `{prop}: {value}` (the house header voice)",
        );
    }
    assert!(
        declared(&th, "background").is_none(),
        "`.markdown th` paints NO header fill — the AC bans a header background \
         (unlike the interactive `DataTable`, which fills its sticky header)",
    );
    assert!(
        declared(&th, "border-top").is_none() && declared(&th, "border").is_none(),
        "`.markdown th` carries no header BAND (a border on every edge, or a top \
         rule) — only the bottom underline",
    );

    // (4) Row rules: a hairline `--border-subtle` rule under every row. Named by
    // the AC as "the row rules"; its removal is the guard's third required
    // mutation.
    let td = rule_body(&css, ".markdown td");
    assert_eq!(
        declared(&td, "border-bottom").as_deref(),
        Some("1px solid var(--border-subtle)"),
        "`.markdown td` declares the hairline row rule",
    );

    // (5) Row hover: `--tint-hover`, on body rows only (not the header row).
    assert_eq!(
        declared(&rule_body(&css, ".markdown tbody tr:hover"), "background").as_deref(),
        Some("var(--tint-hover)"),
        "`.markdown tbody tr:hover` declares the house row-hover tint",
    );

    // (6) `border-collapse: collapse`, and token-only padding on both cell kinds —
    // no literal length anywhere in the table's own box model.
    assert_eq!(
        declared(&rule_body(&css, ".markdown table"), "border-collapse").as_deref(),
        Some("collapse"),
    );
    for selector in [".markdown th", ".markdown td"] {
        let body = rule_body(&css, selector);
        let padding = declared(&body, "padding").expect("token-only padding");
        assert!(
            css_values(&padding).iter().all(|t| t.starts_with("var(--")),
            "`{selector}` padding must be built entirely from tokens: `{padding}`",
        );
    }

    // (7) No content-based column typing: unlike `DataTable.module.css` (`.num`
    // right-aligns a numeric column by its content), the answer table has no
    // selector that re-types a cell from what it holds.
    assert!(
        !css.contains(".num") && !css.contains(".mono"),
        "the chat stylesheet carries no content-based column-typing selector — a \
         GFM answer table is content, not a dataset (frontend-design §5)",
    );

    // (8) No per-table copy control: the table renderer wraps a bare `<table>`,
    // never a `CopyControl` — that affordance stays exclusive to fenced code. Sliced
    // to the next top-level `function ` rather than brace-matched: `TableRenderer`'s
    // own signature destructures its props with a `{ … }` type annotation, which
    // `block_after` (built for CSS, where a selector never contains a brace) would
    // close on before ever reaching the function body.
    let tsx = strip_tsx_comments(&read("src/views/chat/MarkdownAnswer.tsx"));
    let start = tsx.find("function TableRenderer").expect("`TableRenderer` is defined");
    let after = &tsx[start..];
    let end = after[1..].find("function ").map_or(after.len(), |i| i + 1);
    let table_renderer = &after[..end];
    assert!(
        !table_renderer.contains("CopyControl"),
        "the table renderer carries no per-table copy control (unlike `CodeBlock`)",
    );
}

/// The properties that cap an element's inline size, i.e. that set a reading
/// measure. Both the physical and the logical spelling, because a guard that
/// names one spelling guards exactly one spelling.
const MEASURE_PROPERTIES: [&str; 4] = ["max-width", "max-inline-size", "width", "inline-size"];

/// A declared value's top-level components: split on whitespace only at paren depth
/// 0, so `0 auto calc(1rem + 2px)` is three values, not five, with any trailing
/// `!important` dropped (it changes precedence, not the value).
fn css_values(value: &str) -> Vec<String> {
    let v = value.trim().trim_end_matches("!important").trim();
    let (mut out, mut cur, mut depth) = (Vec::new(), String::new(), 0i32);
    for c in v.chars() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {}
        }
        if c.is_whitespace() && depth == 0 {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Whether a rule body CENTRES A MEASURE: it caps the element's inline size AND
/// centres the capped box horizontally — or derives its inline padding from the
/// container's width, which centres a measure with no cap at all. The cap-plus-
/// centring pair — not either half alone — is what S-300's `--chat-measure` column
/// was (`max-width: var(--chat-measure)` + `margin-inline: auto`), and what left the
/// 172px void beside the rail. A cap without centring (`.userBubble`'s
/// `max-width: 80%`) is a bubble, not a measure; centring without a cap is a no-op;
/// `100%` / `none` / `auto` cap nothing.
///
/// The inline margins are RESOLVED, in declaration order, the way the cascade does
/// it: the `margin` shorthand in each of its 1–4 value forms (split at paren depth 0,
/// so a spaced `calc()` is one value), `margin-inline` in its one- and two-value
/// forms, and the physical and logical longhands — so `margin: 0 auto; margin-left: 0`
/// is correctly NOT centred. The self-alignment keywords (`justify-self` /
/// `align-self` / either half of `place-self: center`) centre a capped box in a grid
/// or flex parent without touching a margin. A box centred by its PARENT's alignment
/// is `centres_its_children`' case, and a measure split across several rules is
/// the caller's (`aggregated_rules`).
fn centres_a_measure(body: &str) -> bool {
    let value = |p: &str| declared(body, p).map(|v| css_values(&v));
    let capped = MEASURE_PROPERTIES
        .iter()
        .filter_map(|p| value(p))
        .any(|v| !matches!(v.first().map(String::as_str), Some("100%" | "none" | "auto") | None));
    let (mut left, mut right) = (String::new(), String::new());
    for (name, raw) in declarations_of(body) {
        let v = css_values(&raw);
        let pick = |i: usize| v.get(i).cloned().unwrap_or_default();
        match (name.as_str(), v.len()) {
            ("margin", 1) => (left, right) = (pick(0), pick(0)),
            ("margin", 2 | 3) => (left, right) = (pick(1), pick(1)),
            ("margin", 4) => (left, right) = (pick(3), pick(1)),
            ("margin-inline", 1) => (left, right) = (pick(0), pick(0)),
            ("margin-inline", 2) => (left, right) = (pick(0), pick(1)),
            ("margin-left" | "margin-inline-start", _) => left = pick(0),
            ("margin-right" | "margin-inline-end", _) => right = pick(0),
            _ => {}
        }
    }
    let self_centred = ["justify-self", "align-self", "place-self"]
        .iter()
        .filter_map(|p| value(p))
        .any(|v| v.iter().any(|t| t == "center"));
    let centred = (left == "auto" && right == "auto") || self_centred;
    let padding_from_width = declarations_of(body).iter().any(|(n, v)| {
        n.starts_with("padding") && !n.contains("block") && !n.ends_with("top") && !n.ends_with("bottom")
            && v.contains("100%")
    });
    (capped && centred) || padding_from_width
}

/// Whether a rule body makes its element a container that CENTRES ITS CHILDREN on
/// the inline axis — the other way to centre a capped column, and S-300's own shape
/// one step removed: a column flexbox with `align-items: center`, a row flexbox with
/// `justify-content: center`, or a grid with `justify-items` / `place-items` centred.
/// Row-flex `align-items: center` centres on the BLOCK axis (a toolbar's vertical
/// alignment) and is not this.
fn centres_its_children(body: &str) -> bool {
    let first = |p: &str| declared(body, p).map(|v| css_values(&v));
    let display = declared(body, "display").unwrap_or_default();
    let column = declared(body, "flex-direction").is_some_and(|d| d.starts_with("column"));
    let has = |p: &str, i: usize| first(p).is_some_and(|v| v.get(i).or(v.first()).is_some_and(|t| t == "center"));
    if display.ends_with("flex") {
        if column { has("align-items", 0) || has("place-items", 0) } else { has("justify-content", 0) }
    } else if display.ends_with("grid") {
        has("justify-items", 0) || has("place-items", 1)
    } else {
        false
    }
}

/// Every individual selector in a stylesheet with ALL of its declarations, from
/// every rule that names it — grouped lists split, `@media` rungs included — in
/// source order, so the last one still wins. A measure split across two rules (the
/// cap in one, `margin-inline: auto` in a grouped `.composer, .other` rule) is one
/// measure, and a per-rule scan would never see it.
fn aggregated_rules(css: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for (list, body) in all_style_rules(css) {
        let (mut depth, mut cur, mut sels) = (0i32, String::new(), Vec::new());
        for c in list.chars() {
            match c {
                '(' => depth += 1,
                ')' => depth -= 1,
                ',' if depth == 0 => {
                    sels.push(std::mem::take(&mut cur));
                    continue;
                }
                _ => {}
            }
            cur.push(c);
        }
        sels.push(cur);
        for sel in sels.iter().map(|s| s.trim().to_string()) {
            match out.iter_mut().find(|(s, _)| *s == sel) {
                Some((_, acc)) => {
                    acc.push(';');
                    acc.push_str(&body);
                }
                None => out.push((sel, body.clone())),
            }
        }
    }
    out
}

/// Every `(selector, body)` rule in a stylesheet, INCLUDING the rules nested in
/// `@media` / `@supports` blocks — a measure re-introduced inside a width rung is
/// just as much a measure. `@keyframes` frames are not style rules and are skipped.
fn all_style_rules(css: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (selector, body) in top_level_rules(css) {
        if selector.starts_with("@keyframes") {
            continue;
        }
        if selector.starts_with('@') {
            out.extend(all_style_rules(&body));
        } else {
            out.push((selector, body));
        }
    }
    out
}

/// A TSX source with its comments removed (`/* … */`, `{/* … */}` and `// …`), so a
/// structural scan never matches an element that is only NAMED in prose. A `//`
/// inside a string literal (a URL) is truncated too; harmless for an element scan.
fn strip_tsx_comments(src: &str) -> String {
    strip_comments(src)
        .lines()
        .map(|l| l.find("//").map_or(l, |i| &l[..i]))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Byte offsets of every opening `<Tag` element in a TSX source — `<Card` followed by
/// whitespace, `>` or `/`, so `<CardList` and a closing `</Card>` never count.
fn opening_tags(src: &str, tag: &str) -> Vec<usize> {
    let needle = format!("<{tag}");
    src.match_indices(&needle)
        .filter(|(i, _)| {
            src[i + needle.len()..].chars().next().is_some_and(|c| c.is_whitespace() || c == '>' || c == '/')
        })
        .map(|(i, _)| i)
        .collect()
}

/// The near misses the measure detector must tell apart. Every spelling of a
/// centred measure is ADMITTED and every one-step-away shape is not: a detector
/// proven only by deleting the real rule can never show what it wrongly admits,
/// or what spelling it misses.
#[test]
fn centred_measure_detector_admits_every_spelling_and_rejects_its_near_misses() {
    for centred in [
        "width: 100%; max-width: var(--chat-measure); margin-inline: auto",
        "max-width: 46rem; margin: 0 auto",
        "max-width: 46rem; margin: 0 auto var(--space-4)",
        "max-width: 46rem; margin: 0 auto 0 auto",
        "max-width: 46rem; margin: auto",
        "max-width: 46rem; margin-left: auto; margin-right: auto",
        "max-inline-size: 60ch; margin-inline-start: auto; margin-inline-end: auto",
        "width: 46rem; justify-self: center",
        "max-width: 46rem; align-self: center",
        "inline-size: 46rem; place-self: center",
        // Review (S-308): spellings the first detector missed.
        "max-width: 46rem; margin: calc(var(--space-4) + 2px) auto",
        "max-width: 46rem; margin: 0 auto calc(1rem + 2px)",
        "max-width: 46rem; margin-inline: auto !important",
        "max-width: 46rem; margin-inline: auto auto",
        "inline-size: 46rem; place-self: center center",
        "padding-inline: max(0px, calc((100% - 46rem) / 2))",
    ] {
        assert!(centres_a_measure(centred), "a centred measure must be detected: `{centred}`");
    }
    for not_centred in [
        // The user bubble: capped, but hugging the right edge — not centred.
        "max-width: 80%; padding: var(--space-2) var(--space-3)",
        // Full-width content centred on nothing.
        "width: 100%; margin-inline: auto",
        "max-width: none; margin: 0 auto",
        // The log's negative inline margin: not `auto`, and no cap.
        "margin-inline: calc(-1 * var(--space-4)); padding: var(--space-5) var(--space-4)",
        // Only ONE side auto pushes the box to an edge; it does not centre it.
        "max-width: 46rem; margin-left: auto",
        "max-width: 46rem; margin: 0 auto 0 0",
        // A custom property NAMED like a margin is not a margin.
        "max-width: 46rem; --margin-inline: auto",
        // The cascade: a later longhand un-centres an earlier shorthand.
        "max-width: 46rem; margin: 0 auto; margin-left: 0",
        "max-width: 46rem; margin-inline: auto 0",
        // Block-axis padding is not a measure.
        "padding-block: calc(100% - 2rem)",
    ] {
        assert!(!centres_a_measure(not_centred), "not a centred measure: `{not_centred}`");
    }

    // The parent's half: a container that centres its children on the inline axis.
    for centring in [
        "display: flex; flex-direction: column; align-items: center",
        "display: flex; flex-direction: column; place-items: center",
        "display: flex; justify-content: center",
        "display: inline-flex; justify-content: center",
        "display: grid; justify-items: center",
        "display: grid; place-items: start center",
        "display: grid; place-items: center",
    ] {
        assert!(centres_its_children(centring), "a centring container must be detected: `{centring}`");
    }
    for not_centring in [
        // A toolbar: row flex centring on the BLOCK axis only.
        "display: flex; align-items: center",
        "display: inline-flex; align-items: center; gap: var(--space-2)",
        // A column flex centring on the block axis.
        "display: flex; flex-direction: column; justify-content: center",
        "display: flex; flex-direction: column; align-items: stretch",
        "display: grid; place-items: center start",
        // Not a flex/grid container at all.
        "display: block; align-items: center",
    ] {
        assert!(!centres_its_children(not_centring), "not a centring container: `{not_centring}`");
    }

    // The caller's half: declarations are aggregated per selector across rules, so a
    // measure split between a cap and a grouped centring rule is still one measure.
    let split = "a { max-width: 46rem } .other, a { margin-inline: auto }";
    let a = aggregated_rules(split).into_iter().find(|(s, _)| s == "a").map(|(_, b)| b).unwrap();
    assert!(centres_a_measure(&a), "a cap and its centring in two rules are one measure");
}

/// The retired-grammar guard's replacement (S-308, [FR-UI-33], CR-092): the
/// conversation renders inside the view's ONE shared `Card` (since S-617 the
/// `chatConversation` `Widget`, which composes it), filling its track, and
/// no rule centres a body-content measure. It supersedes S-300's
/// `chat_roles_share_one_centered_readable_measure`, which locked the centred
/// `--chat-measure` column this story retires, and keeps every assertion of it that
/// is still true: neither role self-aligns, the user turn hugs the right edge, and
/// the bubble treatment lives on the inner `.userBubble`.
#[test]
fn chat_conversation_sits_in_one_card_with_no_centred_measure() {
    let css = strip_comments(&read("src/views/chat/Chat.module.css"));

    // (1) No selector anywhere in the chat stylesheets — its declarations gathered
    // from every rule and rung that names it — centres a measure, and none is a
    // container that centres its children (the parent-side spelling of the same
    // column). The retired measure token is gone rather than left dormant.
    for (file, selector, body) in chat_stylesheets()
        .into_iter()
        .flat_map(|(file, css)| aggregated_rules(&css).into_iter().map(move |(s, b)| (file.clone(), s, b)))
    {
        // A control box bounded to ≤ 40×40px (S-619's round in-pill buttons, the rail
        // toggle and trash) centres a GLYPH, never a reading measure: it has no room
        // for one. Its own bound is what exempts it, so it cannot grow into a column
        // and keep the exemption.
        let small_control = ["width", "height"]
            .iter()
            .all(|d| declared(&body, d).as_deref().and_then(length_px).is_some_and(|px| px <= 40.0));
        assert!(
            !centres_a_measure(&body),
            "`{selector}` centres a capped reading measure. The chat view fills its card \
             (FR-UI-33): a centred measure is what left the 172px void beside the rail",
        );
        // The welcome (S-621, FR-UI-46) is the centred empty state of a region with no
        // content, which FR-UI-40 permits: its greeting and its chip row centre their
        // children, and they are gone the moment a message exists. Exempt by NAME, so a
        // new rule cannot borrow the exemption — and a centred column anywhere else, the
        // transcript included, still fails.
        let empty_state = file == "Chat.module.css" && (selector == ".welcome" || selector == ".chips");
        assert!(
            small_control || empty_state || !centres_its_children(&body),
            "`{selector}` centres its children on the inline axis — a capped child inside it \
             is a centred reading measure by another route (FR-UI-33)",
        );
    }
    assert!(!css.contains("--chat-measure"), "the retired `--chat-measure` token is gone");

    // (2) The conversation is inside the view's ONE card. Since S-617 (CR-203) that
    // card is the `chatConversation` tool panel — a `Widget`, which composes the
    // shared `Card` — so the view file renders no raw `<Card>` and exactly one
    // `<Widget>`, and that one encloses the whole thread: the transcript viewport,
    // the messages and the composer. One, not "at least one": a second card is a
    // card per turn (or per notice), which is the CR-089 chrome the flat turn removed.
    let view = strip_tsx_comments(&read("src/views/chat/ChatView.tsx"));
    assert!(
        opening_tags(&view, "Card").is_empty(),
        "ChatView renders no raw `Card`: its one card is the conversation `Widget` (S-617)",
    );
    let cards = opening_tags(&view, "Widget");
    assert_eq!(
        cards.len(),
        1,
        "ChatView renders exactly one card — the conversation `Widget`, never a turn's \
         (FR-UI-33, FR-UI-31)",
    );
    let open = cards[0];
    let close = open + view[open..].find("</Widget>").expect("the view's `<Widget>` is closed");
    let inside = &view[open..close];
    for part in ["<ThreadPrimitive.Root", "<ThreadPrimitive.Viewport", "<ThreadPrimitive.Messages", "<Composer"] {
        assert!(
            inside.contains(part),
            "`{part}` renders INSIDE the view's card: the transcript and the composer live in \
             that one container (FR-UI-33)",
        );
    }
    assert!(
        !inside.contains("<ThreadList"),
        "the history rail is the other pane, not part of the conversation card",
    );

    // (3) The card fills its track: nothing between the grid cell and the card caps
    // or centres it, and the composer rides the card's full content box.
    for selector in [".main", ".threadRoot", ".composer"] {
        let body = rule_body(&css, selector);
        for prop in MEASURE_PROPERTIES {
            assert!(
                !declarations_of(&body).iter().any(|(n, v)| n == prop && v != "100%"),
                "`{selector}` declares `{prop}`: the conversation fills its card and the card its \
                 track (FR-UI-33)",
            );
        }
    }

    // (3b) No unused gutter inside the card (FR-UI-33): the turns, the user bubble and
    // the composer all sit on the card's content edge. The log's inline padding keeps
    // its text off the scrollbar, so an equal NEGATIVE inline margin must hand that
    // inset back to the card's own padding — without it the turns sit 16px inside
    // the composer. And nothing between the grid cell and the turns adds an inline
    // inset of its own (`.main` padding would be a void beside the rail).
    let log = rule_body(&css, ".log");
    let log_padding = css_values(&declared(&log, "padding").expect("`.log` declares its padding"));
    let inline_pad = match log_padding.as_slice() {
        [all] => all.clone(),
        [_, inline] | [_, inline, _] | [_, inline, _, _] => inline.clone(),
        _ => panic!("`.log` padding has 1–4 values"),
    };
    assert_eq!(
        declared(&log, "margin-inline"),
        Some(format!("calc(-1 * {inline_pad})")),
        "`.log` cancels its own inline padding ({inline_pad}) with an equal negative inline margin, \
         so the turns align on the card's content edge with the composer",
    );
    // The pill composer (S-619) is the one exception on the padding side: its inset is
    // the pill's own, INSIDE its border — the border itself still sits on the card's
    // content edge, so it is held to the margin half only.
    for (selector, props) in [(".main", "padding|margin"), (".threadRoot", "padding|margin"), (".composer", "margin")] {
        let body = rule_body(&css, selector);
        for (n, v) in declarations_of(&body) {
            let inline_inset = props.split('|').any(|p| n.starts_with(p))
                && !n.contains("block") && !n.ends_with("top") && !n.ends_with("bottom")
                && css_values(&v).iter().any(|t| t != "0");
            assert!(
                !inline_inset,
                "`{selector}` declares `{n}: {v}` — an inline inset between the card's edge and \
                 the turns is an unused gutter (FR-UI-33)",
            );
        }
    }

    // (4) Still true from S-300: neither role escapes the line with its own
    // alignment; the user bubble is right-aligned INSIDE the transcript.
    let user = rule_body(&css, ".user");
    assert!(!user.contains("align-self"), "the user turn aligns within the transcript, not against it");
    assert!(!rule_body(&css, ".assistant").contains("align-self"));
    assert!(user.contains("justify-content: flex-end"), "the user bubble hugs the transcript's right edge");
    assert!(
        rule_body(&css, ".userBubble").contains("background: var(--surface-2)"),
        "the bubble treatment lives on the inner .userBubble element",
    );

    // (5) S-619 ([FR-UI-45], CR-204 L2): answer prose IS capped — at `--measure-prose`,
    // and LEFT-anchored. The cap lives on `.final`, the answer's prose wrapper and the
    // element carrying the 16px chat size its `ch` is measured in; exactly one rule
    // consumes the token, so the measure cannot quietly spread to the transcript or
    // the card (which would re-open the void beside the rail).
    let measured: Vec<String> = aggregated_rules(&css)
        .into_iter()
        .filter(|(_, body)| declarations_of(body).iter().any(|(_, v)| v.contains("--measure-prose")))
        .map(|(selector, _)| selector)
        .collect();
    assert_eq!(measured, vec![".final".to_string()], "only the answer prose (`.final`) takes the reading measure");
    let fin = aggregated_rules(&css)
        .into_iter()
        .find(|(s, _)| s == ".final")
        .map(|(_, b)| b)
        .expect("`.final` is styled");
    assert_eq!(
        declared(&fin, "max-width").as_deref(),
        Some("var(--measure-prose)"),
        "answer prose is capped by `--measure-prose` (FR-UI-45)",
    );
    // Left-anchored: nothing pushes the capped box off the transcript's left content
    // edge — no auto/non-zero left margin on it, no self-alignment, and no container
    // between it and the transcript that aligns its children anywhere but the start.
    for (n, v) in declarations_of(&fin) {
        // The LEFT margin each spelling resolves to: the shorthand's 1–4 value forms,
        // `margin-inline`'s start, and the longhands. Anything but zero moves the box.
        let vals = css_values(&v);
        let left = match (n.as_str(), vals.len()) {
            ("margin", 4) => vals.get(3).cloned(),
            ("margin", 2 | 3) => vals.get(1).cloned(),
            ("margin" | "margin-inline" | "margin-left" | "margin-inline-start", _) => vals.first().cloned(),
            _ => None,
        };
        assert!(
            left.as_deref().is_none_or(|l| l == "0" || l == "0px"),
            "`.final` declares `{n}: {v}` — the measure starts on the content edge (BR-39)",
        );
        assert!(
            !(matches!(n.as_str(), "align-self" | "justify-self" | "place-self") && v != "stretch" && v != "start" && v != "flex-start"),
            "`.final` self-aligns (`{n}: {v}`) — the measure is left-anchored (FR-UI-45)",
        );
    }
    for container in [".log", ".assistant", ".answer"] {
        let body = rule_body(&css, container);
        for p in ["align-items", "place-items", "justify-items"] {
            let v = declared(&body, p);
            assert!(
                v.as_deref().is_none_or(|v| matches!(v, "stretch" | "normal" | "start" | "flex-start")),
                "`{container}` aligns its children with `{p}: {v:?}` — the prose measure must start on \
                 the transcript's left content edge (FR-UI-45)",
            );
        }
    }
}

/// The two-pane grid is the WIKI's (S-308, [FR-UI-33]): the chat layout reads its
/// rail track and column gap off `WikiView.module.css` rather than restating them,
/// so the two views cannot drift apart without this failing. The rail spans the full
/// column height with no viewport-height cap of its own, and its "+ New chat" is a
/// ≥36px action pinned above the list that scrolls.
#[test]
fn chat_two_pane_grid_is_the_wiki_rail_track_at_full_column_height() {
    let css = strip_comments(&read("src/views/chat/Chat.module.css"));
    let wiki = strip_comments(&read("src/views/wiki/WikiView.module.css"));
    let prop = declared;
    let (chat_layout, wiki_layout) = (rule_body(&css, ".layout"), rule_body(&wiki, ".layout"));
    for p in ["grid-template-columns", "gap"] {
        let (c, w) = (prop(&chat_layout, p), prop(&wiki_layout, p));
        assert!(w.is_some(), "the Wiki `.layout` declares `{p}`");
        assert_eq!(c, w, "the chat two-pane `{p}` is the Wiki menu's, verbatim (FR-UI-33)");
    }

    // Full column height: the row stretches its items (the grid default). The Wiki's
    // own `align-items: start` is exactly what must NOT be copied — it is what sized
    // the chat rail to its items (240×99px).
    // Every spelling of block-axis alignment is read — the row's own (`align-items`,
    // and the first half of `place-items`) and the rail's (`align-self`, the first
    // half of `place-self`) — because any one of them set to `start` collapses the
    // rail to its items again.
    let stretches = |v: Option<String>| {
        v.map(|v| css_values(&v).first().cloned().unwrap_or_default())
            .is_none_or(|t| matches!(t.as_str(), "stretch" | "normal" | "auto"))
    };
    for p in ["align-items", "place-items"] {
        let v = prop(&chat_layout, p);
        assert!(stretches(v.clone()), "the chat `.layout` stretches the rail to the row (found `{p}: {v:?}`)");
    }
    for (selector, body) in aggregated_rules(&css) {
        if selector != ".railPane" && selector != ".rail" {
            continue;
        }
        for p in ["align-self", "place-self"] {
            let v = prop(&body, p);
            assert!(stretches(v.clone()), "`{selector}` opts out of the row's stretch (found `{p}: {v:?}`)");
        }
    }
    let rail = rule_body(&css, ".rail");
    assert_eq!(prop(&rail, "height").as_deref(), Some("100%"), "the rail fills its stretched pane");

    // The pane is size-contained on desktop, so a long list scrolls inside the rail
    // instead of stretching the row past the card — without it the list grows the
    // row, the PAGE scrolls, the list never does, and "+ New chat" is no longer
    // pinned above anything. Required, not merely checked-if-present.
    assert!(
        prop(&rule_body(&css, ".railPane"), "contain").is_some_and(|v| v.contains("size")),
        "the desktop `.railPane` is size-contained, so the row is the card's height and the \
         conversation list scrolls inside the rail (FR-UI-33)",
    );
    // Below the breakpoint the rail stacks and must size to its content again: a
    // containment NOT released there collapses the opened narrow-viewport rail to 0px.
    let narrow = top_level_rules(&css)
        .into_iter()
        .find(|(at, _)| at == "@media (max-width: 1023px)")
        .map(|(_, body)| body)
        .expect("the chat layout has its ≤1023px rung");
    assert_eq!(
        prop(&rule_body(&narrow, ".railPane"), "contain").as_deref(),
        Some("none"),
        "the ≤1023px rung releases the rail pane's size containment, or the opened rail \
         renders 0px tall",
    );
    // …and the pane is the ONLY size-contained part of the rail: the rung releases
    // exactly that one, so containment moved onto `.rail` or the list would survive
    // into the narrow viewport and collapse it just the same.
    for (selector, body) in aggregated_rules(&css) {
        if selector == ".railPane" || !(selector.starts_with(".rail") || selector.starts_with(".thread") || selector == ".newChat") {
            continue;
        }
        assert!(
            !declarations_of(&body).iter().any(|(n, v)| n == "contain" && v.contains("size")),
            "`{selector}` is size-contained; only `.railPane` may be — it is the one the ≤1023px \
             rung releases",
        );
    }

    // No viewport-height cap on any part of the rail, and the rail as a whole does not
    // scroll — only the list does, which is what keeps "+ New chat" pinned.
    // Rungs included, and the logical `block-size` spelling and every viewport unit
    // (`vh`/`dvh`/`svh`/`lvh`, `vb`, `vmin`/`vmax`) read, as the measure guard does.
    let rail_parts = [".railPane", ".rail", ".threadList", ".newChat"];
    for (selector, body) in aggregated_rules(&css) {
        if !rail_parts.contains(&selector.as_str()) {
            continue;
        }
        for (n, v) in declarations_of(&body) {
            let sizes_block = n.contains("height") || n.contains("block-size");
            let viewport = ["vh", "vb", "vmin", "vmax"].iter().any(|u| v.contains(u));
            assert!(
                !(sizes_block && viewport),
                "`{selector}` declares `{n}: {v}` — the rail's bound is its grid row, not the \
                 viewport (FR-UI-33)",
            );
        }
    }
    assert!(prop(&rail, "overflow-y").is_none() && prop(&rail, "overflow").is_none(), "the rail itself does not scroll");
    let list = rule_body(&css, ".threadList");
    assert_eq!(prop(&list, "overflow-y").as_deref(), Some("auto"), "the conversation list is the scroll region");
    assert_eq!(prop(&list, "min-height").as_deref(), Some("0"), "the list can shrink below its content, so it scrolls");

    let new_chat = rule_body(&css, ".newChat");
    assert_eq!(prop(&new_chat, "flex").as_deref(), Some("none"), "+ New chat never shrinks into the list's scroll");
    let floor = prop(&new_chat, "min-height").and_then(|v| length_px(&v));
    assert!(
        floor.is_some_and(|px| px >= 36.0),
        "+ New chat is at least 36px tall (found min-height {floor:?})",
    );
}

/// The transcript stays a BOUNDED scroll box inside the card (FR-UI-33, CR-092
/// §3.2). It is assistant-ui's thread viewport, which follows the stream by driving
/// its own `scrollTop`; an unbounded box would silently stop auto-follow on the app's
/// only streaming surface and walk the composer off-screen mid-answer.
///
/// S-619 ([FR-UI-45], CR-204 L2) changed WHAT bounds it: no longer a `60vh` cap but
/// the card's height. At desktop widths the transcript is the card's `flex: 1`
/// scroller, size-contained so a 200-message thread never asks the page for more
/// than the viewport, with a floor; every link from the view down to it may shrink,
/// and the view fills the shell's content box. Below the breakpoint the panes stack,
/// the view takes its content height, and the transcript is the floor-and-cap box it
/// was. Whether the page then really does not scroll is the `chat-layout` Playwright
/// spec's to measure; this pins the declarations that make it so.
#[test]
fn chat_transcript_is_a_bounded_scroll_box_inside_the_card() {
    let css = strip_comments(&read("src/views/chat/Chat.module.css"));
    let log = rule_body(&css, ".log");
    let get = |p: &str| declared(&log, p);
    assert_eq!(get("overflow-y").as_deref(), Some("auto"), "the transcript scrolls internally");
    assert!(
        get("flex").is_some_and(|v| css_values(&v).first().is_some_and(|g| g != "0")),
        "the transcript is the card's `flex: 1` scroller — it takes the height the card has          left (found `flex: {:?}`)",
        get("flex"),
    );
    assert!(
        get("contain").is_some_and(|v| v.contains("size")),
        "the transcript is size-contained, so a long thread is never the page's height (FR-UI-45)",
    );
    assert!(get("min-height").is_some_and(|v| v != "0"), "the transcript has a floor");
    for p in ["max-height", "height"] {
        assert!(
            get(p).is_none_or(|v| !["vh", "vb", "vmin", "vmax"].iter().any(|u| v.contains(u))),
            "the desktop transcript's bound is the card, not a viewport fraction (found `{p}: {:?}`)",
            get(p),
        );
    }

    // The chain from the view down to the transcript: the view fills the shell's
    // content box, and every flex link between them may shrink below its content.
    assert_eq!(declared(&rule_body(&css, ".view"), "height").as_deref(), Some("100%"), "the view fills its content box");
    for selector in [".chat", ".layout", ".main", ".conversation", ".threadRoot"] {
        let body = rule_body(&css, selector);
        assert_eq!(
            declared(&body, "min-height").as_deref(),
            Some("0"),
            "`{selector}` may shrink below its content, or the transcript above it can never              be the one that scrolls (FR-UI-45)",
        );
    }
    for selector in [".chat", ".layout", ".conversation", ".threadRoot"] {
        let flex = declared(&rule_body(&css, selector), "flex");
        assert!(
            flex.as_deref().is_some_and(|v| css_values(v).first().is_some_and(|g| g != "0")),
            "`{selector}` grows into the height its parent gives it (found `flex: {flex:?}`)",
        );
    }
    // The composer is pinned under the transcript: it never grows or shrinks.
    assert_eq!(declared(&rule_body(&css, ".composer"), "flex").as_deref(), Some("none"), "the composer is pinned");

    // Stacked (≤1023px) the chain is released and the old bounded box returns.
    let narrow: Vec<(String, String)> = top_level_rules(&css)
        .into_iter()
        .filter(|(at, _)| at == "@media (max-width: 1023px)")
        .flat_map(|(_, body)| top_level_rules(&body))
        .collect();
    let rung = |selector: &str| {
        narrow
            .iter()
            .find(|(s, _)| s == selector)
            .map(|(_, b)| b.clone())
            .unwrap_or_else(|| panic!("the ≤1023px rung restyles `{selector}`"))
    };
    assert_eq!(declared(&rung(".view"), "height").as_deref(), Some("auto"), "stacked, the view takes its content height");
    let narrow_log = rung(".log");
    assert_eq!(declared(&narrow_log, "contain").as_deref(), Some("none"), "stacked, the transcript is measured by its content");
    assert!(
        declared(&narrow_log, "max-height").is_some_and(|v| v != "none"),
        "stacked, the transcript carries a height cap again — there is no card height to fill",
    );
}

#[test]
fn chat_transcript_has_generous_spacing_and_viewport_padding() {
    // With the card chrome gone, spacing is what separates one turn from the next.
    let log = rule_body(&strip_comments(&read("src/views/chat/Chat.module.css")), ".log");
    assert!(log.contains("gap: var(--space-6)"), "generous inter-turn spacing");
    assert!(log.contains("padding: var(--space-5) var(--space-4)"), "increased viewport padding");
}

#[test]
fn chat_transcript_text_on_the_card_fill_clears_wcag_aa_in_both_themes() {
    // Every rule in the chat view that declares its own ink clears the 4.5:1 AA body
    // minimum against WHATEVER IT ACTUALLY SITS ON. S-308 moved the transcript into
    // the view's `Card`, so that base is the CARD FILL — read off `Card.module.css`
    // here, not restated, so the guard follows the card if its fill ever moves. It
    // was `--surface-0` while S-300's transcript sat bare on the page.
    //
    // What used to be list (a) here — the closed set of selectors that ink STRAIGHT
    // ONTO a fill they do not own — is now
    // `every_neutral_ink_in_the_chat_stylesheet_is_classified_and_clears_wcag_aa`
    // below (S-311): a hardcoded 4-entry list could say nothing about a selector it
    // did not name, which is the exact shape that let a signal-hue ink (S-301,
    // measured separately by `every_signal_hue_ink_in_the_chat_stylesheet_is_…`
    // above) ship unmeasured a story after this file's fill/ink split was written.
    // What remains here are the two halves that list's inversion does not replace:
    // (a′) the signal hue those same rules carry on a border EDGE instead of their
    // ink, and (b) the Activity glyphs' fill/ink pairs, measured against their OWN
    // fill rather than a shared one.
    let css = strip_comments(&read("src/views/chat/Chat.module.css"));
    let card = strip_comments(&read("src/components/Card.module.css"));
    let tokens = strip_comments(&read("src/styles/tokens.css"));
    let base = declarations(&block_after(&tokens, ":root"));
    let light = declarations(&block_after(&tokens, ":root[data-theme=\"light\"]"));

    let themes = || {
        [("dark", theme_map(&base, None)), ("light", theme_map(&base, Some(&light)))]
    };

    // The base, derived rather than assumed: the card's own fill, and nothing
    // between it and the turns repaints it. A `background` on the thread root or
    // the log would put the transcript on a surface this guard never measured.
    let card_fill = var_token(&rule_body(&card, ".card"), "background")
        .expect("the shared `.card` declares a `background: var(--…)` fill");
    for selector in [".threadRoot", ".log"] {
        assert!(
            !declarations_of(&rule_body(&css, selector)).iter().any(|(n, _)| n.starts_with("background")),
            "`{selector}` paints no fill of its own: the transcript sits on the card fill \
             ({card_fill}), which is what the pairs below are measured against",
        );
    }
    // The rail renders no card since S-619: its notes sit on the page fill `body`
    // paints, which is what `.railError`'s edge is measured against below.
    let rail_fill = var_token(&rule_body(&strip_comments(&read("src/styles/base.css")), "body"), "background")
        .expect("`body` declares a `background: var(--…)` page fill");

    // (a′) The other half of (a): the hue those rules keep OFF their ink must still
    // be ON them, as the left edge — relocated, never dropped (FR-UI-33). Resolved in
    // cascade order over every rule naming the selector (`.error` recolours the edge
    // it shares with `.halt`), and measured as a ≥3:1 non-text affordance on the fill
    // the notice sits on.
    let aggregated = aggregated_rules(&css);
    for (selector, surface) in [
        (".halt", card_fill.as_str()),
        (".error", card_fill.as_str()),
        (".railError", rail_fill.as_str()),
    ] {
        let body = &aggregated.iter().find(|(s, _)| s == selector).expect("the notice rule exists").1;
        let mut edge = None;
        for (n, v) in declarations_of(body) {
            if matches!(n.as_str(), "border-left" | "border-inline-start" | "border-left-color" | "border-inline-start-color") {
                if let Some(t) = css_values(&v).iter().find_map(|t| t.strip_prefix("var(").and_then(|t| t.strip_suffix(')'))) {
                    edge = Some(t.split(',').next().unwrap_or("").trim().to_string());
                }
            }
        }
        let edge = edge.unwrap_or_else(|| {
            panic!("`{selector}` carries its signal on a left edge `border-left: … var(--…)` — the hue was relocated off the ink, not dropped")
        });
        assert!(
            SIGNAL_HUES.contains(&edge.as_str()),
            "`{selector}`'s left edge is `{edge}`, not a signal hue — the notice's tone is gone",
        );
        for (theme, map) in themes() {
            let c = contrast(token_rgb(&edge, &map, &base), token_rgb(surface, &map, &base));
            assert!(
                c >= 3.0,
                "{theme}: `{selector}`'s edge ({edge}) is {c:.2}:1 on {surface}, below the 3:1 \
                 non-text floor an edge affordance must clear",
            );
        }
    }

    // (b) The Activity status glyphs (S-301) are the sprint's near-miss and the
    // other resolution of the same rule. Added INSIDE the unfilled column a story
    // after this invariant was established, they first took the signal hues as
    // `color:` — `--color-pass` was 2.99:1 on the light page surface, under even the
    // 3:1 non-text floor (3.30:1 on today's card fill: still under AA). HF-1 put the
    // hue where the house keeps it: a `Badge`-style chip FILL. So they are measured
    // as fill/ink pairs — carrying them in list (a) would now be worse than wrong, it
    // would be vacuous, because `--ink-on-warm` never touches the card fill and
    // would pass on a contrast it does not have.
    for (selector, fill) in [(".activityRunning", "--so-orange"), (".activityDone", "--so-green")] {
        let body = rule_body(&css, selector);
        let ink = color_token(&body)
            .unwrap_or_else(|| panic!("`{selector}` declares a `color: var(--…)`"));
        let declared = var_token(&body, "background").unwrap_or_else(|| {
            panic!(
                "`{selector}` declares a `background: var(--…)`: the signal hue is this glyph's \
                 FILL, not its ink — as ink on the card fill it does not clear AA",
            )
        });
        assert_eq!(
            declared, fill,
            "`{selector}` carries the state's own signal hue as its fill",
        );
        for (theme, map) in themes() {
            let c = contrast(token_rgb(&ink, &map, &base), token_rgb(fill, &map, &base));
            assert!(
                c >= 4.5,
                "{theme}: `{selector}` ink ({ink}) is {c:.2}:1 on its own fill ({fill}), below the \
                 4.5:1 AA minimum the `Badge` chips hold to — a fill only carries a signal if the \
                 glyph on it stays legible",
            );
        }
    }
}

/// The other half of the fill/ink-pair affordance the contrast guard above measures:
/// the glyphs must actually be CHIPS. Without the `Badge` geometry a fill reads as
/// tinted text on a stray coloured background — and the measurement above would go
/// on passing, because a fill/ink ratio says nothing about whether the fill looks
/// deliberate. Separate test rather than a third block up there: this is a structural
/// assertion, not a contrast one, and a failure should say so by name.
#[test]
fn chat_activity_status_glyphs_carry_badge_chip_geometry() {
    let css = strip_comments(&read("src/views/chat/Chat.module.css"));
    let chip = rule_body(&css, ".activityRunning, .activityDone");
    for decl in [
        "display: inline-flex",
        "align-items: center",
        "border-radius: var(--radius-sm)",
        "padding: 0.2rem 0.45rem",
        "border: 1px solid transparent",
        "line-height: 1",
    ] {
        assert!(
            chip.contains(decl),
            "the Activity glyphs carry `Badge`'s chip geometry (`{decl}`) so the signal hue reads \
             as an affordance, not as tinted text",
        );
    }
}

// ── S-618 (CR-204 L1/L7/L8, FR-UI-45): chat tokens, prose scale, calmer red ────

/// The theme-independent chat scale tokens (L8) and the value each must resolve to in
/// BOTH themes. `--elevation-composer` is the one THEMED chat token and is asserted
/// separately below. Every later CR-204 story consumes these by name, so a rename or a
/// silent value drift fails here rather than in a downstream story's layout.
const CHAT_SCALE_TOKENS: &[(&str, &str)] = &[
    ("--radius-3", "24px"),
    ("--measure-prose", "72ch"),
    ("--text-chat", "1rem"),
    ("--leading-chat", "1.75"),
    ("--tracking-heading", "-0.025em"),
];

#[test]
fn chat_tokens_resolve_in_both_themes_and_are_declared_once() {
    let tokens = strip_comments(&read("src/styles/tokens.css"));
    let base = declarations(&block_after(&tokens, ":root"));
    let light = declarations(&block_after(&tokens, ":root[data-theme=\"light\"]"));
    let dark = declarations(&block_after(&tokens, ":root[data-theme=\"dark\"]"));
    let themes = [("dark", theme_map(&base, Some(&dark))), ("light", theme_map(&base, Some(&light)))];

    for (theme, map) in &themes {
        for (name, value) in CHAT_SCALE_TOKENS {
            assert_eq!(
                map.get(*name).map(String::as_str),
                Some(*value),
                "{theme}: `{name}` resolves to `{value}` (CR-204 L8)",
            );
        }
        assert!(map.contains_key("--elevation-composer"), "{theme}: `--elevation-composer` resolves");
    }
    // The composer's lift is THEMED: a shadow tuned for dark would be a smudge on the
    // white light surface. Each of the four blocks that declares it is pinned to its
    // theme's exact value — including the first-visit `prefers-color-scheme: light`
    // flip, which is what a light-OS user with no stored choice actually gets and which
    // a resolved-theme map never reads (it inherits only `:root` and the explicit
    // blocks).
    const DARK_LIFT: &str = "0 4px 16px rgba(0, 0, 0, 0.45)";
    const LIGHT_LIFT: &str = "0 4px 16px rgba(61, 57, 53, 0.12)";
    let first_visit_light = declarations(&block_after(&tokens, ":root:not([data-theme])"));
    for (block, map, want) in [
        (":root", &base, DARK_LIFT),
        (":root[data-theme=\"dark\"]", &dark, DARK_LIFT),
        (":root[data-theme=\"light\"]", &light, LIGHT_LIFT),
        ("@media (prefers-color-scheme: light) :root:not([data-theme])", &first_visit_light, LIGHT_LIFT),
    ] {
        assert_eq!(
            map.get("--elevation-composer").map(String::as_str),
            Some(want),
            "`{block}` declares the composer lift tuned for its theme",
        );
    }

    // "Defined once": a scale token is declared in exactly one place (the `:root` scale
    // layer), the themed shadow once per theme block (`:root`, the first-visit
    // light flip, explicit light, explicit dark) — and never restated in a module.
    for (name, _) in CHAT_SCALE_TOKENS {
        assert_eq!(tokens.matches(&format!("{name}:")).count(), 1, "`{name}` is declared exactly once in tokens.css");
    }
    assert_eq!(
        tokens.matches("--elevation-composer:").count(),
        4,
        "`--elevation-composer` is declared once per theme block",
    );
    let chat = strip_comments(&read("src/views/chat/Chat.module.css"));
    for name in CHAT_SCALE_TOKENS.iter().map(|(n, _)| *n).chain(["--elevation-composer"]) {
        assert!(!chat.contains(&format!("{name}:")), "Chat.module.css consumes `{name}`, never declares it");
    }

    // The chat scale is chat-scoped: the app-wide base size is untouched.
    assert_eq!(base.get("--text-base").map(String::as_str), Some("0.9375rem"), "`--text-base` stays 15px");
    assert!(
        strip_comments(&read("src/styles/base.css")).contains("font-size: var(--text-base)"),
        "the document still sets its size from `--text-base`",
    );
}

#[test]
fn chat_answer_prose_runs_on_the_chat_scale_and_headings_track_tight() {
    let css = strip_comments(&read("src/views/chat/Chat.module.css"));
    // Aggregated, not `rule_body`: the leading is declared on a grouped list
    // (`.final .markdown p, … ol`), which `rule_body`'s exact full-selector match never
    // finds, and `aggregated_rules` splits it into one entry per selector.
    let rules = aggregated_rules(&css);
    let value = |selector: &str, prop: &str| {
        rules.iter().find(|(s, _)| s == selector).and_then(|(_, body)| declared(body, prop))
    };
    let must = |selector: &str, prop: &str| {
        value(selector, prop).unwrap_or_else(|| panic!("`{selector}` declares no `{prop}`"))
    };

    // 16px / 28px, scoped to the ANSWER: `.final` carries the size (it reaches every
    // prose child by inheritance), the prose blocks carry the 1.75 leading.
    assert_eq!(must(".final", "font-size"), "var(--text-chat)");
    for selector in [".final .markdown p", ".final .markdown ul", ".final .markdown ol"] {
        assert_eq!(must(selector, "line-height"), "var(--leading-chat)", "`{selector}` leading");
    }
    // …and NOT on the renderer itself: `.markdown` also draws the Activity fold's step
    // results (`.activityResult`, `--text-sm`), and a size declared on it would override
    // the fold's and grow the quiet context block to answer size.
    assert_eq!(value(".markdown", "font-size"), None, "`.markdown` declares no size of its own");
    // No prose child is re-sized or re-leaded by a rule of its own, which would defeat
    // the inherited 16px / 28px: the descendants must not name `font-size`, and `li`
    // (which inherits its leading from the list) must not name `line-height`.
    for scope in [".markdown", ".final .markdown"] {
        for tag in ["p", "ul", "ol", "li"] {
            let selector = format!("{scope} {tag}");
            assert_eq!(value(&selector, "font-size"), None, "`{selector}` must inherit the chat size");
        }
        assert_eq!(value(&format!("{scope} li"), "line-height"), None, "`{scope} li` inherits the list's leading");
    }
    // The streamed-text wrapper's leading is the base one: the 1.75 is the prose blocks'.
    assert_eq!(must(".final", "line-height"), "var(--leading-base)");
    // The user's text sits on the same scale as the answer.
    assert_eq!(must(".userBubble", "font-size"), "var(--text-chat)");
    assert_eq!(must(".userBubble", "line-height"), "var(--leading-chat)");

    // h2 → h4: weight 600, tightened tracking, the tight heading leading and the rem
    // ladder; h5/h6 never fall below the prose they sit among.
    for (tag, size) in [
        ("h2", "1.875rem"),
        ("h3", "1.5rem"),
        ("h4", "1.25rem"),
        ("h5", "var(--text-chat)"),
        ("h6", "var(--text-chat)"),
    ] {
        let selector = format!(".final .markdown {tag}");
        assert_eq!(must(&selector, "font-weight"), "var(--weight-semibold)", "`{selector}` weight");
        assert_eq!(must(&selector, "letter-spacing"), "var(--tracking-heading)", "`{selector}` tracking");
        assert_eq!(must(&selector, "line-height"), "var(--leading-tight)", "`{selector}` leading");
        assert_eq!(must(&selector, "font-size"), size, "`{selector}` size");
    }
}

/// Every custom property that IS the signal red: `--so-red` itself and each token that
/// resolves to it through `var()` aliases (`--color-accent`, `--card-accent`,
/// `--focus-ring`, …), read off `tokens.css` rather than hand-listed so a new alias
/// cannot slip a red fill past the guard below. `--color-accent-warm` is `--so-orange`,
/// a different hue, and is not in the set.
fn signal_red_tokens(base: &BTreeMap<String, String>) -> Vec<String> {
    let mut red = vec!["--so-red".to_string()];
    loop {
        let before = red.len();
        for (name, value) in base {
            let aliased = value.trim().strip_prefix("var(").and_then(|v| v.strip_suffix(')'));
            let target = aliased.map(|t| t.split(',').next().unwrap_or("").trim());
            if !red.contains(name) && target.is_some_and(|t| red.iter().any(|r| r == t)) {
                red.push(name.clone());
            }
        }
        if red.len() == before {
            return red;
        }
    }
}

/// Whether a declared value paints the signal red: it names one of `red` as a `var()` —
/// alone, with a fallback, or inside a gradient, and with or without whitespace inside
/// the parentheses. The name must END at `)` or `,`, so `--color-accent-warm` (a
/// different hue) and a hypothetical `--so-red-hover` are not mistaken for it.
fn paints_signal_red(value: &str, red: &[String]) -> bool {
    let compact: String = value.split_whitespace().collect();
    red.iter().any(|name| {
        let prefix = format!("var({name}");
        compact.match_indices(&prefix).any(|(i, _)| {
            matches!(compact[i + prefix.len()..].chars().next(), Some(')' | ','))
        })
    })
}

#[test]
fn signal_red_fill_detector_admits_every_spelling_and_rejects_its_near_misses() {
    let tokens = strip_comments(&read("src/styles/tokens.css"));
    let red = signal_red_tokens(&declarations(&block_after(&tokens, ":root")));
    // The aliases are DERIVED: each of these resolves to `--so-red` in tokens.css.
    for alias in ["--so-red", "--color-accent", "--card-accent", "--focus-ring"] {
        assert!(red.iter().any(|r| r == alias), "`{alias}` is a signal-red alias (derived: {red:?})");
    }
    for hue in ["--color-accent-warm", "--so-orange", "--color-pass", "--surface-2", "--text-1"] {
        assert!(!red.iter().any(|r| r == hue), "`{hue}` is not the signal red");
    }
    for painted in [
        "var(--so-red)",
        "var(--color-accent)",
        "var(--card-accent)",
        "var(--focus-ring)",
        "var( --so-red )",
        "var(--so-red )",
        "var(--color-accent, #f00)",
        "var(--so-red) no-repeat",
        "linear-gradient(var(--so-red), transparent)",
    ] {
        assert!(paints_signal_red(painted, &red), "`{painted}` paints the signal red");
    }
    for not_red in [
        "var(--color-accent-warm)",
        "var(--so-red-hover)",
        "var(--so-orange)",
        "var(--surface-2)",
        "var(--text-1)",
        "transparent",
    ] {
        assert!(!paints_signal_red(not_red, &red), "`{not_red}` is not the signal red");
    }
}

/// The chat rules that fill with the signal red, and why each is allowed: a small
/// signal, never a large control. CR-204 L7: `--so-red` is a small signal in the chat —
/// the focus ring, hover borders, the 3px notice edges (`.threadConfirm`, `.railError`,
/// `.error`), the sendable Send and the rail's active marker. This list is the coverage
/// contract: the test below fails on any other chat rule that paints a red
/// `background`, and on a listed rule that does not bound its own box to ≤ 40×40px.
///
/// S-619 reconciled S-618's sendable dot with the round in-pill Send: the 2rem circle
/// IS the sendable signal now, so the dot is gone — one signal, not two.
const RED_FILL_COVERAGE: &[(&str, &str)] = &[
    (".workingPulse", "the 0.5rem 'working' dot"),
    (".send:not(:disabled)", "the 2rem round Send, red only while there is something to send"),
    (".threadLineActive::before", "the rail's 3px × 1rem active-row marker"),
];

#[test]
fn no_chat_rule_fills_a_control_larger_than_40px_with_the_signal_red() {
    let tokens = strip_comments(&read("src/styles/tokens.css"));
    let red = signal_red_tokens(&declarations(&block_after(&tokens, ":root")));
    // Every chat stylesheet: a red fill in a sibling module is the same chat control.
    let red_fills: Vec<(String, String)> = chat_style_rules(&chat_stylesheets())
        .into_iter()
        .map(|(_, selector, body)| (selector, body))
        .filter(|(_, body)| {
            declarations_of(body).iter().any(|(n, v)| n.starts_with("background") && paints_signal_red(v, &red))
        })
        .collect();
    for (selector, body) in &red_fills {
        assert!(
            RED_FILL_COVERAGE.iter().any(|(s, _)| s == selector),
            "`{selector}` fills with the signal red but is not in `RED_FILL_COVERAGE`. A chat \
             control larger than 40×40px takes a NEUTRAL fill (CR-204 L7, FR-UI-45): \
             `--so-red` is a small signal only",
        );
        for dimension in ["width", "height"] {
            let px = declared(body, dimension).as_deref().and_then(length_px);
            assert!(
                px.is_some_and(|px| px <= 40.0),
                "`{selector}` is a red fill, so it must bound its own `{dimension}` to ≤ 40px \
                 (declared: {:?})",
                declared(body, dimension),
            );
        }
    }
    // …and the list cannot rot: a classified rule that no longer paints red is stale.
    for (selector, note) in RED_FILL_COVERAGE {
        assert!(
            red_fills.iter().any(|(s, _)| s == selector),
            "`{selector}` ({note}) is classified but no longer fills with the signal red — drop it",
        );
    }
}

#[test]
fn start_chatting_is_the_neutral_secondary_button_not_the_red_primary() {
    let view = strip_tsx_comments(&read("src/views/chat/ChatView.tsx"));
    let label = view.find("Start chatting").expect("the consent banner renders 'Start chatting'");
    let button = opening_tags(&view, "Button")
        .into_iter()
        .filter(|&i| i < label)
        .max()
        .expect("'Start chatting' is a `<Button>`'s label");
    let tag = &view[button..label];
    assert!(
        tag.contains("variant=\"secondary\"") && !tag.contains("primary"),
        "'Start chatting' is a neutral `secondary` button — the red `primary` fill is for small \
         signals only in the chat (CR-204 L7): {tag}",
    );
}

// ── S-619 (CR-204 L3/L6/C8, FR-UI-45): pill composer, cardless rail, consent chip ──

/// A resolved colour token as straight sRGB channels plus alpha: a `#hex` is opaque,
/// an `rgba(r, g, b, a)` carries its alpha. The rail's pill tints are translucent
/// (`--tint-hover`, `--tint-selected`), so their contrast is only meaningful once
/// composited over the surface they sit on.
fn token_rgba(name: &str, map: &BTreeMap<String, String>, base: &BTreeMap<String, String>) -> ((u8, u8, u8), f64) {
    let raw = map.get(name).or_else(|| base.get(name)).unwrap_or_else(|| panic!("token `{name}` missing"));
    let value = resolve(raw, map, base);
    if let Some(inner) = value.strip_prefix("rgba(").and_then(|v| v.strip_suffix(')')) {
        let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
        assert_eq!(parts.len(), 4, "`{name}` is `rgba(r, g, b, a)`: {value}");
        let channel = |i: usize| parts[i].parse::<u8>().unwrap_or_else(|_| panic!("`{name}` channel {i}: {value}"));
        let alpha: f64 = parts[3].parse().unwrap_or_else(|_| panic!("`{name}` alpha: {value}"));
        ((channel(0), channel(1), channel(2)), alpha)
    } else {
        (hex_rgb(&value), 1.0)
    }
}

/// `top` at `alpha` composited over the opaque `under` (straight-alpha "over").
fn composite((r, g, b): (u8, u8, u8), alpha: f64, (ur, ug, ub): (u8, u8, u8)) -> (u8, u8, u8) {
    let mix = |t: u8, u: u8| (alpha * t as f64 + (1.0 - alpha) * u as f64).round() as u8;
    (mix(r, ur), mix(g, ug), mix(b, ub))
}

#[test]
fn tint_compositing_reads_alpha_and_hex_alike() {
    // Opaque white at 6% over black is a 15/255 grey — the dark hover tint's arithmetic.
    assert_eq!(composite((255, 255, 255), 0.06, (0, 0, 0)), (15, 15, 15));
    // Fully opaque replaces; fully transparent leaves the surface.
    assert_eq!(composite((218, 41, 28), 1.0, (15, 18, 22)), (218, 41, 28));
    assert_eq!(composite((218, 41, 28), 0.0, (15, 18, 22)), (15, 18, 22));
    let tokens = strip_comments(&read("src/styles/tokens.css"));
    let base = declarations(&block_after(&tokens, ":root"));
    let light = theme_map(&base, Some(&declarations(&block_after(&tokens, ":root[data-theme=\"light\"]"))));
    // Dark: an rgba literal. Light: an alias to an opaque hex (`--so-merlin-50`).
    assert_eq!(token_rgba("--tint-hover", &base, &base), ((255, 255, 255), 0.06));
    assert_eq!(token_rgba("--tint-hover", &light, &base), ((0xf4, 0xf4, 0xf2), 1.0));
}

/// The cardless rail (S-619, [FR-UI-45], CR-204 L6): the rail sits on the page surface
/// with no card of its own; a row's LINE is a filled pill on hover, focus and when
/// open; the open row's signal is a small red marker on its leading edge and never an
/// outline; and the trash glyph is hidden on an idle row and revealed on row hover,
/// row focus, the open row and while its confirm is open. Every ink a row carries
/// clears its floor over the composited pill tints, in both themes.
#[test]
fn chat_rail_is_cardless_with_a_red_marker_and_a_hidden_idle_trash() {
    let css = strip_comments(&read("src/views/chat/Chat.module.css"));
    let rules = aggregated_rules(&css);
    let body_of = |selector: &str| {
        rules
            .iter()
            .find(|(s, _)| s == selector)
            .map(|(_, b)| b.clone())
            .unwrap_or_else(|| panic!("`{selector}` is styled"))
    };

    // (1) No card: no fill, border, radius, shadow or inset on the rail itself.
    for (n, v) in declarations_of(&body_of(".rail")) {
        assert!(
            !(n.starts_with("background") || n.starts_with("border") || n == "box-shadow" || n.starts_with("padding")),
            "the rail declares `{n}: {v}` — it renders no card (FR-UI-45)",
        );
    }

    // (2) The row line is the pill, filled by the two tints.
    assert_eq!(declared(&body_of(".threadLine"), "border-radius").as_deref(), Some("var(--radius-pill)"));
    for (selector, tint) in [
        (".threadLine:hover", "var(--tint-hover)"),
        (".threadLine:focus-within", "var(--tint-hover)"),
        (".threadLineActive", "var(--tint-selected)"),
        (".threadLineActive:hover", "var(--tint-selected)"),
    ] {
        assert_eq!(declared(&body_of(selector), "background").as_deref(), Some(tint), "`{selector}` fills the pill");
    }

    // (3) The open row carries the marker, and NO red outline or border anywhere on
    // a row — the confirm panel's 3px destructive edge is the one red edge, and it
    // is the panel's, not the row's.
    let tokens = strip_comments(&read("src/styles/tokens.css"));
    let base = declarations(&block_after(&tokens, ":root"));
    let red = signal_red_tokens(&base);
    let marker = body_of(".threadLineActive::before");
    assert!(declared(&marker, "background").is_some_and(|v| paints_signal_red(&v, &red)), "the active marker is red");
    assert_eq!(declared(&marker, "left").as_deref(), Some("0"), "the marker sits on the row's LEADING edge");
    for (selector, body) in &rules {
        let row = [".threadLine", ".threadItem", ".threadActive", ".threadDelete", ".threadRow"]
            .iter()
            .any(|r| selector.starts_with(r));
        if !row {
            continue;
        }
        for (n, v) in declarations_of(body) {
            assert!(
                !((n.starts_with("border") || n.starts_with("outline")) && paints_signal_red(&v, &red)),
                "`{selector}` draws `{n}: {v}` — the active row carries a marker, not a red outline \
                 (FR-UI-45)",
            );
        }
    }

    // (4) The trash is hidden on an idle row and revealed by exactly these states.
    // The top-level rule, not the aggregate: the `(hover: none)` rung shows the trash
    // always on a device that cannot hover, and is the deliberate exception.
    assert_eq!(declared(&rule_body(&css, ".threadDelete"), "opacity").as_deref(), Some("0"), "an idle row's trash is hidden");
    for reveal in [
        ".threadLine:hover .threadDelete",
        ".threadLine:focus-within .threadDelete",
        ".threadLineActive .threadDelete",
        ".threadDelete[aria-expanded=\"true\"]",
    ] {
        assert_eq!(declared(&body_of(reveal), "opacity").as_deref(), Some("1"), "`{reveal}` reveals the trash");
    }
    // (4b) The collapse toggle is shown at EVERY width (FR-UI-45): no rule in any rung
    // that names the toggle or the column holding it takes it off the page — at
    // ≤1023px the rail starts collapsed, so a hidden toggle there would leave the
    // conversation list unreachable.
    for (selector, body) in all_style_rules(&css) {
        if !(selector.contains(".railToggle") || selector.contains(".railColumn")) {
            continue;
        }
        assert!(
            !hides_element(&body),
            "`{selector}` hides the rail's collapse toggle — it is shown at every width (FR-UI-45)",
        );
    }

    // Hidden by opacity, never taken out of the tab order.
    assert!(
        !declarations_of(&body_of(".threadDelete"))
            .iter()
            .any(|(n, v)| (n == "visibility" && v == "hidden") || (n == "display" && v == "none")),
        "the hidden trash stays reachable by keyboard",
    );

    // (5) Inks over the composited tints. The page fill is `body`'s; a row's title is
    // text (4.5:1), its trash and the toggle are glyphs (the 3:1 non-text floor).
    let base_css = strip_comments(&read("src/styles/base.css"));
    let page = var_token(&rule_body(&base_css, "body"), "background").expect("`body` paints the page fill");
    let light = declarations(&block_after(&tokens, ":root[data-theme=\"light\"]"));
    for (theme, map) in [("dark", theme_map(&base, None)), ("light", theme_map(&base, Some(&light)))] {
        let under = token_rgb(&page, &map, &base);
        for tint in ["--tint-hover", "--tint-selected"] {
            let (rgb, alpha) = token_rgba(tint, &map, &base);
            let fill = composite(rgb, alpha, under);
            for (selector, floor) in [(".threadItem", 4.5), (".threadDelete", 3.0), (".railToggle:hover", 3.0)] {
                let ink = color_token(&body_of(selector)).unwrap_or_else(|| panic!("`{selector}` inks a token"));
                let c = contrast(token_rgb(&ink, &map, &base), fill);
                assert!(
                    c >= floor,
                    "{theme}: `{selector}` ({ink}) is {c:.2}:1 on the page under {tint}, below {floor}:1",
                );
            }
        }
    }
}

/// The turns' last layer (S-620, [FR-UI-45], [FR-UI-46], CR-204 L4/C10): the user
/// bubble takes the 24px chat radius and a 10px × 20px inset; the action bar is two
/// 24px icon buttons that fill with `--tint-hover` on hover AND keyboard focus; the
/// floating ↓ control is a round neutral button inside the 40×40px a control may take
/// without the signal red, and the muted hint under the pill is a quiet ink.
///
/// This lives here rather than in Vitest because the SPA tests run with `css: false`
/// (the module is a proxy there): the radius, the box and the tint only exist in the
/// stylesheet. The assistant turn staying flat is
/// `chat_assistant_turn_carries_no_card_chrome`, which this story leaves untouched.
#[test]
fn chat_turns_are_bubbles_with_an_icon_action_bar_and_a_scroll_control() {
    let css = strip_comments(&read("src/views/chat/Chat.module.css"));
    let rules = aggregated_rules(&css);
    let body_of = |selector: &str| {
        rules
            .iter()
            .find(|(s, _)| s == selector)
            .map(|(_, b)| b.clone())
            .unwrap_or_else(|| panic!("`{selector}` is styled"))
    };

    // (1) The user bubble: the token (24px, asserted by the S-618 token guard), not a
    // literal that could drift from it, and the 10px × 20px inset.
    let bubble = body_of(".userBubble");
    assert_eq!(declared(&bubble, "border-radius").as_deref(), Some("var(--radius-3)"), "the bubble takes the 24px chat radius");
    assert_eq!(declared(&bubble, "padding").as_deref(), Some("10px 20px"), "the bubble's inset is 10px × 20px");
    let tokens = strip_comments(&read("src/styles/tokens.css"));
    let base = declarations(&block_after(&tokens, ":root"));
    assert_eq!(base.get("--radius-3").map(String::as_str), Some("24px"), "`--radius-3` is the 24px chat radius");

    // (2) The icon action bar: a 24px box (1.5rem at the 16px root), a ghost until
    // hovered or keyboard-focused, then the shared hover tint. Both states.
    let icon = body_of(".actionIcon");
    assert_eq!(declared(&icon, "width").as_deref(), Some("1.5rem"), "24px wide");
    assert_eq!(declared(&icon, "height").as_deref(), Some("1.5rem"), "24px tall");
    assert_eq!(declared(&icon, "background").as_deref(), Some("transparent"), "a ghost at rest");
    for state in [".actionIcon:hover", ".actionIcon:focus-visible"] {
        assert_eq!(
            declared(&body_of(state), "background").as_deref(),
            Some("var(--tint-hover)"),
            "`{state}` takes the --tint-hover fill",
        );
    }
    // Copy's visually-hidden "Copied" status is absolutely positioned (`.sr-only`); the
    // action bar must be its containing block, or it resolves to the conversation card —
    // outside the transcript's scroller — and the page grows by the thread's height.
    assert_eq!(
        declared(&body_of(".actions"), "position").as_deref(),
        Some("relative"),
        "`.actions` contains the hidden status, keeping it inside the transcript's scroller",
    );
    // A control that small cannot be the signal red.
    let red = signal_red_tokens(&base);
    for (selector, body) in &rules {
        if selector.starts_with(".actionIcon") || selector.starts_with(".scrollToBottom") {
            for (n, v) in declarations_of(body) {
                assert!(
                    !(n.starts_with("background") && paints_signal_red(&v, &red)),
                    "`{selector}` fills `{v}` — these small controls stay neutral (FR-UI-45)",
                );
            }
        }
    }
    // …and its glyph still reads over the composited tint, in both themes: the hover
    // ink is a text colour on a translucent fill over the card.
    let card = strip_comments(&read("src/components/Card.module.css"));
    let card_fill = var_token(&rule_body(&card, ".card"), "background").expect("the card paints a fill");
    let light = declarations(&block_after(&tokens, ":root[data-theme=\"light\"]"));
    for (theme, map) in [("dark", theme_map(&base, None)), ("light", theme_map(&base, Some(&light)))] {
        let (rgb, alpha) = token_rgba("--tint-hover", &map, &base);
        let fill = composite(rgb, alpha, token_rgb(&card_fill, &map, &base));
        for selector in [".actionIcon:hover", ".actionIcon[data-copied]"] {
            let ink = color_token(&body_of(selector)).unwrap_or_else(|| panic!("`{selector}` inks a token"));
            let c = contrast(token_rgb(&ink, &map, &base), fill);
            assert!(c >= 4.5, "{theme}: `{selector}` ({ink}) is {c:.2}:1 on the card under --tint-hover, below 4.5:1");
        }
    }

    // (3) The ↓ control: round, 2rem, bounded where it is declared, floating on its
    // sticky anchor rather than taking a place in the flow.
    let down = body_of(".scrollToBottom");
    assert_eq!(declared(&down, "border-radius").as_deref(), Some("var(--radius-pill)"), "round");
    assert_eq!(declared(&down, "width").as_deref(), Some("2rem"), "bounded to 32px wide");
    assert_eq!(declared(&down, "height").as_deref(), Some("2rem"), "bounded to 32px tall");
    assert_eq!(declared(&down, "position").as_deref(), Some("absolute"), "it floats");
    let anchor = body_of(".scrollAnchor");
    assert_eq!(declared(&anchor, "height").as_deref(), Some("0"), "its anchor takes no height of its own");
    assert_eq!(declared(&anchor, "position").as_deref(), Some("sticky"), "the anchor rides the scroller's visible bottom");
    assert_eq!(declared(&anchor, "bottom").as_deref(), Some("0"), "…at its bottom edge");
    // It adds zero height only because its negative top margin takes back exactly the
    // flex gap the transcript puts between its children — two declarations that must move
    // together, so the coupling is pinned rather than left to a layout spec to notice.
    let gap = declared(&rule_body(&css, ".log"), "gap").expect("the transcript declares its inter-turn gap");
    assert_eq!(
        declared(&anchor, "margin-top").as_deref(),
        Some(format!("calc(-1 * {gap})").as_str()),
        "the anchor's negative margin must equal `.log`'s gap ({gap})",
    );
}

/// The pill composer (S-619, [FR-UI-45], CR-204 L3): one container with the pill
/// radius, a hairline and the composer lift; its focus ring drawn on the container
/// (`:focus-within`) in the global ring's token, and the input drawing none of its
/// own; an input that grows to a capped height and then scrolls; and round in-pill
/// buttons that stay inside the 40×40px a red fill may take.
#[test]
fn chat_composer_is_one_pill_with_its_ring_on_the_container() {
    let css = strip_comments(&read("src/views/chat/Chat.module.css"));
    let pill = rule_body(&css, ".composer");
    assert_eq!(declared(&pill, "border-radius").as_deref(), Some("var(--radius-pill)"), "the composer is a pill");
    assert_eq!(declared(&pill, "box-shadow").as_deref(), Some("var(--elevation-composer)"), "it takes the composer lift");
    // A visible NEUTRAL hairline: 1px solid in a neutral token — never transparent
    // (no edge at all) and never the signal red, which FR-UI-45 keeps off the pill at
    // rest (its red is the focus ring and the sendable Send).
    let border = css_values(&declared(&pill, "border").expect("the pill declares its border"));
    assert_eq!(border.get(..2).map(|v| v.join(" ")).as_deref(), Some("1px solid"), "a hairline border: {border:?}");
    let edge = border.get(2).cloned().unwrap_or_default();
    let pill_tokens = strip_comments(&read("src/styles/tokens.css"));
    let pill_red = signal_red_tokens(&declarations(&block_after(&pill_tokens, ":root")));
    assert!(
        edge.starts_with("var(--border-") && !paints_signal_red(&edge, &pill_red),
        "the pill's edge is a neutral border token, not `{edge}`",
    );

    let ring = rule_body(&css, ".composer:focus-within");
    let outline = declared(&ring, "outline").expect("the pill draws the focus ring");
    assert!(outline.starts_with("2px solid") && outline.contains("var(--focus-ring)"), "the global ring's geometry and token: {outline}");
    assert_eq!(
        declared(&rule_body(&css, ".input:focus-visible"), "outline").as_deref(),
        Some("none"),
        "the input draws no second ring inside the pill's",
    );

    let input = rule_body(&css, ".input");
    assert_eq!(declared(&input, "overflow-y").as_deref(), Some("auto"), "a capped input scrolls");
    assert_eq!(declared(&input, "resize").as_deref(), Some("none"), "the pill sizes the input, not a drag handle");
    for p in ["border", "background"] {
        assert!(
            declared(&input, p).is_some_and(|v| v == "0" || v == "transparent"),
            "the input has no `{p}` of its own — the pill is the field (found {:?})",
            declared(&input, p),
        );
    }
    // The cap: the primitive grows the textarea to a bounded number of lines.
    let view = strip_tsx_comments(&read("src/views/chat/ChatView.tsx"));
    let composer = &view[view.find("function Composer(").expect("`Composer` is defined")..];
    let input_tag = &composer[composer.find("<ComposerPrimitive.Input").expect("the composer renders its input")..];
    let input_tag = &input_tag[..input_tag.find("/>").expect("the input tag closes")];
    let rows = |prop: &str| {
        input_tag
            .split(&format!("{prop}={{"))
            .nth(1)
            .and_then(|rest| rest.split('}').next())
            .and_then(|n| n.trim().parse::<u32>().ok())
    };
    assert_eq!(rows("minRows"), Some(1), "the input starts at ONE line (FR-UI-45)");
    assert!(!input_tag.contains("rows={"), "no fixed `rows` competes with the growth bounds");
    let max_rows = rows("maxRows");
    assert!(max_rows.is_some_and(|n| (2..=12).contains(&n)), "the input grows to a capped height (maxRows: {max_rows:?})");

    // Round, small buttons; Send neutral until sendable (the red half is
    // `RED_FILL_COVERAGE`'s).
    let buttons = rule_body(&css, ".composerMenu, .send, .stop");
    assert_eq!(declared(&buttons, "border-radius").as_deref(), Some("var(--radius-pill)"), "the in-pill buttons are round");
    for dimension in ["width", "height"] {
        let px = declared(&buttons, dimension).as_deref().and_then(length_px);
        assert!(px.is_some_and(|px| px <= 40.0), "the in-pill buttons bound their `{dimension}` to ≤ 40px ({px:?})");
    }
    // Each rounded chat control keeps its own radius on keyboard focus: the global
    // `:focus-visible` ring (`styles/base.css`) sets `--radius-sm` and is emitted after
    // this module at the same specificity, so a control that does not restate its
    // radius under `:focus-visible` squares off while focused.
    let base_css = strip_comments(&read("src/styles/base.css"));
    assert!(
        declared(&rule_body(&base_css, ":focus-visible"), "border-radius").is_some(),
        "the global ring sets a radius — if it stops, this check can go",
    );
    let rules = aggregated_rules(&css);
    let radius_of = |selector: &str| {
        rules.iter().find(|(s, _)| s == selector).and_then(|(_, b)| declared(b, "border-radius"))
    };
    for control in [".threadItem", ".threadDelete", ".composerMenu", ".send", ".stop", ".railToggle"] {
        let own = radius_of(control).unwrap_or_else(|| panic!("`{control}` declares its radius"));
        assert_eq!(
            radius_of(&format!("{control}:focus-visible")),
            Some(own.clone()),
            "`{control}` restates its radius ({own}) under `:focus-visible`, or the global ring squares it",
        );
    }

    let tokens = strip_comments(&read("src/styles/tokens.css"));
    let red = signal_red_tokens(&declarations(&block_after(&tokens, ":root")));
    assert!(
        declared(&rule_body(&css, ".send"), "background").is_some_and(|v| !paints_signal_red(&v, &red)),
        "an empty composer's Send is neutral",
    );
    assert!(
        declared(&rule_body(&css, ".send:not(:disabled)"), "background").is_some_and(|v| paints_signal_red(&v, &red)),
        "a sendable Send is the signal red",
    );
    assert!(
        !all_style_rules(&css).iter().any(|(s, _)| s.starts_with(".send") && s.contains("::before")),
        "the sendable signal is the round button's own fill — S-618's dot is gone (one signal, not two)",
    );
}

/// The consent chip (S-619, CR-204 C8): after consent the lead is ONE line — the chip
/// never wraps, and its long line ellipsises instead of growing a second row.
#[test]
fn chat_consent_chip_holds_to_one_line() {
    let css = strip_comments(&read("src/views/chat/Chat.module.css"));
    assert_eq!(declared(&rule_body(&css, ".statusChip"), "white-space").as_deref(), Some("nowrap"), "the chip never wraps");
    let line = rule_body(&css, ".statusLine");
    assert_eq!(declared(&line, "text-overflow").as_deref(), Some("ellipsis"), "a long line ellipsises");
    assert_eq!(declared(&line, "overflow").as_deref(), Some("hidden"));
    assert_eq!(declared(&line, "min-width").as_deref(), Some("0"), "the line may shrink inside the chip");
}

/// The house's signal hues — the tokens that mean something (danger, in-progress,
/// done) rather than the neutral inks (`--text-1`/`--text-2`, asserted against every
/// surface in `text_and_signal_contrast_meets_wcag_aa_in_both_themes`) and the
/// on-fill inks (`--ink-on-*`, asserted against their hues in `badge_ink_meets_…`).
/// Carrying one of these as `color:` is the decision this module polices.
/// The chat view's stylesheets — `Chat.module.css` and every module beside it
/// (S-622's `CodeBlock.module.css`, S-623's `ThreadList.module.css`) — as
/// `(file name, comment-stripped css)`, ENUMERATED from the directory rather than
/// listed. The chat guards walk all of them: a rule split out of the shared
/// stylesheet, or born beside it, is the same chat UI. When S-623 moved the rail's
/// rename styling into its own module, every chat guard that read only
/// `Chat.module.css` went blind to it — the closed-list failure one level up.
fn chat_stylesheets() -> Vec<(String, String)> {
    let mut files = Vec::new();
    collect_module_css(&ui_dir().join("src/views/chat"), &mut files);
    files.sort();
    let sheets: Vec<(String, String)> = files
        .iter()
        .map(|path| {
            let name = path.file_name().and_then(|n| n.to_str()).expect("a UTF-8 file name").to_string();
            let css = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
            (name, strip_comments(&css))
        })
        .collect();
    assert!(
        sheets.iter().any(|(name, _)| name == "Chat.module.css"),
        "the walk finds the shared chat stylesheet",
    );
    sheets
}

/// Every style rule across the chat stylesheets — rungs included — with its file.
fn chat_style_rules(sheets: &[(String, String)]) -> Vec<(String, String, String)> {
    sheets
        .iter()
        .flat_map(|(file, css)| all_style_rules(css).into_iter().map(move |(sel, body)| (file.clone(), sel, body)))
        .collect()
}

/// Every rule body across the chat stylesheets whose full selector list is exactly
/// `selector`, in source order: the top-level rule, a later duplicate and a rung
/// override alike, so a measurement covers each rule that can win the cascade, not
/// just the first. CSS Modules scope a class to its file, so one name in two chat
/// stylesheets is two different classes — refused here rather than guessed.
fn chat_rules_named(sheets: &[(String, String)], selector: &str) -> Vec<String> {
    let found: Vec<(String, String, String)> =
        chat_style_rules(sheets).into_iter().filter(|(_, sel, _)| sel == selector).collect();
    assert!(!found.is_empty(), "rule `{selector}` not found in any chat stylesheet");
    let file = found[0].0.clone();
    assert!(
        found.iter().all(|(f, _, _)| *f == file),
        "`{selector}` is declared in more than one chat stylesheet — they are different classes, \
         so classify the one this entry means",
    );
    found.into_iter().map(|(_, _, body)| body).collect()
}

/// The fill a selector's own rules paint: the LAST `background: var(--…)` across
/// them (the cascade's winner), or `None` when it paints none of its own.
fn chat_own_fill(sheets: &[(String, String)], selector: &str) -> Option<String> {
    chat_rules_named(sheets, selector).iter().rev().find_map(|body| var_token(body, "background"))
}

/// A `color:` value in a rule body that is not a token. A chat ink must be a
/// `var(--…)` (or a keyword that paints no colour of its own) for the
/// classifications to measure it: a literal escapes every contrast check.
fn literal_color(body: &str) -> Option<String> {
    const KEYWORDS: [&str; 6] = ["inherit", "currentColor", "currentcolor", "transparent", "initial", "unset"];
    declarations_of(body)
        .into_iter()
        .filter(|(name, _)| name == "color")
        .map(|(_, value)| value.trim().to_string())
        .find(|value| !value.starts_with("var(") && !KEYWORDS.contains(&value.as_str()))
}

const SIGNAL_HUES: &[&str] = &[
    "--color-accent",
    "--color-accent-warm",
    "--color-pass",
    "--so-red",
    "--so-orange",
    "--so-green",
    "--so-lime",
    "--so-merlin",
];

/// Every rule in `Chat.module.css` that inks a SIGNAL HUE, and why that is allowed.
/// This list is the coverage contract: the test below fails on any signal-hue
/// `color:` rule missing from it, so a new one cannot be added without a decision.
///
/// A rule reaching for a signal hue has three legitimate resolutions, and only the
/// third leaves the hue in `color:` — which is why this list is short:
/// - **Move it to a fill** — carry the hue as `background:` under an `--ink-on-*`
///   ink with `Badge` chip geometry. That is what HF-1 did to `.activityRunning` /
///   `.activityDone`, and it takes them OUT of this list: they now ink
///   `--ink-on-warm`, and the fill/ink pair is measured by
///   `chat_transcript_text_on_the_card_fill_clears_wcag_aa_in_both_themes`.
/// - **Drop it** — keep `--text-1` and carry the signal on a border/underline, as
///   `.halt, .error`, `.markdown a` and (since S-308) `.railError` do. Also not in
///   this list, for the same reason: `.railError` inked `--color-accent` at 3.55:1 on
///   the dark rail fill, which is text, and text needs 4.5:1.
/// - **Keep it, as a UI affordance** — a glyph that is a graphic signal rather than
///   body text, where the hue need only clear the ≥3:1 non-text floor. The test
///   below measures that floor for each entry against the surface named beside it,
///   in both themes. These are the entries below; each one is a deliberate
///   exception, not a default.
///
/// The surface is the one the glyph is painted on: the rule's OWN `background` when
/// it declares one (the test checks the two agree), else the fill it sits on — the
/// view card's `--surface-1` for the transcript (S-308).
const SIGNAL_INK_COVERAGE: &[(&str, &str, &str)] = &[
    (".streaming::after", "--surface-1", "UiAffordance: streaming caret glyph on the card fill"),
];

/// The coverage half of the S-300 invariant, and the guard that would have caught
/// S-301 on the story that introduced it.
///
/// S-300 encoded "a signal hue cannot be ink on the unfilled column" as measurements
/// over a HARDCODED selector list. S-301 then added `.activityRunning`/`.activityDone`
/// to the same stylesheet a story later, inking `--color-pass` at 2.99:1 — and the
/// guard said nothing, because a closed list cannot notice what it does not name.
/// The fix is to invert it: enumerate the stylesheet and require every signal-hue ink
/// to be CLASSIFIED, so the omission fails loudly instead of passing silently.
#[test]
fn every_signal_hue_ink_in_the_chat_stylesheet_is_classified() {
    let sheets = chat_stylesheets();
    // Rungs included: a signal ink re-introduced inside a `@media` block is just as
    // much a signal ink, and a top-level-only walk would never see it. Every chat
    // stylesheet, not only the shared one.
    for (_, selector, body) in chat_style_rules(&sheets) {
        let Some(token) = color_token(&body) else { continue };
        if !SIGNAL_HUES.contains(&token.as_str()) {
            continue;
        }
        assert!(
            SIGNAL_INK_COVERAGE.iter().any(|(sel, _, _)| *sel == selector),
            "`{selector}` inks the signal hue `{token}` but is not classified in \
             `SIGNAL_INK_COVERAGE`. On the view card's fill a signal hue does not clear \
             4.5:1 as text in every theme — move it to a `background:` fill under an \
             `--ink-on-*` ink with `Badge` chip geometry, or drop it and carry the signal \
             on a border/underline. If this rule is a graphic glyph rather than text it may \
             keep the hue as a ≥3:1 UI affordance — then add it below with its surface and \
             reason. Pick one \
             and record it: leaving a rule unclassified is how S-301 shipped at 2.99:1.",
        );
    }
    // …and the contract cannot rot in the other direction either: a classified
    // selector that no longer inks a signal hue is stale and must be dropped, or the
    // list slowly becomes a record of what the stylesheet used to look like.
    let tokens = strip_comments(&read("src/styles/tokens.css"));
    let base = declarations(&block_after(&tokens, ":root"));
    let light = declarations(&block_after(&tokens, ":root[data-theme=\"light\"]"));
    for (selector, surface, note) in SIGNAL_INK_COVERAGE {
        // Every rule that inks it — a later duplicate or a rung override can win the
        // cascade, so each one is measured, not only the first.
        let inks: Vec<String> = chat_rules_named(&sheets, selector).iter().filter_map(|b| color_token(b)).collect();
        assert!(!inks.is_empty(), "`{selector}` is classified ({note}) but declares no `color: var(--…)`");
        // A classified affordance still has to BE one: ≥3:1 against what it is painted
        // on, in both themes. The surface is the rule's own fill when it declares one.
        if let Some(own) = chat_own_fill(&sheets, selector) {
            assert_eq!(
                own, *surface,
                "`{selector}` paints its own `{own}` fill; classify it against that surface",
            );
        }
        for token in &inks {
            assert!(
                SIGNAL_HUES.contains(&token.as_str()),
                "`{selector}` is classified as a signal-hue ink ({note}) but now inks \
                 `{token}`, which is not a signal hue — drop it from `SIGNAL_INK_COVERAGE`",
            );
            for (theme, map) in [("dark", theme_map(&base, None)), ("light", theme_map(&base, Some(&light)))] {
                let c = contrast(token_rgb(token, &map, &base), token_rgb(surface, &map, &base));
                assert!(
                    c >= 3.0,
                    "{theme}: `{selector}` ({note}) inks {token} at {c:.2}:1 on {surface}, below the \
                     3:1 non-text floor a UI affordance must clear",
                );
            }
        }
    }
}

/// Every top-level rule in the chat stylesheet — rungs included — that inks a
/// NEUTRAL colour (`--text-1`/`--text-2`/`--ink-on-*`; a `SIGNAL_HUES` member is
/// `SIGNAL_INK_COVERAGE`'s contract, above), together with the fill it is measured
/// against. This is what list (a) used to be, in
/// `chat_transcript_text_on_the_card_fill_clears_wcag_aa_in_both_themes` — a
/// hardcoded `[".halt, .error", ".markdown a", …]` array that could say nothing
/// about a selector it did not name. INVERTED (S-311): the test below fails on any
/// neutral-ink rule missing from here, so a new one — `.markdown th`/`.markdown
/// td`, this same story's table treatment, among them — cannot ink text past the
/// AA floor unmeasured, the exact closed-list shape that let a signal hue ship at
/// 2.99:1 one story after this file's fill/ink split was first written.
///
/// Every surface here is DERIVED (`var_token` off the selector's own rule, or its
/// container's), never a hand-typed literal: `--surface-1` and `--surface-2` are
/// the identical hex in the light theme (`#ffffff`) but diverge in dark, so an
/// entry that reads the wrong container would still pass in light and only fail in
/// dark — exactly the kind of mistake a derived value catches and a literal would
/// not. Two selectors are deliberately absent: `.activityRunning`/`.activityDone`
/// ink `--ink-on-warm`, a neutral token, but are already measured — against their
/// OWN fill, not one of these shared ones — by list (b) two tests above.
#[test]
fn every_neutral_ink_in_the_chat_stylesheet_is_classified_and_clears_wcag_aa() {
    let sheets = chat_stylesheets();
    let card = strip_comments(&read("src/components/Card.module.css"));
    let callout = strip_comments(&read("src/components/Callout.module.css"));
    let tokens = strip_comments(&read("src/styles/tokens.css"));
    let base = declarations(&block_after(&tokens, ":root"));
    let light = declarations(&block_after(&tokens, ":root[data-theme=\"light\"]"));

    let card_fill = var_token(&rule_body(&card, ".card"), "background")
        .expect("the shared `.card` declares a `background: var(--…)` fill");
    // The rail renders no card since S-619 (CR-204 L6): it sits on the PAGE surface,
    // the fill `body` paints (`styles/base.css`), and declares none of its own.
    let base_css = strip_comments(&read("src/styles/base.css"));
    let page_fill = var_token(&rule_body(&base_css, "body"), "background")
        .expect("`body` declares a `background: var(--…)` page fill");
    assert_eq!(chat_own_fill(&sheets, ".rail"), None, "the cardless rail declares no fill");
    let rail_fill = page_fill;
    let composer_fill = chat_own_fill(&sheets, ".composer")
        .expect("`.composer` declares a `background: var(--…)` fill");
    let chip_fill = chat_own_fill(&sheets, ".statusChip")
        .expect("`.statusChip` declares a `background: var(--…)` fill");
    let sendable_fill = chat_own_fill(&sheets, ".send:not(:disabled)")
        .expect("`.send:not(:disabled)` declares a `background: var(--…)` fill");
    let trash_hover_fill = chat_own_fill(&sheets, ".threadDelete:hover, .threadDelete:focus-visible")
        .expect("the hovered trash declares a `background: var(--…)` fill");
    let raised_fill = chat_own_fill(&sheets, ".codeHeader")
        .expect("`.codeHeader` declares a `background: var(--…)` fill");
    // S-618 (CR-204 L7): neither is the signal red any more — a text-sized control is
    // never a red fill. The destructive confirm is an INVERTED neutral (`--text-1`
    // fill), and Send a raised neutral; each is still measured against ITS OWN fill.
    // The delete confirmation is its own raised panel, and Cancel a button with its
    // own fill inside it — neither sits on the page the rail does.
    let confirm_fill = chat_own_fill(&sheets, ".threadConfirm")
        .expect("`.threadConfirm` declares a `background: var(--…)` fill");
    let cancel_fill = chat_own_fill(&sheets, ".threadConfirmCancel")
        .expect("`.threadConfirmCancel` declares a `background: var(--…)` fill");
    let stop_fill = chat_own_fill(&sheets, ".stop").expect("`.stop` declares a `background: var(--…)` fill");
    // S-623: the inline rename field paints its own fill inside the row pill.
    let rename_fill = chat_own_fill(&sheets, ".renameInput")
        .expect("`.renameInput` declares a `background: var(--…)` fill");
    let delete_fill = chat_own_fill(&sheets, ".threadConfirmDelete")
        .expect("`.threadConfirmDelete` declares a `background: var(--…)` fill");
    let send_fill = chat_own_fill(&sheets, ".send")
        .expect("`.send` declares a `background: var(--…)` fill");
    let scroll_fill = chat_own_fill(&sheets, ".scrollToBottom")
        .expect("`.scrollToBottom` declares a `background: var(--…)` fill");
    // S-625: the preset menu's rows sit on the menu's fill, or on the raised fill when
    // checked or hovered; the chip and its clear control on the chip's own fill.
    let menu_fill = chat_own_fill(&sheets, ".presetMenu")
        .expect("`.presetMenu` declares a `background: var(--…)` fill");
    let row_fill = chat_own_fill(&sheets, ".presetItem[aria-checked=\"true\"]")
        .expect("the checked preset row declares a `background: var(--…)` fill");
    let preset_chip_fill = chat_own_fill(&sheets, ".presetChip")
        .expect("`.presetChip` declares a `background: var(--…)` fill");
    let viewport_fill = chat_own_fill(&sheets, ".mermaidViewport")
        .expect("`.mermaidViewport` declares a `background: var(--…)` fill");
    let callout_fill = var_token(&rule_body(&callout, ".callout"), "background")
        .expect("the shared `.callout` declares a `background: var(--…)` fill");
    // S-621: a welcome chip is a pill with a fill of its own — enabled, and greyed
    // out before consent. The greeting line sits on the card, like `.empty`.
    let welcome_chip_fill = chat_own_fill(&sheets, ".chip")
        .expect("`.chip` declares a `background: var(--…)` fill");
    let welcome_chip_disabled_fill = chat_own_fill(&sheets, ".chip:disabled")
        .expect("`.chip:disabled` declares a `background: var(--…)` fill");

    // The classification: selector → the fill token it visually sits on, traced
    // against the component tree (`ChatView.tsx`, `MermaidBlock.tsx`) rather than
    // assumed — a selector that declares its own `background:` sits on that (noted
    // "own fill" below); every other one is read off its container's.
    let coverage: Vec<(&str, &str)> = vec![
        (".halt, .error", card_fill.as_str()),
        (".markdown a", card_fill.as_str()),
        (".railEmpty, .railError", rail_fill.as_str()),
        (".railError", rail_fill.as_str()),
        // S-619: the toggle is a ghost on the page; its hover tint and the rows'
        // pill tints are translucent, measured composited in
        // `chat_rail_is_cardless_with_a_red_marker_and_a_hidden_idle_trash`.
        (".railToggle", rail_fill.as_str()),
        (".railToggle:hover", rail_fill.as_str()),
        (".threadItem", rail_fill.as_str()),
        (".threadDelete", rail_fill.as_str()),
        (".threadDelete:hover, .threadDelete:focus-visible", trash_hover_fill.as_str()), // own fill
        (".threadConfirmText", confirm_fill.as_str()), // `.threadConfirm`'s own fill
        (".threadConfirmDelete", delete_fill.as_str()), // own fill
        (".threadConfirmCancel", cancel_fill.as_str()), // own fill
        (".providerLine", callout_fill.as_str()),       // the shared `Callout`'s own fill
        (".budgetLine", callout_fill.as_str()),         // the consent `Callout`, and the chip's band (== --surface-2)
        (".statusChip", chip_fill.as_str()),            // own fill (S-619 consent chip)
        (".statusLabel", chip_fill.as_str()),
        (".statusLine", chip_fill.as_str()),
        (".empty", card_fill.as_str()),
        (".welcomeTitle", card_fill.as_str()),
        (".chip", welcome_chip_fill.as_str()), // own fill
        (".chip:disabled", welcome_chip_disabled_fill.as_str()), // own fill
        (".activitySummary", card_fill.as_str()),
        (".activityLabel", card_fill.as_str()),
        (".activityMeta", card_fill.as_str()),
        (".activityCaption", card_fill.as_str()),
        (".planSteps", card_fill.as_str()),
        (".activityRole", card_fill.as_str()),
        (".activityInstruction", card_fill.as_str()),
        (".activityResult", card_fill.as_str()),
        (".activityEmpty", card_fill.as_str()),
        (".working", card_fill.as_str()),
        (".markdown th", raised_fill.as_str()),
        (".markdown td", raised_fill.as_str()),
        (".codeLang", raised_fill.as_str()),
        (".codeCopy, .mermaidControl", raised_fill.as_str()),
        (".codeCopy:hover:not(:disabled), .mermaidControl:hover:not(:disabled)", raised_fill.as_str()),
        (".mermaidZoom", raised_fill.as_str()),
        (".mermaidScale :global(.mermaid:not([data-processed=\"true\"]))", viewport_fill.as_str()),
        (".mermaidFallback", viewport_fill.as_str()), // sibling of `.mermaidScale` INSIDE `.mermaidViewport`, not `.codeHeader`
        (".action", card_fill.as_str()),
        (".action:hover", card_fill.as_str()),
        // S-620: the icon action bar. Its hover/focus fill is a translucent tint,
        // measured composited in `chat_turns_are_bubbles_with_an_icon_action_bar_and_a_scroll_control`.
        (".actionIcon", card_fill.as_str()),
        (".actionIcon:hover, .actionIcon:focus-visible", card_fill.as_str()),
        (".actionIcon[data-copied]", card_fill.as_str()),
        (".scrollToBottom", scroll_fill.as_str()), // own fill
        (".composerHint", card_fill.as_str()),     // under the pill, on the card
        (".input", composer_fill.as_str()), // no fill of its own: the pill's (S-619)
        (".composerMenu", composer_fill.as_str()),
        (".send", send_fill.as_str()),   // own fill: neutral while there is nothing to send
        (".send:not(:disabled)", sendable_fill.as_str()), // own fill: the signal red, white ink
        (".stop", stop_fill.as_str()), // own fill
        // S-625: a row is measured on both fills it can sit on — the menu's own, and
        // the checked / hovered row's.
        (".presetItem", menu_fill.as_str()),
        (".presetItem", row_fill.as_str()),
        (".presetDescription", menu_fill.as_str()),
        (".presetDescription", row_fill.as_str()),
        (".presetChip", preset_chip_fill.as_str()), // own fill
        (".presetChipClear", preset_chip_fill.as_str()),
        (".presetChipClear:hover", preset_chip_fill.as_str()),
        // S-623 (`ThreadList.module.css`): the rename field, and its refusal note
        // under the row on the page.
        (".renameInput", rename_fill.as_str()), // own fill
        (".renameRefused", rail_fill.as_str()),
    ];

    // Every top-level rule (rungs included) that inks a NEUTRAL colour must be
    // classified above. The one deliberate exemption is SVG-internal Mermaid node
    // theming (`.node text`/`.label`/`.cluster-label`): diagram content coloured by
    // the vendored render once it mounts, not chat body text — the PRE-render
    // fallback text (`:not([data-processed])`, above) is real UI text and stays in
    // scope.
    const MERMAID_SVG_LABEL_SELECTOR: &str = ".mermaidScale :global(.mermaid .label), \
        .mermaidScale :global(.mermaid .label text), .mermaidScale :global(.mermaid .label span), \
        .mermaidScale :global(.mermaid .cluster-label text), .mermaidScale :global(.mermaid .cluster-label span)";
    for (file, selector, body) in chat_style_rules(&sheets) {
        // An ink that is not a token cannot be measured by any list here, so it is
        // refused outright rather than skipped.
        if let Some(literal) = literal_color(&body) {
            panic!("`{selector}` ({file}) inks the literal `{literal}` — chat inks are `var(--…)` tokens");
        }
        if file == "Chat.module.css"
            && (selector == MERMAID_SVG_LABEL_SELECTOR || selector == ".activityRunning" || selector == ".activityDone")
        {
            continue;
        }
        let Some(token) = color_token(&body) else { continue };
        if SIGNAL_HUES.contains(&token.as_str()) {
            continue;
        }
        // The highlighter's `--syntax-*` palette in its own module is measured on the
        // code-block fill by `syntax_tokens_clear_wcag_aa_on_the_code_block_fill_in_every_theme_block`.
        if file == "CodeBlock.module.css" && token.starts_with("--syntax-") {
            continue;
        }
        assert!(
            coverage.iter().any(|(sel, _)| *sel == selector),
            "`{selector}` ({file}) inks the neutral colour `{token}` but is not classified in \
             `every_neutral_ink_in_the_chat_stylesheet_is_classified_and_clears_wcag_aa`'s \
             `coverage` — name the fill it sits on so a new rule cannot ship its text past \
             the AA floor unmeasured",
        );
    }

    // …and the reverse: a classified selector that no longer declares a `color:` —
    // or now inks a signal hue instead — is stale and must be dropped or moved.
    for (selector, surface) in &coverage {
        // Every rule that inks it — a later duplicate or a rung override can win the
        // cascade, so each one is measured, not only the first.
        let inks: Vec<String> = chat_rules_named(&sheets, selector).iter().filter_map(|b| color_token(b)).collect();
        assert!(!inks.is_empty(), "`{selector}` is classified but declares no `color: var(--…)`");
        // A rule that paints its own OPAQUE fill is measured on that fill: the entry
        // cannot name the surface around it instead (a translucent tint is measured
        // composited over its surface by the tests named beside those entries).
        if let Some(own) = chat_own_fill(&sheets, selector) {
            let opaque = [theme_map(&base, None), theme_map(&base, Some(&light))]
                .iter()
                .all(|map| token_rgba(&own, map, &base).1 >= 1.0);
            if opaque {
                assert_eq!(own, *surface, "`{selector}` paints its own `{own}` fill; classify it against that surface");
            }
        }
        for token in &inks {
            assert!(
                !SIGNAL_HUES.contains(&token.as_str()),
                "`{selector}` is classified as a neutral ink but now inks the signal hue \
                 `{token}` — move it to `SIGNAL_INK_COVERAGE` instead",
            );
            for (theme, map) in [("dark", theme_map(&base, None)), ("light", theme_map(&base, Some(&light)))] {
                let c = contrast(token_rgb(token, &map, &base), token_rgb(surface, &map, &base));
                assert!(
                    c >= 4.5,
                    "{theme}: `{selector}` ink ({token}) is {c:.2}:1 on {surface}, below the 4.5:1 \
                     AA body minimum",
                );
            }
        }
    }
}

// ── The shell header's progressive disclosure (S-317, FR-UI-34, CR-089/CR-092) ──

/// The one rung of the disclosure ladder still standing, in CSS px: the complement
/// of the tier frontend-design §7 documents (tablet ≥768px), and the shell's
/// existing collapse point (`AppShell.module.css`, `Chat.module.css`).
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
         and the Chat rail already collapse at (frontend-design §7)",
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

/// The value a rule body finally declares for `property` — the LAST declaration of
/// it, the one that wins the cascade within the rule — normalised as
/// `declarations_of` does, or `None` when the body never declares it. Unlike
/// `var_token`, it returns the raw value rather than unwrapping a `var(--…)`.
fn declared(body: &str, property: &str) -> Option<String> {
    declarations_of(body).into_iter().rev().find(|(n, _)| n == property).map(|(_, v)| v)
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

/// The `--token` named by the first `color: var(--token)` declaration in a rule body.
fn color_token(body: &str) -> Option<String> {
    var_token(body, "color")
}

/// The `--token` named by the first `<property>: var(--token)` declaration in a rule
/// body. A declaration that is not a `var(…)` (a keyword, or a literal) is skipped
/// rather than ending the scan, so a rule that declares a fallback before its token
/// — `color: inherit; color: var(--text-1)` — still resolves.
///
/// The value must be a single unadorned `var(…)`: a SHORTHAND that merely contains
/// one (`background: var(--x) no-repeat`) is skipped, not parsed. That never came up
/// while this only read `color`, which is never a shorthand, but `background` is —
/// so a caller that starts matching compound backgrounds must widen this first. It
/// fails loudly rather than silently (both call sites `panic!` on `None`), so the
/// failure mode is a confusing message, never a wrong token measured as if right.
fn var_token(body: &str, property: &str) -> Option<String> {
    for decl in body.split(';') {
        let Some((name, value)) = decl.split_once(':') else { continue };
        if name.trim() != property {
            continue;
        }
        let Some(inner) = value.trim().strip_prefix("var(").and_then(|v| v.strip_suffix(')'))
        else {
            continue;
        };
        return inner.split(',').next().map(|n| n.trim().to_string());
    }
    None
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

// ── Syntax-highlight palette (S-622, CR-204 C4, FR-UI-47) ────────────────────

/// Every syntax colour the code-block module consumes is a `--syntax-*` token
/// declared in ALL FOUR theme blocks, and each clears WCAG 2.1 AA (4.5:1, body
/// text) on the code-block fill in every one of them. The fill is READ from the
/// block chrome rule in `Chat.module.css` rather than assumed, so a later restyle
/// of the block re-measures the palette instead of passing on a stale pairing.
/// No syntax colour is the signal red, which stays signal-only (ADR-44).
#[test]
fn syntax_tokens_clear_wcag_aa_on_the_code_block_fill_in_every_theme_block() {
    let tokens = strip_comments(&read("src/styles/tokens.css"));
    let base = declarations(&block_after(&tokens, ":root"));
    let blocks = [
        (":root (dark default)", theme_map(&base, None)),
        (":root[data-theme=\"dark\"]", theme_map(&base, Some(&declarations(&block_after(&tokens, ":root[data-theme=\"dark\"]"))))),
        (":root[data-theme=\"light\"]", theme_map(&base, Some(&declarations(&block_after(&tokens, ":root[data-theme=\"light\"]"))))),
        (
            "@media (prefers-color-scheme: light) :root:not([data-theme])",
            theme_map(&base, Some(&declarations(&block_after(&tokens, ":root:not([data-theme])")))),
        ),
    ];

    // The fill: whatever `.codeBlock`'s own background token is.
    let chat = strip_comments(&read("src/views/chat/Chat.module.css"));
    let fill = aggregated_rules(&chat)
        .into_iter()
        .filter(|(sel, _)| sel == ".codeBlock")
        .find_map(|(_, body)| var_token(&body, "background"))
        .expect("`.codeBlock` declares its background as a token");

    // The palette: every token the code-block module consumes.
    let module = strip_comments(&read("src/views/chat/CodeBlock.module.css"));
    let mut consumed: Vec<String> = module
        .match_indices("var(--")
        .map(|(at, _)| {
            let rest = &module[at + "var(".len()..];
            rest[..rest.find([')', ',']).unwrap()].trim().to_string()
        })
        .filter(|name| name.starts_with("--syntax-"))
        .collect();
    consumed.sort();
    consumed.dedup();
    assert_eq!(consumed.len(), 7, "the module colours from the seven syntax tokens: {consumed:?}");
    let declared_once_per_block: Vec<String> = base.keys().filter(|k| k.starts_with("--syntax-")).cloned().collect();
    assert_eq!(declared_once_per_block, consumed, "tokens.css declares exactly the syntax tokens the module consumes");
    for name in &consumed {
        assert_eq!(tokens.matches(&format!("{name}:")).count(), 4, "`{name}` is declared once per theme block");
    }

    let red = hex_rgb(base.get("--so-red").expect("--so-red"));
    for (block, map) in &blocks {
        let surface = token_rgb(&fill, map, &base);
        for name in &consumed {
            let ink = token_rgb(name, map, &base);
            let ratio = contrast(ink, surface);
            println!("{block}: {name} on {fill} = {ratio:.2}:1");
            assert!(ratio >= 4.5, "{block}: {name} on the code-block fill {fill} is {ratio:.2}:1, below 4.5:1 AA");
            assert_ne!(ink, red, "{block}: {name} is the signal red, which stays signal-only");
        }
    }
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
