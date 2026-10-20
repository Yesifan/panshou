use pansou::check::{CheckEngine, CheckItem, CheckOptions, CheckState};
use pansou::core::link::extract_password_for_link;
use pansou::core::{CloudType, detect_cloud_type, extract_links};

#[test]
fn new_cloud_types_use_host_boundaries_and_round_trip() {
    for (name, domain, kind) in [
        ("weiyun", "weiyun.com", CloudType::Weiyun),
        ("lanzou", "lanzoui.com", CloudType::Lanzou),
    ] {
        for host in [domain.to_owned(), format!("share.{domain}")] {
            assert_eq!(detect_cloud_type(&format!("https://{host}/abc")), kind);
        }
        for raw in [
            format!("https://evil.test/?url={domain}"),
            format!("https://evil{domain}/abc"),
            format!("https://{domain}.evil.test/abc"),
            format!("not-a-url-{domain}"),
        ] {
            assert_eq!(detect_cloud_type(&raw), CloudType::Others);
        }
        assert_eq!(name.parse::<CloudType>().unwrap(), kind);
        assert_eq!(kind.to_string(), name);
        assert_eq!(serde_json::to_string(&kind).unwrap(), format!("\"{name}\""));
        assert_eq!(
            serde_json::from_str::<CloudType>(&format!("\"{name}\"")).unwrap(),
            kind
        );
    }
}

#[tokio::test]
async fn new_cloud_types_have_no_checker() {
    let engine = CheckEngine::new(reqwest::Client::new(), None);
    let results = engine
        .check(
            vec![
                CheckItem::detect("https://share.weiyun.com/demo"),
                CheckItem::detect("https://demo.lanzoui.com/demo"),
            ],
            CheckOptions::default(),
        )
        .await;
    assert!(
        results
            .iter()
            .all(|result| result.state == CheckState::Unsupported)
    );
}

#[test]
fn explicit_raw_query_codes_override_earlier_guesses() {
    let text = "https://pan.quark.cn/s/same 提取码 guess1\nhttps://pan.quark.cn/s/same?pwd=real12&from=tg\nhttps://drive.uc.cn/s/abc123?password=uc9999";
    let links = extract_links(text);
    // Non-code query parameters remain part of the canonical identity.
    assert_eq!(links[1].password.as_deref(), Some("real12"));
    assert!(links[1].url.contains("pwd=real12"));
    assert_eq!(links[2].password.as_deref(), Some("uc9999"));
    let duplicate = extract_links(
        "https://pan.quark.cn/s/same 提取码 guess1\nhttps://pan.quark.cn/s/same?pwd=real12",
    );
    assert_eq!(duplicate.len(), 1);
    assert_eq!(duplicate[0].password.as_deref(), Some("real12"));
}

#[test]
fn local_password_windows_do_not_borrow_other_links_codes() {
    let first = "https://pan.baidu.com/s/firstlink";
    let second = "https://drive.uc.cn/s/abc123";
    let third = "https://alipan.com/s/thirdlink";
    let text = format!("{first}\n提取码 1234\n{second} 提取码 uc99\n{third}");
    let links = extract_links(&text);
    assert_eq!(links.len(), 3);
    assert_eq!(links[0].password.as_deref(), Some("1234"));
    assert_eq!(links[1].password.as_deref(), Some("uc99"));
    assert!(links[1].url.ends_with("abc123"));
    assert_eq!(links[2].password, None);
    assert_eq!(
        extract_password_for_link("提取码 shared", first, ""),
        Some("shared".into())
    );
    assert_eq!(
        extract_password_for_link("", "https://pan.quark.cn/s/demo?pwd=a1b2", "提取码 c3d4")
            .as_deref(),
        Some("a1b2")
    );
    let long = format!("{first}{}提取码 late", "中".repeat(20));
    // The code is beyond 60 bytes; it is not a message-wide fallback.
    assert_eq!(extract_password_for_link(&long, first, ""), None);
}
