//! Orders: a small fixture crate.
pub mod model;
pub mod service;

pub use service::OrderService;

/// Entry point used by the binary.
pub fn run() -> usize {
    let svc = OrderService::new();
    let total = svc.place(model::Order::new(3, 7));
    serde_json::to_string(&total).map(|s| s.len()).unwrap_or(0)
}
