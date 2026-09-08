use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckCloudType {
    Aliyun,
    Quark,
    Uc,
    Baidu,
    Tianyi,
    #[serde(rename = "123")]
    Pan123,
    Xunlei,
    #[serde(rename = "115")]
    One15,
    Mobile,
    Unsupported,
}

impl From<crate::core::CloudType> for CheckCloudType {
    fn from(value: crate::core::CloudType) -> Self {
        use crate::core::CloudType;
        match value {
            CloudType::Aliyun => Self::Aliyun,
            CloudType::Quark => Self::Quark,
            CloudType::Uc => Self::Uc,
            CloudType::Baidu => Self::Baidu,
            CloudType::Tianyi => Self::Tianyi,
            CloudType::Pan123 => Self::Pan123,
            CloudType::Xunlei => Self::Xunlei,
            CloudType::One15 => Self::One15,
            CloudType::Mobile => Self::Mobile,
            _ => Self::Unsupported,
        }
    }
}

impl From<CheckCloudType> for crate::core::CloudType {
    fn from(value: CheckCloudType) -> Self {
        match value {
            CheckCloudType::Aliyun => Self::Aliyun,
            CheckCloudType::Quark => Self::Quark,
            CheckCloudType::Uc => Self::Uc,
            CheckCloudType::Baidu => Self::Baidu,
            CheckCloudType::Tianyi => Self::Tianyi,
            CheckCloudType::Pan123 => Self::Pan123,
            CheckCloudType::Xunlei => Self::Xunlei,
            CheckCloudType::One15 => Self::One15,
            CheckCloudType::Mobile => Self::Mobile,
            CheckCloudType::Unsupported => Self::Others,
        }
    }
}

impl fmt::Display for CheckCloudType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Aliyun => "aliyun",
            Self::Quark => "quark",
            Self::Uc => "uc",
            Self::Baidu => "baidu",
            Self::Tianyi => "tianyi",
            Self::Pan123 => "123",
            Self::Xunlei => "xunlei",
            Self::One15 => "115",
            Self::Mobile => "mobile",
            Self::Unsupported => "unsupported",
        })
    }
}

impl FromStr for CheckCloudType {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "aliyun" | "alipan" => Ok(Self::Aliyun),
            "quark" => Ok(Self::Quark),
            "uc" => Ok(Self::Uc),
            "baidu" => Ok(Self::Baidu),
            "tianyi" | "189" => Ok(Self::Tianyi),
            "123" | "pan123" => Ok(Self::Pan123),
            "xunlei" => Ok(Self::Xunlei),
            "115" | "one15" => Ok(Self::One15),
            "mobile" | "139" => Ok(Self::Mobile),
            other => Err(format!("unsupported cloud type: {other}")),
        }
    }
}

pub fn detect_check_cloud_type(raw: &str) -> CheckCloudType {
    let host = Url::parse(raw)
        .ok()
        .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
        .unwrap_or_default();
    if host.contains("aliyundrive.com") || host.contains("alipan.com") {
        CheckCloudType::Aliyun
    } else if host.contains("quark.cn") {
        CheckCloudType::Quark
    } else if host.contains("drive.uc.cn") {
        CheckCloudType::Uc
    } else if host.contains("pan.baidu.com") {
        CheckCloudType::Baidu
    } else if host.contains("cloud.189.cn") {
        CheckCloudType::Tianyi
    } else if host.contains("123pan")
        || [
            "123684.com",
            "123685.com",
            "123912.com",
            "123592.com",
            "123865.com",
        ]
        .contains(&host.as_str())
    {
        CheckCloudType::Pan123
    } else if host.contains("pan.xunlei.com") {
        CheckCloudType::Xunlei
    } else if host.contains("115.com") || host.contains("115cdn.com") || host.contains("anxia.com")
    {
        CheckCloudType::One15
    } else if host.contains("139.com") || host.contains("feixin.10086.cn") {
        CheckCloudType::Mobile
    } else {
        CheckCloudType::Unsupported
    }
}

/// Canonicalize without discarding provider-specific path/fragment information.
pub fn normalize_share_link(
    cloud: CheckCloudType,
    raw: &str,
    password: Option<&str>,
) -> Option<String> {
    let mut url = Url::parse(raw.trim()).ok()?;
    let host = url.host_str()?.to_ascii_lowercase();
    url.set_host(Some(&host)).ok()?;
    // Some historical Tianyi links put `/t/<code>` in the fragment. Move it to
    // the path so normalization does not destroy the share identifier.
    if cloud == CheckCloudType::Tianyi
        && !url.path().starts_with("/t/")
        && let Some(fragment) = url.fragment().filter(|value| value.starts_with("/t/"))
    {
        let path = fragment.to_owned();
        url.set_path(&path);
    }
    // Mobile's share id may live in the fragment; all other fragments are tracking noise.
    if cloud != CheckCloudType::Mobile {
        url.set_fragment(None);
    }
    if matches!(
        cloud,
        CheckCloudType::Baidu | CheckCloudType::Quark | CheckCloudType::Uc
    ) && let Some(password) = password.filter(|p| !p.is_empty())
    {
        let has_pwd = url.query_pairs().any(|(k, _)| k == "pwd");
        if !has_pwd {
            url.query_pairs_mut().append_pair("pwd", password);
        }
    }
    let mut query: Vec<(String, String)> = url
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    query.sort();
    url.set_query(None);
    if !query.is_empty() {
        url.query_pairs_mut().extend_pairs(query);
    }
    Some(url.into())
}

pub(crate) fn last_path_segment(raw: &str) -> Option<String> {
    Url::parse(raw)
        .ok()?
        .path_segments()?
        .rfind(|s| !s.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detects_all_supported_hosts() {
        let cases = [
            ("https://www.alipan.com/s/a", CheckCloudType::Aliyun),
            ("https://pan.quark.cn/s/a", CheckCloudType::Quark),
            ("https://drive.uc.cn/s/a", CheckCloudType::Uc),
            ("https://pan.baidu.com/s/1a", CheckCloudType::Baidu),
            ("https://cloud.189.cn/t/a", CheckCloudType::Tianyi),
            ("https://www.123pan.com/s/a", CheckCloudType::Pan123),
            ("https://pan.xunlei.com/s/a", CheckCloudType::Xunlei),
            ("https://115cdn.com/s/a", CheckCloudType::One15),
            ("https://anxia.com/s/a", CheckCloudType::One15),
            (
                "https://yun.139.com/shareweb/#/w/i/a",
                CheckCloudType::Mobile,
            ),
        ];
        for (url, expected) in cases {
            assert_eq!(detect_check_cloud_type(url), expected);
        }
    }
    #[test]
    fn normalizes_password_and_fragment() {
        assert_eq!(
            normalize_share_link(
                CheckCloudType::Quark,
                "https://PAN.QUARK.CN/s/abc#x",
                Some("p 1")
            )
            .unwrap(),
            "https://pan.quark.cn/s/abc?pwd=p+1"
        );
    }
    #[test]
    fn preserves_mobile_fragment() {
        assert!(
            normalize_share_link(
                CheckCloudType::Mobile,
                "https://yun.139.com/shareweb/#/w/i/id",
                None
            )
            .unwrap()
            .contains("#/w/i/id")
        );
    }
}
