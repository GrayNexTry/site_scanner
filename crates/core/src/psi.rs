//! PageSpeed Insights (CrUX + Lighthouse) и Google Safe Browsing — нужны ключи Google Cloud.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PsiResult {
    /// Есть полевые данные CrUX (страница или весь сайт).
    pub field: bool,
    pub field_scope: Option<String>,
    pub lcp_ms: Option<f64>,
    pub inp_ms: Option<f64>,
    pub cls: Option<f64>,
    pub perf: Option<f64>,
    pub a11y: Option<f64>,
    pub fcp_ms: Option<f64>,
    pub tbt_ms: Option<f64>,
    pub total_bytes: Option<f64>,
    pub unused_js_bytes: Option<f64>,
    pub requests: Option<f64>,
    pub modern_images: Option<f64>,
    pub lcp_lazy: Option<f64>,
    pub render_blocking: Option<f64>,
    pub font_display: Option<f64>,
    pub third_party_ms: Option<f64>,
    pub error: Option<String>,
}

fn score(audits: &Value, ids: &[&str]) -> Option<f64> {
    ids.iter().find_map(|id| audits[*id]["score"].as_f64())
}

fn numeric(audits: &Value, id: &str) -> Option<f64> {
    audits[id]["numericValue"].as_f64()
}

pub fn parse_psi(v: &Value) -> PsiResult {
    let mut r = PsiResult::default();
    for (scope, key) in [("страница", "loadingExperience"), ("весь сайт", "originLoadingExperience")] {
        let m = &v[key]["metrics"];
        let lcp = m["LARGEST_CONTENTFUL_PAINT_MS"]["percentile"].as_f64();
        if lcp.is_some() {
            r.field = true;
            r.field_scope = Some(scope.to_string());
            r.lcp_ms = lcp;
            r.inp_ms = m["INTERACTION_TO_NEXT_PAINT"]["percentile"].as_f64();
            r.cls = m["CUMULATIVE_LAYOUT_SHIFT_SCORE"]["percentile"].as_f64().map(|x| x / 100.0);
            break;
        }
    }
    let lh = &v["lighthouseResult"];
    r.perf = lh["categories"]["performance"]["score"].as_f64().map(|s| (s * 100.0).round());
    r.a11y = lh["categories"]["accessibility"]["score"].as_f64().map(|s| (s * 100.0).round());
    let a = &lh["audits"];
    r.fcp_ms = numeric(a, "first-contentful-paint");
    r.tbt_ms = numeric(a, "total-blocking-time");
    r.total_bytes = numeric(a, "total-byte-weight");
    r.unused_js_bytes = a["unused-javascript"]["details"]["overallSavingsBytes"].as_f64();
    r.requests = a["network-requests"]["details"]["items"].as_array().map(|x| x.len() as f64);
    r.modern_images = score(a, &["modern-image-formats", "image-delivery-insight"]);
    r.lcp_lazy = score(a, &["lcp-lazy-loaded", "lcp-discovery-insight"]);
    r.render_blocking = score(a, &["render-blocking-resources", "render-blocking-insight"]);
    r.font_display = score(a, &["font-display", "font-display-insight"]);
    r.third_party_ms = a["third-party-summary"]["details"]["summary"]["wastedMs"].as_f64();
    r
}

pub async fn run(api: &reqwest::Client, key: &str, url: &str) -> PsiResult {
    let req = api
        .get("https://www.googleapis.com/pagespeedonline/v5/runPagespeed")
        .query(&[
            ("url", url),
            ("strategy", "mobile"),
            ("category", "performance"),
            ("category", "accessibility"),
            ("key", key),
        ])
        .timeout(Duration::from_secs(120));
    match req.send().await {
        Err(e) => PsiResult { error: Some(format!("PSI: {e}")), ..Default::default() },
        Ok(resp) => {
            let status = resp.status();
            let v: Value = resp.json().await.unwrap_or(Value::Null);
            if !status.is_success() {
                let msg = v["error"]["message"].as_str().unwrap_or("ошибка API").to_string();
                return PsiResult { error: Some(format!("PSI {}: {msg}", status.as_u16())), ..Default::default() };
            }
            parse_psi(&v)
        }
    }
}

/// Совпадения в Google Safe Browsing (пусто — сайт чистый).
pub async fn safe_browsing(api: &reqwest::Client, key: &str, url: &str) -> Result<Vec<String>, String> {
    let body = serde_json::json!({
        "client": {"clientId": "maps-seo-scanner", "clientVersion": "0.1"},
        "threatInfo": {
            "threatTypes": ["MALWARE", "SOCIAL_ENGINEERING", "UNWANTED_SOFTWARE", "POTENTIALLY_HARMFUL_APPLICATION"],
            "platformTypes": ["ANY_PLATFORM"],
            "threatEntryTypes": ["URL"],
            "threatEntries": [{"url": url}]
        }
    });
    let resp = api
        .post("https://safebrowsing.googleapis.com/v4/threatMatches:find")
        .query(&[("key", key)])
        .json(&body)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("Safe Browsing {}", resp.status().as_u16()));
    }
    let v: Value = resp.json().await.map_err(|e| e.to_string())?;
    Ok(v["matches"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| m["threatType"].as_str().map(str::to_string))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_field_and_lab() {
        let v = serde_json::json!({
            "loadingExperience": {"metrics": {
                "LARGEST_CONTENTFUL_PAINT_MS": {"percentile": 2300},
                "INTERACTION_TO_NEXT_PAINT": {"percentile": 180},
                "CUMULATIVE_LAYOUT_SHIFT_SCORE": {"percentile": 5}
            }},
            "lighthouseResult": {
                "categories": {"performance": {"score": 0.42}, "accessibility": {"score": 0.91}},
                "audits": {"first-contentful-paint": {"numericValue": 1900.0}, "lcp-lazy-loaded": {"score": 0}}
            }
        });
        let r = parse_psi(&v);
        assert!(r.field);
        assert_eq!(r.lcp_ms, Some(2300.0));
        assert_eq!(r.cls, Some(0.05));
        assert_eq!(r.perf, Some(42.0));
        assert_eq!(r.lcp_lazy, Some(0.0));
    }
}
