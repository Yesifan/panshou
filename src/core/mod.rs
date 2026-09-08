pub mod cloud;
pub mod error;
pub mod filter;
pub mod link;
pub mod merge;
pub mod model;
pub mod rank;

pub use cloud::CloudType;
pub use error::{CheckError, ConfigError, HttpError, ParseError, ProviderError, StateError};
pub use filter::{FilterOptions, filter_results};
pub use link::{
    associate_work_titles, canonical_url_key, detect_cloud_type, extract_links, extract_password,
    normalize_url,
};
pub use merge::{merge_links, merge_search_results};
pub use model::{CheckResult, CheckState, Link, MergedLink, SearchResult, Source};
pub use rank::{keyword_score, rank_results, score_result, time_score};
