use std::str::FromStr;

use pansou::core::{
    CloudType, ProviderError, canonical_url_key, detect_cloud_type, extract_links, normalize_url,
};

#[test]
fn cloud_type_roundtrips_display_parse_and_json() {
    for cloud in CloudType::ALL {
        assert_eq!(CloudType::from_str(&cloud.to_string()).unwrap(), cloud);
        let encoded = serde_json::to_string(&cloud).unwrap();
        assert_eq!(serde_json::from_str::<CloudType>(&encoded).unwrap(), cloud);
    }
    assert_eq!(CloudType::from_str("alipan").unwrap(), CloudType::Aliyun);
    assert!(CloudType::from_str("made-up").is_err());
}

#[test]
fn provider_errors_have_machine_readable_kinds() {
    assert_eq!(
        serde_json::to_value(ProviderError::RateLimited).unwrap(),
        serde_json::json!({"kind": "rate_limited"}),
    );
    assert_eq!(
        serde_json::to_value(ProviderError::Protocol("changed response".into())).unwrap(),
        serde_json::json!({"kind": "protocol", "message": "changed response"}),
    );
}

#[test]
fn detects_every_supported_cloud() {
    let cases = [
        ("https://pan.baidu.com/s/abc_1", CloudType::Baidu),
        ("https://www.alipan.com/s/abc", CloudType::Aliyun),
        ("https://pan.quark.cn/s/abc", CloudType::Quark),
        ("https://guangyapan.com/s/abc", CloudType::Guangya),
        ("https://cloud.189.cn/t/abc", CloudType::Tianyi),
        ("https://drive.uc.cn/s/abc", CloudType::Uc),
        ("https://yun.139.com/shareweb/#/w/i/abc", CloudType::Mobile),
        ("https://115.com/s/abc", CloudType::One15),
        ("https://mypikpak.com/s/abc", CloudType::Pikpak),
        ("https://pan.xunlei.com/s/abc", CloudType::Xunlei),
        ("https://www.123pan.com/s/abc", CloudType::Pan123),
        ("magnet:?xt=urn:btih:abcdef", CloudType::Magnet),
        ("ed2k://|file|x|1|ABCDEF|/", CloudType::Ed2k),
        ("https://example.com/s/abc", CloudType::Others),
    ];
    for (url, expected) in cases {
        assert_eq!(detect_cloud_type(url), expected, "{url}");
    }
}

#[test]
fn extracts_normalizes_passwords_and_titles_without_cross_association() {
    let text = "第一部丨百度网盘：https://pan.baidu.com/s/Ab_c?pwd=a1B2\n\
                第二部 夸克网盘：https://pan.quark.cn/s/Qwerty 提取码：z9Y8\n\
                磁力：magnet:?xt=urn:btih:ABCDEF1234";
    let links = extract_links(text);
    assert_eq!(links.len(), 3);
    assert_eq!(links[0].password.as_deref(), Some("a1B2"));
    assert_eq!(links[0].work_title.as_deref(), Some("第一部"));
    assert_eq!(links[1].password.as_deref(), Some("z9Y8"));
    assert_eq!(links[1].work_title.as_deref(), Some("第二部"));
    assert_eq!(links[2].password, None);
}

#[test]
fn canonical_key_deduplicates_encoding_and_password() {
    assert_eq!(
        canonical_url_key("HTTPS://PAN.BAIDU.COM:443/s/a%62c?pwd=1234"),
        canonical_url_key("https://pan.baidu.com/s/abc"),
    );
    assert_eq!(
        normalize_url(" https://pan.quark.cn/s/abc。"),
        "https://pan.quark.cn/s/abc"
    );
}

#[test]
fn duplicate_share_is_emitted_once() {
    let links =
        extract_links("https://pan.baidu.com/s/abc?pwd=1234 以及 https://pan.baidu.com/s/abc");
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].password.as_deref(), Some("1234"));
}

#[test]
fn later_duplicate_can_supply_the_password() {
    let links =
        extract_links("https://pan.baidu.com/s/abc 以及 https://pan.baidu.com/s/abc?pwd=1234");
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].password.as_deref(), Some("1234"));
}
