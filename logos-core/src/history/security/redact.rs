//! Secret-class redaction, applied at parse time — before anything is written
//! ([FR-SC-03], [BR-67], [ADR-79]).
//!
//! A secret scanner's result *is* the secret: Gitleaks puts it in the snippet,
//! Bandit's `B105` puts the password literal in the message text, Trivy masks it
//! but keeps the surrounding line. The snippet is never read at all
//! ([`super::parse`]); this module decides which findings are **secret-class** and
//! replaces their message with fixed text that names only the rule and the
//! location. A false positive here costs a message; a false negative leaks a
//! credential into `history.db`, so the classifier leans wide:
//!
//! - the scanner is Gitleaks (every Gitleaks finding is a secret);
//! - a rule or result tag is `secret`/`secrets` (Trivy's secret scanner);
//! - the rule id mentions `secret` (Checkov `CKV_SECRET_*`, Semgrep
//!   `generic.secrets.*`);
//! - the finding carries a hard-coded-credential CWE — 259 (password), 321
//!   (cryptographic key), 798 (credentials) — which is where Bandit and gosec
//!   report a literal they found.
//!
//! [FR-SC-03]: ../../../../docs/specs/requirements/FR-SC-03.md
//! [BR-67]: ../../../../docs/specs/software-spec.md#328-security-findings
//! [ADR-79]: ../../../../docs/specs/architecture/decisions/ADR-79.md

/// The CWEs whose findings report a hard-coded secret: 259 (hard-coded
/// password), 321 (hard-coded cryptographic key), 798 (hard-coded credentials).
const SECRET_CWES: &[&str] = &["CWE-259", "CWE-321", "CWE-798"];

/// `true` when a finding is secret-class and its message must be redacted.
/// `cwes` is the normalised `CWE-<n>` list.
pub(crate) fn is_secret_class(scanner_name: &str, rule_id: &str, tags: &[&str], cwes: &[String]) -> bool {
    is_gitleaks(scanner_name)
        || tags
            .iter()
            .any(|t| t.eq_ignore_ascii_case("secret") || t.eq_ignore_ascii_case("secrets"))
        || rule_id.to_ascii_lowercase().contains("secret")
        || cwes.iter().any(|c| SECRET_CWES.contains(&c.as_str()))
}

/// `true` when a report's driver is Gitleaks, ASCII case and surrounding space
/// ignored: every one of its findings is a secret, and its identity folds in
/// the secret's keyed digest ([`super::secret`]).
pub(crate) fn is_gitleaks(scanner_name: &str) -> bool {
    scanner_name.trim().eq_ignore_ascii_case("gitleaks")
}

/// The fixed message a secret-class finding is stored with: the rule and the
/// location as the report named it, nothing from the source.
pub(crate) fn redacted_message(rule_id: &str, report_path: Option<&str>, start_line: Option<i64>) -> String {
    let location = match (report_path, start_line) {
        (Some(path), Some(line)) => format!("{path}:{line}"),
        (Some(path), None) => path.to_string(),
        (None, _) => "no location".to_string(),
    };
    format!("secret-class finding redacted: rule {rule_id} at {location}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cwes(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    /// Each route into the class fires on its own, and each sits beside the
    /// near miss that must not: another scanner, a tag that only contains the
    /// word, a CWE one digit away.
    #[test]
    fn each_route_into_the_secret_class_fires_and_its_near_miss_does_not() {
        assert!(is_secret_class("gitleaks", "generic-api-key", &[], &[]));
        assert!(is_secret_class(" Gitleaks ", "x", &[], &[]));
        assert!(!is_secret_class("gitleaks-wrapper", "x", &[], &[]));

        assert!(is_secret_class("Trivy", "aws-access-key-id", &["secret", "security"], &[]));
        assert!(is_secret_class("t", "x", &["Secrets"], &[]));
        assert!(!is_secret_class("Trivy", "CVE-2019-14234", &["vulnerability", "secret-santa"], &[]));

        assert!(is_secret_class("Checkov", "CKV_SECRET_2", &[], &[]));
        assert!(is_secret_class("Semgrep OSS", "generic.secrets.security.detected-private-key", &[], &[]));
        assert!(!is_secret_class("Checkov", "CKV_AWS_20", &[], &[]));

        assert!(is_secret_class("Bandit", "B105", &[], &cwes(&["CWE-259"])));
        assert!(is_secret_class("gosec", "G101", &[], &cwes(&["CWE-798"])));
        assert!(is_secret_class("t", "x", &[], &cwes(&["CWE-89", "CWE-321"])));
        assert!(!is_secret_class("Bandit", "B608", &[], &cwes(&["CWE-89", "CWE-79", "CWE-7980"])));
    }

    #[test]
    fn the_redacted_message_names_only_rule_and_location() {
        assert_eq!(
            redacted_message("B105", Some("app/settings.py"), Some(7)),
            "secret-class finding redacted: rule B105 at app/settings.py:7"
        );
        assert_eq!(
            redacted_message("r", Some("a.py"), None),
            "secret-class finding redacted: rule r at a.py"
        );
        assert_eq!(
            redacted_message("r", None, Some(3)),
            "secret-class finding redacted: rule r at no location"
        );
    }
}
