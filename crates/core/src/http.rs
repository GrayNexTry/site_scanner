//! HTTP-клиент сканера: ручные редиректы с записью цепочки, TTFB, лимит тела, перекодировка.

use std::time::{Duration, Instant};

use reqwest::header::{HeaderMap, ACCEPT, ACCEPT_ENCODING, ACCEPT_LANGUAGE, LOCATION, USER_AGENT};
use serde::{Deserialize, Serialize};
use url::Url;

/// Основная версия Chrome на сегодня: выходит примерно раз в четыре недели
/// (131 — 12.11.2024). Устаревший браузер — частый повод для защиты сайта отказать.
pub fn chrome_major() -> u32 {
    let base = chrono::NaiveDate::from_ymd_opt(2024, 11, 12).unwrap_or_default();
    let days = (chrono::Utc::now().date_naive() - base).num_days().max(0) as u32;
    131 + days / 28
}

/// User-Agent свежего Chrome под Windows — так выглядит самый обычный посетитель.
pub fn browser_ua() -> &'static str {
    static UA: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    UA.get_or_init(|| {
        format!("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{}.0.0.0 Safari/537.36", chrome_major())
    })
}

/// Таймаут выборочных проверок ссылок и картинок: долго ждать их незачем.
const CHECK_TIMEOUT: Duration = Duration::from_secs(8);

impl UrlStatus {
    /// Сервер ответил статусом ошибки (а не просто не ответил).
    pub fn http_error(&self) -> bool {
        self.final_status.is_some_and(|s| s >= 400)
    }
    pub fn unreachable(&self) -> bool {
        self.final_status.is_none()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hop {
    pub url: String,
    pub status: u16,
}

/// Результат загрузки страницы.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fetched {
    pub url: String,
    pub final_url: String,
    pub status: u16,
    /// Редиректы до финального ответа (без него самого).
    pub chain: Vec<Hop>,
    /// Заголовки финального ответа, имена в нижнем регистре.
    pub headers: Vec<(String, String)>,
    #[serde(skip)]
    pub body: String,
    pub body_bytes: usize,
    pub truncated: bool,
    pub ttfb_ms: u64,
    pub total_ms: u64,
    pub http_version: String,
    pub content_type: String,
}

impl Fetched {
    pub fn header(&self, name: &str) -> Option<&str> {
        let n = name.to_ascii_lowercase();
        self.headers.iter().find(|(k, _)| *k == n).map(|(_, v)| v.as_str())
    }
    pub fn headers_all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.headers.iter().filter(move |(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
    pub fn is_html(&self) -> bool {
        self.content_type.contains("html") || (self.content_type.is_empty() && self.body.contains("<html"))
    }
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// Короткая проверка URL: статус, был ли редирект, итоговый статус.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UrlStatus {
    pub url: String,
    pub status: Option<u16>,
    pub final_status: Option<u16>,
    pub final_url: Option<String>,
    pub redirected: bool,
    pub error: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("некорректный URL: {0}")]
    BadUrl(String),
    #[error("слишком много редиректов")]
    TooManyRedirects,
    #[error("{0}")]
    Network(String),
}

fn describe(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        return "таймаут".into();
    }
    if e.is_connect() {
        let s = format!("{e:?}");
        if s.contains("dns") || s.contains("failed to lookup") || s.contains("Name or service") {
            return "домен не резолвится".into();
        }
        if s.contains("certificate") || s.contains("Certificate") {
            return "ошибка TLS-сертификата".into();
        }
        return "не удалось подключиться".into();
    }
    // Полная цепочка причин — для журнала; человеку — короткая формулировка.
    let mut full = e.to_string();
    let mut src = std::error::Error::source(e);
    while let Some(s) = src {
        full = format!("{full}: {s}");
        src = s.source();
    }
    tracing::debug!("сетевая ошибка: {full}");
    let l = full.to_lowercase();
    if l.contains("close_notify") || l.contains("connection closed") || l.contains("reset by peer") || l.contains("broken pipe") {
        "соединение оборвано сервером".into()
    } else if l.contains("certificate") || l.contains("tls") || l.contains("handshake") {
        "ошибка TLS".into()
    } else if l.contains("decoding") || l.contains("decompress") {
        "ответ пришёл повреждённым".into()
    } else if l.contains("timed out") || l.contains("timeout") {
        "таймаут".into()
    } else {
        "ошибка соединения".into()
    }
}

#[derive(Debug, Clone)]
pub struct FetchConfig {
    pub timeout: Duration,
    pub max_body: usize,
    /// Свой прокси (http://, https://, socks5://, socks5h://; можно с логином и паролем).
    pub proxy: Option<String>,
    /// true — всё через прокси; false — только сайты, которые напрямую не открылись.
    pub proxy_always: bool,
    /// Сетевой интерфейс для запросов к сайтам мимо VPN-туннеля (wlp99s0): SO_BINDTODEVICE,
    /// с ядра 5.7 без root. Не открылось напрямую — повтор через туннель.
    pub interface: Option<String>,
}

impl Default for FetchConfig {
    fn default() -> Self {
        Self { timeout: Duration::from_secs(15), max_body: 3 * 1024 * 1024, proxy: None, proxy_always: false, interface: None }
    }
}

/// Клиенты: `site` — без авто-редиректов (цепочка пишется вручную), `raw` — без распаковки
/// (видно Content-Encoding), `api` — для внешних API, редиректы автоматически.
#[derive(Clone)]
pub struct Fetcher {
    site: reqwest::Client,
    raw: reqwest::Client,
    pub api: reqwest::Client,
    cfg: FetchConfig,
    accept_language: String,
    /// Те же клиенты через свой прокси — запасной путь для сайтов, закрытых по географии.
    proxied: Option<std::sync::Arc<Fetcher>>,
    /// Те же клиенты без проверки сертификата: для сайтов с неполной цепочкой, которые
    /// браузеры открывают, а строгий клиент — нет. О сертификате всё равно скажет B04.
    lenient: Option<std::sync::Arc<Fetcher>>,
    /// Адрес прокси, если этот набор клиентов сам ходит через него.
    proxy_url: Option<String>,
    /// Интерфейс, к которому привязаны клиенты (мимо VPN-туннеля).
    interface: Option<String>,
}

/// Физический интерфейс для выхода мимо VPN: маршрут по умолчанию сейчас идёт через туннель
/// (xray0, tun0, wg0…), а у Wi-Fi или Ethernet есть свой маршрут по умолчанию с худшей метрикой.
/// None — VPN не перехватывает маршрут, привязка не нужна.
pub fn direct_interface() -> Option<String> {
    let table = std::fs::read_to_string("/proc/net/route").ok()?;
    direct_interface_from(&table)
}

fn direct_interface_from(table: &str) -> Option<String> {
    const TUNNELS: &[&str] = &["tun", "tap", "xray", "wg", "tailscale", "utun", "ppp", "vpn", "nordlynx", "proton", "zt", "sing", "clash", "mihomo", "outline", "ipsec"];
    let is_tunnel = |name: &str| TUNNELS.iter().any(|t| name.starts_with(t));
    let mut defaults: Vec<(String, u32)> = table
        .lines()
        .skip(1)
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            (f.len() >= 8 && f[1] == "00000000" && f.get(7) == Some(&"00000000")).then(|| (f[0].to_string(), f[6].parse().unwrap_or(u32::MAX)))
        })
        .collect();
    defaults.sort_by_key(|(_, m)| *m);
    let (best, _) = defaults.first()?;
    if !is_tunnel(best) {
        return None;
    }
    defaults.into_iter().map(|(n, _)| n).find(|n| !is_tunnel(n))
}

#[cfg(test)]
mod direct_tests {
    #[test]
    fn picks_wifi_behind_vpn_tunnel() {
        let table = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
xray0\t00000000\t00000000\t0001\t0\t0\t1\t00000000\t0\t0\t0\n\
wlp99s0\t00000000\t010A14AC\t0003\t0\t0\t600\t00000000\t0\t0\t0\n\
wlp99s0\t000A14AC\t00000000\t0001\t0\t0\t600\t00F0FFFF\t0\t0\t0\n";
        assert_eq!(super::direct_interface_from(table).as_deref(), Some("wlp99s0"));
        let no_vpn = "Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\n\
wlp99s0\t00000000\t010A14AC\t0003\t0\t0\t600\t00000000\n";
        assert_eq!(super::direct_interface_from(no_vpn), None, "без VPN привязка не нужна");
    }
}

impl Fetcher {
    pub fn new(cfg: FetchConfig) -> anyhow::Result<Self> {
        let proxy = cfg
            .proxy
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(|p| {
                reqwest::Proxy::all(p).map_err(|_| anyhow::anyhow!("адрес прокси не разобран: «{p}». Пример: socks5h://127.0.0.1:1080 или http://user:pass@host:3128"))
            })
            .transpose()?;
        let iface = cfg.interface.as_deref().map(str::trim).filter(|i| !i.is_empty());
        let mut f = match &proxy {
            Some(p) if cfg.proxy_always => Self::build(&cfg, Some(p.clone()), false, None)?,
            Some(p) => {
                let mut f = Self::build(&cfg, None, false, iface)?;
                f.proxied = Some(std::sync::Arc::new(Self::build(&cfg, Some(p.clone()), false, None)?));
                f
            }
            // Напрямую мимо VPN, запасной путь — через туннель (как без привязки).
            None if iface.is_some() => {
                let mut f = Self::build(&cfg, None, false, iface)?;
                f.proxied = Some(std::sync::Arc::new(Self::build(&cfg, None, false, None)?));
                f
            }
            None => Self::build(&cfg, None, false, None)?,
        };
        let lenient_proxy = proxy.filter(|_| cfg.proxy_always);
        let lenient_iface = if lenient_proxy.is_some() { None } else { iface };
        f.lenient = Some(std::sync::Arc::new(Self::build(&cfg, lenient_proxy, true, lenient_iface)?));
        Ok(f)
    }

    fn build(cfg: &FetchConfig, proxy: Option<reqwest::Proxy>, insecure: bool, iface: Option<&str>) -> anyhow::Result<Self> {
        let with_proxy = |b: reqwest::ClientBuilder| {
            let b = match iface {
                Some(i) => b.interface(i),
                None => b,
            };
            match &proxy {
                Some(p) => b.proxy(p.clone()),
                None => b,
            }
        };
        let base = || {
            with_proxy(
                reqwest::Client::builder()
                    .danger_accept_invalid_certs(insecure)
                    .timeout(cfg.timeout)
                    .connect_timeout(Duration::from_secs(8))
                    .pool_max_idle_per_host(4)
                    .tcp_keepalive(Duration::from_secs(30)),
            )
        };
        let site = base().redirect(reqwest::redirect::Policy::none()).build()?;
        let raw = base()
            .redirect(reqwest::redirect::Policy::none())
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .build()?;
        let api = with_proxy(
            reqwest::Client::builder()
                .timeout(Duration::from_secs(90))
                .connect_timeout(Duration::from_secs(10))
                .user_agent("maps-seo-scanner/0.1 (local tool)"),
        )
        .build()?;
        Ok(Self {
            site,
            raw,
            api,
            cfg: cfg.clone(),
            accept_language: "ru-RU,ru;q=0.9,en;q=0.6".into(),
            proxied: None,
            lenient: None,
            proxy_url: proxy.is_some().then(|| cfg.proxy.clone().unwrap_or_default().trim().to_string()),
            interface: iface.map(str::to_string),
        })
    }

    /// Интерфейс, к которому привязаны запросы (мимо VPN), если привязаны.
    pub fn interface(&self) -> Option<&str> {
        self.interface.as_deref()
    }

    /// Копия, которая ходит через свой прокси (с тем же языком), если прокси задан как запасной.
    pub fn proxied(&self) -> Option<Fetcher> {
        self.proxied.as_ref().map(|p| Fetcher { accept_language: self.accept_language.clone(), ..(**p).clone() })
    }

    /// Копия без проверки сертификата (с тем же языком).
    pub fn lenient(&self) -> Option<Fetcher> {
        self.lenient.as_ref().map(|p| Fetcher { accept_language: self.accept_language.clone(), ..(**p).clone() })
    }

    pub fn is_proxied(&self) -> bool {
        self.proxy_url.is_some()
    }

    /// Прокси для headless-браузера (`--proxy-server`): Chromium не принимает логин и пароль
    /// в командной строке, такой прокси для рендера не годится.
    pub fn browser_proxy(&self) -> Option<String> {
        let u = Url::parse(self.proxy_url.as_deref()?).ok()?;
        if !u.username().is_empty() || u.password().is_some() {
            return None;
        }
        let scheme = match u.scheme() {
            "socks5" | "socks5h" => "socks5",
            "socks4" | "socks4a" => "socks4",
            "https" => "https",
            _ => "http",
        };
        Some(format!("{scheme}://{}:{}", u.host_str()?, u.port_or_known_default()?))
    }

    pub fn accept_language(&self) -> &str {
        &self.accept_language
    }

    /// Копия с другим Accept-Language (клиенты общие): для сайтов другой страны.
    pub fn with_language(&self, accept_language: String) -> Self {
        Self { accept_language, ..self.clone() }
    }

    /// Заголовки, как у Chrome при открытии страницы. Для ботов (I05) — только их User-Agent:
    /// проверяем, пускает ли сайт именно бота.
    fn browser_headers(&self, req: reqwest::RequestBuilder, ua: &str) -> reqwest::RequestBuilder {
        let req = req
            .header(USER_AGENT, ua)
            .header(ACCEPT, "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8")
            .header(ACCEPT_LANGUAGE, self.accept_language.as_str());
        if ua != browser_ua() {
            return req;
        }
        let v = chrome_major();
        req.header("sec-ch-ua", format!("\"Chromium\";v=\"{v}\", \"Google Chrome\";v=\"{v}\", \"Not_A Brand\";v=\"99\""))
            .header("sec-ch-ua-mobile", "?0")
            .header("sec-ch-ua-platform", "\"Windows\"")
            .header("upgrade-insecure-requests", "1")
            .header("sec-fetch-site", "none")
            .header("sec-fetch-mode", "navigate")
            .header("sec-fetch-user", "?1")
            .header("sec-fetch-dest", "document")
    }

    /// GET с ручными редиректами (до 10) и чтением тела до лимита.
    pub async fn get(&self, url: &str, ua: Option<&str>) -> Result<Fetched, FetchError> {
        self.get_limited(url, ua, self.cfg.max_body).await
    }

    pub async fn get_limited(&self, url: &str, ua: Option<&str>, max_body: usize) -> Result<Fetched, FetchError> {
        self.get_with(url, ua, max_body, None).await
    }

    /// То же со своим таймаутом: большой sitemap на медленном сервере грузится дольше обычного.
    pub async fn get_with(&self, url: &str, ua: Option<&str>, max_body: usize, timeout: Option<Duration>) -> Result<Fetched, FetchError> {
        let ua = ua.unwrap_or(browser_ua());
        let start = Instant::now();
        let mut current = Url::parse(url).map_err(|_| FetchError::BadUrl(url.to_string()))?;
        let mut chain = Vec::new();
        for _ in 0..10 {
            let t0 = Instant::now();
            let mut req = self.browser_headers(self.site.get(current.clone()), ua);
            if let Some(t) = timeout {
                req = req.timeout(t);
            }
            let resp = req.send().await.map_err(|e| FetchError::Network(describe(&e)))?;
            let ttfb = t0.elapsed();
            let status = resp.status().as_u16();
            if resp.status().is_redirection()
                && let Some(loc) = resp.headers().get(LOCATION).and_then(|v| v.to_str().ok())
                && let Ok(next) = current.join(loc)
            {
                chain.push(Hop { url: current.to_string(), status });
                current = next;
                continue;
            }
            let headers = header_pairs(resp.headers());
            let http_version = format!("{:?}", resp.version());
            let content_type = resp
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_ascii_lowercase();
            let (bytes, truncated) = read_capped(resp, max_body).await?;
            let body = if is_textual(&content_type) || content_type.is_empty() {
                decode_body(&bytes, &content_type)
            } else {
                String::new()
            };
            return Ok(Fetched {
                url: url.to_string(),
                final_url: current.to_string(),
                status,
                chain,
                headers,
                body,
                body_bytes: bytes.len(),
                truncated,
                ttfb_ms: ttfb.as_millis() as u64,
                total_ms: start.elapsed().as_millis() as u64,
                http_version,
                content_type,
            });
        }
        Err(FetchError::TooManyRedirects)
    }

    /// Проверка ссылки: HEAD (или GET, если HEAD не поддержан) с редиректами до 5.
    pub async fn check(&self, url: &str) -> UrlStatus {
        let mut out = UrlStatus {
            url: url.to_string(),
            status: None,
            final_status: None,
            final_url: None,
            redirected: false,
            error: None,
        };
        let Ok(mut current) = Url::parse(url) else {
            out.error = Some("некорректный URL".into());
            return out;
        };
        for hop in 0..6 {
            let mut resp =
                self.browser_headers(self.site.head(current.clone()), browser_ua()).timeout(CHECK_TIMEOUT).send().await;
            if let Ok(r) = &resp
                // 4xx/5xx на HEAD часто ложные (Bitrix отвечает 500 на HEAD и 200 на GET).
                && (r.status().is_client_error() || r.status().is_server_error())
            {
                resp = self.browser_headers(self.site.get(current.clone()), browser_ua())
                    .header(reqwest::header::RANGE, "bytes=0-0")
                    .timeout(CHECK_TIMEOUT)
                    .send()
                    .await;
            }
            match resp {
                Err(e) => {
                    out.error = Some(describe(&e));
                    return out;
                }
                Ok(r) => {
                    let st = r.status().as_u16();
                    if hop == 0 {
                        out.status = Some(st);
                    }
                    if r.status().is_redirection()
                        && let Some(next) = r
                            .headers()
                            .get(LOCATION)
                            .and_then(|v| v.to_str().ok())
                            .and_then(|l| current.join(l).ok())
                    {
                        out.redirected = true;
                        current = next;
                        continue;
                    }
                    out.final_status = Some(st);
                    out.final_url = Some(current.to_string());
                    return out;
                }
            }
        }
        out.error = Some("слишком много редиректов".into());
        out
    }

    /// Заголовки ответа на HEAD (для кэширования статики).
    pub async fn head(&self, url: &str) -> Result<(u16, Vec<(String, String)>), String> {
        let r = self.browser_headers(self.site.head(url), browser_ua()).send().await.map_err(|e| describe(&e))?;
        Ok((r.status().as_u16(), header_pairs(r.headers())))
    }

    /// Какое сжатие отдаёт сервер (br/gzip/zstd/deflate) — без распаковки тела.
    pub async fn content_encoding(&self, url: &str) -> Option<String> {
        let resp = self.browser_headers(self.raw.get(url), browser_ua())
            .header(ACCEPT_ENCODING, "br, gzip, zstd, deflate")
            .send()
            .await
            .ok()?;
        Some(
            resp.headers()
                .get(reqwest::header::CONTENT_ENCODING)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string(),
        )
    }
}

fn header_pairs(h: &HeaderMap) -> Vec<(String, String)> {
    h.iter()
        .map(|(k, v)| (k.as_str().to_ascii_lowercase(), String::from_utf8_lossy(v.as_bytes()).into_owned()))
        .collect()
}

async fn read_capped(mut resp: reqwest::Response, max: usize) -> Result<(Vec<u8>, bool), FetchError> {
    let mut buf = Vec::new();
    loop {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                buf.extend_from_slice(&chunk);
                if buf.len() >= max {
                    buf.truncate(max);
                    return Ok((buf, true));
                }
            }
            Ok(None) => return Ok((buf, false)),
            Err(e) => {
                if buf.is_empty() {
                    return Err(FetchError::Network(describe(&e)));
                }
                return Ok((buf, true));
            }
        }
    }
}

/// Тело — текст. JS («application/javascript») тоже: без него свои скрипты сайта приходили
/// пустыми, и счётчики в бандлах не находились (everestdent.by: GA4 и Метрика в autoptimize_*.js).
fn is_textual(ct: &str) -> bool {
    ct.starts_with("text/") || ["html", "xml", "json", "javascript", "ecmascript"].iter().any(|t| ct.contains(t))
}

/// Перекодировка тела: charset из Content-Type, иначе из <meta charset>, иначе UTF-8.
/// gzip-тело (sitemap.xml.gz) распаковывается.
pub fn decode_body(bytes: &[u8], content_type: &str) -> String {
    let owned;
    let bytes = if bytes.starts_with(&[0x1f, 0x8b]) {
        use std::io::Read;
        let mut out = Vec::new();
        if flate2::read::GzDecoder::new(bytes).take(20 * 1024 * 1024).read_to_end(&mut out).is_ok() {
            owned = out;
            &owned[..]
        } else {
            bytes
        }
    } else {
        bytes
    };
    let label = charset_from_content_type(content_type).or_else(|| sniff_meta_charset(bytes));
    // Кодировка не указана, а тело не UTF-8: на русскоязычных сайтах это windows-1251 — так же
    // угадывает браузер (rdkv.by: meta charset после длинного скрипта счётчика).
    let fallback = if std::str::from_utf8(bytes).is_ok() { encoding_rs::UTF_8 } else { encoding_rs::WINDOWS_1251 };
    let enc = label
        .and_then(|l| encoding_rs::Encoding::for_label(l.as_bytes()))
        .unwrap_or(fallback);
    let (text, _, _) = enc.decode(bytes);
    text.into_owned()
}

fn charset_from_content_type(ct: &str) -> Option<String> {
    ct.split(';')
        .map(str::trim)
        .find_map(|p| p.strip_prefix("charset="))
        .map(|s| s.trim_matches(['"', '\'']).to_string())
}

fn sniff_meta_charset(bytes: &[u8]) -> Option<String> {
    let head = &bytes[..bytes.len().min(32 * 1024)];
    let s = String::from_utf8_lossy(head).to_ascii_lowercase();
    let i = s.find("charset=")?;
    let rest = &s[i + 8..];
    let rest = rest.trim_start_matches(['"', '\'']);
    let end = rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_')).unwrap_or(rest.len());
    let label = &rest[..end];
    (!label.is_empty()).then(|| label.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_modes() {
        let cfg = |p: &str, always: bool| FetchConfig { proxy: Some(p.into()), proxy_always: always, ..Default::default() };
        let f = Fetcher::new(cfg("socks5h://127.0.0.1:1080", false)).unwrap();
        assert!(!f.is_proxied(), "запасной режим: напрямую по умолчанию");
        let p = f.with_language("de-DE".into()).proxied().unwrap();
        assert!(p.is_proxied());
        assert_eq!(p.accept_language(), "de-DE", "язык переносится в копию через прокси");
        assert_eq!(p.browser_proxy().as_deref(), Some("socks5://127.0.0.1:1080"));
        let always = Fetcher::new(cfg("http://user:pw@10.0.0.2:3128", true)).unwrap();
        assert!(always.is_proxied() && always.proxied().is_none());
        assert_eq!(always.browser_proxy(), None, "Chromium не берёт логин и пароль из командной строки");
        assert!(Fetcher::new(cfg("not a url ::", false)).is_err());
        assert!(!Fetcher::new(FetchConfig::default()).unwrap().is_proxied());
    }

    #[test]
    fn non_utf8_without_charset_is_windows_1251() {
        // Круг 5: rdkv.by — meta charset далеко за длинным скриптом, заголовок без charset.
        let (bytes, _, _) = encoding_rs::WINDOWS_1251.encode("<html><body>Районный дом культуры</body></html>");
        assert!(decode_body(&bytes, "text/html").contains("Районный дом культуры"));
    }

    #[test]
    fn decodes_windows_1251_from_meta() {
        let (bytes, _, _) = encoding_rs::WINDOWS_1251.encode("<meta charset=\"windows-1251\"><p>Привет</p>");
        let s = decode_body(&bytes, "text/html");
        assert!(s.contains("Привет"));
    }

    #[test]
    fn decodes_charset_from_header() {
        let (bytes, _, _) = encoding_rs::WINDOWS_1251.encode("Стоматология");
        assert_eq!(decode_body(&bytes, "text/html; charset=windows-1251"), "Стоматология");
    }

    #[test]
    fn utf8_default() {
        assert_eq!(decode_body("Тест".as_bytes(), "text/html"), "Тест");
    }
}
