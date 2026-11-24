//! Поиск сайта компании в обход карт. Источники кандидатов (от надёжных к слабым):
//! почта из карточки на своём домене, Wikidata бренда сети, ссылки со страниц
//! соцсетей и мультиссылок (Taplink, Linktree, Telegram), домены, угаданные по названию,
//! веб-поиск. Каждый кандидат проверяется по главной (и странице контактов): совпал
//! телефон или адрес из карточки — сайт подтверждён; совпали только название и город —
//! это предположение, его подтверждает человек.
//!
//! Только пассивно: DNS-запросы и обычные GET главных страниц существующих доменов,
//! без перебора путей.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::http::Fetcher;
use crate::model::{Business, ScanParams};
use crate::page::{self, PageInfo};
use crate::search::WebSearch;
use crate::text::{classify_website, host_of, normalize_name, parse_url_lenient, phone_key, phones_in_text, registrable_domain, similarity, SiteKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// Домен почты из карточки.
    Email,
    /// Свой домен среди дополнительных ссылок карточки.
    CardLink,
    /// Официальный сайт бренда в Wikidata.
    Wikidata,
    /// Ссылка со страницы соцсети, мессенджера или мультиссылки из карточки.
    SocialLink,
    /// Домен, угаданный по названию.
    DomainGuess,
    /// Выдача поисковика.
    WebSearch,
    /// Указан вручную.
    Manual,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Email => "почта из карточки",
            Source::CardLink => "ссылка из карточки",
            Source::Wikidata => "Wikidata бренда",
            Source::SocialLink => "ссылка из соцсети",
            Source::DomainGuess => "домен по названию",
            Source::WebSearch => "поисковик",
            Source::Manual => "указан вручную",
        }
    }
    /// Насколько сам источник связывает сайт с компанией.
    fn prior(self) -> f32 {
        match self {
            Source::Email | Source::CardLink => 0.5,
            Source::Wikidata => 0.7,
            Source::SocialLink => 0.45,
            Source::Manual => 1.0,
            Source::DomainGuess | Source::WebSearch => 0.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub url: String,
    pub source: Source,
    /// Откуда взят: адрес почты, ссылка на профиль, запрос.
    pub via: String,
}

/// Проверенный кандидат.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Verdict {
    pub url: String,
    pub domain: String,
    pub source: Source,
    pub via: String,
    pub score: f32,
    pub confirmed: bool,
    pub evidence: Vec<String>,
    pub title: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Discovery {
    /// Подтверждённый сайт (лучший).
    pub found: Option<Verdict>,
    /// Возможные сайты: совпали название или город, но не телефон и не адрес.
    pub suggestions: Vec<Verdict>,
    /// Сколько доменов угадано и сколько из них существует.
    pub guessed: usize,
    pub resolved: usize,
    /// Сколько кандидатов открыто и проверено.
    pub checked: usize,
    /// Что ответил веб-поиск (или почему не ответил).
    pub search: Option<String>,
    /// Бренд сети, если компания — точка сети.
    pub brand: Option<String>,
    pub elapsed_ms: u64,
}

pub struct DiscoverOptions<'a> {
    pub guess: bool,
    pub search: Option<&'a WebSearch>,
    /// Сколько угаданных доменов открывать максимум.
    pub max_guess_checks: usize,
}

impl Default for DiscoverOptions<'_> {
    fn default() -> Self {
        Self { guess: true, search: None, max_guess_checks: 6 }
    }
}

const CONFIRM: f32 = 0.7;
const SUGGEST: f32 = 0.35;

pub async fn discover(f: &Fetcher, b: &Business, params: &ScanParams, opt: &DiscoverOptions<'_>) -> Discovery {
    let t0 = Instant::now();
    let mut d = Discovery { brand: b.brand().map(str::to_string), ..Default::default() };
    let mut seen: HashSet<String> = HashSet::new();
    let mut verdicts: Vec<Verdict> = Vec::new();

    // 1. Надёжные источники: почта, свои ссылки, Wikidata, страницы соцсетей.
    let mut strong: Vec<Candidate> = Vec::new();
    for e in b.emails() {
        if let Some(domain) = e.rsplit('@').next().map(|x| x.trim().to_lowercase())
            && domain.contains('.')
            && !crate::lexicon::is_free_email(&domain)
        {
            strong.push(Candidate { url: format!("https://{domain}/"), source: Source::Email, via: e.to_string() });
        }
    }
    for link in b.all_links() {
        match classify_website(link).0 {
            SiteKind::Own | SiteKind::ConstructorSubdomain if b.website.as_deref() != Some(link) => {
                strong.push(Candidate { url: link.to_string(), source: Source::CardLink, via: link.to_string() })
            }
            SiteKind::LinkInBio | SiteKind::Messenger | SiteKind::Social | SiteKind::Booking => {
                for url in links_from_profile(f, link).await {
                    strong.push(Candidate { url, source: Source::SocialLink, via: link.to_string() });
                }
            }
            _ => {}
        }
    }
    if let Some(q) = b.wikidata()
        && let Some(url) = wikidata_website(&f.api, q).await
    {
        strong.push(Candidate { url, source: Source::Wikidata, via: q.to_string() });
    }
    for c in strong {
        if !seen.insert(cand_key(&c.url)) {
            continue;
        }
        d.checked += 1;
        if let Some(v) = verify(f, &c, b, params).await {
            let done = v.confirmed;
            verdicts.push(v);
            if done {
                break;
            }
        }
    }

    // 2. Домены по названию.
    if opt.guess && !verdicts.iter().any(|v| v.confirmed) {
        let suffixes = params.preset.as_deref().and_then(crate::providers::presets::find).map(|p| p.domain_suffixes).unwrap_or(&[]);
        let names = guess_domains(&b.name, params.country.as_deref(), suffixes);
        d.guessed = names.len();
        let live = resolve_live(&f.api, names).await;
        d.resolved = live.len();
        let cands: Vec<Candidate> = live
            .into_iter()
            .filter(|n| seen.insert(cand_key(n)))
            .take(opt.max_guess_checks)
            .map(|n| Candidate { url: format!("https://{n}/"), source: Source::DomainGuess, via: n })
            .collect();
        d.checked += cands.len();
        let found = futures::future::join_all(cands.iter().map(|c| verify(f, c, b, params))).await;
        verdicts.extend(found.into_iter().flatten());
    }

    // 3. Веб-поиск.
    if let Some(ws) = opt.search
        && !verdicts.iter().any(|v| v.confirmed)
    {
        let city = params.city_variants().into_iter().next().unwrap_or_default();
        let q = format!("\"{}\" {}", b.name.trim(), city).trim().to_string();
        match ws.search(&f.api, &q, params.country.as_deref()).await {
            Ok(hits) => {
                d.search = Some(format!("{}: {} результатов", hits.first().map(|h| h.engine).unwrap_or("поиск"), hits.len()));
                let cands: Vec<Candidate> = hits
                    .iter()
                    .filter(|h| classify_website(&h.url).0.is_auditable())
                    .filter(|h| seen.insert(cand_key(&h.url)))
                    .take(4)
                    .map(|h| Candidate { url: h.url.clone(), source: Source::WebSearch, via: q.clone() })
                    .collect();
                d.checked += cands.len();
                let found = futures::future::join_all(cands.iter().map(|c| verify(f, c, b, params))).await;
                verdicts.extend(found.into_iter().flatten());
            }
            Err(e) => d.search = Some(e),
        }
    }

    verdicts.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    let mut domains = HashSet::new();
    verdicts.retain(|v| domains.insert(v.domain.clone()));
    d.found = verdicts.iter().find(|v| v.confirmed).cloned();
    d.suggestions = verdicts.into_iter().filter(|v| !v.confirmed && v.score >= SUGGEST).take(3).collect();
    d.elapsed_ms = t0.elapsed().as_millis() as u64;
    d
}

fn cand_key(url: &str) -> String {
    parse_url_lenient(url).map(|u| registrable_domain(&host_of(&u))).unwrap_or_else(|| url.to_string())
}

// ---------- Проверка кандидата ----------

const CONTACT_SLUGS: &[&str] = &["contact", "kontakt", "контакт", "contacto", "contatti", "impressum", "about", "o-nas", "location", "find-us", "visit"];

/// Открыть кандидата и собрать доказательства связи с компанией.
pub async fn verify(f: &Fetcher, c: &Candidate, b: &Business, params: &ScanParams) -> Option<Verdict> {
    // Напрямую не открылся или отказал (часто — география выхода VPN): пробуем свой прокси.
    let direct = f.get(&c.url, None).await;
    let refused = direct.as_ref().map_or(true, |h| matches!(h.status, 403 | 451 | 429));
    let proxied = if refused { f.proxied() } else { None };
    let (f, home) = match proxied {
        Some(p) => match p.get(&c.url, None).await {
            Ok(h) if h.ok() => (std::borrow::Cow::Owned(p), h),
            _ => (std::borrow::Cow::Borrowed(f), direct.ok()?),
        },
        None => (std::borrow::Cow::Borrowed(f), direct.ok()?),
    };
    let f: &Fetcher = &f;
    let final_url = parse_url_lenient(&home.final_url)?;
    let (kind, host) = classify_website(final_url.as_str());
    if !kind.is_auditable() {
        return None;
    }
    // Домен почты ведёт на чужой сайт (ramler.ru → aviasales.ru): это не сайт компании.
    if c.source == Source::Email
        && let Some(start) = parse_url_lenient(&c.url)
        && registrable_domain(start.host_str().unwrap_or("")) != registrable_domain(&host)
    {
        return None;
    }
    let blocked = crate::site::is_blocked(&home);
    if !home.ok() && !blocked {
        return None;
    }
    let info = page::extract(&home.body, &home.final_url);
    let text = info.text_lower();
    if crate::lexicon::PARKED.iter().any(|m| text.contains(m)) || (info.words < 120 && crate::lexicon::PLACEHOLDER.iter().any(|m| text.contains(m))) {
        return None;
    }
    let mut ev = Evidence::new(c.source);
    if blocked {
        ev.add(0.0, "сайт закрыт защитой от ботов — проверить вручную");
    } else {
        ev.check_page(&info, &text, &home.body, b, params);
        if ev.score < CONFIRM && (!ev.phone || !ev.address) {
            let contact = info
                .links
                .iter()
                .filter(|l| l.internal)
                .find(|l| CONTACT_SLUGS.iter().any(|s| l.href.to_lowercase().contains(s) || l.text.to_lowercase().contains(s)))
                .map(|l| l.href.clone());
            if let Some(url) = contact
                && let Ok(p) = f.get(&url, None).await
                && p.ok()
            {
                let ci = page::extract(&p.body, &p.final_url);
                let ct = ci.text_lower();
                ev.check_contacts(&ci, &ct, &p.body, b);
            }
        }
    }
    let score = (ev.score * 100.0).round() / 100.0;
    if score < SUGGEST && c.source.prior() == 0.0 {
        return None;
    }
    Some(Verdict {
        url: home.final_url.clone(),
        domain: registrable_domain(&host),
        source: c.source,
        via: c.via.clone(),
        score,
        confirmed: score >= CONFIRM,
        evidence: ev.notes,
        title: info.title.clone().or_else(|| info.og("og:site_name").map(str::to_string)),
    })
}

struct Evidence {
    score: f32,
    notes: Vec<String>,
    phone: bool,
    address: bool,
}

impl Evidence {
    fn new(source: Source) -> Self {
        let mut e = Self { score: 0.0, notes: vec![], phone: false, address: false };
        if source.prior() > 0.0 {
            e.add(source.prior(), &format!("источник: {}", source.label()));
        }
        e
    }

    fn add(&mut self, w: f32, note: &str) {
        self.score += w;
        self.notes.push(note.to_string());
    }

    fn check_contacts(&mut self, info: &PageInfo, text: &str, html: &str, b: &Business) {
        if !self.phone
            && let Some(k) = b.phone.as_deref().and_then(phone_key)
        {
            let keys: HashSet<String> = phones_in_text(&info.text)
                .into_iter()
                .chain(info.tel_links.iter().filter_map(|t| phone_key(t)))
                .chain(raw_phone_keys(html))
                .collect();
            if keys.contains(&k) {
                self.phone = true;
                self.add(0.7, &format!("телефон из карточки ({}) есть на сайте", b.phone.as_deref().unwrap_or("")));
            }
        }
        if !self.address
            && let Some(a) = b.address.as_deref()
            && crate::checks::address_matches(a, text)
        {
            self.address = true;
            self.add(0.45, "адрес из карточки есть на сайте");
        }
    }

    fn check_page(&mut self, info: &PageInfo, text: &str, html: &str, b: &Business, params: &ScanParams) {
        self.check_contacts(info, text, html, b);
        let name = normalize_name(&b.name);
        let heads: Vec<String> = info
            .title
            .iter()
            .cloned()
            .chain(info.og("og:site_name").map(str::to_string))
            .chain(info.h1s().into_iter().map(str::to_string))
            .map(|h| normalize_name(&h))
            .collect();
        let tokens: Vec<&str> = name.split_whitespace().filter(|t| t.chars().count() >= 3 && !GENERIC.contains(t)).collect();
        let head_all = heads.join(" ");
        // Одно отличительное слово («Fulton») слишком слабое: нужен весь набор слов или сходство названия.
        let name_hit = heads.iter().any(|h| similarity(h, &name) >= 0.65 || h.replace(' ', "").contains(&name.replace(' ', "")))
            || (tokens.len() >= 2 && tokens.iter().all(|t| head_all.contains(t)));
        if name_hit {
            self.add(0.3, "название совпадает с заголовком сайта");
        } else if !tokens.is_empty() && tokens.iter().all(|t| text.contains(t)) {
            self.add(0.15, "название упоминается на сайте");
        }
        let city = params.city_variants().iter().map(|c| c.to_lowercase()).find(|c| c.chars().count() >= 3 && text.contains(c.as_str()));
        if let Some(c) = city {
            self.add(0.15, &format!("на сайте упомянут город «{c}»"));
        }
        // Ссылка с сайта на профиль компании из карточки.
        let profiles: Vec<String> = b
            .all_links()
            .iter()
            .filter(|l| !classify_website(l).0.is_auditable())
            .filter_map(|l| parse_url_lenient(l))
            .map(|u| format!("{}{}", host_of(&u).trim_start_matches("www."), u.path().trim_end_matches('/')).to_lowercase())
            .filter(|p| p.contains('/'))
            .collect();
        if !profiles.is_empty() && info.links.iter().any(|l| profiles.iter().any(|p| l.href.to_lowercase().contains(p.as_str()))) {
            self.add(0.6, "сайт ссылается на профиль компании из карточки");
        }
    }
}

/// Телефоны в сыром HTML, включая JSON и скрипты (конструкторы вроде Wix кладут номер
/// только туда). Совпадение последних 9 цифр с карточкой случайно не бывает.
pub fn raw_phone_keys(html: &str) -> Vec<String> {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"\+?\d[\d\s\-().\u{a0}]{7,20}\d").unwrap());
    re.find_iter(html).filter_map(|m| phone_key(m.as_str())).collect()
}

/// Общие слова названий, по которым нельзя узнать компанию.
const GENERIC: &[&str] = &[
    "стоматология", "стоматологическая", "стоматологический", "клиника", "клиники", "салон", "красоты", "студия", "центр",
    "магазин", "мастерская", "парикмахерская", "барбершоп", "кафе", "ресторан", "бар", "медицинский", "центра", "сервис",
    "автосервис", "dental", "dentist", "dentistry", "clinic", "salon", "studio", "center", "centre", "shop", "store",
    "barber", "barbershop", "barbers", "hair", "beauty", "cafe", "restaurant", "the", "and", "praxis", "zahnarztpraxis",
    "zahnarzt", "für", "und", "der", "die", "das", "med", "spa", "nails", "nail", "bar", "grill", "pizza", "kitchen",
    "services", "service", "group", "company", "llc", "inc", "ltd", "gmbh", "unisex", "professional", "african",
    "braiding", "style", "styles", "cuts", "house", "home", "auto", "repair", "car", "new", "york", "brooklyn",
];

// ---------- Ссылки из профилей соцсетей ----------

const NOISE_HOSTS: &[&str] = &[
    "gstatic.com", "googleapis.com", "google.com", "cloudflare.com", "cdnjs.com", "jsdelivr.net", "unpkg.com", "sentry.io",
    "google-analytics.com", "googletagmanager.com", "facebook.net", "schema.org", "w3.org", "apple.com", "youtube.com",
    "ytimg.com", "vk.com", "vk.ru", "userapi.com", "vkuser.net", "mycdn.me", "okcdn.ru", "t.me", "telegram.org",
    "telegram.me", "cdn-telegram.org", "taplink.cc", "taplink.ws", "taplink.at", "linktr.ee", "hipolink.me",
    "instagram.com", "cdninstagram.com", "fbcdn.net", "facebook.com", "tiktok.com", "tiktokcdn.com", "x.com",
    "twitter.com", "twimg.com", "yandex.ru", "yandex.net", "yastatic.net", "mail.ru", "linkedin.com", "licdn.com",
    "wa.me", "whatsapp.com", "play.google.com", "itunes.apple.com", "apps.apple.com", "taplink.ru", "mc.yandex.ru",
    "sentry-cdn.com", "amazonaws.com", "cloudfront.net", "akamaihd.net", "bit.ly", "goo.gl", "linkin.bio",
    "beacons.ai", "fonts.net", "typekit.net", "jquery.com", "recaptcha.net", "hcaptcha.com", "intercom.io",
    "hotjar.com", "stripe.com", "paypal.com", "shopify.com", "imgix.net", "wixstatic.com", "parastorage.com",
];

/// Свои сайты, на которые ссылается страница профиля (Taplink, Linktree, Telegram, VK…).
/// Ссылки ищутся во всём HTML: у мультиссылок они часто лежат в JSON внутри страницы.
async fn links_from_profile(f: &Fetcher, profile: &str) -> Vec<String> {
    let Some(u) = parse_url_lenient(profile) else { return vec![] };
    let host = host_of(&u);
    // Instagram, Facebook и TikTok без входа не показывают профиль.
    if ["instagram.com", "facebook.com", "fb.com", "tiktok.com"].iter().any(|h| host == *h || host.ends_with(&format!(".{h}"))) {
        return vec![];
    }
    let Ok(resp) = f.get_limited(u.as_str(), None, 2_000_000).await else { return vec![] };
    if !resp.ok() {
        return vec![];
    }
    urls_in_html(&resp.body, &registrable_domain(&host))
}

pub fn urls_in_html(html: &str, own: &str) -> Vec<String> {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r#"https?:(?:\\?/){2}[a-zA-Z0-9\-._~%]+(?:\.[a-zA-Z0-9\-]+)+(?:[^\s"'<>\\]*)?"#).unwrap());
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for m in re.find_iter(html) {
        let raw = m.as_str().replace("\\/", "/");
        let lower = raw.to_lowercase();
        if [".js", ".css", ".png", ".jpg", ".jpeg", ".svg", ".webp", ".gif", ".woff", ".woff2", ".ico", ".json", ".mp4"].iter().any(|e| lower.split('?').next().unwrap_or("").ends_with(e)) {
            continue;
        }
        let Some(u) = parse_url_lenient(&raw) else { continue };
        let h = host_of(&u);
        let reg = registrable_domain(&h);
        if reg == own || NOISE_HOSTS.iter().any(|n| reg == *n || h.ends_with(&format!(".{n}"))) || !classify_website(&raw).0.is_auditable() {
            continue;
        }
        if seen.insert(reg) {
            out.push(format!("{}://{}/", u.scheme(), h));
        }
        if out.len() >= 5 {
            break;
        }
    }
    out
}

// ---------- Wikidata ----------

/// Официальный сайт (P856) сущности Wikidata — для брендов сетей.
async fn wikidata_website(api: &reqwest::Client, qid: &str) -> Option<String> {
    let qid = qid.split(';').next()?.trim();
    if !qid.starts_with('Q') || !qid[1..].chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let v: Value = api
        .get(format!("https://www.wikidata.org/wiki/Special:EntityData/{qid}.json"))
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    v["entities"][qid]["claims"]["P856"].as_array()?.iter().find_map(|c| c["mainsnak"]["datavalue"]["value"].as_str()).map(str::to_string)
}

// ---------- Угадывание доменов ----------

/// Существующие домены из списка (по 8 запросов одновременно, порядок сохраняется).
async fn resolve_live(api: &reqwest::Client, names: Vec<String>) -> Vec<String> {
    let mut live = Vec::new();
    for chunk in names.chunks(8) {
        let ok = futures::future::join_all(chunk.iter().map(|n| resolves(api, n))).await;
        live.extend(chunk.iter().zip(ok).filter(|(_, ok)| *ok).map(|(n, _)| n.clone()));
    }
    live
}

/// Существует ли домен (A-запись через DNS-over-HTTPS Cloudflare).
async fn resolves(api: &reqwest::Client, name: &str) -> bool {
    let ascii = url::Url::parse(&format!("https://{name}/")).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_else(|| name.to_string());
    let Ok(resp) = api
        .get("https://cloudflare-dns.com/dns-query")
        .header("accept", "application/dns-json")
        .query(&[("name", ascii.as_str()), ("type", "A")])
        .timeout(Duration::from_secs(5))
        .send()
        .await
    else {
        return false;
    };
    let Ok(v) = resp.json::<Value>().await else { return false };
    v["Status"].as_i64() == Some(0) && v["Answer"].as_array().is_some_and(|a| !a.is_empty())
}

/// Название города латиницей для сравнения с поддоменом: «Минск», «Minsk» → «minsk».
pub fn translit_city(s: &str) -> String {
    translit(&s.to_lowercase().replace('і', "и").replace('ў', "у"), false).chars().filter(|c| c.is_ascii_alphanumeric()).collect()
}

fn translit(s: &str, alt: bool) -> String {
    let mut out = String::new();
    for c in s.chars() {
        let t = match c {
            'а' => "a", 'б' => "b", 'в' => "v", 'г' => "g", 'д' => "d", 'е' | 'ё' | 'э' => "e", 'ж' => "zh", 'з' => "z",
            'и' => "i", 'й' => if alt { "i" } else { "y" }, 'к' => "k", 'л' => "l", 'м' => "m", 'н' => "n", 'о' => "o",
            'п' => "p", 'р' => "r", 'с' => "s", 'т' => "t", 'у' => "u", 'ф' => "f", 'х' => if alt { "h" } else { "kh" },
            'ц' => if alt { "c" } else { "ts" }, 'ч' => "ch", 'ш' => "sh", 'щ' => if alt { "sch" } else { "shch" },
            'ъ' | 'ь' => "", 'ы' => "y", 'ю' => "yu", 'я' => "ya",
            'ä' => if alt { "a" } else { "ae" }, 'ö' => if alt { "o" } else { "oe" }, 'ü' => if alt { "u" } else { "ue" },
            'ß' => "ss", 'é' | 'è' | 'ê' | 'ë' => "e", 'à' | 'á' | 'â' | 'ã' | 'å' => "a", 'ç' => "c", 'í' | 'ì' | 'î' | 'ï' => "i",
            'ó' | 'ò' | 'ô' | 'õ' | 'ø' => "o", 'ú' | 'ù' | 'û' => "u", 'ñ' => "n", 'ł' => "l", 'ś' => "s", 'ż' | 'ź' => "z",
            'ć' => "c", 'ń' => "n", 'ę' => "e", 'ą' => "a", 'č' => "c", 'š' => "s", 'ž' => "z", 'ř' => "r", 'ý' => "y",
            c if c.is_ascii_alphanumeric() => {
                out.push(c);
                continue;
            }
            _ => " ",
        };
        out.push_str(t);
    }
    out
}

/// Варианты доменов по названию: «Gigi Glam Box» → gigiglambox.com, gigi-glam-box.com…
/// Общие слова ниши отбрасываются («Стоматология Улыбка» → ulybka.ru, ulybka-dent.ru).
pub fn guess_domains(name: &str, country: Option<&str>, suffixes: &[&str]) -> Vec<String> {
    let lower = name.to_lowercase().replace(['\'', '’', '`', '´'], "").replace('&', " ");
    let base = normalize_name(&lower);
    let cyr = base.chars().any(|c| ('а'..='я').contains(&c) || c == 'ё');
    let mut labels: Vec<String> = Vec::new();
    let mut push = |l: String| {
        let l = l.trim_matches('-').to_string();
        if (4..=40).contains(&l.chars().count()) && !labels.contains(&l) {
            labels.push(l);
        }
    };
    for alt in [false, true] {
        let latin = translit(&base, alt);
        let tokens: Vec<&str> = latin.split_whitespace().collect();
        if tokens.is_empty() {
            continue;
        }
        let core: Vec<&str> = tokens.iter().copied().filter(|t| !GENERIC.contains(t) && !GENERIC_LATIN.contains(t)).collect();
        push(tokens.concat());
        if tokens.len() > 1 {
            push(tokens.join("-"));
        }
        if !core.is_empty() && core.len() < tokens.len() {
            push(core.concat());
            if core.len() > 1 {
                push(core.join("-"));
            }
            for s in suffixes.iter().take(2) {
                push(format!("{}{s}", core.concat()));
                push(format!("{}-{s}", core.concat()));
            }
        }
        if !cyr && base.is_ascii() {
            break;
        }
    }
    labels.truncate(7);
    let tlds = crate::region::guess_tlds(country);
    let mut out = Vec::new();
    for l in &labels {
        for t in &tlds {
            if t == "рф" {
                continue;
            }
            out.push(format!("{l}.{t}"));
        }
    }
    if cyr && tlds.iter().any(|t| t == "рф") {
        let words: Vec<&str> = base.split_whitespace().collect();
        let core: Vec<&str> = words.iter().copied().filter(|w| !GENERIC.contains(w)).collect();
        for v in [words.concat(), core.concat(), core.join("-")] {
            if v.chars().count() >= 3 && !out.contains(&format!("{v}.рф")) {
                out.push(format!("{v}.рф"));
            }
        }
    }
    out.truncate(18);
    out
}

/// Латинские формы общих слов из русских названий.
const GENERIC_LATIN: &[&str] = &[
    "stomatologiya", "stomatologicheskaya", "klinika", "salon", "krasoty", "studiya", "tsentr", "centr", "magazin",
    "parikmakherskaya", "parikmaherskaya", "barbershop", "kafe", "restoran", "ooo", "ip",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guesses_domains() {
        let g = guess_domains("GiGi Glam Box", Some("us"), &["salon", "hair"]);
        assert_eq!(g[0], "gigiglambox.com");
        assert!(g.contains(&"gigi-glam-box.com".to_string()));
        let g = guess_domains("Benny's Barber Shop", Some("us"), &["salon"]);
        assert!(g.contains(&"bennysbarbershop.com".to_string()));
        assert!(g.contains(&"bennys.com".to_string()));
        let g = guess_domains("Стоматология «Улыбка»", Some("ru"), &["dent", "dental"]);
        assert!(g.contains(&"ulybka.ru".to_string()), "{g:?}");
        assert!(g.contains(&"ulybka-dent.ru".to_string()), "{g:?}");
        assert!(g.contains(&"улыбка.рф".to_string()), "{g:?}");
        let g = guess_domains("Zahnarztpraxis Dr. Müller", Some("de"), &["dent"]);
        assert!(g.iter().any(|d| d.starts_with("mueller") || d.starts_with("drmueller")), "{g:?}");
        assert!(g.len() <= 18);
    }

    #[test]
    fn finds_own_links_in_profile_html() {
        let html = r#"<script>{"links":[{"url":"https:\/\/ulybka-dent.ru\/"},{"url":"https://instagram.com/x"},{"url":"https://cdn.jsdelivr.net/a.js"}]}</script>
            <a href="https://taplink.cc/other">x</a><a href="https://wa.me/7900">w</a><a href="https://booking.ulybka-dent.ru/form">b</a>"#;
        assert_eq!(urls_in_html(html, "taplink.cc"), vec!["https://ulybka-dent.ru/".to_string()]);
    }
}
