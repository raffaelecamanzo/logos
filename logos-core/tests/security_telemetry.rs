//! Each security engine method traces under its own telemetry tool (S-667) —
//! what `stats` attributes a `security_*` call to.
//!
//! A test binary of its own, holding one test: the telemetry event comes from
//! the one emission callsite every traced engine method shares, and a test
//! running concurrently in the same process can cache that callsite's interest
//! before this test's subscriber exists, so the capture would see nothing.

use std::sync::{Arc, Mutex};

use logos_core::security::{FindingsFilter, ScanEnvironment};
use logos_core::Engine;
use tracing_subscriber::layer::SubscriberExt;

/// `logos_core::observability`'s telemetry target (crate-private there).
const TELEMETRY_TARGET: &str = "logos::telemetry";

/// The `tool` of every telemetry-target event that names a security tool.
#[derive(Clone, Default)]
struct Tools(Arc<Mutex<Vec<String>>>);

struct ToolField(Option<String>);

impl tracing::field::Visit for ToolField {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "tool" {
            self.0 = Some(value.to_string());
        }
    }
    fn record_debug(&mut self, _: &tracing::field::Field, _: &dyn std::fmt::Debug) {}
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Tools {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        if event.metadata().target() == TELEMETRY_TARGET {
            let mut field = ToolField(None);
            event.record(&mut field);
            if let Some(tool) = field.0.filter(|t| t.starts_with("security_")) {
                self.0.lock().unwrap().push(tool);
            }
        }
    }
}

#[test]
fn each_security_method_traces_under_its_own_tool() {
    let tmp = tempfile::tempdir().unwrap();
    let engine = Engine::open(tmp.path()).with_scan_environment(ScanEnvironment {
        user_dir: Some(tmp.path().join("user")),
        search_path: None,
        tools_dir: None,
        nvd_api_key: None,
    });
    let absent = tmp.path().join("absent.sarif");
    let tools = Tools::default();
    tracing::subscriber::with_default(tracing_subscriber::registry().with(tools.clone()), || {
        // Each call may fail (no store, no report); the event is written either way.
        let _ = engine.security_status();
        let _ = engine.security_findings(&FindingsFilter::default());
        let _ = engine.security_ingest(&absent);
        let _ = engine.security_ingest_auto(&absent);
        let _ = engine.security_scan(false);
    });
    assert_eq!(
        *tools.0.lock().unwrap(),
        ["security_status", "security_findings", "security_ingest", "security_ingest_auto", "security_scan"]
    );
}
