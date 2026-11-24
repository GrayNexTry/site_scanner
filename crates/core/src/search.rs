//! Веб-поиск сайта компании по названию и городу.
//! С ключом: Brave Search API, Yandex Search API (Yandex Cloud). Без ключа: HTML-выдача
//! DuckDuckGo и Bing — это против их правил, они быстро отвечают капчей (особенно на
//! адреса дата-центров и VPN), поэтому запросы редкие, а поисковик с капчей отключается
//! до перезапуска.

use std::collections::HashSet;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::Engine as _;
use scraper::{Html, Selector};
use serde::Serialize;
use serde_json::{json, Value};

use crate::http::browser_ua;

#[derive(Debug, Clone, Default)]
pub struct SearchConfig {
    pub brave_key: Option<String>,
    pub yandex_key: Option<String>,
    pub yandex_folder: Option<String>,
    /// HTML-выдача DuckDuckGo и Bing без ключа.
    pub free_engines: bool,
}

impl SearchConfig {
    pub fn any(&self) -> bool {
        self.brave_key.is_some() || (self.yandex_key.is_some() && self.yandex_folder.is_some()) || self.free_engines
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Hit {
    pub url: String,
    pub title: String,
    pub engine: &'static str,
}

/// Поисковик с общим ограничением частоты и памятью о капчах (один на приложение).
pub struct WebSearch {
    pub cfg: SearchConfig,
    last_free: tokio::sync::Mutex<Option<Instant>>,
    blocked: Mutex<HashSet<&'static str>>,
}

const FREE_INTERVAL: Duration = Duration::from_millis(2500);

impl WebSearch {
    pub fn new(cfg: SearchConfig) -> Self {
        Self { cfg, last_free: tokio::sync::Mutex::new(None), blocked: Mutex::new(HashSet::new()) }
    }

    pub fn blocked_engines(&self) -> Vec<&'static str> {
        self.blocked.lock().unwrap().iter().copied().collect()
    }

    fn is_blocked(&self, e: &'static str) -> bool {
        self.blocked.lock().unwrap().contains(e)
    }

    fn block(&self, e: &'static str) {
        self.blocked.lock().unwrap().insert(e);
    }

    /// Первая непустая выдача по очереди поисковиков. Ошибка — описание, почему никто не ответил.
    pub async fn search(&self, api: &reqwest::Client, q: &str, country: Option<&str>) -> Result<Vec<Hit>, String> {
        let mut notes = Vec::new();
        if let Some(k) = self.cfg.brave_key.as_deref() {
            match brave(api, k, q, country).await {
                Ok(h) if !h.is_empty() => return Ok(h),
                Ok(_) => notes.push("Brave: пусто".to_string()),
                Err(e) => notes.push(format!("Brave: {e}")),
            }
        }
        if let (Some(k), Some(folder)) = (self.cfg.yandex_key.as_deref(), self.cfg.yandex_folder.as_deref()) {
            match yandex(api, k, folder, q, country).await {
                Ok(h) if !h.is_empty() => return Ok(h),
                Ok(_) => notes.push("Яндекс: пусто".to_string()),
                Err(e) => notes.push(format!("Яндекс: {e}")),
            }
        }
        if self.cfg.free_engines {
            for engine in ["DuckDuckGo", "Bing"] {
                if self.is_blocked(engine) {
                    notes.push(format!("{engine}: капча, отключён до перезапуска"));
                    continue;
                }
                {
                    let mut last = self.last_free.lock().await;
                    if let Some(t) = *last {
                        let wait = FREE_INTERVAL.saturating_sub(t.elapsed());
                        if !wait.is_zero() {
                            tokio::time::sleep(wait).await;
                        }
                    }
                    *last = Some(Instant::now());
                }
                let r = if engine == "DuckDuckGo" { ddg(api, q, country).await } else { bing(api, q, country).await };
                match r {
                    Ok(h) if !h.is_empty() => return Ok(h),
                    Ok(_) => notes.push(format!("{engine}: пусто")),
                    Err(Blocked) => {
                        self.block(engine);
                        notes.push(format!("{engine}: капча"));
                    }
                }
            }
        }
        if notes.is_empty() {
            notes.push("поиск не настроен".into());
        }
        Err(notes.join("; "))
    }
}

/// Поисковик ответил капчей или ошибкой.
struct Blocked;

async fn ddg(api: &reqwest::Client, q: &str, country: Option<&str>) -> Result<Vec<Hit>, Blocked> {
    let kl = match country {
        Some("us") => "us-en".to_string(),
        Some("gb") => "uk-en".to_string(),
        Some(c) => format!("{c}-{}", crate::region::accept_language(Some(c)).get(..2).unwrap_or("en")),
        None => "ru-ru".to_string(),
    };
    let resp = api
        .post("https://html.duckduckgo.com/html/")
        .header("User-Agent", browser_ua())
        .form(&[("q", q), ("kl", kl.as_str())])
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|_| Blocked)?;
    if resp.status().as_u16() != 200 {
        return Err(Blocked);
    }
    let body = resp.text().await.map_err(|_| Blocked)?;
    if body.contains("anomaly-modal") || body.contains("challenge-form") {
        return Err(Blocked);
    }
    Ok(parse_ddg(&body))
}

pub fn parse_ddg(body: &str) -> Vec<Hit> {
    let doc = Html::parse_document(body);
    let sel = Selector::parse("a.result__a").unwrap();
    doc.select(&sel)
        .filter_map(|a| {
            let href = a.value().attr("href")?;
            let url = if href.contains("uddg=") {
                let full = if href.starts_with("//") { format!("https:{href}") } else { href.to_string() };
                let u = url::Url::parse(&full).ok()?;
                u.query_pairs().find(|(k, _)| k == "uddg").map(|(_, v)| v.into_owned())?
            } else {
                href.to_string()
            };
            Some(Hit { url, title: a.text().collect::<String>().trim().to_string(), engine: "DuckDuckGo" })
        })
        .filter(|h| h.url.starts_with("http") && !h.url.contains("duckduckgo.com/y.js"))
        .collect()
}

async fn bing(api: &reqwest::Client, q: &str, country: Option<&str>) -> Result<Vec<Hit>, Blocked> {
    let cc = country.unwrap_or("ru").to_uppercase();
    let resp = api
        .get("https://www.bing.com/search")
        .header("User-Agent", browser_ua())
        .header("Accept-Language", crate::region::accept_language(country))
        .query(&[("q", q), ("cc", cc.as_str())])
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|_| Blocked)?;
    if !resp.status().is_success() {
        return Err(Blocked);
    }
    let body = resp.text().await.map_err(|_| Blocked)?;
    let hits = parse_bing(&body);
    if hits.is_empty() && (body.contains("turnstile") || body.contains("challenge")) {
        return Err(Blocked);
    }
    Ok(hits)
}

pub fn parse_bing(body: &str) -> Vec<Hit> {
    let doc = Html::parse_document(body);
    let sel = Selector::parse("li.b_algo h2 a").unwrap();
    doc.select(&sel)
        .filter_map(|a| {
            let href = a.value().attr("href")?;
            let url = decode_bing(href).unwrap_or_else(|| href.to_string());
            Some(Hit { url, title: a.text().collect::<String>().trim().to_string(), engine: "Bing" })
        })
        .filter(|h| h.url.starts_with("http"))
        .collect()
}

/// Bing оборачивает ссылки: /ck/a?...&u=a1<base64url>.
fn decode_bing(href: &str) -> Option<String> {
    if !href.contains("bing.com/ck/") {
        return None;
    }
    let u = url::Url::parse(href).ok()?;
    let enc = u.query_pairs().find(|(k, _)| k == "u").map(|(_, v)| v.into_owned())?;
    let b64 = enc.strip_prefix("a1")?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(b64.trim_end_matches('=')).ok()?;
    String::from_utf8(bytes).ok()
}

async fn brave(api: &reqwest::Client, key: &str, q: &str, country: Option<&str>) -> Result<Vec<Hit>, String> {
    let mut req = api
        .get("https://api.search.brave.com/res/v1/web/search")
        .header("X-Subscription-Token", key)
        .header("Accept", "application/json")
        .query(&[("q", q), ("count", "10")]);
    if let Some(c) = country.filter(|c| c.len() == 2) {
        req = req.query(&[("country", c.to_uppercase())]);
    }
    let resp = req.timeout(Duration::from_secs(15)).send().await.map_err(|e| e.to_string())?;
    let status = resp.status();
    let v: Value = resp.json().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("{status}: {}", v["error"]["detail"].as_str().unwrap_or("")));
    }
    Ok(v["web"]["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| Some(Hit { url: r["url"].as_str()?.to_string(), title: r["title"].as_str().unwrap_or("").to_string(), engine: "Brave" }))
        .collect())
}

async fn yandex(api: &reqwest::Client, key: &str, folder: &str, q: &str, country: Option<&str>) -> Result<Vec<Hit>, String> {
    let search_type = match country {
        None | Some("ru") => "SEARCH_TYPE_RU",
        Some("tr") => "SEARCH_TYPE_TR",
        Some("kz") => "SEARCH_TYPE_KK",
        Some("by") => "SEARCH_TYPE_BE",
        Some("uz") => "SEARCH_TYPE_UZ",
        _ => "SEARCH_TYPE_COM",
    };
    let body = json!({
        "query": {"searchType": search_type, "queryText": q},
        "folderId": folder,
        "responseFormat": "FORMAT_XML",
        "groupSpec": {"groupMode": "GROUP_MODE_DEEP", "groupsOnPage": "10", "docsInGroup": "1"}
    });
    let resp = api
        .post("https://searchapi.api.cloud.yandex.net/v2/web/search")
        .header("Authorization", format!("Api-Key {key}"))
        .json(&body)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = resp.status();
    let v: Value = resp.json().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("{status}: {}", v["message"].as_str().unwrap_or("")));
    }
    let raw = v["rawData"].as_str().ok_or("нет rawData")?;
    let xml = base64::engine::general_purpose::STANDARD.decode(raw).map_err(|e| e.to_string())?;
    Ok(parse_yandex_xml(&String::from_utf8_lossy(&xml)))
}

pub fn parse_yandex_xml(xml: &str) -> Vec<Hit> {
    use std::sync::OnceLock;
    static DOC: OnceLock<regex::Regex> = OnceLock::new();
    static TAG: OnceLock<regex::Regex> = OnceLock::new();
    let doc_re = DOC.get_or_init(|| regex::Regex::new(r"(?s)<doc\b.*?</doc>").unwrap());
    let tag_re = TAG.get_or_init(|| regex::Regex::new(r"<[^>]+>").unwrap());
    doc_re
        .find_iter(xml)
        .filter_map(|m| {
            let d = m.as_str();
            let url = d.split("<url>").nth(1)?.split("</url>").next()?.trim().to_string();
            let title = d.split("<title>").nth(1).and_then(|t| t.split("</title>").next()).map(|t| tag_re.replace_all(t, "").to_string()).unwrap_or_default();
            Some(Hit { url, title, engine: "Яндекс" })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_result_pages() {
        let ddg = r#"<div class="result"><a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.gigiglambox.com%2F&amp;rut=abc">Gigi Glambox</a></div>"#;
        let h = parse_ddg(ddg);
        assert_eq!(h[0].url, "https://www.gigiglambox.com/");
        assert_eq!(h[0].title, "Gigi Glambox");
        let enc = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode("https://praxis-mueller.de/");
        let bing = format!(r#"<ol id="b_results"><li class="b_algo"><h2><a href="https://www.bing.com/ck/a?!&amp;&amp;p=x&amp;u=a1{enc}&amp;ntb=1">Praxis Müller</a></h2></li></ol>"#);
        assert_eq!(parse_bing(&bing)[0].url, "https://praxis-mueller.de/");
        let xml = "<response><results><grouping><group><doc id=\"1\"><url>https://ulybka.ru/</url><title>Стоматология <hlword>Улыбка</hlword></title></doc></group></grouping></results></response>";
        let y = parse_yandex_xml(xml);
        assert_eq!(y[0].url, "https://ulybka.ru/");
        assert_eq!(y[0].title, "Стоматология Улыбка");
    }
}
