//! Текстовые и доменные утилиты: телефоны, названия, домены, тип ссылки на сайт, гео.

use serde::{Deserialize, Serialize};
use url::Url;

// ---------- Телефоны ----------

/// Длина кода страны в начале международного номера (коды ITU не перекрываются).
pub fn calling_code_len(d: &str) -> usize {
    const TWO: &[&str] = &[
        "20", "27", "30", "31", "32", "33", "34", "36", "39", "40", "41", "43", "44", "45", "46", "47", "48", "49", "51",
        "52", "53", "54", "55", "56", "57", "58", "60", "61", "62", "63", "64", "65", "66", "81", "82", "84", "86",
        "90", "91", "92", "93", "94", "95", "98",
    ];
    if d.starts_with('1') || d.starts_with('7') {
        1
    } else if d.len() >= 2 && TWO.contains(&&d[..2]) {
        2
    } else {
        3
    }
}

/// Ключ телефона для сравнения: национальный номер без кода страны и префикса
/// (0, 8, 1), последние 9 цифр. Одинаков для «+7 495 …» и «8 495 …»,
/// «+49 (0)30 …» и «030 …», «+1 718 …» и «(718) …».
pub fn phone_key(raw: &str) -> Option<String> {
    let decoded = raw.replace("%2B", "+").replace("%2b", "+").replace("%20", " ");
    let t = decoded.trim();
    let mut d: String = t.chars().filter(|c| c.is_ascii_digit()).collect();
    let intl = t.starts_with('+') || t.starts_with("00");
    if t.starts_with("00") && d.len() > 2 {
        d.drain(..2);
    }
    if intl {
        let cc = calling_code_len(&d);
        if d.len() > cc + 6 {
            d.drain(..cc);
        }
        // «+49 (0)30 …»: префикс 0 после кода страны.
        if d.starts_with('0') {
            d.remove(0);
        }
    } else if (d.len() == 11 && (d.starts_with('8') || d.starts_with('7') || d.starts_with('1'))) || d.starts_with('0') {
        // Префикс выхода на межгород: 8 (Россия), 1 (США), 0 (Европа); 7 — код России без «+».
        d.remove(0);
    }
    if !(7..=13).contains(&d.len()) {
        return None;
    }
    Some(d[d.len().saturating_sub(9)..].to_string())
}

/// Ключи всех телефонов в тексте. Берём только оформленные номера: с «+» или
/// с разделителями — так ИНН, ОГРН и артикулы не принимаются за телефоны.
pub fn phones_in_text(text: &str) -> Vec<String> {
    let mut out: Vec<String> = phone_matches(text).iter().filter_map(|m| phone_key(m)).collect();
    out.sort();
    out.dedup();
    out
}

/// Строки телефонов в тексте как написаны («+33 1 42 68 53 00», «212.555.1234») — для кода страны.
pub fn phone_matches(text: &str) -> Vec<String> {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    static SLASH: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"\+?\(?\d[\d\s\-\u{a0}().]{5,22}\d").unwrap());
    let slash = SLASH.get_or_init(|| regex::Regex::new(r"(\d{2,6})/(\d)").unwrap());
    let text = slash.replace_all(text, "$1 $2");
    re.find_iter(&text)
        // Номер — отдельное слово: не кусок GUID «…b21f6d50-1616-4293-b442…» и не часть артикула.
        .filter(|m| {
            let before = text[..m.start()].chars().next_back();
            let after = text[m.end()..].chars().next();
            !before.is_some_and(|c| c.is_alphanumeric() || c == '-') && !after.is_some_and(|c| c.is_alphanumeric() || c == '-')
        })
        // Артикул с ценой — не телефон: «Мод. 062685 161.84 BYN» (marko.by).
        .filter(|m| {
            let s = m.as_str();
            let after = text[m.end()..].trim_start().to_lowercase();
            let before: String = text[..m.start()].chars().rev().take(12).collect::<String>().chars().rev().collect::<String>().to_lowercase();
            let price_tail = s.matches('.').count() + s.matches(',').count() == 1
                && s.rsplit(['.', ',']).next().is_some_and(|t| t.len() == 2 && t.chars().all(|c| c.is_ascii_digit()));
            let currency = ["byn", "br", "руб", "р.", "₽", "$", "€", "usd", "eur", "бел", "£", "chf", "zł", "kč"].iter().any(|c| after.starts_with(c));
            let article = ["мод.", "арт.", "артикул", "код", "sku", "№", "art.", "ref.", "réf.", "item", "model", "modèle", "modelo"]
                .iter()
                .any(|w| before.trim_end().ends_with(w) || before.contains(&format!("{w} ")));
            !(price_tail || currency || (article && !s.starts_with('+')))
        })
        .filter(|m| {
            let s = m.as_str();
            let before: String = text[..m.start()].chars().rev().take(30).collect::<String>().chars().rev().collect::<String>().to_lowercase();
            let digits = s.chars().filter(|c| c.is_ascii_digit()).count();
            // Группы цифр через одни пробелы — это и ОГРН «1 134 025 005 553», и даты со счётчиками
            // «2026 796 0 0». Такой номер засчитываем, только если он начинается как телефон (+, 8, 0).
            let plus = s.starts_with('+');
            // Точка — разделитель дат, цен и чисел, а не номера («222760095 0.0 0» — не телефон).
            let strong = s.chars().any(|c| matches!(c, '-' | '(' | ')'));
            // Точки как разделители групп — американское «212.555.1234» и французское «01.42.68.53.00».
            let dotted = !s.contains(' ') && s.matches('.').count() >= 2 && s.split('.').all(|g| (2..=4).contains(&g.trim_matches(['(', ')']).len()));
            let spaced = s.chars().any(|c| matches!(c, ' ' | '\u{a0}'));
            let trunk = matches!(s.trim_start_matches(['(', ' ']).chars().next(), Some('8' | '0'));
            // Без кода и префикса (Испания «912 345 678», Польша «22 123 45 67») — только с подписью рядом.
            let labelled = ["tel", "тел", "phone", "fon", "llam", "call", "móvil", "movil", "mobile", "whatsapp", "☎", "📞", "kontakt", "contact"]
                .iter()
                .any(|w| before.contains(w))
                && s.split([' ', '\u{a0}']).all(|g| g.len() >= 2);
            (9..=15).contains(&digits) && (plus || (digits <= 12 && (strong || dotted || (spaced && (trunk || labelled)))))
        })
        .map(|m| m.as_str().to_string())
        .collect()
}

/// Латиница без диакритики: «Champs-Élysées» → «champs-elysees», «Świętokrzyska» → «swietokrzyska».
pub fn fold_diacritics(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ą' => 'a',
            'ç' | 'ć' | 'č' => 'c',
            'ď' => 'd',
            'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ę' | 'ě' => 'e',
            'ì' | 'í' | 'î' | 'ï' | 'ī' => 'i',
            'ł' | 'ľ' | 'ĺ' => 'l',
            'ñ' | 'ń' | 'ň' => 'n',
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' => 'o',
            'ŕ' | 'ř' => 'r',
            'ś' | 'š' | 'ş' => 's',
            'ť' | 'ţ' => 't',
            'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' | 'ű' => 'u',
            'ý' | 'ÿ' => 'y',
            'ź' | 'ż' | 'ž' => 'z',
            other => other,
        })
        .collect()
}

// ---------- Названия ----------

/// Нормализованное название: нижний регистр, ё→е, без кавычек, правовых форм и пунктуации.
pub fn normalize_name(s: &str) -> String {
    let lower = s.to_lowercase().replace('ё', "е");
    let cleaned: String =
        lower.chars().map(|c| if c.is_alphanumeric() || c.is_whitespace() { c } else { ' ' }).collect();
    const LEGAL: &[&str] = &["ооо", "ип", "зао", "оао", "пао", "ао", "нко", "ано", "llc", "ltd", "inc"];
    cleaned.split_whitespace().filter(|w| !LEGAL.contains(w)).collect::<Vec<_>>().join(" ")
}

/// Сходство строк по символьным триграммам (коэффициент Дайса), 0..1.
pub fn similarity(a: &str, b: &str) -> f64 {
    fn grams(s: &str) -> Vec<[char; 3]> {
        let chars: Vec<char> = format!("  {s} ").chars().collect();
        let mut g: Vec<[char; 3]> = chars.windows(3).map(|w| [w[0], w[1], w[2]]).collect();
        g.sort();
        g
    }
    let (ga, gb) = (grams(a), grams(b));
    if ga.is_empty() || gb.is_empty() {
        return 0.0;
    }
    let (mut i, mut j, mut common) = (0, 0, 0usize);
    while i < ga.len() && j < gb.len() {
        match ga[i].cmp(&gb[j]) {
            std::cmp::Ordering::Equal => {
                common += 1;
                i += 1;
                j += 1;
            }
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
        }
    }
    2.0 * common as f64 / (ga.len() + gb.len()) as f64
}

/// Основы слов запроса для поиска вхождений с учётом русских окончаний:
/// «стоматология» → «стоматол», «салон красоты» → «сало», «красот».
pub fn stems(phrase: &str) -> Vec<String> {
    phrase
        .to_lowercase()
        .replace('ё', "е")
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 4)
        .map(|w| {
            let n = w.chars().count();
            let keep = ((n as f64 * 0.75).ceil() as usize).clamp(4, 8).min(n);
            w.chars().take(keep).collect()
        })
        .collect()
}

pub fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max_chars).collect();
    out.push('…');
    out
}

pub fn contains_any(hay: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| hay.contains(n))
}

// ---------- Домены ----------

pub fn parse_url_lenient(raw: &str) -> Option<Url> {
    let t = raw.trim();
    if t.is_empty() {
        return None;
    }
    let with_scheme =
        if t.contains("://") { t.to_string() } else { format!("https://{}", t.trim_start_matches("//")) };
    Url::parse(&with_scheme).ok().filter(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some())
}

/// Хост без «www.» в нижнем регистре (punycode для кириллических доменов).
pub fn host_of(u: &Url) -> String {
    u.host_str().unwrap_or("").trim_start_matches("www.").to_lowercase()
}

/// Регистрируемый домен: shop.example.co.uk → example.co.uk.
pub fn registrable_domain(host: &str) -> String {
    let h = host.trim_end_matches('.').to_lowercase();
    psl::domain_str(&h).map(|d| d.to_string()).unwrap_or(h)
}

pub fn same_site(a: &Url, b: &Url) -> bool {
    registrable_domain(&host_of(a)) == registrable_domain(&host_of(b))
}

fn host_matches(host: &str, domains: &[&str]) -> bool {
    domains.iter().any(|d| host == *d || host.ends_with(&format!(".{d}")))
}

/// Что стоит в поле «сайт» карточки (A04).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SiteKind {
    Own,
    ConstructorSubdomain,
    Social,
    Messenger,
    LinkInBio,
    Aggregator,
    Booking,
    Invalid,
}

impl SiteKind {
    pub fn label(self) -> &'static str {
        match self {
            SiteKind::Own => "свой домен",
            SiteKind::ConstructorSubdomain => "поддомен конструктора",
            SiteKind::Social => "соцсеть",
            SiteKind::Messenger => "мессенджер",
            SiteKind::LinkInBio => "мультиссылка",
            SiteKind::Aggregator => "агрегатор или каталог",
            SiteKind::Booking => "страница онлайн-записи",
            SiteKind::Invalid => "некорректная ссылка",
        }
    }
    /// Можно ли по этой ссылке проводить аудит сайта.
    pub fn is_auditable(self) -> bool {
        matches!(self, SiteKind::Own | SiteKind::ConstructorSubdomain)
    }
}

const SOCIAL: &[&str] = &[
    "vk.com", "vk.ru", "ok.ru", "instagram.com", "facebook.com", "fb.com", "youtube.com", "youtu.be",
    "tiktok.com", "dzen.ru", "zen.yandex.ru", "twitter.com", "x.com", "pinterest.com", "pinterest.ru",
    "livejournal.com", "rutube.ru", "threads.net", "linkedin.com", "snapchat.com", "tumblr.com", "nextdoor.com",
    "bsky.app", "taplink.ru",
];
const MESSENGER: &[&str] =
    &["t.me", "telegram.me", "wa.me", "whatsapp.com", "max.ru", "viber.com", "icq.im", "m.me", "line.me", "wa.link", "walink.co"];
const LINK_IN_BIO: &[&str] = &[
    "taplink.cc", "taplink.ws", "taplink.at", "linktr.ee", "hipolink.me", "hipolink.net", "mssg.me",
    "lnk.bio", "link.me", "msg.link", "beacons.ai", "linkin.bio", "solo.to", "bio.link", "campsite.bio", "allmylinks.com",
    "linkpop.com", "carrd.co", "milkshake.app", "tap.bio", "flow.page", "tapy.me", "tap.link", "t.co", "bit.ly", "clck.ru",
    "vk.cc", "goo.gl", "tinyurl.com", "cutt.ly", "is.gd", "u.to",
];
const AGGREGATOR: &[&str] = &[
    "2gis.ru", "2gis.com", "yell.ru", "zoon.ru", "flamp.ru", "prodoctorov.ru", "napopravku.ru", "docdoc.ru",
    "profi.ru", "avito.ru", "youla.ru", "uslugi.yandex.ru", "yandex.ru", "yandex.by", "yandex.kz", "yandex.com", "ya.ru",
    "google.com", "goo.gl", "2gis.by", "2gis.kz", "relax.by", "restoran.by", "tam.by", "untappd.com", "kufar.by", "deal.by",
    "103.by", "ydoc.by", "e-ticket.by", "uniticket.by",
    "g.page", "tripadvisor.ru", "tripadvisor.com", "yelp.com", "booking.com", "ostrovok.ru", "restoclub.ru",
    "afisha.ru", "hh.ru", "ozon.ru", "wildberries.ru", "otzovik.com", "irecommend.ru", "spr.ru",
    "orgpage.ru", "rusprofile.ru", "list-org.com", "tripadvisor.de", "tripadvisor.co.uk", "tripadvisor.fr",
    "tripadvisor.es", "tripadvisor.it", "yelp.de", "yelp.co.uk", "yelp.fr", "yellowpages.com", "yp.com",
    "mapquest.com", "foursquare.com", "groupon.com", "groupon.de", "angi.com", "thumbtack.com", "houzz.com",
    "healthgrades.com", "vitals.com", "webmd.com", "gelbeseiten.de", "dasoertliche.de", "11880.com", "golocal.de",
    "meinestadt.de", "werkenntdenbesten.de", "jameda.de", "pagesjaunes.fr", "paginasamarillas.es",
    "paginegialle.it", "yell.com", "192.com", "scoot.co.uk", "uber.com", "ubereats.com", "doordash.com",
    "grubhub.com", "deliveroo.co.uk", "deliveroo.fr", "lieferando.de", "just-eat.co.uk", "wolt.com",
    "seamless.com", "postmates.com", "trustpilot.com", "provenexpert.com", "apple.com", "bing.com",
    "facebook.com", "linktr.ee", "squareup.com", "bbb.org", "manta.com", "chamberofcommerce.com",
    "birdeye.com", "superpages.com", "citysearch.com", "zomato.com", "opentable.de", "thefork.com",
    "thefork.de", "thefork.fr", "thefork.it", "quandoo.de", "restaurantguru.com", "sluurpy.com",
    // Энциклопедии, путеводители и площадки Европы и США — не сайт компании.
    "wikipedia.org", "wikimedia.org", "your.beer", "wikidata.org", "londontown.com", "timeout.com", "visitlondon.com", "tripadvisor.it",
    "tripadvisor.ca", "tripadvisor.com.au", "yelp.es", "yelp.it", "yelp.ca", "eltenedor.es", "thefork.es", "deliveroo.es",
    "glovoapp.com", "just-eat.es", "just-eat.fr", "just-eat.ie", "airbnb.com", "airbnb.co.uk", "airbnb.fr", "airbnb.es",
    "hotels.com", "expedia.com", "agoda.com", "lafourchette.com", "petitfute.com", "routard.com", "minube.com",
    "eventbrite.com", "eventbrite.co.uk", "meetup.com", "nextdoor.com", "alignable.com", "zillow.com", "realtor.com",
];
const BOOKING: &[&str] = &[
    "yclients.com", "dikidi.net", "dikidi.ru", "dikidi.online", "booksy.com", "sonline.su", "restoplace.ws", "bookform.ru",
    "arnica.pro", "medflex.ru", "calendly.com", "fresha.com", "vagaro.com", "setmore.com", "simplybook.me",
    "mindbodyonline.com", "mindbody.io", "styleseat.com", "schedulicity.com", "acuityscheduling.com",
    "as.me", "booker.com", "treatwell.co.uk", "treatwell.de", "treatwell.fr", "treatwell.it", "treatwell.es",
    "planity.com", "doctolib.de", "doctolib.fr", "doctolib.it", "zocdoc.com", "opentable.com", "opentable.co.uk",
    "resy.com", "sevenrooms.com", "exploretock.com", "quandoo.com", "glossgenius.com", "getsquire.com",
    "square.site", "booksy.net", "salonized.com", "phorest.com", "shore.com", "etermin.net", "doctoralia.es",
    "miodottore.it", "znanylekarz.pl",
];
const CONSTRUCTOR: &[&str] = &[
    "tilda.ws", "tilda.site", "wixsite.com", "ucoz.ru", "ucoz.net", "ucoz.com", "ucoz.org", "narod.ru",
    "narod2.ru", "nethouse.ru", "business.site", "sites.google.com", "webflow.io", "framer.website",
    "framer.ai", "notion.site", "lpmotor.ru", "umi.ru", "setup.ru", "ukit.me", "turbo.site",
    "wordpress.com", "blogspot.com", "github.io", "netlify.app", "vercel.app", "carrd.co", "flexbe.online",
    "tb.ru", "mya5.ru", "jimdo.com", "jimdofree.com", "weebly.com", "squarespace.com", "craftum.io",
    "godaddysites.com", "myshopify.com", "wixstudio.io", "webnode.page", "webnode.com", "site123.me",
    "mystrikingly.com", "strikingly.com", "yolasite.com", "jimdosite.com", "simplesite.com", "ueniweb.com",
    "wix.com", "editorx.io", "duda.co", "hubspotpagebuilder.com", "hs-sites.com", "mozello.com", "onepage.website",
    "ionos.site", "business.page", "wordpress.org", "web.app", "firebaseapp.com", "pages.dev",
    "glitch.me", "replit.app",
];

pub fn classify_website(raw: &str) -> (SiteKind, String) {
    let Some(u) = parse_url_lenient(raw) else {
        return (SiteKind::Invalid, raw.to_string());
    };
    let host = host_of(&u);
    let kind = if host_matches(&host, MESSENGER) {
        SiteKind::Messenger
    } else if host_matches(&host, SOCIAL) {
        SiteKind::Social
    } else if host_matches(&host, LINK_IN_BIO) {
        SiteKind::LinkInBio
    } else if host_matches(&host, BOOKING) {
        SiteKind::Booking
    } else if host == "sites.google.com" || host_matches(&host, CONSTRUCTOR) {
        // sites.google.com/view/… — сайт-конструктор Google.
        SiteKind::ConstructorSubdomain
    } else if host_matches(&host, AGGREGATOR) {
        SiteKind::Aggregator
    } else {
        SiteKind::Own
    };
    (kind, host)
}

// ---------- Гео ----------

/// Расстояние между точками, метры.
/// Русское склонение по числу: 1 проблема, 2 проблемы, 5 проблем.
pub fn plural_ru<'a>(n: usize, one: &'a str, few: &'a str, many: &'a str) -> &'a str {
    let (m10, m100) = (n % 10, n % 100);
    if m10 == 1 && m100 != 11 {
        one
    } else if (2..=4).contains(&m10) && !(12..=14).contains(&m100) {
        few
    } else {
        many
    }
}

pub fn haversine_m(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let r = 6_371_000.0_f64;
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dp = (lat2 - lat1).to_radians();
    let dl = (lon2 - lon1).to_radians();
    let a = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * r * a.sqrt().asin()
}

/// Прямоугольник вокруг круга: (юг, запад, север, восток).
pub fn bbox(lat: f64, lon: f64, radius_m: f64) -> (f64, f64, f64, f64) {
    let dlat = radius_m / 111_320.0;
    let dlon = radius_m / (111_320.0 * lat.to_radians().cos().max(0.01));
    (lat - dlat, lon - dlon, lat + dlat, lon + dlon)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encyclopedias_and_whatsapp_links_are_not_own_sites() {
        // Париж: в карточке музея — it.wikipedia.org; Мадрид: walink.co — короткая ссылка на WhatsApp.
        assert_ne!(classify_website("http://it.wikipedia.org/wiki/index.html?curid=1233627").0, SiteKind::Own);
        assert_ne!(classify_website("https://walink.co/11c8a3").0, SiteKind::Own);
        assert_ne!(classify_website("http://www.londontown.com/LondonInformation/Restaurant/Bar_Bruno/3710").0, SiteKind::Own);
    }

    #[test]
    fn phones_in_us_and_european_formats() {
        assert_eq!(phones_in_text("Call 212.555.1234 today").len(), 1, "американский формат с точками");
        assert_eq!(phones_in_text("Tél. 01.42.68.53.00").len(), 1, "французский формат с точками");
        assert_eq!(phones_in_text("Teléfono: 912 345 678").len(), 1, "испанский номер с подписью");
        assert_eq!(phones_in_text("(718) 555-0199").len(), 1);
        assert!(phones_in_text("Prices: 250 300 450 euro").is_empty(), "цены через пробел — не телефон");
        assert!(phones_in_text("Version 1.2.3 released 2026.11.24").is_empty(), "версии и даты с точками — не телефон");
    }

    #[test]
    fn article_with_price_is_not_a_phone() {
        // Круг 4: marko.by — «Мод. 062685 161.84 BYN» принимался за телефон.
        assert!(phones_in_text("Мод. 062685 161.84 BYN, Мод. 8066001 82.02 BYN").is_empty());
        assert_eq!(phones_in_text("Звоните: +375 29 123-45-67, 8 (017) 222-33-44").len(), 2);
    }

    #[test]
    fn phone_keys() {
        assert_eq!(phone_key("8 (495) 123-45-67").as_deref(), Some("951234567"));
        assert_eq!(phone_key("+7 495 123 45 67").as_deref(), Some("951234567"));
        assert_eq!(phone_key("7(495)1234567").as_deref(), Some("951234567"));
        assert_eq!(phone_key("123"), None);
        // США, Германия, Великобритания, Италия, Дания: международная и национальная записи.
        assert_eq!(phone_key("+1-718-622-1900"), phone_key("(718) 622-1900"));
        assert_eq!(phone_key("1-718-622-1900"), phone_key("718.622.1900"));
        assert_eq!(phone_key("+49 (0)30 1234567"), phone_key("030 1234567"));
        assert_eq!(phone_key("0049 30 1234567"), phone_key("030/1234567"));
        assert_eq!(phone_key("+44 20 7946 0958"), phone_key("020 7946 0958"));
        assert_eq!(phone_key("+39 06 1234567"), phone_key("06 1234567"));
        assert_eq!(phone_key("+45 12 34 56 78"), phone_key("12 34 56 78"));
        let found = phones_in_text("Звоните: +7 (812) 555-12-34 или 8 800 100-20-30.");
        assert_eq!(found, vec!["001002030".to_string(), "125551234".to_string()]);
        // ИНН и ОГРН без разделителей — не телефоны.
        assert!(phones_in_text("ИНН 7707083893, ОГРН 1027700132195").is_empty());
        assert_eq!(phones_in_text("Tel. 030/1234567"), vec!["301234567".to_string()]);
        assert_eq!(phones_in_text("Call (718) 622-1900 today"), vec!["186221900".to_string()]);
    }

    #[test]
    fn names_and_similarity() {
        assert_eq!(normalize_name("ООО «Зуб-Мастер»"), "зуб мастер");
        assert!(similarity("зуб мастер", "зубмастер") > 0.6);
        assert!(similarity("зуб мастер", "автосервис колесо") < 0.2);
    }

    #[test]
    fn russian_stems() {
        assert_eq!(stems("стоматология"), vec!["стоматол"]);
        assert_eq!(stems("салон красоты"), vec!["сало", "красот"]);
        assert!(stems("кафе в центре").contains(&"кафе".to_string()));
    }

    #[test]
    fn website_kinds() {
        assert_eq!(classify_website("https://vk.com/club1").0, SiteKind::Social);
        assert_eq!(classify_website("taplink.cc/zubmaster").0, SiteKind::LinkInBio);
        assert_eq!(classify_website("https://zub.tilda.ws").0, SiteKind::ConstructorSubdomain);
        assert_eq!(classify_website("https://n123.yclients.com").0, SiteKind::Booking);
        assert_eq!(classify_website("https://2gis.ru/moscow/firm/1").0, SiteKind::Aggregator);
        assert_eq!(classify_website("https://t.me/zub").0, SiteKind::Messenger);
        assert_eq!(classify_website("zub-master.ru").0, SiteKind::Own);
        assert_eq!(classify_website("не сайт").0, SiteKind::Invalid);
    }

    #[test]
    fn domains() {
        assert_eq!(registrable_domain("shop.example.co.uk"), "example.co.uk");
        assert_eq!(registrable_domain("www.zub.spb.ru"), "zub.spb.ru");
    }

    #[test]
    fn geo() {
        let d = haversine_m(55.7558, 37.6173, 55.7539, 37.6208);
        assert!((250.0..350.0).contains(&d), "{d}");
    }
}

#[cfg(test)]
mod phone_boundary_tests {
    use super::phones_in_text;

    #[test]
    fn guid_and_ogrn_are_not_phones() {
        assert!(phones_in_text("img/b21f6d50-1616-4293-b442-77a1c0d1e2f3.png").is_empty(), "кусок GUID");
        assert!(phones_in_text("ОГРН 1 134 025 005 553").is_empty());
        assert_eq!(phones_in_text("звоните +375 29 128-60-28").len(), 1);
        assert_eq!(phones_in_text("тел.: 8 (017) 222-33-44").len(), 1);
    }
}
