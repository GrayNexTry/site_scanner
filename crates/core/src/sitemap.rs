//! Разбор sitemap.xml и индексов sitemap.

use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SitemapKind {
    UrlSet,
    Index,
    Invalid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedSitemap {
    pub kind: SitemapKind,
    /// (loc, lastmod) — страницы для urlset или дочерние sitemap для индекса.
    pub entries: Vec<(String, Option<String>)>,
}

fn re(pattern: &'static str, cell: &'static OnceLock<Regex>) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).unwrap())
}

fn tag_value(block: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = block.find(&open)? + open.len();
    let end = block[start..].find(&close)? + start;
    let v = block[start..end].trim();
    let v = v.strip_prefix("<![CDATA[").and_then(|x| x.strip_suffix("]]>")).unwrap_or(v).trim();
    Some(v.replace("&amp;", "&")).filter(|s| !s.is_empty())
}

pub fn parse(body: &str) -> ParsedSitemap {
    static URL_RE: OnceLock<Regex> = OnceLock::new();
    static SM_RE: OnceLock<Regex> = OnceLock::new();
    // Убираем префиксы пространств имён: <ns:url> → <url>.
    static NS_RE: OnceLock<Regex> = OnceLock::new();
    let cleaned = re(r"<(/?)[A-Za-z0-9]+:(url|loc|lastmod|sitemap|urlset|sitemapindex)\b", &NS_RE)
        .replace_all(body, "<$1$2");
    let lower_head: String = cleaned.chars().take(2000).collect::<String>().to_ascii_lowercase();
    let (kind, block_re) = if lower_head.contains("<sitemapindex") {
        (SitemapKind::Index, re(r"(?s)<sitemap>(.*?)</sitemap>", &SM_RE))
    } else if lower_head.contains("<urlset") {
        (SitemapKind::UrlSet, re(r"(?s)<url>(.*?)</url>", &URL_RE))
    } else {
        return ParsedSitemap { kind: SitemapKind::Invalid, entries: vec![] };
    };
    let entries = block_re
        .captures_iter(&cleaned)
        .filter_map(|c| {
            let block = c.get(1)?.as_str();
            Some((tag_value(block, "loc")?, tag_value(block, "lastmod")))
        })
        .collect();
    ParsedSitemap { kind, entries }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_urlset() {
        let xml = r#"<?xml version="1.0"?><urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
<url><loc>https://a.ru/</loc><lastmod>2026-10-01</lastmod></url>
<url><loc><![CDATA[https://a.ru/uslugi?x=1&amp;y=2]]></loc></url></urlset>"#;
        let s = parse(xml);
        assert_eq!(s.kind, SitemapKind::UrlSet);
        assert_eq!(s.entries.len(), 2);
        assert_eq!(s.entries[0].1.as_deref(), Some("2026-10-01"));
        assert_eq!(s.entries[1].0, "https://a.ru/uslugi?x=1&y=2");
    }

    #[test]
    fn parses_index_and_invalid() {
        let xml = "<sitemapindex><sitemap><loc>https://a.ru/s1.xml</loc></sitemap></sitemapindex>";
        let s = parse(xml);
        assert_eq!(s.kind, SitemapKind::Index);
        assert_eq!(s.entries[0].0, "https://a.ru/s1.xml");
        assert_eq!(parse("<html>404</html>").kind, SitemapKind::Invalid);
    }
}
