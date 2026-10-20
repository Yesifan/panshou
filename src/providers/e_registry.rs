//! Dedicated upstream-sync adapters with provider-specific protocol boundaries.
use super::Provider;
use std::sync::Arc;
#[path = "buerchen/mod.rs"]
pub mod buerchen;
#[path = "erxiaopan/mod.rs"]
pub mod erxiaopan;
#[path = "hjzhencai/mod.rs"]
pub mod hjzhencai;
#[path = "e_html.rs"]
mod html;

pub fn providers() -> Vec<Arc<dyn Provider>> {
    vec![
        Arc::new(buerchen::Buerchen::default()),
        Arc::new(erxiaopan::Erxiaopan::default()),
        Arc::new(hjzhencai::Hjzhencai::default()),
    ]
}
