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

// ── 8. The chat transcript is one aligned conversation column (S-300) ─────────
//
// [FR-UI-31] restyled the transcript to the base assistant-ui grammar. The
// invariants below are pure CSS, and the SPA's Vitest run disables CSS entirely
// (`css: false` — CSS Modules resolve to empty objects, so class names never
// reach the jsdom DOM), which makes the authored stylesheet the only place the
// contract can be checked without a headless browser — the same reasoning that
// puts the token/contrast checks above in this file.

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

#[test]
fn chat_roles_share_one_centered_readable_measure() {
    let css = strip_comments(&read("src/views/chat/Chat.module.css"));
    // The measure is declared once, on the thread root, and inherited by the column.
    assert!(
        rule_body(&css, ".threadRoot").contains("--chat-measure:"),
        "the shared conversation measure is declared on .threadRoot",
    );
    let column = rule_body(&css, ".empty, .user, .assistant");
    assert!(
        column.contains("max-width: var(--chat-measure)") && column.contains("margin-inline: auto"),
        "the empty hint and BOTH roles are centred on the shared measure",
    );
    assert!(
        rule_body(&css, ".composer").contains("max-width: var(--chat-measure)"),
        "the composer rides the same measure, so the column is one continuous surface",
    );
    // Neither role escapes the column with its own alignment; the user bubble is
    // right-aligned INSIDE the column, not against the viewport.
    let user = rule_body(&css, ".user");
    assert!(!user.contains("align-self"), "the user turn aligns within the column, not against it");
    assert!(!rule_body(&css, ".assistant").contains("align-self"));
    assert!(user.contains("justify-content: flex-end"), "the user bubble hugs the column's right edge");
    assert!(
        rule_body(&css, ".userBubble").contains("background: var(--surface-2)"),
        "the bubble treatment moved to the inner .userBubble element",
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
fn chat_transcript_text_on_the_page_surface_clears_wcag_aa_in_both_themes() {
    // The realigned turn has no fill of its own, so its text renders directly on
    // `--surface-0`. Any rule that previously leaned on the card's `--surface-1`
    // must therefore use ink that clears the 4.5:1 AA body minimum THERE — which
    // is why the halt/error notices and the answer links carry the signal hue on a
    // border/underline (a 3:1 UI affordance) rather than in the text colour.
    let css = strip_comments(&read("src/views/chat/Chat.module.css"));
    let tokens = strip_comments(&read("src/styles/tokens.css"));
    let base = declarations(&block_after(&tokens, ":root"));
    let light = declarations(&block_after(&tokens, ":root[data-theme=\"light\"]"));

    // Every rule in the transcript that declares its own ink. The Activity glyphs
    // (S-301) are here because they were the sprint's near-miss: added INSIDE the
    // unfilled column a story after this invariant was established, they took the
    // signal hues as `color:` — `--color-pass` is 2.99:1 on the light page surface,
    // under even the 3:1 non-text floor. A signal hue belongs on a fill or an edge.
    for selector in [".halt, .error", ".markdown a", ".activityRunning", ".activityDone"] {
        let token = color_token(&rule_body(&css, selector))
            .unwrap_or_else(|| panic!("`{selector}` declares a `color: var(--…)`"));
        for (theme, map) in [
            ("dark", theme_map(&base, None)),
            ("light", theme_map(&base, Some(&light))),
        ] {
            let c = contrast(token_rgb(&token, &map, &base), token_rgb("--surface-0", &map, &base));
            assert!(
                c >= 4.5,
                "{theme}: `{selector}` ink ({token}) is {c:.2}:1 on the page surface, below the \
                 4.5:1 AA body minimum — the transcript has no card fill to sit on, so a signal \
                 hue must be carried by a border/underline, not by the text colour",
            );
        }
    }
}

// ── helpers ──────────────────────────────────────────────────────────────────

/// Every top-level `selector { body }` pair in a stylesheet, selectors normalised
/// to single-spaced form (`".empty,\n.user"` → `".empty, .user"`). Comments must
/// already be stripped. At-rule bodies (`@media`, `@keyframes`) are returned under
/// the at-rule's own "selector" and are not descended into — the rules asserted on
/// above are all top-level.
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

/// The `--token` named by the first `color: var(--token)` declaration in a rule
/// body. A `color:` declaration that is not a `var(…)` (a keyword, or a literal)
/// is skipped rather than ending the scan, so a rule that declares a fallback
/// before its token — `color: inherit; color: var(--text-1)` — still resolves.
fn color_token(body: &str) -> Option<String> {
    for decl in body.split(';') {
        let Some((name, value)) = decl.split_once(':') else { continue };
        if name.trim() != "color" {
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
