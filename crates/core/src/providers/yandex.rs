//! Яндекс «API Поиска по организациям» (search-maps.yandex.ru, нужен ключ).

use std::time::Duration;

use serde_json::Value;

use super::query_text;
use crate::model::{Place, ProviderKind, ScanParams};
use crate::text::haversine_m;

const PAGE: usize = 50;

pub fn parse_feature(f: &Value) -> Option<Place> {
    let meta = &f["properties"]["CompanyMetaData"];
    let coords = f["geometry"]["coordinates"].as_array()?;
    let id = meta["id"].as_str()?.to_string();
    Some(Place {
        provider: Some(ProviderKind::Yandex),
        card_url: Some(format!("https://yandex.ru/maps/org/{id}/")),
        provider_id: id,
        name: meta["name"].as_str()?.to_string(),
        address: meta["address"].as_str().map(str::to_string),
        lon: coords.first()?.as_f64()?,
        lat: coords.get(1)?.as_f64()?,
        phone: meta["Phones"].as_array().and_then(|a| a.first()).and_then(|p| p["formatted"].as_str()).map(str::to_string),
        website: meta["url"].as_str().map(str::to_string),
        category: meta["Categories"].as_array().and_then(|a| a.first()).and_then(|c| c["name"].as_str()).map(str::to_string),
        rating: None,
        reviews: None,
        hours: meta["Hours"]["text"].as_str().map(str::to_string),
        business_status: None,
        ..Default::default()
    })
}

pub async fn search(api: &reqwest::Client, key: &str, p: &ScanParams) -> Result<Vec<Place>, String> {
    let q = query_text(p);
    if q.is_empty() {
        return Err("для Яндекса нужен текст запроса или пресет".into());
    }
    let dlat = p.radius_m / 111_320.0 * 2.0;
    let dlon = p.radius_m / (111_320.0 * p.center.lat.to_radians().cos().max(0.01)) * 2.0;
    let mut out: Vec<Place> = Vec::new();
    let mut skip = 0;
    while out.len() < p.max_places && skip < 1000 {
        let resp = api
            .get("https://search-maps.yandex.ru/v1/")
            .query(&[
                ("apikey", key),
                ("text", q.as_str()),
                ("lang", if super::answer_lang(p) == "ru" { "ru_RU" } else { "en_US" }),
                ("type", "biz"),
                ("ll", &format!("{:.6},{:.6}", p.center.lon, p.center.lat)),
                ("spn", &format!("{dlon:.6},{dlat:.6}")),
                ("rspn", "1"),
                ("results", &PAGE.to_string()),
                ("skip", &skip.to_string()),
            ])
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| format!("Яндекс: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            let hint = if status.as_u16() == 403 { " (ключ не подходит к «Поиску по организациям» или исчерпан лимит)" } else { "" };
            return Err(format!("Яндекс {}{hint}: {}", status.as_u16(), crate::text::truncate(&text, 200)));
        }
        let v: Value = resp.json().await.map_err(|e| format!("Яндекс: {e}"))?;
        let features = v["features"].as_array().cloned().unwrap_or_default();
        let n = features.len();
        out.extend(
            features
                .iter()
                .filter_map(parse_feature)
                .filter(|pl| haversine_m(p.center.lat, p.center.lon, pl.lat, pl.lon) <= p.radius_m),
        );
        if n < PAGE {
            break;
        }
        skip += PAGE;
    }
    out.truncate(p.max_places);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_feature() {
        let f = serde_json::json!({
            "geometry": {"coordinates": [37.6, 55.75]},
            "properties": {"CompanyMetaData": {
                "id": "123", "name": "Зуб-Мастер", "address": "Москва, Тверская, 1", "url": "http://zub.ru",
                "Phones": [{"formatted": "+7 (495) 123-45-67"}], "Hours": {"text": "ежедневно, 9:00–21:00"},
                "Categories": [{"name": "Стоматологическая клиника"}]
            }}
        });
        let p = parse_feature(&f).unwrap();
        assert_eq!((p.lat, p.lon), (55.75, 37.6));
        assert_eq!(p.card_url.as_deref(), Some("https://yandex.ru/maps/org/123/"));
        assert_eq!(p.category.as_deref(), Some("Стоматологическая клиника"));
    }
}
