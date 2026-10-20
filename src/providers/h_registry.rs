//! Fixed-upstream H-group registrations. No background work or disk search caches.
use super::Provider;
use std::sync::Arc;
#[path = "kkv.rs"]
pub mod kkv;
#[path = "labi.rs"]
pub mod labi;
#[path = "leijing.rs"]
pub mod leijing;
#[path = "lingjisp.rs"]
pub mod lingjisp;
#[path = "lou1.rs"]
pub mod lou1;
#[path = "miaoso.rs"]
pub mod miaoso;
#[path = "mikuclub.rs"]
pub mod mikuclub;
#[path = "miosou.rs"]
pub mod miosou;
#[path = "h_support.rs"]
pub(crate) mod support;
pub fn providers() -> Vec<Arc<dyn Provider>> {
    providers_with_options(&crate::http::ClientOptions::default())
}
pub fn providers_with_options(options: &crate::http::ClientOptions) -> Vec<Arc<dyn Provider>> {
    let miaoso = miaoso::Miaoso::with_options(
        "https://miaosou.fun/api/secendsearch".parse().unwrap(),
        options.clone(),
    );
    let miaoso = match std::env::var("PANSOU_MIAOSO_TOKEN")
        .ok()
        .filter(|token| !token.trim().is_empty())
    {
        Some(token) => miaoso.with_token(token),
        None => miaoso,
    };
    vec![
        Arc::new(kkv::Kkv::default()),
        Arc::new(labi::Labi::default()),
        Arc::new(leijing::Leijing::default()),
        Arc::new(lingjisp::Lingjisp::default()),
        Arc::new(lou1::Lou1::default()),
        Arc::new(miaoso),
        Arc::new(mikuclub::Mikuclub::default()),
        Arc::new(miosou::Miosou::with_options(
            "https://miosou.cc/".parse().unwrap(),
            options.clone(),
        )),
    ]
}
#[cfg(test)]
#[path = "h_tests.rs"]
mod tests;
