use std::collections::HashMap;
use std::sync::OnceLock;

use percent_encoding::percent_decode_str;
use regex::Regex;
use url::Url;

use super::{CloudType, Link};

fn link_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(
        r#"(?xi)
        magnet:\?xt=urn:btih:[a-z0-9]+(?:&[^\s<>"']*)?
        |ed2k://\|file\|[^|]+\|\d+\|[a-f0-9]+\|/?
        |https?://(?:www\.)?(?:
          pan\.baidu\.com/s/[a-z0-9_-]+(?:\?pwd=[a-z0-9]{4})?
         |(?:alipan|aliyundrive)\.com/s/[a-z0-9_-]+
         |pan\.quark\.cn/s/[a-z0-9_-]+
         |guangyapan\.com/s/[a-z0-9_-]+
         |cloud\.189\.cn/t/[a-z0-9%_-]+(?:（访问码：[a-z0-9]+）|%EF%BC%88%E8%AE%BF%E9%97%AE%E7%A0%81%EF%BC%9A[a-z0-9]+%EF%BC%89)?
         |drive\.uc\.cn/s/[a-z0-9_-]+(?:\?public=\d)?
         |(?:yun\.139\.com/shareweb/\#/w/i/[a-z0-9_-]+|caiyun\.139\.com/(?:w/i/[a-z0-9_-]+|m/i\?[^\s<>"']+)|caiyun\.feixin\.10086\.cn/[a-z0-9_-]+)
         |(?:115\.com|115cdn\.com|anxia\.com)/s/[a-z0-9_-]+(?:\?password=[a-z0-9]{4})?
         |mypikpak\.com/s/[a-z0-9_-]+
         |pan\.xunlei\.com/s/[a-z0-9_-]+(?:\?pwd=[a-z0-9]{4})?\#?
         |(?:123684|123685|123865|123912|123pan|123592)\.(?:com|cn)/s/[a-z0-9_-]+(?:\?(?:%E6%8F%90%E5%8F%96%E7%A0%81|提取码)[:：][a-z0-9]+)?
        )"#,
    ).expect("the built-in link regex is valid"))
}

fn url_password_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)[?&](?:pwd|password)=([a-z0-9]{4,6})(?:[^a-z0-9]|$)").unwrap()
    })
}

fn text_password_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)(?:(?:提取|访问|提取密|密)码|pwd)\s*[：:]\s*([a-z0-9]{4,6})(?:[^a-z0-9]|$)",
        )
        .unwrap()
    })
}

pub fn detect_cloud_type(raw_url: &str) -> CloudType {
    let value = raw_url.trim().to_ascii_lowercase();
    if value.starts_with("magnet:") {
        return CloudType::Magnet;
    }
    if value.starts_with("ed2k:") {
        return CloudType::Ed2k;
    }
    if value.contains("pan.baidu.com") {
        return CloudType::Baidu;
    }
    if value.contains("alipan.com") || value.contains("aliyundrive.com") {
        return CloudType::Aliyun;
    }
    if value.contains("pan.quark.cn") {
        return CloudType::Quark;
    }
    if value.contains("guangyapan.com") {
        return CloudType::Guangya;
    }
    if value.contains("cloud.189.cn") {
        return CloudType::Tianyi;
    }
    if value.contains("drive.uc.cn") {
        return CloudType::Uc;
    }
    if value.contains("yun.139.com")
        || value.contains("caiyun.139.com")
        || value.contains("caiyun.feixin.10086.cn")
    {
        return CloudType::Mobile;
    }
    if value.contains("115.com") || value.contains("115cdn.com") || value.contains("anxia.com") {
        return CloudType::One15;
    }
    if value.contains("mypikpak.com") {
        return CloudType::Pikpak;
    }
    if value.contains("pan.xunlei.com") {
        return CloudType::Xunlei;
    }
    if [
        "123684.com",
        "123685.com",
        "123865.com",
        "123912.com",
        "123pan.com",
        "123pan.cn",
        "123592.com",
    ]
    .iter()
    .any(|domain| value.contains(domain))
    {
        return CloudType::Pan123;
    }
    CloudType::Others
}

pub fn normalize_url(raw_url: &str) -> String {
    let trimmed = raw_url.trim().trim_matches(|c: char| {
        matches!(
            c,
            '，' | '。' | '；' | ';' | ',' | ')' | '）' | ']' | '】' | '>' | '"' | '\''
        )
    });
    let html_decoded = trimmed.replace("&amp;", "&");
    if html_decoded.starts_with("magnet:") || html_decoded.starts_with("ed2k:") {
        return html_decoded;
    }

    let decoded = percent_decode_str(&html_decoded)
        .decode_utf8_lossy()
        .into_owned();
    let Ok(mut parsed) = Url::parse(&decoded) else {
        return decoded;
    };
    let scheme = parsed.scheme().to_ascii_lowercase();
    let _ = parsed.set_scheme(&scheme);
    if let Some(host) = parsed.host_str().map(str::to_ascii_lowercase) {
        let _ = parsed.set_host(Some(&host));
    }
    if (parsed.scheme() == "http" && parsed.port() == Some(80))
        || (parsed.scheme() == "https" && parsed.port() == Some(443))
    {
        let _ = parsed.set_port(None);
    }
    if parsed.fragment().is_some()
        && !matches!(detect_cloud_type(parsed.as_str()), CloudType::Mobile)
    {
        parsed.set_fragment(None);
    }
    parsed.to_string().trim_end_matches('/').to_owned()
}

/// A comparison key that deliberately ignores extraction-code parameters.
pub fn canonical_url_key(raw_url: &str) -> String {
    let normalized = normalize_url(raw_url);
    if normalized.starts_with("magnet:") || normalized.starts_with("ed2k:") {
        return normalized.to_ascii_lowercase();
    }
    let Ok(mut parsed) = Url::parse(&normalized) else {
        return normalized.to_ascii_lowercase();
    };
    let kept: Vec<(String, String)> = parsed
        .query_pairs()
        .filter(|(key, _)| {
            !key.eq_ignore_ascii_case("pwd") && !key.eq_ignore_ascii_case("password")
        })
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    parsed.set_query(None);
    if !kept.is_empty() {
        parsed.query_pairs_mut().extend_pairs(kept);
    }
    parsed.to_string().trim_end_matches('/').to_owned()
}

pub fn extract_password(content: &str, url: &str) -> Option<String> {
    if let Some(found) = url_password_regex().captures(url).and_then(|c| c.get(1)) {
        return Some(found.as_str().to_owned());
    }
    let decoded = percent_decode_str(url).decode_utf8_lossy();
    if let Some(found) = text_password_regex()
        .captures(&decoded)
        .and_then(|c| c.get(1))
    {
        return Some(found.as_str().to_owned());
    }
    text_password_regex()
        .captures(content)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_owned())
}

pub fn extract_links(text: &str) -> Vec<Link> {
    let matches: Vec<_> = link_regex().find_iter(text).collect();
    let mut positions = HashMap::<String, usize>::new();
    let mut links: Vec<Link> = Vec::new();

    for found in &matches {
        let url = normalize_url(found.as_str());
        let key = canonical_url_key(&url);
        let line_start = text[..found.start()].rfind('\n').map_or(0, |i| i + 1);
        let line_end = text[found.end()..]
            .find('\n')
            .map_or(text.len(), |i| found.end() + i);
        let line = &text[line_start..line_end];
        let password = extract_password(line, &url).or_else(|| {
            (matches.len() == 1)
                .then(|| extract_password(text, &url))
                .flatten()
        });
        if let Some(&position) = positions.get(&key) {
            if links[position].password.is_none() && password.is_some() {
                links[position].password = password;
                if url_password_regex().is_match(&url) {
                    links[position].url = url;
                }
            }
            continue;
        }
        positions.insert(key, links.len());
        links.push(Link {
            cloud_type: detect_cloud_type(&url),
            url,
            password,
            datetime: None,
            work_title: None,
        });
    }
    associate_work_titles(&mut links, text, None);
    links
}

pub fn associate_work_titles(links: &mut [Link], text: &str, default_title: Option<&str>) {
    let mut title_by_key = HashMap::new();
    let mut previous_text = None::<String>;
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        let urls = link_regex().find_iter(line).collect::<Vec<_>>();
        if urls.is_empty() {
            previous_text = Some(clean_title(line));
            continue;
        }
        let prefix = line[..urls[0].start()].trim();
        let inline = inline_title(prefix);
        let title = inline
            .or_else(|| previous_text.clone())
            .or_else(|| default_title.map(str::to_owned));
        if let Some(title) = title.filter(|s| !s.is_empty()) {
            for found in urls {
                title_by_key.insert(canonical_url_key(found.as_str()), title.clone());
            }
        }
    }
    for link in links {
        if link.work_title.is_none() {
            link.work_title = title_by_key
                .get(&canonical_url_key(&link.url))
                .cloned()
                .or_else(|| default_title.map(str::to_owned));
        }
    }
}

fn inline_title(prefix: &str) -> Option<String> {
    let mut value = prefix.trim_end_matches(|c: char| c.is_whitespace() || matches!(c, ':' | '：'));
    if let Some((title, _)) = value.split_once('丨') {
        value = title;
    }
    for marker in [
        "夸克网盘",
        "百度网盘",
        "迅雷网盘",
        "阿里云盘",
        "天翼云盘",
        "UC网盘",
        "移动云盘",
        "115网盘",
        "123网盘",
        "PikPak网盘",
        "夸克",
        "百度",
        "迅雷",
        "阿里",
        "天翼",
        "UC",
        "115",
        "123",
        "链接",
        "地址",
    ] {
        value = value.trim_end_matches(marker).trim_end();
    }
    let title = clean_title(value);
    (!title.is_empty()).then_some(title)
}

fn clean_title(value: &str) -> String {
    value
        .trim()
        .trim_start_matches(['#', '-', '•', '·'])
        .trim()
        .strip_prefix("名称：")
        .or_else(|| value.trim().strip_prefix("标题："))
        .unwrap_or(value.trim())
        .trim()
        .to_owned()
}
