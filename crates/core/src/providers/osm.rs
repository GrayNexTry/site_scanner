//! OpenStreetMap через Overpass API: бесплатно, без ключа.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use futures::stream::{FuturesUnordered, StreamExt};

use serde_json::Value;

use super::presets;
use crate::model::{Place, ProviderKind, ScanParams};
use crate::text::haversine_m;

const ENDPOINTS: &[&str] = &[
    "https://overpass-api.de/api/interpreter",
    "https://maps.mail.ru/osm/tools/overpass/api/interpreter",
    "https://overpass.private.coffee/api/interpreter",
    "https://overpass.kumi.systems/api/interpreter",
];

fn escape_regex(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if "\\.^$|?*+()[]{}".contains(c) {
            out.push('\\');
            out.push('\\');
        } else if c == '"' {
            out.push_str("\\\"");
            continue;
        }
        out.push(c);
    }
    out
}

pub fn build_query(p: &ScanParams) -> String {
    build_query_in(p, &format!("(around:{:.0},{:.6},{:.6})", p.radius_m, p.center.lat, p.center.lon))
}

/// Запрос в заданной области: `(around:…)` для круга или `(юг,запад,север,восток)` для квадрата.
/// Без ограничения числа объектов: Overpass отдаёт их в произвольном порядке, и срез отрезал бы
/// случайные точки, а не дальние. Обрезка по расстоянию — после разбора.
pub fn build_query_in(p: &ScanParams, area: &str) -> String {
    let mut parts = Vec::new();
    if let Some(preset) = p.preset.as_deref().and_then(presets::find).filter(|_| p.query.trim().is_empty()) {
        for (k, v) in preset.osm {
            parts.push(format!("nwr[\"{k}\"=\"{v}\"][\"name\"]{area};"));
        }
    } else if !p.query.trim().is_empty() {
        let q = escape_regex(p.query.trim());
        parts.push(format!(
            "nwr[\"name\"~\"{q}\",i][~\"^(amenity|shop|craft|office|healthcare|tourism|leisure)$\"~\".\"]{area};"
        ));
    } else {
        // Все организации: любые магазины, мастерские, медицина, коммерческие офисы и
        // бизнес-виды amenity/leisure/tourism.
        for k in presets::ALL_KEYS {
            parts.push(format!("nwr[\"name\"][\"{k}\"]{area};"));
        }
        parts.push(format!("nwr[\"name\"][\"office\"][\"office\"!~\"^({})$\"]{area};", presets::OFFICE_EXCLUDE));
        parts.push(format!("nwr[\"name\"][\"amenity\"~\"^({})$\"]{area};", presets::ALL_AMENITY));
        parts.push(format!("nwr[\"name\"][\"leisure\"~\"^({})$\"]{area};", presets::ALL_LEISURE));
        parts.push(format!("nwr[\"name\"][\"tourism\"~\"^({})$\"]{area};", presets::ALL_TOURISM));
    }
    let timeout = if p.radius_m > 2500.0 { 120 } else { 60 };
    format!("[out:json][timeout:{timeout}];\n(\n{}\n);\nout center tags;", parts.join("\n"))
}

/// Теги соцсетей и мессенджеров OSM и базовый адрес, если в теге только имя.
const SOCIAL_TAGS: &[(&str, &str)] = &[
    ("contact:facebook", "https://www.facebook.com/"),
    ("facebook", "https://www.facebook.com/"),
    ("contact:instagram", "https://www.instagram.com/"),
    ("instagram", "https://www.instagram.com/"),
    ("contact:vk", "https://vk.com/"),
    ("vk", "https://vk.com/"),
    ("contact:telegram", "https://t.me/"),
    ("telegram", "https://t.me/"),
    ("contact:whatsapp", "https://wa.me/"),
    ("contact:tiktok", "https://www.tiktok.com/@"),
    ("contact:youtube", "https://www.youtube.com/"),
    ("contact:twitter", "https://x.com/"),
    ("contact:linkedin", "https://www.linkedin.com/"),
    ("website:menu", ""),
    ("booking", ""),
];

fn social_url(v: &str, base: &str) -> String {
    let v = v.split(';').next().unwrap_or(v).trim();
    if v.starts_with("http://") || v.starts_with("https://") || base.is_empty() || v.contains('.') && v.contains('/') {
        v.to_string()
    } else {
        format!("{base}{}", v.trim_start_matches('@'))
    }
}

fn tag<'a>(tags: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| tags[*k].as_str()).map(str::trim).filter(|s| !s.is_empty())
}

pub fn parse(v: &Value, p: &ScanParams) -> Vec<Place> {
    let category = p.preset.as_deref().and_then(presets::find).map(|x| x.label.to_string());
    let mut out: Vec<Place> = v["elements"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let tags = &e["tags"];
            let name = tag(tags, &["name"])?.to_string();
            let lat = e["lat"].as_f64().or_else(|| e["center"]["lat"].as_f64())?;
            let lon = e["lon"].as_f64().or_else(|| e["center"]["lon"].as_f64())?;
            let ty = e["type"].as_str().unwrap_or("node");
            let id = e["id"].as_i64().unwrap_or(0);
            let street = tag(tags, &["addr:street", "addr:place"]);
            let house = tag(tags, &["addr:housenumber"]);
            let city = tag(tags, &["addr:city"]);
            // США, Британия, Франция: «484 Tompkins Avenue, Brooklyn»; Россия: «Москва, Тверская, 1»;
            // остальная Европа: «Oranienstraße 53, Berlin».
            let house_first = crate::region::house_number_first(p.country.as_deref());
            let russian = crate::region::russian_speaking(p.country.as_deref());
            let address = match (street, house) {
                (Some(s), Some(h)) => Some(match (house_first, russian, city) {
                    (true, _, Some(c)) => format!("{h} {s}, {c}"),
                    (true, _, None) => format!("{h} {s}"),
                    (false, true, Some(c)) => format!("{c}, {s}, {h}"),
                    (false, true, None) => format!("{s}, {h}"),
                    (false, false, Some(c)) => format!("{s} {h}, {c}"),
                    (false, false, None) => format!("{s} {h}"),
                }),
                (Some(s), None) => Some(s.to_string()),
                _ => tag(tags, &["addr:full"]).map(str::to_string),
            };
            let links: Vec<String> = SOCIAL_TAGS
                .iter()
                .filter_map(|(k, base)| tag(tags, &[k]).map(|v| social_url(v, base)))
                .collect();
            Some(Place {
                provider: Some(ProviderKind::Osm),
                provider_id: format!("{ty}/{id}"),
                name,
                address,
                lat,
                lon,
                phone: tag(tags, &["phone", "contact:phone", "contact:mobile", "mobile"]).map(|s| s.split(';').next().unwrap_or(s).trim().to_string()),
                website: tag(tags, &["website", "contact:website", "url", "contact:url"]).map(|s| s.split(';').next().unwrap_or(s).trim().to_string()),
                category: category.clone().or_else(|| {
                    ["amenity", "shop", "craft", "office", "healthcare", "tourism", "leisure"].iter().find_map(|k| {
                        let v = tag(tags, &[k])?;
                        Some(presets::osm_label(k, v).map(str::to_string).unwrap_or_else(|| v.replace('_', " ")))
                    })
                }),
                rating: None,
                reviews: None,
                hours: tag(tags, &["opening_hours"]).map(str::to_string),
                business_status: None,
                card_url: Some(format!("https://www.openstreetmap.org/{ty}/{id}")),
                email: tag(tags, &["email", "contact:email"]).map(|s| s.split(';').next().unwrap_or(s).trim().to_string()),
                links,
                brand: tag(tags, &["brand"]).map(str::to_string),
                wikidata: tag(tags, &["brand:wikidata", "operator:wikidata"]).map(str::to_string),
            })
        })
        .filter(|pl| haversine_m(p.center.lat, p.center.lon, pl.lat, pl.lon) <= p.radius_m * 1.05)
        .collect();
    // Ближние — первыми: если мест больше лимита скана, отрезаются дальние.
    out.sort_by(|a, b| {
        let d = |x: &Place| haversine_m(p.center.lat, p.center.lon, x.lat, x.lon);
        d(a).partial_cmp(&d(b)).unwrap_or(std::cmp::Ordering::Equal)
    });
    out.truncate(p.max_places);
    out
}

/// Короткая причина сбоя: «нет ответа», «не подключился» — без длинного текста reqwest.
fn short_err(e: &reqwest::Error) -> &'static str {
    if e.is_timeout() {
        "нет ответа"
    } else if e.is_connect() {
        "не подключился"
    } else if e.is_decode() {
        "битый ответ"
    } else {
        "обрыв соединения"
    }
}

fn host_of(ep: &str) -> &str {
    ep.trim_start_matches("https://").split('/').next().unwrap_or(ep)
}

async fn query_endpoint(api: &reqwest::Client, ep: &'static str, q: &str, timeout: Duration) -> Result<Value, String> {
    let host = host_of(ep);
    let r = api
        .post(ep)
        .form(&[("data", q)])
        .timeout(timeout)
        .send()
        .await
        .map_err(|e| format!("{host} — {}", short_err(&e)))?;
    if !r.status().is_success() {
        return Err(format!("{host} — {}", r.status().as_u16()));
    }
    let v: Value = r.json().await.map_err(|e| format!("{host} — {}", short_err(&e)))?;
    // При нехватке ресурсов Overpass отвечает 200 с remark «runtime error» и неполным списком.
    if v["remark"].as_str().is_some_and(|r| r.contains("error")) {
        return Err(format!("{host} — прервал запрос"));
    }
    Ok(v)
}

/// Запрос на все зеркала сразу; первый успешный ответ или причины отказа каждого.
async fn overpass_round(api: &reqwest::Client, q: &str, timeout: Duration) -> Result<Value, Vec<String>> {
    let mut pending: FuturesUnordered<_> = ENDPOINTS.iter().map(|ep| query_endpoint(api, ep, q, timeout)).collect();
    let mut errors = Vec::new();
    while let Some(r) = pending.next().await {
        match r {
            Ok(v) => return Ok(v),
            Err(e) => errors.push(e),
        }
    }
    Err(errors)
}

const NOMINATIM: &str = "https://nominatim.openstreetmap.org/search";
/// Nominatim отдаёт не больше 40 мест на запрос: если упёрлись, участок делится на четыре.
const NOMINATIM_CAP: usize = 40;
const NOMINATIM_MAX_DEPTH: u8 = 2;
const NOMINATIM_MAX_REQUESTS: usize = 24;

/// Прямоугольник вокруг круга поиска: (запад, юг, восток, север).
type BBox = (f64, f64, f64, f64);

fn bbox(p: &ScanParams) -> BBox {
    let dlat = p.radius_m / 111_320.0;
    let dlon = p.radius_m / (111_320.0 * p.center.lat.to_radians().cos().max(0.01));
    (p.center.lon - dlon, p.center.lat - dlat, p.center.lon + dlon, p.center.lat + dlat)
}

fn quarters((w, s, e, n): BBox) -> [BBox; 4] {
    let (mx, my) = ((w + e) / 2.0, (s + n) / 2.0);
    [(w, s, mx, my), (mx, s, e, my), (w, my, mx, n), (mx, my, e, n)]
}

/// Квадраты, покрывающие круг поиска (не больше 6×6): маленькие запросы проходят там, где
/// перегруженный Overpass отказывает большому. Углы вне круга пропускаются.
fn tiles(p: &ScanParams) -> Vec<BBox> {
    let (w, s, e, n) = bbox(p);
    let k = (p.radius_m * 2.0 / 2000.0).ceil().clamp(1.0, 6.0) as usize;
    let (dx, dy) = ((e - w) / k as f64, (n - s) / k as f64);
    let mut out = Vec::new();
    for i in 0..k {
        for j in 0..k {
            let t = (w + dx * i as f64, s + dy * j as f64, w + dx * (i + 1) as f64, s + dy * (j + 1) as f64);
            let (cx, cy) = (p.center.lon.clamp(t.0, t.2), p.center.lat.clamp(t.1, t.3));
            if haversine_m(p.center.lat, p.center.lon, cy, cx) <= p.radius_m {
                out.push(t);
            }
        }
    }
    out
}

/// Сколько OSM ищет места в сумме. Перегруженный Overpass иначе держит скан десятки минут
/// (до 36 квадратов по очереди, по 75 с на каждый); недостающее добавит «Повторить поиск».
const OSM_BUDGET: Duration = Duration::from_secs(240);
/// Таймаут большого запроса на весь круг.
const ROUND_TIMEOUT: Duration = Duration::from_secs(75);
/// Квадрат маленький: зеркало, не ответившее за это время, перегружено.
const TILE_TIMEOUT: Duration = Duration::from_secs(45);
/// Квадратов одновременно: каждый идёт на все зеркала, а они ограничивают запросы с одного IP.
const TILE_PARALLEL: usize = 2;
/// Первые квадраты не ответили ни на одном зеркале — Overpass лежит, остальные не ждём.
const TILE_GIVE_UP: usize = 4;

/// Что дали квадраты: элементы без повторов, сколько квадратов ответило, причины отказов.
struct TileHarvest {
    elements: Vec<Value>,
    answered: usize,
    errors: Vec<String>,
}

/// Квадраты по `TILE_PARALLEL` сразу, пока не наступит `deadline`; не успевшие считаются
/// неответившими. Если первые `TILE_GIVE_UP` квадратов отказали, остальные не спрашиваются.
async fn collect_tiles<F, Fut>(list: Vec<BBox>, deadline: tokio::time::Instant, fetch: F) -> TileHarvest
where
    F: Fn(BBox) -> Fut,
    Fut: std::future::Future<Output = Result<Value, Vec<String>>>,
{
    let mut out = TileHarvest { elements: Vec::new(), answered: 0, errors: Vec::new() };
    let mut seen = std::collections::HashSet::new();
    let mut failed = 0;
    let mut stream = std::pin::pin!(futures::stream::iter(list).map(fetch).buffer_unordered(TILE_PARALLEL));
    while let Ok(Some(r)) = tokio::time::timeout_at(deadline, stream.next()).await {
        match r {
            Ok(v) => {
                out.answered += 1;
                for el in v["elements"].as_array().into_iter().flatten() {
                    if seen.insert((el["type"].to_string(), el["id"].to_string())) {
                        out.elements.push(el.clone());
                    }
                }
            }
            Err(e) => {
                failed += 1;
                out.errors.extend(e);
                if out.answered == 0 && failed >= TILE_GIVE_UP {
                    break;
                }
            }
        }
    }
    out
}

/// Что спрашивать у Nominatim: `[amenity=dentist]` по тегам пресета или текст запроса.
fn nominatim_queries(p: &ScanParams) -> Vec<String> {
    if let Some(preset) = p.preset.as_deref().and_then(presets::find).filter(|_| p.query.trim().is_empty()) {
        preset.osm.iter().map(|(k, v)| format!("[{k}={v}]")).collect()
    } else if !p.query.trim().is_empty() {
        vec![p.query.trim().to_string()]
    } else {
        Vec::new()
    }
}

/// Правила Nominatim — не чаще запроса в секунду на всё приложение.
async fn nominatim_slot() {
    static LAST: OnceLock<tokio::sync::Mutex<Option<Instant>>> = OnceLock::new();
    let mut last = LAST.get_or_init(Default::default).lock().await;
    if let Some(t) = *last {
        let wait = Duration::from_millis(1100).saturating_sub(t.elapsed());
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }
    *last = Some(Instant::now());
}

async fn nominatim_box(api: &reqwest::Client, q: &str, (w, s, e, n): BBox, lang: &str) -> Result<Vec<Value>, String> {
    nominatim_slot().await;
    let viewbox = format!("{w:.6},{n:.6},{e:.6},{s:.6}");
    let r = api
        .get(NOMINATIM)
        .query(&[
            ("q", q),
            ("viewbox", viewbox.as_str()),
            ("bounded", "1"),
            ("format", "jsonv2"),
            ("limit", "40"),
            ("extratags", "1"),
            ("namedetails", "1"),
            ("addressdetails", "1"),
            ("accept-language", lang),
        ])
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("nominatim.openstreetmap.org — {}", short_err(&e)))?;
    if !r.status().is_success() {
        return Err(format!("nominatim.openstreetmap.org — {}", r.status().as_u16()));
    }
    r.json().await.map_err(|e| format!("nominatim.openstreetmap.org — {}", short_err(&e)))
}

/// Ответ Nominatim → элемент в формате Overpass, чтобы разбирать одним `parse`.
fn nominatim_element(item: &Value) -> Option<Value> {
    let lat: f64 = item["lat"].as_str()?.parse().ok()?;
    let lon: f64 = item["lon"].as_str()?.parse().ok()?;
    let mut tags = serde_json::Map::new();
    for key in ["extratags", "namedetails"] {
        if let Some(m) = item[key].as_object() {
            tags.extend(m.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
    }
    if let (Some(c), Some(t)) = (item["category"].as_str(), item["type"].as_str()) {
        tags.entry(c).or_insert_with(|| t.into());
    }
    let a = &item["address"];
    let fields: [(&str, &[&str]); 4] = [
        ("addr:street", &["road", "pedestrian", "footway", "square", "place"]),
        ("addr:housenumber", &["house_number"]),
        ("addr:city", &["city", "town", "village", "hamlet"]),
        ("addr:postcode", &["postcode"]),
    ];
    for (k, keys) in fields {
        if let Some(v) = keys.iter().find_map(|x| a[*x].as_str()) {
            tags.entry(k).or_insert_with(|| v.into());
        }
    }
    if !tags.contains_key("name") {
        tags.insert("name".into(), item["name"].as_str().filter(|n| !n.is_empty())?.into());
    }
    Some(serde_json::json!({"type": item["osm_type"], "id": item["osm_id"], "lat": lat, "lon": lon, "tags": tags}))
}

/// Запасной путь, когда Overpass лежит: поиск по категории в Nominatim с делением участка.
/// После `deadline` новых запросов не делает и отдаёт найденное.
async fn nominatim_search(api: &reqwest::Client, p: &ScanParams, deadline: tokio::time::Instant) -> Result<Vec<Value>, String> {
    let queries = nominatim_queries(p);
    if queries.is_empty() {
        return Err("без категории и названия Nominatim не ищет".into());
    }
    let lang = crate::region::accept_language(p.country.as_deref());
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    let mut requests = 0;
    let mut last_err = None;
    for q in &queries {
        let mut boxes = vec![(bbox(p), 0u8)];
        while let Some((b, depth)) = boxes.pop() {
            if requests >= NOMINATIM_MAX_REQUESTS || tokio::time::Instant::now() >= deadline {
                break;
            }
            requests += 1;
            match nominatim_box(api, q, b, &lang).await {
                Ok(list) => {
                    if list.len() >= NOMINATIM_CAP && depth < NOMINATIM_MAX_DEPTH {
                        boxes.extend(quarters(b).map(|x| (x, depth + 1)));
                    }
                    for item in &list {
                        if seen.insert((item["osm_type"].to_string(), item["osm_id"].to_string())) {
                            out.extend(nominatim_element(item));
                        }
                    }
                }
                Err(e) => last_err = Some(e),
            }
        }
    }
    match last_err {
        Some(e) if out.is_empty() => Err(e),
        _ => Ok(out),
    }
}

/// Места из OSM и пометка, если список собран запасным путём (может быть неполным).
///
/// Порядок: все зеркала Overpass сразу → для большого района по квадратам → если никто не
/// ответил, Nominatim → если и он пуст, ещё один заход на Overpass через 15 с. Всё вместе — не
/// дольше `OSM_BUDGET` (последний начатый запрос может добрать свой таймаут).
pub async fn search(api: &reqwest::Client, p: &ScanParams) -> Result<(Vec<Place>, Option<String>), String> {
    let deadline = tokio::time::Instant::now() + OSM_BUDGET;
    let q = build_query(p);
    let first = match overpass_round(api, &q, ROUND_TIMEOUT).await {
        Ok(v) => return Ok((parse(&v, p), None)),
        Err(e) => e,
    };
    // Большой район: по квадратам запросы легче и проходят на перегруженных серверах.
    if p.radius_m > 1000.0 {
        let list = tiles(p);
        let total = list.len();
        let got = collect_tiles(list, deadline, |(w, s, e, n)| {
            let q = build_query_in(p, &format!("({s:.6},{w:.6},{n:.6},{e:.6})"));
            async move { overpass_round(api, &q, TILE_TIMEOUT).await }
        })
        .await;
        if !got.elements.is_empty() {
            let missing = total - got.answered;
            let note = (missing > 0).then(|| {
                format!("Overpass перегружен: {missing} из {total} участков карты не ответили, список неполный. Позже нажмите «Повторить поиск» — недостающие компании добавятся.")
            });
            return Ok((parse(&serde_json::json!({ "elements": got.elements }), p), note));
        }
    }
    let fallback = match nominatim_search(api, p, deadline).await {
        Ok(elements) => {
            let places = parse(&serde_json::json!({ "elements": elements }), p);
            if !places.is_empty() {
                let note = format!(
                    "серверы Overpass не ответили ({}). Список собран через Nominatim: он находит не всё, \
                     до 40 мест на участок карты. Позже нажмите «Повторить поиск» — новые компании добавятся к этим.",
                    first.join(", ")
                );
                return Ok((places, Some(note)));
            }
            "Nominatim тоже ничего не нашёл".to_string()
        }
        Err(e) => format!("запасной Nominatim: {e}"),
    };
    // Последний заход — только если на него осталось время.
    let mut errors = first;
    let retry_at = tokio::time::Instant::now() + Duration::from_secs(15);
    if retry_at + Duration::from_secs(20) < deadline {
        tokio::time::sleep_until(retry_at).await;
        match tokio::time::timeout_at(deadline, overpass_round(api, &q, ROUND_TIMEOUT)).await {
            Ok(Ok(v)) => return Ok((parse(&v, p), None)),
            Ok(Err(e)) => errors = e,
            Err(_) => {}
        }
    }
    Err(format!(
        "серверы Overpass перегружены или недоступны ({}); {fallback}. Повторите поиск через пару минут \
         или добавьте источник с ключом (Google, Яндекс) в настройках.",
        errors.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::LatLon;

    fn params(preset: Option<&str>, query: &str) -> ScanParams {
        ScanParams {
            query: query.into(),
            preset: preset.map(str::to_string),
            city: None,
            center: LatLon { lat: 55.75, lon: 37.6 },
            radius_m: 1000.0,
            providers: vec![ProviderKind::Osm],
            max_places: 50,
            quick: false,
            country: Some("ru".into()),
            city_names: vec![],
        }
    }

    #[test]
    fn query_from_preset_and_text() {
        let q = build_query(&params(Some("dentist"), ""));
        assert!(q.contains("nwr[\"amenity\"=\"dentist\"][\"name\"](around:1000,55.750000,37.600000);"));
        let q = build_query(&params(None, "Зуб (центр)"));
        assert!(q.contains("\"name\"~\"Зуб \\\\(центр\\\\)\",i"));
    }

    #[test]
    fn parses_elements() {
        let v = serde_json::json!({"elements": [
            {"type": "node", "id": 1, "lat": 55.7501, "lon": 37.6001, "tags": {"name": "Зуб", "website": "zub.ru", "phone": "+7 495 1;+7 495 2", "addr:street": "Тверская", "addr:housenumber": "1"}},
            {"type": "way", "id": 2, "center": {"lat": 55.751, "lon": 37.601}, "tags": {"name": "Клык"}},
            {"type": "node", "id": 3, "lat": 56.5, "lon": 38.0, "tags": {"name": "Далеко"}},
            {"type": "node", "id": 4, "lat": 55.75, "lon": 37.6, "tags": {}}
        ]});
        let places = parse(&v, &params(Some("dentist"), ""));
        assert_eq!(places.len(), 2);
        assert_eq!(places[0].phone.as_deref(), Some("+7 495 1"));
        assert_eq!(places[0].address.as_deref(), Some("Тверская, 1"));
        assert_eq!(places[1].card_url.as_deref(), Some("https://www.openstreetmap.org/way/2"));
        assert_eq!(places[0].category.as_deref(), Some("Стоматология"));
    }

    #[test]
    fn all_mode_and_tiles() {
        let q = build_query(&params(None, ""));
        assert!(q.contains("nwr[\"name\"][\"shop\"](around:1000"));
        assert!(q.contains("[\"amenity\"~\"^(restaurant|cafe"));
        assert!(q.contains("[\"office\"!~\"^(government"));
        assert!(q.ends_with("out center tags;"), "без среза по числу: {q}");
        let mut p = params(None, "");
        p.radius_m = 5000.0;
        let t = tiles(&p);
        assert!(t.len() > 16 && t.len() <= 25, "квадратов: {}", t.len());
        p.radius_m = 20_000.0;
        assert!(tiles(&p).len() <= 36);
    }

    #[test]
    fn nominatim_fallback_shapes() {
        let item = serde_json::json!({
            "osm_type": "node", "osm_id": 3727817314u64, "lat": "53.9049288", "lon": "27.5591889",
            "category": "amenity", "type": "dentist", "name": "ТикоМед",
            "address": {"road": "Музычны завулак", "house_number": "3", "city": "Мінск"},
            "extratags": {"website": "https://tikomed.by/", "phone": "+375 29 128 60 28", "healthcare": "dentist"},
            "namedetails": {"name": "ЦікаМед", "name:ru": "ТикоМед"}
        });
        let e = nominatim_element(&item).unwrap();
        let mut p = params(Some("dentist"), "");
        p.center = LatLon { lat: 53.9005, lon: 27.5583 };
        p.country = Some("by".into());
        let places = parse(&serde_json::json!({ "elements": [e] }), &p);
        assert_eq!(places.len(), 1);
        assert_eq!(places[0].name, "ЦікаМед");
        assert_eq!(places[0].website.as_deref(), Some("https://tikomed.by/"));
        assert_eq!(places[0].address.as_deref(), Some("Мінск, Музычны завулак, 3"));
        assert_eq!(places[0].provider_id, "node/3727817314");
        assert_eq!(nominatim_queries(&p), vec!["[amenity=dentist]", "[healthcare=dentist]"]);
        assert_eq!(nominatim_queries(&params(None, "Зуб")), vec!["Зуб"]);
        assert!(nominatim_queries(&params(None, "")).is_empty());
        let b = bbox(&p);
        let q = quarters(b);
        assert!((q[0].0 - b.0).abs() < 1e-9 && (q[3].2 - b.2).abs() < 1e-9 && (q[0].3 - q[3].1).abs() < 1e-9);
    }

    fn boxes(n: usize) -> Vec<BBox> {
        (0..n).map(|i| (i as f64, 0.0, i as f64 + 1.0, 1.0)).collect()
    }

    /// Медленный Overpass (минута на квадрат) не держит скан дольше бюджета: отдаётся то, что
    /// успело прийти, остальное — «не ответили».
    #[tokio::test(start_paused = true)]
    async fn tiles_stop_at_deadline() {
        let start = tokio::time::Instant::now();
        let got = collect_tiles(boxes(32), start + OSM_BUDGET, |b| async move {
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok(serde_json::json!({"elements": [{"type": "node", "id": b.0 as i64}, {"type": "node", "id": -1}]}))
        })
        .await;
        assert!(start.elapsed() <= OSM_BUDGET, "ждали {:?}", start.elapsed());
        assert!(got.answered >= 6 && got.answered < 32, "ответило квадратов: {}", got.answered);
        // Общий для всех квадратов элемент — один раз.
        assert_eq!(got.elements.len(), got.answered + 1);
    }

    /// Overpass лежит: после первых отказов остальные квадраты не спрашиваются.
    #[tokio::test(start_paused = true)]
    async fn tiles_give_up_when_overpass_is_down() {
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let got = collect_tiles(boxes(32), tokio::time::Instant::now() + OSM_BUDGET, |_| {
            calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            async {
                tokio::time::sleep(TILE_TIMEOUT).await;
                Err(vec!["overpass-api.de — нет ответа".to_string()])
            }
        })
        .await;
        assert_eq!(got.answered, 0);
        assert!(got.elements.is_empty() && !got.errors.is_empty());
        let n = calls.load(std::sync::atomic::Ordering::Relaxed);
        assert!(n <= TILE_GIVE_UP + TILE_PARALLEL, "спросили квадратов: {n}");
    }

    /// Живой запрос к Nominatim (запасной путь, когда Overpass лежит). Запуск:
    /// `cargo test -p scanner-core nominatim_live -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn nominatim_live() {
        let api = reqwest::Client::builder().user_agent("maps-seo-scanner/0.1 (local tool)").build().unwrap();
        let mut p = params(Some("dentist"), "");
        p.center = LatLon { lat: 53.900535, lon: 27.558346 };
        p.country = Some("by".into());
        let far = || tokio::time::Instant::now() + Duration::from_secs(600);
        let elements = nominatim_search(&api, &p, far()).await.unwrap();
        let places = parse(&serde_json::json!({ "elements": elements }), &p);
        for pl in &places {
            println!("{} | {:?} | {:?} | {:?}", pl.name, pl.address, pl.website, pl.phone);
        }
        assert!(places.len() >= 3, "мест: {}", places.len());
        // Плотная ниша на 3 км: Nominatim упрётся в 40 и поделит участок.
        let mut big = params(Some("cafe"), "");
        big.center = p.center;
        big.radius_m = 3000.0;
        big.country = Some("by".into());
        big.max_places = 300;
        let t = std::time::Instant::now();
        let n = nominatim_search(&api, &big, far()).await.unwrap().len();
        println!("кафе в радиусе 3 км: {n} за {:?}", t.elapsed());
        assert!(n > NOMINATIM_CAP, "деление участка не сработало: {n}");
    }

    #[test]
    fn foreign_address_and_contacts() {
        let v = serde_json::json!({"elements": [
            {"type": "node", "id": 1, "lat": 55.7501, "lon": 37.6001, "tags": {"name": "Coilz", "addr:street": "Nostrand Avenue", "addr:housenumber": "624",
             "contact:instagram": "coilzsalon", "email": "hi@coilz.com", "brand": "Supercuts", "brand:wikidata": "Q7643239"}}
        ]});
        let mut p = params(Some("hairdresser"), "");
        p.country = Some("us".into());
        let places = parse(&v, &p);
        assert_eq!(places[0].address.as_deref(), Some("624 Nostrand Avenue"));
        p.country = Some("de".into());
        assert_eq!(parse(&v, &p)[0].address.as_deref(), Some("Nostrand Avenue 624"));
        assert_eq!(places[0].links, vec!["https://www.instagram.com/coilzsalon".to_string()]);
        assert_eq!(places[0].email.as_deref(), Some("hi@coilz.com"));
        assert_eq!(places[0].wikidata.as_deref(), Some("Q7643239"));
    }
}
