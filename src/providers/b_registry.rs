//! Registration boundary for provider group B.
//!
//! Root integration: add `pub mod b_registry;` to providers/mod.rs and extend
//! `builtin_stateless_providers()` with `b_registry::providers()`.
use super::Provider;
use std::sync::Arc;
#[cfg(test)]
#[path = "b_boundary_tests.rs"]
mod boundary_tests;
#[path = "b_http.rs"]
pub(crate) mod http;
#[path = "jikepan.rs"]
pub mod jikepan;
#[path = "jutoushe.rs"]
pub mod jutoushe;
#[path = "kkmao.rs"]
pub mod kkmao;
#[path = "kpkuang.rs"]
pub mod kpkuang;
#[path = "b_markup.rs"]
pub(crate) mod markup;
#[path = "melost.rs"]
pub mod melost;
#[path = "mizixing.rs"]
pub mod mizixing;
#[path = "muou.rs"]
pub mod muou;
#[path = "nyaa.rs"]
pub mod nyaa;
#[path = "ouge.rs"]
pub mod ouge;
#[path = "pan365/mod.rs"]
pub mod pan365;
#[path = "pan666.rs"]
pub mod pan666;
#[path = "panta.rs"]
pub mod panta;
#[path = "panwiki.rs"]
pub mod panwiki;
#[path = "panyq.rs"]
pub mod panyq;
#[path = "panzun.rs"]
pub mod panzun;
#[path = "pianku.rs"]
pub mod pianku;
#[path = "qingying.rs"]
pub mod qingying;
#[path = "qiwei.rs"]
pub mod qiwei;
#[path = "quark4k.rs"]
pub mod quark4k;
pub fn providers() -> Vec<Arc<dyn Provider>> {
    vec![
        Arc::new(kpkuang::Kpkuang::default()),
        Arc::new(pan365::Pan365::default()),
        Arc::new(jikepan::Jikepan::default()),
        Arc::new(nyaa::Nyaa::default()),
        Arc::new(pan666::Pan666::default()),
        Arc::new(quark4k::Quark4k::default()),
        Arc::new(kkmao::Kkmao::default()),
        Arc::new(ouge::Ouge::default()),
        Arc::new(melost::Melost::default()),
        Arc::new(jutoushe::Jutoushe::default()),
        Arc::new(mizixing::Mizixing::default()),
        Arc::new(muou::Muou::default()),
        Arc::new(panta::Panta::default()),
        Arc::new(panwiki::Panwiki::default()),
        Arc::new(panyq::Panyq::default()),
        Arc::new(panzun::Panzun::default()),
        Arc::new(pianku::Pianku::default()),
        Arc::new(qingying::Qingying::default()),
        Arc::new(qiwei::Qiwei::default()),
    ]
}
