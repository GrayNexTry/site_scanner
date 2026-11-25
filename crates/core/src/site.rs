//! Сбор данных о сайте: всё, что видит посетитель и поисковый бот, без активного вмешательства.
//! Порядок: главная → параллельные пробы (robots, sitemap, http/www, 404, боты, TLS, домен, PSI)
//! → ограниченный обход внутренних страниц → проверка ссылок и картинок.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::domain_info::{self, DnsInfo, DomainInfo};
use crate::http::{Fetched, Fetcher, UrlStatus, browser_ua};
use crate::page::{self, PageInfo};
use crate::psi::{self, PsiResult};
use crate::robots::{self, RobotsDirectives};
use crate::sitemap::{self, SitemapKind};
use crate::text::{host_of, parse_url_lenient, registrable_domain};
use crate::tls::{self, TlsInfo};

/// Агент нашего обходчика в robots.txt (подпадает под `*`).
pub const CRAWLER_AGENT: &str = "MapsSeoScanner";

/// Внутренним страницам — меньше терпения, чем главной.
const INNER_PAGE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// ИИ-боты, которыми проверяем WAF/CDN (I05): токен и строка User-Agent.
pub const BOT_PROBES: &[(&str, &str)] = &[
    ("OAI-SearchBot", "Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko); compatible; OAI-SearchBot/1.0; +https://openai.com/searchbot"),
    ("PerplexityBot", "Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko; compatible; PerplexityBot/1.0; +https://perplexity.ai/perplexitybot)"),
    ("Claude-SearchBot", "Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko; compatible; Claude-SearchBot/1.0; +https://www.anthropic.com)"),
    // Агент Google, который открывает сайты и действует по просьбе пользователя (Gemini, AI Mode).
    (
        "Google-Agent",
        "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko; compatible; Google-Agent; +https://developers.google.com/crawling/docs/crawlers-fetchers/google-agent) Chrome/142.0.0.0 Safari/537.36",
    ),
];

#[derive(Debug, Clone)]
pub struct CollectOptions {
    pub max_pages: usize,
    pub link_checks: usize,
    pub sitemap_sample: usize,
    pub image_checks: usize,
    pub concurrency: usize,
    pub psi_key: Option<String>,
    pub safe_browsing_key: Option<String>,
    /// Браузер для рендера сайтов, которые рисуются скриптами (None — без рендера).
    pub browser: Option<std::path::PathBuf>,
    /// Скан идёт с IP другой страны, чем район (VPN): таймаут — ещё не смерть сайта.
    pub abroad: bool,
    /// Спрашивать открытые данные Яндекса (ИКС, знаки, Safe Browsing): страна скана — рынок Яндекса.
    /// Для сайтов в зонах .ru/.рф/.by/.kz/.uz спрашивается всегда.
    pub yandex: bool,
}

impl Default for CollectOptions {
    fn default() -> Self {
        Self {
            max_pages: 15,
            link_checks: 30,
            sitemap_sample: 20,
            image_checks: 10,
            concurrency: 6,
            psi_key: None,
            safe_browsing_key: None,
            browser: None,
            abroad: false,
            yandex: false,
        }
    }
}

/// В исходном HTML почти нет текста, его рисуют скрипты (React, Vue, Angular…).
pub fn is_js_shell(p: &PageInfo) -> bool {
    p.words < 50 && (spa_markers(p) || p.scripts.iter().any(|s| s.is_module) || p.scripts.len() > 3)
}

/// Приложение на React/Vue/Angular/Next: title и H1 оно может ставить скриптом.
fn spa_markers(p: &PageInfo) -> bool {
    const MARKERS: &[&str] = &["id=\"root\"", "id=\"app\"", "id=\"__nuxt\"", "id=\"__next\"", "<app-root", "ng-version", "data-reactroot", "id=\"___gatsby\""];
    MARKERS.iter().any(|m| p.html_lower.contains(m))
}

/// Рендерить ли главную: текст рисуют скрипты, страница почти пустая, нет title, у приложения
/// нет H1 или почти нет ссылок в исходном HTML (меню и каталог рисует JS).
fn needs_render(p: &PageInfo) -> bool {
    is_js_shell(p)
        || p.words < 5
        || p.title.is_none()
        || (spa_markers(p) && (p.h1s().is_empty() || p.links.iter().filter(|l| l.internal).count() < 5))
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RedirectProbe {
    pub url: String,
    pub first_status: Option<u16>,
    pub final_url: Option<String>,
    pub final_status: Option<u16>,
    pub hops: usize,
    pub error: Option<String>,
    /// Конечный ответ прислал Strict-Transport-Security: браузер, открывший его, дальше сам ходит по https.
    #[serde(default)]
    pub hsts: bool,
}

impl RedirectProbe {
    fn from(url: &str, r: Result<Fetched, crate::http::FetchError>) -> Self {
        match r {
            Ok(f) => Self {
                url: url.to_string(),
                first_status: Some(f.chain.first().map(|h| h.status).unwrap_or(f.status)),
                final_url: Some(f.final_url.clone()),
                final_status: Some(f.status),
                hops: f.chain.len(),
                error: None,
                hsts: f.headers.iter().any(|(k, v)| k.eq_ignore_ascii_case("strict-transport-security") && !v.replace(' ', "").contains("max-age=0")),
            },
            Err(e) => Self { url: url.to_string(), error: Some(e.to_string()), ..Default::default() },
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BotProbe {
    pub agent: String,
    pub status: Option<u16>,
    pub bytes: usize,
    /// Похоже на страницу-заглушку WAF (капча, «Just a moment…»).
    pub challenge: bool,
    pub error: Option<String>,
}

/// Ответ главной для отдельного клиента (робот Яндекса): что он увидел.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BotView {
    pub status: Option<u16>,
    pub final_url: Option<String>,
    pub title: Option<String>,
    pub words: usize,
    /// Заглушка защиты от ботов (капча, «Just a moment…»).
    pub challenge: bool,
    pub error: Option<String>,
}

/// User-Agent основного робота Яндекса.
pub const YANDEX_BOT_UA: &str = "Mozilla/5.0 (compatible; YandexBot/3.0; +http://yandex.com/bots)";

/// User-Agent мобильного Chrome той же версии, что и настольный.
fn mobile_ua() -> String {
    browser_ua().replace("(Windows NT 10.0; Win64; x64)", "(Linux; Android 14; Pixel 8)").replace(" Safari/", " Mobile Safari/")
}

async fn bot_view(f: &Fetcher, url: &str, ua: &str) -> BotView {
    match f.get_limited(url, Some(ua), 1024 * 1024).await {
        Ok(r) => {
            let challenge = looks_like_challenge(&r);
            let page = if r.is_html() { parse_page(r.body.clone(), r.final_url.clone()).await } else { None };
            BotView {
                status: Some(r.status),
                final_url: Some(r.final_url.clone()),
                title: page.as_ref().and_then(|p| p.title.clone()),
                words: page.map(|p| p.words).unwrap_or(0),
                challenge,
                error: None,
            }
        }
        Err(e) => BotView { error: Some(e.to_string()), ..Default::default() },
    }
}

/// Зона домена, где Яндекс — основной поиск: открытые данные Яндекса нужны всегда.
fn yandex_zone(host: &str) -> bool {
    matches!(crate::region::zone_of(host), crate::region::Zone::Country(c) if crate::region::yandex_market(Some(&c)))
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TextResource {
    pub url: String,
    pub status: Option<u16>,
    #[serde(skip)]
    pub body: String,
    pub bytes: usize,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SitemapInfo {
    /// Sitemap, указанные в robots.txt.
    pub declared: Vec<String>,
    /// Загруженные файлы: (url, статус, тип).
    pub fetched: Vec<(String, Option<u16>, Option<SitemapKind>)>,
    pub urls: Vec<String>,
    pub lastmods: Vec<String>,
    pub too_big: bool,
    /// Индекс прочитан не целиком (дочерних файлов больше, чем мы открываем).
    #[serde(default)]
    pub partial: bool,
    /// Индекс сделан как urlset со ссылками на другие sitemap — Google такое не читает как индекс.
    #[serde(default)]
    pub nested_in_urlset: bool,
    /// Sitemap других доменов, указанные в robots.txt.
    #[serde(default)]
    pub foreign: Vec<String>,
}

impl SitemapInfo {
    pub fn found(&self) -> bool {
        self.fetched.iter().any(|(_, st, k)| *st == Some(200) && matches!(k, Some(SitemapKind::UrlSet | SitemapKind::Index)))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrawledPage {
    pub url: String,
    pub final_url: String,
    pub depth: u8,
    pub status: Option<u16>,
    pub redirected: bool,
    pub error: Option<String>,
    /// Размер HTML без сжатия, байт (I21).
    #[serde(default)]
    pub bytes: usize,
    #[serde(skip)]
    pub info: Option<PageInfo>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AssetCache {
    pub url: String,
    pub cache_control: Option<String>,
    pub max_age: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct SiteSnapshot {
    pub input_url: String,
    pub home: Option<Fetched>,
    pub home_error: Option<String>,
    pub page: Option<PageInfo>,
    pub http_probe: Option<RedirectProbe>,
    pub mirror_probe: Option<RedirectProbe>,
    /// Сайт открылся по http:// — ответ того же хоста по https:// (есть ли HTTPS вообще).
    pub https_probe: Option<RedirectProbe>,
    pub compression: Option<String>,
    pub robots: Option<TextResource>,
    pub robots_directives: Option<RobotsDirectives>,
    pub sitemap: SitemapInfo,
    pub sitemap_checks: Vec<UrlStatus>,
    pub not_found: Option<RedirectProbe>,
    pub bots: Vec<BotProbe>,
    pub pages: Vec<CrawledPage>,
    pub link_checks: Vec<UrlStatus>,
    pub image_checks: Vec<UrlStatus>,
    pub assets: Vec<AssetCache>,
    pub favicon_status: Option<u16>,
    pub llms_txt: Option<u16>,
    pub tls: Option<TlsInfo>,
    pub domain: Option<DomainInfo>,
    pub dns: Option<DnsInfo>,
    pub psi: Option<PsiResult>,
    pub safe_browsing: Option<Result<Vec<String>, String>>,
    /// Safe Browsing Яндекса (B13): найденные угрозы или причина, почему не проверено.
    pub yandex_safety: Option<Result<Vec<String>, String>>,
    /// ИКС и знаки сайта из webmaster.yandex.ru/siteinfo (C19, R16).
    pub yandex: Option<crate::yandex_info::SiteInfo>,
    /// Главная, запрошенная с User-Agent робота Яндекса (R14, клоакинг).
    pub yandex_bot: Option<BotView>,
    /// Главная, запрошенная мобильным браузером (R13: редирект мобильных на чужой сайт).
    pub mobile_probe: Option<RedirectProbe>,
    /// Сайт перестал отвечать на серию запросов (защита от частых запросов) — обход прерван.
    pub throttled: bool,
    /// Ссылка из карточки вела на внутреннюю страницу с ошибкой (код), сайт проверен с главной.
    pub input_broken: Option<u16>,
    /// Сайт проверен через свой прокси (напрямую не открылся или прокси включён для всего).
    pub via_proxy: bool,
    /// Свои JS-файлы сайта (до шести), если счётчиков аналитики нет в HTML.
    pub site_js_lower: String,
    /// Своих скриптов сайта, которые не скачивались (бандлы Next.js — десятки файлов):
    /// не нашли счётчик — это ещё не «аналитики нет».
    pub site_js_unchecked: usize,
    /// Сертификат не прошёл строгую проверку, содержимое загружено без неё.
    pub tls_lenient: bool,
    /// Скан шёл с IP другой страны, чем район.
    pub abroad: bool,
    /// Что было при попытке напрямую, если пришлось идти через прокси.
    pub direct_error: Option<String>,
    /// Главная рисуется скриптами: в исходном HTML почти нет текста.
    pub js_shell: bool,
    /// Слов в исходном HTML, если главная отрендерена браузером (тогда `page` — после рендера).
    pub raw_words: Option<usize>,
    /// Блоков JSON-LD в исходном HTML, если главная отрендерена браузером.
    pub raw_jsonld: Option<usize>,
    /// title и первый H1, которых нет в исходном HTML и которые появились только после рендера.
    pub js_title: Option<String>,
    pub js_h1: Option<String>,
    pub render_error: Option<String>,
    pub elapsed_ms: u64,
}

impl SiteSnapshot {
    pub fn final_url(&self) -> Option<Url> {
        self.home.as_ref().and_then(|h| Url::parse(&h.final_url).ok())
    }
    pub fn robots_body(&self) -> Option<&str> {
        self.robots.as_ref().filter(|r| r.status == Some(200)).map(|r| r.body.as_str())
    }
    /// Главная плюс обойдённые страницы с разобранным HTML.
    /// Главная и обойдённые страницы своего домена: обход, ушедший по редиректу на чужой сайт
    /// (mogo.by/blog → yoowills.by), в метрики сайта не попадает.
    pub fn all_pages(&self) -> impl Iterator<Item = &PageInfo> {
        let reg = |u: &str| Url::parse(u).ok().and_then(|u| u.host_str().map(registrable_domain));
        let home = self.page.as_ref().and_then(|p| reg(&p.url));
        self.page.iter().chain(self.pages.iter().filter_map(|p| p.info.as_ref()).filter(move |i| home.is_none() || reg(&i.url) == home))
    }

    /// Сайт открылся по http://, но тот же хост отвечает и по https:// с валидным сертификатом.
    /// https-версия без перенаправления отдаёт HSTS: кто хоть раз её открыл, тому браузер сам
    /// переводит http:// на https:// (bonus-travel.by). Новый посетитель по http-ссылке — нет.
    pub fn https_hsts(&self) -> bool {
        self.https_probe.as_ref().is_some_and(|p| p.hsts)
    }

    pub fn https_without_redirect(&self) -> bool {
        self.https_probe.as_ref().is_some_and(|p| p.error.is_none() && p.final_status.is_some_and(|c| c < 400))
            && self.tls.as_ref().is_some_and(|t| t.valid || (self.tls_unread() && !self.tls_lenient))
    }

    /// TLS-проба не получила сертификат вовсе (сеть оборвала рукопожатие) — о сертификате она
    /// ничего не знает. Если при этом https-страница открылась со строгой проверкой
    /// (не `tls_lenient`), сертификат у сайта рабочий.
    pub fn tls_unread(&self) -> bool {
        self.tls.as_ref().is_some_and(|t| !t.valid && t.issuer.is_none() && t.days_left.is_none())
    }

    /// Короткая сводка для интерфейса.
    pub fn summary(&self) -> serde_json::Value {
        serde_json::json!({
            "input_url": self.input_url,
            "final_url": self.home.as_ref().map(|h| h.final_url.clone()),
            "status": self.home.as_ref().map(|h| h.status),
            "error": self.home_error,
            "ttfb_ms": self.home.as_ref().map(|h| h.ttfb_ms),
            "total_ms": self.home.as_ref().map(|h| h.total_ms),
            "http_version": self.home.as_ref().map(|h| h.http_version.clone()),
            "server": self.home.as_ref().and_then(|h| h.header("server").map(str::to_string)),
            "throttled": self.throttled,
            "input_broken": self.input_broken,
            "via_proxy": self.via_proxy,
            "tls_lenient": self.tls_lenient,
            "direct_error": self.direct_error,
            "js_shell": self.js_shell,
            "rendered": self.raw_words.is_some(),
            "render_error": self.render_error,
            "title": self.page.as_ref().and_then(|p| p.title.clone()),
            "site_name": self.page.as_ref().and_then(|p| p.og.iter().find(|(k, _)| k == "og:site_name").map(|(_, v)| v.clone())),
            "words": self.page.as_ref().map(|p| p.words),
            "pages_crawled": self.pages.len(),
            "sitemap_urls": self.sitemap.urls.len(),
            "links_checked": self.link_checks.len(),
            "tls": self.tls,
            "domain": self.domain,
            "psi": self.psi,
            "contacts": crate::sales::site_contacts(self),
            "elapsed_ms": self.elapsed_ms,
        })
    }
}

fn is_page_like(u: &Url) -> bool {
    const SKIP: &[&str] = &[
        ".jpg", ".jpeg", ".png", ".gif", ".webp", ".avif", ".svg", ".ico", ".pdf", ".doc", ".docx", ".xls",
        ".xlsx", ".zip", ".rar", ".7z", ".mp4", ".mp3", ".webm", ".css", ".js", ".xml", ".txt", ".json", ".apk",
    ];
    let path = u.path().to_ascii_lowercase();
    !SKIP.iter().any(|ext| path.ends_with(ext))
}

fn normalize(u: &Url) -> String {
    let mut u = u.clone();
    u.set_fragment(None);
    u.to_string()
}

fn parse_max_age(cc: &str) -> Option<u64> {
    cc.split(',').map(str::trim).find_map(|p| p.strip_prefix("max-age=").and_then(|v| v.parse().ok()))
}

async fn parse_page(body: String, url: String) -> Option<PageInfo> {
    tokio::task::spawn_blocking(move || page::extract(&body, &url)).await.ok()
}

/// robots.txt и sitemap: дольше обычного (большие файлы на медленных серверах) и с одним повтором.
async fn text_resource(f: &Fetcher, url: String, max: usize) -> TextResource {
    let patient = Some(Duration::from_secs(45));
    let mut r = f.get_with(&url, None, max, patient).await;
    if matches!(r, Err(crate::http::FetchError::Network(_))) {
        r = f.get_with(&url, None, max, patient).await;
    }
    match r {
        Ok(r) => TextResource { url, status: Some(r.status), bytes: r.body_bytes, body: r.body, error: None },
        Err(e) => TextResource { url, error: Some(e.to_string()), ..Default::default() },
    }
}

async fn bot_probe(f: &Fetcher, url: &str, agent: &'static str, ua: &'static str) -> BotProbe {
    match f.get_limited(url, Some(ua), 256 * 1024).await {
        Ok(r) => BotProbe {
            agent: agent.to_string(),
            status: Some(r.status),
            bytes: r.body_bytes,
            challenge: looks_like_challenge(&r),
            error: None,
        },
        Err(e) => BotProbe { agent: agent.to_string(), error: Some(e.to_string()), ..Default::default() },
    }
}

/// Много ли среди проверок «не ответил» (таймаут, обрыв) — признак защиты от частых запросов.
fn mostly_unreachable(list: &[UrlStatus]) -> bool {
    let bad = list.iter().filter(|c| c.unreachable()).count();
    bad >= 3 && bad * 2 >= list.len()
}

/// Ответ похож на заглушку WAF/защиты от ботов (капча, «Just a moment…», DDoS-Guard).
pub fn looks_like_challenge(f: &Fetched) -> bool {
    if f.header("cf-mitigated").is_some() {
        return true;
    }
    let head: String = f.body.chars().take(4000).collect::<String>().to_lowercase();
    if [
        "<title>just a moment", "<title>attention required", "<title>проверка браузера", "<title>access denied",
        // WPX («Security Challenge», grid24), Simply.com («Checking your browser», 454), SiteGround, Sucuri, Imunify360.
        "<title>checking your browser", "<title>security challenge", "<title>verifying", "<title>please wait while",
        "<title>robot challenge", "<title>sucuri website firewall", "<title>bot verification",
    ]
    .iter()
    .any(|m| head.contains(m))
    {
        return true;
    }
    // Заглушки защиты маленькие; на больших страницах слова вроде «captcha» встречаются законно.
    if f.body_bytes > 60_000 {
        return false;
    }
    let head: String = f.body.chars().take(20_000).collect::<String>().to_lowercase();
    [
        "just a moment", "cf-chl", "/cdn-cgi/challenge-platform", "attention required", "ddos-guard",
        "checking your browser", "qrator", "servicepipe", "variti", "stormwall", "access denied", "bm-verify",
        "_sec/cp_challenge", "sec-if-cpt", "проверка браузера", "geoip",
        "wpx security", "wpxsecurity", "sgcaptcha", "sucuri website firewall", "imunify360", "verify you are human",
        "verifying your browser", "bot verification",
    ]
    .iter()
    .any(|m| head.contains(m))
}

/// Главная не пустила сканер: 401/403/429/503 или заглушка защиты. Посетители сайт, скорее всего, видят.
pub fn is_blocked(home: &Fetched) -> bool {
    // 451 — закрыто по закону или для страны; 404 от DDoS-Guard — его заглушка для чужих IP.
    let ddos_guard = home.header("server").is_some_and(|s| s.to_ascii_lowercase().contains("ddos-guard"))
        && !home.body.to_lowercase().contains("domain has been assigned");
    // Заглушка защиты — при любом коде: Simply.com отдаёт «Checking your browser» с кодом 454.
    matches!(home.status, 401 | 403 | 429 | 451 | 498 | 503) || (home.status == 404 && ddos_guard) || looks_like_challenge(home)
}

/// Код ответа публичного DNS (Cloudflare DoH): 0 — есть, 2 — сервер домена не отвечает,
/// 3 — домена нет, 5 — отказ.
async fn doh_status(api: &reqwest::Client, host: &str) -> Option<i64> {
    let v: serde_json::Value = api
        .get("https://cloudflare-dns.com/dns-query")
        .header("accept", "application/dns-json")
        .query(&[("name", host), ("type", "A")])
        .timeout(Duration::from_secs(6))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    v["Status"].as_i64()
}

/// Сайт открылся на поддомене конструктора (pekarnipirogova.tilda.ws), а canonical указывает
/// на свой домен компании: технический поддомен обычно закрыт от поиска, проверять надо свой.
fn own_site_from_canonical(s: &SiteSnapshot) -> Option<String> {
    let home = s.home.as_ref()?;
    let canonical = s.page.as_ref()?.canonical.as_deref()?;
    let (kind_home, host_home) = crate::text::classify_website(&home.final_url);
    let (kind_can, host_can) = crate::text::classify_website(canonical);
    let technical = kind_home == crate::text::SiteKind::ConstructorSubdomain || [".tilda.ws", ".tilda.ru", ".tilda.site"].iter().any(|t| host_home.ends_with(t));
    (technical && kind_can == crate::text::SiteKind::Own && host_can != host_home)
        .then(|| canonical.to_string())
}

/// Приоритет ссылки при обходе: контакты, цены, точки продаж — вперёд, блог — назад.
fn crawl_priority(href: &str, text: &str) -> i32 {
    let s = format!("{} {}", href, text).to_lowercase();
    let has = |ws: &[&str]| ws.iter().any(|w| s.contains(w));
    if has(&[
        "contact", "kontakt", "контакт", "svyaz", "связ", "get-in-touch", "get in touch", "impressum", "reach-us", "find-us", "contatt",
        "contato", "nous-trouver", "como-llegar", "anfahrt",
        // США и Европа: «Hours/Location», «Directions», «Visit us» — там телефон и адрес (brewersrownyc.com/location).
        "location", "hours", "directions", "visit-us", "visit us", "как добраться", "kak-dobratsya", "standort", "orari",
    ]) {
        30
    } else if has(&[
        "price", "ceny", "tseny", "prais", "прайс", "прейскурант", "цены", "стоимост", "menu", "меню", "preise", "prix", "tarif",
        "precio", "prezzi", "cennik", "prijs", "preco", "preço", "carta", "speisekarte",
    ]) {
        25
    } else if has(&[
        "magazin", "магазин", "salon", "салон", "stores", "shops", "filial", "филиал", "adres", "адрес", "аптек", "apteki", "pharmac",
        "otdelen", "отделен", "tiendas", "negozi", "standort", "locations", "sklepy",
    ]) {
        20
    } else if has(&[
        "uslug", "услуг", "service", "leistung", "prestation", "servicio", "servizi", "diensten", "servicos", "behandlung", "soins",
        "treatment",
    ]) {
        15
    } else if has(&["faq", "vopros", "voprosy", "вопрос", "questions", "preguntas", "fragen", "domande", "pytania"]) {
        // Выше страниц услуг: страница вопросов одна, а услуг много — иначе до /faq обход не доходит
        // (vip-permanent.by/vopros/, 8tm.by/faq — I09 «вопросов нет»).
        24
    } else if has(&[
        "about", "o-nas", "о нас", "о компании", "o-kompanii", "ueber-uns", "uber-uns", "a-propos", "qui-sommes", "sobre-nosotros",
        "quienes-somos", "chi-siamo", "over-ons", "sobre-nos", "/company", "о-нас", "о-компании", "о-клинике", "o-klinike", "o-firmie",
    ]) {
        10
    } else if has(&["blog", "news", "novost", "новост", "stati", "статьи"]) {
        -10
    } else {
        0
    }
}

/// Стоит ли повторить через прокси: сервер не ответил или ответил отказом. Несуществующий
/// домен и битый адрес прокси не исправит.
fn worth_proxy(s: &SiteSnapshot) -> bool {
    match (&s.home, s.home_error.as_deref()) {
        (Some(h), _) => matches!(h.status, 403 | 451 | 429) || looks_like_challenge(h),
        (None, Some(e)) => !e.contains("не резолвится") && !e.contains("некорректный"),
        (None, None) => false,
    }
}

/// Собрать снимок сайта. Если задан свой прокси как запасной, а напрямую сайт не открылся
/// или закрыт (часто — по географии выхода VPN), проверка целиком повторяется через прокси.
pub async fn collect(f: &Fetcher, input: &str, opt: &CollectOptions) -> SiteSnapshot {
    let mut snap = collect_once(f, input, opt).await;
    // Сертификат не прошёл строгую проверку (чаще всего неполная цепочка): браузеры такие сайты
    // обычно открывают, поэтому содержимое проверяем без проверки сертификата, а о самом
    // сертификате скажет B04.
    if snap.home.is_none()
        && snap.home_error.as_deref().is_some_and(|e| e.contains("TLS") || e.contains("сертификат"))
        && let Some(l) = f.lenient()
    {
        let alt = collect_once(&l, input, opt).await;
        if alt.home.is_some() {
            snap = alt;
            snap.tls_lenient = true;
        }
    }
    // Системный DNS не нашёл домен: публичный DNS скажет, домена нет совсем или не отвечает
    // его DNS-сервер (бывает, что серверы страны не отвечают на запросы из-за границы).
    if snap.home.is_none() && snap.home_error.as_deref().is_some_and(|e| e.contains("не резолвится")) {
        let host = parse_url_lenient(input).map(|u| host_of(&u)).unwrap_or_default();
        match doh_status(&f.api, &host).await {
            Some(3) => snap.home_error = Some("домен не существует".into()),
            Some(2) | Some(5) => snap.home_error = Some("DNS-сервер домена не отвечает".into()),
            _ => {}
        }
    }
    if let Some(own) = own_site_from_canonical(&snap) {
        let alt = Box::pin(collect_once(f, &own, opt)).await;
        if alt.home.as_ref().is_some_and(|h| h.ok()) {
            snap = alt;
        }
    }
    snap.via_proxy = f.is_proxied();
    snap.abroad = opt.abroad;
    let Some(p) = f.proxied().filter(|_| worth_proxy(&snap)) else {
        return snap;
    };
    let mut alt = collect_once(&p, input, opt).await;
    if !alt.home.as_ref().is_some_and(|h| h.ok()) {
        return snap;
    }
    alt.via_proxy = true;
    alt.abroad = opt.abroad;
    alt.direct_error = snap.home_error.clone().or_else(|| snap.home.as_ref().map(|h| format!("HTTP {}", h.status)));
    alt
}

async fn collect_once(f: &Fetcher, input: &str, opt: &CollectOptions) -> SiteSnapshot {
    let started = Instant::now();
    let mut s = SiteSnapshot { input_url: input.to_string(), ..Default::default() };
    let Some(start_url) = parse_url_lenient(input) else {
        s.home_error = Some("некорректный адрес сайта".into());
        return s;
    };

    // Главная. Если https не отвечает и адрес был без схемы — пробуем http.
    let mut home = f.get(start_url.as_str(), None).await;
    if home.is_err() && !input.contains("://") {
        let mut alt = start_url.clone();
        let _ = alt.set_scheme("http");
        if let Ok(h) = f.get(alt.as_str(), None).await {
            home = Ok(h);
        }
    }
    let host_for_domain = host_of(&start_url);
    let domain = registrable_domain(&host_for_domain);

    // Ссылка из карточки на внутреннюю страницу отвечает 404/410 — проверяем главную сайта.
    if let Ok(h) = &home
        && matches!(h.status, 404 | 410)
        && start_url.path() != "/"
    {
        let mut root = start_url.clone();
        root.set_path("/");
        root.set_query(None);
        if let Ok(r) = f.get(root.as_str(), None).await
            && r.ok()
        {
            s.input_broken = Some(h.status);
            home = Ok(r);
        }
    }
    let home = match home {
        Ok(h) => h,
        Err(e) => {
            s.home_error = Some(e.to_string());
            // Даже у мёртвого сайта полезно знать судьбу домена.
            let (d, dns) = tokio::join!(domain_info::lookup(&f.api, &domain), domain_info::dns(&f.api, &domain));
            s.domain = Some(d);
            s.dns = Some(dns);
            s.elapsed_ms = started.elapsed().as_millis() as u64;
            return s;
        }
    };
    let final_url = Url::parse(&home.final_url).unwrap_or(start_url.clone());
    let origin = final_url.origin().ascii_serialization();
    let host = final_url.host_str().unwrap_or("").to_string();
    if is_blocked(&home) {
        // Дальше долбить сайт бессмысленно: собираем только TLS и данные домена.
        let reg = registrable_domain(&host);
        let (tls_i, dom, dns) = tokio::join!(
            async { if final_url.scheme() == "https" { Some(tls::probe(&host, f.interface()).await) } else { None } },
            domain_info::lookup(&f.api, &reg),
            domain_info::dns(&f.api, &reg),
        );
        s.tls = tls_i;
        s.domain = Some(dom);
        s.dns = Some(dns);
        s.home = Some(home);
        s.elapsed_ms = started.elapsed().as_millis() as u64;
        return s;
    }
    if home.is_html() {
        s.page = parse_page(home.body.clone(), home.final_url.clone()).await;
        // Счётчики аналитики часто подключают в своём JS-файле сайта, а не в HTML.
        if let Some(p) = &s.page {
            let markers = ["mc.yandex.ru/metrika", "ym(", "googletagmanager.com", "gtag(", "google-analytics.com", "matomo", "plausible"];
            if !markers.iter().any(|m| p.html_lower.contains(m)) {
                let host = host_of(&Url::parse(&home.final_url).unwrap_or(start_url.clone()));
                let own: Vec<String> = p
                    .scripts
                    .iter()
                    .filter_map(|t| t.src.clone())
                    .filter(|src| Url::parse(src).ok().is_some_and(|u| host_of(&u).trim_start_matches("www.") == host.trim_start_matches("www.")))
                    .filter(|src| !["jquery", "vendor", "bootstrap", "swiper", "slick", "polyfill"].iter().any(|v| src.to_lowercase().contains(v)))
                    .collect();
                // Сначала скрипты с говорящими именами (cookies.js, metrika.js, analytics), всего до 6:
                // счётчики часто в общем бандле (main.*.js, autoptimize_*.js), включаемые после согласия на cookie.
                let mut own = own;
                own.sort_by_key(|src| {
                    let l = src.to_lowercase();
                    !["metrika", "analytic", "counter", "cookie", "gtm", "ym", "stat"].iter().any(|k| l.contains(k))
                });
                s.site_js_unchecked = own.len().saturating_sub(6);
                own.truncate(6);
                for src in own {
                    if let Ok(r) = f.get_limited(&src, None, 2 * 1024 * 1024).await
                        && r.ok()
                    {
                        s.site_js_lower.push_str(&r.body.to_lowercase());
                        s.site_js_lower.push('\n');
                    }
                }
            }
        }
    }
    if let Some(raw) = s.page.as_ref().filter(|p| needs_render(p)) {
        s.js_shell = is_js_shell(raw);
        let raw_words = raw.words;
        let (no_title, no_h1) = (raw.title.is_none(), raw.h1s().is_empty());
        let raw_links = raw.links.iter().filter(|l| l.internal).count();
        let raw_jsonld = raw.jsonld.len();
        if let Some(browser) = &opt.browser {
            match crate::render::dump_dom(browser, &home.final_url, browser_ua(), f.accept_language(), f.browser_proxy().as_deref(), Duration::from_secs(35)).await {
                // Рендер попал на проверку защиты (Cloudflare «Just a moment») — это не страница сайта.
                Ok(html) if ["<title>just a moment", "<title>attention required", "cf-chl"].iter().any(|m| html.to_lowercase().contains(m)) => {
                    s.render_error = Some("рендер попал на проверку защиты от ботов".into())
                }
                Ok(html) => match parse_page(html, home.final_url.clone()).await {
                    Some(rendered) => {
                        if no_title {
                            s.js_title = rendered.title.clone();
                        }
                        if no_h1 {
                            s.js_h1 = rendered.h1s().first().map(|h| h.to_string());
                        }
                        // Больше текста или заметно больше ссылок (меню и каталог рисует JS) — берём отрисованную.
                        if rendered.words > raw_words || rendered.links.iter().filter(|l| l.internal).count() >= raw_links + 5 {
                            s.raw_words = Some(raw_words);
                            s.raw_jsonld = Some(raw_jsonld);
                            s.page = Some(rendered);
                        } else if s.js_shell {
                            s.render_error = Some("после рендера текста не прибавилось".into());
                        }
                    }
                    None if s.js_shell => s.render_error = Some("после рендера текста не прибавилось".into()),
                    None => {}
                },
                Err(e) => s.render_error = Some(e),
            }
        }
    }

    // Параллельные пробы.
    let http_url = format!("http://{host}/");
    let mirror_host = host.strip_prefix("www.").map(str::to_string).unwrap_or_else(|| format!("www.{host}"));
    let mirror_url = format!("{}://{mirror_host}/", final_url.scheme());
    let nonce = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
    let not_found_url = format!("{origin}/msscan-check-{nonce:x}");
    let small = 64 * 1024;
    let home_url: &str = &home.final_url;
    let reg_domain = registrable_domain(&host);

    let https_url = format!("https://{host}/");
    let plain_http = final_url.scheme() == "http";
    let probes = async {
        tokio::join!(
            async { RedirectProbe::from(&http_url, f.get_limited(&http_url, None, small).await) },
            async {
                if plain_http {
                    // Разовый сбой пробы https — ещё не «сайт без HTTPS»: второй попыткой.
                    let mut r = f.get_limited(&https_url, None, small).await;
                    if r.is_err() {
                        r = f.get_limited(&https_url, None, small).await;
                    }
                    Some(RedirectProbe::from(&https_url, r))
                } else {
                    None
                }
            },
            async { RedirectProbe::from(&mirror_url, f.get_limited(&mirror_url, None, small).await) },
            f.content_encoding(home_url),
            text_resource(f, format!("{origin}/robots.txt"), 600 * 1024),
            async { RedirectProbe::from(&not_found_url, f.get_limited(&not_found_url, None, small).await) },
            async { f.get_limited(&format!("{origin}/llms.txt"), None, small).await.ok().map(|r| r.status) },
            futures::future::join_all(BOT_PROBES.iter().map(|&(agent, ua)| bot_probe(f, home_url, agent, ua))),
            bot_view(f, home_url, YANDEX_BOT_UA),
            async { RedirectProbe::from(home_url, f.get_limited(home_url, Some(&mobile_ua()), small).await) },
        )
    };
    let ask_yandex = opt.yandex || yandex_zone(&host);
    let external = async {
        tokio::join!(
            // Сертификат проверяем и у сайта на http://: часто HTTPS есть, но нет перенаправления.
            async { Some(tls::probe(&host, f.interface()).await) },
            domain_info::lookup(&f.api, &reg_domain),
            domain_info::dns_for(&f.api, &host, &reg_domain),
            async {
                match &opt.psi_key {
                    Some(k) if !k.is_empty() => Some(psi::run(&f.api, k, home_url).await),
                    _ => None,
                }
            },
            async {
                match &opt.safe_browsing_key {
                    Some(k) if !k.is_empty() => Some(psi::safe_browsing(&f.api, k, home_url).await),
                    _ => None,
                }
            },
            async { if ask_yandex { Some(crate::yandex_info::site_info(&f.api, &host).await) } else { None } },
            async { if ask_yandex { Some(crate::yandex_info::safety(&f.api, home_url).await) } else { None } },
        )
    };
    let ((http_p, https_p, mirror_p, enc, robots_r, nf, llms, bots, ya_bot, mobile), (tls_i, dom, dns, psi_r, sb, ya_info, ya_safe)) =
        tokio::join!(probes, external);
    // Ответ роботу сильно расходится с ответом браузеру — переспросить один раз: урезанная страница
    // бывает и случайно (перегрузка, обрыв), это ещё не клоакинг.
    let raw = s.raw_words.or(s.page.as_ref().map(|p| p.words)).unwrap_or(0);
    let differs = |b: &BotView| b.error.is_none() && b.status == Some(200) && (b.words * 2 < raw || b.words > raw * 2 + 300);
    let ya_bot = if differs(&ya_bot) {
        let again = bot_view(f, home_url, YANDEX_BOT_UA).await;
        if differs(&again) { ya_bot } else { again }
    } else {
        ya_bot
    };
    s.yandex_bot = Some(ya_bot);
    s.mobile_probe = Some(mobile);
    s.yandex = ya_info;
    s.yandex_safety = ya_safe;
    s.http_probe = Some(http_p);
    s.https_probe = https_p;
    s.mirror_probe = Some(mirror_p);
    s.compression = enc;
    s.not_found = Some(nf);
    s.llms_txt = llms;
    s.bots = bots;
    s.tls = tls_i;
    s.domain = Some(dom);
    s.dns = Some(dns);
    s.psi = psi_r;
    s.safe_browsing = sb;
    if robots_r.status == Some(200) {
        s.robots_directives = Some(robots::parse(&robots_r.body));
    }
    s.robots = Some(robots_r);

    // Sitemap: из robots.txt или /sitemap.xml; индекс — до 3 дочерних файлов.
    let all_declared: Vec<String> = s.robots_directives.as_ref().map(|d| d.sitemaps.clone()).unwrap_or_default();
    // Sitemap чужого домена в robots.txt (скопировали robots.txt с другого сайта) — ошибка,
    // а свой sitemap ищем по обычному адресу.
    let own_domain = registrable_domain(&host);
    let (declared, foreign): (Vec<String>, Vec<String>) = all_declared
        .into_iter()
        .partition(|u| Url::parse(u).ok().is_some_and(|x| registrable_domain(x.host_str().unwrap_or("")) == own_domain));
    s.sitemap.foreign = foreign;
    s.sitemap.declared = declared.clone();
    // Сайт в подпапке бесплатного Wix (имя.wixsite.com/сайт) — sitemap лежит в этой подпапке.
    let sitemap_base = match final_url.path_segments().and_then(|mut p| p.next()).filter(|seg| !seg.is_empty() && host.ends_with(".wixsite.com")) {
        Some(seg) => format!("{origin}/{seg}"),
        None => origin.to_string(),
    };
    let mut queue: Vec<String> = if declared.is_empty() { vec![format!("{sitemap_base}/sitemap.xml")] } else { declared.into_iter().take(3).collect() };
    let mut fetched_maps = 0;
    while let Some(sm_url) = queue.pop() {
        if fetched_maps >= 6 {
            s.sitemap.partial = true;
            break;
        }
        fetched_maps += 1;
        let r = text_resource(f, sm_url.clone(), 20 * 1024 * 1024).await;
        let parsed = (r.status == Some(200)).then(|| sitemap::parse(&r.body));
        s.sitemap.fetched.push((sm_url, r.status, parsed.as_ref().map(|p| p.kind)));
        if let Some(p) = parsed {
            match p.kind {
                SitemapKind::Index => {
                    let mut children: Vec<String> = p.entries.into_iter().map(|(l, _)| l).collect();
                    children.sort_by_key(|c| c.contains("image") || c.contains("video"));
                    if children.len() > 3 {
                        s.sitemap.partial = true;
                    }
                    queue.extend(children.into_iter().take(3));
                }
                // urlset, где вместо страниц ссылки на другие sitemap (furnitop.by) — читаем как индекс.
                SitemapKind::UrlSet
                    if !p.entries.is_empty()
                        && p.entries.iter().all(|(l, _)| {
                            let l = l.to_lowercase();
                            l.ends_with(".xml") || l.ends_with(".xml.gz")
                        }) =>
                {
                    s.sitemap.nested_in_urlset = true;
                    let children: Vec<String> = p.entries.into_iter().map(|(l, _)| l).collect();
                    if children.len() > 3 {
                        s.sitemap.partial = true;
                    }
                    queue.extend(children.into_iter().take(3));
                }
                SitemapKind::UrlSet => {
                    if p.entries.len() > 50_000 {
                        s.sitemap.too_big = true;
                    }
                    for (loc, lm) in p.entries.into_iter().take(50_000) {
                        s.sitemap.urls.push(loc);
                        if let Some(l) = lm {
                            s.sitemap.lastmods.push(l);
                        }
                    }
                }
                SitemapKind::Invalid => {}
            }
        }
    }
    if !s.sitemap.urls.is_empty() && opt.sitemap_sample > 0 {
        let step = (s.sitemap.urls.len() / opt.sitemap_sample).max(1);
        let sample: Vec<String> = s.sitemap.urls.iter().step_by(step).take(opt.sitemap_sample).cloned().collect();
        s.sitemap_checks =
            stream::iter(sample).map(|u| async move { f.check(&u).await }).buffer_unordered(opt.concurrency).collect().await;
        s.throttled = mostly_unreachable(&s.sitemap_checks);
    }

    // Обход внутренних страниц (BFS, глубина ≤ 3, с учётом robots.txt для `*`).
    let robots_body = s.robots_body().map(str::to_string);
    let crawl_allowed = |u: &str| robots_body.as_deref().is_none_or(|b| robots::allowed(b, CRAWLER_AGENT, u));
    let mut seen: HashSet<String> = HashSet::from([normalize(&final_url), normalize(&start_url)]);
    let mut frontier: Vec<(String, u8)> = Vec::new();
    // Обойти можно немного страниц — сначала те, где контакты, цены и адреса точек.
    let mut prio: HashMap<String, i32> = HashMap::new();
    let push_links = |info: &PageInfo, depth: u8, seen: &mut HashSet<String>, frontier: &mut Vec<(String, u8)>, prio: &mut HashMap<String, i32>| {
        for l in info.links.iter().filter(|l| l.internal) {
            let Ok(mut u) = Url::parse(&l.href) else { continue };
            // Сайт на www.8tm.by, а меню ссылается на 8tm.by/faq: та же страница — приводим к адресу
            // сайта, иначе обход её пропускал (I09 «вопросов нет» при странице /faq в меню).
            let bare = |h: &str| h.trim_start_matches("www.").to_string();
            let Some(link_host) = u.host_str().map(str::to_string) else { continue };
            if bare(&link_host) != bare(&host) || !is_page_like(&u) {
                continue;
            }
            if link_host != host && u.set_host(Some(&host)).is_err() {
                continue;
            }
            let n = normalize(&u);
            if seen.insert(n.clone()) && crawl_allowed(&n) {
                prio.insert(n.clone(), crawl_priority(&l.href, &l.text));
                frontier.push((n, depth));
            }
        }
    };
    if let Some(p) = &s.page {
        push_links(p, 1, &mut seen, &mut frontier, &mut prio);
    }
    // Пачками по числу параллельных запросов: так серия таймаутов замечается после первой пачки.
    while !frontier.is_empty() && s.pages.len() < opt.max_pages && !s.throttled {
        frontier.sort_by_key(|(u, d)| (*d, -prio.get(u).copied().unwrap_or(0)));
        let take = (opt.max_pages - s.pages.len()).min(frontier.len()).min(opt.concurrency.max(1) * 2);
        let batch: Vec<(String, u8)> = frontier.drain(..take).collect();
        let results: Vec<(String, u8, Result<Fetched, crate::http::FetchError>)> = stream::iter(batch)
            .map(|(u, d)| async move {
                let r = match tokio::time::timeout(INNER_PAGE_TIMEOUT, f.get(&u, None)).await {
                    Ok(r) => r,
                    Err(_) => Err(crate::http::FetchError::Network("таймаут".into())),
                };
                (u, d, r)
            })
            .buffer_unordered(opt.concurrency)
            .collect()
            .await;
        let errors = results.iter().filter(|(_, _, r)| r.is_err()).count();
        if errors >= 3 && errors * 2 >= results.len() {
            s.throttled = true;
        }
        let mut next = Vec::new();
        for (u, depth, r) in results {
            match r {
                Ok(page) => {
                    let info = if page.ok() && page.is_html() { parse_page(page.body.clone(), page.final_url.clone()).await } else { None };
                    if let Some(i) = &info
                        && depth < 3
                    {
                        push_links(i, depth + 1, &mut seen, &mut next, &mut prio);
                    }
                    s.pages.push(CrawledPage {
                        url: u,
                        final_url: page.final_url.clone(),
                        depth,
                        status: Some(page.status),
                        redirected: !page.chain.is_empty(),
                        error: None,
                        bytes: page.body_bytes,
                        info,
                    });
                }
                Err(e) => s.pages.push(CrawledPage {
                    url: u.clone(),
                    final_url: u,
                    depth,
                    status: None,
                    redirected: false,
                    error: Some(e.to_string()),
                    bytes: 0,
                    info: None,
                }),
            }
        }
        frontier.extend(next);
    }

    // Ссылки, которые не обходили: проверяем статус выборочно.
    let crawled: HashSet<&str> = s.pages.iter().map(|p| p.url.as_str()).collect();
    let mut targets: Vec<String> = frontier.iter().map(|(u, _)| u.clone()).filter(|u| !crawled.contains(u.as_str())).collect();
    targets.truncate(if s.throttled { 0 } else { opt.link_checks });
    s.link_checks =
        stream::iter(targets).map(|u| async move { f.check(&u).await }).buffer_unordered(opt.concurrency).collect().await;
    if mostly_unreachable(&s.link_checks) {
        s.throttled = true;
    }

    // Картинки главной и кэширование статики.
    if let Some(p) = &s.page {
        let imgs: Vec<String> = p
            .images
            .iter()
            .filter(|i| !i.tiny)
            .filter_map(|i| i.shown.clone().or_else(|| i.src.clone()))
            .filter(|src| src.starts_with("http"))
            .take(if s.throttled { 0 } else { opt.image_checks })
            .collect();
        s.image_checks =
            stream::iter(imgs).map(|u| async move { f.check(&u).await }).buffer_unordered(opt.concurrency).collect().await;
        // Разовый сбой сервера — ещё не битая картинка: ошибочные проверяем второй раз.
        for c in s.image_checks.iter_mut().filter(|c| c.http_error() || c.unreachable()) {
            let again = f.check(&c.url).await;
            if !again.http_error() && !again.unreachable() {
                *c = again;
            }
        }

        let mut assets: Vec<String> = Vec::new();
        if let Some((css, _)) = p.stylesheets.first() {
            assets.push(css.clone());
        }
        if let Some(js) = p.scripts.iter().filter_map(|x| x.src.clone()).find(|x| x.contains(&host)) {
            assets.push(js);
        }
        if let Some(img) = p.images.iter().filter(|i| !i.tiny).filter_map(|i| i.src.clone()).find(|x| x.starts_with("http")) {
            assets.push(img);
        }
        for a in assets {
            // GET, а не HEAD: часть серверов не присылает Cache-Control на HEAD (marc-cain.com).
            if let Ok(r) = f.get_limited(&a, None, 4 * 1024).await {
                let cc = r.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("cache-control")).map(|(_, v)| v.clone());
                s.assets.push(AssetCache { max_age: cc.as_deref().and_then(parse_max_age), cache_control: cc, url: a });
            }
        }
        if !p.icon_link {
            s.favicon_status = f.check(&format!("{origin}/favicon.ico")).await.final_status;
        }
    }

    s.home = Some(home);
    s.elapsed_ms = started.elapsed().as_millis() as u64;
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_age_and_page_like() {
        assert_eq!(parse_max_age("public, max-age=31536000, immutable"), Some(31_536_000));
        assert_eq!(parse_max_age("no-cache"), None);
        assert!(is_page_like(&Url::parse("https://a.ru/uslugi/").unwrap()));
        assert!(!is_page_like(&Url::parse("https://a.ru/price.pdf").unwrap()));
    }
}
