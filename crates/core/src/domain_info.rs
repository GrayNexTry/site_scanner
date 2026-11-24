//! Данные домена: даты регистрации (RDAP, WHOIS для .ru/.su/.рф) и DNS-записи SPF/DMARC.

use std::time::Duration;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DomainInfo {
    pub domain: String,
    pub created: Option<String>,
    pub expires: Option<String>,
    pub days_to_expiry: Option<i64>,
    pub age_years: Option<f64>,
    /// rdap | whois
    pub source: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DnsInfo {
    /// Поддомен конструктора: DNS не в руках клиента.
    #[serde(default)]
    pub platform: bool,
    pub spf: Option<String>,
    pub dmarc: Option<String>,
    pub error: Option<String>,
}

fn date_prefix(s: &str) -> Option<NaiveDate> {
    let t = s.trim();
    NaiveDate::parse_from_str(t.get(..10)?, "%Y-%m-%d").ok()
}

fn finish(mut info: DomainInfo, created: Option<NaiveDate>, expires: Option<NaiveDate>) -> DomainInfo {
    let today = chrono::Utc::now().date_naive();
    if let Some(c) = created {
        info.created = Some(c.to_string());
        info.age_years = Some((today - c).num_days() as f64 / 365.25);
    }
    if let Some(e) = expires {
        info.expires = Some(e.to_string());
        info.days_to_expiry = Some((e - today).num_days());
    }
    info
}

/// Зоны, которые обслуживает WHOIS Координационного центра (RDAP у них нет).
fn tcinet_zone(domain: &str) -> bool {
    domain.ends_with(".ru") || domain.ends_with(".su") || domain.ends_with(".xn--p1ai")
}

pub async fn lookup(api: &reqwest::Client, domain: &str) -> DomainInfo {
    let base = DomainInfo { domain: domain.to_string(), ..Default::default() };
    if tcinet_zone(domain) {
        return match whois_tcinet(domain).await {
            Ok(text) => {
                let (c, e) = parse_tcinet(&text);
                if c.is_none() && e.is_none() {
                    return DomainInfo { error: Some("WHOIS не вернул дат".into()), ..base };
                }
                finish(DomainInfo { source: Some("whois".into()), ..base }, c, e)
            }
            Err(err) => DomainInfo { error: Some(err), ..base },
        };
    }
    match rdap(api, domain).await {
        Ok((c, e)) => finish(DomainInfo { source: Some("rdap".into()), ..base }, c, e),
        Err(err) => DomainInfo { error: Some(err), ..base },
    }
}

async fn whois_tcinet(domain: &str) -> Result<String, String> {
    let fut = async {
        let mut s = TcpStream::connect(("whois.tcinet.ru", 43)).await.map_err(|e| e.to_string())?;
        s.write_all(format!("{domain}\r\n").as_bytes()).await.map_err(|e| e.to_string())?;
        let mut buf = Vec::new();
        s.read_to_end(&mut buf).await.map_err(|e| e.to_string())?;
        Ok::<_, String>(String::from_utf8_lossy(&buf).into_owned())
    };
    timeout(Duration::from_secs(10), fut).await.map_err(|_| "таймаут WHOIS".to_string())?
}

pub fn parse_tcinet(text: &str) -> (Option<NaiveDate>, Option<NaiveDate>) {
    let mut created = None;
    let mut expires = None;
    for line in text.lines() {
        let l = line.trim();
        if let Some(v) = l.strip_prefix("created:") {
            created = date_prefix(v);
        } else if let Some(v) = l.strip_prefix("paid-till:") {
            expires = date_prefix(v);
        }
    }
    (created, expires)
}

async fn rdap(api: &reqwest::Client, domain: &str) -> Result<(Option<NaiveDate>, Option<NaiveDate>), String> {
    let v: serde_json::Value = api
        .get(format!("https://rdap.org/domain/{domain}"))
        .header("accept", "application/rdap+json")
        .timeout(Duration::from_secs(12))
        .send()
        .await
        .map_err(|e| format!("RDAP: {e}"))?
        .error_for_status()
        .map_err(|e| format!("RDAP: {e}"))?
        .json()
        .await
        .map_err(|e| format!("RDAP: {e}"))?;
    let mut created = None;
    let mut expires = None;
    for ev in v["events"].as_array().into_iter().flatten() {
        let date = ev["eventDate"].as_str().and_then(date_prefix);
        match ev["eventAction"].as_str() {
            Some("registration") => created = date,
            Some("expiration") => expires = date,
            _ => {}
        }
    }
    Ok((created, expires))
}

/// TXT-записи через DNS-over-HTTPS (Cloudflare): без отдельного резолвера.
async fn txt(api: &reqwest::Client, name: &str) -> Result<Vec<String>, String> {
    let v: serde_json::Value = api
        .get("https://cloudflare-dns.com/dns-query")
        .query(&[("name", name), ("type", "TXT")])
        .header("accept", "application/dns-json")
        .timeout(Duration::from_secs(8))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    // SERVFAIL/REFUSED у серверов домена — это «не ответили», а не «записей нет».
    if let Some(st) = v["Status"].as_i64().filter(|s| *s != 0 && *s != 3) {
        return Err(format!("DNS домена не ответил (код {st})"));
    }
    Ok(v["Answer"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|a| a["type"].as_u64() == Some(16))
        .filter_map(|a| a["data"].as_str())
        .map(|d| d.split("\" \"").collect::<String>().trim_matches('"').to_string())
        .collect())
}

pub async fn dns(api: &reqwest::Client, domain: &str) -> DnsInfo {
    let dmarc_name = format!("_dmarc.{domain}");
    let (root, dmarc) = tokio::join!(txt(api, domain), txt(api, &dmarc_name));
    let mut info = DnsInfo {
        platform: crate::text::classify_website(&format!("https://{domain}/")).0 == crate::text::SiteKind::ConstructorSubdomain,
        ..Default::default()
    };
    match root {
        Ok(list) => info.spf = list.into_iter().find(|t| t.to_ascii_lowercase().starts_with("v=spf1")),
        Err(e) => info.error = Some(e),
    }
    if let Ok(list) = dmarc {
        info.dmarc = list.into_iter().find(|t| t.to_ascii_uppercase().starts_with("V=DMARC1"));
    }
    info
}

/// SPF и DMARC сайта на поддомене (smart.minsk.by — своя почта на Яндексе): сначала записи
/// самого поддомена, DMARC — с откатом на домен организации (RFC 7489); своих нет — записи домена.
pub async fn dns_for(api: &reqwest::Client, host: &str, reg: &str) -> DnsInfo {
    let host = host.trim_start_matches("www.");
    if host.is_empty() || host == reg {
        return dns(api, reg).await;
    }
    let (own, parent) = tokio::join!(dns(api, host), dns(api, reg));
    if own.platform || own.spf.is_some() || own.dmarc.is_some() {
        DnsInfo { dmarc: own.dmarc.clone().or(parent.dmarc), ..own }
    } else {
        parent
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tcinet_whois() {
        let text = "domain:        EXAMPLE.RU\ncreated:       2010-03-15T12:00:00Z\npaid-till:     2027-03-15T21:00:00Z\n";
        let (c, e) = parse_tcinet(text);
        assert_eq!(c.unwrap().to_string(), "2010-03-15");
        assert_eq!(e.unwrap().to_string(), "2027-03-15");
    }
}
