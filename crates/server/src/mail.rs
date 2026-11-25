//! Рассылка писем по почте (SMTP). Менеджер сам ставит письма в очередь — черновики нейросети, шаблон
//! или свой текст — и запускает рассылку. Письма уходят по одному: случайная пауза между письмами,
//! дневной лимит, окно отправки по часам. После отправки — статус «Написали», касание в журнале и шаг
//! «Проверить ответ» через 3 дня. Сбои почты (логин, связь, отказы подряд) ставят рассылку на паузу с причиной.

use std::hash::{BuildHasher, RandomState};
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Datelike, Days, Local, TimeZone, Timelike, Utc};
use lettre::message::header::{ContentType, HeaderName, HeaderValue};
use lettre::message::{Attachment, Mailbox, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Notify;
use tracing::{info, warn};

use scanner_core::mini::SiteState;
use scanner_core::sales;

use crate::api::{bad, not_found, ApiError, ApiResult, LeadCtx};
use crate::db::{BusinessRow, Message};
use crate::jobs::Shared;
use crate::settings::Settings;

/// Отказов адресатов подряд, после которых рассылка встаёт на паузу (адреса плохие или письма считают спамом).
const MAX_REJECTS_IN_ROW: u32 = 3;

#[derive(Default)]
pub struct MailState {
    wake: Notify,
    info: std::sync::Mutex<MailInfo>,
}

/// Что сейчас с рассылкой — для страницы «Рассылка».
#[derive(Debug, Clone, Serialize, Default)]
pub struct MailInfo {
    /// Когда уйдёт следующее письмо (если рассылка идёт и очередь не пуста).
    pub next_at: Option<DateTime<Utc>>,
    /// Чего ждём: окна отправки, завтрашнего лимита, паузы между письмами, писем в очереди.
    pub waiting: Option<String>,
    /// Почему рассылка остановилась сама.
    pub stop_reason: Option<String>,
    pub last_error: Option<String>,
    /// Письмо, которое отправляется прямо сейчас.
    pub sending: Option<i64>,
    #[serde(skip)]
    rejects_in_row: u32,
    #[serde(skip)]
    next_send: Option<DateTime<Utc>>,
}

impl MailState {
    fn info(&self) -> std::sync::MutexGuard<'_, MailInfo> {
        self.info.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Разбудить рассылку: поставили письмо, запустили, поменяли настройки.
    pub fn wake(&self) {
        self.wake.notify_one();
    }
}

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/mail/status", get(status))
        .route("/mail/start", post(start))
        .route("/mail/pause", post(pause))
        .route("/mail/test", post(test_mail))
        .route("/mail/outbox", get(outbox))
        .route("/mail/queue", post(queue_many))
        .route("/businesses/{id}/mail", post(queue_one))
        .route("/messages/{mid}/unqueue", post(unqueue))
}

// ---------------------------------------------------------------- окно и лимит

fn in_window(t: DateTime<Local>, s: &Settings) -> bool {
    if s.mail_weekdays_only && t.weekday().number_from_monday() > 5 {
        return false;
    }
    // Конец не позже начала — ограничения по часам нет.
    s.mail_window_end <= s.mail_window_start || (t.hour() >= s.mail_window_start && t.hour() < s.mail_window_end)
}

/// Ближайший момент не раньше `t`, когда можно отправлять.
fn window_open_at(t: DateTime<Local>, s: &Settings) -> DateTime<Local> {
    if in_window(t, s) {
        return t;
    }
    let start = if s.mail_window_end <= s.mail_window_start { 0 } else { s.mail_window_start };
    for d in 0..9 {
        let Some(date) = t.date_naive().checked_add_days(Days::new(d)) else { break };
        let Some(cand) = date.and_hms_opt(start, 0, 0).and_then(|n| Local.from_local_datetime(&n).earliest()) else { continue };
        if cand >= t && in_window(cand, s) {
            return cand;
        }
    }
    t + chrono::Duration::hours(1)
}

/// Начало сегодняшнего дня по местному времени — в UTC, для подсчёта писем за сегодня.
fn today_start_utc(now: DateTime<Local>) -> String {
    let start = now.date_naive().and_hms_opt(0, 0, 0).and_then(|n| Local.from_local_datetime(&n).earliest()).unwrap_or(now);
    start.with_timezone(&Utc).to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn random_between(lo: u64, hi: u64) -> u64 {
    if hi <= lo {
        return lo;
    }
    let r = RandomState::new().hash_one(std::time::SystemTime::now());
    lo + r % (hi - lo + 1)
}

// ---------------------------------------------------------------- отправка

/// Итог отправки: что делать с письмом и рассылкой.
enum Outcome {
    Sent(String),
    /// Адресат отклонён — письмо в ошибки, рассылка идёт дальше.
    Rejected(String),
    /// Временно — письмо обратно в очередь, повтор позже.
    Later(String),
    /// Беда с почтой (логин, связь, PDF) — письмо обратно в очередь, рассылка на паузу.
    Stop(String),
}

fn is_local(host: &str) -> bool {
    matches!(host.trim(), "localhost" | "127.0.0.1" | "::1")
}

fn transport(s: &Settings) -> Result<AsyncSmtpTransport<Tokio1Executor>, String> {
    let host = s.smtp_host.trim();
    let b = match s.smtp_security.as_str() {
        "starttls" => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(host).map_err(|e| format!("Почтовый сервер {host}: {e}"))?,
        // Без шифрования — только для почтового сервера на этом же компьютере.
        "plain" if is_local(host) => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host),
        _ => AsyncSmtpTransport::<Tokio1Executor>::relay(host).map_err(|e| format!("Почтовый сервер {host}: {e}"))?,
    };
    Ok(b.port(s.smtp_port).credentials(Credentials::new(s.smtp_user.trim().to_string(), s.smtp_password.clone())).timeout(Some(Duration::from_secs(60))).build())
}

fn sender(s: &Settings) -> Result<Mailbox, String> {
    let addr = s.mail_from().parse().map_err(|_| format!("Неверный адрес отправителя «{}»", s.mail_from()))?;
    let name = s.mail_from_name.trim();
    Ok(Mailbox::new((!name.is_empty()).then(|| name.to_string()), addr))
}

/// Понятное объяснение ошибки SMTP и что с ней делать.
fn classify(e: &lettre::transport::smtp::Error) -> Outcome {
    let code: Option<u16> = e.status().map(u16::from);
    let text = e.to_string();
    match code {
        Some(c @ (530 | 534 | 535 | 538)) => {
            Outcome::Stop(format!("Почта не приняла логин или пароль ({c}): {text}. Для Gmail, Яндекса и Mail.ru нужен пароль приложения, а не обычный пароль."))
        }
        Some(c) if e.is_permanent() && matches!(c, 501 | 550 | 551 | 552 | 553 | 554 | 555) => Outcome::Rejected(format!("Адрес или письмо отклонены ({c}): {text}")),
        Some(c) if e.is_transient() => Outcome::Later(format!("Почтовый сервер просит подождать ({c}): {text}")),
        _ if e.is_timeout() => Outcome::Stop(format!("Почтовый сервер не отвечает: {text}")),
        _ if e.is_tls() => Outcome::Stop(format!("Не удалось установить защищённое соединение — проверьте порт и шифрование (465 — SSL, 587 — STARTTLS): {text}")),
        _ => Outcome::Stop(format!("Ошибка почты: {text}")),
    }
}

/// Собрать и отправить письмо. `pdf` — (имя файла, содержимое).
async fn deliver(s: &Settings, to: &str, subject: &str, body: &str, pdf: Option<(String, Vec<u8>)>) -> Outcome {
    let from = match sender(s) {
        Ok(f) => f,
        Err(e) => return Outcome::Stop(e),
    };
    let to: Mailbox = match to.trim().parse() {
        Ok(t) => t,
        Err(_) => return Outcome::Rejected(format!("Неверный адрес получателя «{to}»")),
    };
    let domain = s.mail_from().rsplit('@').next().unwrap_or("localhost").to_string();
    let token = format!("{:x}{:x}", Utc::now().timestamp_micros(), RandomState::new().hash_one(to.to_string()));
    let message_id = format!("<{token}@{domain}>");
    let reply = s.mail_reply_to.trim();
    let reply_addr = if reply.is_empty() { s.mail_from() } else { reply.to_string() };
    let mut b = lettre::Message::builder().from(from).to(to).subject(subject).message_id(Some(message_id.clone())).date_now();
    if !reply.is_empty() {
        match reply.parse() {
            Ok(r) => b = b.reply_to(r),
            Err(_) => return Outcome::Stop(format!("Неверный адрес для ответов «{reply}»")),
        }
    }
    // Почтовые сервисы ждут от рассылок способ отписки; ответ «unsubscribe» приходит вам в ящик.
    if let Ok(name) = HeaderName::new_from_ascii("List-Unsubscribe".into()) {
        b = b.raw_header(HeaderValue::new(name, format!("<mailto:{reply_addr}?subject=unsubscribe>")));
    }
    let text = SinglePart::builder().header(ContentType::TEXT_PLAIN).body(body.to_string());
    let email = match pdf {
        Some((name, bytes)) => {
            let pdf_type = ContentType::parse("application/pdf").unwrap_or(ContentType::TEXT_PLAIN);
            b.multipart(MultiPart::mixed().singlepart(text).singlepart(Attachment::new(name).body(bytes, pdf_type)))
        }
        None => b.singlepart(text),
    };
    let email = match email {
        Ok(e) => e,
        Err(e) => return Outcome::Rejected(format!("Письмо не собралось: {e}")),
    };
    let tr = match transport(s) {
        Ok(t) => t,
        Err(e) => return Outcome::Stop(e),
    };
    match tr.send(email).await {
        Ok(_) => Outcome::Sent(message_id),
        Err(e) => classify(&e),
    }
}

/// Строка отказа от писем на языке письма; не добавляется, если уже есть в тексте.
fn with_footer(body: &str, footer: &str) -> String {
    let f = footer.trim();
    if f.is_empty() || body.contains(f) { body.trim_end().to_string() } else { format!("{}\n\n{f}", body.trim_end()) }
}

fn no_site(state: SiteState) -> bool {
    matches!(state, SiteState::None | SiteState::Social | SiteState::Dead | SiteState::Stub)
}

/// Язык письма и будет ли PDF-разбор: (lang, attach).
async fn letter_lang(st: &Shared, b: &BusinessRow, s: &Settings) -> (&'static str, bool) {
    let scan = st.db.get_scan(b.scan_id).await.ok().flatten();
    let ctx = LeadCtx::new(b, scan.as_ref(), s);
    (sales::outreach_lang(ctx.country(), &ctx.cities), s.mail_attach_report && !no_site(ctx.state))
}

async fn send_one(st: &Shared, s: &Settings, m: Message) -> Outcome {
    let Some(b) = st.db.get_business(m.business_id).await.ok().flatten() else {
        return Outcome::Rejected("Компания удалена".into());
    };
    let (lang, attach) = letter_lang(st, &b, s).await;
    let pdf = if attach {
        match crate::api::render_pdf(st, b.id, "report").await {
            Ok(bytes) => {
                let host = crate::api::file_host(&b).unwrap_or_else(|| crate::api::client_name(&b));
                let title = if lang == "en" { "Website review" } else { "Разбор сайта" };
                Some((format!("{title} {host}.pdf").replace(['/', '\\', '"'], " "), bytes))
            }
            Err(e) => return Outcome::Stop(format!("Не собрался PDF-разбор для вложения: {e}. Поставьте Chromium или выключите вложение в настройках почты.")),
        }
    } else {
        None
    };
    let footer = if lang == "en" { &s.mail_footer_en } else { &s.mail_footer };
    let body = with_footer(&m.body, footer);
    let subject = m.subject.clone().unwrap_or_default();
    let to = m.to_addr.clone().unwrap_or_default();
    let out = deliver(s, &to, &subject, &body, pdf).await;
    if let Outcome::Sent(_) = &out {
        // Итоговый текст (со строкой отказа) — в письмо и журнал, как ушёл.
        let _ = st.db.update_message(m.id, None, Some(body.clone()), None).await;
        let note = format!("Рассылка на {to}\n{subject}\n\n{body}");
        if let Err(e) = crate::api::record_sent(st, &b, "email".into(), Some(note)).await {
            warn!("рассылка: журнал компании {}: {e:#}", b.id);
        }
    }
    out
}

/// Остановить рассылку с причиной: флаг в настройках, чтобы после перезапуска она не пошла сама.
async fn stop_with(st: &Shared, reason: String) {
    warn!("рассылка остановлена: {reason}");
    {
        let mut i = st.mail.info();
        i.stop_reason = Some(reason);
        i.waiting = None;
        i.next_at = None;
    }
    let mut s = st.settings.read().await.clone();
    if s.mail_running {
        s.mail_running = false;
        if let Err(e) = st.save_settings(s).await {
            warn!("рассылка: не сохранить остановку: {e:#}");
        }
    }
}

fn set_wait(st: &Shared, why: &str, at: Option<DateTime<Utc>>) {
    let mut i = st.mail.info();
    i.waiting = Some(why.to_string());
    i.next_at = at;
}

/// Один шаг рассылки; возвращает, сколько ждать до следующего.
async fn step(st: &Shared) -> Duration {
    let s = st.settings.read().await.clone();
    if !s.mail_running {
        let mut i = st.mail.info();
        i.waiting = None;
        i.next_at = None;
        return Duration::from_secs(3600);
    }
    if !s.mail_configured() {
        stop_with(st, "Почта не настроена: Настройки → Почта для рассылки.".into()).await;
        return Duration::from_secs(3600);
    }
    let now = Local::now();
    let open = window_open_at(now, &s);
    if open > now {
        set_wait(st, "ждёт окна отправки", Some(open.with_timezone(&Utc)));
        return (open - now).to_std().unwrap_or_default().min(Duration::from_secs(900));
    }
    let sent_today = st.db.mail_sent_since(today_start_utc(now)).await.unwrap_or(0);
    if sent_today >= s.mail_daily_limit as i64 {
        let tomorrow = now.date_naive().checked_add_days(Days::new(1)).and_then(|d| d.and_hms_opt(0, 0, 0)).and_then(|n| Local.from_local_datetime(&n).earliest()).unwrap_or(now);
        let at = window_open_at(tomorrow, &s);
        set_wait(st, "дневной лимит исчерпан — продолжит завтра", Some(at.with_timezone(&Utc)));
        return (at - now).to_std().unwrap_or_default().min(Duration::from_secs(1800));
    }
    let next_send = st.mail.info().next_send;
    if let Some(at) = next_send
        && at > Utc::now()
    {
        set_wait(st, "пауза между письмами", Some(at));
        return (at - Utc::now()).to_std().unwrap_or_default();
    }
    let m = match st.db.claim_next_mail().await {
        Ok(Some(m)) => m,
        Ok(None) => {
            set_wait(st, "очередь пуста — добавьте письма", None);
            return Duration::from_secs(3600);
        }
        Err(e) => {
            warn!("рассылка: очередь: {e:#}");
            return Duration::from_secs(60);
        }
    };
    {
        let mut i = st.mail.info();
        i.sending = Some(m.id);
        i.waiting = Some("отправляется".into());
    }
    let out = send_one(st, &s, m.clone()).await;
    let pause = Duration::from_secs(random_between(s.mail_interval_min, s.mail_interval_max));
    let mut stop = None;
    let result = match out {
        Outcome::Sent(id) => {
            info!("рассылка: отправлено письмо {} → {}", m.id, m.to_addr.as_deref().unwrap_or(""));
            {
                let mut i = st.mail.info();
                i.rejects_in_row = 0;
                i.last_error = None;
            }
            st.db.finish_mail(m.id, "sent", Some(id), None).await
        }
        Outcome::Rejected(e) => {
            {
                let mut i = st.mail.info();
                i.rejects_in_row += 1;
                i.last_error = Some(e.clone());
                if i.rejects_in_row >= MAX_REJECTS_IN_ROW {
                    stop = Some(format!("{MAX_REJECTS_IN_ROW} письма подряд отклонены — проверьте адреса и не попала ли почта в спам-листы. Последняя ошибка: {e}"));
                }
            }
            st.db.finish_mail(m.id, "failed", None, Some(e)).await
        }
        Outcome::Later(e) => {
            {
                let mut i = st.mail.info();
                i.last_error = Some(e.clone());
                i.next_send = Some(Utc::now() + chrono::Duration::minutes(20));
            }
            st.db.finish_mail(m.id, "queued", None, Some(e)).await
        }
        Outcome::Stop(e) => {
            stop = Some(e.clone());
            st.db.finish_mail(m.id, "queued", None, Some(e)).await
        }
    };
    if let Err(e) = result {
        warn!("рассылка: сохранить итог письма {}: {e:#}", m.id);
    }
    {
        let mut i = st.mail.info();
        i.sending = None;
        if i.next_send.is_none_or(|t| t <= Utc::now()) {
            i.next_send = Some(Utc::now() + chrono::Duration::from_std(pause).unwrap_or_default());
        }
    }
    if let Some(reason) = stop {
        stop_with(st, reason).await;
    }
    Duration::from_millis(200)
}

/// Фоновая рассылка: одна задача на сервер, спит до следующего письма или до «разбудить».
pub fn spawn_worker(st: Shared) {
    tokio::spawn(async move {
        match st.db.recover_sending().await {
            Ok(n) if n > 0 => warn!("рассылка: {n} писем отправлялись при остановке сервера — помечены ошибкой, проверьте «Отправленные»"),
            Err(e) => warn!("рассылка: {e:#}"),
            _ => {}
        }
        loop {
            let wait = step(&st).await;
            tokio::select! {
                _ = st.mail.wake.notified() => {}
                _ = tokio::time::sleep(wait) => {}
            }
        }
    });
}

// ---------------------------------------------------------------- очередь

/// Почта компании для рассылки: первая не для вакансий, иначе любая.
fn pick_email(contacts: &[sales::Contact]) -> Option<String> {
    let emails: Vec<&sales::Contact> = contacts.iter().filter(|c| c.kind == "email" && c.value.contains('@')).collect();
    emails.iter().find(|c| c.note.as_deref().is_none_or(|n| !n.contains("ваканс"))).or(emails.first()).map(|c| c.value.trim().to_string())
}

async fn email_of(st: &Shared, b: &BusinessRow) -> Option<String> {
    let scan = st.db.get_scan(b.scan_id).await.ok().flatten();
    let s = st.settings.read().await.clone();
    let ctx = LeadCtx::new(b, scan.as_ref(), &s);
    pick_email(&crate::api::lead_contacts(b, &ctx))
}

/// Поставить готовый черновик в рассылку (массовая генерация «сразу в рассылку»). Ошибка — причина словами.
pub async fn enqueue_draft(st: &Shared, bid: i64, mid: i64) -> Result<(), String> {
    let b = st.db.get_business(bid).await.ok().flatten().ok_or("компания не найдена")?;
    let to = email_of(st, &b).await.ok_or("у компании нет почты")?;
    if st.db.mail_state(bid).await.map(|x| x.0).unwrap_or(false) {
        return Err("письмо уже в очереди".into());
    }
    st.db.queue_message(mid, to, None, None).await.map_err(|e| e.to_string())?.ok_or("черновик не подходит")?;
    st.mail.wake();
    Ok(())
}

#[derive(Deserialize)]
struct QueueMany {
    ids: Vec<i64>,
    /// `draft` — последний черновик нейросети; `template` — письмо по шаблону сканера; `draft_or_template`.
    #[serde(default = "default_source")]
    source: String,
    /// Не писать компаниям, с которыми уже работаете (статус не «Новый»).
    #[serde(default = "yes")]
    skip_contacted: bool,
    /// Не писать, если письмо уже отправляли.
    #[serde(default = "yes")]
    skip_sent: bool,
}

fn default_source() -> String {
    "draft".into()
}

fn yes() -> bool {
    true
}

#[derive(Serialize)]
struct Skipped {
    id: i64,
    name: String,
    reason: String,
}

async fn queue_many(State(st): State<Shared>, Json(r): Json<QueueMany>) -> ApiResult<Json<Value>> {
    if !["draft", "template", "draft_or_template"].contains(&r.source.as_str()) {
        return Err(bad("source: draft, template или draft_or_template"));
    }
    let s = st.settings.read().await.clone();
    let mut queued = 0;
    let mut skipped: Vec<Skipped> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for id in r.ids.into_iter().filter(|id| seen.insert(*id)) {
        let Some(b) = st.db.get_business(id).await? else { continue };
        let name = crate::api::client_name(&b);
        let mut skip = |reason: &str| skipped.push(Skipped { id, name: name.clone(), reason: reason.into() });
        let scan = st.db.get_scan(b.scan_id).await?;
        let ctx = LeadCtx::new(&b, scan.as_ref(), &s);
        if !ctx.ready {
            skip("сайт ещё не проверен");
            continue;
        }
        let Some(to) = pick_email(&crate::api::lead_contacts(&b, &ctx)) else {
            skip("нет почты");
            continue;
        };
        let (in_queue, sent) = st.db.mail_state(id).await?;
        if in_queue {
            skip("уже в очереди");
            continue;
        }
        if r.skip_sent && sent {
            skip("письмо уже отправляли");
            continue;
        }
        if r.skip_contacted && b.lead_status != "new" {
            skip(&format!("уже в работе: «{}»", crate::db::status_label(&b.lead_status)));
            continue;
        }
        let draft = if r.source == "template" { None } else { st.db.latest_draft(id, "email").await? };
        let mid = match draft {
            Some(d) => d.id,
            None if r.source == "draft" => {
                skip("нет черновика письма — составьте нейросетью или возьмите шаблон");
                continue;
            }
            None => match sales::outreach(&ctx.input()) {
                Some(o) => st.db.insert_message(id, "email", Some(o.email_subject), o.email_body, json!({"source": "template"})).await?.id,
                None => {
                    skip("писать не о чем: сайт закрыт для сканера, точка сети или компания закрыта");
                    continue;
                }
            },
        };
        if st.db.queue_message(mid, to, None, None).await?.is_some() {
            queued += 1;
        }
    }
    st.mail.wake();
    Ok(Json(json!({ "queued": queued, "skipped": skipped })))
}

#[derive(Deserialize)]
struct QueueOne {
    /// Черновик, который в поле письма; без него — новое письмо из текста.
    message_id: Option<i64>,
    to: String,
    subject: String,
    body: String,
}

async fn queue_one(State(st): State<Shared>, Path(id): Path<i64>, Json(r): Json<QueueOne>) -> ApiResult<Json<Message>> {
    st.db.get_business(id).await?.ok_or_else(not_found)?;
    let (to, subject, body) = (r.to.trim().to_string(), r.subject.trim().to_string(), r.body.trim().to_string());
    if to.parse::<Mailbox>().is_err() {
        return Err(bad("Неверный адрес почты получателя."));
    }
    if subject.is_empty() || body.is_empty() {
        return Err(bad("Нужны тема и текст письма."));
    }
    let mid = match r.message_id {
        Some(mid) => {
            let m = st.db.get_message(mid).await?.filter(|m| m.business_id == id && m.kind == "email").ok_or_else(|| bad("черновик не от этой компании"))?;
            if m.status == "sent" || m.status == "sending" {
                return Err(bad("Это письмо уже отправлено."));
            }
            mid
        }
        None => st.db.insert_message(id, "email", Some(subject.clone()), body.clone(), json!({"source": "manual"})).await?.id,
    };
    let m = st.db.queue_message(mid, to, Some(subject), Some(body)).await?.ok_or_else(|| bad("Письмо нельзя поставить в очередь."))?;
    st.mail.wake();
    Ok(Json(m))
}

async fn unqueue(State(st): State<Shared>, Path(mid): Path<i64>) -> ApiResult<Json<Value>> {
    if !st.db.unqueue_message(mid).await? {
        return Err(bad("Письмо уже отправляется или отправлено."));
    }
    st.mail.wake();
    Ok(Json(json!({"ok": true})))
}

async fn outbox(State(st): State<Shared>) -> ApiResult<Json<Value>> {
    Ok(Json(json!(st.db.outbox().await?)))
}

// ---------------------------------------------------------------- управление

async fn status_value(st: &Shared) -> anyhow::Result<Value> {
    let s = st.settings.read().await.clone();
    let now = Local::now();
    let (queued, failed) = st.db.mail_counts().await?;
    let sent_today = st.db.mail_sent_since(today_start_utc(now)).await?;
    let info = st.mail.info().clone();
    Ok(json!({
        "configured": s.mail_configured(),
        "running": s.mail_running,
        "from": s.mail_from(),
        "queued": queued,
        "failed": failed,
        "sent_today": sent_today,
        "daily_limit": s.mail_daily_limit,
        "in_window": in_window(now, &s),
        "window": {"start": s.mail_window_start, "end": s.mail_window_end, "weekdays_only": s.mail_weekdays_only},
        "interval": {"min": s.mail_interval_min, "max": s.mail_interval_max},
        "attach_report": s.mail_attach_report,
        "info": info,
    }))
}

async fn status(State(st): State<Shared>) -> ApiResult<Json<Value>> {
    Ok(Json(status_value(&st).await?))
}

async fn set_running(st: &Shared, on: bool) -> ApiResult<()> {
    let mut s = st.settings.read().await.clone();
    if on && !s.mail_configured() {
        return Err(bad("Сначала настройте почту: Настройки → Почта для рассылки."));
    }
    s.mail_running = on;
    st.save_settings(s).await?;
    if on {
        let mut i = st.mail.info();
        i.stop_reason = None;
        i.rejects_in_row = 0;
    }
    st.mail.wake();
    Ok(())
}

async fn start(State(st): State<Shared>) -> ApiResult<Json<Value>> {
    set_running(&st, true).await?;
    // Дать шагу рассылки посчитать «чего ждём», чтобы ответ сразу это показал.
    tokio::time::sleep(Duration::from_millis(150)).await;
    Ok(Json(status_value(&st).await?))
}

async fn pause(State(st): State<Shared>) -> ApiResult<Json<Value>> {
    set_running(&st, false).await?;
    Ok(Json(status_value(&st).await?))
}

#[derive(Deserialize, Default)]
struct TestReq {
    to: Option<String>,
}

/// Пробное письмо — себе (на адрес отправителя) или на указанный адрес; сразу, мимо очереди и лимитов.
async fn test_mail(State(st): State<Shared>, body: Option<Json<TestReq>>) -> ApiResult<Json<Value>> {
    let s = st.settings.read().await.clone();
    if !s.mail_configured() {
        return Err(bad("Заполните сервер, логин, пароль и адрес отправителя и нажмите «Сохранить»."));
    }
    let to = body.and_then(|b| b.0.to).map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).unwrap_or_else(|| s.mail_from());
    let text = "Если вы читаете это письмо, почта для рассылки в сканере карт настроена правильно.\n\nОтправлено кнопкой «Проверить» в настройках.";
    match deliver(&s, &to, "Проверка почты для рассылки", text, None).await {
        Outcome::Sent(id) => Ok(Json(json!({"ok": true, "to": to, "message_id": id}))),
        Outcome::Rejected(e) | Outcome::Later(e) | Outcome::Stop(e) => Err(ApiError(StatusCode::BAD_GATEWAY, e)),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use scanner_core::Business;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tower::ServiceExt;

    use super::*;

    #[test]
    fn window_and_footer() {
        let mut s = Settings { mail_window_start: 9, mail_window_end: 19, mail_weekdays_only: true, ..Default::default() };
        let at = |y, mo, d, h, mi| Local.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap();
        // Среда 25.11.2026, 08:30 → откроется в 9:00; 12:00 — можно; 19:00 — до четверга 9:00.
        assert_eq!(window_open_at(at(2026, 11, 25, 8, 30), &s), at(2026, 11, 25, 9, 0));
        assert_eq!(window_open_at(at(2026, 11, 25, 12, 0), &s), at(2026, 11, 25, 12, 0));
        assert_eq!(window_open_at(at(2026, 11, 25, 19, 0), &s), at(2026, 11, 26, 9, 0));
        // Пятница вечером → понедельник; без «только будни» — суббота.
        assert_eq!(window_open_at(at(2026, 11, 27, 20, 0), &s), at(2026, 11, 30, 9, 0));
        s.mail_weekdays_only = false;
        assert_eq!(window_open_at(at(2026, 11, 27, 20, 0), &s), at(2026, 11, 28, 9, 0));
        // Конец не позже начала — без ограничения по часам.
        s.mail_window_end = 9;
        assert!(in_window(at(2026, 11, 28, 3, 0), &s));
        assert_eq!(with_footer("Текст\n", "Ответьте «нет»."), "Текст\n\nОтветьте «нет».");
        assert_eq!(with_footer("Текст\n\nОтветьте «нет».", "Ответьте «нет»."), "Текст\n\nОтветьте «нет».");
        assert_eq!(with_footer("Текст", " "), "Текст");
        for _ in 0..50 {
            let r = random_between(120, 300);
            assert!((120..=300).contains(&r));
        }
    }

    #[test]
    fn picks_business_email_not_vacancies() {
        let c = |v: &str, note: Option<&str>| sales::Contact { kind: "email".into(), value: v.into(), url: None, source: "site", note: note.map(str::to_string) };
        assert_eq!(pick_email(&[c("hr@x.by", Some("вакансии")), c("info@x.by", None)]).as_deref(), Some("info@x.by"));
        assert_eq!(pick_email(&[c("hr@x.by", Some("вакансии"))]).as_deref(), Some("hr@x.by"));
        assert_eq!(pick_email(&[]), None);
    }

    /// Поддельный SMTP без шифрования: принимает логин, отклоняет адреса `bad@…` (550), пишет письма в список.
    async fn fake_smtp(mails: Arc<Mutex<Vec<String>>>) -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (sock, _) = listener.accept().await.unwrap();
                let mails = mails.clone();
                tokio::spawn(async move {
                    let (r, mut w) = sock.into_split();
                    let mut r = BufReader::new(r);
                    w.write_all(b"220 fake ESMTP\r\n").await.unwrap();
                    let mut line = String::new();
                    loop {
                        line.clear();
                        if r.read_line(&mut line).await.unwrap_or(0) == 0 {
                            return;
                        }
                        let cmd = line.trim_end().to_uppercase();
                        let reply: &[u8] = if cmd.starts_with("EHLO") {
                            b"250-fake\r\n250 AUTH PLAIN LOGIN\r\n"
                        } else if cmd.starts_with("AUTH") {
                            b"235 ok\r\n"
                        } else if cmd.starts_with("RCPT") && cmd.contains("BAD@") {
                            b"550 5.1.1 no such user\r\n"
                        } else if cmd.starts_with("DATA") {
                            w.write_all(b"354 go\r\n").await.unwrap();
                            let mut data = String::new();
                            loop {
                                line.clear();
                                r.read_line(&mut line).await.unwrap();
                                if line == ".\r\n" {
                                    break;
                                }
                                data.push_str(&line);
                            }
                            mails.lock().unwrap().push(data);
                            b"250 queued\r\n"
                        } else if cmd.starts_with("QUIT") {
                            w.write_all(b"221 bye\r\n").await.unwrap();
                            return;
                        } else {
                            b"250 ok\r\n"
                        };
                        w.write_all(reply).await.unwrap();
                    }
                });
            }
        });
        port
    }

    async fn call(app: Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let req = Request::builder().method(method).uri(uri).header("content-type", "application/json").body(body.map(|b| Body::from(b.to_string())).unwrap_or(Body::empty())).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn queue_send_and_status_change() {
        let mails = Arc::new(Mutex::new(Vec::new()));
        let port = fake_smtp(mails.clone()).await;
        let db = crate::db::Db::in_memory().unwrap();
        let sid = db.insert_scan("Тест".into(), "map", json!({"center": {"lat": 1.0, "lon": 1.0}, "radius_m": 1000.0, "providers": ["osm"], "query": "", "preset": "dentist", "country": "by", "city_names": ["Мінск", "Минск"]})).await.unwrap();
        let card = |name: &str| Business { name: name.into(), website: Some(format!("https://{}.by", name.to_lowercase())), ..Default::default() };
        db.insert_businesses(sid, vec![card("Белодент"), card("Безпочты"), card("Плохой")]).await.unwrap();
        let ids: Vec<i64> = db.scan_businesses(sid, false).await.unwrap().into_iter().map(|x| x.0).collect();
        // Почты — добавленные вручную контакты.
        for (id, email) in [(ids[0], "info@belodent.by"), (ids[2], "bad@plohoi.by")] {
            let patch = crate::db::MetaPatch { extra_contacts: Some(json!([{"kind": "email", "value": email}])), ..Default::default() };
            db.update_business_meta(id, patch).await.unwrap();
        }
        let results = json!([{"id": "B05", "status": "fail", "value": "no_redirect", "detail": ""}]);
        let report = json!({"mini": {"state": "fair", "headline": "Средний сайт", "problems": [], "offer": [], "fail_ids": []}});
        for id in &ids {
            db.save_audit(*id, "warm".into(), Some(60.0), 100.0, report.clone(), results.clone(), json!({"final_url": "https://x.by/"}), 5, Value::Null, None).await.unwrap();
        }
        let st = crate::jobs::AppState::with_ai(db, None).await.unwrap();
        let app = crate::api::router(st.clone());

        // Не настроено — запуск отказывает с подсказкой.
        let (code, v) = call(app.clone(), "POST", "/api/mail/start", None).await;
        assert_eq!(code, StatusCode::BAD_REQUEST, "{v}");
        let (code, _) = call(
            app.clone(),
            "PUT",
            "/api/settings",
            Some(json!({"smtp_host": "127.0.0.1", "smtp_port": port, "smtp_user": "me@example.com", "smtp_password": "pw",
                        "mail_from_name": "Никита", "mail_attach_report": false, "mail_window_end": 0, "mail_weekdays_only": false, "mail_interval_min": 10, "mail_interval_max": 10})),
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        // «plain» сохраняется только для локального сервера (хост уже 127.0.0.1).
        call(app.clone(), "PUT", "/api/settings", Some(json!({"smtp_security": "plain"}))).await;
        assert_eq!(st.settings.read().await.smtp_security, "plain");

        // Пробное письмо — себе.
        let (code, v) = call(app.clone(), "POST", "/api/mail/test", Some(json!({}))).await;
        assert_eq!(code, StatusCode::OK, "{v}");
        assert_eq!(v["to"], "me@example.com");

        // В очередь шаблоном: без почты — пропуск с причиной.
        let (code, v) = call(app.clone(), "POST", "/api/mail/queue", Some(json!({"ids": ids, "source": "draft_or_template"}))).await;
        assert_eq!(code, StatusCode::OK, "{v}");
        assert_eq!(v["queued"], 2, "{v}");
        assert_eq!(v["skipped"][0]["reason"], "нет почты");
        let (_, v) = call(app.clone(), "POST", "/api/mail/queue", Some(json!({"ids": [ids[0]], "source": "template"}))).await;
        assert_eq!(v["skipped"][0]["reason"], "уже в очереди");

        // Шаги рассылки: Белодент уходит (статус «Написали», журнал, шаг), плохой адрес — в ошибки.
        assert!(set_running(&st, true).await.is_ok());
        step(&st).await;
        st.mail.info().next_send = None;
        step(&st).await;
        let sent = mails.lock().unwrap().clone();
        assert_eq!(sent.len(), 2, "проба + одно письмо");
        let letter = &sent[1];
        assert!(letter.contains("To: info@belodent.by") && letter.contains("List-Unsubscribe: <mailto:me@example.com?subject=unsubscribe>"), "{letter}");
        let (_, biz) = call(app.clone(), "GET", &format!("/api/businesses/{}", ids[0]), None).await;
        assert_eq!(biz["business"]["lead_status"], "contacted");
        assert_eq!(biz["business"]["next_action"], "Проверить ответ");
        let journal: Vec<&str> = biz["activities"].as_array().unwrap().iter().filter_map(|a| a["text"].as_str()).collect();
        assert!(journal.iter().any(|t| t.starts_with("Рассылка на info@belodent.by")), "{journal:?}");
        let m = &biz["messages"][0];
        assert_eq!(m["status"], "sent");
        assert!(m["message_id"].as_str().unwrap().ends_with("@example.com>"));
        assert!(m["body"].as_str().unwrap().ends_with("больше писать не буду."), "строка отказа в итоговом тексте");

        let (_, out) = call(app.clone(), "GET", "/api/mail/outbox", None).await;
        let statuses: Vec<&str> = out.as_array().unwrap().iter().map(|m| m["status"].as_str().unwrap()).collect();
        assert_eq!(statuses, vec!["failed", "sent"]);
        assert!(out[0]["error"].as_str().unwrap().contains("550"));
        let (_, stv) = call(app.clone(), "GET", "/api/mail/status", None).await;
        assert_eq!((stv["sent_today"].as_i64(), stv["queued"].as_i64(), stv["failed"].as_i64()), (Some(1), Some(0), Some(1)), "{stv}");
        assert_eq!(stv["running"], true);

        // Повторно тем же — «уже отправляли»; ошибочное можно вернуть в черновики.
        let (_, v) = call(app.clone(), "POST", "/api/mail/queue", Some(json!({"ids": [ids[0]], "source": "template", "skip_contacted": false}))).await;
        assert_eq!(v["skipped"][0]["reason"], "письмо уже отправляли");
        let failed_id = out[0]["id"].as_i64().unwrap();
        let (code, _) = call(app.clone(), "POST", &format!("/api/messages/{failed_id}/unqueue"), None).await;
        assert_eq!(code, StatusCode::OK);

        // Свой текст с карточки компании → в очередь; пауза останавливает отправку.
        let (code, m) = call(app.clone(), "POST", &format!("/api/businesses/{}/mail", ids[1]), Some(json!({"to": "owner@bezpochty.by", "subject": "Тема", "body": "Текст"}))).await;
        assert_eq!((code, m["status"].as_str()), (StatusCode::OK, Some("queued")), "{m}");
        let (code, _) = call(app.clone(), "POST", &format!("/api/businesses/{}/mail", ids[1]), Some(json!({"to": "не адрес", "subject": "Т", "body": "Т"}))).await;
        assert_eq!(code, StatusCode::BAD_REQUEST);
        assert!(set_running(&st, false).await.is_ok());
        st.mail.info().next_send = None;
        step(&st).await;
        assert_eq!(mails.lock().unwrap().len(), 2, "на паузе не отправляет");

        // Почтовый сервер недоступен — рассылка встаёт с понятной причиной, письмо остаётся в очереди.
        call(app.clone(), "PUT", "/api/settings", Some(json!({"smtp_port": 1}))).await;
        assert!(set_running(&st, true).await.is_ok());
        step(&st).await;
        assert!(!st.settings.read().await.mail_running);
        let reason = st.mail.info().stop_reason.clone().unwrap();
        assert!(reason.contains("Ошибка почты") || reason.contains("не отвечает"), "{reason}");
        let (_, stv) = call(app, "GET", "/api/mail/status", None).await;
        assert_eq!(stv["queued"], 1);
    }
}
