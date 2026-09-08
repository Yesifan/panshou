//! Search provider contracts and the explicit built-in registry.

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use reqwest::Client;

use crate::core::{ProviderError, SearchResult};

pub mod cldi;
pub mod clmao;
pub mod clxiong;
pub mod cyg;
pub mod djgou;
pub mod duanjuw;
pub mod dyyj;
pub mod gying;
pub mod hdmoli;
pub mod jsnoteclub;
pub mod jupansou;
pub mod meitizy;
pub mod panlian;
pub mod pansearch;
pub mod qqpd;
pub mod susu;
pub mod weibo;
pub mod yulinshufa;
pub mod yunsou;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeywordFilterMode {
    Core,
    Provider,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthKind {
    None,
    Qr,
    Password,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderMeta {
    pub name: &'static str,
    pub priority: i32,
    pub requires_auth: bool,
    pub auth_kind: AuthKind,
    pub keyword_filter: KeywordFilterMode,
}

impl ProviderMeta {
    pub const fn stateless(
        name: &'static str,
        priority: i32,
        keyword_filter: KeywordFilterMode,
    ) -> Self {
        Self {
            name,
            priority,
            requires_auth: false,
            auth_kind: AuthKind::None,
            keyword_filter,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SearchContext {
    pub client: Client,
    pub timeout: Duration,
}

impl SearchContext {
    pub fn new(client: Client, timeout: Duration) -> Self {
        Self { client, timeout }
    }
}

#[async_trait]
pub trait Provider: Send + Sync {
    fn meta(&self) -> ProviderMeta;

    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError>;
}

pub struct ProviderRegistration {
    pub provider: Arc<dyn Provider>,
    pub ready_profiles: usize,
}

impl ProviderRegistration {
    pub fn is_ready(&self) -> bool {
        !self.provider.meta().requires_auth || self.ready_profiles > 0
    }
}

pub fn builtin_stateless_providers() -> Vec<Arc<dyn Provider>> {
    vec![
        Arc::new(pansearch::Pansearch::default()),
        Arc::new(yunsou::YunsouProvider::default()),
        Arc::new(djgou::Djgou::default()),
        Arc::new(hdmoli::HdmoliProvider::default()),
        Arc::new(meitizy::MeitizyProvider::default()),
        Arc::new(yulinshufa::Yulinshufa::default()),
        Arc::new(clxiong::ClxiongProvider::default()),
        Arc::new(jsnoteclub::Jsnoteclub::default()),
        Arc::new(duanjuw::DuanjuwProvider::default()),
        Arc::new(dyyj::DyyjProvider::default()),
        Arc::new(jupansou::Jupansou::default()),
        Arc::new(cldi::CldiProvider::default()),
        Arc::new(clmao::Clmao::default()),
        Arc::new(cyg::CygProvider::default()),
        Arc::new(susu::Susu::default()),
    ]
}

pub fn find_stateless(name: &str) -> Option<Arc<dyn Provider>> {
    builtin_stateless_providers()
        .into_iter()
        .find(|provider| provider.meta().name == name)
}
