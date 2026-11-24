//! Google Places API (New), Text Search. Ответ — до 20 мест на страницу и до 3 страниц;
//! если квадрат «насыщен» (60 мест), делим его на 4 и ищем глубже.

use std::collections::HashSet;
use std::time::Duration;

use serde_json::{json, Value};

use super::query_text;
use crate::model::{Place, ProviderKind, ScanParams};
use crate::text::{bbox, haversine_m};

const FIELDS: &str = "places.id,places.displayName,places.formattedAddress,places.location,places.websiteUri,\
places.nationalPhoneNumber,places.internationalPhoneNumber,places.rating,places.userRatingCount,\
places.businessStatus,places.regularOpeningHours,places.googleMapsUri,places.primaryTypeDisplayName,nextPageToken";

/// Предел запросов к API на один скан (стоимость).
const MAX_REQUESTS: usize = 40;

#[derive(Clone, Copy)]
struct Rect {
    s: f64,
    w: f64,
    n: f64,
    e: f64,
}

impl Rect {
    fn quarters(self) -> [Rect; 4] {
        let (mlat, mlon) = ((self.s + self.n) / 2.0, (self.w + self.e) / 2.0);
        [
            Rect { s: self.s, w: self.w, n: mlat, e: mlon },
            Rect { s: self.s, w: mlon, n: mlat, e: self.e },
            Rect { s: mlat, w: self.w, n: self.n, e: mlon },
            Rect { s: mlat, w: mlon, n: self.n, e: self.e },
        ]
    }
}

pub fn parse_place(v: &Value) -> Option<Place> {
    Some(Place {
        provider: Some(ProviderKind::Google),
        provider_id: v["id"].as_str()?.to_string(),
        name: v["displayName"]["text"].as_str()?.to_string(),
        address: v["formattedAddress"].as_str().map(str::to_string),
        lat: v["location"]["latitude"].as_f64()?,
        lon: v["location"]["longitude"].as_f64()?,
        phone: v["internationalPhoneNumber"].as_str().or(v["nationalPhoneNumber"].as_str()).map(str::to_string),
        website: v["websiteUri"].as_str().map(str::to_string),
        category: v["primaryTypeDisplayName"]["text"].as_str().map(str::to_string),
        rating: v["rating"].as_f64(),
        reviews: v["userRatingCount"].as_u64().map(|x| x as u32),
        hours: v["regularOpeningHours"]["weekdayDescriptions"]
            .as_array()
            .map(|d| d.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join("; "))
            .filter(|s| !s.is_empty()),
        business_status: v["businessStatus"].as_str().map(str::to_string),
        card_url: v["googleMapsUri"].as_str().map(str::to_string),
        ..Default::default()
    })
}

async fn page(api: &reqwest::Client, key: &str, q: &str, lang: &str, r: Rect, token: Option<&str>) -> Result<Value, String> {
    let mut body = json!({
        "textQuery": q,
        "languageCode": lang,
        "pageSize": 20,
        "locationRestriction": {"rectangle": {
            "low": {"latitude": r.s, "longitude": r.w},
            "high": {"latitude": r.n, "longitude": r.e}
        }}
    });
    if let Some(t) = token {
        body["pageToken"] = json!(t);
    }
    let resp = api
        .post("https://places.googleapis.com/v1/places:searchText")
        .header("X-Goog-Api-Key", key)
        .header("X-Goog-FieldMask", FIELDS)
        .json(&body)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("Google Places: {e}"))?;
    let status = resp.status();
    let v: Value = resp.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        let msg = v["error"]["message"].as_str().unwrap_or("ошибка API");
        return Err(format!("Google Places {}: {msg}", status.as_u16()));
    }
    Ok(v)
}

pub async fn search(api: &reqwest::Client, key: &str, p: &ScanParams) -> Result<Vec<Place>, String> {
    let q = query_text(p);
    if q.is_empty() {
        return Err("для Google нужен текст запроса или пресет".into());
    }
    let (s, w, n, e) = bbox(p.center.lat, p.center.lon, p.radius_m);
    let mut stack = vec![(Rect { s, w, n, e }, 0u8)];
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let mut requests = 0;
    while let Some((rect, depth)) = stack.pop() {
        let mut token: Option<String> = None;
        let mut in_rect = 0;
        for _ in 0..3 {
            if requests >= MAX_REQUESTS {
                break;
            }
            requests += 1;
            let v = page(api, key, &q, super::answer_lang(p), rect, token.as_deref()).await?;
            for pl in v["places"].as_array().into_iter().flatten().filter_map(parse_place) {
                in_rect += 1;
                if haversine_m(p.center.lat, p.center.lon, pl.lat, pl.lon) <= p.radius_m && seen.insert(pl.provider_id.clone()) {
                    out.push(pl);
                }
            }
            token = v["nextPageToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
            // Токен следующей страницы активируется не сразу.
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
        if out.len() >= p.max_places || requests >= MAX_REQUESTS {
            break;
        }
        if in_rect >= 60 && depth < 2 {
            stack.extend(rect.quarters().into_iter().map(|r| (r, depth + 1)));
        }
    }
    out.truncate(p.max_places);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_place() {
        let v = json!({
            "id": "abc", "displayName": {"text": "Зуб-Мастер"}, "formattedAddress": "Москва, Тверская, 1",
            "location": {"latitude": 55.75, "longitude": 37.6}, "websiteUri": "https://zub.ru/",
            "internationalPhoneNumber": "+7 495 123-45-67", "rating": 4.7, "userRatingCount": 312,
            "businessStatus": "OPERATIONAL", "googleMapsUri": "https://maps.google.com/?cid=1",
            "regularOpeningHours": {"weekdayDescriptions": ["понедельник: 09:00–21:00"]}
        });
        let p = parse_place(&v).unwrap();
        assert_eq!(p.reviews, Some(312));
        assert_eq!(p.hours.as_deref(), Some("понедельник: 09:00–21:00"));
        assert_eq!(p.business_status.as_deref(), Some("OPERATIONAL"));
    }

    #[test]
    fn rect_quarters_cover_parent() {
        let r = Rect { s: 0.0, w: 0.0, n: 2.0, e: 2.0 };
        let q = r.quarters();
        assert_eq!((q[0].s, q[0].w, q[0].n, q[0].e), (0.0, 0.0, 1.0, 1.0));
        assert_eq!((q[3].s, q[3].w, q[3].n, q[3].e), (1.0, 1.0, 2.0, 2.0));
    }
}
