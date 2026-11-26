//! Рассылка писем по почте (SMTP). Менеджер сам ставит письма в очередь — черновики нейросети, шаблон
//! или свой текст — и запускает рассылку. Письма уходят по одному: случайная пауза между письмами,
//! дневной лимит, только по будням и днём по местному времени получателя. После отправки — статус «Написали»,
//! касание в журнале и шаг «Проверить ответ» через 3 дня. Сбои почты (логин, связь, отказы подряд) ставят
//! рассылку на паузу с причиной.
//!
//! Без спама: одно письмо на сайт (домен), на адрес и на компанию — по всем сканам, повторных писем нет; сетям и
//! госорганизациям не пишем. Правила проверяются при постановке в очередь и ещё раз прямо перед отправкой
//! (`MailGuard`). Вход в почту — паролем приложения или через аккаунт Google/Яндекс/Microsoft (`oauth.rs`).

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, RandomState};
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Days, Local, TimeZone as _, Utc};
use jiff::tz::TimeZone;
use jiff::{Timestamp, ToSpan};
use lettre::message::header::{ContentType, HeaderName, HeaderValue};
use lettre::message::{Attachment, Mailbox, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::{Credentials, Mechanism};
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Notify;
use tracing::{info, warn};

use scanner_core::lexicon::is_free_email;
use scanner_core::mini::SiteState;
use scanner_core::public_sector::public_sector;
use scanner_core::sales;
use scanner_core::scoring::CHAIN_MIN;
use scanner_core::text::{classify_website, registrable_domain, SiteKind};

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
    /// Вход в почту через аккаунт: незавершённые входы, итог, обновление токена.
    pub oauth: crate::oauth::OAuthState,
    /// Подменённое «сейчас» — только для проверок.
    clock: std::sync::Mutex<Option<Timestamp>>,
}

/// Что сейчас с рассылкой — для страницы «Рассылка».
#[derive(Debug, Clone, Serialize, Default)]
pub struct MailInfo {
    /// Когда уйдёт следующее письмо (если рассылка идёт и очередь не пуста).
    pub next_at: Option<DateTime<Utc>>,
    /// Чего ждём: рабочего времени получателей, завтрашнего лимита, паузы между письмами, писем в очереди.
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

    fn now(&self) -> Timestamp {
        self.clock.lock().unwrap_or_else(|e| e.into_inner()).unwrap_or_else(Timestamp::now)
    }

    #[cfg(test)]
    fn set_clock(&self, t: Timestamp) {
        *self.clock.lock().unwrap_or_else(|e| e.into_inner()) = Some(t);
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
        .merge(crate::oauth::routes())
}

// ---------------------------------------------------------------- рабочее время получателя

/// Часовой пояс получателя: по стране скана и координатам компании; страна неизвестна — по долготе;
/// нет и координат — ваш.
fn recipient_tz(country: Option<&str>, lat: Option<f64>, lon: Option<f64>) -> TimeZone {
    let point = lat.zip(lon).filter(|(la, lo)| *la != 0.0 || *lo != 0.0);
    let (la, lo) = point.unwrap_or((0.0, 0.0));
    if let Some(tz) = scanner_core::region::time_zone(country, la, lo).and_then(|name| TimeZone::get(name).ok()) {
        return tz;
    }
    if point.is_some()
        && let Ok(off) = jiff::tz::Offset::from_hours((lo / 15.0).round().clamp(-12.0, 14.0) as i8)
    {
        return TimeZone::fixed(off);
    }
    TimeZone::system()
}

/// Рабочее время получателя: будни, с `mail_window_start` до `mail_window_end` по его часам.
fn in_window(t: Timestamp, tz: &TimeZone, s: &Settings) -> bool {
    let z = t.to_zoned(tz.clone());
    let (a, b) = s.mail_window();
    let hour = z.hour() as u32;
    z.weekday().to_monday_one_offset() <= 5 && hour >= a && hour < b
}

/// Ближайший момент не раньше `t`, когда у получателя рабочее время.
fn window_open_at(t: Timestamp, tz: &TimeZone, s: &Settings) -> Timestamp {
    if in_window(t, tz, s) {
        return t;
    }
    let (start, _) = s.mail_window();
    let today = t.to_zoned(tz.clone()).date();
    for d in 0..9i64 {
        let Ok(day) = today.checked_add(d.days()) else { break };
        let Ok(cand) = day.at(start as i8, 0, 0, 0).to_zoned(tz.clone()) else { continue };
        let cand = cand.timestamp();
        if cand >= t && in_window(cand, tz, s) {
            return cand;
        }
    }
    t.checked_add(1.hour()).unwrap_or(t)
}

fn utc(t: Timestamp) -> DateTime<Utc> {
    DateTime::from_timestamp(t.as_second(), 0).unwrap_or_else(Utc::now)
}

fn secs_until(from: Timestamp, to: Timestamp) -> Duration {
    Duration::from_secs((to.as_second() - from.as_second()).max(0) as u64)
}

/// Начало сегодняшнего дня по вашему времени — в UTC, для подсчёта писем за сегодня (лимит — репутация вашего ящика).
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

// ---------------------------------------------------------------- без спама: одно письмо на сайт, не сетям, не госорганизациям

/// Региональные зоны, которых нет в списке публичных суффиксов: smart.minsk.by и anatomy.minsk.by — разные компании.
const SHARED_ZONES: &[&str] = &["minsk.by", "brest.by", "gomel.by", "grodno.by", "vitebsk.by", "mogilev.by", "net.by", "org.by"];

/// Ключ сайта для правила «одно письмо на сайт»: регистрируемый домен своего сайта; поддомен конструктора
/// (у каждого сайта на Тильде свой) и сайт в региональной зоне — целиком; соцсети, агрегаторы и мессенджеры —
/// не сайт компании.
fn site_key(url_or_host: &str) -> Option<String> {
    let (kind, host) = classify_website(url_or_host);
    match kind {
        SiteKind::Own => {
            let reg = registrable_domain(&host);
            Some(if SHARED_ZONES.contains(&reg.as_str()) && host != reg { host } else { reg })
        }
        SiteKind::ConstructorSubdomain => Some(host),
        _ => None,
    }
}

/// Ключи «кому уже писали»: компания, сайт, адрес и домен адреса (кроме бесплатной почты).
fn contact_keys(business_id: i64, host: Option<&str>, to: Option<&str>) -> Vec<String> {
    let mut keys = vec![format!("b:{business_id}")];
    if let Some(d) = host.and_then(site_key) {
        keys.push(format!("d:{d}"));
    }
    if let Some(to) = to.map(str::trim).filter(|t| t.contains('@')) {
        keys.push(format!("a:{}", to.to_lowercase()));
        let domain = to.rsplit('@').next().unwrap_or("").to_lowercase();
        if !is_free_email(&domain)
            && let Some(d) = site_key(&domain)
        {
            keys.push(format!("d:{d}"));
        }
    }
    let mut seen = HashSet::new();
    keys.retain(|k| seen.insert(k.clone()));
    keys
}

#[derive(Clone)]
struct Prior {
    name: String,
    sent: bool,
    at: Option<String>,
}

fn short_date(rfc: &str) -> String {
    DateTime::parse_from_rfc3339(rfc).map(|d| d.with_timezone(&Local).format(" %d.%m.%Y").to_string()).unwrap_or_default()
}

fn lower_first(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_lowercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

/// Правила рассылки против спама. Собирается один раз на партию (и перед каждой отправкой).
pub(crate) struct MailGuard {
    /// Ключ (компания, сайт, адрес) → письмо, которое уже ушло или стоит в очереди.
    prior: HashMap<String, Prior>,
    /// Сколько разных точек во всех сканах проверялись по этому сайту (хост без «www.»).
    site_points: HashMap<String, usize>,
}

impl MailGuard {
    /// Для постановки в очередь: мешают отправленные и стоящие в очереди письма. `exclude` — письмо, которое
    /// сейчас ставится: само себе оно не помеха.
    pub(crate) async fn load(st: &Shared, exclude: Option<i64>) -> anyhow::Result<Self> {
        Self::load_with(st, exclude, true).await
    }

    /// Прямо перед отправкой: мешают только ушедшие письма (иначе два письма одному сайту в очереди
    /// не пустили бы друг друга, и не ушло бы ни одно).
    async fn before_send(st: &Shared, mid: i64) -> anyhow::Result<Self> {
        Self::load_with(st, Some(mid), false).await
    }

    async fn load_with(st: &Shared, exclude: Option<i64>, queued_too: bool) -> anyhow::Result<Self> {
        let mut prior: HashMap<String, Prior> = HashMap::new();
        for c in st.db.mail_contacts().await? {
            if Some(c.message_id) == exclude || (!queued_too && c.status == "queued") {
                continue;
            }
            let state: SiteState = c.state.as_deref().and_then(|x| serde_json::from_value(json!(x)).ok()).unwrap_or_default();
            let host = sales::client_host(state, c.website.as_deref(), c.site_url.as_deref(), c.final_url.as_deref());
            let p = Prior { name: c.name.clone(), sent: c.status == "sent", at: c.at.clone() };
            for k in contact_keys(c.business_id, host.as_deref(), c.to_addr.as_deref()) {
                // Отправленное важнее стоящего в очереди — причина пропуска понятнее.
                let e = prior.entry(k).or_insert_with(|| p.clone());
                if p.sent && !e.sent {
                    *e = p.clone();
                }
            }
        }
        let mut points: HashMap<String, HashSet<(i64, i64)>> = HashMap::new();
        for (url, lat, lon) in st.db.site_points().await? {
            let (kind, host) = classify_website(&url);
            if kind == SiteKind::Own {
                // Одна точка в пересекающихся сканах считается один раз (клетка ≈ 100 м).
                let cell = ((lat.unwrap_or(0.0) * 1000.0).round() as i64, (lon.unwrap_or(0.0) * 1000.0).round() as i64);
                points.entry(host).or_default().insert(cell);
            }
        }
        Ok(Self { prior, site_points: points.into_iter().map(|(k, v)| (k, v.len())).collect() })
    }

    /// Почему этой компании нельзя писать на адрес `to`; None — можно.
    pub(crate) fn check(&self, b: &BusinessRow, ctx: &LeadCtx, to: &str) -> Option<String> {
        match ctx.state {
            SiteState::Closed => return Some("компания закрыта".into()),
            SiteState::Chain => {
                let headline = b.report["mini"]["headline"].as_str().filter(|h| !h.is_empty()).unwrap_or("Точка сети — сайтом занимается головная компания");
                return Some(lower_first(headline));
            }
            _ => {}
        }
        let host = ctx.host.as_deref().map(|h| h.trim_start_matches("www.").to_string());
        if let Some(h) = &host
            && let Some(n) = self.site_points.get(h).filter(|n| **n >= CHAIN_MIN as usize)
        {
            return Some(format!("сеть: сайт {h} у {n} точек в ваших сканах — сайтом занимается головная компания"));
        }
        // Только адрес письма: среди контактов сайта бывают чужие адреса (белорусские магазины обязаны указывать
        // почту исполкома для жалоб), по ним компания — не госорганизация.
        let mut names: Vec<&str> = vec![b.name.as_str()];
        names.extend(b.display_name.as_deref());
        names.extend(ctx.card.sources.iter().map(|p| p.name.as_str()));
        if let Some(why) = public_sector(host.as_deref(), &[to], &names, &ctx.card.operator_types()) {
            return Some(why);
        }
        for k in contact_keys(b.id, ctx.host.as_deref(), Some(to)) {
            let Some(p) = self.prior.get(&k) else { continue };
            let when = p.at.as_deref().map(short_date).unwrap_or_default();
            let (kind, value) = k.split_at(2);
            return Some(match (kind, p.sent) {
                ("b:", true) => format!("письмо уже отправляли{when}"),
                ("b:", false) => "уже в очереди".into(),
                ("d:", true) => format!("этому сайту ({value}) уже писали: «{}»{when}", p.name),
                ("d:", false) => format!("письмо этому сайту ({value}) уже в очереди: «{}»", p.name),
                (_, true) => format!("на этот адрес уже писали: «{}»{when}", p.name),
                (_, false) => format!("письмо на этот адрес уже в очереди: «{}»", p.name),
            });
        }
        None
    }

    /// Запомнить поставленное письмо, чтобы в той же партии не поставить второе на тот же сайт или адрес.
    fn remember(&mut self, b: &BusinessRow, ctx: &LeadCtx, to: &str) {
        let p = Prior { name: crate::api::client_name(b), sent: false, at: None };
        for k in contact_keys(b.id, ctx.host.as_deref(), Some(to)) {
            self.prior.entry(k).or_insert_with(|| p.clone());
        }
    }
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
    /// Почта не приняла вход через аккаунт — то же, что Stop, и короткий токен обновляется заново.
    AuthFailed(String),
    /// Не отправлено по правилам (уже писали этому сайту, сеть, госорганизация) — в «не ушли», без паузы.
    Skipped(String),
}

fn is_local(host: &str) -> bool {
    matches!(host.trim(), "localhost" | "127.0.0.1" | "::1")
}

/// Как входить на SMTP: логин и пароль приложения или токен входа через аккаунт (XOAUTH2).
struct SmtpAuth {
    user: String,
    secret: String,
    xoauth2: bool,
}

async fn smtp_auth(st: &Shared, s: &Settings) -> Result<SmtpAuth, String> {
    if s.oauth() {
        let (user, token) = crate::oauth::smtp_token(st).await?;
        Ok(SmtpAuth { user, secret: token, xoauth2: true })
    } else {
        Ok(SmtpAuth { user: s.smtp_user.trim().to_string(), secret: s.smtp_password.clone(), xoauth2: false })
    }
}

fn transport(s: &Settings, auth: &SmtpAuth) -> Result<AsyncSmtpTransport<Tokio1Executor>, String> {
    let host = s.smtp_host.trim();
    let b = match s.smtp_security.as_str() {
        "starttls" => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(host).map_err(|e| format!("Почтовый сервер {host}: {e}"))?,
        // Без шифрования — только для почтового сервера на этом же компьютере.
        "plain" if is_local(host) => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host),
        _ => AsyncSmtpTransport::<Tokio1Executor>::relay(host).map_err(|e| format!("Почтовый сервер {host}: {e}"))?,
    };
    let mut b = b.port(s.smtp_port).credentials(Credentials::new(auth.user.clone(), auth.secret.clone())).timeout(Some(Duration::from_secs(60)));
    if auth.xoauth2 {
        b = b.authentication(vec![Mechanism::Xoauth2]);
    }
    Ok(b.build())
}

fn sender(s: &Settings) -> Result<Mailbox, String> {
    let addr = s.mail_from().parse().map_err(|_| format!("Неверный адрес отправителя «{}»", s.mail_from()))?;
    let name = s.mail_from_name.trim();
    Ok(Mailbox::new((!name.is_empty()).then(|| name.to_string()), addr))
}

/// Понятное объяснение ошибки SMTP и что с ней делать.
fn classify(e: &lettre::transport::smtp::Error, xoauth2: bool) -> Outcome {
    let code: Option<u16> = e.status().map(u16::from);
    let text = e.to_string();
    match code {
        Some(c @ (530 | 534 | 535 | 538)) if xoauth2 => {
            Outcome::AuthFailed(format!("Почта не приняла вход через аккаунт ({c}): {text}. Если повторится — войдите заново: Настройки → Почта для рассылки."))
        }
        Some(c @ (530 | 534 | 535 | 538)) => {
            Outcome::Stop(format!("Почта не приняла логин или пароль ({c}): {text}. Для Gmail, Яндекса и Mail.ru нужен пароль приложения, а не обычный пароль — или войдите через аккаунт."))
        }
        Some(c) if e.is_permanent() && matches!(c, 501 | 550 | 551 | 552 | 553 | 554 | 555) => Outcome::Rejected(format!("Адрес или письмо отклонены ({c}): {text}")),
        Some(c) if e.is_transient() => Outcome::Later(format!("Почтовый сервер просит подождать ({c}): {text}")),
        _ if e.is_timeout() => Outcome::Stop(format!("Почтовый сервер не отвечает: {text}")),
        _ if e.is_tls() => Outcome::Stop(format!("Не удалось установить защищённое соединение — проверьте порт и шифрование (465 — SSL, 587 — STARTTLS): {text}")),
        _ => Outcome::Stop(format!("Ошибка почты: {text}")),
    }
}

/// Собрать и отправить письмо. `pdf` — (имя файла, содержимое).
async fn deliver(s: &Settings, auth: &SmtpAuth, to: &str, subject: &str, body: &str, pdf: Option<(String, Vec<u8>)>) -> Outcome {
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
    let tr = match transport(s, auth) {
        Ok(t) => t,
        Err(e) => return Outcome::Stop(e),
    };
    match tr.send(email).await {
        Ok(_) => Outcome::Sent(message_id),
        Err(e) => classify(&e, auth.xoauth2),
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

async fn send_one(st: &Shared, s: &Settings, m: Message) -> Outcome {
    let Some(b) = st.db.get_business(m.business_id).await.ok().flatten() else {
        return Outcome::Rejected("Компания удалена".into());
    };
    let to = m.to_addr.clone().unwrap_or_default();
    let scan = st.db.get_scan(b.scan_id).await.ok().flatten();
    let ctx = LeadCtx::new(&b, scan.as_ref(), s);
    // Правила — ещё раз прямо перед отправкой: за время в очереди могли написать этому сайту с другой карточки,
    // а пересчёт мог отметить компанию сетью.
    match MailGuard::before_send(st, m.id).await {
        Ok(guard) => {
            if let Some(why) = guard.check(&b, &ctx, &to) {
                return Outcome::Skipped(format!("Не отправлено: {why}"));
            }
        }
        Err(e) => return Outcome::Stop(format!("Не проверить, писали ли этому сайту: {e:#}")),
    }
    let lang = sales::outreach_lang(ctx.country(), &ctx.cities);
    let pdf = if s.mail_attach_report && !no_site(ctx.state) {
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
    let auth = match smtp_auth(st, s).await {
        Ok(a) => a,
        Err(e) => return Outcome::Stop(e),
    };
    let footer = if lang == "en" { &s.mail_footer_en } else { &s.mail_footer };
    let body = with_footer(&m.body, footer);
    let subject = m.subject.clone().unwrap_or_default();
    let out = deliver(s, &auth, &to, &subject, &body, pdf).await;
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

/// Почта не приняла короткий токен — в следующий раз взять новый.
async fn forget_access_token(st: &Shared) {
    let mut s = st.settings.read().await.clone();
    s.oauth_expires_at = 0;
    if let Err(e) = st.save_settings(s).await {
        warn!("рассылка: не сбросить токен входа: {e:#}");
    }
}

fn set_wait(st: &Shared, why: &str, at: Option<DateTime<Utc>>) {
    let mut i = st.mail.info();
    i.waiting = Some(why.to_string());
    i.next_at = at;
}

/// Одно письмо очереди, у получателя которого сейчас рабочее время; иначе — когда оно наступит у первого.
async fn next_in_window(st: &Shared, s: &Settings, now: Timestamp) -> anyhow::Result<Result<i64, Option<Timestamp>>> {
    let mut earliest: Option<Timestamp> = None;
    for q in st.db.queued_mail().await? {
        let tz = recipient_tz(q.country.as_deref(), q.lat, q.lon);
        let open = window_open_at(now, &tz, s);
        if open <= now {
            return Ok(Ok(q.message_id));
        }
        earliest = Some(earliest.map_or(open, |e| e.min(open)));
    }
    Ok(Err(earliest))
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
    let now = st.mail.now();
    let local = utc(now).with_timezone(&Local);
    let sent_today = st.db.mail_sent_since(today_start_utc(local)).await.unwrap_or(0);
    if sent_today >= s.mail_daily_limit as i64 {
        let tomorrow = local.date_naive().checked_add_days(Days::new(1)).and_then(|d| d.and_hms_opt(0, 0, 0)).and_then(|n| Local.from_local_datetime(&n).earliest()).unwrap_or(local);
        set_wait(st, "дневной лимит исчерпан — продолжит завтра", Some(tomorrow.with_timezone(&Utc)));
        return (tomorrow - local).to_std().unwrap_or_default().min(Duration::from_secs(1800));
    }
    let next_send = st.mail.info().next_send;
    if let Some(at) = next_send
        && at > Utc::now()
    {
        set_wait(st, "пауза между письмами", Some(at));
        return (at - Utc::now()).to_std().unwrap_or_default();
    }
    let mid = match next_in_window(st, &s, now).await {
        Ok(Ok(mid)) => mid,
        Ok(Err(None)) => {
            set_wait(st, "очередь пуста — добавьте письма", None);
            return Duration::from_secs(3600);
        }
        Ok(Err(Some(at))) => {
            set_wait(st, "ждёт рабочего времени получателей — по будням, днём по их местному времени", Some(utc(at)));
            return secs_until(now, at).min(Duration::from_secs(900));
        }
        Err(e) => {
            warn!("рассылка: очередь: {e:#}");
            return Duration::from_secs(60);
        }
    };
    let m = match st.db.claim_mail(mid).await {
        Ok(Some(m)) => m,
        Ok(None) => return Duration::from_millis(200),
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
    let mut sent_something = true;
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
        Outcome::Skipped(e) => {
            info!("рассылка: письмо {} не отправлено по правилам: {e}", m.id);
            sent_something = false;
            st.db.finish_mail(m.id, "failed", None, Some(e)).await
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
        Outcome::AuthFailed(e) => {
            forget_access_token(st).await;
            stop = Some(e.clone());
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
        if sent_something && i.next_send.is_none_or(|t| t <= Utc::now()) {
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

/// Почта компании для рассылки: первая не для вакансий и не на госдомене (почта исполкома для жалоб на сайте
/// магазина — не адрес магазина), иначе любая — тогда правила объяснят, почему писать нельзя.
fn pick_email(contacts: &[sales::Contact]) -> Option<String> {
    let emails: Vec<&sales::Contact> = contacts.iter().filter(|c| c.kind == "email" && c.value.contains('@')).collect();
    let gov = |c: &&sales::Contact| scanner_core::public_sector::gov_domain(c.value.rsplit('@').next().unwrap_or(""));
    let vacancy = |c: &&sales::Contact| c.note.as_deref().is_some_and(|n| n.contains("ваканс"));
    emails
        .iter()
        .find(|c| !vacancy(c) && !gov(c))
        .or_else(|| emails.iter().find(|c| !gov(c)))
        .or(emails.first())
        .map(|c| c.value.trim().to_string())
}

/// Поставить готовый черновик в рассылку (массовая генерация «сразу в рассылку»). Ошибка — причина словами.
pub async fn enqueue_draft(st: &Shared, bid: i64, mid: i64) -> Result<(), String> {
    let b = st.db.get_business(bid).await.ok().flatten().ok_or("компания не найдена")?;
    let scan = st.db.get_scan(b.scan_id).await.ok().flatten();
    let s = st.settings.read().await.clone();
    let ctx = LeadCtx::new(&b, scan.as_ref(), &s);
    let contacts = crate::api::lead_contacts(&b, &ctx);
    let to = pick_email(&contacts).ok_or("у компании нет почты")?;
    let guard = MailGuard::load(st, Some(mid)).await.map_err(|e| e.to_string())?;
    if let Some(why) = guard.check(&b, &ctx, &to) {
        return Err(why);
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
    let mut guard = MailGuard::load(&st, None).await?;
    let mut queued = 0;
    let mut skipped: Vec<Skipped> = Vec::new();
    let mut seen = HashSet::new();
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
        let contacts = crate::api::lead_contacts(&b, &ctx);
        let Some(to) = pick_email(&contacts) else {
            skip("нет почты");
            continue;
        };
        if let Some(why) = guard.check(&b, &ctx, &to) {
            skip(&why);
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
        if st.db.queue_message(mid, to.clone(), None, None).await?.is_some() {
            guard.remember(&b, &ctx, &to);
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
    let b = st.db.get_business(id).await?.ok_or_else(not_found)?;
    let (to, subject, body) = (r.to.trim().to_string(), r.subject.trim().to_string(), r.body.trim().to_string());
    if to.parse::<Mailbox>().is_err() {
        return Err(bad("Неверный адрес почты получателя."));
    }
    if subject.is_empty() || body.is_empty() {
        return Err(bad("Нужны тема и текст письма."));
    }
    if let Some(mid) = r.message_id {
        let m = st.db.get_message(mid).await?.filter(|m| m.business_id == id && m.kind == "email").ok_or_else(|| bad("черновик не от этой компании"))?;
        if m.status == "sent" || m.status == "sending" {
            return Err(bad("Это письмо уже отправлено."));
        }
    }
    let s = st.settings.read().await.clone();
    let scan = st.db.get_scan(b.scan_id).await?;
    let ctx = LeadCtx::new(&b, scan.as_ref(), &s);
    let guard = MailGuard::load(&st, r.message_id).await?;
    if let Some(why) = guard.check(&b, &ctx, &to) {
        return Err(bad(format!("Не поставлено в рассылку: {why}. Одно письмо на сайт и адрес, сетям и госорганизациям рассылка не пишет.")));
    }
    let mid = match r.message_id {
        Some(mid) => mid,
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
    let now = st.mail.now();
    let (queued, failed) = st.db.mail_counts().await?;
    let sent_today = st.db.mail_sent_since(today_start_utc(utc(now).with_timezone(&Local))).await?;
    let info = st.mail.info().clone();
    let (start, end) = s.mail_window();
    Ok(json!({
        "configured": s.mail_configured(),
        "running": s.mail_running,
        "from": s.mail_from(),
        "auth": if s.oauth() { s.oauth_provider.as_str() } else { "password" },
        "queued": queued,
        "failed": failed,
        "sent_today": sent_today,
        "daily_limit": s.mail_daily_limit,
        "window": {"start": start, "end": end, "weekdays_only": true, "recipient_time": true},
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

/// Пробное письмо — себе (на адрес отправителя) или на указанный адрес; сразу, мимо очереди, лимитов и окна.
async fn test_mail(State(st): State<Shared>, body: Option<Json<TestReq>>) -> ApiResult<Json<Value>> {
    let s = st.settings.read().await.clone();
    if !s.mail_configured() {
        let what = if s.oauth() { "Войдите через аккаунт почты" } else { "Заполните сервер, логин, пароль и адрес отправителя и нажмите «Сохранить»" };
        return Err(bad(format!("{what}.")));
    }
    let to = body.and_then(|b| b.0.to).map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).unwrap_or_else(|| s.mail_from());
    let text = "Если вы читаете это письмо, почта для рассылки в сканере карт настроена правильно.\n\nОтправлено кнопкой «Проверить» в настройках.";
    let auth = smtp_auth(&st, &s).await.map_err(|e| ApiError(StatusCode::BAD_GATEWAY, e))?;
    match deliver(&s, &auth, &to, "Проверка почты для рассылки", text, None).await {
        Outcome::Sent(id) => Ok(Json(json!({"ok": true, "to": to, "message_id": id}))),
        Outcome::AuthFailed(e) => {
            forget_access_token(&st).await;
            Err(ApiError(StatusCode::BAD_GATEWAY, e))
        }
        Outcome::Rejected(e) | Outcome::Later(e) | Outcome::Stop(e) | Outcome::Skipped(e) => Err(ApiError(StatusCode::BAD_GATEWAY, e)),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::body::Body;
    use axum::http::Request;
    use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
    use base64::Engine;
    use http_body_util::BodyExt;
    use scanner_core::Business;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tower::ServiceExt;

    use super::*;

    /// Момент по местному времени пояса `tz`.
    fn at(tz: &str, y: i16, mo: i8, d: i8, h: i8, mi: i8) -> Timestamp {
        jiff::civil::date(y, mo, d).at(h, mi, 0, 0).to_zoned(TimeZone::get(tz).unwrap()).unwrap().timestamp()
    }

    #[test]
    fn recipient_working_hours_and_footer() {
        let s = Settings { mail_window_start: 9, mail_window_end: 18, ..Default::default() };
        let minsk = recipient_tz(Some("by"), Some(53.9), Some(27.56));
        assert_eq!(minsk.iana_name(), Some("Europe/Minsk"));
        // Среда 25.11.2026: 08:30 → 9:00; 12:00 — можно; 18:00 — до четверга 9:00.
        assert_eq!(window_open_at(at("Europe/Minsk", 2026, 11, 25, 8, 30), &minsk, &s), at("Europe/Minsk", 2026, 11, 25, 9, 0));
        assert_eq!(window_open_at(at("Europe/Minsk", 2026, 11, 25, 12, 0), &minsk, &s), at("Europe/Minsk", 2026, 11, 25, 12, 0));
        assert_eq!(window_open_at(at("Europe/Minsk", 2026, 11, 25, 18, 0), &minsk, &s), at("Europe/Minsk", 2026, 11, 26, 9, 0));
        // Пятница вечером и суббота → понедельник: выходных нет.
        assert_eq!(window_open_at(at("Europe/Minsk", 2026, 11, 27, 20, 0), &minsk, &s), at("Europe/Minsk", 2026, 11, 30, 9, 0));
        assert!(!in_window(at("Europe/Minsk", 2026, 11, 28, 12, 0), &minsk, &s));
        // Нью-Йорк: в 12:00 по Минску там 4 утра — письмо ждёт 9:00 по Нью-Йорку (17:00 по Минску).
        let ny = recipient_tz(Some("us"), Some(40.71), Some(-74.0));
        let noon_minsk = at("Europe/Minsk", 2026, 11, 25, 12, 0);
        assert!(in_window(noon_minsk, &minsk, &s) && !in_window(noon_minsk, &ny, &s));
        assert_eq!(window_open_at(noon_minsk, &ny, &s), at("America/New_York", 2026, 11, 25, 9, 0));
        assert_eq!(window_open_at(noon_minsk, &ny, &s), at("Europe/Minsk", 2026, 11, 25, 17, 0));
        // Старое «круглые сутки» в базе — всё равно 9–18.
        let legacy = Settings { mail_window_start: 9, mail_window_end: 0, ..Default::default() };
        assert!(!in_window(at("Europe/Minsk", 2026, 11, 25, 3, 0), &minsk, &legacy));
        // Страна неизвестна — пояс по долготе; нет и координат — свой.
        assert_eq!(recipient_tz(Some("xx"), Some(-33.9), Some(151.2)).to_offset(Timestamp::UNIX_EPOCH).seconds(), 10 * 3600);
        assert_eq!(with_footer("Текст\n", "Ответьте «нет»."), "Текст\n\nОтветьте «нет».");
        assert_eq!(with_footer("Текст\n\nОтветьте «нет».", "Ответьте «нет»."), "Текст\n\nОтветьте «нет».");
        assert_eq!(with_footer("Текст", " "), "Текст");
        for _ in 0..50 {
            let r = random_between(120, 300);
            assert!((120..=300).contains(&r));
        }
    }

    #[test]
    fn one_letter_keys() {
        assert_eq!(contact_keys(1, Some("www.belodent.by"), Some(" Info@Belodent.by ")), vec!["b:1", "d:belodent.by", "a:info@belodent.by"]);
        assert_eq!(contact_keys(2, Some("shop.belodent.by"), Some("me@gmail.com")), vec!["b:2", "d:belodent.by", "a:me@gmail.com"], "бесплатная почта — не сайт");
        assert_eq!(contact_keys(3, Some("instagram.com"), None), vec!["b:3"], "соцсеть — не сайт");
        assert_eq!(contact_keys(4, Some("salon.tilda.ws"), None), vec!["b:4", "d:salon.tilda.ws"], "сайт на конструкторе — свой поддомен");
        assert_eq!(contact_keys(5, None, Some("hello@studio.co.uk")), vec!["b:5", "a:hello@studio.co.uk", "d:studio.co.uk"]);
        assert_eq!(contact_keys(6, Some("smart.minsk.by"), None), vec!["b:6", "d:smart.minsk.by"], "региональная зона: разные компании");
        assert_eq!(contact_keys(7, Some("minsk.by"), None), vec!["b:7", "d:minsk.by"]);
    }

    #[test]
    fn picks_business_email_not_vacancies() {
        let c = |v: &str, note: Option<&str>| sales::Contact { kind: "email".into(), value: v.into(), url: None, source: "site", note: note.map(str::to_string) };
        assert_eq!(pick_email(&[c("hr@x.by", Some("вакансии")), c("info@x.by", None)]).as_deref(), Some("info@x.by"));
        assert_eq!(pick_email(&[c("hr@x.by", Some("вакансии"))]).as_deref(), Some("hr@x.by"));
        assert_eq!(pick_email(&[]), None);
        // Почта исполкома для жалоб на сайте магазина — не адрес магазина.
        assert_eq!(pick_email(&[c("tsr@minsk.gov.by", None), c("shop@cvety.by", None)]).as_deref(), Some("shop@cvety.by"));
        assert_eq!(pick_email(&[c("info@school5.minsk.edu.by", None)]).as_deref(), Some("info@school5.minsk.edu.by"), "только госпочта — правила объяснят пропуск");
    }

    /// Поддельный SMTP без шифрования: принимает любой вход (строки AUTH — в `auths`), отклоняет адреса `bad@…` (550),
    /// пишет письма в `mails`.
    async fn fake_smtp(mails: Arc<Mutex<Vec<String>>>, auths: Arc<Mutex<Vec<String>>>) -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (sock, _) = listener.accept().await.unwrap();
                let (mails, auths) = (mails.clone(), auths.clone());
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
                        let raw = line.trim_end().to_string();
                        let cmd = raw.to_uppercase();
                        let reply: &[u8] = if cmd.starts_with("EHLO") {
                            b"250-fake\r\n250 AUTH PLAIN LOGIN XOAUTH2\r\n"
                        } else if cmd.starts_with("AUTH") {
                            if raw.split_whitespace().count() == 2 {
                                // Вход без начального ответа: попросить его отдельной строкой.
                                w.write_all(b"334 \r\n").await.unwrap();
                                line.clear();
                                r.read_line(&mut line).await.unwrap();
                                auths.lock().unwrap().push(format!("{raw} {}", line.trim_end()));
                            } else {
                                auths.lock().unwrap().push(raw.clone());
                            }
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

    async fn call_raw(app: Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, String) {
        let req = Request::builder().method(method).uri(uri).header("content-type", "application/json").body(body.map(|b| Body::from(b.to_string())).unwrap_or(Body::empty())).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8_lossy(&bytes).to_string())
    }

    async fn call(app: Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let (status, text) = call_raw(app, method, uri, body).await;
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    /// Среда, полдень по Минску — рабочее время белорусских получателей.
    fn wednesday_noon() -> Timestamp {
        at("Europe/Minsk", 2026, 11, 25, 12, 0)
    }

    fn scan_params(country: &str) -> Value {
        json!({"center": {"lat": 53.9, "lon": 27.56}, "radius_m": 1000.0, "providers": ["osm"], "query": "", "preset": "dentist", "country": country, "city_names": ["Мінск", "Минск"]})
    }

    /// Проверенная компания: сайт `site` (свой домен), почта — добавленный вручную контакт.
    async fn audited(db: &crate::db::Db, id: i64, site: &str, email: Option<&str>, report: Value) {
        if let Some(email) = email {
            let patch = crate::db::MetaPatch { extra_contacts: Some(json!([{"kind": "email", "value": email}])), ..Default::default() };
            db.update_business_meta(id, patch).await.unwrap();
        }
        let results = json!([{"id": "B05", "status": "fail", "value": "no_redirect", "detail": ""}]);
        db.save_audit(id, "warm".into(), Some(60.0), 100.0, report, results, json!({"final_url": format!("https://{site}/")}), 5, Value::Null, Some(format!("https://{site}/")))
            .await
            .unwrap();
    }

    fn fair() -> Value {
        json!({"mini": {"state": "fair", "headline": "Средний сайт", "problems": [], "offer": [], "fail_ids": []}})
    }

    async fn configure_smtp(app: &Router, st: &Shared, port: u16) {
        let (code, _) = call(
            app.clone(),
            "PUT",
            "/api/settings",
            Some(json!({"smtp_host": "127.0.0.1", "smtp_port": port, "smtp_user": "me@example.com", "smtp_password": "pw",
                        "mail_from_name": "Никита", "mail_attach_report": false, "mail_interval_min": 10, "mail_interval_max": 10})),
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        // «plain» сохраняется только для локального сервера (хост уже 127.0.0.1).
        call(app.clone(), "PUT", "/api/settings", Some(json!({"smtp_security": "plain"}))).await;
        assert_eq!(st.settings.read().await.smtp_security, "plain");
    }

    #[tokio::test]
    async fn queue_send_and_status_change() {
        let (mails, auths) = (Arc::new(Mutex::new(Vec::new())), Arc::new(Mutex::new(Vec::new())));
        let port = fake_smtp(mails.clone(), auths.clone()).await;
        let db = crate::db::Db::in_memory().unwrap();
        let sid = db.insert_scan("Тест".into(), "map", scan_params("by")).await.unwrap();
        let card = |name: &str| Business { name: name.into(), website: Some(format!("https://{}.by", name.to_lowercase())), ..Default::default() };
        db.insert_businesses(sid, vec![card("Белодент"), card("Безпочты"), card("Плохой")]).await.unwrap();
        let ids: Vec<i64> = db.scan_businesses(sid, false).await.unwrap().into_iter().map(|x| x.0).collect();
        audited(&db, ids[0], "belodent.by", Some("info@belodent.by"), fair()).await;
        audited(&db, ids[1], "bezpochty.by", None, fair()).await;
        audited(&db, ids[2], "plohoi.by", Some("bad@plohoi.by"), fair()).await;
        let st = crate::jobs::AppState::with_ai(db, None).await.unwrap();
        st.mail.set_clock(wednesday_noon());
        let app = crate::api::router(st.clone());

        // Не настроено — запуск отказывает с подсказкой.
        let (code, v) = call(app.clone(), "POST", "/api/mail/start", None).await;
        assert_eq!(code, StatusCode::BAD_REQUEST, "{v}");
        configure_smtp(&app, &st, port).await;

        // Пробное письмо — себе, с логином и паролем.
        let (code, v) = call(app.clone(), "POST", "/api/mail/test", Some(json!({}))).await;
        assert_eq!(code, StatusCode::OK, "{v}");
        assert_eq!(v["to"], "me@example.com");
        assert!(auths.lock().unwrap()[0].starts_with("AUTH PLAIN"), "{:?}", auths.lock().unwrap());

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
        assert_eq!((stv["window"]["start"].as_u64(), stv["window"]["end"].as_u64(), stv["window"]["weekdays_only"].as_bool()), (Some(9), Some(18), Some(true)));

        // Повторно тем же — «уже отправляли», даже если снять «пропускать, с кем работаете»; ошибочное можно вернуть в черновики.
        let (_, v) = call(app.clone(), "POST", "/api/mail/queue", Some(json!({"ids": [ids[0]], "source": "template", "skip_contacted": false}))).await;
        assert!(v["skipped"][0]["reason"].as_str().unwrap().starts_with("письмо уже отправляли"), "{v}");
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

        // Суббота — письмо ждёт понедельника 9:00 по времени получателя.
        assert!(set_running(&st, true).await.is_ok());
        st.mail.set_clock(at("Europe/Minsk", 2026, 11, 28, 12, 0));
        step(&st).await;
        assert_eq!(mails.lock().unwrap().len(), 2, "в выходной не отправляет");
        let info = st.mail.info().clone();
        assert!(info.waiting.as_deref().unwrap().contains("рабочего времени"), "{info:?}");
        assert_eq!(info.next_at.map(|t| t.timestamp()), Some(at("Europe/Minsk", 2026, 11, 30, 9, 0).as_second()));
        st.mail.set_clock(wednesday_noon());

        // Почтовый сервер недоступен — рассылка встаёт с понятной причиной, письмо остаётся в очереди.
        call(app.clone(), "PUT", "/api/settings", Some(json!({"smtp_port": 1}))).await;
        step(&st).await;
        assert!(!st.settings.read().await.mail_running);
        let reason = st.mail.info().stop_reason.clone().unwrap();
        assert!(reason.contains("Ошибка почты") || reason.contains("не отвечает"), "{reason}");
        let (_, stv) = call(app, "GET", "/api/mail/status", None).await;
        assert_eq!(stv["queued"], 1);
    }

    #[tokio::test]
    async fn no_spam_one_letter_per_site_no_chains_no_public_sector() {
        let (mails, auths) = (Arc::new(Mutex::new(Vec::new())), Arc::new(Mutex::new(Vec::new())));
        let port = fake_smtp(mails.clone(), auths).await;
        let db = crate::db::Db::in_memory().unwrap();
        let sid = db.insert_scan("Минск".into(), "map", scan_params("by")).await.unwrap();
        let card = |name: &str, lat: f64| Business { name: name.into(), lat, lon: 27.5, ..Default::default() };
        let names = ["Белодент", "Белодент на Притыцкого", "Другая клиника", "ГУО «Средняя школа № 5 г. Минска»", "Евроопт", "Пицца Темпо", "Минздрав", "Свои зубы"];
        db.insert_businesses(sid, names.iter().enumerate().map(|(i, n)| card(n, 53.9 + i as f64 * 0.01)).collect()).await.unwrap();
        let ids: Vec<i64> = db.scan_businesses(sid, false).await.unwrap().into_iter().map(|x| x.0).collect();
        let chain = json!({"mini": {"state": "chain", "headline": "Точка сети «Евроопт» — сайтом занимается головная компания", "problems": [], "offer": [], "fail_ids": []}});
        audited(&db, ids[0], "belodent.by", Some("info@belodent.by"), fair()).await;
        audited(&db, ids[1], "www.belodent.by", Some("filial@belodent.by"), fair()).await;
        audited(&db, ids[2], "drugaya.by", Some("info@belodent.by"), fair()).await;
        audited(&db, ids[3], "school5.by", Some("school5@tut.by"), fair()).await;
        audited(&db, ids[4], "evroopt.by", Some("shop@evroopt.by"), chain).await;
        audited(&db, ids[5], "pizza.by", Some("tempo@pizza.by"), fair()).await;
        audited(&db, ids[6], "minzdrav.gov.by", Some("mz@minzdrav.gov.by"), fair()).await;
        audited(&db, ids[7], "svoizuby.by", Some("hello@svoizuby.by"), fair()).await;
        // Тот же сайт pizza.by ещё у пяти точек в другом скане — это сеть, хотя в первом скане точка одна.
        let other = db.insert_scan("Гродно".into(), "map", scan_params("by")).await.unwrap();
        db.insert_businesses(other, (0..5).map(|i| card("Пицца", 53.6 + i as f64 * 0.01)).collect()).await.unwrap();
        for (id, _) in db.scan_businesses(other, false).await.unwrap() {
            audited(&db, id, "pizza.by", None, fair()).await;
        }
        let st = crate::jobs::AppState::with_ai(db, None).await.unwrap();
        st.mail.set_clock(wednesday_noon());
        let app = crate::api::router(st.clone());
        configure_smtp(&app, &st, port).await;

        let (code, v) = call(app.clone(), "POST", "/api/mail/queue", Some(json!({"ids": ids, "source": "template"}))).await;
        assert_eq!(code, StatusCode::OK, "{v}");
        let reasons: HashMap<i64, String> = v["skipped"].as_array().unwrap().iter().map(|x| (x["id"].as_i64().unwrap(), x["reason"].as_str().unwrap().to_string())).collect();
        assert_eq!(v["queued"], 2, "Белодент и «Свои зубы»: {v}");
        assert!(reasons[&ids[1]].starts_with("письмо этому сайту (belodent.by) уже в очереди"), "{}", reasons[&ids[1]]);
        assert!(reasons[&ids[2]].starts_with("письмо на этот адрес уже в очереди"), "{}", reasons[&ids[2]]);
        assert!(reasons[&ids[3]].starts_with("госорганизация"), "{}", reasons[&ids[3]]);
        assert!(reasons[&ids[4]].starts_with("точка сети «Евроопт»"), "{}", reasons[&ids[4]]);
        assert!(reasons[&ids[5]].starts_with("сеть: сайт pizza.by у 6 точек"), "{}", reasons[&ids[5]]);
        assert!(reasons[&ids[6]].starts_with("государственный сайт"), "{}", reasons[&ids[6]]);

        // Вручную со страницы компании правила те же.
        let (code, v) = call(app.clone(), "POST", &format!("/api/businesses/{}/mail", ids[1]), Some(json!({"to": "filial@belodent.by", "subject": "Т", "body": "Т"}))).await;
        assert_eq!(code, StatusCode::BAD_REQUEST);
        assert!(v.as_str().or(v["error"].as_str()).unwrap_or(&v.to_string()).contains("этому сайту"), "{v}");

        // Письмо филиалу, попавшее в очередь в обход правил (до обновления), не уходит: проверка прямо перед отправкой.
        let m = st.st_db_insert(ids[1]).await;
        st.db.queue_message(m, "filial@belodent.by".into(), None, None).await.unwrap().unwrap();
        assert!(set_running(&st, true).await.is_ok());
        for _ in 0..3 {
            st.mail.info().next_send = None;
            step(&st).await;
        }
        assert_eq!(mails.lock().unwrap().len(), 2, "Белодент и «Свои зубы», филиалу — нет");
        let (_, out) = call(app.clone(), "GET", "/api/mail/outbox", None).await;
        let failed: Vec<&Value> = out.as_array().unwrap().iter().filter(|m| m["status"] == "failed").collect();
        assert_eq!(failed.len(), 1, "{out}");
        assert!(failed[0]["error"].as_str().unwrap().starts_with("Не отправлено: этому сайту (belodent.by) уже писали: «Белодент»"), "{}", failed[0]["error"]);
        assert!(st.mail.info().stop_reason.is_none() && st.settings.read().await.mail_running, "пропуск по правилам — не повод останавливать рассылку");
        // «Повторить» не обходит правило.
        let (code, _) = call(app, "POST", &format!("/api/businesses/{}/mail", ids[1]), Some(json!({"message_id": m, "to": "filial@belodent.by", "subject": "Т", "body": "Т"}))).await;
        assert_eq!(code, StatusCode::BAD_REQUEST);
    }

    impl crate::jobs::AppState {
        /// Черновик письма компании — для проверок.
        async fn st_db_insert(&self, bid: i64) -> i64 {
            self.db.insert_message(bid, "email", Some("Тема".into()), "Текст".into(), json!({"source": "manual"})).await.unwrap().id
        }
    }

    type FormLog = Arc<Mutex<Vec<HashMap<String, String>>>>;

    /// Поддельный Google OAuth: код «good» → токены и адрес me@gmail.com; долгий токен «ref1» → новый короткий.
    async fn fake_oauth(log: FormLog) -> String {
        async fn token(State(log): State<FormLog>, axum::Form(f): axum::Form<HashMap<String, String>>) -> (StatusCode, Json<Value>) {
            log.lock().unwrap().push(f.clone());
            let get = |k: &str| f.get(k).map(String::as_str);
            match get("grant_type") {
                Some("authorization_code") if get("code") == Some("good") && get("client_secret") == Some("sec") && get("code_verifier").is_some_and(|v| v.len() >= 43) => {
                    let id_token = format!("h.{}.s", URL_SAFE_NO_PAD.encode(br#"{"email":"me@gmail.com"}"#));
                    (StatusCode::OK, Json(json!({"access_token": "acc1", "refresh_token": "ref1", "expires_in": 3599, "id_token": id_token})))
                }
                Some("refresh_token") if get("refresh_token") == Some("ref1") => (StatusCode::OK, Json(json!({"access_token": "acc2", "expires_in": 3599}))),
                _ => (StatusCode::BAD_REQUEST, Json(json!({"error": "invalid_grant", "error_description": "Token has been expired or revoked."}))),
            }
        }
        let app = Router::new().route("/google/token", post(token)).route("/google/revoke", post(|| async { "ok" })).with_state(log);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        base
    }

    fn xoauth2(line: &str) -> String {
        let b64 = line.rsplit(' ').next().unwrap();
        String::from_utf8(STANDARD.decode(b64).unwrap()).unwrap()
    }

    #[tokio::test]
    async fn oauth_login_and_xoauth2_send() {
        let (mails, auths) = (Arc::new(Mutex::new(Vec::new())), Arc::new(Mutex::new(Vec::new())));
        let port = fake_smtp(mails.clone(), auths.clone()).await;
        let log: FormLog = Arc::new(Mutex::new(Vec::new()));
        let base = fake_oauth(log.clone()).await;
        let st = crate::jobs::AppState::with_ai(crate::db::Db::in_memory().unwrap(), None).await.unwrap();
        st.mail.oauth.set_base(&base);
        let app = crate::api::router(st.clone());

        // Без Client ID — подсказка; с ним — страница входа с PKCE и адресом возврата на этот сервер.
        let (code, _) = call(app.clone(), "POST", "/api/mail/oauth/start", Some(json!({"provider": "google"}))).await;
        assert_eq!(code, StatusCode::BAD_REQUEST);
        let (code, v) = call(app.clone(), "POST", "/api/mail/oauth/start", Some(json!({"provider": "google", "client_id": "cid.apps.googleusercontent.com", "client_secret": "sec"}))).await;
        assert_eq!(code, StatusCode::OK, "{v}");
        let url = url::Url::parse(v["url"].as_str().unwrap()).unwrap();
        assert!(url.as_str().starts_with(&format!("{base}/google/authorize")));
        let q: HashMap<String, String> = url.query_pairs().into_owned().collect();
        assert_eq!((q["client_id"].as_str(), q["code_challenge_method"].as_str(), q["access_type"].as_str()), ("cid.apps.googleusercontent.com", "S256", "offline"));
        assert_eq!(q["redirect_uri"], "http://127.0.0.1:8787/api/mail/oauth/callback");
        assert!(q["scope"].contains("https://mail.google.com/"));
        assert!(!st.settings.read().await.public().to_string().contains("\"sec\""), "секрет приложения наружу не отдаётся");

        // Возврат со страницы входа: токены, адрес ящика и SMTP Gmail — в настройках; токены наружу не отдаются.
        let (code, html) = call_raw(app.clone(), "GET", &format!("/api/mail/oauth/callback?code=good&state={}", q["state"]), None).await;
        assert_eq!(code, StatusCode::OK);
        assert!(html.contains("Почта подключена") && html.contains("me@gmail.com"), "{html}");
        {
            let s = st.settings.read().await;
            assert!(s.oauth() && s.oauth_connected() && s.mail_configured());
            assert_eq!((s.smtp_host.as_str(), s.smtp_port, s.smtp_user.as_str(), s.mail_from()), ("smtp.gmail.com", 465, "me@gmail.com", "me@gmail.com".to_string()));
            let p = s.public().to_string();
            assert!(!p.contains("ref1") && !p.contains("acc1"), "{p}");
        }
        let (_, stv) = call(app.clone(), "GET", "/api/mail/oauth/status", None).await;
        assert_eq!((stv["connected"].as_bool(), stv["email"].as_str(), stv["last_ok"].as_str()), (Some(true), Some("me@gmail.com"), Some("me@gmail.com")));
        // Та же ссылка второй раз — не принимается.
        let (_, html) = call_raw(app.clone(), "GET", &format!("/api/mail/oauth/callback?code=good&state={}", q["state"]), None).await;
        assert!(html.contains("устарела"), "{html}");

        // Письмо через поддельный SMTP — вход XOAUTH2 с коротким токеном.
        call(app.clone(), "PUT", "/api/settings", Some(json!({"smtp_host": "127.0.0.1", "smtp_port": port}))).await;
        call(app.clone(), "PUT", "/api/settings", Some(json!({"smtp_security": "plain"}))).await;
        let (code, v) = call(app.clone(), "POST", "/api/mail/test", Some(json!({}))).await;
        assert_eq!(code, StatusCode::OK, "{v}");
        let last = auths.lock().unwrap().last().cloned().unwrap();
        assert!(last.starts_with("AUTH XOAUTH2"), "{last}");
        assert_eq!(xoauth2(&last), "user=me@gmail.com\x01auth=Bearer acc1\x01\x01");
        assert!(mails.lock().unwrap()[0].contains("From: =?utf-8?") || mails.lock().unwrap()[0].contains("me@gmail.com"));

        // Короткий токен истёк — обновляется по долгому сам.
        let mut s = st.settings.read().await.clone();
        s.oauth_expires_at = 0;
        st.save_settings(s).await.unwrap();
        let (code, v) = call(app.clone(), "POST", "/api/mail/test", Some(json!({}))).await;
        assert_eq!(code, StatusCode::OK, "{v}");
        assert_eq!(xoauth2(&auths.lock().unwrap().last().cloned().unwrap()), "user=me@gmail.com\x01auth=Bearer acc2\x01\x01");
        assert_eq!(log.lock().unwrap().last().unwrap()["grant_type"], "refresh_token");

        // Вход отозван — понятная причина (и подсказка про режим «Testing» у Google).
        let mut s = st.settings.read().await.clone();
        s.oauth_refresh_token = "revoked".into();
        s.oauth_expires_at = 0;
        st.save_settings(s).await.unwrap();
        let (code, text) = call_raw(app.clone(), "POST", "/api/mail/test", Some(json!({}))).await;
        assert_eq!(code, StatusCode::BAD_GATEWAY);
        assert!(text.contains("7 дней") && text.contains("invalid_grant"), "{text}");

        // Выйти — токены стёрты, почта больше не настроена.
        let (code, _) = call(app.clone(), "POST", "/api/mail/oauth/disconnect", None).await;
        assert_eq!(code, StatusCode::OK);
        let s = st.settings.read().await;
        assert!(!s.oauth_connected() && !s.mail_configured() && s.oauth_refresh_token.is_empty());
    }
}
