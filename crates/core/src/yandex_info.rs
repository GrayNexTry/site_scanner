//! Открытые данные Яндекса о сайте, без ключей и входа:
//! - ИКС и знаки сайта — страница webmaster.yandex.ru/siteinfo (C19, R16);
//! - угрозы по Safe Browsing Яндекса — страница yandex.ru/safety (B13).
//!
//! Это страницы для людей, а не документированный API: если разметка сменится, результат —
//! «не измерено» с причиной, а не ошибка сайта. Запросы к Яндексу идут не чаще одного
//! в `GAP` на весь процесс, чтобы большой скан не упёрся в капчу.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Mutex;

use crate::http::browser_ua;

/// Пауза между запросами к Яндексу.
const GAP: Duration = Duration::from_millis(350);
const TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Badge {
    /// Тип знака как у Яндекса: OFFICIAL, GOODS, VERTICAL_SHARE_RATING…
    pub kind: String,
    pub value: Value,
}

impl Badge {
    /// Название знака для отчёта.
    pub fn label(&self) -> String {
        let num = || self.value.as_f64().map(crate::checks::fmt_num).unwrap_or_default();
        match self.kind.as_str() {
            "OFFICIAL" => {
                let what: Vec<&str> =
                    self.value.as_array().map(|a| a.iter().filter_map(Value::as_str).map(official_kind).collect()).unwrap_or_default();
                if what.is_empty() { "Официальный сайт".into() } else { format!("Официальный сайт ({})", what.join(", ")) }
            }
            "GOODS" => "Товары в поиске".into(),
            "VERTICAL_SHARE_RATING" => format!("Рейтинг магазина {}", num()),
            "VIDEOHOST_RATING" => format!("Рейтинг видеоплатформы {}", num()),
            "SERVICES_SHARE_RATING" => format!("Рейтинг услуг {}", num()),
            "POPULAR" => "Популярный сайт".into(),
            "USERS_CHOICE" => "Выбор пользователей".into(),
            "HTTPS" => "Безопасное соединение".into(),
            other => other.to_string(),
        }
    }
}

fn official_kind(k: &str) -> &str {
    match k {
        "GOV" => "госорган",
        "CREDIT_ORGANIZATION" => "кредитная организация",
        "INSURANCE" => "страховая",
        "AIRLINE" => "авиакомпания",
        "EMBASSY" => "посольство",
        other => other,
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SiteInfo {
    /// Хост, по которому спрашивали.
    pub host: String,
    /// ИКС; 0 — ниже порога показа или сайта нет в данных Яндекса.
    pub sqi: Option<u64>,
    /// Знаки, кроме ИКС.
    pub badges: Vec<Badge>,
    pub error: Option<String>,
}

/// Не чаще одного запроса к Яндексу в `GAP` на весь процесс.
async fn wait_turn() {
    static LAST: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
    let mut last = LAST.get_or_init(|| Mutex::new(None)).lock().await;
    if let Some(t) = *last {
        let next = t + GAP;
        let now = Instant::now();
        if next > now {
            tokio::time::sleep(next - now).await;
        }
    }
    *last = Some(Instant::now());
}

fn net_error(e: &reqwest::Error) -> &'static str {
    if e.is_timeout() { "таймаут" } else { "сетевая ошибка" }
}

async fn fetch(api: &reqwest::Client, url: &str) -> Result<String, String> {
    wait_turn().await;
    let r = api
        .get(url)
        .header(reqwest::header::USER_AGENT, browser_ua())
        .header(reqwest::header::ACCEPT_LANGUAGE, "ru-RU,ru;q=0.9")
        .timeout(TIMEOUT)
        .send()
        .await
        .map_err(|e| format!("Яндекс не ответил: {}", net_error(&e)))?;
    let final_url = r.url().to_string();
    let status = r.status().as_u16();
    let body = r.text().await.map_err(|e| format!("Яндекс оборвал ответ: {}", net_error(&e)))?;
    if final_url.contains("showcaptcha") || (body.len() < 40_000 && body.to_lowercase().contains("smartcaptcha")) {
        return Err("Яндекс попросил капчу — повторите проверку позже".into());
    }
    if status != 200 {
        return Err(format!("Яндекс ответил {status}"));
    }
    Ok(body)
}

/// ИКС и знаки сайта.
pub async fn site_info(api: &reqwest::Client, host: &str) -> SiteInfo {
    let host = host.trim_start_matches("www.").to_ascii_lowercase();
    let url = format!("https://webmaster.yandex.ru/siteinfo/?host={host}");
    match fetch(api, &url).await.and_then(|b| parse_siteinfo(&b)) {
        Ok((sqi, badges)) => SiteInfo { host, sqi: Some(sqi), badges, error: None },
        Err(e) => SiteInfo { host, error: Some(e), ..Default::default() },
    }
}

/// Угрозы на сайте по данным Safe Browsing Яндекса; пустой список — угроз не найдено.
pub async fn safety(api: &reqwest::Client, url: &str) -> Result<Vec<String>, String> {
    let q: String = url::form_urlencoded::byte_serialize(url.as_bytes()).collect();
    let body = fetch(api, &format!("https://yandex.ru/safety/?url={q}")).await?;
    parse_safety(&body)
}

/// Первое JSON-значение сразу после `key` в тексте страницы.
fn json_after(html: &str, key: &str) -> Option<Value> {
    let start = html.find(key)? + key.len();
    serde_json::Deserializer::from_str(&html[start..]).into_iter::<Value>().next()?.ok()
}

/// Разбор siteinfo: объект `"quality":{"achievements":[{"type":"SQI","sqi":…}, …]}`.
pub fn parse_siteinfo(html: &str) -> Result<(u64, Vec<Badge>), String> {
    let quality = json_after(html, "\"quality\":").ok_or("в ответе Яндекса нет данных о качестве сайта")?;
    let list = quality.get("achievements").and_then(Value::as_array).ok_or("в ответе Яндекса нет знаков сайта")?;
    let mut sqi = None;
    let mut badges = Vec::new();
    for a in list {
        let kind = a.get("type").and_then(Value::as_str).unwrap_or("").to_string();
        if kind == "SQI" {
            sqi = a.get("sqi").and_then(Value::as_u64);
        } else if !kind.is_empty() {
            badges.push(Badge { kind, value: a.get("value").cloned().unwrap_or(Value::Null) });
        }
    }
    Ok((sqi.ok_or("ИКС в ответе Яндекса не найден")?, badges))
}

/// Разбор yandex.ru/safety: `"result":{"info":[…],"origin":{"info":[…]}}`.
/// Непустой `info` — найденные угрозы (у страницы и у сайта целиком).
pub fn parse_safety(html: &str) -> Result<Vec<String>, String> {
    let result = json_after(html, "\"result\":").filter(|r| r.get("info").is_some()).ok_or("ответ проверки безопасности Яндекса не разобран")?;
    let mut found: Vec<String> = Vec::new();
    for list in [result.get("info"), result.get("origin").and_then(|o| o.get("info"))] {
        for item in list.and_then(Value::as_array).into_iter().flatten() {
            let name = match item {
                Value::String(s) => s.clone(),
                Value::Object(o) => ["type", "name", "threat", "title"]
                    .iter()
                    .find_map(|k| o.get(*k).and_then(Value::as_str).map(str::to_string))
                    .unwrap_or_else(|| item.to_string()),
                other => other.to_string(),
            };
            if !found.contains(&name) {
                found.push(name);
            }
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn siteinfo_parsed() {
        let html = r#"<script>window.__X={"flags":{"A":"MAIN"},"quality":{"achievementView":"FULL","attentionSeen":false,"achievements":[{"sqi":177000,"valueType":"numeric","type":"SQI"},{"valueType":"numeric","value":4.4,"type":"VERTICAL_SHARE_RATING"},{"valueType":"official","value":["GOV"],"type":"OFFICIAL"}],"hostName":"ozon.ru"},"noticesCounter":{}}</script>"#;
        let (sqi, badges) = parse_siteinfo(html).unwrap();
        assert_eq!(sqi, 177000);
        assert_eq!(badges.len(), 2);
        assert_eq!(badges[0].label(), "Рейтинг магазина 4,4");
        assert_eq!(badges[1].label(), "Официальный сайт (госорган)");
        let zero = r#""quality":{"achievements":[{"sqi":0,"valueType":"numeric","type":"SQI"}],"hostName":"x.ru"}"#;
        assert_eq!(parse_siteinfo(zero).unwrap(), (0, vec![]));
        assert!(parse_siteinfo("<html>капча</html>").is_err());
    }

    #[test]
    fn safety_parsed() {
        let clean = r#"{"bundle":"index","type":"safety","result":{"info":[],"url":"http:\/\/ozon.ru"},"text":{}}"#;
        assert_eq!(parse_safety(clean).unwrap(), Vec::<String>::new());
        let page = r#""type":"safety","result":{"info":[],"url":"http:\/\/a.ru\/x","origin":{"info":[{"type":"malware"}],"url":"http:\/\/a.ru"}}"#;
        assert_eq!(parse_safety(page).unwrap(), vec!["malware".to_string()]);
        assert!(parse_safety("<html></html>").is_err());
    }
}
