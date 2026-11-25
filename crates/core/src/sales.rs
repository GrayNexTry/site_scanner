//! Работа менеджера с лидом: контакты компании, план работ с ценой (КП) и текст первого сообщения.
//! Чистые функции без сети; сервер только достаёт данные и отдаёт результат интерфейсу.

use std::collections::HashSet;
use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

use crate::catalog::catalog;
use crate::mini::{self, SiteState};
use crate::model::{Business, MetricResult, Status};
use crate::page::PageInfo;
use crate::site::SiteSnapshot;
use crate::text::{classify_website, host_of, parse_url_lenient, phone_key, phone_matches, SiteKind};

// ---------------------------------------------------------------------------------------------
// Контакты
// ---------------------------------------------------------------------------------------------

/// Ссылка на соцсеть или мессенджер, найденная на сайте.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SiteLink {
    /// instagram, telegram, whatsapp, viber, vk, facebook, tiktok, youtube, ok, linkedin, x, max, taplink…
    pub kind: String,
    pub url: String,
}

/// Контакты, найденные на обойдённых страницах сайта (кладутся в сводку сайта).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Contacts {
    pub emails: Vec<String>,
    /// В том виде, как написаны на сайте.
    pub phones: Vec<String>,
    pub links: Vec<SiteLink>,
}

const MAX_EMAILS: usize = 8;
const MAX_PHONES: usize = 8;
const MAX_LINKS: usize = 20;

fn email_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[A-Za-z0-9._%+\-]+@[A-Za-z0-9\-]+(?:\.[A-Za-z0-9\-]+)*\.[A-Za-z]{2,24}").unwrap())
}

/// Почта похожа на настоящую: не адрес картинки (`logo@2x.png`), не заглушка и не служебный адрес сборщиков.
fn good_email(e: &str) -> bool {
    let l = e.to_lowercase();
    let Some((local, domain)) = l.rsplit_once('@') else { return false };
    const FILE_EXT: &[&str] = &[".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg", ".ico", ".css", ".js", ".woff", ".woff2", ".avif"];
    const BAD_DOMAIN: &[&str] =
        &["example.", "sentry", "wixpress", "yourdomain", "your-domain", "domain.com", "email.com", "site.com", "schema.org", "w3.org", "localhost", "godaddy.com"];
    const BAD_LOCAL: &[&str] = &["your", "name", "user", "username", "youremail", "test", "email"];
    !local.is_empty()
        && !l.contains("@2x")
        && !l.contains("@3x")
        && !FILE_EXT.iter().any(|x| l.ends_with(x))
        && !BAD_DOMAIN.iter().any(|d| domain.contains(d))
        && !(BAD_LOCAL.contains(&local) && domain.starts_with("mail."))
}

/// Дата, принятая за телефон: «07.03.2026 0» → «070320260».
fn is_date_like(raw: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?:^|\D)(?:\d{1,2}[./]\d{1,2}[./](?:19|20)\d{2}|(?:19|20)\d{2}-\d{2}-\d{2})(?:\D|$)").unwrap()).is_match(raw)
}

/// Подпись «+375 29 123-45-67» из текста страницы: неразрывные пробелы и края — по-человечески.
fn tidy_phone(raw: &str) -> String {
    let joined = raw.replace('\u{a0}', " ").split_whitespace().collect::<Vec<_>>().join(" ");
    let mut t = joined.trim_matches(|c: char| !c.is_ascii_digit() && c != '+' && c != '(' && c != ')').to_string();
    // Скобки не обрезаем наполовину: «(017) 276 74 26», а не «017) 276 74 26».
    let (open, close) = (t.matches('(').count(), t.matches(')').count());
    if open > close && t.starts_with('(') && !t.contains(')') || open > close && t.ends_with('(') {
        t = t.trim_matches(|c: char| !c.is_ascii_digit() && c != '+').to_string();
    } else if close > open {
        t = t.trim_start_matches(')').to_string();
        if t.matches(')').count() > t.matches('(').count() {
            t = t.replace(')', "");
        }
    }
    t.trim().to_string()
}

/// Телефон для показа менеджеру и клиенту: запись с сайта целиком, если она аккуратная
/// (скобки парные); обрезанную или сломанную показываем в международном виде «+375 17 276-74-26».
pub fn display_phone(raw: &str, country: Option<&str>) -> String {
    let t = raw.trim();
    let balanced = t.matches('(').count() == t.matches(')').count();
    let clean_edges = t.chars().next().is_some_and(|c| c.is_ascii_digit() || c == '+' || c == '(') && t.chars().last().is_some_and(|c| c.is_ascii_digit());
    let first_paren_ok = t.find('(').is_none_or(|o| t.find(')').is_none_or(|c| o < c));
    // «+749 932 28 515»: первая группа не совпадает с кодом страны — пересобираем по длине кода.
    let code_ok = !t.starts_with('+') || {
        let first = t.trim_start_matches('+').split(|c: char| !c.is_ascii_digit()).next().unwrap_or("");
        first.len() == crate::text::calling_code_len(&digits(t))
    };
    if balanced && clean_edges && first_paren_ok && !t.contains("  ") && code_ok {
        return t.to_string();
    }
    let intl = international_phone(t, country);
    group_international(&intl).unwrap_or(intl)
}

/// Первый телефон из списка контактов — в единой международной записи («+375 29 637-12-26») для шапок документов.
/// Телефон, введённый менеджером, не трогаем. Без кода страны и вне известной разбивки — как в списке контактов.
pub fn client_phone(list: &[Contact], country: Option<&str>) -> Option<String> {
    let c = list.iter().find(|c| c.kind == "phone")?;
    Some(if c.source == "manual" { c.value.clone() } else { phone_grouped(&c.value, country) })
}

/// Телефон в единой международной записи; не укладывается в известную разбивку — без изменений.
pub fn phone_grouped(raw: &str, country: Option<&str>) -> String {
    group_international(&international_phone(raw, country)).unwrap_or_else(|| raw.to_string())
}

/// «+375172767426» → «+375 17 276-74-26», «+74993228515» → «+7 499 322-85-15»; для прочих кодов — «+код номер»
/// (код — по `text::calling_code_len`), если номер не укладывается в известную разбивку — `None`.
fn group_international(intl: &str) -> Option<String> {
    let d = intl.strip_prefix('+')?;
    if let Some(rest) = d.strip_prefix("375").filter(|r| r.len() == 9) {
        return Some(format!("+375 {} {}-{}-{}", &rest[..2], &rest[2..5], &rest[5..7], &rest[7..]));
    }
    if let Some(rest) = d.strip_prefix('7').filter(|r| r.len() == 10) {
        return Some(format!("+7 {} {}-{}-{}", &rest[..3], &rest[3..6], &rest[6..8], &rest[8..]));
    }
    let cc = crate::text::calling_code_len(d);
    (d.len() > cc && d.chars().all(|c| c.is_ascii_digit())).then(|| format!("+{} {}", &d[..cc], &d[cc..]))
}

/// Длина номера правдоподобна для страны по коду: +375 — 12 цифр, +7 — 11, +44 — 11–13, +1 — 11, иначе 8–15.
/// Без «+» код страны неизвестен — годится любой номер из 7–15 цифр.
fn phone_plausible(intl: &str) -> bool {
    let Some(d) = intl.strip_prefix('+') else { return (7..=15).contains(&digits(intl).len()) };
    let n = d.len();
    if d.starts_with("375") {
        n == 12
    } else if d.starts_with('7') || d.starts_with('1') {
        n == 11
    } else if d.starts_with("44") {
        (11..=13).contains(&n)
    } else {
        (8..=15).contains(&n)
    }
}

/// Номер из ссылки мессенджера: `wa.me/+375…`, `api.whatsapp.com/send?phone=…`, `viber://chat?number=%2B375…`.
/// Ссылки без номера (приглашения в группы, `wa.me/message/…`) — `None`.
fn messenger_number(kind: &str, value: &str) -> Option<String> {
    let v = value.trim();
    let num = match kind {
        "whatsapp" if is_http(v) => {
            let u = parse_url_lenient(v)?;
            let from_query = u.query_pairs().find(|(k, _)| k == "phone").map(|(_, x)| x.to_string());
            from_query.or_else(|| u.path_segments().and_then(|mut s| s.next_back()).map(str::to_string)).filter(|x| x.trim_start_matches('+').chars().all(|c| c.is_ascii_digit()))?
        }
        "whatsapp" => v.to_string(),
        "viber" => v.split("number=").nth(1).unwrap_or(v).replace("%2B", "+").replace("%2b", "+"),
        _ => return None,
    };
    let d = digits(&num);
    (d.len() >= 5).then_some(format!("+{d}"))
}

/// Номер из карточки или с сайта настоящий: телефон и номер в ссылке мессенджера по длине подходят стране.
fn contact_plausible(kind: &str, value: &str, country: Option<&str>) -> bool {
    match kind {
        "phone" => phone_plausible(&international_phone(value, country)),
        "whatsapp" | "viber" => messenger_number(kind, value).is_none_or(|n| phone_plausible(&n)),
        _ => true,
    }
}

fn percent_decode_basic(s: &str) -> String {
    s.replace("%40", "@").replace("%2B", "+").replace("%2b", "+").replace("%20", " ")
}

/// Домен-сокращатель: ссылка через него — не контакт.
fn is_shortener(host: &str) -> bool {
    matches!(host, "t.co" | "bit.ly" | "clck.ru" | "vk.cc" | "goo.gl" | "tinyurl.com" | "cutt.ly" | "is.gd" | "u.to")
}

/// Вид соцсети или мессенджера по хосту.
pub fn social_kind(host: &str) -> &'static str {
    let h = host.trim_start_matches("www.");
    let is = |d: &str| h == d || h.ends_with(&format!(".{d}"));
    if is("instagram.com") {
        "instagram"
    } else if is("t.me") || is("telegram.me") {
        "telegram"
    } else if is("wa.me") || is("whatsapp.com") || is("wa.link") || is("walink.co") {
        "whatsapp"
    } else if is("viber.com") {
        "viber"
    } else if is("vk.com") || is("vk.ru") {
        "vk"
    } else if is("facebook.com") || is("fb.com") || is("m.me") {
        "facebook"
    } else if is("tiktok.com") {
        "tiktok"
    } else if is("youtube.com") || is("youtu.be") {
        "youtube"
    } else if is("ok.ru") {
        "ok"
    } else if is("linkedin.com") {
        "linkedin"
    } else if is("twitter.com") || is("x.com") {
        "x"
    } else if is("max.ru") {
        "max"
    } else if h.starts_with("taplink.") || h.contains(".taplink.") {
        "taplink"
    } else if is("linktr.ee") {
        "linktree"
    } else if is("dzen.ru") || is("zen.yandex.ru") {
        "dzen"
    } else if is("rutube.ru") {
        "rutube"
    } else if is("pinterest.com") || is("pinterest.ru") {
        "pinterest"
    } else if is("threads.net") {
        "threads"
    } else if is("icq.im") {
        "icq"
    } else if is("line.me") {
        "line"
    } else {
        "link"
    }
}

/// Ссылка ведёт на профиль или чат, а не на кнопку «поделиться», пиксель или конкретный пост.
fn is_profile_link(u: &Url) -> bool {
    let first = u.path().split('/').nth(1).unwrap_or("").to_lowercase();
    if first.is_empty() {
        return false;
    }
    const NOT_PROFILE: &[&str] = &[
        "sharer", "sharer.php", "share", "share.php", "intent", "tr", "plugins", "dialog", "watch", "embed", "p", "reel", "reels", "explore", "hashtag",
        "login", "signup", "tags", "feeds", "policies", "legal", "shorts", "stories", "pin", "playlist", "live", "video", "videos", "photo", "photos", "story",
        "story.php", "permalink.php", "offer", "dk",
    ];
    if NOT_PROFILE.contains(&first.as_str()) {
        return false;
    }
    let host = u.host_str().unwrap_or("").to_lowercase();
    // Виджеты «поделиться»: connect.ok.ru/offer, ok.ru/dk?st.cmd=WidgetSharePreview…, vk.com/share.php.
    if host == "connect.ok.ru" || (u.query().unwrap_or("").to_lowercase().contains("st.cmd=") && u.query().unwrap_or("").to_lowercase().contains("share")) {
        return false;
    }
    // youtu.be/<id> — всегда конкретное видео.
    if host == "youtu.be" || host.ends_with(".youtu.be") {
        return false;
    }
    // Пост или видео внутри профиля: instagram.com/<user>/p/…, facebook.com/<page>/posts/…, x.com/<user>/status/….
    const POST_SEGMENTS: &[&str] = &["p", "reel", "reels", "stories", "posts", "videos", "status", "shorts", "wall"];
    if u.path().split('/').skip(2).any(|seg| POST_SEGMENTS.contains(&seg.to_lowercase().as_str())) {
        return false;
    }
    // api.whatsapp.com/send?text=… — «поделиться», а не номер.
    !(u.host_str().is_some_and(|h| h.ends_with("whatsapp.com")) && first == "send" && !u.query().unwrap_or("").contains("phone="))
}

/// Ссылка соцсети без меток слежения: `?utm_medium=copy_link`, `igsh`, `igshid`, `hl`, `fbclid`, `mibextid` и подобные.
/// У WhatsApp и Viber остальной запрос (номер, текст) сохраняется.
pub fn clean_social_url(raw: &str) -> String {
    let Some(mut u) = parse_url_lenient(raw) else { return raw.to_string() };
    u.set_fragment(None);
    if u.query().is_some() {
        let keep: Vec<(String, String)> = u
            .query_pairs()
            .filter(|(k, _)| {
                let k = k.to_lowercase();
                !(k.starts_with("utm_") || matches!(k.as_str(), "igsh" | "igshid" | "hl" | "fbclid" | "gclid" | "mibextid" | "ref" | "ref_src" | "si" | "feature" | "locale" | "lang"))
            })
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        if keep.is_empty() {
            u.set_query(None);
        } else {
            u.query_pairs_mut().clear().extend_pairs(keep);
        }
    }
    u.to_string()
}

/// Почта вакансий и HR: оставляем в списке, но помечаем и ставим в конец.
fn is_hr_email(e: &str) -> bool {
    let local = e.to_lowercase();
    let local = local.split('@').next().unwrap_or("");
    ["vakans", "vacanc", "job", "career", "rezume", "resume", "rabota"].iter().any(|k| local.contains(k))
        || local == "hr"
        || ["hr.", "hr-", "hr_"].iter().any(|p| local.starts_with(p))
        || local.ends_with(".hr")
}

/// Ключ ссылки для поиска дублей: хост без www и путь без хвостового «/»; у WhatsApp и Viber важен и запрос.
fn link_key(u: &Url) -> String {
    let host = host_of(u);
    let mut k = format!("{host}{}", u.path().trim_end_matches('/').to_lowercase());
    if matches!(social_kind(&host), "whatsapp" | "viber")
        && let Some(q) = u.query()
    {
        k.push('?');
        k.push_str(&q.to_lowercase());
    }
    k
}

/// Контакты со всех переданных страниц сайта.
pub fn contacts_from_pages<'a>(pages: impl Iterator<Item = &'a PageInfo>) -> Contacts {
    let mut out = Contacts::default();
    let mut email_seen = HashSet::new();
    let mut phone_seen = HashSet::new();
    let mut link_seen = HashSet::new();
    for p in pages {
        // Почты: mailto: и в тексте.
        let mailtos = p.mailtos.iter().flat_map(|m| percent_decode_basic(m).split([',', ';']).map(str::to_string).collect::<Vec<_>>());
        let from_text = email_re().find_iter(&p.text).map(|m| m.as_str().to_string());
        for raw in mailtos.chain(from_text) {
            let Some(m) = email_re().find(&raw) else { continue };
            let e = m.as_str().trim_matches('.').to_string();
            if good_email(&e) && email_seen.insert(e.to_lowercase()) && out.emails.len() < MAX_EMAILS {
                out.emails.push(e);
            }
        }
        // Телефоны: сначала в тексте (там они красиво записаны), затем tel:-ссылки.
        for raw in phone_matches(&p.text).into_iter().filter(|r| !is_date_like(r)) {
            if let Some(k) = phone_key(&raw)
                && phone_seen.insert(k)
                && out.phones.len() < MAX_PHONES
            {
                out.phones.push(tidy_phone(&raw));
            }
        }
        for raw in &p.tel_links {
            let t = percent_decode_basic(raw);
            if let Some(k) = phone_key(&t)
                && phone_seen.insert(k)
                && out.phones.len() < MAX_PHONES
            {
                out.phones.push(tidy_phone(&t));
            }
        }
        // Соцсети и мессенджеры.
        for l in &p.links {
            let Some(u) = parse_url_lenient(&l.href) else { continue };
            let (kind, host) = classify_website(&l.href);
            if !matches!(kind, SiteKind::Social | SiteKind::Messenger | SiteKind::LinkInBio) || is_shortener(&host) || !is_profile_link(&u) {
                continue;
            }
            if link_seen.insert(link_key(&u)) && out.links.len() < MAX_LINKS {
                out.links.push(SiteLink { kind: social_kind(&host).to_string(), url: clean_social_url(u.as_str()) });
            }
        }
    }
    out
}

/// Контакты со всех обойдённых страниц сайта.
pub fn site_contacts(snap: &SiteSnapshot) -> Contacts {
    contacts_from_pages(snap.all_pages())
}

/// Контакт в карточке лида.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Contact {
    /// phone, email, instagram, telegram, whatsapp, viber, vk, facebook… (у ручных — как ввёл менеджер).
    pub kind: String,
    pub value: String,
    /// Готовая ссылка: `tel:`, `mailto:`, `https://wa.me/…`, `https://t.me/…`, сама ссылка соцсети.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// `card` — из карточек карт, `site` — с сайта, `manual` — добавлено вручную.
    pub source: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Контакт, добавленный менеджером (колонка `extra_contacts`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ManualContact {
    pub kind: String,
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

fn digits(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_digit()).collect()
}

fn is_http(s: &str) -> bool {
    let l = s.trim().to_ascii_lowercase();
    l.starts_with("http://") || l.starts_with("https://")
}

/// Ник из «@nick», «t.me/nick» или «nick».
fn handle(value: &str) -> String {
    let v = value.trim();
    let v = v.rsplit('/').find(|x| !x.is_empty()).unwrap_or(v);
    v.trim_start_matches('@').to_string()
}

/// Номер в международном виде `+<код><номер>` для `tel:`: «8 017 345-67-89» + `by` → `+375173456789`.
/// Без «+» и без кода страны скана нельзя угадать код — тогда только цифры, как в записи.
pub fn international_phone(raw: &str, country: Option<&str>) -> String {
    let t = raw.trim();
    let mut d = digits(t);
    if t.starts_with('+') {
        // «+44 (0)20 …»: нуль после кода страны лишний.
        if let Some(cc) = crate::region::calling_code(country)
            && d.starts_with(cc)
            && d[cc.len()..].starts_with('0')
            && !matches!(cc, "7" | "1")
        {
            d.remove(cc.len());
        }
        return format!("+{d}");
    }
    if t.starts_with("00") && d.len() > 9 {
        return format!("+{}", &d[2..]);
    }
    let Some(cc) = crate::region::calling_code(country) else { return d };
    // Уже с кодом страны, но без «+»: 375291234567, 74951234567, 442072875327.
    let with_cc_len = if cc.len() == 1 { 11 } else { cc.len() + 7 };
    if d.starts_with(cc) && d.len() >= with_cc_len && !(cc == "375" && d.len() < 12) {
        return format!("+{d}");
    }
    // Национальный формат: выход на межгород — «8 0xx» в Беларуси, «8» в России и Казахстане, «0» в Европе.
    if cc == "375" && d.starts_with("80") && d.len() >= 11 {
        d.drain(..2);
    } else if (cc == "7" && d.starts_with('8') && d.len() == 11) || (d.starts_with('0') && cc != "7" && cc != "1") {
        d.remove(0);
    }
    if (6..=11).contains(&d.len()) { format!("+{cc}{d}") } else { d }
}

/// Готовая ссылка для контакта по его виду и значению (телефон — как записан, без кода страны).
pub fn contact_url(kind: &str, value: &str) -> Option<String> {
    contact_url_in(kind, value, None)
}

/// То же с кодом страны скана: `tel:` всегда в международном формате (+375…).
pub fn contact_url_in(kind: &str, value: &str, country: Option<&str>) -> Option<String> {
    let v = value.trim();
    if v.is_empty() {
        return None;
    }
    let looks_phone = || digits(v).len() >= 7 && v.chars().all(|c| c.is_ascii_digit() || " +-()\u{a0}.".contains(c));
    match kind {
        "phone" => {
            let d = digits(v);
            (d.len() >= 5).then(|| format!("tel:{}", international_phone(v, country)))
        }
        "email" => Some(format!("mailto:{v}")),
        "whatsapp" if is_http(v) => Some(v.to_string()),
        "whatsapp" => {
            let d = digits(v);
            (d.len() >= 7).then(|| format!("https://wa.me/{d}"))
        }
        "viber" if is_http(v) || v.starts_with("viber://") => Some(v.to_string()),
        "viber" => {
            let d = digits(v);
            (d.len() >= 7).then(|| format!("viber://chat?number=%2B{d}"))
        }
        "telegram" if is_http(v) => Some(v.to_string()),
        "telegram" if looks_phone() => Some(format!("https://t.me/+{}", digits(v))),
        "telegram" => Some(format!("https://t.me/{}", handle(v))),
        "instagram" | "vk" | "facebook" | "tiktok" if !is_http(v) => {
            let host = match kind {
                "instagram" => "instagram.com",
                "vk" => "vk.com",
                "facebook" => "facebook.com",
                _ => "tiktok.com/@",
            };
            let nick = handle(v);
            Some(if host.ends_with('@') { format!("https://www.{host}{nick}") } else { format!("https://www.{host}/{nick}") })
        }
        _ if is_http(v) => Some(v.to_string()),
        _ if !v.contains(' ') && v.contains('.') && !v.contains('@') => Some(format!("https://{v}")),
        _ => None,
    }
}

/// Ключ для поиска дублей между карточкой, сайтом и ручными контактами.
fn contact_key(kind: &str, value: &str, url: Option<&str>) -> String {
    match kind {
        "phone" => format!("phone:{}", phone_key(value).unwrap_or_else(|| digits(value))),
        "email" => format!("email:{}", value.trim().to_lowercase()),
        _ => match url.and_then(parse_url_lenient) {
            Some(u) => format!("link:{}", link_key(&u)),
            None => format!("{kind}:{}", value.trim().to_lowercase()),
        },
    }
}

/// Разделители названия и «хвоста» в заголовке страницы. Закрывающую кавычку «»» разделителем не считаем:
/// «Стоматология «Вива Медика» | Минск» должна дать «Стоматология «Вива Медика»», а не обрезок.
fn title_head(title: &str) -> &str {
    let mut cut = title.len();
    for sep in ["|", "—", "–", "·", " - ", ", ", ","] {
        if let Some(p) = title.find(sep) {
            cut = cut.min(p);
        }
    }
    // Точка с пробелом — конец предложения («Клиника. Лечение зубов»), но не сокращение («Dr. Smith»).
    for (p, _) in title.match_indices(". ") {
        if title[..p].chars().count() >= 4 {
            cut = cut.min(p);
            break;
        }
    }
    &title[..cut]
}

/// Кавычки названия приводим к одному виду: внешние убираем («Вива Медика» → Вива Медика), недостающую «»» добавляем.
fn tidy_name_quotes(raw: &str) -> String {
    let mut t = raw.split_whitespace().collect::<Vec<_>>().join(" ").trim_matches(|c: char| matches!(c, '.' | ',' | ';' | ':' | '-' | ' ')).to_string();
    let wrapped = |t: &str, o: char, c: char| t.starts_with(o) && t.ends_with(c) && t.matches(o).count() == 1 && t.matches(c).count() == 1;
    if wrapped(&t, '«', '»') || wrapped(&t, '"', '"') || wrapped(&t, '“', '”') {
        t = t.chars().skip(1).collect::<String>();
        t.pop();
        return t.trim().to_string();
    }
    let (open, close) = (t.matches('«').count(), t.matches('»').count());
    if open > close {
        t.push('»');
    } else if close > open && t.ends_with('»') {
        t.pop();
    }
    t
}

/// Название похоже на SEO-заголовок, домен или служебный текст — компанией не называется.
fn not_a_company_name(cand: &str) -> bool {
    static DOMAIN: OnceLock<Regex> = OnceLock::new();
    static CITY: OnceLock<Regex> = OnceLock::new();
    let domain = DOMAIN.get_or_init(|| Regex::new(r"(?i)^(?:https?://)?(?:[a-z0-9\-]+\.)+[a-z]{2,}(?:/\S*)?$").unwrap());
    let city = CITY.get_or_init(|| Regex::new(r"\b(?:в|во)\s+[А-ЯЁ][а-яё\-]+(?:е|и|у|ь)\b|\bin\s+[A-Z][a-z]+").unwrap());
    let low = cand.to_lowercase();
    const SEO: &[&str] = &[
        "цены", "цена", "отзыв", "платная", "платные", "бесплатн", "официальный сайт", "купить", "недорого", "под ключ", "заказать",
        "запись онлайн", "лечение", "услуги", "prices", "reviews", "official site", "official website", "best ", "cheap", "near me", "book online",
    ];
    const SERVICE: &[&str] = &[
        "403", "404", "forbidden", "access denied", "just a moment", "attention required", "welcome to nginx", "coming soon", "under construction",
        "untitled", "wordpress", "loading", "redirect", "в разработке", "парковк", "домен", "account suspended", "apache", "it works",
    ];
    domain.is_match(cand) || city.is_match(cand) || SEO.iter().chain(SERVICE).any(|w| low.contains(w))
}

/// Подсказка менеджеру: название компании с сайта (`og:site_name`, иначе начало `<title>` до разделителя),
/// если оно отличается от названия в карточке и от того, что менеджер уже задал.
/// Не предлагаем длиннее 50 знаков, SEO-заголовки («цены», «отзывы», «в Минске»), домены и служебные заголовки.
pub fn name_suggestion(site_name: Option<&str>, title: Option<&str>, current: &str, card: &str) -> Option<String> {
    let norm = |x: &str| x.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect::<String>();
    let from_title = title.map(title_head);
    let (cur, card) = (norm(current), norm(card));
    let cand = [site_name, from_title].into_iter().flatten().map(tidy_name_quotes).filter(|n| !n.is_empty()).find(|n| {
        // Служебные заголовки главной — не название компании.
        let generic = [
            "главная", "главнаястраница", "home", "homepage", "index", "welcome", "добропожаловать", "добропожаловатьнанашсайт",
            // Одно слово вида деятельности — не название компании.
            "стоматология", "клиника", "салон", "центр", "студия", "ресторан", "кафе", "магазин", "компания", "dentist", "dental", "clinic", "salon", "restaurant", "cafe", "shop", "store",
        ]
        .contains(&norm(n).as_str());
        !generic && n.chars().count() <= 50 && !not_a_company_name(n)
    })?;
    (norm(&cand) != cur && norm(&cand) != card).then_some(cand)
}

/// Контакты с сайта принадлежат компании? Нет, если вместо сайта заглушка или он не открывается
/// (контакты и соцсети хостера: Hover, 7246) и если сайт чужой или не подтверждён (H02 `foreign`/`unverified`).
pub fn site_contacts_trusted(state: SiteState, results: &[MetricResult]) -> bool {
    if matches!(state, SiteState::Stub | SiteState::Dead) {
        return false;
    }
    let h02 = results.iter().find(|r| r.id == "H02").and_then(|r| r.value.as_ref()).and_then(|v| v.as_str());
    !matches!(h02, Some("foreign" | "unverified"))
}

/// Список контактов компании: карточки карт, сайт и ручные. Без дублей (первый побеждает).
/// `country` — страна скана: по ней `tel:` получает международный код.
/// Почты вакансий и HR идут в конце списка с пометкой «вакансии».
pub fn contact_list(card: &Business, site: &Contacts, manual: &[ManualContact], country: Option<&str>) -> Vec<Contact> {
    let mut out: Vec<Contact> = Vec::new();
    let mut seen = HashSet::new();
    let mut push = |kind: &str, value: &str, source: &'static str, note: Option<String>| {
        let value = value.trim();
        if value.is_empty() || (kind == "phone" && source != "manual" && is_date_like(value)) {
            return;
        }
        // Обрубок номера («wa.me/+3753860005», «+375 17 270») — не контакт; ручной ввод менеджера не трогаем.
        if source != "manual" && !contact_plausible(kind, value, country) {
            return;
        }
        // Ссылки соцсетей без меток слежения (в старых сохранённых замерах они ещё есть).
        let cleaned;
        let value = if is_http(value) && !matches!(kind, "phone" | "email") && social_kind(&host_of_str(value)) != "link" {
            cleaned = clean_social_url(value);
            cleaned.as_str()
        } else {
            value
        };
        let note = note.or_else(|| (kind == "email" && source != "manual" && is_hr_email(value)).then(|| "вакансии".to_string()));
        let url = contact_url_in(kind, value, country);
        if seen.insert(contact_key(kind, value, url.as_deref())) {
            // Телефон с сайта и карточки — аккуратно (в старых замерах попадались «017) 276 74 26»).
            let shown = if kind == "phone" && source != "manual" { display_phone(value, country) } else { value.to_string() };
            out.push(Contact { kind: kind.to_string(), value: shown, url, source, note });
        }
    };
    // Карточки: телефоны, почты, ссылки (соцсети и мессенджеры; свой сайт — не контакт).
    for ph in card.phone.iter().chain(card.sources.iter().filter_map(|s| s.phone.as_ref())) {
        push("phone", ph, "card", None);
    }
    for e in card.emails() {
        push("email", e, "card", None);
    }
    for l in card.all_links() {
        let (kind, host) = classify_website(l);
        if matches!(kind, SiteKind::Social | SiteKind::Messenger | SiteKind::LinkInBio) && !is_shortener(&host) && parse_url_lenient(l).is_some_and(|u| is_profile_link(&u)) {
            push(social_kind(&host), l, "card", None);
        }
    }
    for ph in &site.phones {
        push("phone", ph, "site", None);
    }
    for e in &site.emails {
        push("email", e, "site", None);
    }
    // Фильтр и при чтении: в старых сохранённых замерах могли остаться ссылки на отдельные видео и посты.
    for l in site.links.iter().filter(|l| parse_url_lenient(&l.url).is_none_or(|u| is_profile_link(&u))) {
        push(&l.kind, &l.url, "site", None);
    }
    for m in manual {
        push(m.kind.trim(), &m.value, "manual", m.note.clone().filter(|n| !n.trim().is_empty()));
    }
    // Стабильно: сначала обычные контакты, затем почты вакансий.
    let hr = |c: &Contact| c.kind == "email" && c.source != "manual" && c.note.as_deref() == Some("вакансии");
    out.sort_by_key(hr);
    out
}

fn host_of_str(url: &str) -> String {
    parse_url_lenient(url).map(|u| host_of(&u)).unwrap_or_default()
}

// ---------------------------------------------------------------------------------------------
// План работ и КП
// ---------------------------------------------------------------------------------------------

/// Пакет работ в прайсе.
pub struct PackageDef {
    pub id: &'static str,
    pub title: &'static str,
    /// Что входит — для клиента, одной строкой.
    pub summary: &'static str,
    /// Цена по умолчанию (в валюте настроек; примерные BYN).
    pub price: f64,
}

pub const PACKAGES: &[PackageDef] = &[
    PackageDef {
        id: "new_site",
        title: "Новый сайт: одностраничник с записью и SEO под «услуга + город»",
        summary: "Сайт, удобный на телефоне, с услугами, ценами, отзывами и формой записи; заголовки и тексты написаны так, чтобы вас находили по запросам «услуга + город»; счётчик посетителей и быстрый хостинг",
        price: 900.0,
    },
    PackageDef {
        id: "maps",
        title: "Карточки на картах и контакты",
        summary: "Привести карточки в Google, Яндекс и 2ГИС в порядок: сайт, часы, фото, описание, одинаковые адрес и телефон везде",
        price: 100.0,
    },
    PackageDef { id: "https", title: "HTTPS и перенаправление", summary: "Включить защищённое соединение (https), чтобы старые адреса сами открывали защищённую версию, — браузер перестанет писать «Не защищено»", price: 60.0 },
    PackageDef { id: "speed", title: "Скорость загрузки", summary: "Ускорить загрузку: сжать картинки и файлы, чтобы браузер запоминал их, — сайт быстро открывается и с мобильного интернета", price: 150.0 },
    PackageDef { id: "mobile", title: "Мобильная версия", summary: "Адаптировать страницы под телефон: крупный текст, удобные кнопки, нажимаемые телефон и адрес", price: 300.0 },
    PackageDef { id: "indexing", title: "Индексация в Google и Яндексе", summary: "Открыть сайт для поисковиков, добавить карту сайта (список страниц для поисковиков), подключить сайт к кабинетам Яндекса и Google — так поисковики быстрее узнают о страницах, — убрать дубли и битые ссылки", price: 120.0 },
    PackageDef { id: "content", title: "Тексты и заголовки под «услуга + город»", summary: "Переписать заголовки и описания страниц под запросы клиентов, сделать страницы под основные услуги", price: 400.0 },
    PackageDef { id: "schema", title: "Разметка для поисковиков и ИИ", summary: "Добавить на сайт скрытые подписи для поисковиков и ИИ: название компании, адрес, часы работы, услуги — так они точнее показывают компанию", price: 120.0 },
    PackageDef { id: "ai", title: "Готовность к ИИ-поиску (ChatGPT, Алиса, Perplexity)", summary: "Разрешить ИИ-ассистентам читать сайт, добавить ответы на частые вопросы и факты, которые ИИ сможет процитировать", price: 150.0 },
    PackageDef { id: "legal", title: "Документы и право: политика ПД, cookie, реквизиты", summary: "Политика обработки персональных данных, согласие в формах, уведомление о cookie, реквизиты компании", price: 120.0 },
    PackageDef { id: "conversion", title: "Заявки: кнопки, формы, онлайн-запись, отзывы", summary: "Заметные кнопки записи и звонка, форма заявки, онлайн-запись, блок отзывов", price: 200.0 },
    PackageDef { id: "security", title: "Безопасность", summary: "Базовая защита сайта, обновление программы, на которой он работает, проверка на взлом и попадание в чёрные списки, защита от подделки писем с вашего домена", price: 80.0 },
    PackageDef { id: "analytics", title: "Аналитика и цели", summary: "Счётчики посетителей — Яндекс Метрика и Google Analytics, учёт звонков и заявок: видно, сколько клиентов даёт сайт", price: 80.0 },
    PackageDef { id: "a11y", title: "Доступность", summary: "Читаемый контраст, подписи у форм и картинок — сайтом удобно пользоваться всем, в том числе людям с плохим зрением", price: 100.0 },
    PackageDef { id: "yandex", title: "Факторы Яндекса", summary: "Указать регион и контакты так, как их читает Яндекс, страница «Контакты», способы оплаты и гарантии, убрать приёмы, за которые Яндекс понижает сайт", price: 150.0 },
];

/// Пакет, к которому относится метрика; None — метрика в план работ не входит.
pub fn package_of(id: &str) -> Option<&'static str> {
    let cat = id.chars().next()?;
    let n: u32 = id.get(1..)?.parse().ok()?;
    Some(match (cat, n) {
        ('A', _) | ('R', 1) | ('H', 2..=8) => "maps",
        ('B', 4 | 5) | ('O', 2) => "https",
        ('B', 7..=11) | ('D', _) => "speed",
        ('E', _) => "mobile",
        ('B', 6 | 12) | ('C', _) => "indexing",
        ('F', _) => "content",
        ('G', _) => "schema",
        ('I', _) => "ai",
        ('M', _) => "legal",
        ('N', _) | ('H', 1) => "conversion",
        ('O', _) | ('B', 13 | 2 | 14) => "security",
        ('P', _) => "analytics",
        ('Q', _) => "a11y",
        ('R', _) => "yandex",
        _ => return None,
    })
}

/// Английские названия и описания пакетов: (id, название, что входит).
/// Нужны КП для клиентов из англоязычных стран; пакет «Факторы Яндекса» там не предлагается.
const PACKAGES_EN: &[(&str, &str, &str)] = &[
    ("new_site", "New website: a one-page site with booking and local SEO", "A mobile-friendly site with services, prices, reviews and an enquiry form; headings and copy written so people find you for “service + city” searches; a visitor counter and fast hosting"),
    ("maps", "Map listings and contact details", "Bring your Google, Apple and Bing listings in order: website, opening hours, photos, description, and the same address and phone everywhere"),
    ("https", "HTTPS and redirects", "Turn on the secure connection (https) and make old addresses open the secure version, so the browser stops saying “Not secure”"),
    ("speed", "Page speed", "Speed up loading: compress images and files so the browser remembers them, so the site opens quickly even on mobile data"),
    ("mobile", "Mobile version", "Adapt the pages to phones: readable text, easy-to-tap buttons, a tappable phone number and address"),
    ("indexing", "Google and Bing indexing", "Open the site to search engines, add a sitemap (a list of pages for them), connect the site to Google Search Console and Bing Webmaster Tools so search engines learn about your pages faster, and remove duplicates and broken links"),
    ("content", "Copy and headings for “service + city”", "Rewrite page titles and descriptions for the searches your customers use, and build pages for the main services"),
    ("schema", "Search and AI markup", "Add hidden labels for search engines and AI: business name, address, opening hours, services, so they show the business accurately"),
    ("ai", "AI search readiness (ChatGPT, Perplexity, Google AI)", "Let AI assistants read the site, add answers to common questions and facts that AI can quote"),
    ("legal", "Legal pages: privacy policy, cookies, company details", "Privacy policy, consent in forms, a cookie notice and the company's registration details"),
    ("conversion", "Enquiries: buttons, forms, online booking, reviews", "Prominent booking and call buttons, an enquiry form, online booking and a reviews block"),
    ("security", "Security", "Basic site protection, updates to the software the site runs on, a check for hacks and blacklisting, protection against forged emails from your domain"),
    ("analytics", "Analytics and goals", "Visitor counters (Google Analytics) and goals for calls and enquiries, so you can see how many customers the site brings"),
    ("a11y", "Accessibility", "Readable contrast, labels on forms and images, so everyone can use the site"),
    ("yandex", "Yandex ranking factors", "Region and contacts for Yandex, a Contacts page, payment methods and guarantees"),
];

fn package_en(id: &str) -> Option<&'static (&'static str, &'static str, &'static str)> {
    PACKAGES_EN.iter().find(|p| p.0 == id)
}

/// Валюта КП по стране скана: by→BYN, ru→RUB, kz→KZT, ua→UAH, pl→PLN, gb→GBP, us→USD,
/// еврозона→EUR, ch→CHF, cz→CZK; для остальных и без страны — валюта из настроек.
pub fn currency_for_country(country: Option<&str>, fallback: &str) -> String {
    let c = match country.map(|c| c.trim().to_ascii_lowercase()).as_deref() {
        Some("by") => "BYN",
        Some("ru") => "RUB",
        Some("kz") => "KZT",
        Some("ua") => "UAH",
        Some("pl") => "PLN",
        Some("gb" | "uk") => "GBP",
        Some("us") => "USD",
        Some("ch" | "li") => "CHF",
        Some("cz") => "CZK",
        // Еврозона на 2026 год (с Болгарией).
        Some("at" | "be" | "bg" | "cy" | "de" | "ee" | "es" | "fi" | "fr" | "gr" | "hr" | "ie" | "it" | "lt" | "lu" | "lv" | "mt" | "nl" | "pt" | "sk" | "si" | "mc") => "EUR",
        _ => return fallback.to_string(),
    };
    c.to_string()
}

/// Цены по умолчанию для валют, в которых прайс из настроек не задан: GBP, USD, EUR.
/// Это не пересчёт по курсу, а ориентиры по рынку (новый сайт — 1200 GBP, остальные пакеты — примерно
/// 0,4–0,5 от цен в BYN, округлено); их стоит поправить под себя в настройках (`price_lists`).
/// USD и EUR — те же ориентиры с рыночным коэффициентом 1,25 и 1,15. Для других валют None.
pub fn default_price_list_for(currency: &str) -> Option<Vec<PriceItem>> {
    // По порядку PACKAGES: new_site, maps, https, speed, mobile, indexing, content, schema, ai, legal,
    // conversion, security, analytics, a11y, yandex.
    const GBP: [f64; 15] = [1200.0, 50.0, 35.0, 75.0, 150.0, 60.0, 200.0, 60.0, 75.0, 60.0, 100.0, 40.0, 40.0, 50.0, 75.0];
    let k = match currency.to_ascii_uppercase().as_str() {
        "GBP" => 1.0,
        "USD" => 1.25,
        "EUR" => 1.15,
        _ => return None,
    };
    // Красивое округление: до 5 ниже 500 и до 50 выше.
    let nice = |x: f64| if x >= 500.0 { (x / 50.0).round() * 50.0 } else { (x / 5.0).round() * 5.0 };
    Some(PACKAGES.iter().zip(GBP).map(|(p, g)| PriceItem { id: p.id.to_string(), title: p.title.to_string(), price: nice(g * k), enabled: true }).collect())
}

/// Прайс для валюты КП. Второе значение — `true`, если цены взяты из настроек «как есть» для чужой валюты
/// (своих цен для неё нет и курсом не пересчитываем): менеджеру стоит проверить суммы.
/// Порядок: валюта настроек → отдельный прайс из `extra` → ориентиры GBP/USD/EUR → прайс настроек с пометкой.
/// Названия и «включён» берутся из прайса настроек.
pub fn price_list_for(
    currency: &str,
    settings_currency: &str,
    settings_list: &[PriceItem],
    extra: &std::collections::BTreeMap<String, Vec<PriceItem>>,
) -> (Vec<PriceItem>, bool) {
    let cur = currency.to_ascii_uppercase();
    if cur == settings_currency.to_ascii_uppercase() {
        return (settings_list.to_vec(), false);
    }
    if let Some(own) = extra.get(&cur) {
        return (normalize_price_list(own), false);
    }
    if let Some(mut def) = default_price_list_for(&cur) {
        for d in def.iter_mut() {
            if let Some(row) = settings_list.iter().find(|r| r.id == d.id) {
                d.enabled = row.enabled;
                d.title = row.title.clone();
            }
        }
        return (def, false);
    }
    (settings_list.to_vec(), true)
}

/// Метрики карточки, которые менеджер действительно чинит (рейтинг, отзывы и позиции — не его работа),
/// и их формулировки простыми словами.
fn card_label(id: &str) -> Option<&'static str> {
    Some(match id {
        "A01" => "Карточка есть не на всех картах",
        "A02" => "Карточка не подтверждена владельцем",
        "A03" => "В карточке не указан сайт",
        "A04" => "В карточке вместо сайта соцсеть или каталог",
        "A10" => "Мало фотографий в карточке",
        "A11" => "Не заполнены категории карточки",
        "A12" => "Не заполнены часы работы в карточке",
        "A13" => "Нет описания в карточке",
        "A14" => "Нет услуг и цен в карточке",
        "A15" => "Нет кнопки записи в карточке",
        "A16" => "Нет публикаций и акций в карточке",
        "A17" => "Не заполнены атрибуты карточки",
        "A18" => "Адрес, телефон и сайт различаются на разных картах",
        "A19" => "Дубли карточки на картах",
        "A21" => "Карточка по ошибке помечена закрытой",
        "A24" => "Переходы с карт на сайт не отмечены — не видно, сколько клиентов приходит с карт",
        _ => return None,
    })
}

/// Строка прайса: правки менеджера к пакету (настройки).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PriceItem {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub price: f64,
    /// false — пакет не продаётся и в КП не попадает.
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

/// Прайс по умолчанию — все пакеты с примерными ценами.
pub fn default_price_list() -> Vec<PriceItem> {
    PACKAGES.iter().map(|p| PriceItem { id: p.id.to_string(), title: p.title.to_string(), price: p.price, enabled: true }).collect()
}

/// Прайс из настроек, дополненный умолчаниями: неизвестные пакеты отбрасываются, недостающие возвращаются.
pub fn normalize_price_list(items: &[PriceItem]) -> Vec<PriceItem> {
    PACKAGES
        .iter()
        .map(|d| match items.iter().find(|i| i.id == d.id) {
            Some(i) => PriceItem {
                id: d.id.to_string(),
                title: if i.title.trim().is_empty() { d.title.to_string() } else { i.title.trim().to_string() },
                price: if i.price.is_finite() && i.price >= 0.0 { i.price } else { d.price },
                enabled: i.enabled,
            },
            None => PriceItem { id: d.id.to_string(), title: d.title.to_string(), price: d.price, enabled: true },
        })
        .collect()
}

/// Правка пакета менеджером.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SavedPackage {
    pub id: String,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub price: Option<f64>,
    #[serde(default)]
    pub title: Option<String>,
}

/// Дополнительная строка КП.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ExtraLine {
    pub title: String,
    #[serde(default)]
    pub price: f64,
}

/// Сохранённые правки КП (колонка `proposal`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedProposal {
    pub packages: Vec<SavedPackage>,
    pub extra: Vec<ExtraLine>,
    pub discount: Option<f64>,
    pub note: Option<String>,
    pub valid_days: Option<u32>,
}

impl SavedProposal {
    /// Из JSON колонки; повреждённое или пустое — без правок.
    pub fn from_value(v: &Value) -> Self {
        serde_json::from_value(v.clone()).unwrap_or_default()
    }
}

/// Пункт плана: проблема, которую закрывает пакет.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProposalItem {
    /// Номер метрики; у пунктов про отсутствие сайта — `site`.
    pub id: String,
    pub text: String,
    /// `fail` — проблема, `warn` — стоит поправить.
    pub level: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProposalPackage {
    pub id: String,
    pub title: String,
    /// Что входит в работу — для клиента.
    pub summary: String,
    pub items: Vec<ProposalItem>,
    pub price: f64,
    /// Включён в итог (с учётом правок менеджера).
    pub enabled: bool,
    /// Включён по умолчанию: есть хотя бы одна проваленная метрика.
    pub default_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Proposal {
    pub packages: Vec<ProposalPackage>,
    pub extra: Vec<ExtraLine>,
    pub discount: f64,
    /// Сумма включённых пакетов и дополнительных строк.
    pub subtotal: f64,
    pub total: f64,
    pub note: String,
    pub valid_days: u32,
    /// Язык текстов КП: `ru` или `en`.
    pub lang: String,
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

fn money(x: Option<f64>) -> Option<f64> {
    x.filter(|v| v.is_finite() && *v >= 0.0)
}

/// Собрать план работ: пакеты с пунктами из проваленных и «внимание»-метрик, цены из прайса, правки менеджера.
/// `lang` — язык текстов КП (`ru`/`en`, см. `outreach_lang`).
pub fn build_proposal(results: &[MetricResult], state: SiteState, price_list: &[PriceItem], saved: &SavedProposal, lang: &str) -> Proposal {
    let en = lang == "en";
    let no_site = matches!(state, SiteState::None | SiteState::Social | SiteState::Dead | SiteState::Stub);
    let main = mini::main_problem_ids(results, state);
    let mut packages = Vec::new();
    for def in PACKAGES {
        if no_site && !matches!(def.id, "new_site" | "maps") {
            continue;
        }
        // Яндекс в англоязычных странах не нужен.
        if en && def.id == "yandex" {
            continue;
        }
        let row = price_list.iter().find(|p| p.id == def.id);
        if row.is_some_and(|r| !r.enabled) {
            continue;
        }
        let mut items: Vec<ProposalItem> = Vec::new();
        let mut fails = 0;
        if def.id == "new_site" && no_site {
            let text = match (state, en) {
                (SiteState::None, false) => "Сайта нет — клиенты с карт не могут посмотреть услуги и цены и записаться",
                (SiteState::None, true) => "There is no website: customers coming from the maps can't see services or prices, or get in touch",
                (SiteState::Social, false) => "Вместо сайта — страница в соцсети или каталог",
                (SiteState::Social, true) => "A social media page or a directory entry stands in for a website",
                (SiteState::Dead, false) => "Сайт не открывается — по факту сайта нет",
                (SiteState::Dead, true) => "The website doesn't open, so in practice there is no website",
                (_, false) => "Вместо сайта заглушка — по факту сайта нет",
                (_, true) => "A placeholder page stands in for the website, so in practice there is no website",
            };
            items.push(ProposalItem { id: "site".into(), text: text.into(), level: "fail" });
            fails += 1;
        }
        for r in results {
            if !matches!(r.status, Status::Fail | Status::Warn) || package_of(&r.id) != Some(def.id) {
                continue;
            }
            // «Защита может не пускать ИИ-ботов» при уже закрытом для ИИ сайте — та же мысль второй раз.
            if r.id == "I05" && fail(results, "I02").is_some() {
                continue;
            }
            let text = if r.id.starts_with('A') && !en {
                card_label(&r.id).map(|t| mini::with_maps(&r.id, t, r.maps.as_deref(), lang))
            } else if r.id.starts_with('A') {
                mini::label_en(&r.id).filter(|_| card_label(&r.id).is_some()).map(|t| mini::with_maps(&r.id, t, r.maps.as_deref(), lang))
            } else {
                Some(mini::plain_text_lang(r, lang))
            };
            let Some(text) = text.filter(|t| !t.is_empty()) else { continue };
            if r.status == Status::Fail {
                fails += 1;
            }
            if !items.iter().any(|i| i.text == text) {
                items.push(ProposalItem { id: r.id.clone(), text, level: if r.status == Status::Fail { "fail" } else { "warn" } });
            }
        }
        // Сначала проблемы, потом «стоит поправить».
        items.sort_by_key(|i| i.level != "fail");
        // Пакет включён, если в нём есть проблема или его метрика попала в «главное» мини-отчёта
        // (gor-dent.by: «На телефоне нельзя увеличить текст» в главном, а пакет «Мобильная версия» был выключен).
        let default_enabled = fails > 0 || items.iter().any(|i| main.contains(&i.id));
        let mine = saved.packages.iter().find(|s| s.id == def.id);
        let en_pack = package_en(def.id).filter(|_| en);
        // Название из прайса, если менеджер его менял; стандартное русское название в английском КП заменяется английским.
        let title = mine
            .and_then(|s| s.title.as_deref())
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .or(row.map(|r| r.title.trim()).filter(|t| !t.is_empty() && !(en_pack.is_some() && *t == def.title)))
            .or(en_pack.map(|p| p.1))
            .unwrap_or(def.title)
            .to_string();
        let price = money(mine.and_then(|s| s.price)).or(money(row.map(|r| r.price))).unwrap_or(def.price);
        packages.push(ProposalPackage {
            id: def.id.to_string(),
            title,
            summary: en_pack.map_or(def.summary, |p| p.2).to_string(),
            items,
            price,
            enabled: mine.and_then(|s| s.enabled).unwrap_or(default_enabled),
            default_enabled,
        });
    }
    let extra: Vec<ExtraLine> =
        saved.extra.iter().filter(|e| !e.title.trim().is_empty()).map(|e| ExtraLine { title: e.title.trim().to_string(), price: money(Some(e.price)).unwrap_or(0.0) }).collect();
    let subtotal = round2(packages.iter().filter(|p| p.enabled).map(|p| p.price).sum::<f64>() + extra.iter().map(|e| e.price).sum::<f64>());
    let discount = round2(money(saved.discount).unwrap_or(0.0).min(subtotal));
    Proposal {
        packages,
        extra,
        discount,
        subtotal,
        total: round2(subtotal - discount),
        note: saved.note.clone().unwrap_or_default(),
        valid_days: saved.valid_days.filter(|d| (1..=365).contains(d)).unwrap_or(14),
        lang: if en { "en" } else { "ru" }.to_string(),
    }
}

// ---------------------------------------------------------------------------------------------
// Первое сообщение
// ---------------------------------------------------------------------------------------------

/// Профиль менеджера из настроек: подпись и первая фраза.
#[derive(Debug, Clone, Default)]
pub struct Manager {
    pub name: String,
    pub email: String,
    pub phone: String,
    pub telegram: String,
    pub company: String,
    /// Своя первая фраза; пусто — по категории.
    pub intro: String,
    /// Ссылка на примеры работ — в подписи и для нейросети.
    pub portfolio: String,
    /// Пара фраз о себе для нейросети: опыт, с кем работаете, примеры.
    pub about: String,
}

/// Домен клиента для сообщения и КП: из карточки (magicart.by), затем адрес найденного сайта,
/// и только в последнюю очередь — конечный адрес после перенаправлений: у мёртвого сайта это
/// страница хостера («start.hoster.by»), которую клиенту называть нельзя.
///
/// У заглушки и мёртвого сайта адрес — как в ссылке карточки, с «www.», если он там есть: голый домен
/// может вести на другой сайт (magicart.by → чужой сайт, www.magicart.by → заглушка хостера).
pub fn client_host(state: SiteState, card_website: Option<&str>, site_url: Option<&str>, final_url: Option<&str>) -> Option<String> {
    let linked = matches!(state, SiteState::Stub | SiteState::Dead);
    // Работающий сайт называем ровно тем хостом, который открылся (с «www.», если он там): голый домен может
    // не резолвиться (metamute.org → работает только www.metamute.org). Адрес из карточки — запасной.
    let order = if linked { [card_website, site_url, final_url] } else { [final_url, site_url, card_website] };
    order.into_iter().flatten().filter_map(parse_url_lenient).find_map(|u| Some(u.host_str().unwrap_or("").to_lowercase()).filter(|h| !h.is_empty()))
}

/// Слово по категории в родительном падеже для темы письма: «Сайт для клиники «…»».
fn kind_gen_ru(profile_id: &str) -> &'static str {
    match profile_id {
        "dentist" | "clinic" | "vet" => "клиники",
        "beauty" | "hairdresser" => "салона",
        "restaurant" => "ресторана",
        "cafe" => "кафе",
        "florist" | "furniture" | "hardware" | "optician" => "магазина",
        "fitness" => "студии",
        _ => "компании",
    }
}

/// Что нужно знать о компании, чтобы написать ей первое сообщение.
pub struct OutreachInput<'a> {
    /// Название компании.
    pub business: &'a str,
    pub category: Option<&'a str>,
    /// Пресет категории скана (`dentist`, `restaurant`…).
    pub preset: Option<&'a str>,
    pub country: Option<&'a str>,
    /// Города скана: местное, русское и английское названия.
    pub cities: &'a [String],
    /// Адрес сайта без схемы: `belodent.by`.
    pub host: Option<&'a str>,
    pub state: SiteState,
    pub results: &'a [MetricResult],
    pub manager: &'a Manager,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Outreach {
    /// `ru` или `en`.
    pub lang: &'static str,
    /// Короткий текст для мессенджера.
    pub short: String,
    pub email_subject: String,
    pub email_body: String,
}

#[derive(Clone, Copy, PartialEq)]
enum Lang {
    Ru,
    En,
}

/// Русский — для Беларуси, России, Казахстана, Украины, Узбекистана, Киргизии и без страны;
/// для остальных стран — русский только у городов с русским названием в скане, иначе английский.
pub fn outreach_lang(country: Option<&str>, cities: &[String]) -> &'static str {
    match lang_of(country, cities) {
        Lang::Ru => "ru",
        Lang::En => "en",
    }
}

fn lang_of(country: Option<&str>, cities: &[String]) -> Lang {
    match country.map(str::to_ascii_lowercase).as_deref() {
        None | Some("") | Some("by" | "ru" | "kz" | "ua" | "uz" | "kg") => Lang::Ru,
        Some("md" | "am" | "az" | "ge" | "lv" | "lt" | "ee" | "tj" | "tm") if cities.iter().any(|c| is_plain_russian(c)) => Lang::Ru,
        _ => Lang::En,
    }
}

fn is_cyrillic(s: &str) -> bool {
    s.chars().any(|c| ('\u{400}'..='\u{4ff}').contains(&c))
}

/// Русское название без белорусских и украинских букв: «Минск», а не «Мінск».
fn is_plain_russian(s: &str) -> bool {
    is_cyrillic(s) && !s.chars().any(|c| "іїєґўІЇЄҐЎ".contains(c))
}

#[derive(Clone, Copy)]
enum Who {
    Patients,
    Clients,
    Guests,
}

impl Who {
    fn ru(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Who::Patients => ("пациенты", "пациентов", "пациентам"),
            Who::Clients => ("клиенты", "клиентов", "клиентам"),
            Who::Guests => ("гости", "гостей", "гостям"),
        }
    }
    fn en(self) -> &'static str {
        match self {
            Who::Patients => "patients",
            Who::Clients => "customers",
            Who::Guests => "guests",
        }
    }
}

/// Кому пишем: «для клиник» / «пациентов» и пример запроса.
struct Profile {
    id: &'static str,
    noun_ru: &'static str,
    noun_en: &'static str,
    for_ru: &'static str,
    for_en: &'static str,
    who: Who,
}

const PROFILES: &[Profile] = &[
    Profile { id: "dentist", noun_ru: "стоматолог", noun_en: "dentist", for_ru: "клиник", for_en: "dental practices", who: Who::Patients },
    Profile { id: "clinic", noun_ru: "клиника", noun_en: "clinic", for_ru: "клиник", for_en: "clinics", who: Who::Patients },
    Profile { id: "vet", noun_ru: "ветеринар", noun_en: "vet", for_ru: "ветклиник", for_en: "veterinary clinics", who: Who::Clients },
    Profile { id: "beauty", noun_ru: "салон красоты", noun_en: "beauty salon", for_ru: "салонов красоты", for_en: "beauty salons", who: Who::Clients },
    Profile { id: "hairdresser", noun_ru: "парикмахерская", noun_en: "hair salon", for_ru: "салонов красоты", for_en: "hair salons", who: Who::Clients },
    Profile { id: "fitness", noun_ru: "фитнес-клуб", noun_en: "gym", for_ru: "фитнес-клубов", for_en: "gyms", who: Who::Clients },
    Profile { id: "car_repair", noun_ru: "автосервис", noun_en: "car repair", for_ru: "автосервисов", for_en: "auto repair shops", who: Who::Clients },
    Profile { id: "tyres", noun_ru: "шиномонтаж", noun_en: "tire shop", for_ru: "автосервисов", for_en: "auto shops", who: Who::Clients },
    Profile { id: "car_wash", noun_ru: "автомойка", noun_en: "car wash", for_ru: "автомоек", for_en: "car washes", who: Who::Clients },
    Profile { id: "cafe", noun_ru: "кафе", noun_en: "cafe", for_ru: "ресторанов и кафе", for_en: "restaurants and cafes", who: Who::Guests },
    Profile { id: "restaurant", noun_ru: "ресторан", noun_en: "restaurant", for_ru: "ресторанов и кафе", for_en: "restaurants", who: Who::Guests },
    Profile { id: "hotel", noun_ru: "гостиница", noun_en: "hotel", for_ru: "отелей", for_en: "hotels", who: Who::Guests },
    Profile { id: "lawyer", noun_ru: "юрист", noun_en: "lawyer", for_ru: "юристов", for_en: "law firms", who: Who::Clients },
    Profile { id: "notary", noun_ru: "нотариус", noun_en: "notary", for_ru: "нотариусов", for_en: "notaries", who: Who::Clients },
    Profile { id: "estate_agent", noun_ru: "агентство недвижимости", noun_en: "estate agent", for_ru: "агентств недвижимости", for_en: "estate agents", who: Who::Clients },
    Profile { id: "florist", noun_ru: "цветы", noun_en: "florist", for_ru: "цветочных магазинов", for_en: "florists", who: Who::Clients },
    Profile { id: "electronics_repair", noun_ru: "ремонт телефонов", noun_en: "phone repair", for_ru: "сервисных центров", for_en: "repair shops", who: Who::Clients },
    Profile { id: "dry_cleaning", noun_ru: "химчистка", noun_en: "dry cleaner", for_ru: "химчисток", for_en: "dry cleaners", who: Who::Clients },
    Profile { id: "optician", noun_ru: "оптика", noun_en: "optician", for_ru: "оптик", for_en: "opticians", who: Who::Clients },
    Profile { id: "language_school", noun_ru: "языковая школа", noun_en: "language school", for_ru: "школ", for_en: "language schools", who: Who::Clients },
    Profile { id: "driving_school", noun_ru: "автошкола", noun_en: "driving school", for_ru: "автошкол", for_en: "driving schools", who: Who::Clients },
    Profile { id: "furniture", noun_ru: "мебель", noun_en: "furniture store", for_ru: "мебельных магазинов", for_en: "furniture stores", who: Who::Clients },
    Profile { id: "hardware", noun_ru: "стройматериалы", noun_en: "hardware store", for_ru: "магазинов стройматериалов", for_en: "hardware stores", who: Who::Clients },
];

/// Бизнесу есть смысл обещать «записаться/позвонить»: приём, услуга по записи, столик, номер.
/// Магазинам, издательствам и прочим компаниям без профиля такого не обещаем.
fn takes_bookings(p: &Profile) -> bool {
    !p.id.is_empty() && !matches!(p.id, "florist" | "furniture" | "hardware" | "dry_cleaning" | "electronics_repair")
}

const GENERIC: Profile = Profile { id: "", noun_ru: "", noun_en: "", for_ru: "бизнеса", for_en: "local businesses", who: Who::Clients };

/// Пресет по тексту категории — для ручных проверок без пресета.
fn preset_by_category(category: &str) -> Option<&'static str> {
    let c = category.to_lowercase();
    const RULES: &[(&str, &[&str])] = &[
        ("dentist", &["стомат", "dent"]),
        ("vet", &["ветер", "veterin"]),
        ("clinic", &["клиник", "медиц", "врач", "clinic", "doctor", "hospital", "medical"]),
        ("hairdresser", &["парикм", "барбер", "hair", "barber"]),
        ("beauty", &["салон", "красот", "beauty", "nail", "spa"]),
        ("fitness", &["фитнес", "тренаж", "fitness", "gym"]),
        ("car_wash", &["мойк", "car wash"]),
        ("tyres", &["шиномонт", "tire", "tyre"]),
        ("car_repair", &["автосервис", "автомастер", "car repair", "auto repair", "mechanic"]),
        ("cafe", &["кафе", "кофейн", "cafe", "coffee", "bakery", "пекарн"]),
        ("restaurant", &["ресторан", "пицц", "restaurant", "pizza", "bar", "pub"]),
        ("hotel", &["гостиниц", "отель", "хостел", "hotel", "hostel", "guest house"]),
        ("lawyer", &["юрист", "юрид", "адвокат", "lawyer", "attorney", "solicitor"]),
        ("notary", &["нотариус", "notary"]),
        ("estate_agent", &["недвижим", "real estate", "estate agent"]),
    ];
    // Короткие английские ключи («bar», «pub», «spa», «gym») — только целыми словами: «publisher» — не паб.
    let words: Vec<&str> = c.split(|ch: char| !ch.is_alphanumeric()).collect();
    let hit = |k: &str| if k.len() <= 3 && k.is_ascii() { words.iter().any(|w| *w == k || (w.len() == k.len() + 1 && w.starts_with(k) && w.ends_with('s'))) } else { c.contains(k) };
    RULES.iter().find(|(_, keys)| keys.iter().any(|k| hit(k))).map(|(id, _)| *id)
}

fn profile(preset: Option<&str>, category: Option<&str>) -> &'static Profile {
    let id = preset.filter(|p| PROFILES.iter().any(|x| x.id == *p)).or_else(|| category.and_then(preset_by_category));
    id.and_then(|id| PROFILES.iter().find(|p| p.id == id)).unwrap_or(&GENERIC)
}

/// Город для примера запроса: «Минск», а не «Мінск» и не район; для английского — латиницей.
fn query_city(cities: &[String], lang: Lang) -> Option<String> {
    let c = cities.iter().map(|c| c.trim()).filter(|c| !c.is_empty());
    match lang {
        Lang::Ru => c.clone().find(|c| is_plain_russian(c)).or_else(|| c.clone().find(|c| is_cyrillic(c))).map(str::to_string),
        Lang::En => c.clone().find(|c| c.is_ascii()).map(|c| c.trim_start_matches("City of ").to_string()),
    }
}

/// Пример запроса клиента: «стоматолог Минск».
fn query_example(p: &Profile, cities: &[String], lang: Lang) -> Option<String> {
    let noun = if lang == Lang::Ru { p.noun_ru } else { p.noun_en };
    let city = query_city(cities, lang)?;
    (!noun.is_empty()).then(|| format!("{noun} {city}"))
}

/// Кандидат в список проблем: ранг и тексты на двух языках.
struct Bullet {
    rank: f64,
    ru: String,
    en: String,
}

fn fail<'a>(results: &'a [MetricResult], id: &str) -> Option<&'a MetricResult> {
    results.iter().find(|r| r.id == id && r.status == Status::Fail)
}

/// Вес проблемы для сортировки: стоп-факторы выше, затем по весу метрики.
fn rank_of(ids: &[&str], results: &[MetricResult]) -> f64 {
    ids.iter()
        .filter(|id| fail(results, id).is_some())
        .filter_map(|id| catalog().get(id))
        .map(|d| if d.stop_factor { 1000.0 } else { 0.0 } + d.weight)
        .fold(0.0, f64::max)
}

fn any_fail(results: &[MetricResult], ids: &[&str]) -> bool {
    ids.iter().any(|id| fail(results, id).is_some())
}

/// «15,1 МБ» → «около 15 МБ».
fn megabytes(detail: &str) -> Option<String> {
    let num = detail.trim().strip_suffix("МБ")?.trim().replace(',', ".");
    let v: f64 = num.parse().ok()?;
    Some(format!("около {} МБ", v.round().max(1.0)))
}

/// Главные подтверждённые проблемы простыми словами — по две формулировки на проблему.
/// Предупреждения и непроверенное сюда не попадают: клиенту пишем только о том, в чём уверены.
fn bullets(results: &[MetricResult], q: Option<&str>, books: bool) -> Vec<Bullet> {
    let mut v: Vec<Bullet> = Vec::new();
    let mut add = |rank: f64, ru: String, en: String| v.push(Bullet { rank, ru, en });
    let (ru_q, en_q) = match q {
        Some(q) => (format!(" по запросам вроде «{q}»"), format!(" for searches like “{q}”")),
        None => (String::new(), String::new()),
    };

    // HTTPS.
    let https = ["B04", "B05"];
    if any_fail(results, &https) {
        let b04 = fail(results, "B04");
        let rank = rank_of(&https, results);
        let value = fail(results, "B05").and_then(|r| r.value.as_ref()).and_then(|x| x.as_str()).unwrap_or("");
        if b04.is_some_and(|r| r.detail.starts_with("Сертификат") || r.detail.starts_with("HTTPS не работает")) {
            add(
                rank,
                "браузер предупреждает о небезопасном соединении: у сайта проблема с сертификатом".into(),
                "the browser warns that the connection is not secure: there is a problem with the site's certificate".into(),
            );
        } else if b04.is_some() {
            add(rank, "браузер пишет «Не защищено»: на сайте нет HTTPS".into(), "the browser says “Not secure”: the site has no HTTPS".into());
        } else if value.starts_with("no_redirect") {
            add(
                rank,
                "браузер пишет «Не защищено»: сертификат у вас есть, но нет перенаправления на https".into(),
                "the browser says “Not secure”: you have a certificate, but there is no redirect to https".into(),
            );
        } else {
            add(rank, "браузер пишет «Не защищено»: нет перенаправления на https".into(), "the browser says “Not secure”: there is no redirect to https".into());
        }
    }

    // Скорость.
    let speed = ["D08", "D01", "D02", "D05", "D06", "D07", "B08"];
    if any_fail(results, &speed) {
        let rank = rank_of(&speed, results);
        match fail(results, "D08").and_then(|r| megabytes(&r.detail)) {
            Some(mb) => add(
                rank,
                format!("главная весит {mb}, с мобильного интернета грузится долго"),
                format!("the home page weighs {}, so it loads slowly on mobile data", mb.replace("около", "about")),
            ),
            None => add(
                rank,
                "сайт медленно грузится с телефона — многие уходят, не дождавшись".into(),
                "the site loads slowly on a phone, and many visitors leave before it opens".into(),
            ),
        }
    }

    // Мобильная версия.
    let mobile = ["E01", "E02", "E03", "E04", "E05", "E06", "E07"];
    if any_fail(results, &mobile) {
        let rank = rank_of(&mobile, results);
        if fail(results, "E01").is_some() {
            add(rank, "у сайта нет мобильной версии, а большинство людей ищут с телефона".into(), "the site has no mobile version, and most people search on a phone".into());
        } else {
            add(rank, "на телефоне сайтом неудобно пользоваться".into(), "the site is awkward to use on a phone".into());
        }
    }

    // Закрыт от поисковиков.
    let blocked = ["C02", "C10"];
    if any_fail(results, &blocked) {
        add(
            rank_of(&blocked, results),
            "сайт закрыт от индексации, поэтому поисковики его не показывают".into(),
            "the site is blocked from indexing, so search engines don't show it".into(),
        );
    }

    // Карта сайта и разметка.
    // Разметки нет совсем (G01) — не то же, что разметка есть, а данных о компании в ней нет (только G02).
    let (sitemap, markup, company_data) = (any_fail(results, &["C04", "C06"]), fail(results, "G01").is_some(), fail(results, "G02").is_some());
    if sitemap || markup || company_data {
        let rank = rank_of(&["C04", "C06", "G01", "G02"], results);
        let (ru_what, en_what) = match (sitemap, markup, company_data) {
            (true, true, _) => ("нет карты сайта и разметки для поисковиков", "there is no sitemap or search markup"),
            (true, false, true) => ("нет карты сайта, а в разметке нет данных о компании", "there is no sitemap, and the markup has no business data"),
            (true, false, false) => ("нет карты сайта для поисковиков", "there is no sitemap for search engines"),
            (false, true, _) => ("нет разметки для поисковиков", "there is no search markup"),
            _ => ("в разметке сайта нет данных о компании: адреса, часов, телефона", "the site's markup has no business data: address, hours, phone"),
        };
        add(rank, format!("{ru_what}, поэтому вас хуже находят{ru_q}"), format!("{en_what}, so you are harder to find{en_q}"));
    }

    // Заголовки под «услуга + город».
    if fail(results, "F06").is_some() {
        let rank = rank_of(&["F06"], results);
        add(
            rank,
            format!("в заголовках сайта нет связки «услуга + город», поэтому{} вас не показывают", if q.is_some() { &ru_q[..] } else { " по таким запросам" }),
            format!("the headings never pair a service with a city, so you don't show up{}", if q.is_some() { &en_q[..] } else { " for those searches" }),
        );
    } else if fail(results, "F01").is_some() {
        add(
            rank_of(&["F01"], results),
            "заголовок страницы не говорит, чем вы занимаетесь и где".into(),
            "the page title doesn't say what you do or where".into(),
        );
    }

    // Заявки (только бизнесу, которому есть смысл записываться или звонить).
    if !books {
    } else if fail(results, "N04").is_some() {
        add(rank_of(&["N04"], results), "нет онлайн-записи — записаться можно только по телефону".into(), "there is no online booking — the only way to book is by phone".into());
    } else if any_fail(results, &["N01", "N02"]) {
        add(
            rank_of(&["N01", "N02"], results),
            "на первом экране не видно, как записаться или позвонить".into(),
            "it isn't obvious on the first screen how to book or call".into(),
        );
    }

    // Документы.
    if fail(results, "M01").is_some() {
        add(rank_of(&["M01"], results), "нет политики обработки персональных данных — это риск штрафа".into(), "there is no privacy policy — a legal risk".into());
    } else if fail(results, "M03").is_some() {
        add(rank_of(&["M03"], results), "нет уведомления о cookie — это риск штрафа".into(), "there is no cookie notice — a legal risk".into());
    }

    // ИИ-поиск.
    let ai_blocked = ["I02", "I03"];
    if any_fail(results, &ai_blocked) {
        // Называем только тех, кого настройки реально закрывают.
        let yandex = fail(results, "I03").is_some();
        let ru_list = mini::ai_blocked_services(fail(results, "I02"), yandex, false);
        let en_list = mini::ai_blocked_services(fail(results, "I02"), yandex, true);
        let (ru, en) = if fail(results, "I02").is_none() {
            ("сайт закрыт для ответов Яндекса с ИИ (Алиса, Нейро), поэтому вас там не рекомендуют".to_string(), "the site's settings block Yandex's AI answers, so they can't show the site in their answers".to_string())
        } else if ru_list.is_empty() {
            ("сайт закрыт для ИИ-поиска, поэтому вас там не рекомендуют".to_string(), "the site's settings block AI search, so those assistants can't show the site in their answers".to_string())
        } else if ru_list.len() == 1 && !yandex {
            (
                format!("сайт закрыт для поиска {}, поэтому вас там не рекомендуют", ru_list[0]),
                format!("the site's settings block {} search, so it can't show the site in its answers", en_list[0]),
            )
        } else {
            (
                format!("сайт закрыт для ИИ-поиска ({}), поэтому вас там не рекомендуют", mini::join_list(&ru_list, false)),
                format!("the site's settings block AI search ({}), so those assistants can't show the site in their answers", mini::join_list(&en_list, true)),
            )
        };
        add(rank_of(&ai_blocked, results), ru, en);
    } else if let Some(r) = fail(results, "I06") {
        // «Пустая страница» — только когда без скриптов меньше 100 слов; иначе часть текста.
        let (ru, en) = if r.detail.contains("видна только часть текста") {
            (
                "без выполнения скриптов видна только часть текста сайта — остальное ИИ-ассистенты не прочитают",
                "without running scripts only part of the site's text is visible — AI assistants won't read the rest",
            )
        } else {
            ("сайт рисуется скриптами — ИИ-ассистенты видят пустую страницу", "the site is drawn by scripts — AI assistants see an empty page")
        };
        add(rank_of(&["I06"], results), ru.into(), en.into());
    } else if fail(results, "I08").is_some() {
        add(
            rank_of(&["I08"], results),
            "на сайте нет цен — клиенты не могут узнать, сколько стоят услуги".into(),
            "there are no prices on the site, so customers can't find out what things cost".into(),
        );
    }

    // Аналитика.
    if fail(results, "P01").is_some() {
        add(rank_of(&["P01"], results), "нет аналитики — не видно, сколько заявок приходит с сайта".into(), "there is no analytics — you can't see how many enquiries the site brings".into());
    }

    // Безопасность.
    if fail(results, "B13").is_some() {
        add(
            rank_of(&["B13"], results),
            "сайт в чёрных списках — браузеры и почта могут его блокировать".into(),
            "the site is on blacklists — browsers and email may block it".into(),
        );
    }
    if fail(results, "O04").is_some() {
        add(rank_of(&["O04"], results), "на сайте скрытые спам-ссылки — это признак взлома".into(), "the site has hidden spam links — a sign it was hacked".into());
    }

    // Контакты на сайте не совпадают с картами (без «не уверены, что это их сайт»).
    if fail(results, "H02").is_some_and(|r| r.value.as_ref().and_then(|x| x.as_str()) != Some("unverified")) {
        add(
            rank_of(&["H02"], results),
            "контакты на сайте не совпадают с данными на картах, из-за чего им меньше доверяют".into(),
            "the contacts on the site don't match the map listings, which lowers trust".into(),
        );
    }

    // Яндекс.
    let yandex = ["R11", "R12", "R13", "R14"];
    if any_fail(results, &yandex) {
        add(rank_of(&yandex, results), "на сайте приёмы, за которые Яндекс понижает в выдаче".into(), "the site uses tricks that search engines penalise".into());
    }

    if fail(results, "F13").is_some() {
        let year = fail(results, "F13").and_then(|r| r.value.as_ref()).and_then(|v| v.as_i64());
        let (ru, en) = match year {
            Some(y) => (format!("в подвале сайта указан {y} год — он выглядит давно не обновлённым"), format!("the site footer says {y} — it looks like it hasn't been updated for a long time")),
            None => ("сайт выглядит давно не обновлённым".to_string(), "the site looks like it hasn't been updated for a long time".to_string()),
        };
        add(rank_of(&["F13"], results), ru, en);
    }
    if fail(results, "C12").is_some() {
        add(rank_of(&["C12"], results), "на сайте есть битые ссылки".into(), "the site has broken links".into());
    }

    v.sort_by(|a, b| b.rank.partial_cmp(&a.rank).unwrap_or(std::cmp::Ordering::Equal));
    // Одна и та же фраза из разных метрик не повторяется в сообщении.
    let mut seen = HashSet::new();
    v.retain(|b| seen.insert(b.ru.clone()));
    v
}

fn join_contact_ru(m: &Manager) -> String {
    let mut parts = Vec::new();
    if !m.email.is_empty() {
        parts.push(m.email.clone());
    }
    if !m.telegram.is_empty() {
        parts.push(format!("Telegram {}", at_handle(&m.telegram)));
    }
    if !m.phone.is_empty() {
        parts.push(m.phone.clone());
    }
    match (parts.len(), m.email.is_empty()) {
        (0, _) => String::new(),
        (1, false) => format!("Почта для связи: {}", parts[0]),
        _ => format!("Для связи: {}", parts.join(", ")),
    }
}

fn join_contact_en(m: &Manager) -> String {
    let mut parts = Vec::new();
    if !m.email.is_empty() {
        parts.push(m.email.clone());
    }
    if !m.telegram.is_empty() {
        parts.push(format!("Telegram {}", at_handle(&m.telegram)));
    }
    if !m.phone.is_empty() {
        parts.push(m.phone.clone());
    }
    match (parts.len(), m.email.is_empty()) {
        (0, _) => String::new(),
        (1, false) => format!("Email: {}", parts[0]),
        _ => format!("Contact: {}", parts.join(", ")),
    }
}

fn at_handle(t: &str) -> String {
    let h = t.trim();
    if h.starts_with('@') || h.starts_with("http") || h.starts_with('+') { h.to_string() } else { format!("@{h}") }
}

/// Подпись письма: имя, компания и все заполненные контакты.
fn signature(m: &Manager, lang: Lang) -> String {
    let mut lines: Vec<String> = Vec::new();
    for s in [&m.name, &m.company, &m.email, &m.phone] {
        if !s.trim().is_empty() {
            lines.push(s.trim().to_string());
        }
    }
    if !m.telegram.trim().is_empty() {
        lines.push(format!("Telegram {}", at_handle(&m.telegram)));
    }
    if !m.portfolio.trim().is_empty() {
        lines.push(format!("{} {}", if lang == Lang::Ru { "Примеры работ:" } else { "Portfolio:" }, m.portfolio.trim()));
    }
    if lines.is_empty() {
        return String::new();
    }
    format!("{}\n{}", if lang == Lang::Ru { "С уважением," } else { "Best regards," }, lines.join("\n"))
}

/// Первое сообщение клиенту: короткое для мессенджера и письмо с темой и подписью.
/// None — писать не о чем: сайт не проверен, закрыт для сканера, это точка сети или компания закрыта.
pub fn outreach(i: &OutreachInput) -> Option<Outreach> {
    use SiteState::*;
    if matches!(i.state, Unknown | Blocked | Chain | Closed) {
        return Option::None;
    }
    let lang = lang_of(i.country, i.cities);
    let p = profile(i.preset, i.category);
    let q = query_example(p, i.cities, lang);
    let host = i.host.map(str::trim).filter(|h| !h.is_empty()).filter(|_| i.state != Social);
    let m = i.manager;
    let ru = lang == Lang::Ru;
    let (_, acc, dat) = p.who.ru();
    let en_who = p.who.en();
    let no_site = matches!(i.state, None | Social | Dead | Stub);

    let greeting = if ru { "Здравствуйте!" } else { "Hello!" };
    let intro = intro_line(m, p, ru);

    let books = takes_bookings(p);
    let body = if no_site {
        let (problem, offer) = no_site_text(i.state, host, ru, p, q.as_deref(), books);
        format!("{greeting} {intro} {problem}\n\n{offer}")
    } else {
        let list = site_problems(i, q.as_deref(), ru, books);
        let site = host.map(str::to_string);
        let good = i.state == Good;
        // Хорошему сайту не пишем «вы теряете»: он хороший, мешают отдельные вещи.
        let lead = match (ru, good, list.len()) {
            (true, _, 0) => format!("На {} в целом всё в порядке, но есть что улучшить, чтобы вас находило больше {acc}.", site.as_deref().unwrap_or("вашем сайте")),
            (true, true, 1) => format!("{} хороший, но есть одна вещь, которая мешает привлекать больше {acc}:", site.as_ref().map(|h| format!("Сайт {h}")).unwrap_or_else(|| "Ваш сайт".into())),
            (true, true, _) => format!("{} хороший, но есть несколько вещей, которые мешают привлекать больше {acc}:", site.as_ref().map(|h| format!("Сайт {h}")).unwrap_or_else(|| "Ваш сайт".into())),
            (true, false, 1) => format!("На {} нашлась проблема, из-за которой вы теряете {acc}:", site.as_deref().unwrap_or("вашем сайте")),
            (true, false, _) => format!("На {} нашлось несколько проблем, из-за которых вы теряете {acc}:", site.as_deref().unwrap_or("вашем сайте")),
            (false, _, 0) => format!("{} is in decent shape overall, but there is room to bring in more {en_who}.", site.as_deref().unwrap_or("Your website")),
            (false, true, 1) => format!("{} is good, but one thing is holding it back from bringing in more {en_who}:", site.as_deref().unwrap_or("Your website")),
            (false, true, _) => format!("{} is good, but a few things are holding it back from bringing in more {en_who}:", site.as_deref().unwrap_or("Your website")),
            (false, false, 1) => format!("I took a look at {} and found a problem that is costing you {en_who}:", site.as_deref().unwrap_or("your website")),
            (false, false, _) => format!("I took a look at {} and found a few problems that are costing you {en_who}:", site.as_deref().unwrap_or("your website")),
        };
        let items = list.iter().map(|b| format!("— {b}")).collect::<Vec<_>>().join("\n");
        let closing = match (ru, list.is_empty()) {
            (true, false) => "Подробный разбор прикрепляю. Могу всё исправить и провести SEO-оптимизацию. Если интересно, пришлю план и стоимость.",
            (true, true) => "Подробный разбор прикрепляю. Если интересно, пришлю план и стоимость.",
            (false, false) => "I'm attaching a detailed breakdown. I can fix everything and handle the SEO. If you're interested, I'll send a plan and a price.",
            (false, true) => "I'm attaching a detailed breakdown. If you're interested, I'll send a plan and a price.",
        };
        if list.is_empty() {
            format!("{greeting} {intro} {lead}\n\n{closing}")
        } else {
            format!("{greeting} {intro} {lead}\n\n{items}\n\n{closing}")
        }
    };

    let contact = if ru { join_contact_ru(m) } else { join_contact_en(m) };
    let short = if contact.is_empty() { body.clone() } else { format!("{body}\n\n{contact}") };
    let sign = signature(m, lang);
    let email_body = if sign.is_empty() { body } else { format!("{body}\n\n{sign}") };
    // Тема по названию компании, если сайта по факту нет (нет, соцсеть, не открывается, заглушка) —
    // адрес «instagram.com» или заглушки в теме письма клиенту ни к чему.
    // Название — в «ёлочках» без склонения (в английском — в обычных кавычках); свои кавычки из названия убираем.
    let title: String = i.business.chars().filter(|c| !matches!(c, '«' | '»' | '"' | '“' | '”')).collect::<String>().trim().to_string();
    // В русской теме перед названием — слово по категории в родительном падеже («для клиники «…»»), если название его уже не содержит.
    let kind = kind_gen_ru(p.id);
    let tl = title.to_lowercase();
    let kind_word = if ["клиник", "салон", "ресторан", "кафе", "магазин", "студи", "компани"].iter().any(|w| tl.starts_with(w)) { String::new() } else { format!("{kind} ") };
    let email_subject = match (ru, no_site) {
        (true, true) => format!("Сайт для {kind_word}«{title}» — как привлекать больше {acc}"),
        (true, false) => format!("Сайт {} — что мешает {dat} вас найти", host.unwrap_or(i.business)),
        (false, true) => format!("A website for \"{title}\" — how to bring in more {en_who}"),
        (false, false) => format!("{} — what's keeping {en_who} from finding you", host.unwrap_or(i.business)),
    };
    Some(Outreach { lang: if ru { "ru" } else { "en" }, short, email_subject, email_body })
}


/// Первая фраза о себе: своя из настроек или по категории («Я делаю сайты и SEO для клиник.»).
fn intro_line(m: &Manager, p: &Profile, ru: bool) -> String {
    if !m.intro.trim().is_empty() {
        m.intro.trim().to_string()
    } else if ru {
        format!("Я делаю сайты и SEO для {}.", p.for_ru)
    } else {
        format!("I build websites and do SEO for {}.", p.for_en)
    }
}

/// Факты для письма нейросетью — те же, что берёт шаблон: только подтверждённые проблемы и простыми словами.
#[derive(Debug, Clone, Serialize)]
pub struct LetterFacts {
    /// `ru` или `en`.
    pub lang: &'static str,
    /// Сайта по факту нет (нет, соцсеть, не открывается, заглушка).
    pub no_site: bool,
    /// Главные проблемы для клиента; без сайта — одна фраза о том, что сайта по факту нет.
    pub problems: Vec<String>,
    /// Что предложить, одной-двумя фразами.
    pub offer: String,
    /// Первая фраза о себе.
    pub intro: String,
    /// Пример поискового запроса: «стоматолог Минск».
    pub query: Option<String>,
    /// Город на языке письма: «Минск», а не «Мінск» из карточки; для английского — латиницей.
    pub city: Option<String>,
    /// Кого компания теряет: «пациентов», `patients`.
    pub audience: &'static str,
    /// Адрес сайта клиента без схемы (у соцсети — нет).
    pub host: Option<String>,
    /// Подпись письма: «С уважением,» и контакты из профиля.
    pub signature: String,
    /// Контакты одной строкой — для конца сообщения в мессенджер.
    pub contact_line: String,
    /// Сообщение и письмо по шаблону — образец для нейросети.
    pub template: Outreach,
}

/// Факты для письма нейросетью. None — писать не о чем (как у `outreach`).
pub fn letter_facts(i: &OutreachInput) -> Option<LetterFacts> {
    use SiteState::*;
    let template = outreach(i)?;
    let lang = lang_of(i.country, i.cities);
    let ru = lang == Lang::Ru;
    let p = profile(i.preset, i.category);
    let q = query_example(p, i.cities, lang);
    let host = i.host.map(str::trim).filter(|h| !h.is_empty()).filter(|_| i.state != Social);
    let no_site = matches!(i.state, None | Social | Dead | Stub);
    let books = takes_bookings(p);
    let (problems, offer) = if no_site {
        let (problem, offer) = no_site_text(i.state, host, ru, p, q.as_deref(), books);
        (vec![problem], offer)
    } else {
        let offer = if ru {
            "Могу всё исправить и провести SEO-оптимизацию. Если интересно, пришлю план и стоимость."
        } else {
            "I can fix everything and handle the SEO. If you're interested, I'll send a plan and a price."
        };
        (site_problems(i, q.as_deref(), ru, books), offer.to_string())
    };
    Some(LetterFacts {
        lang: template.lang,
        no_site,
        problems,
        offer,
        intro: intro_line(i.manager, p, ru),
        query: q,
        city: query_city(i.cities, lang).or_else(|| i.cities.iter().map(|c| c.trim()).find(|c| !c.is_empty()).map(str::to_string)),
        audience: if ru { p.who.ru().1 } else { p.who.en() },
        host: host.map(str::to_string),
        signature: signature(i.manager, lang),
        contact_line: if ru { join_contact_ru(i.manager) } else { join_contact_en(i.manager) },
        template,
    })
}

/// Проблемы работающего сайта для клиента (до четырёх): «сайт не в карточке» и главные подтверждённые провалы.
fn site_problems(i: &OutreachInput, q: Option<&str>, ru: bool, books: bool) -> Vec<String> {
    let mut list: Vec<String> = Vec::new();
    if i.state == SiteState::Hidden {
        list.push(
            if ru { "сайт не указан в карточках на картах, поэтому люди с карт на него не попадают" } else { "the website isn't listed on your map profiles, so people on the map never reach it" }
                .into(),
        );
    }
    list.extend(bullets(i.results, q, books).into_iter().map(|b| if ru { b.ru } else { b.en }));
    list.truncate(4);
    list
}

/// Сайта по факту нет (нет, соцсеть, не открывается, заглушка): проблема и предложение — по одной фразе.
fn no_site_text(state: SiteState, host: Option<&str>, ru: bool, p: &Profile, q: Option<&str>, books: bool) -> (String, String) {
    use SiteState::*;
    let (nom, _, _) = p.who.ru();
    let en_who = p.who.en();
    if ru {
        let what = if books { "посмотреть услуги и цены и записаться" } else { "узнать, чем вы занимаетесь, и связаться с вами" };
        let problem = match (state, host) {
            (None, _) => format!("Не нашлось сайта у вашей компании: {nom} с карт не находят сайт, чтобы {what}, и могут выбрать конкурентов."),
            (Social, _) => format!("Вместо сайта у вас страница в соцсети: с карт и из поиска людям трудно разобраться, чем вы занимаетесь, и {nom} могут выбрать конкурентов."),
            (Dead, Some(h)) => format!("Сайт {h}, указанный в вашей карточке, не открывается, по факту сайта у компании нет: {nom}, которые переходят с карт, видят ошибку и могут выбрать конкурентов."),
            (Dead, _) => format!("Сайт, указанный в вашей карточке, не открывается, по факту сайта у компании нет: {nom}, которые переходят с карт, видят ошибку и могут выбрать конкурентов."),
            (_, Some(h)) => format!("По адресу {h} вместо сайта сейчас заглушка, по факту сайта у компании нет: {nom}, которые заходят с карт, не видят ни услуг, ни цен и могут выбрать конкурентов."),
            _ => format!("Вместо сайта сейчас заглушка, по факту сайта у компании нет: {nom}, которые заходят с карт, не видят ни услуг, ни цен и могут выбрать конкурентов."),
        };
        let target = q.map(|q| format!("запросы вроде «{q}»")).unwrap_or_else(|| "поисковые запросы".into());
        let site_kind = if books { "сайт с услугами, ценами и онлайн-записью" } else { "сайт с услугами, ценами и контактами" };
        let offer = format!("Могу сделать {site_kind} и настроить его под {target}, чтобы вас находили в Google и Яндексе. Если интересно, пришлю примеры и стоимость.");
        (problem, offer)
    } else {
        let what = if books { "see your services or prices or book" } else { "see what you do or get in touch" };
        let problem = match (state, host) {
            (None, _) => format!("I couldn't find a website for your business: {en_who} who find you on the map can't find a website to {what}, and may choose a competitor."),
            (Social, _) => format!("Instead of a website you have a social media page: it's hard for {en_who} to see what you do from the map or search, and they may choose a competitor."),
            (Dead, Some(h)) => format!("The website {h} listed on your map profile doesn't open, so in practice you have no website: {en_who} coming from the map see an error and may choose a competitor."),
            (Dead, _) => format!("The website listed on your map profile doesn't open, so in practice you have no website: {en_who} coming from the map see an error and may choose a competitor."),
            (_, Some(h)) => format!("{h} currently shows a placeholder page instead of a website, so in practice you have no website: {en_who} coming from the map see no services or prices and may choose a competitor."),
            _ => format!("Your website currently shows a placeholder page, so in practice you have no website: {en_who} coming from the map see no services or prices and may choose a competitor."),
        };
        let target = q.map(|q| format!("searches like “{q}”")).unwrap_or_else(|| "local searches".into());
        let site_kind = if books { "a website with services, prices and online booking" } else { "a website with your services, prices and contact details" };
        let offer = format!("I can build {site_kind}, optimised for {target}, so people find you on Google. If you're interested, I'll send examples and a price.");
        (problem, offer)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn phone_grouped_unifies_grouping() {
        assert_eq!(phone_grouped("+375 29 6371226", Some("by")), "+375 29 637-12-26");
        assert_eq!(phone_grouped("+375 (29) 391-40-48", Some("by")), "+375 29 391-40-48");
        assert_eq!(phone_grouped("+375172345003", Some("by")), "+375 17 234-50-03");
    }

    use super::*;
    use crate::model::Place;
    use crate::page;

    fn r(id: &str, s: Status, detail: &str) -> MetricResult {
        MetricResult::new(id, s, detail)
    }

    #[test]
    fn site_contacts_from_real_markup() {
        let html = r#"<html><head><title>Белодент</title></head><body>
            <header><a href="tel:+375291234567">+375 29 123-45-67</a></header>
            <p>Минск, ул. Притыцкого 29. Тел. +375 (17) 345-67-89, <a href="mailto:Info@belodent.by?subject=hi">info@belodent.by</a></p>
            <p>Запись: reg@belodent.by</p>
            <img src="/img/logo@2x.png"><a href="mailto:bug@sentry.io">x</a><a href="mailto:u123@sentry-next.wixpress.com">y</a>
            <a href="https://www.instagram.com/belodent.by/">inst</a>
            <a href="https://instagram.com/belodent.by">inst</a>
            <a href="https://t.me/belodent_minsk">tg</a>
            <a href="https://wa.me/375291234567">wa</a>
            <a href="https://www.facebook.com/sharer/sharer.php?u=x">share</a>
            <a href="https://t.me/share/url?url=x">share</a>
            <a href="https://youtu.be/dQw4w9WgXcQ">v</a>
            <a href="https://www.youtube.com/watch?v=abc">v</a>
            <a href="https://www.youtube.com/shorts/abc">v</a>
            <a href="https://www.instagram.com/p/Cxyz/">post</a>
            <a href="https://www.instagram.com/reel/Cxyz/">reel</a>
            <a href="https://www.instagram.com/stories/belodent/123/">story</a>
            <a href="https://vk.com/share.php?url=x">s</a>
            <a href="https://wa.me/?text=hello">s</a>
            <a href="https://twitter.com/intent/tweet?text=x">s</a>
            <a href="https://pinterest.com/pin/create/button/?url=x">s</a>
            <a href="https://www.youtube.com/@belodent">yt</a>
            <a href="https://www.youtube.com/channel/UC123">yt</a>
            <a href="https://www.youtube.com/c/belodent">yt</a>
            <a href="https://www.youtube.com/user/belodent">yt</a>
            <a href="https://www.facebook.com/belodent">fb</a>
            <a href="https://bit.ly/abc">short</a>
            <a href="https://yandex.by/maps/org/1">maps</a>
            </body></html>"#;
        let p = page::extract(html, "https://belodent.by/");
        let c = contacts_from_pages(std::iter::once(&p));
        assert_eq!(c.emails, vec!["Info@belodent.by", "reg@belodent.by"]);
        assert_eq!(c.phones.len(), 2, "{:?}", c.phones);
        assert_eq!(c.phones[0], "+375 29 123-45-67", "красивая запись из текста, а не из tel:");
        assert_eq!(c.phones[1], "+375 (17) 345-67-89");
        let kinds: Vec<&str> = c.links.iter().map(|l| l.kind.as_str()).collect();
        assert_eq!(kinds, vec!["instagram", "telegram", "whatsapp", "youtube", "youtube", "youtube", "youtube", "facebook"], "{:?}", c.links);
    }

    #[test]
    fn contact_list_merges_sources_without_duplicates() {
        let card = Business {
            name: "Белодент".into(),
            phone: Some("+375 17 345-67-89".into()),
            website: Some("https://belodent.by/".into()),
            sources: vec![Place {
                phone: Some("8 017 345-67-89".into()),
                email: Some("info@belodent.by".into()),
                links: vec!["https://www.instagram.com/belodent.by/".into(), "https://belodent.by/".into()],
                ..Default::default()
            }],
            ..Default::default()
        };
        let site = Contacts {
            emails: vec!["INFO@belodent.by".into(), "reg@belodent.by".into()],
            phones: vec!["+375 (17) 345-67-89".into(), "+375 29 123-45-67".into()],
            links: vec![
                SiteLink { kind: "instagram".into(), url: "https://instagram.com/belodent.by".into() },
                SiteLink { kind: "telegram".into(), url: "https://t.me/belodent_minsk".into() },
            ],
        };
        let manual = vec![
            ManualContact { kind: "whatsapp".into(), value: "+375 29 123-45-67".into(), note: Some("директор".into()) },
            ManualContact { kind: "email".into(), value: "reg@belodent.by".into(), note: None },
        ];
        let list = contact_list(&card, &site, &manual, Some("by"));
        let view: Vec<(&str, &str)> = list.iter().map(|c| (c.kind.as_str(), c.source)).collect();
        assert_eq!(
            view,
            vec![("phone", "card"), ("email", "card"), ("instagram", "card"), ("phone", "site"), ("email", "site"), ("telegram", "site"), ("whatsapp", "manual")]
        );
        assert_eq!(list[0].url.as_deref(), Some("tel:+375173456789"));
        assert_eq!(list[1].url.as_deref(), Some("mailto:info@belodent.by"));
        assert_eq!(list[6].url.as_deref(), Some("https://wa.me/375291234567"));
        assert_eq!(list[6].note.as_deref(), Some("директор"));
    }

    #[test]
    fn contact_urls() {
        assert_eq!(contact_url("telegram", "@belodent").as_deref(), Some("https://t.me/belodent"));
        assert_eq!(contact_url("telegram", "+375 29 111-22-33").as_deref(), Some("https://t.me/+375291112233"));
        assert_eq!(contact_url("viber", "+375291112233").as_deref(), Some("viber://chat?number=%2B375291112233"));
        assert_eq!(contact_url("instagram", "@belodent.by").as_deref(), Some("https://www.instagram.com/belodent.by"));
        assert_eq!(contact_url("other", "belodent.by").as_deref(), Some("https://belodent.by"));
        assert_eq!(contact_url("other", "спросить Анну"), None);
    }

    /// Белый сайт с HTTPS без перенаправления и без разметки: https и schema включены.
    #[test]
    fn proposal_for_site_with_https_redirect_and_markup_problems() {
        let results = vec![
            r("B04", Status::Pass, ""),
            r("B05", Status::Fail, "HTTP не перенаправляет на HTTPS").with_value("no_redirect"),
            r("G01", Status::Fail, "нет JSON-LD"),
            r("G03", Status::Warn, "нет sameAs"),
            r("D08", Status::Warn, "15,1 МБ"),
            r("E01", Status::Pass, ""),
            r("A05", Status::Fail, "рейтинг 3.1"),
            r("B11", Status::Fail, "kernel_main.css?1: без Cache-Control"),
        ];
        let p = build_proposal(&results, SiteState::Good, &default_price_list(), &SavedProposal::default(), "ru");
        let on: Vec<&str> = p.packages.iter().filter(|x| x.enabled).map(|x| x.id.as_str()).collect();
        assert_eq!(on, vec!["https", "speed", "schema"], "speed включён из-за B11");
        let https = p.packages.iter().find(|x| x.id == "https").unwrap();
        assert_eq!(https.items[0].text, "Нет перенаправления на https — по ссылке из карточки браузер пишет «Не защищено»");
        let schema = p.packages.iter().find(|x| x.id == "schema").unwrap();
        assert_eq!(schema.items.iter().map(|i| i.level).collect::<Vec<_>>(), vec!["fail", "warn"]);
        let speed = p.packages.iter().find(|x| x.id == "speed").unwrap();
        assert!(speed.items.iter().all(|i| !i.text.contains(".css")));
        assert!(p.packages.iter().find(|x| x.id == "maps").unwrap().items.is_empty(), "рейтинг — не работа менеджера");
        assert_eq!(p.total, 60.0 + 150.0 + 120.0);
        assert_eq!(p.valid_days, 14);
    }

    #[test]
    fn proposal_edits_extra_lines_and_discount() {
        let results = vec![r("B05", Status::Fail, "").with_value("no_redirect"), r("G01", Status::Fail, "")];
        let mut pl = default_price_list();
        pl.iter_mut().find(|x| x.id == "https").unwrap().price = 80.0;
        pl.iter_mut().find(|x| x.id == "a11y").unwrap().enabled = false;
        let saved = SavedProposal::from_value(&serde_json::json!({
            "packages": [{"id": "schema", "enabled": false}, {"id": "speed", "enabled": true, "price": 175.5, "title": "Ускорение"}],
            "extra": [{"title": "Фотосъёмка", "price": 200}, {"title": " ", "price": 5}],
            "discount": 50, "note": "Оплата 50/50", "valid_days": 30
        }));
        let p = build_proposal(&results, SiteState::Fair, &pl, &saved, "ru");
        assert!(p.packages.iter().all(|x| x.id != "a11y"), "выключенный в прайсе пакет не предлагается");
        assert_eq!(p.packages.iter().find(|x| x.id == "https").unwrap().price, 80.0);
        assert!(!p.packages.iter().find(|x| x.id == "schema").unwrap().enabled);
        let speed = p.packages.iter().find(|x| x.id == "speed").unwrap();
        assert_eq!((speed.enabled, speed.default_enabled, speed.title.as_str()), (true, false, "Ускорение"));
        assert_eq!(p.extra.len(), 1);
        assert_eq!((p.subtotal, p.discount, p.total), (80.0 + 175.5 + 200.0, 50.0, 405.5));
        assert_eq!((p.note.as_str(), p.valid_days), ("Оплата 50/50", 30));
        // Скидка не больше суммы.
        let big = SavedProposal { discount: Some(1e6), ..Default::default() };
        assert_eq!(build_proposal(&results, SiteState::Fair, &pl, &big, "ru").total, 0.0);
    }

    #[test]
    fn proposal_without_site_has_only_new_site_and_maps() {
        let results = vec![r("A03", Status::Fail, "")];
        let p = build_proposal(&results, SiteState::None, &default_price_list(), &SavedProposal::default(), "ru");
        assert_eq!(p.packages.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), vec!["new_site", "maps"]);
        assert!(p.packages.iter().all(|x| x.enabled));
        assert_eq!(p.packages[0].items[0].id, "site");
        assert_eq!(p.packages[1].items[0].text, "В карточке не указан сайт");
        assert_eq!(p.total, 900.0 + 100.0);
        // Сайт есть, но не в карточке: обычные пакеты по сайту тоже на месте.
        let hidden = build_proposal(&[r("A03", Status::Fail, ""), r("B05", Status::Fail, "")], SiteState::Hidden, &default_price_list(), &SavedProposal::default(), "ru");
        assert!(hidden.packages.len() > 2 && hidden.packages.iter().find(|x| x.id == "https").unwrap().enabled);
    }

    #[test]
    fn price_list_normalization() {
        let partial = vec![PriceItem { id: "https".into(), title: " ".into(), price: -5.0, enabled: false }, PriceItem { id: "bogus".into(), title: "x".into(), price: 1.0, enabled: true }];
        let n = normalize_price_list(&partial);
        assert_eq!(n.len(), PACKAGES.len());
        let https = n.iter().find(|x| x.id == "https").unwrap();
        assert_eq!((https.price, https.enabled, https.title.as_str()), (60.0, false, "HTTPS и перенаправление"));
    }

    #[test]
    fn every_catalog_metric_is_routed_or_skipped_on_purpose() {
        // Без пакета: B01 (сайт не открывается — решает new_site), B03 (возраст домена) и внешние
        // метрики K (позиции, трафик) и L (ссылки) — это не работа над сайтом.
        let unrouted: Vec<&str> = catalog().metrics.iter().map(|m| m.id.as_str()).filter(|id| package_of(id).is_none()).collect();
        assert!(unrouted.iter().all(|id| matches!(id.chars().next(), Some('K' | 'L')) || ["B01", "B03"].contains(id)), "{unrouted:?}");
    }

    fn manager() -> Manager {
        Manager { email: "hello@example.com".into(), ..Default::default() }
    }

    fn belodent_results() -> Vec<MetricResult> {
        vec![
            r("B01", Status::Pass, ""),
            r("B04", Status::Pass, ""),
            r("B05", Status::Fail, "").with_value("no_redirect"),
            r("D08", Status::Fail, "15,1 МБ"),
            r("C04", Status::Fail, "нет Sitemap в robots.txt"),
            r("G01", Status::Fail, "нет JSON-LD"),
            r("P01", Status::Pass, ""),
            r("I08", Status::Warn, ""),
        ]
    }

    fn assert_no_gender(text: &str) {
        for w in text.split(|c: char| !c.is_alphabetic()) {
            let l = w.to_lowercase();
            let bad = ["нашёл", "нашел", "нашла", "посмотрел", "посмотрела", "заметил", "заметила", "изучил", "изучила", "увидел", "увидела", "проверил", "проверила"];
            assert!(!bad.contains(&l.as_str()), "гендерный глагол «{w}» в тексте:\n{text}");
        }
    }

    #[test]
    fn outreach_for_minsk_dentist_matches_approved_style() {
        let cities = vec!["Мінск".to_string(), "Minsk".to_string(), "Минск".to_string()];
        let m = manager();
        let out = outreach(&OutreachInput {
            business: "Любімы стаматолаг",
            category: Some("Стоматология"),
            preset: Some("dentist"),
            country: Some("by"),
            cities: &cities,
            host: Some("belodent.by"),
            // belodent.by по факту «средний» (74/100); у хорошего сайта текст мягче — см. outreach_for_good_site.
            state: SiteState::Fair,
            results: &belodent_results(),
            manager: &m,
        })
        .unwrap();
        assert_eq!(out.lang, "ru");
        let expected = "Здравствуйте! Я делаю сайты и SEO для клиник. На belodent.by нашлось несколько проблем, из-за которых вы теряете пациентов:\n\n\
— браузер пишет «Не защищено»: сертификат у вас есть, но нет перенаправления на https\n\
— главная весит около 15 МБ, с мобильного интернета грузится долго\n\
— нет карты сайта и разметки для поисковиков, поэтому вас хуже находят по запросам вроде «стоматолог Минск»\n\n\
Подробный разбор прикрепляю. Могу всё исправить и провести SEO-оптимизацию. Если интересно, пришлю план и стоимость.\n\n\
Почта для связи: hello@example.com";
        assert_eq!(out.short, expected);
        assert_eq!(out.email_subject, "Сайт belodent.by — что мешает пациентам вас найти");
        assert!(out.email_body.ends_with("С уважением,\nhello@example.com"), "{}", out.email_body);
        assert_no_gender(&out.short);
        assert_no_gender(&out.email_body);
    }

    #[test]
    fn outreach_signature_and_custom_intro() {
        let cities: Vec<String> = vec![];
        let m = Manager { name: "Никита".into(), email: "hello@example.com".into(), phone: "+375 29 000-00-00".into(), telegram: "nextry".into(), company: "NexTry".into(), intro: "Меня зовут Никита, делаю сайты для малого бизнеса.".into(), ..Default::default() };
        let out = outreach(&OutreachInput {
            business: "Кафе Уют",
            category: Some("Кафе"),
            preset: Some("cafe"),
            country: Some("by"),
            cities: &cities,
            host: Some("uyut.by"),
            state: SiteState::Fair,
            results: &[r("P01", Status::Fail, "")],
            manager: &m,
        })
        .unwrap();
        assert!(out.short.starts_with("Здравствуйте! Меня зовут Никита, делаю сайты для малого бизнеса. На uyut.by нашлась проблема, из-за которой вы теряете гостей:"), "{}", out.short);
        assert!(out.short.contains("— нет аналитики"));
        assert!(out.short.ends_with("Для связи: hello@example.com, Telegram @nextry, +375 29 000-00-00"));
        assert!(out.email_body.ends_with("С уважением,\nНикита\nNexTry\nhello@example.com\n+375 29 000-00-00\nTelegram @nextry"), "{}", out.email_body);
    }

    #[test]
    fn outreach_without_site_and_hidden_site() {
        let cities = vec!["Минск".to_string()];
        let m = manager();
        let mk = |state, host, results: &[MetricResult]| {
            outreach(&OutreachInput { business: "Салон Анна", category: Some("Салон красоты"), preset: Some("beauty"), country: Some("by"), cities: &cities, host, state, results, manager: &m }).unwrap()
        };
        let none = mk(SiteState::None, None, &[]);
        assert!(none.short.contains("Не нашлось сайта у вашей компании: клиенты с карт"), "{}", none.short);
        assert!(none.short.contains("запросы вроде «салон красоты Минск»"));
        assert_eq!(none.email_subject, "Сайт для «Салон Анна» — как привлекать больше клиентов");
        assert_no_gender(&none.short);
        let social = mk(SiteState::Social, None, &[]);
        assert!(social.short.contains("Вместо сайта у вас страница в соцсети"));
        let stub = mk(SiteState::Stub, Some("anna.by"), &[]);
        assert!(stub.short.contains("По адресу anna.by вместо сайта сейчас заглушка, по факту сайта у компании нет"), "{}", stub.short);
        assert_eq!(stub.email_subject, "Сайт для «Салон Анна» — как привлекать больше клиентов");
        let hidden = mk(SiteState::Hidden, Some("anna.by"), &[r("B05", Status::Fail, "").with_value("no_redirect")]);
        assert!(hidden.short.contains("— сайт не указан в карточках на картах"), "{}", hidden.short);
        assert!(hidden.short.contains("нашлось несколько проблем"));
        // Нечего сказать: не проверен, сеть, закрыта.
        for s in [SiteState::Unknown, SiteState::Blocked, SiteState::Chain, SiteState::Closed] {
            assert!(mk_none(s, &cities, &m));
        }
    }

    fn mk_none(state: SiteState, cities: &[String], m: &Manager) -> bool {
        outreach(&OutreachInput { business: "x", category: None, preset: None, country: Some("by"), cities, host: None, state, results: &[], manager: m }).is_none()
    }

    #[test]
    fn outreach_in_english_for_london_and_new_york() {
        let m = manager();
        let london = vec!["City of Westminster".to_string(), "Вестминстер".to_string(), "Westminster".to_string()];
        let results = vec![
            r("B04", Status::Fail, "Сайт работает без HTTPS"),
            r("D01", Status::Fail, ""),
            r("C06", Status::Fail, ""),
            r("N04", Status::Fail, ""),
            r("M03", Status::Fail, ""),
        ];
        let out = outreach(&OutreachInput {
            business: "The Crown Pub",
            category: Some("restaurant"),
            preset: Some("restaurant"),
            country: Some("gb"),
            cities: &london,
            host: Some("crownpub.co.uk"),
            state: SiteState::Weak,
            results: &results,
            manager: &m,
        })
        .unwrap();
        assert_eq!(out.lang, "en");
        assert!(out.short.starts_with("Hello! I build websites and do SEO for restaurants. I took a look at crownpub.co.uk and found a few problems that are costing you guests:\n\n— "), "{}", out.short);
        assert_eq!(out.short.matches("\n— ").count(), 4, "не больше четырёх: {}", out.short);
        assert!(out.short.contains("the browser says “Not secure”: the site has no HTTPS"));
        assert!(out.short.contains("I can fix everything and handle the SEO."));
        assert!(out.short.ends_with("Email: hello@example.com"));
        assert_eq!(out.email_subject, "crownpub.co.uk — what's keeping guests from finding you");
        assert!(!out.short.chars().any(|c| ('\u{400}'..='\u{4ff}').contains(&c)), "в английском тексте нет кириллицы");

        let ny = vec!["New York".to_string(), "Brooklyn".to_string(), "Бруклин".to_string()];
        let none = outreach(&OutreachInput {
            business: "Bella Hair",
            category: Some("hairdresser"),
            preset: Some("hairdresser"),
            country: Some("us"),
            cities: &ny,
            host: None,
            state: SiteState::None,
            results: &[],
            manager: &m,
        })
        .unwrap();
        assert!(none.short.contains("I couldn't find a website for your business: customers who find you on the map"), "{}", none.short);
        assert!(none.short.contains("searches like “hair salon New York”"));
        assert_eq!(none.email_subject, "A website for \"Bella Hair\" — how to bring in more customers");
    }

    #[test]
    fn language_choice() {
        assert_eq!(outreach_lang(Some("by"), &[]), "ru");
        assert_eq!(outreach_lang(None, &[]), "ru");
        assert_eq!(outreach_lang(Some("kz"), &[]), "ru");
        assert_eq!(outreach_lang(Some("gb"), &["Лондон".into()]), "en");
        assert_eq!(outreach_lang(Some("de"), &[]), "en");
        assert_eq!(outreach_lang(Some("lv"), &["Rīga".into(), "Рига".into()]), "ru");
        assert_eq!(outreach_lang(Some("lv"), &["Rīga".into()]), "en");
    }

    #[test]
    fn category_fallback_without_preset() {
        assert_eq!(profile(None, Some("Стоматологическая клиника")).id, "dentist");
        assert_eq!(profile(None, Some("Barber shop")).id, "hairdresser");
        assert_eq!(profile(None, Some("Магазин обуви")).id, "");
        assert_eq!(profile(None, Some("Book publisher")).id, "", "publisher — не паб");
        assert_eq!(profile(None, Some("Cocktail bar")).id, "restaurant");
        assert_eq!(profile(None, Some("Day spa")).id, "beauty");
        assert_eq!(profile(Some("unknown"), None).for_ru, "бизнеса");
    }

    #[allow(clippy::too_many_arguments)]
    fn mk_out<'a>(m: &'a Manager, cities: &'a [String], country: &'a str, preset: Option<&'a str>, category: &'a str, host: Option<&'a str>, state: SiteState, results: &'a [MetricResult]) -> Outreach {
        outreach(&OutreachInput { business: "Компания", category: Some(category), preset, country: Some(country), cities, host, state, results, manager: m }).unwrap()
    }

    /// minskdentist.by (11284): сайт хороший — без «вы теряете», мягко; разметки LocalBusiness нет, но микроданные есть.
    #[test]
    fn outreach_for_good_site_is_soft() {
        let cities = vec!["Мінск".to_string(), "Минск".to_string()];
        let m = manager();
        let results = vec![r("G01", Status::Pass, ""), r("G02", Status::Fail, "Разметки LocalBusiness нет"), r("C06", Status::Fail, ""), r("I01", Status::Warn, ""), r("P01", Status::Fail, "")];
        let out = mk_out(&m, &cities, "by", Some("dentist"), "Стоматология", Some("minskdentist.by"), SiteState::Good, &results);
        assert!(out.short.contains("Сайт minskdentist.by хороший, но есть несколько вещей, которые мешают привлекать больше пациентов:"), "{}", out.short);
        assert!(!out.short.contains("теряете"), "{}", out.short);
        assert!(out.short.contains("— нет карты сайта, а в разметке нет данных о компании"), "{}", out.short);
        assert_no_gender(&out.short);
        let one = mk_out(&m, &cities, "by", Some("dentist"), "Стоматология", Some("minskdentist.by"), SiteState::Good, &[r("P01", Status::Fail, "")]);
        assert!(one.short.contains("хороший, но есть одна вещь, которая мешает привлекать больше пациентов:"), "{}", one.short);
        // Нечего исправлять — прежняя мягкая фраза.
        let none = mk_out(&m, &cities, "by", Some("dentist"), "Стоматология", Some("minskdentist.by"), SiteState::Good, &[]);
        assert!(none.short.contains("в целом всё в порядке"), "{}", none.short);
    }

    /// belodent.by (11236): разметки нет вовсе — «нет разметки», а не «неполные данные».
    #[test]
    fn outreach_markup_wording_by_fact() {
        let cities = vec!["Минск".to_string()];
        let m = manager();
        let g02_only = mk_out(&m, &cities, "by", Some("dentist"), "", Some("x.by"), SiteState::Fair, &[r("G02", Status::Fail, "")]);
        assert!(g02_only.short.contains("в разметке сайта нет данных о компании: адреса, часов, телефона"), "{}", g02_only.short);
        let both = mk_out(&m, &cities, "by", Some("dentist"), "", Some("x.by"), SiteState::Fair, &[r("G01", Status::Fail, ""), r("G02", Status::Fail, "")]);
        assert!(both.short.contains("нет разметки для поисковиков"), "{}", both.short);
    }

    /// F13 в сообщении клиенту: год из подвала, без «заброшен».
    #[test]
    fn outreach_abandoned_site_says_footer_year() {
        let cities = vec!["Минск".to_string()];
        let m = manager();
        let res = [r("F13", Status::Fail, "© 2021 — сайт давно не обновлялся").with_value(2021)];
        let out = mk_out(&m, &cities, "by", Some("dentist"), "Стоматология", Some("a.by"), SiteState::Weak, &res);
        assert!(out.short.contains("в подвале сайта указан 2021 год — он выглядит давно не обновлённым"), "{}", out.short);
        assert!(!out.short.contains("заброшен"), "{}", out.short);
    }

    /// И05 при проваленном I02 не попадает в пункты КП.
    #[test]
    fn proposal_skips_waf_hint_when_ai_search_is_closed() {
        let res = vec![r("I02", Status::Fail, "Закрыты поисковые ИИ-боты: OAI-SearchBot").with_value("OAI-SearchBot, GPTBot"), r("I05", Status::Warn, "защита отказала")];
        let p = build_proposal(&res, SiteState::Weak, &default_price_list(), &SavedProposal::default(), "ru");
        let ids: Vec<&str> = p.packages.iter().flat_map(|pk| pk.items.iter().map(|i| i.id.as_str())).collect();
        assert!(ids.contains(&"I02") && !ids.contains(&"I05"), "{ids:?}");
        let only05 = build_proposal(&[r("I05", Status::Warn, "защита отказала")], SiteState::Weak, &default_price_list(), &SavedProposal::default(), "ru");
        assert!(only05.packages.iter().flat_map(|pk| pk.items.iter()).any(|i| i.id == "I05"));
    }

    /// Вместо сайта соцсеть (11264): тема письма с названием компании, не «Сайт instagram.com».
    #[test]
    fn outreach_for_social_page_uses_company_in_subject() {
        let cities = vec!["Минск".to_string()];
        let m = manager();
        let out = mk_out(&m, &cities, "by", Some("beauty"), "Салон красоты", Some("instagram.com"), SiteState::Social, &[]);
        assert_eq!(out.email_subject, "Сайт для «Компания» — как привлекать больше клиентов");
        let named = outreach(&OutreachInput { business: "Стоматология Альянс-Дент", category: Some("Стоматология"), preset: Some("dentist"), country: Some("by"), cities: &cities, host: Some("instagram.com"), state: SiteState::Social, results: &[], manager: &m }).unwrap();
        assert_eq!(named.email_subject, "Сайт для клиники «Стоматология Альянс-Дент» — как привлекать больше пациентов");
        let quoted = outreach(&OutreachInput { business: "Клиника \"Стоматология Васильевых\"", category: None, preset: None, country: Some("by"), cities: &cities, host: None, state: SiteState::None, results: &[], manager: &m }).unwrap();
        assert!(quoted.email_subject.starts_with("Сайт для «Клиника Стоматология Васильевых» —"), "{}", quoted.email_subject);
        assert!(!out.short.contains("instagram.com") && !out.email_subject.contains("instagram"), "{}", out.short);
    }

    /// magicart.by (11258) и заглушка (7246): шаблон «сайта по факту нет», домен клиента из карточки, а не хостера.
    #[test]
    fn outreach_for_dead_and_stub_says_no_site_in_fact() {
        let host = client_host(SiteState::Stub, Some("https://www.magicart.by/"), Some("https://www.magicart.by/"), Some("https://start.hoster.by/"));
        // Адрес — как в ссылке карточки: голый magicart.by ведёт на чужой сайт.
        assert_eq!(host.as_deref(), Some("www.magicart.by"));
        assert_eq!(client_host(SiteState::Dead, Some("http://WWW.Anna.by/x"), None, None).as_deref(), Some("www.anna.by"));
        assert_eq!(client_host(SiteState::Weak, Some("https://www.magicart.by/"), None, None).as_deref(), Some("www.magicart.by"), "запасной вариант — адрес из карточки как есть");
        // metamute.org: голый домен не резолвится, открылся www — его и называем.
        assert_eq!(client_host(SiteState::Weak, Some("https://metamute.org/"), Some("https://metamute.org/"), Some("https://www.metamute.org/")).as_deref(), Some("www.metamute.org"));
        assert_eq!(client_host(SiteState::Good, Some("https://www.clinic.by/"), None, Some("https://clinic.by/ru/")).as_deref(), Some("clinic.by"), "хост final_url, даже если в карточке с www");
        assert_eq!(client_host(SiteState::Stub, Some("https://magicart.by/"), None, None).as_deref(), Some("magicart.by"), "www нет в ссылке — не добавляем");
        assert_eq!(client_host(SiteState::Dead, None, None, Some("https://start.hoster.by/")).as_deref(), Some("start.hoster.by"), "только если больше нечего");
        let m = manager();
        let cities = vec!["Минск".to_string()];
        let dead = mk_out(&m, &cities, "by", Some("dentist"), "", host.as_deref(), SiteState::Dead, &[]);
        assert!(dead.short.contains("Сайт www.magicart.by, указанный в вашей карточке, не открывается, по факту сайта у компании нет"), "{}", dead.short);
        assert!(!dead.short.contains("hoster") && !dead.email_body.contains("hoster"));
        assert_eq!(dead.email_subject, "Сайт для «Компания» — как привлекать больше пациентов");
        let london = vec!["City of Westminster".to_string()];
        let stub = mk_out(&m, &london, "gb", None, "Metal casting", Some("sysongraingercasting.com"), SiteState::Stub, &[]);
        assert_eq!(stub.lang, "en");
        assert!(stub.short.contains("sysongraingercasting.com currently shows a placeholder page instead of a website, so in practice you have no website"), "{}", stub.short);
        assert!(!stub.short.chars().any(|c| ('\u{400}'..='\u{4ff}').contains(&c)));
    }

    /// Издательство в Лондоне (8215): без «записаться/позвонить», без спекуляций про ИИ и цены конкурента.
    #[test]
    fn english_outreach_for_publisher_has_no_booking_or_speculation() {
        let london = vec!["City of Westminster".to_string(), "Westminster".to_string()];
        let m = manager();
        let results = vec![r("N04", Status::Fail, ""), r("N01", Status::Fail, ""), r("I08", Status::Fail, ""), r("B04", Status::Fail, "")];
        let out = mk_out(&m, &london, "gb", None, "Book publisher", Some("metamute.org"), SiteState::Weak, &results);
        let all = format!("{}\n{}", out.short, out.email_body).to_lowercase();
        for bad in ["book", "call", "competitor's price", "will answer", "don't recommend"] {
            assert!(!all.contains(bad), "«{bad}» в тексте:\n{all}");
        }
        assert!(out.short.contains("there are no prices on the site, so customers can't find out what things cost"), "{}", out.short);
        // 8215: закрыты GPTBot, OAI-SearchBot и ClaudeBot; в I02 — только поисковый OAI-SearchBot (ChatGPT), ClaudeBot — обучение (I04).
        let i02 = r("I02", Status::Fail, "Закрыты поисковые ИИ-боты: OAI-SearchBot").with_value("OAI-SearchBot");
        let i05 = r("I05", Status::Warn, "защита отказала OAI-SearchBot (403)");
        let ai = mk_out(&m, &london, "gb", None, "Book publisher", Some("metamute.org"), SiteState::Weak, &[i02.clone(), i05.clone()]);
        assert!(ai.short.contains("block ChatGPT search, so it can't show the site in its answers"), "{}", ai.short);
        assert!(!ai.short.contains("can't read") && !ai.short.contains("Claude"), "{}", ai.short);
        assert!(!ai.short.contains("Perplexity") && !ai.short.contains("Gemini"), "{}", ai.short);
        assert!(!ai.short.to_lowercase().contains("protection"), "I05 не дублирует I02: {}", ai.short);
        let ai_ru = mk_out(&m, &["Минск".to_string()], "by", Some("dentist"), "Стоматология", Some("metamute.org"), SiteState::Weak, &[i02, i05]);
        assert!(ai_ru.short.contains("сайт закрыт для поиска ChatGPT, поэтому вас там не рекомендуют"), "{}", ai_ru.short);
        // Яндекс закрыт отдельно — добавляются Алиса и Нейро.
        let both = mk_out(&m, &["Минск".to_string()], "by", Some("dentist"), "Стоматология", Some("a.by"), SiteState::Weak, &[r("I02", Status::Fail, "Закрыты поисковые ИИ-боты: PerplexityBot"), r("I03", Status::Fail, "YandexAdditional запрещён")]);
        assert!(both.short.contains("(Perplexity, Алиса и Нейро)"), "{}", both.short);
        let ya = mk_out(&m, &["Минск".to_string()], "by", Some("dentist"), "Стоматология", Some("a.by"), SiteState::Weak, &[r("I03", Status::Fail, "YandexAdditional запрещён")]);
        assert!(ya.short.contains("закрыт для ответов Яндекса с ИИ (Алиса, Нейро)") && !ya.short.contains("ChatGPT"), "{}", ya.short);
        let none = mk_out(&m, &london, "gb", None, "Book publisher", None, SiteState::None, &[]);
        assert!(none.short.contains("see what you do or get in touch"), "{}", none.short);
        assert!(!none.short.to_lowercase().contains("booking"), "{}", none.short);
        // Салону запись обещать можно.
        let salon = mk_out(&m, &london, "gb", Some("hairdresser"), "Hair salon", None, SiteState::None, &[]);
        assert!(salon.short.contains("services, prices and online booking"), "{}", salon.short);
    }

    /// КП на английском: названия и описания пакетов, без пакета Яндекса, пункт «нет сайта».
    #[test]
    fn proposal_in_english() {
        let results = vec![r("A03", Status::Fail, ""), r("A10", Status::Fail, ""), r("B05", Status::Fail, "").with_value("no_redirect"), r("G01", Status::Fail, ""), r("R05", Status::Fail, "")];
        let p = build_proposal(&results, SiteState::Fair, &default_price_list_for("GBP").unwrap(), &SavedProposal::default(), "en");
        assert_eq!(p.lang, "en");
        assert!(p.packages.iter().all(|x| x.id != "yandex"));
        let https = p.packages.iter().find(|x| x.id == "https").unwrap();
        assert_eq!(https.title, "HTTPS and redirects");
        assert!(https.summary.starts_with("Turn on the secure connection"));
        assert_eq!(https.items[0].text, "No redirect to https: the browser says “Not secure” on the link from the listing");
        let maps = p.packages.iter().find(|x| x.id == "maps").unwrap();
        assert_eq!(maps.items.iter().map(|i| i.text.as_str()).collect::<Vec<_>>(), vec!["The listing has no website", "Few photos in the listing"]);
        for pk in &p.packages {
            let text = format!("{} {} {}", pk.title, pk.summary, pk.items.iter().map(|i| i.text.as_str()).collect::<String>());
            assert!(!text.chars().any(|c| ('\u{400}'..='\u{4ff}').contains(&c)), "кириллица в английском КП: {text}");
        }
        let none = build_proposal(&[r("A03", Status::Fail, "")], SiteState::None, &default_price_list_for("GBP").unwrap(), &SavedProposal::default(), "en");
        assert_eq!(none.packages[0].title, "New website: a one-page site with booking and local SEO");
        assert!(none.packages[0].items[0].text.starts_with("There is no website"));
        assert_eq!(none.total, 1200.0 + 50.0);
        // Своё название пакета в прайсе — сохраняется.
        let mut custom = default_price_list_for("GBP").unwrap();
        custom.iter_mut().find(|x| x.id == "https").unwrap().title = "Secure site".into();
        let named = build_proposal(&results, SiteState::Fair, &custom, &SavedProposal::default(), "en");
        assert_eq!(named.packages.iter().find(|x| x.id == "https").unwrap().title, "Secure site");
    }

    #[test]
    fn currency_by_country_and_price_lists() {
        for (c, cur) in [("by", "BYN"), ("ru", "RUB"), ("kz", "KZT"), ("ua", "UAH"), ("pl", "PLN"), ("gb", "GBP"), ("us", "USD"), ("de", "EUR"), ("fr", "EUR"), ("lv", "EUR"), ("ch", "CHF"), ("cz", "CZK"), ("GB", "GBP")] {
            assert_eq!(currency_for_country(Some(c), "BYN"), cur, "{c}");
        }
        assert_eq!(currency_for_country(Some("jp"), "BYN"), "BYN");
        assert_eq!(currency_for_country(None, "USD"), "USD");
        let gbp = default_price_list_for("gbp").unwrap();
        assert_eq!(gbp.len(), PACKAGES.len());
        assert_eq!(gbp[0].price, 1200.0);
        let usd = default_price_list_for("USD").unwrap();
        let eur = default_price_list_for("EUR").unwrap();
        assert!(usd[0].price > gbp[0].price && eur[0].price > gbp[0].price && eur[0].price < usd[0].price);
        assert!(gbp.iter().skip(1).all(|p| p.price > 0.0 && p.price < 250.0));
        assert!(default_price_list_for("PLN").is_none());
        // Выбор прайса: валюта настроек → отдельный → ориентиры → прайс настроек с пометкой.
        let mut settings = default_price_list();
        settings.iter_mut().find(|x| x.id == "https").unwrap().enabled = false;
        let none = Default::default();
        assert_eq!(price_list_for("byn", "BYN", &settings, &none), (settings.clone(), false));
        let (gb, mismatch) = price_list_for("GBP", "BYN", &settings, &none);
        assert!(!mismatch && gb[0].price == 1200.0 && !gb.iter().find(|x| x.id == "https").unwrap().enabled, "выключенный пакет остаётся выключенным");
        let mut extra = std::collections::BTreeMap::new();
        extra.insert("GBP".to_string(), vec![PriceItem { id: "https".into(), title: String::new(), price: 99.0, enabled: true }]);
        assert_eq!(price_list_for("GBP", "BYN", &settings, &extra).0.iter().find(|x| x.id == "https").unwrap().price, 99.0);
        assert_eq!(price_list_for("PLN", "BYN", &settings, &none), (settings, true));
    }

    /// Контакты: даты — не телефоны, share-ссылки отброшены, метки слежения убраны.
    #[test]
    fn contacts_dates_shares_and_tracking() {
        let html = r#"<html><body><p>Обновлено 07.03.2026 0 и 2026-03-07. Тел. +375 17 345-67-89</p>
            <a href="https://connect.ok.ru/offer?url=x">s</a>
            <a href="https://ok.ru/dk?st.cmd=WidgetSharePreview&st.shareUrl=x">s</a>
            <a href="https://vk.com/share.php?url=x">s</a>
            <a href="https://vk.com/share?url=x">s</a>
            <a href="https://www.facebook.com/sharer/sharer.php?u=x">s</a>
            <a href="https://www.instagram.com/belodent.by/?utm_medium=copy_link&igsh=abc&hl=ru">i</a>
            <a href="https://ok.ru/belodent">ok</a></body></html>"#;
        let p = page::extract(html, "https://belodent.by/");
        let c = contacts_from_pages(std::iter::once(&p));
        assert_eq!(c.phones, vec!["+375 17 345-67-89"], "{:?}", c.phones);
        let urls: Vec<&str> = c.links.iter().map(|l| l.url.as_str()).collect();
        assert_eq!(urls, vec!["https://www.instagram.com/belodent.by/", "https://ok.ru/belodent"], "{urls:?}");
        assert!(is_date_like("07.03.2026 0") && is_date_like("2026-03-07") && !is_date_like("+375 17 345-67-89"));
        assert_eq!(clean_social_url("https://t.me/x?utm_source=a"), "https://t.me/x");
        assert_eq!(clean_social_url("https://wa.me/375291234567?text=hi&utm_medium=x"), "https://wa.me/375291234567?text=hi");
        assert_eq!(clean_social_url("https://www.facebook.com/profile.php?id=123&igsh=1"), "https://www.facebook.com/profile.php?id=123");
    }

    /// В списке контактов: старые данные тоже чистятся, почты вакансий — с пометкой и в конце, tel: международный.
    #[test]
    fn contact_list_cleans_hr_mail_and_international_tel() {
        let card = Business { name: "Белодент".into(), phone: Some("8 017 345-67-89".into()), ..Default::default() };
        let site = Contacts {
            emails: vec!["hr@belodent.by".into(), "info@belodent.by".into(), "vakansii@belodent.by".into(), "jobs@x.by".into(), "career@x.by".into(), "rezume@x.by".into()],
            phones: vec!["07.03.2026 0".into(), "29 123-45-67".into(), "(029) 765-43-21".into()],
            links: vec![SiteLink { kind: "instagram".into(), url: "https://www.instagram.com/belodent.by/?utm_medium=copy_link&igsh=1".into() }],
        };
        let manual = vec![ManualContact { kind: "email".into(), value: "hr@mine.by".into(), note: None }];
        let list = contact_list(&card, &site, &manual, Some("by"));
        let view: Vec<(&str, Option<&str>)> = list.iter().map(|c| (c.value.as_str(), c.note.as_deref())).collect();
        let phones: Vec<&str> = list.iter().filter(|c| c.kind == "phone").map(|c| c.url.as_deref().unwrap()).collect();
        assert_eq!(phones, vec!["tel:+375173456789", "tel:+375291234567", "tel:+375297654321"], "{view:?}");
        assert!(list.iter().all(|c| !c.value.contains("2026")), "{view:?}");
        assert_eq!(list.iter().find(|c| c.kind == "instagram").unwrap().value, "https://www.instagram.com/belodent.by/");
        let emails: Vec<&str> = list.iter().filter(|c| c.kind == "email").map(|c| c.value.as_str()).collect();
        assert_eq!(emails, vec!["info@belodent.by", "hr@mine.by", "hr@belodent.by", "vakansii@belodent.by", "jobs@x.by", "career@x.by", "rezume@x.by"], "ручная почта hr@ не помечается и остаётся на месте");
        let n = |v: &str| list.iter().find(|c| c.value == v).unwrap().note.clone();
        assert_eq!((n("hr@belodent.by").as_deref(), n("info@belodent.by")), (Some("вакансии"), None));
        assert_eq!(list.last().unwrap().note.as_deref(), Some("вакансии"));
    }

    #[test]
    fn tel_links_are_international() {
        let t = |v: &str, c: &str| international_phone(v, Some(c));
        assert_eq!(t("+375 29 123-45-67", "by"), "+375291234567");
        assert_eq!(t("8 (029) 123-45-67", "by"), "+375291234567");
        assert_eq!(t("375291234567", "by"), "+375291234567");
        assert_eq!(t("(017) 345-67-89", "by"), "+375173456789");
        assert_eq!(t("8 (495) 123-45-67", "ru"), "+74951234567");
        assert_eq!(t("495 123-45-67", "ru"), "+74951234567");
        assert_eq!(t("020 7287 5327", "gb"), "+442072875327");
        assert_eq!(t("+44 (0)20 7287 5327", "gb"), "+442072875327");
        assert_eq!(t("(212) 555-1234", "us"), "+12125551234");
        assert_eq!(t("1 212 555 1234", "us"), "+12125551234");
        assert_eq!(t("00375 29 123-45-67", "by"), "+375291234567");
        assert_eq!(international_phone("29 123-45-67", None), "291234567", "без страны код не угадать");
        assert_eq!(contact_url_in("phone", "29 123-45-67", Some("by")).as_deref(), Some("tel:+375291234567"));
        assert_eq!(contact_url("phone", "+375 29 123-45-67").as_deref(), Some("tel:+375291234567"));
    }

    /// Круг 6: 11303 — «017) 276 74 26» без скобки; 11236 — vakansii-ls@mail.ru из сохранённых данных.
    #[test]
    fn phones_are_displayed_neatly_and_hr_mail_is_marked() {
        assert_eq!(tidy_phone("(017) 276 74 26"), "(017) 276 74 26");
        assert_eq!(tidy_phone("  +375 (17) 276-74-26. "), "+375 (17) 276-74-26");
        assert_eq!(display_phone("017) 276 74 26", Some("by")), "+375 17 276-74-26");
        assert_eq!(display_phone("(017) 276 74 26", Some("by")), "(017) 276 74 26");
        assert_eq!(display_phone("+375 29 765-54-00", Some("by")), "+375 29 765-54-00");
        let card = Business { name: "Любимый стоматолог".into(), ..Default::default() };
        let site = Contacts { emails: vec!["vakansii-ls@mail.ru".into()], phones: vec!["017) 276 74 26".into()], links: vec![] };
        let list = contact_list(&card, &site, &[], Some("by"));
        let phone = list.iter().find(|c| c.kind == "phone").unwrap();
        assert_eq!((phone.value.as_str(), phone.url.as_deref()), ("+375 17 276-74-26", Some("tel:+375172767426")));
        assert_eq!(list.iter().find(|c| c.kind == "email").unwrap().note.as_deref(), Some("вакансии"));
    }

    /// Круг 6: 7246 — соцсети Hover с заглушки не контакты компании; чужой и неподтверждённый сайт тоже.
    #[test]
    fn site_contacts_are_not_trusted_for_stub_dead_and_foreign_sites() {
        assert!(!site_contacts_trusted(SiteState::Stub, &[]));
        assert!(!site_contacts_trusted(SiteState::Dead, &[]));
        assert!(!site_contacts_trusted(SiteState::Fair, &[r("H02", Status::Fail, "").with_value("foreign")]));
        assert!(!site_contacts_trusted(SiteState::Fair, &[r("H02", Status::Warn, "").with_value("unverified")]));
        assert!(site_contacts_trusted(SiteState::Fair, &[r("H02", Status::Pass, "")]));
    }

    /// Круг 6: gor-dent.by — «На телефоне нельзя увеличить текст» в главном, значит, пакет «Мобильная версия» включён.
    #[test]
    fn package_with_a_main_problem_is_on_by_default() {
        let results = vec![r("E01", Status::Warn, "Масштабирование запрещено").with_value("no_zoom")];
        let p = build_proposal(&results, SiteState::Good, &default_price_list(), &SavedProposal::default(), "ru");
        let mobile = p.packages.iter().find(|x| x.id == "mobile").unwrap();
        assert!(mobile.default_enabled && mobile.enabled);
        let quiet = build_proposal(&[r("F04", Status::Warn, "")], SiteState::Good, &default_price_list(), &SavedProposal::default(), "ru");
        assert!(quiet.packages.iter().all(|x| !x.enabled || x.id == "content"), "{:?}", quiet.packages.iter().filter(|x| x.enabled).map(|x| &x.id).collect::<Vec<_>>());
    }

    /// Круг 6: карточка OSM — «Любімы стаматолаг»; на сайте — «Любимый стоматолог».
    #[test]
    fn name_suggestion_comes_from_the_site() {
        assert_eq!(name_suggestion(Some("Любимый стоматолог"), Some("Стоматология в Минске | Любимый стоматолог"), "Любімы стаматолаг", "Любімы стаматолаг").as_deref(), Some("Любимый стоматолог"));
        assert_eq!(name_suggestion(None, Some("Любимый стоматолог — клиника в Минске"), "Любімы стаматолаг", "Любімы стаматолаг").as_deref(), Some("Любимый стоматолог"));
        assert_eq!(name_suggestion(None, Some("Главная страница | Клиника"), "Клиника", "Клиника"), None, "служебный заголовок");
        assert_eq!(name_suggestion(None, Some("Белодент | Минск"), "Белодент", "Белодент"), None, "совпадает с карточкой");
        assert_eq!(name_suggestion(Some("Любимый стоматолог"), None, "Любимый стоматолог", "Любімы стаматолаг"), None, "менеджер уже задал");
    }

    #[test]
    fn name_suggestion_filters_seo_titles_domains_and_aligns_quotes() {
        let n = |site: Option<&str>, title: &str| name_suggestion(site, Some(title), "Віва Медыка", "Віва Медыка");
        // Кавычки не обрезаются по «»».
        assert_eq!(n(None, "Стоматология «Вива Медика» | Лечение зубов").as_deref(), Some("Стоматология «Вива Медика»"));
        assert_eq!(n(None, "«Вива Медика» — клиника").as_deref(), Some("Вива Медика"));
        assert_eq!(n(None, "Стоматология «Вива Медика, цены").as_deref(), Some("Стоматология «Вива Медика»"));
        // og:site_name важнее заголовка.
        assert_eq!(n(Some("Вива Медика"), "Что-то другое | сайт").as_deref(), Some("Вива Медика"));
        // Разделители: точка с пробелом, запятая, дефис.
        assert_eq!(n(None, "Норимед. Стоматологическая клиника").as_deref(), Some("Норимед"));
        assert_eq!(n(None, "Норимед, клиника на Немиге").as_deref(), Some("Норимед"));
        // SEO-заголовки, домены и служебное — нет.
        assert_eq!(n(None, "Стоматология в Минске: цены и отзывы"), None);
        assert_eq!(n(None, "Платная стоматология"), None);
        assert_eq!(n(None, "Стоматология. Платная стоматологическая клиника в Минске. Цены. Отзывы | Норимед"), None, "одно слово вида деятельности");
        assert_eq!(n(None, "Официальный сайт клиники"), None);
        assert_eq!(n(None, "stomcompliance.com"), None);
        assert_eq!(n(Some("www.stomcompliance.com"), "Главная"), None);
        assert_eq!(n(None, "Страница 404 — не найдено"), None);
        assert_eq!(n(None, &"Очень длинное название клиники ".repeat(3)), None, "длиннее 50 знаков");
        // Совпадение с текущим без учёта регистра и кавычек.
        assert_eq!(name_suggestion(None, Some("«ВИВА МЕДИКА» | Минск"), "Вива Медика", "x"), None);
        assert_eq!(name_suggestion(None, Some("Dr. Smith Dental | London"), "Other", "Other").as_deref(), Some("Dr. Smith Dental"));
    }

    #[test]
    fn implausible_phone_numbers_are_dropped_and_codes_grouped() {
        let card = Business { name: "К".into(), ..Default::default() };
        let site = Contacts {
            phones: vec!["+375 (29) 386-00-05".into(), "+375 29 386".into(), "+749 932 28 515".into(), "+44 20 7287 5327".into(), "+371 66 555 076".into()],
            links: vec![
                SiteLink { kind: "whatsapp".into(), url: "https://wa.me/+375293860005".into() },
                SiteLink { kind: "whatsapp".into(), url: "https://wa.me/+3753860005".into() },
                SiteLink { kind: "whatsapp".into(), url: "https://wa.me/message/ABCDEF".into() },
            ],
            ..Default::default()
        };
        let list = contact_list(&card, &site, &[], Some("by"));
        let vals: Vec<&str> = list.iter().map(|c| c.value.as_str()).collect();
        assert!(!vals.contains(&"+375 29 386"), "{vals:?}");
        assert!(!vals.iter().any(|v| v.contains("3753860005")), "неполный wa.me отброшен: {vals:?}");
        assert!(vals.contains(&"https://wa.me/+375293860005") && vals.contains(&"https://wa.me/message/ABCDEF"));
        assert!(vals.contains(&"+7 499 322-85-15"), "{vals:?}");
        assert!(vals.contains(&"+44 20 7287 5327") && vals.contains(&"+371 66 555 076"));
        assert!(phone_plausible("+442072875327") && !phone_plausible("+4420728") && !phone_plausible("+7499322851") && !phone_plausible("+37529386"));
        assert!(phone_plausible("+12125551234") && !phone_plausible("+1212555123"));
        assert_eq!(display_phone("+74993228515", Some("ru")), "+7 499 322-85-15");
        assert_eq!(display_phone("+375291235000", Some("by")), "+375 29 123-50-00");
        // Ручной контакт менеджера не трогаем.
        let manual = vec![ManualContact { kind: "phone".into(), value: "+375 29 386".into(), note: None }];
        assert!(contact_list(&card, &Contacts::default(), &manual, Some("by")).iter().any(|c| c.value == "+375 29 386"));
    }

    #[test]
    fn no_site_outreach_does_not_claim_patients_leave() {
        let m = Manager::default();
        let cities = vec!["Минск".to_string()];
        for (state, lang_country) in [(SiteState::None, "by"), (SiteState::Social, "by"), (SiteState::Dead, "by"), (SiteState::Stub, "by"), (SiteState::None, "gb"), (SiteState::Dead, "gb")] {
            let cs: Vec<String> = if lang_country == "by" { cities.clone() } else { vec!["London".into()] };
            let out = outreach(&OutreachInput { business: "К", category: Some("Стоматология"), preset: Some("dentist"), country: Some(lang_country), cities: &cs, host: Some("x.by"), state, results: &[], manager: &m }).unwrap();
            assert!(!out.short.contains("уходят к конкурентам") && !out.short.contains("go elsewhere"), "{}", out.short);
            assert!(out.short.contains("могут выбрать конкурентов") || out.short.contains("may choose a competitor"), "{}", out.short);
        }
    }

    #[test]
    fn package_summaries_have_no_jargon() {
        for p in PACKAGES {
            for bad in ["Вебмастер", "Search Console", "кэширован", "разметку:", "движк", "ИИ-ботов"] {
                assert!(!p.summary.contains(bad), "{}: {}", p.id, p.summary);
            }
        }
        for (id, _, text) in PACKAGES_EN {
            for bad in ["caching", "structured data", "crawlers", "engine updates", "enable compression"] {
                assert!(!text.contains(bad), "{id}: {text}");
            }
        }
    }

    #[test]
    fn proposal_has_no_duplicate_items_and_names_the_map() {
        let mut a03 = r("A03", Status::Fail, "В карточке нет сайта");
        a03.maps = Some("OpenStreetMap".into());
        let mut a12 = r("A12", Status::Warn, "");
        a12.maps = Some("OpenStreetMap".into());
        let results = vec![a03, a12, r("D01", Status::Fail, ""), r("D05", Status::Fail, ""), r("D06", Status::Fail, ""), r("D07", Status::Fail, "")];
        let prices = default_price_list();
        let p = build_proposal(&results, SiteState::Fair, &prices, &SavedProposal::default(), "ru");
        let texts: Vec<String> = p.packages.iter().flat_map(|k| k.items.iter().map(|i| i.text.clone())).collect();
        let unique: HashSet<&String> = texts.iter().collect();
        assert_eq!(texts.len(), unique.len(), "{texts:?}");
        assert!(texts.iter().any(|t| t == "В карточке на OpenStreetMap не указан сайт"), "{texts:?}");
        assert_eq!(texts.len(), 6, "четыре разные D-формулировки и два пункта карточки: {texts:?}");
        let en = build_proposal(&results, SiteState::Fair, &prices, &SavedProposal::default(), "en");
        let en_texts: Vec<String> = en.packages.iter().flat_map(|k| k.items.iter().map(|i| i.text.clone())).collect();
        assert!(en_texts.iter().any(|t| t == "The OpenStreetMap listing has no website"), "{en_texts:?}");
    }

    #[test]
    fn outreach_bullets_are_not_repeated() {
        let results = vec![r("D01", Status::Fail, ""), r("D05", Status::Fail, ""), r("D06", Status::Fail, ""), r("D07", Status::Fail, ""), r("B08", Status::Fail, "")];
        let b = bullets(&results, None, true);
        let ru: HashSet<&String> = b.iter().map(|x| &x.ru).collect();
        assert_eq!(b.len(), ru.len());
        assert_eq!(b.iter().filter(|x| x.ru.contains("медленно грузится")).count(), 1);
    }
}
