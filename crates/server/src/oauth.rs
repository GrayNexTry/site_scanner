//! Вход в почту через аккаунт (OAuth 2.0) — Google, Яндекс, Microsoft — вместо пароля приложения.
//! Письма уходят по SMTP с механизмом XOAUTH2: программа хранит только токены входа, пароль ящика ей не нужен,
//! а доступ можно отозвать в аккаунте в любой момент.
//!
//! Приложение OAuth регистрирует сам владелец ящика (бесплатно, один раз): Google не выдаёт программе с открытым
//! кодом общий ключ к чужой почте без платной проверки безопасности, а Яндекс и Microsoft привязывают адрес возврата
//! к приложению. Client ID (и Client secret) вписываются в настройках почты, пошаговая инструкция — там же.
//! Mail.ru вход через аккаунт для сторонних программ выдаёт только партнёрам — для него остаётся пароль приложения.
//!
//! Поток: «Войти через …» → `POST /api/mail/oauth/start` (страница входа с PKCE и `state`) → вход в браузере →
//! сервис возвращает на `/api/mail/oauth/callback` → обмен кода на токены → адрес ящика, SMTP-сервер и токены
//! в настройках. Перед каждой отправкой короткий токен при необходимости обновляется по долгому (`smtp_token`).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::Html;
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD};
use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};
use tracing::{info, warn};

use crate::api::{bad, ApiResult};
use crate::jobs::Shared;
use crate::settings::Settings;

/// Почтовый сервис со входом через аккаунт.
pub struct Provider {
    pub id: &'static str,
    pub name: &'static str,
    auth_url: &'static str,
    token_url: &'static str,
    revoke_url: Option<&'static str>,
    /// Права; None — все, что отмечены в приложении (Яндекс).
    scope: Option<&'static str>,
    extra: &'static [(&'static str, &'static str)],
    smtp_host: &'static str,
    smtp_port: u16,
    smtp_security: &'static str,
    /// Нужен ли Client secret: у Microsoft для настольных программ его нет.
    needs_secret: bool,
}

pub const PROVIDERS: &[Provider] = &[
    Provider {
        id: "google",
        name: "Google",
        auth_url: "https://accounts.google.com/o/oauth2/v2/auth",
        token_url: "https://oauth2.googleapis.com/token",
        revoke_url: Some("https://oauth2.googleapis.com/revoke"),
        // https://mail.google.com/ — единственное право Gmail для SMTP (и IMAP — для будущего чтения ответов).
        scope: Some("https://mail.google.com/ openid email"),
        // offline + consent — чтобы Google всегда выдал долгий токен.
        extra: &[("access_type", "offline"), ("prompt", "consent")],
        smtp_host: "smtp.gmail.com",
        smtp_port: 465,
        smtp_security: "ssl",
        needs_secret: true,
    },
    Provider {
        id: "yandex",
        name: "Яндекс",
        auth_url: "https://oauth.yandex.ru/authorize",
        token_url: "https://oauth.yandex.ru/token",
        revoke_url: Some("https://oauth.yandex.ru/revoke_token"),
        scope: None,
        extra: &[("force_confirm", "yes")],
        smtp_host: "smtp.yandex.ru",
        smtp_port: 465,
        smtp_security: "ssl",
        needs_secret: true,
    },
    Provider {
        id: "microsoft",
        name: "Microsoft (Outlook)",
        auth_url: "https://login.microsoftonline.com/common/oauth2/v2.0/authorize",
        token_url: "https://login.microsoftonline.com/common/oauth2/v2.0/token",
        revoke_url: None,
        scope: Some("offline_access https://outlook.office.com/SMTP.Send openid email"),
        extra: &[("prompt", "select_account"), ("response_mode", "query")],
        smtp_host: "smtp-mail.outlook.com",
        smtp_port: 587,
        smtp_security: "starttls",
        needs_secret: false,
    },
];

pub fn provider(id: &str) -> Option<&'static Provider> {
    PROVIDERS.iter().find(|p| p.id == id)
}

/// Сколько ждать возврата со страницы входа.
const PENDING_TTL: Duration = Duration::from_secs(15 * 60);

struct Pending {
    provider: &'static str,
    verifier: String,
    redirect: String,
    created: Instant,
}

#[derive(Default)]
pub struct OAuthState {
    pending: std::sync::Mutex<HashMap<String, Pending>>,
    /// Итог последнего входа для интерфейса: Ok(адрес) или Err(причина).
    last: std::sync::Mutex<Option<Result<String, String>>>,
    /// Адрес поддельного сервиса для проверок вместо Google/Яндекса/Microsoft (`{base}/{provider}/token`…).
    base: std::sync::Mutex<Option<String>>,
    /// Одно обновление токена за раз (рассылка и пробное письмо).
    refresh: tokio::sync::Mutex<()>,
}

impl OAuthState {
    #[cfg(test)]
    pub fn set_base(&self, base: &str) {
        *self.base.lock().unwrap_or_else(|e| e.into_inner()) = Some(base.trim_end_matches('/').to_string());
    }

    fn base(&self) -> Option<String> {
        self.base.lock().unwrap_or_else(|e| e.into_inner()).clone().or_else(|| std::env::var("MAPS_SCANNER_OAUTH_URL").ok().map(|b| b.trim_end_matches('/').to_string()))
    }

    /// Адрес сервиса: настоящий или поддельный для проверок.
    fn url(&self, p: &Provider, kind: &str, real: &str) -> String {
        match self.base() {
            Some(b) => format!("{b}/{}/{kind}", p.id),
            None => real.to_string(),
        }
    }

    fn set_last(&self, r: Option<Result<String, String>>) {
        *self.last.lock().unwrap_or_else(|e| e.into_inner()) = r;
    }
}

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/mail/oauth/status", get(status))
        .route("/mail/oauth/start", post(start))
        .route("/mail/oauth/callback", get(callback))
        .route("/mail/oauth/disconnect", post(disconnect))
}

fn now_unix() -> i64 {
    chrono::Utc::now().timestamp()
}

fn random_token(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    // Системный генератор случайных чисел; без него вход небезопасен, и такой системы почти не бывает.
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut buf).expect("системный генератор случайных чисел");
    URL_SAFE_NO_PAD.encode(buf)
}

/// PKCE: проверочная строка и её хэш для страницы входа.
fn pkce() -> (String, String) {
    let verifier = random_token(48);
    let challenge = URL_SAFE_NO_PAD.encode(ring::digest::digest(&ring::digest::SHA256, verifier.as_bytes()));
    (verifier, challenge)
}

/// Адрес возврата — на этот же сервер, только через loopback (Google принимает от настольных программ только его).
fn redirect_uri(headers: &HeaderMap) -> Result<String, String> {
    let host = headers.get("host").and_then(|h| h.to_str().ok()).unwrap_or("127.0.0.1:8787").trim().to_string();
    let name = host.rsplit_once(':').map(|(h, _)| h).unwrap_or(&host);
    if !matches!(name, "127.0.0.1" | "localhost" | "[::1]") {
        return Err(format!(
            "Вход через аккаунт работает, только когда сканер открыт на этом компьютере (http://127.0.0.1:…), а сейчас — {host}. Откройте сканер по адресу 127.0.0.1."
        ));
    }
    Ok(format!("http://{host}/api/mail/oauth/callback"))
}

fn client(s: &Settings, p: &Provider) -> (String, String) {
    match p.id {
        "google" => (s.oauth_google_id.clone(), s.oauth_google_secret.clone()),
        "yandex" => (s.oauth_yandex_id.clone(), s.oauth_yandex_secret.clone()),
        _ => (s.oauth_microsoft_id.clone(), String::new()),
    }
}

fn http() -> reqwest::Client {
    reqwest::Client::builder().timeout(Duration::from_secs(30)).user_agent(concat!("maps-seo-scanner/", env!("CARGO_PKG_VERSION"))).build().unwrap_or_default()
}

// ---------------------------------------------------------------- состояние

async fn status(State(st): State<Shared>, headers: HeaderMap) -> Json<Value> {
    let s = st.settings.read().await.clone();
    let last = st.mail.oauth.last.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let providers: Vec<Value> = PROVIDERS
        .iter()
        .map(|p| {
            let (id, secret) = client(&s, p);
            json!({"id": p.id, "name": p.name, "needs_secret": p.needs_secret, "client_id": id, "secret_set": !secret.is_empty()})
        })
        .collect();
    let redirect = redirect_uri(&headers);
    Json(json!({
        "connected": s.oauth_connected(),
        "provider": s.oauth_provider,
        "email": s.oauth_email,
        "redirect_uri": redirect.as_ref().ok(),
        "redirect_error": redirect.as_ref().err(),
        "last_ok": last.as_ref().and_then(|r| r.as_ref().ok()),
        "last_error": last.as_ref().and_then(|r| r.as_ref().err()),
        "providers": providers,
    }))
}

#[derive(Deserialize)]
struct StartReq {
    provider: String,
    /// Client ID и секрет из формы — сохраняются до входа, чтобы не нажимать «Сохранить» отдельно.
    client_id: Option<String>,
    client_secret: Option<String>,
}

async fn start(State(st): State<Shared>, headers: HeaderMap, Json(r): Json<StartReq>) -> ApiResult<Json<Value>> {
    let p = provider(&r.provider).ok_or_else(|| bad("Неизвестный почтовый сервис."))?;
    let redirect = redirect_uri(&headers).map_err(bad)?;
    let mut s = st.settings.read().await.clone();
    let mut patch = serde_json::Map::new();
    if let Some(id) = r.client_id.map(|x| x.trim().to_string()).filter(|x| !x.is_empty()) {
        patch.insert(format!("oauth_{}_id", p.id), json!(id));
    }
    if let Some(sec) = r.client_secret.map(|x| x.trim().to_string()).filter(|x| !x.is_empty() && p.needs_secret) {
        patch.insert(format!("oauth_{}_secret", p.id), json!(sec));
    }
    if !patch.is_empty() {
        s.apply(&Value::Object(patch));
        st.save_settings(s.clone()).await?;
    }
    let (client_id, secret) = client(&s, p);
    if client_id.is_empty() {
        return Err(bad(format!("Впишите Client ID приложения {} — как его получить, написано под полем.", p.name)));
    }
    if p.needs_secret && secret.is_empty() {
        return Err(bad(format!("Впишите Client secret приложения {}.", p.name)));
    }
    let (verifier, challenge) = pkce();
    let state = random_token(24);
    let mut params: Vec<(&str, String)> = vec![
        ("client_id", client_id),
        ("redirect_uri", redirect.clone()),
        ("response_type", "code".into()),
        ("state", state.clone()),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256".into()),
    ];
    if let Some(scope) = p.scope {
        params.push(("scope", scope.into()));
    }
    params.extend(p.extra.iter().map(|(k, v)| (*k, v.to_string())));
    let login = s.smtp_user.trim();
    if p.id == "google" && login.contains('@') {
        params.push(("login_hint", login.to_string()));
    }
    let url = url::Url::parse_with_params(&st.mail.oauth.url(p, "authorize", p.auth_url), &params).map_err(|e| bad(e.to_string()))?;
    {
        let mut pending = st.mail.oauth.pending.lock().unwrap_or_else(|e| e.into_inner());
        pending.retain(|_, x| x.created.elapsed() < PENDING_TTL);
        pending.insert(state, Pending { provider: p.id, verifier, redirect: redirect.clone(), created: Instant::now() });
    }
    st.mail.oauth.set_last(None);
    Ok(Json(json!({"url": url.to_string(), "redirect_uri": redirect})))
}

// ---------------------------------------------------------------- возврат со страницы входа

#[derive(Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn page(ok: bool, text: &str) -> Html<String> {
    let title = if ok { "Почта подключена" } else { "Не удалось войти" };
    let close = if ok { "<script>setTimeout(() => window.close(), 2500)</script>" } else { "" };
    Html(format!(
        "<!doctype html><html lang=\"ru\"><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\"><title>{title}</title>\
         <style>body{{font:16px/1.5 system-ui,sans-serif;max-width:560px;margin:15vh auto;padding:0 20px;color:#222;background:#fff}}\
         @media(prefers-color-scheme:dark){{body{{color:#eee;background:#161616}}}}h1{{font-size:22px}}</style>\
         <h1>{title}</h1><p>{}</p><p style=\"opacity:.7\">Эту вкладку можно закрыть и вернуться в сканер.</p>{close}</html>",
        escape(text)
    ))
}

/// Токены из ответа сервиса: (короткий, долгий, срок в секундах, id_token).
fn tokens(v: &Value) -> (Option<String>, Option<String>, i64, Option<String>) {
    let s = |k: &str| v[k].as_str().map(str::to_string).filter(|x| !x.is_empty());
    let expires = v["expires_in"].as_i64().or_else(|| v["expires_in"].as_str().and_then(|x| x.parse().ok())).unwrap_or(3600);
    (s("access_token"), s("refresh_token"), expires, s("id_token"))
}

/// Ошибка сервиса словами: что случилось и что делать.
fn explain(p: &Provider, v: &Value, http: reqwest::StatusCode) -> String {
    let code = v["error"].as_str().unwrap_or("");
    let desc = v["error_description"].as_str().unwrap_or("");
    let hint = match code {
        "invalid_client" | "unauthorized_client" => format!("{} не узнал приложение — проверьте Client ID и Client secret.", p.name),
        "invalid_grant" if p.id == "google" => {
            "Вход больше не действует: доступ отозван или истёк. Если приложение в Google Cloud в режиме «Testing», Google держит вход только 7 дней — переведите его в «In production» (Audience → Publish app) и войдите заново.".into()
        }
        "invalid_grant" => format!("Вход через {} больше не действует (доступ отозван или истёк) — войдите заново.", p.name),
        "redirect_uri_mismatch" => "Адрес возврата не совпадает с указанным в приложении — скопируйте его из настроек почты в настройки приложения.".into(),
        _ => format!("{} ответил ошибкой {}.", p.name, http.as_u16()),
    };
    let detail = [code, desc].iter().filter(|x| !x.is_empty()).cloned().collect::<Vec<_>>().join(": ");
    if detail.is_empty() { hint } else { format!("{hint} ({detail})") }
}

async fn post_form(url: &str, form: &[(&str, String)]) -> Result<(reqwest::StatusCode, Value), String> {
    let resp = http().post(url).form(form).send().await.map_err(|e| format!("нет связи: {e}"))?;
    let code = resp.status();
    let v: Value = resp.json().await.unwrap_or(Value::Null);
    Ok((code, v))
}

/// Адрес из id_token (JWT): подпись не проверяем — токен получен напрямую от сервиса по TLS.
fn email_from_id_token(t: &str) -> Option<String> {
    let payload = t.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).or_else(|_| URL_SAFE.decode(payload)).ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    ["email", "preferred_username", "upn"].iter().find_map(|k| v[*k].as_str().filter(|e| e.contains('@')).map(str::to_string))
}

/// Адрес ящика Яндекса: из Яндекс ID (право «доступ к адресу почты»), иначе логин@yandex.ru.
async fn yandex_email(st: &Shared, p: &Provider, token: &str) -> Option<String> {
    let url = st.mail.oauth.url(p, "info", "https://login.yandex.ru/info?format=json");
    let v: Value = http().get(url).header("Authorization", format!("OAuth {token}")).send().await.ok()?.json().await.ok()?;
    v["default_email"]
        .as_str()
        .map(str::to_string)
        .or_else(|| v["emails"][0].as_str().map(str::to_string))
        .or_else(|| v["login"].as_str().filter(|l| !l.is_empty()).map(|l| format!("{l}@yandex.ru")))
}

/// SMTP-сервер Microsoft: личные ящики (outlook.com, hotmail…) — smtp-mail.outlook.com, рабочие — smtp.office365.com.
fn smtp_host_for(p: &Provider, email: &str) -> &'static str {
    if p.id != "microsoft" {
        return p.smtp_host;
    }
    let domain = email.rsplit('@').next().unwrap_or("").to_lowercase();
    let personal = ["outlook.", "hotmail.", "live.", "msn."].iter().any(|x| domain.starts_with(x));
    if personal { p.smtp_host } else { "smtp.office365.com" }
}

async fn callback(State(st): State<Shared>, Query(q): Query<CallbackQuery>) -> Html<String> {
    match finish_login(&st, q).await {
        Ok(email) => {
            info!("почта: вход через аккаунт выполнен ({email})");
            st.mail.oauth.set_last(Some(Ok(email.clone())));
            st.mail.wake();
            page(true, &format!("Письма будут уходить с {email}. В настройках почты можно отправить пробное письмо."))
        }
        Err(e) => {
            warn!("почта: вход через аккаунт не удался: {e}");
            st.mail.oauth.set_last(Some(Err(e.clone())));
            page(false, &e)
        }
    }
}

async fn finish_login(st: &Shared, q: CallbackQuery) -> Result<String, String> {
    let pending = q.state.as_deref().and_then(|s| st.mail.oauth.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(s));
    if let Some(err) = q.error {
        let why = if err == "access_denied" { "вход отменён или доступ не разрешён".to_string() } else { err };
        let desc = q.error_description.map(|d| format!(" ({d})")).unwrap_or_default();
        return Err(format!("Сервис не дал доступ: {why}{desc}. Нажмите «Войти» в настройках почты ещё раз."));
    }
    let pending = pending.filter(|x| x.created.elapsed() < PENDING_TTL).ok_or("Ссылка входа устарела или уже использована — нажмите «Войти» в настройках почты ещё раз.")?;
    let code = q.code.filter(|c| !c.is_empty()).ok_or("Сервис не вернул код входа — попробуйте ещё раз.")?;
    let p = provider(pending.provider).ok_or("неизвестный сервис")?;
    let s = st.settings.read().await.clone();
    let (client_id, secret) = client(&s, p);
    let mut form = vec![
        ("grant_type", "authorization_code".to_string()),
        ("code", code),
        ("redirect_uri", pending.redirect.clone()),
        ("client_id", client_id),
        ("code_verifier", pending.verifier.clone()),
    ];
    if !secret.is_empty() {
        form.push(("client_secret", secret));
    }
    if let Some(scope) = p.scope.filter(|_| p.id == "microsoft") {
        form.push(("scope", scope.into()));
    }
    let (http_code, v) = post_form(&st.mail.oauth.url(p, "token", p.token_url), &form).await.map_err(|e| format!("Не удалось получить токен {}: {e}", p.name))?;
    let (access, refresh, expires, id_token) = tokens(&v);
    let (Some(access), true) = (access, http_code.is_success()) else {
        return Err(explain(p, &v, http_code));
    };
    let refresh = refresh.ok_or_else(|| {
        format!("{} не выдал долгий токен входа. Уберите доступ приложения в настройках аккаунта (для Google — myaccount.google.com/permissions) и войдите снова.", p.name)
    })?;
    let email = match p.id {
        "yandex" => yandex_email(st, p, &access).await,
        _ => id_token.as_deref().and_then(email_from_id_token),
    }
    .or_else(|| Some(s.smtp_user.trim().to_string()).filter(|u| u.contains('@')))
    .ok_or("Не удалось узнать адрес ящика — впишите его в поле «Логин» в настройках почты и войдите снова.")?;
    let mut s = st.settings.read().await.clone();
    s.smtp_auth = "oauth".into();
    s.oauth_provider = p.id.into();
    s.oauth_email = email.clone();
    s.oauth_refresh_token = refresh;
    s.oauth_access_token = access;
    s.oauth_expires_at = now_unix() + expires - 60;
    s.smtp_host = smtp_host_for(p, &email).into();
    s.smtp_port = p.smtp_port;
    s.smtp_security = p.smtp_security.into();
    s.smtp_user = email.clone();
    // Адрес отправителя чужого ящика почта не примет (или письмо уйдёт в спам) — берём адрес этого входа.
    if !s.mail_from_email.trim().is_empty() && !s.mail_from_email.trim().eq_ignore_ascii_case(&email) {
        s.mail_from_email = String::new();
    }
    st.save_settings(s).await.map_err(|e| format!("Не сохранить вход: {e:#}"))?;
    Ok(email)
}

// ---------------------------------------------------------------- токен для отправки

/// Логин и короткий токен для SMTP XOAUTH2; при необходимости обновляет токен по долгому. Ошибка — причина словами.
pub async fn smtp_token(st: &Shared) -> Result<(String, String), String> {
    let _one = st.mail.oauth.refresh.lock().await;
    let s = st.settings.read().await.clone();
    let p = provider(&s.oauth_provider).filter(|_| s.oauth_connected()).ok_or("Вход через аккаунт не выполнен: Настройки → Почта для рассылки → «Войти через …».")?;
    let user = Some(s.smtp_user.trim().to_string()).filter(|u| !u.is_empty()).unwrap_or_else(|| s.oauth_email.clone());
    if !s.oauth_access_token.is_empty() && s.oauth_expires_at > now_unix() + 120 {
        return Ok((user, s.oauth_access_token.clone()));
    }
    let (client_id, secret) = client(&s, p);
    let mut form = vec![("grant_type", "refresh_token".to_string()), ("refresh_token", s.oauth_refresh_token.clone()), ("client_id", client_id)];
    if !secret.is_empty() {
        form.push(("client_secret", secret));
    }
    if let Some(scope) = p.scope.filter(|_| p.id == "microsoft") {
        form.push(("scope", scope.into()));
    }
    let (code, v) = post_form(&st.mail.oauth.url(p, "token", p.token_url), &form).await.map_err(|e| format!("Не удалось обновить вход через {}: {e}", p.name))?;
    let (access, refresh, expires, _) = tokens(&v);
    let (Some(access), true) = (access, code.is_success()) else {
        return Err(format!("Вход в почту через {}: {}", p.name, explain(p, &v, code)));
    };
    let mut s = st.settings.read().await.clone();
    s.oauth_access_token = access.clone();
    s.oauth_expires_at = now_unix() + expires - 60;
    // Microsoft и Яндекс выдают новый долгий токен при обновлении — старый перестаёт действовать.
    if let Some(r) = refresh {
        s.oauth_refresh_token = r;
    }
    st.save_settings(s).await.map_err(|e| format!("Не сохранить обновлённый вход: {e:#}"))?;
    Ok((user, access))
}

// ---------------------------------------------------------------- выход

async fn disconnect(State(st): State<Shared>) -> ApiResult<Json<Value>> {
    let mut s = st.settings.read().await.clone();
    if let Some(p) = provider(&s.oauth_provider)
        && let Some(url) = p.revoke_url
    {
        // Отозвать доступ у сервиса — по возможности; не вышло — токены всё равно стираются здесь.
        let (client_id, secret) = client(&s, p);
        let form: Vec<(&str, String)> = match p.id {
            "yandex" => vec![("access_token", s.oauth_access_token.clone()), ("client_id", client_id), ("client_secret", secret)],
            _ => vec![("token", s.oauth_refresh_token.clone())],
        };
        if let Err(e) = post_form(&st.mail.oauth.url(p, "revoke", url), &form).await {
            warn!("почта: отозвать вход у {}: {e}", p.name);
        }
    }
    s.oauth_provider = String::new();
    s.oauth_email = String::new();
    s.oauth_refresh_token = String::new();
    s.oauth_access_token = String::new();
    s.oauth_expires_at = 0;
    st.save_settings(s).await?;
    st.mail.oauth.set_last(None);
    Ok(Json(json!({"ok": true})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_and_id_token() {
        let (v, c) = pkce();
        assert!(v.len() >= 43 && v.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'));
        assert_eq!(c, URL_SAFE_NO_PAD.encode(ring::digest::digest(&ring::digest::SHA256, v.as_bytes())));
        assert_ne!(pkce().0, v, "каждый вход — своя строка");
        let payload = URL_SAFE_NO_PAD.encode(br#"{"email":"me@gmail.com","sub":"1"}"#);
        assert_eq!(email_from_id_token(&format!("x.{payload}.sig")).as_deref(), Some("me@gmail.com"));
        let ms = URL_SAFE_NO_PAD.encode(br#"{"preferred_username":"me@outlook.com"}"#);
        assert_eq!(email_from_id_token(&format!("x.{ms}.sig")).as_deref(), Some("me@outlook.com"));
        assert_eq!(email_from_id_token("мусор"), None);
        let p = provider("microsoft").unwrap();
        assert_eq!(smtp_host_for(p, "me@outlook.com"), "smtp-mail.outlook.com");
        assert_eq!(smtp_host_for(p, "me@company.com"), "smtp.office365.com");
        assert_eq!(smtp_host_for(provider("google").unwrap(), "me@gmail.com"), "smtp.gmail.com");
    }

    #[test]
    fn redirect_only_loopback() {
        let h = |host: &str| {
            let mut m = HeaderMap::new();
            m.insert("host", host.parse().unwrap());
            redirect_uri(&m)
        };
        assert_eq!(h("127.0.0.1:8787").unwrap(), "http://127.0.0.1:8787/api/mail/oauth/callback");
        assert_eq!(h("localhost:18789").unwrap(), "http://localhost:18789/api/mail/oauth/callback");
        assert!(h("192.168.1.5:8787").is_err());
    }
}
