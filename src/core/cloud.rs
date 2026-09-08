use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

use super::error::ParseError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CloudType {
    Baidu,
    Aliyun,
    Quark,
    Guangya,
    Tianyi,
    Uc,
    Mobile,
    #[serde(rename = "115")]
    One15,
    Pikpak,
    Xunlei,
    #[serde(rename = "123")]
    Pan123,
    Magnet,
    Ed2k,
    #[default]
    Others,
}

impl CloudType {
    pub const ALL: [Self; 14] = [
        Self::Baidu,
        Self::Aliyun,
        Self::Quark,
        Self::Guangya,
        Self::Tianyi,
        Self::Uc,
        Self::Mobile,
        Self::One15,
        Self::Pikpak,
        Self::Xunlei,
        Self::Pan123,
        Self::Magnet,
        Self::Ed2k,
        Self::Others,
    ];
}

impl fmt::Display for CloudType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Baidu => "baidu",
            Self::Aliyun => "aliyun",
            Self::Quark => "quark",
            Self::Guangya => "guangya",
            Self::Tianyi => "tianyi",
            Self::Uc => "uc",
            Self::Mobile => "mobile",
            Self::One15 => "115",
            Self::Pikpak => "pikpak",
            Self::Xunlei => "xunlei",
            Self::Pan123 => "123",
            Self::Magnet => "magnet",
            Self::Ed2k => "ed2k",
            Self::Others => "others",
        };
        f.write_str(value)
    }
}

impl FromStr for CloudType {
    type Err = ParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "baidu" | "baidupan" => Ok(Self::Baidu),
            "aliyun" | "alipan" | "aliyundrive" => Ok(Self::Aliyun),
            "quark" => Ok(Self::Quark),
            "guangya" | "guangyapan" => Ok(Self::Guangya),
            "tianyi" | "189" => Ok(Self::Tianyi),
            "uc" => Ok(Self::Uc),
            "mobile" | "139" | "caiyun" => Ok(Self::Mobile),
            "115" | "one15" => Ok(Self::One15),
            "pikpak" => Ok(Self::Pikpak),
            "xunlei" => Ok(Self::Xunlei),
            "123" | "pan123" | "123pan" => Ok(Self::Pan123),
            "magnet" => Ok(Self::Magnet),
            "ed2k" => Ok(Self::Ed2k),
            "other" | "others" | "unknown" => Ok(Self::Others),
            other => Err(ParseError::UnknownCloudType(other.to_owned())),
        }
    }
}
