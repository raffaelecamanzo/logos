//! The web surface's HTML-escaping boundary (CR-012, FR-UI-02).
//!
//! The SOURCESENSE server-rendered component template library that once lived here
//! (cards, verdict bands, badges, data tables, the interactive table, empty-state
//! and error panels) was **retired** when the web UI collapsed to the embedded
//! client-side SPA ([FR-UI-22], S-192): all view markup is now produced in React.
//! What remains is the one boundary helper still needed on the Rust side —
//! [`escape`] — used by [`crate::spa::served_shell`] to attribute-escape the
//! per-session intent token it injects into the served shell ([NFR-SE-06]).
//!
//! [FR-UI-22]: ../../docs/specs/requirements/FR-UI-22.md
//! [NFR-SE-06]: ../../docs/specs/requirements/NFR-SE-06.md

/// HTML-escape a text run for safe interpolation into element content **or** a
/// double-quoted attribute. Defense in depth — the intent token is read-model
/// derived (64 hex chars by construction), but the surface never trusts a string
/// it did not author.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_neutralizes_every_html_metacharacter() {
        assert_eq!(escape("a<b>&\"'"), "a&lt;b&gt;&amp;&quot;&#39;");
        // A plain run is untouched (the common case — a hex intent token).
        assert_eq!(escape("deadbeefcafef00d"), "deadbeefcafef00d");
        // A quote cannot break out of a double-quoted attribute.
        assert_eq!(escape("\"><script>"), "&quot;&gt;&lt;script&gt;");
    }
}
