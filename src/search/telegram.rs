use chrono::{DateTime, Utc};
use scraper::{Html, Selector};
use url::Url;

use crate::core::{
    Link, ProviderError, SearchResult, Source, associate_work_titles, canonical_url_key,
    extract_links, extract_password,
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
        let channel = normalize_channel(channel)?;
        let mut url = self
            .base_url
            .join(&format!("s/{channel}"))
            .map_err(|error| ProviderError::Protocol(error.to_string()))?;
        url.query_pairs_mut().append_pair("q", query);
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|error| ProviderError::Network(error.to_string()))?;
        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(ProviderError::RateLimited);
        }
        if !response.status().is_success() {
            return Err(ProviderError::Unavailable(format!(
                "HTTP {}",
                response.status()
            )));
        }
        let body = response
            .text()
            .await
            .map_err(|error| ProviderError::Network(error.to_string()))?;
        parse_telegram(&body, &channel)
    }
}

fn normalize_channel(channel: &str) -> Result<String, ProviderError> {
    let channel = channel.trim().trim_start_matches('@');
    if channel.is_empty()
        || !channel
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return Err(ProviderError::Protocol("invalid Telegram channel".into()));
    }
    Ok(channel.to_owned())
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
    let tag = selector("a[href^='?q=%23']")?;

    let mut results = Vec::new();
    for node in document.select(&wrap) {
        let Some(message_node) = node.select(&message).next() else {
            continue;
        };
        let Some(post) = message_node.value().attr("data-post") else {
            continue;
        };
        let Some((_, id)) = post.rsplit_once('/') else {
            continue;
        };
        let Some(text_node) = message_node.select(&text).next() else {
            continue;
        };
        let content = text_node
            .text()
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_owned();
        let mut links = extract_links(&content);
        for node in text_node.select(&anchor) {
            let Some(href) = node.value().attr("href") else {
                continue;
            };
            let mut link = Link::new(href);
            if link.cloud_type == crate::core::CloudType::Others {
                continue;
            }
            link.password = extract_password(&content, href);
            let key = canonical_url_key(href);
            if let Some(existing) = links
                .iter_mut()
                .find(|known| canonical_url_key(&known.url) == key)
            {
                if existing.password.is_none() {
                    existing.password = link.password;
                }
            } else {
                links.push(link);
            }
        }
        if links.is_empty() {
            continue;
        }
        let title: String = content
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or(channel)
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
            .select(&tag)
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
    Ok(results)
}

fn selector(value: &str) -> Result<Selector, ProviderError> {
    Selector::parse(value).map_err(|_| ProviderError::Parse(format!("invalid selector: {value}")))
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
