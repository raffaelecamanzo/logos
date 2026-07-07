//! The embedded SPA bundle (CR-049, FR-UI-22, ADR-43).
//!
//! The Vite + React project under `web/ui/` builds to `web/ui/dist/` (hashed
//! JS/CSS, `index.html`, fonts) — a **build-time-only** toolchain (Node/npm/Vite
//! run at build time only; the served binary needs no Node runtime and opens no
//! socket to deliver assets). [`Spa`] embeds that directory into the binary with
//! `rust-embed`, the directory-embedding successor to the retired hand-maintained
//! `include_bytes!` asset table — it handles the hashed filenames a
//! Vite build emits without a per-file registry edit.
//!
//! # Carve-out invariants preserved (ADR-43, NFR-SE-01)
//! - **Offline.** `rust-embed` is a pure compile-time embedder — no socket/HTTP
//!   surface — and is compiled only under `--features ui`, so it never enters
//!   the default-feature dependency tree the no-network fitness test guards.
//! - **CSP-clean.** The Vite build emits external hashed `<script src>`/`<link>`
//!   only (no inline `<script>`/`<style>`, no `unsafe-eval`); the bundle names no
//!   external origin. [`tests/spa_bundle.rs`](../../tests/spa_bundle.rs) asserts
//!   this over whatever is embedded.
//! - **Node-free `cargo build`.** A committed placeholder `web/ui/dist/index.html`
//!   keeps the embedded folder non-empty so a plain `cargo build --features ui`
//!   compiles with no Node present; a real CI/local Vite build overwrites it with
//!   the hashed bundle.
//!
//! # The sole asset path (CR-049, FR-UI-22, ADR-43, S-192)
//! Since the decommission this embedded bundle is the **only** asset source: the
//! server-rendered views and their legacy `include_bytes!` table were removed, so
//! the SPA's hashed `/assets/*` files (and root-level assets like `theme-init.js`)
//! resolve solely through the history fallback into [`asset`], served byte-verbatim
//! with no runtime socket or egress.

use std::borrow::Cow;

use rust_embed::RustEmbed;

/// The embedded Vite + React build output (`web/ui/dist/`). Keys are paths
/// relative to `dist/` (`index.html`, `assets/index-<hash>.js`, …).
#[derive(RustEmbed)]
#[folder = "ui/dist/"]
pub struct Spa;

/// The content type for an embedded SPA file, by extension — each text type with
/// an explicit charset so browsers never sniff. Unknown extensions fall back to
/// `application/octet-stream` (served verbatim, never executed).
pub fn content_type(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("");
    match ext {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml; charset=utf-8",
        "png" => "image/png",
        "webp" => "image/webp",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// Look up an embedded SPA file by its `dist/`-relative path, returning its
/// content type and bytes, or `None` when absent. The bytes are byte-verbatim
/// from the binary — no filesystem read at runtime, no egress.
pub fn asset(path: &str) -> Option<(&'static str, Cow<'static, [u8]>)> {
    Spa::get(path).map(|file| (content_type(path), file.data))
}

/// The embedded served-shell document (`dist/index.html`) — the SPA entrypoint
/// the [web-surface] serves at `/`. Always present: a committed placeholder
/// guarantees it. Prefer [`served_shell`], which injects the per-session intent
/// token before serving; this raw accessor backs the bundle fitness tests.
///
/// [web-surface]: ../../../docs/specs/architecture/components/web-surface.md
pub fn index_html() -> Option<Cow<'static, [u8]>> {
    Spa::get("index.html").map(|file| file.data)
}

/// The served SPA shell (S-185, CR-049, FR-UI-22): the embedded [`index_html`]
/// with the per-session intent token ([`IntentToken`](crate::IntentToken))
/// injected as a `<meta name="logos-intent">` tag the SPA reads once at startup
/// and echoes in the `x-logos-intent` header on every mutating request
/// ([NFR-SE-06], [ADR-31]). Returns `None` only if the bundle is somehow not
/// embedded — a committed placeholder makes that unreachable in practice.
///
/// The token is HTML-escaped at the boundary ([`crate::components::escape`]) —
/// defense in depth, though it is 64 hex chars by construction. The tag is inert
/// markup (no inline script/style), so the byte-identical self-only CSP is
/// unaffected; it is inserted immediately before `</head>` so it parses before the
/// module entry. The masked chat key is never placed here (it stays a separate
/// read-model field, [NFR-SE-07]).
///
/// [NFR-SE-06]: ../../../docs/specs/requirements/NFR-SE-06.md
/// [ADR-31]: ../../../docs/specs/architecture/decisions/ADR-31.md
/// [NFR-SE-07]: ../../../docs/specs/requirements/NFR-SE-07.md
pub fn served_shell(intent: &str) -> Option<String> {
    let bytes = index_html()?;
    let html = String::from_utf8_lossy(&bytes);
    let tag = format!(
        "<meta name=\"logos-intent\" content=\"{}\">",
        crate::components::escape(intent)
    );
    // Insert before the first `</head>` (both the committed placeholder and a real
    // Vite build carry one). Defensively prepend if no head close is present, so
    // the token is delivered no matter the shell's exact shape.
    let injected = match html.find("</head>") {
        Some(idx) => {
            let mut out = String::with_capacity(html.len() + tag.len());
            out.push_str(&html[..idx]);
            out.push_str(&tag);
            out.push_str(&html[idx..]);
            out
        }
        None => format!("{tag}{html}"),
    };
    Some(injected)
}

/// The absolute `/assets/*.{js,css}` entrypoints a shell document references —
/// read from `src="…"` / `href="…"` **attributes** only. A Vite build stamps these
/// with content hashes (`/assets/index-<hash>.js`); the committed placeholder carries
/// no such reference. Pure over the HTML — the embed presence check lives in
/// [`missing_shell_assets`].
///
/// Attribute-scoped (not a bare substring scan) so prose that merely mentions the
/// path — e.g. the `index.html` comment "Vite rewrites this to the hashed
/// `/assets/*.js` bundle" — is never mistaken for a live reference. Mirrors the
/// `shell_absolute_refs` helper in `tests/spa_shell.rs`.
pub fn shell_asset_refs(html: &str) -> Vec<String> {
    let mut refs = Vec::new();
    for attr in ["src=\"", "href=\""] {
        let mut from = 0;
        while let Some(rel) = html[from..].find(attr) {
            let start = from + rel + attr.len();
            let end = html[start..].find('"').map(|e| start + e).unwrap_or(html.len());
            let value = &html[start..end];
            if value.starts_with("/assets/") && (value.ends_with(".js") || value.ends_with(".css")) {
                refs.push(value.to_string());
            }
            from = end;
        }
    }
    refs
}

/// The `/assets/*.{js,css}` references in `html` for which `is_present` reports
/// false — i.e. the shell names assets this binary does not carry. Pure; the
/// presence check is injected so it is unit-testable without touching the embed.
/// `is_present` receives the `dist/`-relative key (leading `/` stripped).
fn missing_refs(html: &str, is_present: impl Fn(&str) -> bool) -> Vec<String> {
    shell_asset_refs(html)
        .into_iter()
        .filter(|href| !is_present(href.trim_start_matches('/')))
        .collect()
}

/// The hashed entrypoints the embedded shell references but does **not** embed —
/// the signature of a binary built without a matching `npm run build` (e.g. the
/// frozen-hash placeholder shipped beside a differently-hashed real build, or the
/// placeholder embedded alone). Empty when the bundle is internally consistent.
///
/// This is the fix for the recurring white-page trap (CR-049): without it, such a
/// binary serves the shell `200` while the browser's `/assets/index-<hash>.js`
/// fetch `404`s, leaving a blank page with no explanation. The serve-time guard in
/// [`crate::render_shell`] turns a non-empty result into a loud diagnostic instead.
pub fn missing_shell_assets() -> Vec<String> {
    match index_html() {
        Some(bytes) => {
            let html = String::from_utf8_lossy(&bytes);
            missing_refs(&html, |key| Spa::get(key).is_some())
        }
        None => Vec::new(),
    }
}

/// A CSP-clean diagnostic document served in place of a white page when the
/// embedded bundle is inconsistent ([`missing_shell_assets`] non-empty). It names
/// the missing assets and the exact rebuild recipe. No inline `<script>`/`<style>`
/// (the self-only CSP is unchanged); the message lives in inert body markup.
pub fn inconsistent_bundle_page(missing: &[String]) -> String {
    let items: String = missing
        .iter()
        .map(|m| format!("<li><code>{}</code></li>", crate::components::escape(m)))
        .collect();
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>Logos — UI bundle not built</title></head><body>\
<h1>Logos UI bundle is inconsistent</h1>\
<p>This binary was built <strong>without a matching <code>npm run build</code></strong>: \
the embedded shell references assets that are not in the binary:</p>\
<ul>{items}</ul>\
<p>Rebuild the SPA and reinstall so the shell and its assets are embedded as a \
consistent pair:</p>\
<pre>cd web/ui &amp;&amp; npm run build\n\
touch web/src/spa.rs   # force rust-embed to re-read dist/\n\
cargo install --path cli --root ~/.logos-bin/&lt;version&gt; --locked --features ui --force</pre>\
<p>The API remains available at <code>/api/v1</code>.</p></body></html>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_index_html_is_always_embedded() {
        // The committed placeholder (or a real build's index.html) is what keeps
        // `cargo build --features ui` Node-free — it must always be embedded.
        assert!(index_html().is_some(), "web/ui/dist/index.html must be embedded");
    }

    #[test]
    fn index_html_is_served_as_html() {
        let (mime, bytes) = asset("index.html").expect("index.html embedded");
        assert_eq!(mime, "text/html; charset=utf-8");
        assert!(!bytes.is_empty(), "index.html is non-empty");
    }

    #[test]
    fn content_type_maps_the_bundle_extensions() {
        // The extensions a Vite build emits — each served with the right type so
        // the browser parses the module/stylesheet correctly under the CSP.
        assert_eq!(content_type("assets/index-abc123.js"), "text/javascript; charset=utf-8");
        assert_eq!(content_type("assets/index-abc123.css"), "text/css; charset=utf-8");
        assert_eq!(content_type("assets/inter-400-abc.woff2"), "font/woff2");
        assert_eq!(content_type("favicon.svg"), "image/svg+xml; charset=utf-8");
        // An unknown/extension-less path is served verbatim, never executed.
        assert_eq!(content_type("assets/data"), "application/octet-stream");
    }

    #[test]
    fn a_missing_asset_is_none() {
        assert!(asset("assets/does-not-exist-xyz.js").is_none());
    }

    #[test]
    fn served_shell_injects_the_intent_meta_tag() {
        // The per-session token is delivered to the SPA via a `<meta>` tag in the
        // served shell (NFR-SE-06, ADR-31) — the SPA reads it once at startup.
        let html = served_shell("deadbeefcafef00d").expect("shell is embedded");
        assert!(
            html.contains("<meta name=\"logos-intent\" content=\"deadbeefcafef00d\">"),
            "the served shell carries the intent token as a meta tag: {html}",
        );
        // Injected inside <head>, before the close.
        let meta = html.find("logos-intent").expect("meta present");
        let head_close = html.find("</head>").expect("head close present");
        assert!(meta < head_close, "the intent meta is inside <head>");
    }

    #[test]
    fn served_shell_stays_csp_clean() {
        // Injecting the token is inert markup — it must not introduce an inline
        // <script> or <style> that would breach the byte-identical self-only CSP
        // (FR-UI-22, ADR-44). The bundle fitness test (tests/spa_bundle.rs) guards
        // the embedded bytes; this guards the runtime injection over the shell.
        let html = served_shell("0011223344556677").expect("shell is embedded");
        assert!(!html.contains("<script>"), "no inline <script> in the served shell");
        assert!(!html.contains("<style>"), "no inline <style> in the served shell");
    }

    #[test]
    fn served_shell_escapes_the_token_into_the_attribute() {
        // The token is read-model-trusted hex, but the boundary escapes anyway
        // (defense in depth) so a stray quote can never break out of the attribute.
        let html = served_shell("a\"b").expect("shell is embedded");
        assert!(html.contains("content=\"a&quot;b\">"), "the token is attribute-escaped: {html}");
        assert!(!html.contains("content=\"a\"b\""), "no unescaped quote breaks the attribute");
    }

    #[test]
    fn shell_asset_refs_extracts_hashed_js_and_css_only() {
        let html = "<script type=\"module\" src=\"/assets/index-ABC12345.js\"></script>\
<link rel=\"stylesheet\" href=\"/assets/index-DEF67890.css\">\
<link rel=\"icon\" href=\"/favicon.svg\">\
<script src=\"/theme-init.js\"></script>";
        let refs = shell_asset_refs(html);
        assert_eq!(
            refs,
            vec![
                "/assets/index-ABC12345.js".to_string(),
                "/assets/index-DEF67890.css".to_string(),
            ],
            "only the hashed /assets/*.js|css entrypoints are extracted (not favicon or root theme-init)"
        );
    }

    #[test]
    fn shell_asset_refs_ignores_paths_mentioned_only_in_comments() {
        // Regression: a bare substring scan matched `/assets/*.js` inside the
        // index.html comment "Vite rewrites this to the hashed /assets/*.js bundle",
        // producing a phantom "missing asset" and a false 503. Attribute-scoping
        // fixes it — only real src=/href= references count.
        let html = "<!-- Vite rewrites this to the hashed /assets/*.js bundle at build time. -->\
<script src=\"/assets/index-REAL01.js\"></script>";
        assert_eq!(shell_asset_refs(html), vec!["/assets/index-REAL01.js".to_string()]);
    }

    #[test]
    fn missing_refs_flags_only_the_absent_assets() {
        let html = "<script src=\"/assets/present.js\"></script>\
<link href=\"/assets/absent.css\">";
        // Presence oracle: only `assets/present.js` is embedded.
        let missing = missing_refs(html, |key| key == "assets/present.js");
        assert_eq!(missing, vec!["/assets/absent.css".to_string()]);
    }

    #[test]
    fn missing_refs_empty_when_all_present() {
        let html = "<script src=\"/assets/a.js\"></script><link href=\"/assets/b.css\">";
        assert!(missing_refs(html, |_| true).is_empty());
    }

    #[test]
    fn inconsistent_bundle_page_names_the_gaps_and_is_csp_clean() {
        let page = inconsistent_bundle_page(&["/assets/index-STALE99.js".to_string()]);
        assert!(page.contains("/assets/index-STALE99.js"), "names the missing asset");
        assert!(page.contains("npm run build"), "gives the rebuild recipe");
        // No inline <script>/<style> — the self-only CSP stays byte-identical.
        assert!(!page.contains("<script"), "no inline script in the diagnostic page");
        assert!(!page.contains("<style"), "no inline style in the diagnostic page");
    }

    #[test]
    fn the_embedded_bundle_is_self_consistent() {
        // Guards against ever shipping a shell that references an asset the binary
        // does not embed — the recurring CR-049 white-page trap. With only the
        // committed placeholder embedded (no real `npm run build` at test-compile
        // time) this is EXPECTED to be non-empty; the assertion holds precisely
        // when a consistent bundle was built. We therefore assert the invariant
        // the guard relies on: every reported gap is genuinely absent from Spa.
        for gap in missing_shell_assets() {
            assert!(
                Spa::get(gap.trim_start_matches('/')).is_none(),
                "a reported missing asset must truly be absent from the embed: {gap}"
            );
        }
    }
}
