use pansou::check::{CheckCloudType, normalize_share_link};

#[test]
fn removes_fragments_and_lowercases_hosts() {
    let got = normalize_share_link(
        CheckCloudType::Baidu,
        "https://PAN.BAIDU.COM/s/1abc#ignored",
        None,
    )
    .unwrap();
    assert_eq!(got, "https://pan.baidu.com/s/1abc");
}

#[test]
fn password_is_added_only_when_missing() {
    assert_eq!(
        normalize_share_link(
            CheckCloudType::Quark,
            "https://pan.quark.cn/s/abc",
            Some("xy z")
        )
        .unwrap(),
        "https://pan.quark.cn/s/abc?pwd=xy+z"
    );
    assert_eq!(
        normalize_share_link(
            CheckCloudType::Quark,
            "https://pan.quark.cn/s/abc?pwd=old",
            Some("new")
        )
        .unwrap(),
        "https://pan.quark.cn/s/abc?pwd=old"
    );
}

#[test]
fn query_is_canonical_and_mobile_fragment_survives() {
    assert_eq!(
        normalize_share_link(
            CheckCloudType::Baidu,
            "https://pan.baidu.com/s/1abc?z=2&a=1",
            None
        )
        .unwrap(),
        "https://pan.baidu.com/s/1abc?a=1&z=2"
    );
    assert_eq!(
        normalize_share_link(
            CheckCloudType::Mobile,
            "https://yun.139.com/shareweb/#/w/i/abc",
            None
        )
        .unwrap(),
        "https://yun.139.com/shareweb/#/w/i/abc"
    );
}

#[test]
fn rejects_non_urls_and_empty_values() {
    assert!(normalize_share_link(CheckCloudType::Quark, "", None).is_none());
    assert!(normalize_share_link(CheckCloudType::Quark, "not a URL", None).is_none());
}

#[test]
fn preserves_fragment_style_tianyi_share_code_as_path() {
    assert_eq!(
        normalize_share_link(
            CheckCloudType::Tianyi,
            "https://cloud.189.cn/web/main#/t/abc123",
            None
        )
        .unwrap(),
        "https://cloud.189.cn/t/abc123"
    );
}
