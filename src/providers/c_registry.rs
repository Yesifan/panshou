//! Stateless providers implemented by the C protocol group.
use super::c_support;
use super::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};
use std::sync::Arc;
#[path = "quarkres.rs"]
pub mod quarkres;
#[path = "quarksoo.rs"]
pub mod quarksoo;
#[path = "qupansou.rs"]
pub mod qupansou;
#[path = "sopanya.rs"]
pub mod sopanya;
#[path = "u3c3.rs"]
pub mod u3c3;
#[path = "wanou.rs"]
pub mod wanou;
#[path = "xdpan.rs"]
pub mod xdpan;
#[path = "xdyh.rs"]
pub mod xdyh;
#[path = "xiaoji.rs"]
pub mod xiaoji;
#[path = "xiaoyu.rs"]
pub mod xiaoyu;
#[path = "xiaozhang.rs"]
pub mod xiaozhang;
#[path = "xinjuc.rs"]
pub mod xinjuc;
#[path = "xys.rs"]
pub mod xys;
#[path = "yingso.rs"]
pub mod yingso;
#[path = "yiove.rs"]
pub mod yiove;
#[path = "ypfxw.rs"]
pub mod ypfxw;
#[path = "yuhuage.rs"]
pub mod yuhuage;
#[path = "yunso.rs"]
pub mod yunso;
#[path = "zhizhen.rs"]
pub mod zhizhen;
#[path = "zlxapp.rs"]
pub mod zlxapp;
#[path = "zxzj.rs"]
pub mod zxzj;

pub fn providers() -> Vec<Arc<dyn Provider>> {
    providers_with_options(&crate::http::ClientOptions::default())
}
pub fn providers_with_options(options: &crate::http::ClientOptions) -> Vec<Arc<dyn Provider>> {
    vec![
        Arc::new(quarkres::Quarkres::default()),
        Arc::new(qupansou::Qupansou::default()),
        Arc::new(xdyh::Xdyh::default()),
        Arc::new(yingso::Yingso::default()),
        Arc::new(sopanya::Sopanya::default()),
        Arc::new(yunso::Yunso::default()),
        Arc::new(zlxapp::Zlxapp::default()),
        Arc::new(wanou::Wanou::default()),
        Arc::new(quarksoo::Quarksoo::default()),
        Arc::new(zhizhen::Zhizhen::default()),
        Arc::new(xys::Xys::default()),
        Arc::new(u3c3::U3c3::default()),
        Arc::new(xdpan::Xdpan::default()),
        Arc::new(xiaoji::Xiaoji::default()),
        Arc::new(xiaoyu::Xiaoyu::default()),
        Arc::new(xiaozhang::Xiaozhang::new(
            xiaozhang::Endpoints::default(),
            options.clone(),
        )),
        Arc::new(xinjuc::Xinjuc::default()),
        Arc::new(yiove::Yiove::default()),
        Arc::new(ypfxw::Ypfxw::default()),
        Arc::new(yuhuage::Yuhuage::default()),
        Arc::new(zxzj::Zxzj::default()),
    ]
}
