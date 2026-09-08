//! Link validity checking, intentionally independent from the search providers.

mod aliyun;
mod baidu;
mod cache;
mod engine;
mod mobile;
mod mobile_crypto;
mod one15;
mod pan123;
mod protocol;
mod quark;
mod tianyi;
mod uc;
mod xunlei;

pub mod normalize;

pub use cache::{CacheError, CheckCache};
pub use engine::{
    CheckContext, CheckEngine, CheckError, CheckEvaluation, CheckItem, CheckOptions, CheckResult,
    CheckState, LinkChecker, proxy_scope, ttl_for_state,
};
pub use normalize::{CheckCloudType, detect_check_cloud_type, normalize_share_link};

use std::sync::Arc;

/// The nine checkers supported by v1.
pub fn builtin_checkers() -> Vec<Arc<dyn LinkChecker>> {
    vec![
        Arc::new(aliyun::AliyunChecker::default()),
        Arc::new(quark::QuarkChecker::default()),
        Arc::new(uc::UcChecker),
        Arc::new(baidu::BaiduChecker::default()),
        Arc::new(tianyi::TianyiChecker::default()),
        Arc::new(pan123::Pan123Checker::default()),
        Arc::new(xunlei::XunleiChecker::default()),
        Arc::new(one15::One15Checker::default()),
        Arc::new(mobile::MobileChecker::default()),
    ]
}
