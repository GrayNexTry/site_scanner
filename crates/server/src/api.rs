//! HTTP API (/api/*) и события прогресса (SSE).

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt};
use tower_http::compression::CompressionLayer;

use scanner_core::catalog::catalog;
use scanner_core::mini::SiteState;
use scanner_core::sales::{self, Contacts, ManualContact, OutreachInput, SavedProposal};
use scanner_core::providers::{self, presets};
use scanner_core::text::{host_of, parse_url_lenient};
use scanner_core::{Business, LatLon, MetricResult, Place, ProviderKind, ScanParams};

use crate::jobs::{spawn_business_audit, spawn_scan, Mode, Shared};

pub struct ApiError(pub StatusCode, pub String);

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

pub(crate) fn bad(msg: impl Into<String>) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, msg.into())
}

pub(crate) fn not_found() -> ApiError {
    ApiError(StatusCode::NOT_FOUND, "не найдено".into())
}

pub(crate) type ApiResult<T> = Result<T, ApiError>;

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
        .route("/leads.csv", get(leads_csv))
        .route("/businesses/{id}/activities", post(add_activity))
        .route("/activities/{aid}", delete(delete_activity))
        .route("/businesses/{id}/sent", post(mark_sent))
        .route("/businesses/{id}/proposal.pdf", get(proposal_pdf))
        .route("/businesses/{id}/audit", post(audit_business))
        .route("/businesses/{id}/report.pdf", get(report_pdf))
        .route("/audit", post(manual_audit))
        .route("/events", get(events))
        .route("/geocode", get(geocode))
        .route("/reverse", get(reverse))
        .route("/net", get(net))
        .merge(crate::ai::routes())
        .merge(crate::mail::routes());
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
        // Проблема простыми словами по номеру метрики — заголовки в отчёте для клиента.
        "labels": catalog()
            .metrics
            .iter()
            .filter_map(|m| scanner_core::mini::label(&m.id).map(|l| (m.id.clone(), l)))
            .collect::<std::collections::BTreeMap<_, _>>(),
        // То же в виде «в порядке» — для пройденных пунктов отчёта.
        "labels_ok": catalog()
            .metrics
            .iter()
            .filter_map(|m| scanner_core::mini::label_ok(&m.id).map(|l| (m.id.clone(), l)))
            .collect::<std::collections::BTreeMap<_, _>>(),
        // Английские формулировки — для материалов клиентам из англоязычных стран (`lang: "en"`).
        "labels_en": catalog()
            .metrics
            .iter()
            .filter_map(|m| scanner_core::mini::label_en(&m.id).map(|l| (m.id.clone(), l)))
            .collect::<std::collections::BTreeMap<_, _>>(),
        "labels_ok_en": catalog()
            .metrics
            .iter()
            .filter_map(|m| scanner_core::mini::label_ok_en(&m.id).map(|l| (m.id.clone(), l)))
            .collect::<std::collections::BTreeMap<_, _>>(),
    }))
}

/// Адрес самого сервера (для браузера, который печатает отчёт в PDF).
static SELF_URL: std::sync::OnceLock<String> = std::sync::OnceLock::new();

pub fn set_self_url(listen: std::net::SocketAddr) {
    // Слушает все интерфейсы — к себе ходим через loopback.
    let ip = if listen.ip().is_unspecified() { std::net::IpAddr::from([127, 0, 0, 1]) } else { listen.ip() };
    let _ = SELF_URL.set(format!("http://{}", std::net::SocketAddr::new(ip, listen.port())));
}

/// Имя файла для Content-Disposition: латиница в `filename`, полное — в `filename*` (RFC 5987).
fn attachment(name: &str, ascii_fallback: &str) -> String {
    let enc: String = name
        .bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect();
    format!("attachment; filename=\"{ascii_fallback}\"; filename*=UTF-8''{enc}")
}

/// Страница `/{page}/{id}` приложения в PDF — печатает headless-браузер, без колонтитулов браузера.
/// PDF страницы сканера (`report`, `proposal`) через headless Chromium. Ошибка — понятным текстом.
pub(crate) async fn render_pdf(st: &Shared, id: i64, page: &str) -> Result<Vec<u8>, String> {
    let browser = {
        let s = st.settings.read().await;
        scanner_core::render::find_browser(Some(s.browser_path.as_str()).filter(|p| !p.trim().is_empty()))
    }
    .ok_or_else(|| "Нет браузера для PDF: поставьте Chromium (pacman -S chromium) или укажите путь в настройках".to_string())?;
    let base = SELF_URL.get().cloned().unwrap_or_else(|| "http://127.0.0.1:8787".into());
    scanner_core::render::print_pdf(&browser, &format!("{base}/{page}/{id}"), Duration::from_secs(60)).await.map_err(|e| format!("PDF не собрался: {e}"))
}

/// Домен сайта компании для имени файла.
pub(crate) fn file_host(b: &crate::db::BusinessRow) -> Option<String> {
    b.site.get("final_url").and_then(|v| v.as_str()).or(b.website.as_deref()).and_then(parse_url_lenient).map(|u| host_of(&u)).filter(|h| !h.is_empty())
}

async fn page_pdf(st: &Shared, id: i64, page: &str, title: &str, ascii_prefix: &str) -> ApiResult<Response> {
    let b = st.db.get_business(id).await?.ok_or_else(not_found)?;
    let pdf = render_pdf(st, id, page).await.map_err(|e| if e.starts_with("Нет браузера") { bad(format!("{e}. Пока можно «Печать» → «Сохранить как PDF»")) } else { ApiError(StatusCode::BAD_GATEWAY, e) })?;
    let host = file_host(&b);
    let name = format!("{title} {}.pdf", host.clone().unwrap_or_else(|| b.name.clone())).replace(['/', '\\', '"'], " ");
    let ascii = format!("{ascii_prefix}-{}.pdf", host.filter(|h| h.is_ascii()).unwrap_or_else(|| id.to_string()));
    Ok(([(header::CONTENT_TYPE, "application/pdf".to_string()), (header::CONTENT_DISPOSITION, attachment(&name, &ascii))], pdf).into_response())
}

/// Отчёт для клиента в PDF.
async fn report_pdf(State(st): State<Shared>, Path(id): Path<i64>) -> ApiResult<Response> {
    page_pdf(&st, id, "report", "Аудит", "audit").await
}

/// Коммерческое предложение в PDF (страница `/proposal/{id}`).
async fn proposal_pdf(State(st): State<Shared>, Path(id): Path<i64>) -> ApiResult<Response> {
    page_pdf(&st, id, "proposal", "КП", "proposal").await
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

/// Валюта по умолчанию для лида: по стране скана (BYN, GBP, USD, EUR…), иначе из настроек.
fn scan_country(scan: Option<&crate::db::ScanRow>) -> Option<String> {
    scan.and_then(|sc| serde_json::from_value::<ScanParams>(sc.params.clone()).ok()).and_then(|p| p.country)
}

async fn default_currency(st: &Shared, scan_id: i64) -> String {
    let country = scan_country(st.db.get_scan(scan_id).await.ok().flatten().as_ref());
    sales::currency_for_country(country.as_deref(), &st.settings.read().await.currency)
}

/// Всё о компании для первого сообщения: параметры скана, карточка, результаты проверки, профиль менеджера.
pub(crate) struct LeadCtx {
    pub params: Option<ScanParams>,
    /// Название города — из city_names; «city» часто район («Запад»), поэтому он последним.
    pub cities: Vec<String>,
    pub card: Business,
    pub results: Vec<MetricResult>,
    pub state: SiteState,
    /// Домен клиента — из карточки, а не конечный адрес после перенаправления (у мёртвого сайта это страница хостера).
    pub host: Option<String>,
    pub name: String,
    pub category: Option<String>,
    pub manager: sales::Manager,
    /// Проверка прошла и мини-отчёт есть — можно писать.
    pub ready: bool,
}

impl LeadCtx {
    pub fn new(b: &crate::db::BusinessRow, scan: Option<&crate::db::ScanRow>, s: &crate::settings::Settings) -> Self {
        let params: Option<ScanParams> = scan.and_then(|sc| serde_json::from_value(sc.params.clone()).ok());
        let cities: Vec<String> = params.as_ref().map(|p| p.city_names.iter().chain(p.city.iter()).cloned().collect()).unwrap_or_default();
        let card: Business = serde_json::from_value(b.data.clone()).unwrap_or_default();
        let mut results: Vec<MetricResult> = serde_json::from_value(b.results.clone()).unwrap_or_default();
        fill_maps(&mut results, &card);
        let mini = &b.report["mini"];
        let state: SiteState = serde_json::from_value(mini["state"].clone()).unwrap_or_default();
        let host = sales::client_host(state, b.website.as_deref(), b.site_url.as_deref(), b.site.get("final_url").and_then(|v| v.as_str()));
        Self {
            params,
            cities,
            card,
            results,
            state,
            host,
            name: client_name(b),
            category: b.category.clone(),
            manager: s.manager(),
            ready: b.audit_status == "done" && !mini.is_null(),
        }
    }

    pub fn country(&self) -> Option<&str> {
        self.params.as_ref().and_then(|p| p.country.as_deref())
    }

    pub fn input(&self) -> OutreachInput<'_> {
        OutreachInput {
            business: &self.name,
            category: self.category.as_deref(),
            preset: self.params.as_ref().and_then(|p| p.preset.as_deref()),
            country: self.country(),
            cities: &self.cities,
            host: self.host.as_deref(),
            state: self.state,
            results: &self.results,
            manager: &self.manager,
        }
    }
}

/// Контакты компании: карточки, сайт (если сайт её и настоящий), добавленные вручную.
pub(crate) fn lead_contacts(b: &crate::db::BusinessRow, ctx: &LeadCtx) -> Vec<sales::Contact> {
    // Заглушка, мёртвый, чужой или неподтверждённый сайт: его контакты и соцсети принадлежат хостеру или другой компании.
    let site: Contacts = if sales::site_contacts_trusted(ctx.state, &ctx.results) { serde_json::from_value(b.site["contacts"].clone()).unwrap_or_default() } else { Contacts::default() };
    let manual: Vec<ManualContact> = serde_json::from_value(b.extra_contacts.clone()).unwrap_or_default();
    sales::contact_list(&ctx.card, &site, &manual, ctx.country())
}

/// Всё, что менеджеру нужно к карточке лида: контакты, собранное КП, первое сообщение и язык материалов (`lang`).
async fn sales_view(st: &Shared, b: &crate::db::BusinessRow, scan: Option<&crate::db::ScanRow>) -> (Value, Value, Value, &'static str) {
    let s = st.settings.read().await.clone();
    let ctx = LeadCtx::new(b, scan, &s);
    let country = ctx.country();
    let lang = sales::outreach_lang(country, &ctx.cities);
    let contacts = lead_contacts(b, &ctx);
    // Валюта КП: у лида заданная, иначе по стране скана. Цены — из прайса этой валюты (курсом не пересчитываются).
    let currency = b.deal_currency.clone().filter(|c| !c.is_empty()).unwrap_or_else(|| sales::currency_for_country(country, &s.currency));
    let (prices, mismatch) = sales::price_list_for(&currency, &s.currency, &s.price_list, &s.price_lists);
    let mut proposal = json!(sales::build_proposal(&ctx.results, ctx.state, &prices, &SavedProposal::from_value(&b.proposal), lang));
    proposal["currency"] = json!(currency);
    // true — для этой валюты своих цен нет, показаны цены из настроек как есть: менеджеру стоит проверить суммы.
    proposal["prices_unverified"] = json!(mismatch);
    let outreach = if ctx.ready { sales::outreach(&ctx.input()) } else { None };
    (json!(contacts), proposal, json!(outreach), lang)
}

/// Результаты, сохранённые до появления `maps`: карту раздела A берём из источников карточки.
pub(crate) fn fill_maps(results: &mut [MetricResult], card: &Business) {
    if let Some(maps) = scanner_core::checks::open_maps_label(card) {
        for r in results.iter_mut().filter(|r| r.id.starts_with('A') && r.maps.is_none()) {
            r.maps = Some(maps.clone());
        }
    }
}

/// Название компании для документов клиенту: заданное вручную, иначе из карточки.
pub(crate) fn client_name(b: &crate::db::BusinessRow) -> String {
    b.display_name.as_deref().map(str::trim).filter(|n| !n.is_empty()).unwrap_or(&b.name).to_string()
}

/// Адрес для документов клиенту: заданный вручную, иначе из карточки.
pub(crate) fn client_address(b: &crate::db::BusinessRow) -> Option<String> {
    b.display_address.as_deref().map(str::trim).filter(|a| !a.is_empty()).or(b.address.as_deref().map(str::trim).filter(|a| !a.is_empty())).map(str::to_string)
}

async fn get_business(State(st): State<Shared>, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let b = st.db.get_business(id).await?.ok_or_else(not_found)?;
    let scan = st.db.get_scan(b.scan_id).await?;
    let (contacts, proposal, outreach, lang) = sales_view(&st, &b, scan.as_ref()).await;
    let activities = st.db.list_activities(id).await?;
    let messages = st.db.list_messages(id).await?;
    // Формулировка по фактическому результату (статус и значение), а не общая по метрике: у каждого результата
    // `plain` (по-русски) и `plain_en`; для пройденных — «в порядке».
    let mut biz = json!(b);
    let mut parsed: Vec<MetricResult> = serde_json::from_value(b.results.clone()).unwrap_or_default();
    fill_maps(&mut parsed, &serde_json::from_value(b.data.clone()).unwrap_or_default());
    if let Some(arr) = biz["results"].as_array_mut() {
        for (item, r) in arr.iter_mut().zip(parsed.iter()) {
            item["plain"] = json!(scanner_core::mini::plain_text_lang(r, "ru"));
            item["plain_en"] = json!(scanner_core::mini::plain_text_lang(r, "en"));
        }
    }
    let (name, address) = (client_name(&b), client_address(&b));
    // У заглушки и мёртвого сайта заголовок страницы — хостера, не компании.
    let no_site = matches!(b.report["mini"]["state"].as_str(), Some("stub" | "dead"));
    let suggestion = if no_site { None } else { sales::name_suggestion(b.site["site_name"].as_str(), b.site["title"].as_str(), &name, &b.name) };
    biz["client_name"] = json!(name);
    biz["client_address"] = json!(address);
    biz["name_suggestion"] = json!(suggestion);
    // Контактов нет совсем — менеджеру предупреждение: писать некуда, нужно искать вручную.
    let has_contacts = contacts.as_array().is_some_and(|a| !a.is_empty());
    biz["has_contacts"] = json!(has_contacts);
    // Первый телефон в международном виде — для шапок страницы компании, отчёта и КП.
    let country = scan.as_ref().and_then(|sc| sc.params["country"].as_str());
    let phone = contacts.as_array().and_then(|a| a.iter().find(|c| c["kind"] == "phone")).and_then(|c| {
        let v = c["value"].as_str()?;
        Some(if c["source"] == "manual" { v.to_string() } else { sales::phone_grouped(v, country) })
    });
    biz["client_phone"] = json!(phone);
    Ok(Json(json!({
        "has_contacts": has_contacts,
        "business": biz, "scan": scan, "contacts": contacts, "activities": activities, "proposal": proposal, "outreach": outreach, "lang": lang,
        "messages": messages,
        "client_name": name, "client_address": address, "name_suggestion": suggestion,
    })))
}

/// Отличить «поля нет» от `null` (очистить).
fn double_option<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

#[derive(Deserialize)]
struct BusinessPatch {
    lead_status: Option<String>,
    notes: Option<String>,
    /// Сайт компании вручную; "" — убрать.
    website_override: Option<String>,
    /// Цена сделки; null — убрать.
    #[serde(default, deserialize_with = "double_option")]
    deal_price: Option<Option<f64>>,
    deal_currency: Option<String>,
    /// YYYY-MM-DD; "" — убрать.
    next_action_at: Option<String>,
    next_action: Option<String>,
    contact_person: Option<String>,
    /// Название компании для документов клиенту; "" — вернуть название из карточки.
    display_name: Option<String>,
    /// Адрес для документов клиенту; "" — вернуть адрес из карточки.
    display_address: Option<String>,
    /// [{kind, value, note?}]; null — очистить.
    #[serde(default, deserialize_with = "double_option")]
    extra_contacts: Option<Option<Value>>,
    /// Правки КП; null — сбросить.
    #[serde(default, deserialize_with = "double_option")]
    proposal: Option<Option<Value>>,
}

/// «1200», «1200.5» — цена без лишних нулей.
fn price_text(x: f64) -> String {
    if x.fract() == 0.0 { format!("{x:.0}") } else { format!("{x:.2}").trim_end_matches('0').to_string() }
}

/// Сегодняшняя дата менеджера (по часам машины) плюс дни.
fn today_plus(days: i64) -> String {
    (chrono::Local::now().date_naive() + chrono::Duration::days(days)).format("%Y-%m-%d").to_string()
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
    if let Some(Some(x)) = p.deal_price
        && !(x.is_finite() && x >= 0.0)
    {
        return Err(bad("цена должна быть неотрицательным числом"));
    }
    if let Some(d) = p.next_action_at.as_deref().map(str::trim).filter(|d| !d.is_empty())
        && chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").is_err()
    {
        return Err(bad("дата следующего шага — в формате ГГГГ-ММ-ДД"));
    }
    let currency = match p.deal_currency.as_deref().map(|c| c.trim().to_uppercase()) {
        Some(c) if c.is_empty() => Some(String::new()),
        Some(c) if (3..=5).contains(&c.chars().count()) && c.chars().all(|x| x.is_alphabetic()) => Some(c),
        Some(_) => return Err(bad("валюта — код вроде BYN, RUB, USD")),
        None => None,
    };
    let extra_contacts = match p.extra_contacts {
        None => None,
        Some(None) => Some(Value::Null),
        Some(Some(v)) => {
            let list: Vec<ManualContact> = serde_json::from_value(v).map_err(|_| bad("контакты — массив {kind, value, note?}"))?;
            if list.len() > 50 {
                return Err(bad("слишком много контактов"));
            }
            let clean: Vec<ManualContact> = list
                .into_iter()
                .map(|c| ManualContact { kind: c.kind.trim().to_lowercase(), value: c.value.trim().to_string(), note: c.note.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()) })
                .filter(|c| !c.kind.is_empty() && !c.value.is_empty())
                .collect();
            Some(json!(clean))
        }
    };
    let proposal = match p.proposal {
        None => None,
        Some(None) => Some(Value::Null),
        Some(Some(v)) => {
            if !v.is_object() {
                return Err(bad("КП — объект {packages, extra, discount, note, valid_days}"));
            }
            serde_json::from_value::<SavedProposal>(v.clone()).map_err(|_| bad("неверная форма КП"))?;
            Some(v)
        }
    };
    let status_changed = p.lead_status.as_deref().filter(|s| *s != before.lead_status);
    let price_changed = p.deal_price.filter(|x| *x != before.deal_price);
    let cur_for_text = currency.clone().filter(|c| !c.is_empty()).or(before.deal_currency.clone().filter(|c| !c.is_empty())).unwrap_or_default();
    let status_text = status_changed.map(|s| format!("Статус: {} → {}", crate::db::status_label(&before.lead_status), crate::db::status_label(s)));
    let meta = crate::db::MetaPatch {
        lead_status: p.lead_status,
        notes: p.notes,
        website_override: site,
        deal_price: p.deal_price,
        deal_currency: currency,
        next_action_at: p.next_action_at.map(|d| d.trim().to_string()),
        next_action: p.next_action,
        contact_person: p.contact_person,
        extra_contacts,
        proposal,
        display_name: p.display_name,
        display_address: p.display_address,
    };
    st.db.update_business_meta(id, meta).await?;
    if let Some(t) = status_text {
        st.db.add_activity(id, "status".into(), None, Some(t), None).await?;
    }
    if let Some(new) = price_changed {
        let cur = if cur_for_text.is_empty() { default_currency(&st, before.scan_id).await } else { cur_for_text };
        let text = match (before.deal_price, new) {
            (Some(o), Some(n)) => format!("Цена: {} → {} {cur}", price_text(o), price_text(n)),
            (None, Some(n)) => format!("Цена: {} {cur}", price_text(n)),
            (_, None) => "Цена сброшена".to_string(),
        };
        st.db.add_activity(id, "price".into(), None, Some(text), None).await?;
    }
    if site_changed {
        spawn_business_audit(st.clone(), id);
    }
    st.emit_business(id).await;
    Ok(Json(json!({ "business": st.db.business_brief(id).await?, "reaudit": site_changed, "activities": st.db.list_activities(id).await? })))
}

#[derive(Deserialize)]
struct NewActivity {
    kind: String,
    channel: Option<String>,
    text: Option<String>,
    /// RFC 3339 или ГГГГ-ММ-ДД; по умолчанию — сейчас.
    at: Option<String>,
}

fn parse_at(raw: &str) -> Option<String> {
    let t = raw.trim();
    let utc = if let Ok(d) = chrono::DateTime::parse_from_rfc3339(t) {
        d.with_timezone(&chrono::Utc)
    } else {
        let d = chrono::NaiveDate::parse_from_str(t, "%Y-%m-%d").ok()?;
        d.and_hms_opt(12, 0, 0)?.and_utc()
    };
    Some(utc.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

fn clean_channel(c: Option<String>) -> Result<Option<String>, ApiError> {
    match c.map(|c| c.trim().to_lowercase()).filter(|c| !c.is_empty()) {
        Some(c) if crate::db::ACTIVITY_CHANNELS.contains(&c.as_str()) => Ok(Some(c)),
        Some(_) => Err(bad("неизвестный канал")),
        None => Ok(None),
    }
}

async fn add_activity(State(st): State<Shared>, Path(id): Path<i64>, Json(a): Json<NewActivity>) -> ApiResult<Json<Value>> {
    st.db.business_brief(id).await?.ok_or_else(not_found)?;
    let kind = a.kind.trim().to_lowercase();
    if !crate::db::ACTIVITY_KINDS.contains(&kind.as_str()) {
        return Err(bad("неизвестный вид касания"));
    }
    let channel = clean_channel(a.channel)?;
    let at = match a.at.as_deref().map(str::trim).filter(|x| !x.is_empty()) {
        Some(x) => Some(parse_at(x).ok_or_else(|| bad("время — в формате RFC 3339 или ГГГГ-ММ-ДД"))?),
        None => None,
    };
    let text = a.text.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
    let activity = st.db.add_activity(id, kind, channel, text, at).await?;
    st.emit_business(id).await;
    Ok(Json(json!({ "activity": activity, "activities": st.db.list_activities(id).await? })))
}

async fn delete_activity(State(st): State<Shared>, Path(aid): Path<i64>) -> ApiResult<Json<Value>> {
    let bid = st.db.delete_activity(aid).await?.ok_or_else(not_found)?;
    st.emit_business(bid).await;
    Ok(Json(json!({ "deleted": true, "activities": st.db.list_activities(bid).await? })))
}

#[derive(Deserialize)]
struct Sent {
    channel: Option<String>,
    text: Option<String>,
    /// Черновик нейросети, который отправили: сохранить итоговый текст и пометить отправленным.
    #[serde(default)]
    message_id: Option<i64>,
    #[serde(default)]
    subject: Option<String>,
    #[serde(default)]
    body: Option<String>,
}

/// «Отправили первое сообщение»: касание, статус «новый» → «написали», следующий шаг через 3 дня.
async fn mark_sent(State(st): State<Shared>, Path(id): Path<i64>, Json(s): Json<Sent>) -> ApiResult<Json<Value>> {
    let before = st.db.business_brief(id).await?.ok_or_else(not_found)?;
    let channel = clean_channel(s.channel)?.ok_or_else(|| bad("укажите канал отправки"))?;
    let text = s.text.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
    if let Some(mid) = s.message_id {
        let m = st.db.get_message(mid).await?.filter(|m| m.business_id == id).ok_or_else(|| bad("черновик не от этой компании"))?;
        let subject = s.subject.map(|x| x.trim().to_string()).filter(|_| m.kind == "email");
        st.db.update_message(mid, subject, s.body.map(|x| x.trim().to_string()).filter(|x| !x.is_empty()), Some("sent")).await?;
    }
    let activity = record_sent(&st, &before, channel, text).await?;
    Ok(Json(json!({ "activity": activity, "business": st.db.business_brief(id).await?, "activities": st.db.list_activities(id).await? })))
}

/// Сообщение отправлено (вручную или рассылкой): касание в журнал, «Новый» → «Написали»,
/// следующий шаг «Проверить ответ» через 3 дня, если шага ещё нет.
pub(crate) async fn record_sent(st: &Shared, before: &crate::db::BusinessRow, channel: String, text: Option<String>) -> anyhow::Result<crate::db::Activity> {
    let id = before.id;
    let activity = st.db.add_activity(id, "message".into(), Some(channel), text, None).await?;
    let mut meta = crate::db::MetaPatch::default();
    let empty = |v: &Option<String>| v.as_deref().is_none_or(|x| x.trim().is_empty());
    let (no_date, no_step) = (empty(&before.next_action_at), empty(&before.next_action));
    if no_date {
        meta.next_action_at = Some(today_plus(3));
    }
    if no_date || no_step {
        meta.next_action = Some("Проверить ответ".into());
    }
    if before.lead_status == "new" {
        meta.lead_status = Some("contacted".into());
        st.db.add_activity(id, "status".into(), None, Some(format!("Статус: {} → {}", crate::db::status_label("new"), crate::db::status_label("contacted"))), None).await?;
    }
    st.db.update_business_meta(id, meta).await?;
    st.emit_business(id).await;
    Ok(activity)
}

async fn list_leads(State(st): State<Shared>) -> ApiResult<Json<Value>> {
    Ok(Json(json!(st.db.list_leads().await?)))
}

/// Лиды в CSV для Excel: UTF-8 с BOM, разделитель «;».
async fn leads_csv(State(st): State<Shared>) -> ApiResult<Response> {
    let rows = st.db.list_leads().await?;
    let mut scan_cur: std::collections::HashMap<i64, String> = Default::default();
    let mut out = String::from("\u{feff}Компания;Статус;Следующий шаг (дата);Следующий шаг;Цена;Валюта;Контактное лицо;Телефон;Сайт;Последнее касание;Балл;Заметки;Скан\n");
    for b in rows {
        let default_cur = match scan_cur.get(&b.scan_id) {
            Some(c) => c.clone(),
            None => {
                let c = default_currency(&st, b.scan_id).await;
                scan_cur.insert(b.scan_id, c.clone());
                c
            }
        };
        let cells = [
            b.name.clone(),
            crate::db::status_label(&b.lead_status).to_string(),
            b.next_action_at.clone().unwrap_or_default(),
            b.next_action.clone().unwrap_or_default(),
            b.deal_price.map(|p| price_text(p).replace('.', ",")).unwrap_or_default(),
            b.deal_price.map(|_| b.deal_currency.clone().filter(|c| !c.is_empty()).unwrap_or_else(|| default_cur.clone())).unwrap_or_default(),
            b.contact_person.clone().unwrap_or_default(),
            b.phone.clone().unwrap_or_default(),
            b.site_url.clone().or(b.website.clone()).unwrap_or_default(),
            b.last_activity_at.clone().unwrap_or_default(),
            b.overall.map(|o| o.to_string()).unwrap_or_default(),
            b.notes.clone().unwrap_or_default().replace('\n', " "),
            b.scan_name.clone().unwrap_or_default(),
        ];
        out.push_str(&cells.iter().map(|c| csv_field(c)).collect::<Vec<_>>().join(";"));
        out.push('\n');
    }
    Ok(([(header::CONTENT_TYPE, "text/csv; charset=utf-8".to_string()), (header::CONTENT_DISPOSITION, "attachment; filename=\"leads.csv\"".to_string())], out).into_response())
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
        "proposal" => "КП отправлено",
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
        let (st, v) = call(app.clone(), "PUT", "/api/settings", Some(json!({"psi_key": "AIzaKEY0012345"}))).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(v["psi_key"]["hint"], "••••2345");
        let (_, v) = call(app, "GET", "/api/meta", None).await;
        assert_eq!(v["psi"], true);
        assert!(v["labels_en"]["B04"].as_str().unwrap().contains("HTTPS"));
        assert!(v["labels_ok_en"]["B04"].as_str().is_some());
        assert_eq!(v["labels"].as_object().unwrap().len(), v["labels_en"].as_object().unwrap().len());
    }

    /// Лондон: язык материалов en, валюта КП GBP по стране скана, прайс GBP, домен клиента из карточки.
    #[tokio::test]
    async fn london_lead_gets_english_and_pounds() {
        let db = crate::db::Db::in_memory().unwrap();
        let sid = db.insert_scan("London".into(), "map", json!({"center": {"lat": 51.5, "lon": -0.1}, "radius_m": 1000.0, "providers": ["osm"], "query": "", "country": "gb", "city_names": ["City of Westminster", "Westminster"]})).await.unwrap();
        let b = Business { name: "Mute Publishing Ltd.".into(), phone: Some("020 3287 9005".into()), website: Some("http://www.metamute.org/".into()), ..Default::default() };
        db.insert_businesses(sid, vec![b]).await.unwrap();
        let bid = db.scan_businesses(sid, false).await.unwrap()[0].0;
        let results = json!([{"id": "B04", "status": "fail", "detail": "Сайт работает без HTTPS"}]);
        let report = json!({"mini": {"state": "weak", "headline": "", "problems": [], "offer": [], "fail_ids": []}});
        db.save_audit(bid, "warm".into(), Some(25.0), 100.0, report, results, json!({"final_url": "https://www.metamute.org/", "contacts": {"emails": [], "phones": [], "links": []}}), 5, Value::Null, Some("http://www.metamute.org/".into())).await.unwrap();
        let app = router(crate::jobs::AppState::new(db).await.unwrap());
        let (_, v) = call(app, "GET", &format!("/api/businesses/{bid}"), None).await;
        assert_eq!((v["lang"].as_str(), v["proposal"]["lang"].as_str(), v["proposal"]["currency"].as_str()), (Some("en"), Some("en"), Some("GBP")));
        assert_eq!(v["proposal"]["prices_unverified"], false);
        assert_eq!(v["contacts"][0]["url"], "tel:+442032879005");
        let pk = v["proposal"]["packages"].as_array().unwrap().iter().find(|p| p["id"] == "https").unwrap();
        assert_eq!((pk["price"].as_f64(), pk["title"].as_str()), (Some(35.0), Some("HTTPS and redirects")));
        let short = v["outreach"]["short"].as_str().unwrap();
        assert!(short.contains("www.metamute.org") && !short.contains("hoster") && short.starts_with("Hello!"), "{short}");
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

    #[tokio::test]
    async fn sales_flow() {
        let db = crate::db::Db::in_memory().unwrap();
        let sid = db.insert_scan("Тест".into(), "map", json!({"center": {"lat": 1.0, "lon": 1.0}, "radius_m": 1000.0, "providers": ["osm"], "query": "", "preset": "dentist", "country": "by", "city_names": ["Мінск", "Минск"]})).await.unwrap();
        let b = Business { name: "Белодент".into(), phone: Some("+375 17 345-67-89".into()), website: Some("https://belodent.by".into()), ..Default::default() };
        db.insert_businesses(sid, vec![b]).await.unwrap();
        let bid = db.scan_businesses(sid, false).await.unwrap()[0].0;
        let results = json!([{"id": "B05", "status": "fail", "value": "no_redirect", "detail": ""}, {"id": "G01", "status": "fail", "detail": ""}]);
        let report = json!({"mini": {"state": "fair", "headline": "", "problems": [], "offer": [], "fail_ids": []}});
        db.save_audit(bid, "warm".into(), Some(60.0), 100.0, report, results, json!({"final_url": "https://belodent.by/", "contacts": {"emails": ["info@belodent.by"], "phones": [], "links": []}}), 5, Value::Null, Some("https://belodent.by".into())).await.unwrap();
        let app = router(crate::jobs::AppState::new(db).await.unwrap());

        let (st, v) = call(app.clone(), "GET", &format!("/api/businesses/{bid}"), None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(v["contacts"][0]["url"], "tel:+375173456789");
        assert_eq!(v["contacts"][1]["source"], "site");
        assert_eq!(v["proposal"]["total"], 180.0);
        assert_eq!(v["proposal"]["currency"], "BYN", "валюта по стране скана (by)");
        assert_eq!((v["lang"].as_str(), v["proposal"]["lang"].as_str()), (Some("ru"), Some("ru")));
        assert!(v["outreach"]["short"].as_str().unwrap().contains("belodent.by нашлось несколько проблем"));
        assert!(v["outreach"]["short"].as_str().unwrap().contains("«стоматолог Минск»"));

        let (st, v) = call(app.clone(), "PATCH", &format!("/api/businesses/{bid}"), Some(json!({"lead_status": "talking", "deal_price": 250, "deal_currency": "usd", "next_action_at": "2026-12-01", "next_action": "Позвонить"}))).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(v["business"]["deal_currency"], "USD");
        let texts: Vec<&str> = v["activities"].as_array().unwrap().iter().filter_map(|a| a["text"].as_str()).collect();
        assert!(texts.contains(&"Статус: Новый → Переговоры") && texts.contains(&"Цена: 250 USD"), "{texts:?}");
        let (st, _) = call(app.clone(), "PATCH", &format!("/api/businesses/{bid}"), Some(json!({"next_action_at": "завтра"}))).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        let (_, v) = call(app.clone(), "PATCH", &format!("/api/businesses/{bid}"), Some(json!({"deal_price": null, "proposal": {"discount": 10}}))).await;
        assert!(v["business"]["deal_price"].is_null());
        let (_, v) = call(app.clone(), "GET", &format!("/api/businesses/{bid}"), None).await;
        // Валюта лида теперь USD: КП берёт прайс USD (ориентиры, без пересчёта курсом): https 45 + разметка 75 − скидка 10.
        assert_eq!((v["proposal"]["currency"].as_str(), v["proposal"]["total"].as_f64()), (Some("USD"), Some(110.0)));

        let (st, v) = call(app.clone(), "POST", &format!("/api/businesses/{bid}/activities"), Some(json!({"kind": "call", "channel": "phone", "text": "ответили"}))).await;
        assert_eq!(st, StatusCode::OK);
        let aid = v["activity"]["id"].as_i64().unwrap();
        let (st, _) = call(app.clone(), "POST", &format!("/api/businesses/{bid}/activities"), Some(json!({"kind": "status"}))).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);

        // «Отправили»: статус не «новый» не трогаем, дата уже есть.
        let (_, v) = call(app.clone(), "POST", &format!("/api/businesses/{bid}/sent"), Some(json!({"channel": "telegram"}))).await;
        assert_eq!(v["business"]["lead_status"], "talking");
        assert_eq!(v["business"]["next_action_at"], "2026-12-01");
        let (st, _) = call(app.clone(), "POST", &format!("/api/businesses/{bid}/sent"), Some(json!({}))).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);

        let (_, v) = call(app.clone(), "GET", "/api/leads", None).await;
        assert_eq!(v[0]["next_action"], "Позвонить");
        assert!(v[0]["last_activity_at"].is_string());
        let (st, v) = call(app.clone(), "DELETE", &format!("/api/activities/{aid}"), None).await;
        assert_eq!((st, v["deleted"].clone()), (StatusCode::OK, json!(true)));
        let (st, _) = call(app, "DELETE", &format!("/api/activities/{aid}"), None).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
    }

    /// Круг 6: `plain`/`plain_en` по факту, ручные название и адрес, подсказка названия, контакты заглушки не берутся.
    #[tokio::test]
    async fn client_texts_plain_results_and_untrusted_contacts() {
        let db = crate::db::Db::in_memory().unwrap();
        let sid = db.insert_scan("Тест".into(), "map", json!({"center": {"lat": 1.0, "lon": 1.0}, "radius_m": 1000.0, "providers": ["osm"], "query": "", "preset": "dentist", "country": "by", "city_names": ["Минск"]})).await.unwrap();
        let b = Business { name: "Любімы стаматолаг".into(), address: Some("praspiekt Dziaržynskaha 78".into()), website: Some("https://lubimyj.by".into()), ..Default::default() };
        db.insert_businesses(sid, vec![b]).await.unwrap();
        let bid = db.scan_businesses(sid, false).await.unwrap()[0].0;
        let results = json!([
            {"id": "G02", "status": "fail", "detail": "Разметки LocalBusiness нет"},
            {"id": "B01", "status": "pass", "detail": "200 за 400 мс"},
            {"id": "H02", "status": "fail", "value": "foreign", "detail": "чужой сайт"},
            {"id": "I06", "status": "fail", "value": 0.2, "detail": "Без выполнения скриптов видна только часть текста: 155 слов из 700 — остальное ИИ-боты не прочитают"}
        ]);
        let report = json!({"mini": {"state": "fair", "headline": "Сайт средний: 60/100", "problems": [], "offer": [], "fail_ids": []}});
        let site = json!({"final_url": "https://lubimyj.by/", "title": "Любимый стоматолог — клиника", "site_name": "Любимый стоматолог", "contacts": {"emails": [], "phones": [], "links": [{"kind": "facebook", "url": "https://www.facebook.com/hover"}]}});
        db.save_audit(bid, "hot".into(), Some(20.0), 100.0, report, results, site, 5, Value::Null, Some("https://lubimyj.by".into())).await.unwrap();
        let app = router(crate::jobs::AppState::new(db).await.unwrap());

        let (_, v) = call(app.clone(), "GET", &format!("/api/businesses/{bid}"), None).await;
        let res = v["business"]["results"].as_array().unwrap();
        let by = |id: &str| res.iter().find(|r| r["id"] == id).unwrap().clone();
        assert!(by("G02")["plain"].as_str().unwrap().starts_with("В разметке сайта нет данных о компании"));
        assert_eq!(by("B01")["plain"], "Главная страница открывается без ошибок");
        assert_eq!(by("B01")["plain_en"], "The home page opens without errors");
        assert_eq!(by("I06")["plain"], "Без выполнения скриптов видна только часть текста (155 из 700 слов)");
        assert!(v["contacts"].as_array().unwrap().iter().all(|c| c["source"] != "site"), "соцсети хостера с заглушки не контакты: {}", v["contacts"]);
        assert_eq!((v["client_name"].as_str(), v["client_address"].as_str()), (Some("Любімы стаматолаг"), Some("praspiekt Dziaržynskaha 78")));
        assert_eq!(v["name_suggestion"], "Любимый стоматолог");
        assert_eq!(v["has_contacts"], false, "контактов нет — менеджеру предупреждение");

        let (_, v) = call(app.clone(), "PATCH", &format!("/api/businesses/{bid}"), Some(json!({"display_name": "Любимый стоматолог", "display_address": "проспект Дзержинского, 78"}))).await;
        assert_eq!((v["business"]["display_name"].as_str(), v["business"]["display_address"].as_str()), (Some("Любимый стоматолог"), Some("проспект Дзержинского, 78")));
        let (_, v) = call(app.clone(), "GET", &format!("/api/businesses/{bid}"), None).await;
        assert_eq!((v["client_name"].as_str(), v["client_address"].as_str()), (Some("Любимый стоматолог"), Some("проспект Дзержинского, 78")));
        assert!(v["name_suggestion"].is_null(), "задано вручную — подсказки нет");
        call(app.clone(), "PATCH", &format!("/api/businesses/{bid}"), Some(json!({"lead_status": "contacted"}))).await;
        let (_, v) = call(app.clone(), "GET", "/api/leads", None).await;
        assert_eq!(v[0]["has_contacts"], false, "в «Лидах» компания без контактов помечена");
        call(app.clone(), "PATCH", &format!("/api/businesses/{bid}"), Some(json!({"extra_contacts": [{"kind": "email", "value": "a@lubimyj.by"}]}))).await;
        let (_, v) = call(app.clone(), "GET", "/api/leads", None).await;
        assert_eq!(v[0]["has_contacts"], true);
        let (_, v) = call(app.clone(), "GET", &format!("/api/businesses/{bid}"), None).await;
        assert_eq!(v["has_contacts"], true);
        // Список компаний скана: те же контакты и значки каналов, что в «Лидах».
        let (_, v) = call(app, "GET", &format!("/api/scans/{sid}/businesses"), None).await;
        assert_eq!((v[0]["has_contacts"].clone(), v[0]["contact_kinds"].clone()), (json!(true), json!(["email"])));
    }

    #[test]
    fn csv_escaping() {
        assert_eq!(csv_field("a;b"), "\"a;b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_field("plain"), "plain");
    }
}
