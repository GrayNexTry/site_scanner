//! Нейросеть для писем (OpenRouter): каталог моделей, проверка ключа, предпросмотр запроса,
//! черновики писем и массовая генерация в фоне. Логика запроса и разбора ответа — в `scanner_core::llm`.

use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Semaphore;
use tracing::{info, warn};

use scanner_core::catalog::catalog;
use scanner_core::llm::{self, AiError, AiProfile, Kind, ModelInfo};
use scanner_core::{mini, sales, Status};

use crate::api::{bad, client_address, not_found, ApiError, ApiResult, LeadCtx};
use crate::db::{BusinessRow, Message};
use crate::jobs::Shared;

/// Каталог моделей живёт час: цены и список меняются редко.
const MODELS_TTL: Duration = Duration::from_secs(3600);

pub struct AiState {
    client: llm::Client,
    models: tokio::sync::Mutex<Option<(Instant, Arc<Vec<ModelInfo>>)>>,
    batch: std::sync::Mutex<Batch>,
    cancel: Arc<AtomicBool>,
}

impl AiState {
    pub fn new(base: Option<&str>) -> anyhow::Result<Self> {
        Ok(Self { client: llm::Client::new(base)?, models: Default::default(), batch: Default::default(), cancel: Default::default() })
    }
}

/// Массовая генерация: что идёт и чем закончилось.
#[derive(Debug, Clone, Serialize, Default)]
pub struct Batch {
    pub running: bool,
    pub kind: String,
    pub profile: String,
    pub model: String,
    pub total: usize,
    /// Составлено черновиков.
    pub done: usize,
    pub failed: usize,
    /// Пропущено: черновик уже есть или писать не о чем.
    pub skipped: usize,
    /// Потрачено кредитов OpenRouter (≈ доллары).
    pub cost: f64,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    /// Остановлено: кнопкой или из-за ошибки, при которой дальше всё равно не выйдет (ключ, кредиты, лимит).
    pub stopped: bool,
    pub stop_reason: Option<String>,
    /// Ошибки и пропуски по компаниям (последние 100).
    pub errors: Vec<BatchIssue>,
    /// Компании, для которых составлен черновик.
    pub drafted: Vec<i64>,
    /// Поставлено в рассылку (опция «сразу в рассылку»).
    pub queued: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct BatchIssue {
    pub id: i64,
    pub name: String,
    pub error: String,
    /// true — пропуск (не о чем писать), false — ошибка нейросети.
    pub skipped: bool,
}

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/ai/meta", get(meta))
        .route("/ai/models", get(models))
        .route("/ai/key", get(key_info))
        .route("/ai/batch", get(batch_status).post(batch_start))
        .route("/ai/batch/cancel", post(batch_cancel))
        .route("/businesses/{id}/ai/preview", post(preview))
        .route("/businesses/{id}/drafts", get(list_drafts).post(create_draft))
        .route("/messages/{mid}", patch(patch_message).delete(delete_message))
}

fn ai_error(e: AiError) -> ApiError {
    ApiError(StatusCode::BAD_GATEWAY, e.message)
}

// ---------------------------------------------------------------- каталог и ключ

async fn models_cached(st: &Shared, refresh: bool) -> Result<Arc<Vec<ModelInfo>>, AiError> {
    let mut g = st.ai.models.lock().await;
    if let Some((at, list)) = g.as_ref()
        && !refresh
        && at.elapsed() < MODELS_TTL
    {
        return Ok(list.clone());
    }
    match st.ai.client.models().await {
        Ok(list) => {
            let list = Arc::new(list);
            *g = Some((Instant::now(), list.clone()));
            Ok(list)
        }
        // OpenRouter недоступен — старый каталог лучше, чем никакого.
        Err(e) => g.as_ref().map(|(_, l)| l.clone()).ok_or(e),
    }
}

/// Что умеет модель; каталог недоступен — None (запрос всё равно уйдёт, без подсказок).
async fn caps(st: &Shared, model: &str) -> Option<ModelInfo> {
    models_cached(st, false).await.ok()?.iter().find(|m| m.id == model).cloned()
}

#[derive(Deserialize)]
struct ModelsQuery {
    #[serde(default)]
    refresh: bool,
}

async fn models(State(st): State<Shared>, Query(q): Query<ModelsQuery>) -> ApiResult<Json<Value>> {
    let list = models_cached(&st, q.refresh).await.map_err(ai_error)?;
    Ok(Json(json!({ "models": *list })))
}

async fn key_info(State(st): State<Shared>) -> ApiResult<Json<Value>> {
    let key = st.settings.read().await.openrouter_key.clone();
    Ok(Json(st.ai.client.key_info(&key).await.map_err(ai_error)?))
}

async fn meta() -> Json<Value> {
    Json(json!({
        "vars": llm::VARS.iter().map(|(name, help)| json!({"name": name, "help": help})).collect::<Vec<_>>(),
        "default_profile": AiProfile::default(),
        "efforts": llm::EFFORTS,
    }))
}

// ---------------------------------------------------------------- подготовка запроса

/// Переменные для письма одной компании.
struct Prepared {
    vars: BTreeMap<String, String>,
    lang: &'static str,
    /// Что обязательно должно быть в тексте: ваша почта.
    must: Vec<String>,
}

/// Почему для компании письмо не составить (не ошибка нейросети).
struct NotReady(String);

async fn prepare(st: &Shared, b: &BusinessRow, kind: Kind) -> anyhow::Result<Result<Prepared, NotReady>> {
    let scan = st.db.get_scan(b.scan_id).await?;
    let s = st.settings.read().await.clone();
    let ctx = LeadCtx::new(b, scan.as_ref(), &s);
    if !ctx.ready {
        return Ok(Err(NotReady("сайт ещё не проверен — писать пока не о чем".into())));
    }
    let Some(facts) = sales::letter_facts(&ctx.input()) else {
        return Ok(Err(NotReady("писать не о чем: сайт не пустил сканер, это точка сети или компания закрыта".into())));
    };
    let lang = facts.lang;
    let cat = catalog();
    let weight = |id: &str| cat.get(id).map(|d| d.weight).unwrap_or(0.0);
    let mut fails: Vec<&scanner_core::MetricResult> = ctx.results.iter().filter(|r| r.status == Status::Fail).collect();
    fails.sort_by(|a, b| weight(&b.id).total_cmp(&weight(&a.id)));
    let more: Vec<String> = dedup(fails.iter().map(|r| mini::plain_text_lang(r, lang)), 12);
    let mut good: Vec<&scanner_core::MetricResult> = ctx.results.iter().filter(|r| r.status == Status::Pass && weight(&r.id) > 0.0).collect();
    good.sort_by(|a, b| weight(&b.id).total_cmp(&weight(&a.id)));
    let good: Vec<String> = dedup(good.iter().map(|r| mini::plain_text_lang(r, lang)), 10);
    let mini_r = &b.report["mini"];
    let headline = if lang == "en" { mini_r["headline_en"].as_str().filter(|h| !h.is_empty()).or(mini_r["headline"].as_str()) } else { mini_r["headline"].as_str() };
    let address = client_address(b);
    let vars = llm::letter_vars(&llm::LetterContext {
        facts: &facts,
        company: &ctx.name,
        category: ctx.category.as_deref(),
        country: ctx.country(),
        address: address.as_deref(),
        site_state: headline.unwrap_or(""),
        score: b.overall,
        more_problems: &more,
        good: &good,
        contact_person: b.contact_person.as_deref(),
        notes: b.notes.as_deref(),
        manager: &ctx.manager,
        // PDF-разбор прикладывается к письму из рассылки, если сайт есть (у «сайта нет» разбирать нечего).
        attach: kind == Kind::Email && s.mail_attach_report && !facts.no_site,
    });
    let must = [s.manager_email.trim()].into_iter().filter(|e| !e.is_empty()).map(str::to_string).collect();
    Ok(Ok(Prepared { vars, lang, must }))
}

fn dedup(items: impl Iterator<Item = String>, limit: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in items.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) {
        if !out.contains(&t) {
            out.push(t);
        }
        if out.len() >= limit {
            break;
        }
    }
    out
}

#[derive(Deserialize)]
struct DraftReq {
    kind: Kind,
    /// Профиль из настроек; пусто — по умолчанию.
    #[serde(default)]
    profile_id: Option<String>,
    /// Несохранённый профиль из редактора настроек — проверить правки до сохранения.
    #[serde(default)]
    profile: Option<AiProfile>,
}

async fn pick_profile(st: &Shared, r: &DraftReq) -> AiProfile {
    match &r.profile {
        Some(p) => llm::normalize_profile(p, "draft"),
        None => st.settings.read().await.ai_profile(r.profile_id.as_deref()),
    }
}

async fn preview(State(st): State<Shared>, Path(id): Path<i64>, Json(r): Json<DraftReq>) -> ApiResult<Json<Value>> {
    let b = st.db.get_business(id).await?.ok_or_else(not_found)?;
    let prep = prepare(&st, &b, r.kind).await?.map_err(|n| bad(format!("Для этой компании письмо не составить: {}.", n.0)))?;
    let profile = pick_profile(&st, &r).await;
    let caps = caps(&st, &profile.model).await;
    let built = llm::build_request(&profile, r.kind, &prep.vars, caps.as_ref());
    Ok(Json(json!({
        "request": built.body,
        "unknown_vars": built.unknown_vars,
        "unsupported": built.unsupported,
        "vars": prep.vars,
        "lang": prep.lang,
        "model": caps,
        "profile": {"id": profile.id, "name": profile.name},
    })))
}

// ---------------------------------------------------------------- генерация

enum GenError {
    NotReady(String),
    Ai(AiError),
    Other(anyhow::Error),
}

impl From<anyhow::Error> for GenError {
    fn from(e: anyhow::Error) -> Self {
        GenError::Other(e)
    }
}

/// Пауза, которую можно прервать кнопкой «Остановить».
async fn pause(secs: u64, cancel: Option<&AtomicBool>) -> bool {
    for _ in 0..secs {
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            return false;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    true
}

/// Дневной лимит бесплатных моделей: повторять сегодня бессмысленно.
fn daily_limit(e: &AiError) -> bool {
    e.status == 429 && e.message.to_lowercase().contains("per-day")
}

/// Составить черновик для компании и сохранить его. Сбои провайдера и лимит частоты — до трёх попыток.
async fn generate(st: &Shared, bid: i64, kind: Kind, profile: &AiProfile, cancel: Option<&AtomicBool>) -> Result<Message, GenError> {
    let b = st.db.get_business(bid).await?.ok_or_else(|| GenError::NotReady("компания не найдена".into()))?;
    let prep = prepare(st, &b, kind).await?.map_err(|n| GenError::NotReady(n.0))?;
    if profile.model.trim().is_empty() {
        return Err(GenError::Ai(AiError { status: 400, message: "В профиле нейросети не выбрана модель.".into(), retry_after: None }));
    }
    let caps = caps(st, &profile.model).await;
    let built = llm::build_request(profile, kind, &prep.vars, caps.as_ref());
    let key = st.settings.read().await.openrouter_key.clone();
    let mut attempt = 0;
    let res = loop {
        attempt += 1;
        match st.ai.client.chat(&key, &built.body).await {
            Ok(r) => break r,
            Err(e) if e.retryable() && attempt < 3 && !daily_limit(&e) => {
                let wait = e.retry_after.unwrap_or(if attempt == 1 { 10 } else { 30 }).min(120);
                warn!("нейросеть, компания {bid}: {} — повтор через {wait} с", e.message);
                if !pause(wait, cancel).await {
                    return Err(GenError::Ai(e));
                }
            }
            Err(e) => return Err(GenError::Ai(e)),
        }
    };
    let letter = llm::parse_letter(&res.content, kind);
    if letter.body.trim().is_empty() {
        return Err(GenError::Ai(AiError { status: 502, message: format!("Не удалось достать текст из ответа модели: {}", res.content.chars().take(200).collect::<String>()), retry_after: None }));
    }
    let lint = llm::lint_letter(&letter, kind, prep.lang, &prep.must);
    let ai = json!({
        "profile_id": profile.id,
        "profile_name": profile.name,
        "model_requested": profile.model,
        "model": res.model,
        "provider": res.provider,
        "request": built.body,
        "unsupported": built.unsupported,
        "unknown_vars": built.unknown_vars,
        "content": res.content,
        "reasoning": res.reasoning,
        "usage": {"prompt": res.prompt_tokens, "completion": res.completion_tokens, "reasoning": res.reasoning_tokens, "cached": res.cached_tokens},
        "cost": res.cost,
        "ms": res.ms,
        "finish_reason": res.finish_reason,
        "generation_id": res.id,
        "lint": lint,
        "lang": prep.lang,
        "attempts": attempt,
        // Исходный текст модели — чтобы потом видеть, что менеджер поправил перед отправкой.
        "original": {"subject": letter.subject, "body": letter.body},
    });
    let subject = (kind == Kind::Email).then(|| letter.subject.clone());
    Ok(st.db.insert_message(bid, kind.as_str(), subject, letter.body, ai).await?)
}

async fn create_draft(State(st): State<Shared>, Path(id): Path<i64>, Json(r): Json<DraftReq>) -> ApiResult<Json<Message>> {
    let profile = pick_profile(&st, &r).await;
    match generate(&st, id, r.kind, &profile, None).await {
        Ok(m) => Ok(Json(m)),
        Err(GenError::NotReady(why)) => Err(bad(format!("Для этой компании письмо не составить: {why}."))),
        Err(GenError::Ai(e)) => Err(ai_error(e)),
        Err(GenError::Other(e)) => Err(e.into()),
    }
}

async fn list_drafts(State(st): State<Shared>, Path(id): Path<i64>) -> ApiResult<Json<Vec<Message>>> {
    Ok(Json(st.db.list_messages(id).await?))
}

#[derive(Deserialize)]
struct MessagePatch {
    subject: Option<String>,
    body: Option<String>,
}

async fn patch_message(State(st): State<Shared>, Path(mid): Path<i64>, Json(p): Json<MessagePatch>) -> ApiResult<Json<Message>> {
    let m = st.db.update_message(mid, p.subject, p.body, None).await?.ok_or_else(not_found)?;
    Ok(Json(m))
}

async fn delete_message(State(st): State<Shared>, Path(mid): Path<i64>) -> ApiResult<Json<Value>> {
    st.db.delete_message(mid).await?.ok_or_else(not_found)?;
    Ok(Json(json!({"ok": true})))
}

// ---------------------------------------------------------------- массовая генерация

#[derive(Deserialize)]
struct BatchReq {
    ids: Vec<i64>,
    kind: Kind,
    #[serde(default)]
    profile_id: Option<String>,
    /// Не составлять, если у компании уже есть письмо этого вида (черновик или отправленное).
    #[serde(default = "yes")]
    skip_existing: bool,
    /// Составленное письмо сразу поставить в рассылку (если у компании есть почта).
    #[serde(default)]
    enqueue: bool,
}

fn yes() -> bool {
    true
}

fn batch_snapshot(st: &Shared) -> Batch {
    st.ai.batch.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn with_batch(st: &Shared, f: impl FnOnce(&mut Batch)) {
    f(&mut st.ai.batch.lock().unwrap_or_else(|e| e.into_inner()));
}

async fn batch_status(State(st): State<Shared>) -> Json<Batch> {
    Json(batch_snapshot(&st))
}

async fn batch_cancel(State(st): State<Shared>) -> Json<Batch> {
    st.ai.cancel.store(true, Ordering::Relaxed);
    with_batch(&st, |b| {
        if b.running {
            b.stopped = true;
            b.stop_reason.get_or_insert_with(|| "Остановлено — текущие запросы доделываются.".into());
        }
    });
    Json(batch_snapshot(&st))
}

async fn batch_start(State(st): State<Shared>, Json(r): Json<BatchReq>) -> ApiResult<Json<Batch>> {
    if batch_snapshot(&st).running {
        return Err(ApiError(StatusCode::CONFLICT, "Письма уже составляются — дождитесь окончания или остановите.".into()));
    }
    let mut seen = HashSet::new();
    let ids: Vec<i64> = r.ids.into_iter().filter(|id| seen.insert(*id)).collect();
    if ids.is_empty() {
        return Err(bad("Нет компаний для писем."));
    }
    let (profile, key, conc) = {
        let s = st.settings.read().await;
        (s.ai_profile(r.profile_id.as_deref()), s.openrouter_key.clone(), s.ai_concurrency.max(1))
    };
    if key.trim().is_empty() {
        return Err(bad("Не задан ключ OpenRouter: Настройки → Нейросеть."));
    }
    if profile.model.trim().is_empty() {
        return Err(bad("В профиле нейросети не выбрана модель."));
    }
    st.ai.cancel.store(false, Ordering::Relaxed);
    with_batch(&st, |b| {
        *b = Batch {
            running: true,
            kind: r.kind.as_str().into(),
            profile: profile.name.clone(),
            model: profile.model.clone(),
            total: ids.len(),
            started_at: Some(crate::db::now()),
            ..Default::default()
        }
    });
    let st2 = st.clone();
    let enqueue = r.enqueue && r.kind == Kind::Email;
    tokio::spawn(async move { run_batch(st2, ids, r.kind, profile, r.skip_existing, conc, enqueue).await });
    Ok(Json(batch_snapshot(&st)))
}

fn push_issue(b: &mut Batch, issue: BatchIssue) {
    b.errors.push(issue);
    if b.errors.len() > 100 {
        b.errors.remove(0);
    }
}

async fn run_batch(st: Shared, ids: Vec<i64>, kind: Kind, profile: AiProfile, skip_existing: bool, conc: usize, enqueue: bool) {
    let t = Instant::now();
    let existing = if skip_existing { st.db.with_messages(kind.as_str(), ids.clone()).await.unwrap_or_default() } else { HashSet::new() };
    let sem = Arc::new(Semaphore::new(conc));
    let profile = Arc::new(profile);
    let mut tasks = tokio::task::JoinSet::new();
    for id in ids {
        if st.ai.cancel.load(Ordering::Relaxed) {
            break;
        }
        if existing.contains(&id) {
            with_batch(&st, |b| b.skipped += 1);
            continue;
        }
        let Ok(permit) = sem.clone().acquire_owned().await else { break };
        if st.ai.cancel.load(Ordering::Relaxed) {
            break;
        }
        let (st, profile) = (st.clone(), profile.clone());
        tasks.spawn(async move {
            let _permit = permit;
            let cancel = st.ai.cancel.clone();
            let r = generate(&st, id, kind, &profile, Some(&cancel)).await;
            let name = st.db.get_business(id).await.ok().flatten().map(|b| crate::api::client_name(&b)).unwrap_or_else(|| format!("#{id}"));
            match r {
                Ok(m) => {
                    // «Сразу в рассылку»: адрес — первая почта компании (не для вакансий).
                    let queued = if enqueue { crate::mail::enqueue_draft(&st, id, m.id).await } else { Ok(()) };
                    with_batch(&st, |b| {
                        b.done += 1;
                        b.cost += m.ai["cost"].as_f64().unwrap_or(0.0);
                        b.drafted.push(id);
                        match queued {
                            Ok(()) if enqueue => b.queued += 1,
                            Ok(()) => {}
                            Err(why) => push_issue(b, BatchIssue { id, name, error: format!("черновик есть, в рассылку не поставлен: {why}"), skipped: true }),
                        }
                    })
                }
                Err(GenError::NotReady(why)) => with_batch(&st, |b| {
                    b.skipped += 1;
                    push_issue(b, BatchIssue { id, name, error: why, skipped: true });
                }),
                Err(GenError::Ai(e)) => {
                    // Ключ, кредиты, неверный запрос, нет модели, лимит частоты после повторов — остальные письма упадут так же.
                    let fatal = matches!(e.status, 400 | 401 | 402 | 404 | 429);
                    if fatal {
                        cancel.store(true, Ordering::Relaxed);
                    }
                    with_batch(&st, |b| {
                        b.failed += 1;
                        if fatal && b.stop_reason.is_none() {
                            b.stopped = true;
                            b.stop_reason = Some(format!("Остановлено: {}", e.message));
                        }
                        push_issue(b, BatchIssue { id, name, error: e.message, skipped: false });
                    })
                }
                Err(GenError::Other(e)) => with_batch(&st, |b| {
                    b.failed += 1;
                    push_issue(b, BatchIssue { id, name, error: format!("{e:#}"), skipped: false });
                }),
            }
        });
    }
    while tasks.join_next().await.is_some() {}
    with_batch(&st, |b| {
        b.running = false;
        b.finished_at = Some(crate::db::now());
        if st.ai.cancel.load(Ordering::Relaxed) {
            b.stopped = true;
        }
    });
    let b = batch_snapshot(&st);
    info!("письма нейросетью: составлено {}, ошибок {}, пропущено {}, ${:.4}, {:.0} с", b.done, b.failed, b.skipped, b.cost, t.elapsed().as_secs_f64());
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use scanner_core::Business;
    use tower::ServiceExt;

    use super::*;

    async fn call(app: Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let req = Request::builder().method(method).uri(uri).header("content-type", "application/json").body(body.map(|b| Body::from(b.to_string())).unwrap_or(Body::empty())).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    /// Поддельный OpenRouter: каталог из одной модели и ответ письмом; «нет денег» — 402.
    async fn fake_openrouter(seen: Arc<Mutex<Vec<Value>>>, broke: Arc<AtomicBool>) -> String {
        let app = Router::new()
            .route(
                "/models",
                get(|| async {
                    Json(json!({"data": [{"id": "test/model", "name": "Test", "pricing": {"prompt": "0.000001", "completion": "0.000002"},
                        "architecture": {"output_modalities": ["text"]}, "supported_parameters": ["temperature", "structured_outputs", "response_format", "reasoning"]}]}))
                }),
            )
            .route(
                "/chat/completions",
                post(move |Json(body): Json<Value>| {
                    let (seen, broke) = (seen.clone(), broke.clone());
                    async move {
                        seen.lock().unwrap().push(body.clone());
                        if broke.load(Ordering::Relaxed) {
                            return (StatusCode::PAYMENT_REQUIRED, Json(json!({"error": {"code": 402, "message": "Insufficient credits"}})));
                        }
                        let email = body["response_format"]["json_schema"]["schema"]["required"] == json!(["subject", "body"]);
                        let content = if email {
                            json!({"subject": "Сайт belodent.by — что мешает пациентам", "body": "Здравствуйте! На belodent.by нашлось две проблемы:\n— раз\n— два\n\nС уважением,\nhello@example.com"}).to_string()
                        } else {
                            json!({"body": "Здравствуйте! Коротко о сайте.\n\nПочта для связи: hello@example.com"}).to_string()
                        };
                        (StatusCode::OK, Json(json!({"id": "gen-1", "model": "test/model", "provider": "Fake",
                            "choices": [{"finish_reason": "stop", "message": {"content": content, "reasoning": "Сначала о главном."}}],
                            "usage": {"prompt_tokens": 800, "completion_tokens": 100, "cost": 0.0011}})))
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}")
    }

    async fn wait_batch(app: &Router) -> Value {
        for _ in 0..100 {
            let (_, v) = call(app.clone(), "GET", "/api/ai/batch", None).await;
            if v["running"] == false {
                return v;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("массовая генерация не закончилась");
    }

    #[tokio::test]
    async fn drafts_preview_sent_and_batch() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let broke = Arc::new(AtomicBool::new(false));
        let base = fake_openrouter(seen.clone(), broke.clone()).await;
        let db = crate::db::Db::in_memory().unwrap();
        let sid = db.insert_scan("Тест".into(), "map", json!({"center": {"lat": 1.0, "lon": 1.0}, "radius_m": 1000.0, "providers": ["osm"], "query": "", "preset": "dentist", "country": "by", "city_names": ["Мінск", "Минск"]})).await.unwrap();
        let b = Business { name: "Белодент".into(), website: Some("https://belodent.by".into()), ..Default::default() };
        let pending = Business { name: "Не проверена".into(), ..Default::default() };
        db.insert_businesses(sid, vec![b, pending]).await.unwrap();
        let ids: Vec<i64> = db.scan_businesses(sid, false).await.unwrap().into_iter().map(|x| x.0).collect();
        let (bid, pid) = (ids[0], ids[1]);
        let results = json!([{"id": "B05", "status": "fail", "value": "no_redirect", "detail": ""}, {"id": "G01", "status": "fail", "detail": ""}]);
        let report = json!({"mini": {"state": "fair", "headline": "Средний сайт: 60/100", "problems": [], "offer": [], "fail_ids": []}});
        db.save_audit(bid, "warm".into(), Some(60.0), 100.0, report, results, json!({"final_url": "https://belodent.by/"}), 5, Value::Null, Some("https://belodent.by".into())).await.unwrap();
        let app = crate::api::router(crate::jobs::AppState::with_ai(db, Some(base)).await.unwrap());

        // Без ключа — понятная ошибка, а не падение.
        let (st, v) = call(app.clone(), "POST", &format!("/api/businesses/{bid}/drafts"), Some(json!({"kind": "email"}))).await;
        assert_eq!(st, StatusCode::BAD_GATEWAY);
        assert!(v["error"].as_str().unwrap().contains("ключ OpenRouter"), "{v}");

        let (st, _) = call(app.clone(), "PUT", "/api/settings", Some(json!({"openrouter_key": "sk-or-test", "ai_profiles": [{"id": "default", "name": "Тест", "model": "test/model", "temperature": 0.4}]}))).await;
        assert_eq!(st, StatusCode::OK);

        // Предпросмотр: тот же запрос, что уйдёт в OpenRouter, с подставленными фактами и схемой ответа.
        let (st, v) = call(app.clone(), "POST", &format!("/api/businesses/{bid}/ai/preview"), Some(json!({"kind": "email"}))).await;
        assert_eq!(st, StatusCode::OK, "{v}");
        assert_eq!((v["request"]["model"].as_str(), v["request"]["temperature"].as_f64()), (Some("test/model"), Some(0.4)));
        assert_eq!(v["request"]["response_format"]["type"], "json_schema");
        let user = v["request"]["messages"][1]["content"].as_str().unwrap();
        assert!(user.contains("belodent.by") && user.contains("нет перенаправления на https") && user.contains("Средний сайт: 60/100"), "{user}");
        assert!(user.contains("Город: Минск") && !user.contains("Мінск"), "город на языке письма: {user}");
        assert!(v["unknown_vars"].as_array().unwrap().is_empty());
        let (st, v) = call(app.clone(), "POST", &format!("/api/businesses/{pid}/ai/preview"), Some(json!({"kind": "email"}))).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert!(v["error"].as_str().unwrap().contains("не проверен"));

        // Черновик: текст, ход рассуждения, цена, исходник модели; виден в карточке компании.
        let (st, m) = call(app.clone(), "POST", &format!("/api/businesses/{bid}/drafts"), Some(json!({"kind": "email"}))).await;
        assert_eq!(st, StatusCode::OK, "{m}");
        assert_eq!((m["status"].as_str(), m["kind"].as_str()), (Some("draft"), Some("email")));
        assert!(m["subject"].as_str().unwrap().starts_with("Сайт belodent.by"));
        assert_eq!((m["ai"]["reasoning"].as_str(), m["ai"]["cost"].as_f64(), m["ai"]["provider"].as_str()), (Some("Сначала о главном."), Some(0.0011), Some("Fake")));
        assert_eq!(m["ai"]["lint"], json!([]), "письмо по правилам — замечаний нет");
        let mid = m["id"].as_i64().unwrap();
        let (_, v) = call(app.clone(), "GET", &format!("/api/businesses/{bid}"), None).await;
        assert_eq!(v["messages"].as_array().unwrap().len(), 1);

        // Правка и «отправлено»: итоговый текст сохраняется, исходник модели остаётся в ai.original.
        let (st, _) = call(app.clone(), "PATCH", &format!("/api/messages/{mid}"), Some(json!({"body": "Правленый текст"}))).await;
        assert_eq!(st, StatusCode::OK);
        let (st, v) = call(app.clone(), "POST", &format!("/api/businesses/{bid}/sent"), Some(json!({"channel": "email", "text": "Тема\n\nИтог", "message_id": mid, "subject": "Тема", "body": "Итог"}))).await;
        assert_eq!(st, StatusCode::OK, "{v}");
        let (_, list) = call(app.clone(), "GET", &format!("/api/businesses/{bid}/drafts"), None).await;
        let sent = &list[0];
        assert_eq!((sent["status"].as_str(), sent["subject"].as_str(), sent["body"].as_str()), (Some("sent"), Some("Тема"), Some("Итог")));
        assert!(sent["sent_at"].is_string() && sent["ai"]["original"]["body"].as_str().unwrap().contains("две проблемы"));
        let (st, _) = call(app.clone(), "POST", &format!("/api/businesses/{pid}/sent"), Some(json!({"channel": "email", "message_id": mid}))).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "чужой черновик не отмечаем");

        // Массово: у Белодента письмо уже есть — пропуск; непроверенная — пропуск с причиной.
        let (st, v) = call(app.clone(), "POST", "/api/ai/batch", Some(json!({"ids": [bid, pid, bid], "kind": "email"}))).await;
        assert_eq!((st, v["total"].as_u64()), (StatusCode::OK, Some(2)));
        let v = wait_batch(&app).await;
        assert_eq!((v["done"].as_u64(), v["skipped"].as_u64(), v["failed"].as_u64()), (Some(0), Some(2), Some(0)), "{v}");
        assert!(v["errors"][0]["error"].as_str().unwrap().contains("не проверен"));
        // Сообщения в мессенджер ещё нет — составится.
        call(app.clone(), "POST", "/api/ai/batch", Some(json!({"ids": [bid], "kind": "short"}))).await;
        let v = wait_batch(&app).await;
        assert_eq!((v["done"].as_u64(), v["drafted"][0].as_i64()), (Some(1), Some(bid)), "{v}");
        assert!((v["cost"].as_f64().unwrap() - 0.0011).abs() < 1e-9);

        // Кредиты кончились — партия останавливается с причиной, а не перебирает всех.
        broke.store(true, Ordering::Relaxed);
        call(app.clone(), "POST", "/api/ai/batch", Some(json!({"ids": [bid], "kind": "short", "skip_existing": false}))).await;
        let v = wait_batch(&app).await;
        assert_eq!((v["failed"].as_u64(), v["stopped"].as_bool()), (Some(1), Some(true)), "{v}");
        assert!(v["stop_reason"].as_str().unwrap().contains("кредитов"), "{v}");
        let requests = seen.lock().unwrap().len();
        assert_eq!(requests, 3, "402 не повторяем");

        let (st, _) = call(app.clone(), "DELETE", &format!("/api/messages/{mid}"), None).await;
        assert_eq!(st, StatusCode::OK);
        let (_, v) = call(app, "GET", "/api/ai/meta", None).await;
        assert_eq!(v["default_profile"]["model"], llm::DEFAULT_MODEL);
    }
}
