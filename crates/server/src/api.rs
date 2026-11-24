//! HTTP API (/api/*) и события прогресса (SSE).

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt};
use tower_http::compression::CompressionLayer;

use scanner_core::catalog::catalog;
use scanner_core::providers::{self, presets};
use scanner_core::text::{host_of, parse_url_lenient};
use scanner_core::{Business, LatLon, Place, ProviderKind, ScanParams};

use crate::jobs::{spawn_business_audit, spawn_scan, Mode, Shared};

pub struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(e: E) -> Self {
        ApiError(StatusCode::INTERNAL_SERVER_ERROR, format!("{:#}", e.into()))
    }
}

fn bad(msg: impl Into<String>) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, msg.into())
}

fn not_found() -> ApiError {
    ApiError(StatusCode::NOT_FOUND, "не найдено".into())
}

type ApiResult<T> = Result<T, ApiError>;

pub fn router(state: Shared) -> Router {
    let api = Router::new()
        .route("/health", get(|| async { Json(json!({"ok": true, "version": env!("CARGO_PKG_VERSION")})) }))
        .route("/meta", get(meta))
        .route("/settings", get(get_settings).put(put_settings))
        .route("/scans", get(list_scans).post(create_scan))
        .route("/scans/{id}", get(get_scan).delete(delete_scan))
        .route("/scans/{id}/cancel", post(cancel_scan))
        .route("/scans/{id}/reaudit", post(reaudit_scan))
        .route("/scans/{id}/search", post(search_again))
        .route("/scans/{id}/overview", get(scan_overview))
        .route("/scans/{id}/rescore", post(rescore_scan))
        .route("/scans/{id}/businesses", get(list_businesses))
        .route("/scans/{id}/export.csv", get(export_csv))
        .route("/businesses/{id}", get(get_business).patch(patch_business))
        .route("/leads", get(list_leads))
        .route("/businesses/{id}/audit", post(audit_business))
        .route("/audit", post(manual_audit))
        .route("/events", get(events))
        .route("/geocode", get(geocode))
        .route("/reverse", get(reverse))
        .route("/net", get(net));
    Router::new()
        .nest("/api", api)
        .fallback(crate::assets::serve)
        .layer(CompressionLayer::new())
        .with_state(state)
}

async fn meta(State(st): State<Shared>) -> Json<Value> {
    let s = st.settings.read().await;
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "catalog": catalog(),
        "presets": presets::PRESETS,
        "providers": providers::describe(!s.google_places_key.is_empty(), !s.yandex_geosearch_key.is_empty()),
        "psi": !s.psi_key.is_empty() && s.psi_enabled,
        "browser": scanner_core::render::find_browser(Some(s.browser_path.as_str()).filter(|p| !p.trim().is_empty())).map(|p| p.display().to_string()),
    }))
}

async fn get_settings(State(st): State<Shared>) -> Json<Value> {
    Json(st.settings.read().await.public())
}

async fn put_settings(State(st): State<Shared>, Json(patch): Json<Value>) -> ApiResult<Json<Value>> {
    let mut s = st.settings.read().await.clone();
    s.apply(&patch);
    if let Err(e) = scanner_core::http::Fetcher::new(s.fetch_config()) {
        return Err(bad(format!("{e:#}")));
    }
    st.save_settings(s.clone()).await?;
    Ok(Json(s.public()))
}

async fn list_scans(State(st): State<Shared>) -> ApiResult<Json<Value>> {
    Ok(Json(json!(st.db.list_scans().await?)))
}

/// Предел мест в одном скане: дальше таблица и проверка становятся неподъёмными.
const MAX_PLACES: usize = 20_000;

#[derive(Deserialize)]
struct NewScan {
    name: Option<String>,
    #[serde(default)]
    query: String,
    preset: Option<String>,
    city: Option<String>,
    center: LatLon,
    radius_m: f64,
    providers: Vec<ProviderKind>,
    max_places: Option<usize>,
    /// Быстрая проверка; по умолчанию — для сканов больше 300 мест.
    quick: Option<bool>,
}

async fn create_scan(State(st): State<Shared>, Json(req): Json<NewScan>) -> ApiResult<Json<Value>> {
    if req.providers.is_empty() {
        return Err(bad("выберите хотя бы один источник"));
    }
    if !(100.0..=50_000.0).contains(&req.radius_m) {
        return Err(bad("радиус — от 100 м до 50 км"));
    }
    if req.query.trim().is_empty() && req.preset.is_none() && req.providers.iter().any(|p| matches!(p, ProviderKind::Google | ProviderKind::Yandex)) {
        return Err(bad("для Google и Яндекса нужен запрос или категория"));
    }
    let (country, city_names) = locate(&st, req.center).await;
    let params = ScanParams {
        query: req.query.trim().to_string(),
        preset: req.preset.filter(|p| presets::find(p).is_some()),
        city: req.city.map(|c| c.trim().to_string()).filter(|c| !c.is_empty()),
        country,
        city_names,
        center: req.center,
        radius_m: req.radius_m,
        providers: req.providers,
        max_places: req.max_places.unwrap_or(200).clamp(1, MAX_PLACES),
        quick: req.quick.unwrap_or(req.max_places.unwrap_or(200) > 300),
    };
    // Название скана — по-русски: подпись пресета, а не запрос на языке страны.
    let label = match params.preset.as_deref().and_then(presets::find) {
        Some(p) if params.query.is_empty() => p.label.to_string(),
        _ => providers::query_text(&params),
    };
    let label = {
        let mut c = label.chars();
        c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
    };
    let name = req
        .name
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| format!("{}{}", if label.is_empty() { "Все организации".into() } else { label }, params.city.as_ref().map(|c| format!(", {c}")).unwrap_or_default()));
    let id = st.db.insert_scan(name, "map", serde_json::to_value(&params)?).await?;
    spawn_scan(st.clone(), id, Mode::Full);
    Ok(Json(json!(st.db.get_scan(id).await?)))
}

/// Страна и названия города в точке (Nominatim, без языка — местные названия + en/ru).
async fn locate(st: &Shared, c: LatLon) -> (Option<String>, Vec<String>) {
    let f = st.fetcher.read().await.clone();
    let resp = f
        .api
        .get("https://nominatim.openstreetmap.org/reverse")
        .query(&[("format", "jsonv2"), ("lat", &c.lat.to_string()), ("lon", &c.lon.to_string()), ("zoom", "10"), ("namedetails", "1")])
        .timeout(Duration::from_secs(10))
        .send()
        .await;
    let Ok(v) = (match resp {
        Ok(r) => r.json::<Value>().await,
        Err(e) => Err(e),
    }) else {
        tracing::warn!("не удалось определить страну точки");
        return (None, vec![]);
    };
    let country = v["address"]["country_code"].as_str().map(str::to_lowercase);
    let a = &v["address"];
    let nd = &v["namedetails"];
    let mut names: Vec<String> = ["city", "town", "village", "municipality"].iter().filter_map(|k| a[*k].as_str()).map(str::to_string).collect();
    names.extend(["name", "name:en", "name:ru", "name:de", "name:fr", "name:es"].iter().filter_map(|k| nd[*k].as_str()).map(str::to_string));
    // «City of Westminster, London, Greater London, England, United Kingdom»: берём всё, кроме региона и страны.
    if let Some(dn) = v["display_name"].as_str() {
        let parts: Vec<&str> = dn.split(", ").collect();
        names.extend(parts.iter().take(parts.len().saturating_sub(2)).filter(|p| !p.chars().any(|c| c.is_ascii_digit())).map(|p| p.to_string()));
    }
    let mut seen = std::collections::HashSet::new();
    names.retain(|n| seen.insert(n.to_lowercase()));
    (country, names)
}

async fn get_scan(State(st): State<Shared>, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let scan = st.db.get_scan(id).await?.ok_or_else(not_found)?;
    Ok(Json(json!({ "scan": scan, "running": st.is_running(id) })))
}

async fn delete_scan(State(st): State<Shared>, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    if st.is_running(id) {
        st.cancel(id);
        return Err(ApiError(StatusCode::CONFLICT, "скан ещё идёт — остановлен, удалите через пару секунд".into()));
    }
    Ok(Json(json!({ "deleted": st.db.delete_scan(id).await? })))
}

async fn cancel_scan(State(st): State<Shared>, Path(id): Path<i64>) -> Json<Value> {
    Json(json!({ "cancelled": st.cancel(id) }))
}

#[derive(Deserialize, Default)]
struct Reaudit {
    #[serde(default)]
    only_pending: bool,
    /// Только сайты, которые не открылись или не пустили сканер.
    #[serde(default)]
    only_unreachable: bool,
    /// Только эти компании скана — выборка для проверки качества (scripts/verify_sample.py).
    #[serde(default)]
    ids: Vec<i64>,
}

async fn reaudit_scan(State(st): State<Shared>, Path(id): Path<i64>, body: Option<Json<Reaudit>>) -> ApiResult<Json<Value>> {
    st.db.get_scan(id).await?.ok_or_else(not_found)?;
    if st.is_running(id) {
        return Err(ApiError(StatusCode::CONFLICT, "скан уже идёт".into()));
    }
    let body = body.map(|b| b.0).unwrap_or_default();
    let mode = if !body.ids.is_empty() {
        let n = st.db.mark_pending(id, body.ids).await?;
        if n == 0 {
            return Err(bad("в скане нет таких компаний"));
        }
        Mode::AuditPending
    } else if body.only_unreachable {
        Mode::AuditUnreachable
    } else if body.only_pending {
        Mode::AuditPending
    } else {
        Mode::AuditAll
    };
    spawn_scan(st.clone(), id, mode);
    Ok(Json(json!({ "started": true })))
}

/// Повторить поиск мест: новые компании добавляются к найденным, статусы и заметки не трогаются.
async fn search_again(State(st): State<Shared>, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let scan = st.db.get_scan(id).await?.ok_or_else(not_found)?;
    if scan.kind != "map" {
        return Err(ApiError(StatusCode::BAD_REQUEST, "повторный поиск — только для сканов карты".into()));
    }
    if st.is_running(id) {
        return Err(ApiError(StatusCode::CONFLICT, "скан уже идёт".into()));
    }
    spawn_scan(st.clone(), id, Mode::Search);
    Ok(Json(json!({ "started": true })))
}

/// Пересчитать оценки и мини-отчёты скана по сохранённым результатам (без сети).
async fn rescore_scan(State(st): State<Shared>, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    st.db.get_scan(id).await?.ok_or_else(not_found)?;
    let n = crate::jobs::rescore(&st, Some(id), false).await?;
    st.emit_scan(id).await;
    Ok(Json(json!({ "rescored": n })))
}

/// Мини-отчёт по району: что с сайтами у компаний и какие проблемы встречаются чаще всего.
async fn scan_overview(State(st): State<Shared>, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let scan = st.db.get_scan(id).await?.ok_or_else(not_found)?;
    let rows = st.db.scan_minis(id).await?;
    let mut states: std::collections::BTreeMap<String, usize> = Default::default();
    let mut leads: std::collections::BTreeMap<String, usize> = Default::default();
    let mut fails: std::collections::HashMap<String, usize> = Default::default();
    let (mut audited, mut scored, mut sum, mut recheckable) = (0usize, 0usize, 0.0, 0usize);
    for (status, lead, overall, mini) in &rows {
        if status == "done" {
            audited += 1;
        }
        let state = mini["state"].as_str().unwrap_or("unknown").to_string();
        // Что может ожить после смены сети: заблокированные и неработающие, кроме несуществующих доменов.
        if state == "blocked" || (state == "dead" && !mini["headline"].as_str().unwrap_or("").contains("не существует")) {
            recheckable += 1;
        }
        *states.entry(state.clone()).or_default() += 1;
        if let Some(l) = lead {
            *leads.entry(l.clone()).or_default() += 1;
        }
        if let Some(o) = overall {
            scored += 1;
            sum += o;
        }
        // Частые проблемы — среди сайтов, которые удалось проверить, без сетей.
        if overall.is_some() && state != "chain" {
            for f in mini["fail_ids"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                *fails.entry(f.to_string()).or_default() += 1;
            }
        }
    }
    let cat = scanner_core::catalog::catalog();
    let mut top: Vec<(String, usize)> = fails.into_iter().filter(|(id, _)| !id.starts_with('A')).collect();
    top.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let top: Vec<Value> = top
        .into_iter()
        .take(10)
        .map(|(id, n)| {
            let name = scanner_core::mini::label(&id).map(str::to_string).or_else(|| cat.get(&id).map(|d| d.name.clone())).unwrap_or_default();
            json!({"id": id, "label": name, "count": n, "share": if scored > 0 { (n as f64 / scored as f64 * 100.0).round() } else { 0.0 }})
        })
        .collect();
    Ok(Json(json!({
        "total": rows.len(),
        "audited": audited,
        "sites_scored": scored,
        "avg_score": (scored > 0).then(|| (sum / scored as f64).round()),
        "states": states,
        "leads": leads,
        "top_problems": top,
        "recheckable": recheckable,
        "country": scan.params["country"],
        "egress": st.egress_country().await,
    })))
}

async fn list_businesses(State(st): State<Shared>, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    Ok(Json(json!(st.db.list_businesses(id, true).await?)))
}

async fn get_business(State(st): State<Shared>, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let b = st.db.get_business(id).await?.ok_or_else(not_found)?;
    let scan = st.db.get_scan(b.scan_id).await?;
    Ok(Json(json!({ "business": b, "scan": scan })))
}

#[derive(Deserialize)]
struct BusinessPatch {
    lead_status: Option<String>,
    notes: Option<String>,
    /// Сайт компании вручную; "" — убрать.
    website_override: Option<String>,
}

async fn patch_business(State(st): State<Shared>, Path(id): Path<i64>, Json(p): Json<BusinessPatch>) -> ApiResult<Json<Value>> {
    let before = st.db.business_brief(id).await?.ok_or_else(not_found)?;
    if let Some(s) = p.lead_status.as_deref()
        && !crate::db::LEAD_STATUSES.contains(&s)
    {
        return Err(bad("неизвестный статус лида"));
    }
    let site = match p.website_override.as_deref().map(str::trim) {
        Some("") => Some(String::new()),
        Some(w) => Some(parse_url_lenient(w).ok_or_else(|| bad("некорректный адрес сайта"))?.to_string()),
        None => None,
    };
    let site_changed = site.as_ref().is_some_and(|w| Some(w.as_str()) != before.website_override.as_deref().or(Some("")));
    st.db.update_business_meta(id, p.lead_status, p.notes, site).await?;
    if site_changed {
        spawn_business_audit(st.clone(), id);
    }
    st.emit_business(id).await;
    Ok(Json(json!({ "business": st.db.business_brief(id).await?, "reaudit": site_changed })))
}

async fn list_leads(State(st): State<Shared>) -> ApiResult<Json<Value>> {
    Ok(Json(json!(st.db.list_leads().await?)))
}

async fn audit_business(State(st): State<Shared>, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    st.db.business_brief(id).await?.ok_or_else(not_found)?;
    spawn_business_audit(st.clone(), id);
    Ok(Json(json!({ "started": true })))
}

#[derive(Deserialize)]
struct ManualAudit {
    url: String,
    query: Option<String>,
    city: Option<String>,
    /// Страна для правовых проверок; по умолчанию — по доменной зоне.
    country: Option<String>,
}

/// Проверка одного сайта без карт.
async fn manual_audit(State(st): State<Shared>, Json(req): Json<ManualAudit>) -> ApiResult<Json<Value>> {
    let u = parse_url_lenient(&req.url).ok_or_else(|| bad("некорректный адрес сайта"))?;
    let host = host_of(&u);
    let params = ScanParams {
        query: req.query.unwrap_or_default(),
        preset: None,
        city: req.city.filter(|c| !c.trim().is_empty()),
        country: req.country.map(|c| c.trim().to_lowercase()).filter(|c| c.len() == 2).or_else(|| scanner_core::region::country_from_host(&host)),
        city_names: vec![],
        center: LatLon { lat: 0.0, lon: 0.0 },
        radius_m: 0.0,
        providers: vec![ProviderKind::Manual],
        max_places: 1,
        quick: false,
    };
    let id = st.db.insert_scan(format!("Сайт {host}"), "manual", serde_json::to_value(&params)?).await?;
    let place = Place { provider: Some(ProviderKind::Manual), name: host.clone(), website: Some(u.to_string()), ..Default::default() };
    let b = Business { name: host, website: Some(u.to_string()), sources: vec![place], ..Default::default() };
    st.db.insert_businesses(id, vec![b]).await?;
    st.db.refresh_progress(id).await?;
    spawn_scan(st.clone(), id, Mode::AuditAll);
    Ok(Json(json!(st.db.get_scan(id).await?)))
}

#[derive(Deserialize)]
struct EventsQuery {
    scan_id: Option<i64>,
}

async fn events(State(st): State<Shared>, Query(q): Query<EventsQuery>) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let stream = BroadcastStream::new(st.events.subscribe()).filter_map(move |ev| {
        let ev = ev.ok()?;
        if q.scan_id.is_some_and(|id| id != ev.scan_id()) {
            return None;
        }
        SseEvent::default().json_data(&ev).ok().map(Ok)
    });
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}

fn join(v: &Value, sep: &str) -> String {
    v.as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(sep)).unwrap_or_default()
}

fn csv_field(s: &str) -> String {
    if s.contains([';', '"', '\n', '\r']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() }
}

async fn export_csv(State(st): State<Shared>, Path(id): Path<i64>) -> ApiResult<Response> {
    let scan = st.db.get_scan(id).await?.ok_or_else(not_found)?;
    let rows = st.db.list_businesses(id, false).await?;
    let lead_label = |l: Option<&str>| match l {
        Some("hot") => "горячий",
        Some("warm") => "тёплый",
        Some("cold") => "холодный",
        Some("closed") => "закрыто",
        _ => "не проверен",
    };
    // BOM + «;» — чтобы Excel открыл кириллицу и колонки без настройки.
    let status_label = |s: &str| match s {
        "contacted" => "написали",
        "talking" => "переговоры",
        "client" => "клиент",
        "rejected" => "отказ",
        "skip" => "не подходит",
        _ => "новый",
    };
    let mut out = String::from("\u{feff}Название;Адрес;Телефон;Сайт в карточке;Проверенный сайт;Как найден;Категория;Рейтинг;Отзывы;Лид;Балл;Ситуация с сайтом;Главные проблемы;Что предложить;Причины;Стоп-факторы;Статус;Заметки;Карточки\n");
    for b in rows {
        let reasons = b.report["lead_reasons"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(" | ")).unwrap_or_default();
        let stops = b.report["stop_factors"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default();
        let cards = b.data["sources"].as_array().map(|a| a.iter().filter_map(|s| s["card_url"].as_str()).collect::<Vec<_>>().join(" ")).unwrap_or_default();
        let cells = [
            b.name.clone(),
            b.address.clone().unwrap_or_default(),
            b.phone.clone().unwrap_or_default(),
            b.website.clone().unwrap_or_default(),
            b.site_url.clone().unwrap_or_default(),
            b.discovery["found"]["evidence"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default(),
            b.category.clone().unwrap_or_default(),
            b.rating.map(|r| r.to_string().replace('.', ",")).unwrap_or_default(),
            b.reviews.map(|r| r.to_string()).unwrap_or_default(),
            lead_label(b.lead.as_deref()).to_string(),
            b.overall.map(|o| o.to_string()).unwrap_or_default(),
            b.report["mini"]["headline"].as_str().unwrap_or_default().to_string(),
            join(&b.report["mini"]["problems"], "; "),
            join(&b.report["mini"]["offer"], ", "),
            reasons,
            stops,
            status_label(&b.lead_status).to_string(),
            b.notes.clone().unwrap_or_default(),
            cards,
        ];
        out.push_str(&cells.iter().map(|c| csv_field(c)).collect::<Vec<_>>().join(";"));
        out.push('\n');
    }
    let fname = format!("scan-{id}.csv");
    let _ = scan;
    Ok((
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_string()),
            (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{fname}\"")),
        ],
        out,
    )
        .into_response())
}

#[derive(Deserialize)]
struct GeoQuery {
    q: String,
}

/// Поиск места через Nominatim (OpenStreetMap). Не чаще раза в секунду — интерфейс делает паузу.
async fn geocode(State(st): State<Shared>, Query(q): Query<GeoQuery>) -> ApiResult<Json<Value>> {
    let f = st.fetcher.read().await.clone();
    let v: Value = f
        .api
        .get("https://nominatim.openstreetmap.org/search")
        .query(&[("format", "jsonv2"), ("q", q.q.as_str()), ("limit", "6"), ("accept-language", "ru")])
        .timeout(Duration::from_secs(10))
        .send()
        .await?
        .json()
        .await?;
    let items: Vec<Value> = v
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|x| {
            Some(json!({
                "label": x["display_name"].as_str()?,
                "lat": x["lat"].as_str()?.parse::<f64>().ok()?,
                "lon": x["lon"].as_str()?.parse::<f64>().ok()?,
                "bbox": x["boundingbox"],
            }))
        })
        .collect();
    Ok(Json(json!(items)))
}

/// С какого IP и страны идёт скан: российские сайты часто закрыты для зарубежных IP (VPN).
#[derive(Deserialize, Default)]
struct NetQuery {
    /// `proxy` — выход через свой прокси (проверка из настроек).
    via: Option<String>,
}

/// Откуда идут запросы: IP и страна выхода (напрямую или через свой прокси).
async fn net(State(st): State<Shared>, Query(q): Query<NetQuery>) -> ApiResult<Json<Value>> {
    let base = st.fetcher.read().await.clone();
    let f = match q.via.as_deref() {
        Some("proxy") => match base.proxied() {
            Some(p) => Arc::new(p),
            None if base.is_proxied() => base,
            None => return Err(bad("прокси не задан")),
        },
        _ => base,
    };
    let text: String = f
        .api
        .get("https://www.cloudflare.com/cdn-cgi/trace")
        .timeout(Duration::from_secs(12))
        .send()
        .await
        .map_err(|e| bad(format!("нет ответа: {e}")))?
        .text()
        .await?;
    let field = |k: &str| text.lines().find_map(|l| l.strip_prefix(k).and_then(|x| x.strip_prefix('='))).map(str::to_string);
    Ok(Json(json!({ "ip": field("ip"), "country": field("loc"), "interface": f.interface() })))
}

#[derive(Deserialize)]
struct RevQuery {
    lat: f64,
    lon: f64,
}

/// Город и район по точке — подставляется в поле «город/район» скана.
async fn reverse(State(st): State<Shared>, Query(q): Query<RevQuery>) -> ApiResult<Json<Value>> {
    let f = st.fetcher.read().await.clone();
    let v: Value = f
        .api
        .get("https://nominatim.openstreetmap.org/reverse")
        .query(&[("format", "jsonv2"), ("lat", &q.lat.to_string()), ("lon", &q.lon.to_string()), ("zoom", "14"), ("accept-language", "ru")])
        .timeout(Duration::from_secs(10))
        .send()
        .await?
        .json()
        .await?;
    let a = &v["address"];
    let city = ["city", "town", "village", "municipality"].iter().find_map(|k| a[*k].as_str());
    let district = ["suburb", "city_district", "quarter", "neighbourhood"].iter().find_map(|k| a[*k].as_str());
    Ok(Json(json!({ "city": city, "district": district, "label": v["display_name"] })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn app() -> Router {
        let db = crate::db::Db::in_memory().unwrap();
        router(crate::jobs::AppState::new(db).await.unwrap())
    }

    async fn call(app: Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let req = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(body.map(|b| Body::from(b.to_string())).unwrap_or(Body::empty()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn meta_and_settings() {
        let app = app().await;
        let (st, v) = call(app.clone(), "GET", "/api/meta", None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(v["catalog"]["metrics"].as_array().unwrap().len(), 214);
        assert_eq!(v["providers"][0]["id"], "osm");
        let (st, v) = call(app.clone(), "PUT", "/api/settings", Some(json!({"psi_key": "KEY12345"}))).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(v["psi_key"]["hint"], "••••2345");
        let (_, v) = call(app, "GET", "/api/meta", None).await;
        assert_eq!(v["psi"], true);
    }

    #[tokio::test]
    async fn validates_new_scan() {
        let app = app().await;
        let (st, v) = call(app.clone(), "POST", "/api/scans", Some(json!({"center": {"lat": 55.7, "lon": 37.6}, "radius_m": 10.0, "providers": ["osm"]}))).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert!(v["error"].as_str().unwrap().contains("радиус"));
        let (st, _) = call(app, "POST", "/api/scans", Some(json!({"center": {"lat": 55.7, "lon": 37.6}, "radius_m": 1000.0, "providers": ["google"]}))).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn csv_escaping() {
        assert_eq!(csv_field("a;b"), "\"a;b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_field("plain"), "plain");
    }
}
