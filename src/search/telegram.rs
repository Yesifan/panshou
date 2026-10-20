use chrono::{DateTime, Utc};
use scraper::{Html, Selector};
use url::Url;

use crate::channel::{ChannelPageKind, classify_channel_page};
use crate::core::{
    Link, ProviderError, SearchResult, Source, associate_work_titles, canonical_url_key,
    extract_links,
    link::{extract_password_for_link, message_title_line},
};

#[derive(Debug, Clone)]
pub struct TelegramSource {
    client: reqwest::Client,
    base_url: Url,
}

impl TelegramSource {
    pub fn new(client: reqwest::Client) -> Self {
        Self {
            client,
            base_url: Url::parse("https://t.me/").expect("static Telegram URL"),
        }
    }

    pub fn with_base_url(client: reqwest::Client, base_url: Url) -> Self {
        Self { client, base_url }
    }

    pub async fn search(
        &self,
        channel: &str,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let channel = crate::channel::normalize_channel(channel)
            .map_err(|error| ProviderError::Protocol(error.to_string()))?;
        let mut url = self
            .base_url
            .join(&format!("s/{channel}"))
            .map_err(|error| ProviderError::Protocol(error.to_string()))?;
        url.query_pairs_mut().append_pair("q", query);
        let response = crate::http::execute(self.client.get(url))
            .await
            .map_err(|error| ProviderError::Network(error.to_string()))?;
        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(ProviderError::RateLimited);
        }
        if matches!(
            response.status(),
            reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::GONE
        ) {
            return Err(ProviderError::InvalidSource(format!(
                "Telegram returned HTTP {}",
                response.status()
            )));
        }
        if !response.status().is_success() {
            return Err(ProviderError::Unavailable(format!(
                "HTTP {}",
                response.status()
            )));
        }
        let body = crate::http::read_response(response, 2 * 1024 * 1024)
            .await
            .map_err(|_| {
                ProviderError::Protocol(
                    "Telegram response body read failed or exceeded limit".into(),
                )
            })?;
        let body = String::from_utf8_lossy(&body);
        match classify_channel_page(&body, &channel) {
            ChannelPageKind::Bot => {
                return Err(ProviderError::InvalidSource(
                    "it is a bot, not a public channel".into(),
                ));
            }
            ChannelPageKind::NoPublicMessages => {
                return Err(ProviderError::InvalidSource(
                    "it has no public /s/<name> message archive".into(),
                ));
            }
            ChannelPageKind::PublicMessages | ChannelPageKind::Unknown => {}
        }
        parse_telegram(&body, &channel)
    }
}

pub fn parse_telegram(html: &str, channel: &str) -> Result<Vec<SearchResult>, ProviderError> {
    let document = Html::parse_document(html);
    let wrap = selector(".tgme_widget_message_wrap")?;
    let message = selector(".tgme_widget_message")?;
    let text = selector(".tgme_widget_message_text")?;
    let time = selector(".tgme_widget_message_date time")?;
    let image = selector(".tgme_widget_message_photo_wrap")?;
    let inline_image = selector(".tgme_widget_message_bubble img[src]")?;
    let anchor = selector("a[href]")?;
    let keyboard_anchor = selector(".tgme_widget_message_inline_keyboard a[href]")?;
    let tag = selector("a[href^='?q=%23']")?;

    let mut results = Vec::new();
    let mut recognized = 0;
    let mut dated = 0;
    let mut wraps = 0;
    for node in document.select(&wrap) {
        wraps += 1;
        let Some(message_node) = node.select(&message).next() else {
            continue;
        };
        let Some(post) = message_node.value().attr("data-post") else {
            continue;
        };
        let Some((_, id)) = post.rsplit_once('/') else {
            continue;
        };
        if id.is_empty() {
            continue;
        }
        recognized += 1;
        let text_node = message_node.select(&text).next();
        let content = text_node
            .map(text_with_breaks)
            .unwrap_or_default()
            .trim()
            .to_owned();
        let buttons: Vec<_> = node.select(&keyboard_anchor).collect();
        // Keep href positions in a separate password projection: visible labels
        // alone cannot locate adjacent codes for multiple anchor-only links.
        let password_context = text_node
            .map(|node| project_text(node, true))
            .unwrap_or_default();
        let mut links = extract_links(&password_context);
        let body_anchors = text_node.into_iter().flat_map(|node| node.select(&anchor));
        for node in body_anchors.chain(buttons.iter().copied()) {
            let Some(href) = node.value().attr("href") else {
                continue;
            };
            let mut link = Link::new(href);
            if link.cloud_type == crate::core::CloudType::Others {
                continue;
            }
            let label = node.text().collect::<Vec<_>>().join(" ");
            link.password = extract_password_for_link(&password_context, href, &label);
            let key = canonical_url_key(href);
            if let Some(existing) = links
                .iter_mut()
                .find(|known| canonical_url_key(&known.url) == key)
            {
                if crate::core::extract_password("", href).is_some() || existing.password.is_none()
                {
                    existing.password = link.password;
                    if crate::core::extract_password("", href).is_some() {
                        existing.url = link.url;
                    }
                }
            } else {
                links.push(link);
            }
        }
        if links.is_empty() {
            continue;
        }
        let button_text = buttons
            .iter()
            .map(|node| node.text().collect::<Vec<_>>().join(" "))
            .collect::<Vec<_>>()
            .join("\n");
        let content = if content.is_empty() {
            button_text
        } else {
            content
        };
        let title: String = content
            .lines()
            .find_map(message_title_line)
            .unwrap_or_else(|| channel.to_owned())
            .chars()
            .take(160)
            .collect();
        associate_work_titles(&mut links, &content, Some(&title));
        let datetime = message_node
            .select(&time)
            .next()
            .and_then(|node| node.value().attr("datetime"))
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.with_timezone(&Utc));
        if datetime.is_some() {
            dated += 1;
        }
        let mut images: Vec<String> = message_node
            .select(&image)
            .filter_map(|node| {
                node.value()
                    .attr("style")?
                    .split("url('")
                    .nth(1)?
                    .split("')")
                    .next()
                    .map(str::to_owned)
            })
            .collect();
        for src in message_node
            .select(&inline_image)
            .filter_map(|node| node.value().attr("src"))
        {
            if !images.iter().any(|known| known == src) {
                images.push(src.to_owned());
            }
        }
        let tags = text_node
            .into_iter()
            .flat_map(|node| node.select(&tag))
            .filter_map(|node| {
                node.text()
                    .collect::<String>()
                    .strip_prefix('#')
                    .map(str::to_owned)
            })
            .collect();
        results.push(SearchResult {
            id: format!("{channel}_{id}"),
            source: Source::telegram(channel),
            datetime,
            title,
            content,
            links,
            tags,
            images,
        });
    }
    if wraps > 0 && recognized == 0 {
        tracing::warn!(
            channel,
            "Telegram message structure changed: no recognizable posts"
        );
    } else if !results.is_empty() && dated == 0 {
        tracing::warn!(
            channel,
            "Telegram message dates missing; retaining parsed links"
        );
    }
    Ok(results)
}

fn text_with_breaks(node: scraper::ElementRef<'_>) -> String {
    project_text(node, false)
}

fn project_text(node: scraper::ElementRef<'_>, include_links: bool) -> String {
    let mut text = String::new();
    for child in node.children() {
        match child.value() {
            scraper::Node::Text(value) => text.push_str(value),
            scraper::Node::Element(value) if value.name() == "br" => text.push('\n'),
            scraper::Node::Element(_) => {
                if let Some(element) = scraper::ElementRef::wrap(child) {
                    if include_links
                        && element.value().name() == "a"
                        && let Some(href) = element.value().attr("href")
                    {
                        text.push_str(href);
                        text.push(' ');
                        let label = text_with_breaks(element);
                        if !label.contains("http") {
                            text.push_str(&label);
                        }
                    } else {
                        text.push_str(&project_text(element, include_links));
                    }
                }
            }
            _ => {}
        }
    }
    text
}

fn selector(value: &str) -> Result<Selector, ProviderError> {
    Selector::parse(value).map_err(|_| ProviderError::Parse(format!("invalid selector: {value}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path, query_param},
    };

    #[test]
    fn anchor_only_links_keep_their_adjacent_codes_and_explicit_url_wins() {
        let html = r#"<div class="tgme_widget_message_wrap"><div class="tgme_widget_message" data-post="demo/1"><div class="tgme_widget_message_text">名称：作品<br><a href="https://pan.baidu.com/s/firstlink">百度</a> 提取码 a1b2<br><a href="https://drive.uc.cn/s/secondlink">UC</a> 提取码 c3d4<br><a href="https://alipan.com/s/thirdlink">阿里</a><br>https://pan.quark.cn/s/duplicate 提取码 guess1</div></div><div class="tgme_widget_message_inline_keyboard"><a href="https://pan.quark.cn/s/duplicate?pwd=real12">下载</a></div></div>"#;
        let results = parse_telegram(html, "demo").unwrap();
        let links = &results[0].links;
        assert_eq!(links.len(), 4);
        assert_eq!(links[0].password.as_deref(), Some("a1b2"));
        assert_eq!(links[1].password.as_deref(), Some("c3d4"));
        assert_eq!(links[2].password, None);
        assert_eq!(links[3].password.as_deref(), Some("real12"));
        assert!(links[3].url.contains("pwd=real12"));
    }

    #[test]
    fn sibling_buttons_dom_dates_and_password_boundaries() {
        let html = r#"<div class="tgme_widget_message_wrap"><div class="tgme_widget_message" data-post="demo/1"><div class="tgme_widget_message_text">📅 9月9日<br>#影视<br>名称：<b>兰香</b>如故 &amp; 番外<!-- ignored --><br>简介：目标关键词<br>https://pan.baidu.com/s/firstlink<br>提取码 1234<br>https://drive.uc.cn/s/abc123 提取码 uc99<br>https://alipan.com/s/nopassword</div><a class="tgme_widget_message_date"><time datetime="2026-09-09T00:00:00Z"></time></a></div><div class="tgme_widget_message_inline_keyboard"><a href="https://pan.quark.cn/s/button?pwd=a1b2">提取码 c3d4</a><a href="https://pan.baidu.com/s/firstlink">百度</a><a>callback</a></div></div><div class="tgme_widget_message_wrap"><div class="tgme_widget_message" data-post="demo/2"></div><div class="tgme_widget_message_inline_keyboard"><a href="https://pan.baidu.com/s/withoutcode">下载</a></div></div>"#;
        let results = parse_telegram(html, "demo").unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "兰香如故 & 番外");
        assert!(results[0].datetime.is_some());
        assert!(results[0].content.contains("兰香如故 & 番外\n简介"));
        assert!(!results[0].content.contains("ignored"));
        assert_eq!(results[0].links.len(), 4);
        assert_eq!(results[0].links[0].password.as_deref(), Some("1234"));
        assert_eq!(results[0].links[1].password.as_deref(), Some("uc99"));
        assert_eq!(results[0].links[2].password, None);
        assert_eq!(results[0].links[3].password.as_deref(), Some("a1b2"));
        assert_eq!(results[1].links.len(), 1);
        assert_eq!(results[1].links[0].password, None);
    }

    #[test]
    fn parses_message_and_ignores_linkless_message() {
        let html = r#"<div class="tgme_widget_message_wrap"><div class="tgme_widget_message" data-post="demo/42"><a class="tgme_widget_message_date"><time datetime="2026-01-02T03:04:05+00:00"></time></a><div class="tgme_widget_message_text">仙逆 全集 https://pan.quark.cn/s/abc123</div></div></div><div class="tgme_widget_message_wrap"><div class="tgme_widget_message" data-post="demo/43"><div class="tgme_widget_message_text">nothing</div></div></div>"#;
        let values = parse_telegram(html, "demo").unwrap();
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].id, "demo_42");
    }

    #[test]
    fn parses_anchor_only_links_password_tags_and_images() {
        let html = include_str!("../../tests/fixtures/telegram/success.html");
        let values = parse_telegram(html, "demo").unwrap();
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].links[0].password.as_deref(), Some("a1b2"));
        assert_eq!(values[0].tags, ["动画"]);
        assert_eq!(values[0].images.len(), 2);
    }

    #[test]
    fn malformed_message_is_ignored() {
        assert!(
            parse_telegram(
                include_str!("../../tests/fixtures/telegram/malformed.html"),
                "demo"
            )
            .unwrap()
            .is_empty()
        );
    }

    #[test]
    fn parses_keyboard_links_with_local_passwords_and_body_fallback() {
        let values = parse_telegram(
            include_str!("../../tests/fixtures/telegram/keyboard_success.html"),
            "demo",
        )
        .unwrap();
        assert_eq!(values.len(), 3);
        assert_eq!(values[0].links.len(), 2);
        assert_eq!(values[0].links[0].password.as_deref(), Some("a1b2"));
        assert_eq!(values[0].links[1].password.as_deref(), Some("c3d4"));
        assert_eq!(values[1].links.len(), 1);
        assert_eq!(values[1].links[0].password.as_deref(), Some("e5f6"));
        assert!(values[1].title.contains("凡人修仙传"));
        assert_eq!(values[2].links[0].password.as_deref(), Some("g7h8"));
    }

    #[test]
    fn empty_and_malformed_keyboards_do_not_create_results() {
        for html in [
            include_str!("../../tests/fixtures/telegram/keyboard_empty.html"),
            include_str!("../../tests/fixtures/telegram/keyboard_malformed.html"),
        ] {
            assert!(parse_telegram(html, "demo").unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn search_distinguishes_invalid_transient_and_empty_channels() {
        let server = MockServer::start().await;
        for (name, response) in [
            ("missing", ResponseTemplate::new(404)),
            (
                "bot",
                ResponseTemplate::new(200).set_body_string(
                    r#"<div class="tgme_page_wrap">20 monthly users <a>Start Bot</a></div>"#,
                ),
            ),
            ("busy", ResponseTemplate::new(503)),
            (
                "empty",
                ResponseTemplate::new(200).set_body_string(
                    r#"<div class="tgme_channel_info"></div><div class="tme_no_messages_found">No posts found</div>"#,
                ),
            ),
        ] {
            Mock::given(method("GET"))
                .and(path(format!("/s/{name}")))
                .and(query_param("q", "needle"))
                .respond_with(response)
                .mount(&server)
                .await;
        }
        let source = TelegramSource::with_base_url(
            reqwest::Client::new(),
            Url::parse(&format!("{}/", server.uri())).unwrap(),
        );

        for name in ["missing", "bot"] {
            assert!(matches!(
                source.search(name, "needle").await,
                Err(ProviderError::InvalidSource(_))
            ));
        }
        assert!(matches!(
            source.search("busy", "needle").await,
            Err(ProviderError::Unavailable(_))
        ));
        assert!(source.search("empty", "needle").await.unwrap().is_empty());
    }
}
