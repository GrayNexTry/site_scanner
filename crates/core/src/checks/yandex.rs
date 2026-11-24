//! R. Факторы Яндекса: региональные (R01, R03, R04), коммерческие (R05–R10), нарушения и фильтры
//! (R11–R15), открытые данные Яндекса (C19 — ИКС, R16 — знаки). R02, R17–R19 требуют доступа
//! к кабинетам клиента, R20 — этап аудита (поиск других сайтов компании).
//!
//! Нарушения — по «Рекомендациям Яндекса для сайтов»; у Google те же правила против спама,
//! поэтому R05–R15 проверяются в любой стране. Словари — на ru, en, de, fr, es, it, pl, pt, nl.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use regex::Regex;

use super::{fmt_num, list_short, CheckCtx, SiteView};
use crate::model::{MetricResult, ProviderKind, Status};
use crate::region::{self, Zone};
use crate::text::{phone_matches, registrable_domain};

pub fn run(ctx: &CheckCtx, v: &SiteView, out: &mut Vec<MetricResult>) {
    let country = ctx.params.country.as_deref().map(str::to_ascii_lowercase);
    let country = country.as_deref();
    let market = region::yandex_market(country);
    let r09 = r09_offer(out);
    out.extend([
        r01_yandex_business(ctx, v, market, country),
        r03_zone(v, country),
        r04_landline(v, market, country),
        r05_contacts_page(v),
        r06_channels(v),
        r07_payment(v),
        r08_warranty(v),
        r09,
        r10_social(v),
        r11_keyword_spam(ctx, v),
        r12_hidden_text(v),
        r13_aggressive(v),
        r14_cloaking(v),
        r15_thin_pages(v),
    ]);
    yandex_data(v, out);
}

// ---------- словари ----------

/// Карточка организации в Яндексе: ссылка или виджет на сайте.
const YANDEX_ORG: &[&str] = &[
    "/maps/org/", "yandex.ru/profile/", "yandex.by/profile/", "yandex.kz/profile/", "yandex.ru/maps/-/", "yandex.by/maps/-/",
    "yandex.kz/maps/-/", "yandex.com/maps/-/", "yandex.uz/maps/-/", "maps-reviews-widget", "yandex.ru/sprav/", "map-widget/v1/org/",
];

/// Онлайн-чаты и обратный звонок (R06).
const CHAT_WIDGETS: &[&str] = &[
    "jivosite", "jivo.ru", "b24-widget", "bitrix24", "talk-me", "chatra", "livetex", "redhelper", "envybox", "callbackhunter",
    "carrotquest", "tawk.to", "intercom", "crisp.chat", "tidio", "livechatinc", "zopim", "zdassets", "drift.com", "callibri",
    "leadback", "wazzup", "chat2desk", "verbox", "onicon", "smartsupp", "freshchat", "userlike", "olark", "usemessages.com",
    "chaport", "replain", "umnico",
];

/// Способы оплаты (R07). «=» — целое слово, иначе — с начала слова.
const PAYMENT_TERMS: &[&str] = &[
    "способы оплаты", "способ оплаты", "оплата наличн", "наличными", "наличный расчет", "банковской карт", "банковские карт",
    "оплата карт", "оплатить карт", "безналичн", "=сбп", "системе быстрых платежей", "система быстрых платежей", "рассрочк",
    "оплата частями", "=долями", "оплата по qr", "qr-код для оплаты", "=ерип", "оплата при получении", "apple pay", "google pay", "samsung pay", "mir pay",
    "яндекс пэй", "yandex pay", "=kaspi", "каспи", "=halyk", "=payme", "=uzcard", "оплата по счет", "спосабы аплаты",
    "payment methods", "payment options", "we accept", "pay by card", "credit card", "debit card", "=cash", "=paypal", "=venmo",
    "=zelle", "=klarna", "=afterpay", "contactless", "zahlungsarten", "zahlungsmöglichkeiten", "zahlungsmethoden", "barzahlung",
    "bargeld", "ec-karte", "girocard", "kartenzahlung", "kreditkarte", "auf rechnung", "sofortüberweisung", "moyens de paiement",
    "modes de paiement", "paiement par carte", "carte bancaire", "en espèces", "=chèque", "titres-restaurant", "ticket restaurant",
    "paiement en plusieurs fois", "formas de pago", "métodos de pago", "medios de pago", "en efectivo", "tarjeta de crédito",
    "pago con tarjeta", "=bizum", "datáfono", "metodi di pagamento", "modalità di pagamento", "pagamento con carta", "=contanti",
    "carta di credito", "=bancomat", "satispay", "formy płatności", "metody płatności", "sposoby płatności", "płatność kartą",
    "gotówk", "=blik", "przelew", "formas de pagamento", "métodos de pagamento", "multibanco", "mb way", "em dinheiro",
    "cartão de crédito", "via pix", "com pix", "chave pix", "boleto bancário", "betaalmethoden", "betaalmogelijkheden", "=pinnen",
    "=contant", "met ideal", "via ideal", "ideal-betaling", "creditcard", "tikkie", "achteraf betalen",
];

/// Логотипы платёжных систем в разметке (R07): два разных — способы оплаты показаны.
const PAYMENT_LOGOS: &[&str] = &[
    "mastercard", "maestro", "visa.svg", "visa.png", "visa-logo", "logo-visa", "mir.svg", "mir.png", "mir-logo", "logo-mir",
    "apple-pay", "applepay", "google-pay", "googlepay", "paypal", "unionpay", "belkart", "белкарт", "ideal-logo", "logo-ideal",
];

/// Гарантии и возврат (R08).
const WARRANTY_TERMS: &[&str] = &[
    "гаранти", "возврат денег", "вернем деньги", "обмен и возврат", "возврат и обмен", "возврат товара", "гарантыя", "вяртанне",
    "кепілдік", "warranty", "guarantee", "money-back", "money back", "=refund", "refunds", "return policy", "=returns",
    "garantie", "gewährleistung", "rückgabe", "umtausch", "geld zurück", "remboursement", "satisfait ou rembours",
    "politique de retour", "retours et rembours", "garantía", "devolución", "devoluciones", "reembolso", "garanzia", "=reso", "=resi", "rimborso", "gwarancj",
    "=zwrot", "rękojmi", "reklamacj", "garantia", "devolução", "retourneren", "geld terug",
];

/// Страница контактов (R05): адрес и текст ссылки.
const CONTACT_SLUGS: &[&str] = &["contact", "kontakt", "контакт", "contatti", "contato", "kantakt"];
const CONTACT_TEXT: &[&str] = &[
    "контакты", "кантакты", "контакти", "contacts", "contact", "contact us", "kontakt", "kontakty", "kontakte", "contatti",
    "contacto", "contactos", "contato", "contatos", "contactez-nous", "nous contacter", "neem contact op",
];

/// Сети popunder/clickunder — агрессивная реклама (R13).
const POPUNDER: &[&str] = &[
    "popunder", "pop-under", "clickunder", "click-under", "onclickads", "propellerads", "adcash", "adsterra", "popads.net",
    "popcash.net", "clickadu", "hilltopads", "exoclick", "juicyads", "trafficstars",
];
/// Тизерные сети (R13): на сайте компании — признак рекламы ради рекламы.
const TEASERS: &[&str] = &["mgid.com", "marketgid", "lentainform", "kadam.net", "teasernet", "directadvert", "redtram", "24smi.org", "smi2.ru", "bodyclick"];

/// Юридические и служебные страницы (R11): адрес или заголовок.
const LEGAL_PAGES: &[&str] = &[
    "privacy", "policy", "polit", "политик", "konfid", "конфиденц", "agreement", "soglas", "соглашен", "oferta", "оферт", "terms",
    "cookie", "datenschutz", "impressum", "mentions", "cgv", "cgu", "aviso-legal", "privacidad", "regulamin", "voorwaarden",
    "contact", "kontakt", "контакт", "rekvizit", "реквизит",
];
/// Основы слов юридического текста: страница без говорящего адреса (?page_id=301), но это политика.
const LEGAL_STEMS: &[&str] = &["персон", "обрабо", "person", "privac", "datens", "donnée", "datos", "dati", "dane", "dados", "persoo", "cookie"];

/// Служебные слова от 4 букв (R11): их повтор — не переспам.
const STOPWORDS: &[&str] = &[
    "этот", "этого", "этой", "этом", "того", "тому", "также", "такой", "такие", "более", "менее", "очень", "можно", "нужно",
    "если", "когда", "чтобы", "который", "которые", "которая", "которых", "которой", "после", "перед", "через", "между",
    "только", "всего", "всех", "свой", "своих", "своей", "вашего", "вашей", "наши", "наших", "нашей", "наша", "ваша", "вашу",
    "нашу", "свою", "будет", "были", "было", "может", "могут", "является", "каждый", "любой", "другие", "других", "самый",
    "самые", "есть", "быть", "this", "that", "with", "from", "your", "have", "will", "more", "about", "they", "their", "there",
    "which", "what", "when", "where", "been", "were", "into", "than", "then", "also", "only", "other", "some", "such", "these",
    "those", "most", "over", "very", "just", "each", "them", "here", "would", "could", "should", "after", "before", "while",
    "because", "does", "make", "like", "well", "many", "much", "even", "both", "being", "through", "ours", "yours", "eine",
    "einer", "einen", "einem", "eines", "sind", "nicht", "auch", "oder", "wird", "werden", "wurde", "haben", "sein", "dass",
    "diese", "dieser", "dieses", "mehr", "sich", "noch", "nach", "über", "unter", "beim", "durch", "ihre", "ihren", "ihrem",
    "ihrer", "unsere", "unser", "unseren", "können", "kann", "alle", "sowie", "wenn", "dann", "schon", "sehr", "hier", "immer",
    "pour", "dans", "avec", "sont", "vous", "nous", "plus", "mais", "cette", "leur", "leurs", "tout", "tous", "toutes", "comme",
    "être", "avoir", "sans", "sous", "chez", "votre", "notre", "entre", "aussi", "très", "peut", "fait", "faire", "dont",
    "elle", "elles", "ainsi", "para", "como", "pero", "este", "esta", "estos", "estas", "todo", "todos", "sobre", "también",
    "desde", "hasta", "nuestro", "nuestra", "nuestros", "porque", "cuando", "donde", "está", "están", "puede", "hacer",
    "tiene", "tienen", "ellos", "ellas", "della", "delle", "dello", "degli", "nella", "nelle", "sono", "anche", "questo",
    "questa", "questi", "tutto", "tutti", "molto", "nostro", "nostra", "nostri", "vostro", "perché", "quando", "dove",
    "essere", "avere", "fare", "loro", "alla", "alle", "dalla", "dalle", "jest", "oraz", "przez", "jako", "które", "który",
    "która", "tylko", "także", "bardzo", "może", "można", "nasz", "nasza", "nasze", "naszych", "wasz", "wszystkie", "przy",
    "tego", "takie", "więcej", "gdzie", "kiedy", "nosso", "nossa", "nossos", "muito", "pode", "fazer", "temos", "seus", "suas",
    "pelo", "pela", "voor", "zijn", "niet", "maar", "deze", "onze", "jouw", "meer", "naar", "door", "worden", "wordt", "kunnen",
    "heeft", "hebben", "waar", "wanneer", "omdat", "zeer", "onder",
];

// ---------- помощники ----------

/// Найденные термины: «=слово» — целое слово, иначе — с начала слова.
fn lex_hits<'a>(text: &str, terms: &[&'a str]) -> Vec<&'a str> {
    let boundary = |c: Option<char>| !c.is_some_and(char::is_alphanumeric);
    terms
        .iter()
        .filter(|t| {
            let (whole, term) = match t.strip_prefix('=') {
                Some(x) => (true, x),
                None => (false, **t),
            };
            text.match_indices(term).any(|(i, _)| boundary(text[..i].chars().next_back()) && (!whole || boundary(text[i + term.len()..].chars().next())))
        })
        .copied()
        .collect()
}

/// Ссылки всех обойдённых страниц: (адрес и текст в нижнем регистре, внутренняя ли).
fn all_links<'a>(v: &'a SiteView) -> impl Iterator<Item = (String, String, bool)> + 'a {
    v.pages.iter().flat_map(|p| p.links.iter()).map(|l| (l.href.to_lowercase(), l.text.trim().to_lowercase(), l.internal))
}

fn re(cell: &'static OnceLock<Regex>, p: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(p).unwrap())
}

// ---------- R01–R04: региональность ----------

fn r01_yandex_business(ctx: &CheckCtx, v: &SiteView, market: bool, country: Option<&str>) -> MetricResult {
    if !market {
        return MetricResult::na("R01", format!("Яндекс — не основной поисковик в стране «{}»", country.unwrap_or("не задана")));
    }
    static WIDGET: OnceLock<Regex> = OnceLock::new();
    static ORG: OnceLock<Regex> = OnceLock::new();
    // Ссылка на карточку — в меню, подвале или в sameAs разметки.
    let link = all_links(v).map(|(h, _, _)| h).find(|h| h.contains("yandex.") && YANDEX_ORG.iter().any(|m| h.contains(m))).or_else(|| {
        re(&ORG, r"yandex\.(?:ru|by|kz|uz|com)/(?:maps/org/|profile/|maps/-/)[^\s\x22'<>]*").find(&v.html_all).map(|m| m.as_str().to_string())
    });
    let widget = ["maps-reviews-widget", "yandex.ru/sprav/widget", "map-widget/v1/org/"].iter().any(|m| v.html_all.contains(m))
        || re(&WIDGET, r#"map-widget/v1/[^"'\s>]*oid="#).is_match(&v.html_all);
    let own = registrable_domain(v.final_url.host_str().unwrap_or(""));
    let card = ctx.business.source(ProviderKind::Yandex);
    let card_site = card.and_then(|p| p.website.as_deref()).and_then(|w| url::Url::parse(w).ok()).map(|u| registrable_domain(u.host_str().unwrap_or("")));
    if card_site.as_deref() == Some(own.as_str()) {
        return MetricResult::pass("R01", "Сайт указан в карточке организации в Яндексе");
    }
    match (link, widget, card) {
        (Some(l), _, _) => MetricResult::pass("R01", format!("С сайта есть ссылка на карточку в Яндексе: {}", crate::text::truncate(&l, 80))),
        (None, true, _) => MetricResult::pass("R01", "На сайте виджет карточки или отзывов Яндекса"),
        (None, false, Some(_)) => MetricResult::fail("R01", "Карточка в Яндексе есть, но сайт в ней не указан и с сайта нет ссылки на неё"),
        (None, false, None) => MetricResult::warn(
            "R01",
            "С сайта нет ссылки на карточку в Яндекс Картах — Яндексу труднее связать сайт с организацией и регионом (есть ли карточка, без провайдера Яндекса не видно)",
        ),
    }
}

/// Платформа отдаёт сайт на своём поддомене: зону выбирала не компания.
const PLATFORM_HOSTS: &[&str] = &["tilda.ws", "wixsite.com", "taplink", "ucoz", "narod.ru", "nethouse", "business.site", "webflow.io", "squarespace.com", "github.io"];

fn r03_zone(v: &SiteView, country: Option<&str>) -> MetricResult {
    let host = v.final_url.host_str().unwrap_or("").to_lowercase();
    let Some(country) = country else {
        return MetricResult::na("R03", "Страна скана не задана");
    };
    if PLATFORM_HOSTS.iter().any(|p| host.contains(p)) {
        return MetricResult::na("R03", "Сайт на поддомене конструктора — доменную зону выбирала платформа");
    }
    let tld = host.rsplit('.').next().unwrap_or("").to_string();
    let country = if country == "uk" { "gb" } else { country };
    match region::zone_of(&host) {
        Zone::Country(c) if c == country => MetricResult::pass("R03", format!(".{tld} — зона страны компании")),
        Zone::Country(c) => MetricResult::warn(
            "R03",
            format!(".{tld} — зона другой страны ({}): поисковик по умолчанию отнесёт сайт к ней, регион нужно указать в Вебмастере", c.to_uppercase()),
        )
        .with_value(c),
        Zone::Generic => MetricResult::pass("R03", format!(".{tld} — общая зона: регион задают карточка и Вебмастер (R01, R02)")),
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum PhoneKind {
    Mobile,
    Landline,
    TollFree,
}

/// Номер в международном виде (только цифры) по записи на сайте и стране скана.
pub(crate) fn intl_digits(raw: &str, country: &str) -> Option<String> {
    let t = raw.trim().replace("%2b", "+").replace("%2B", "+");
    let d: String = t.chars().filter(|c| c.is_ascii_digit()).collect();
    if t.starts_with('+') {
        return Some(d);
    }
    if let Some(rest) = d.strip_prefix("00").filter(|_| t.starts_with("00")) {
        return Some(rest.to_string());
    }
    match country {
        "ru" | "kz" if d.len() == 11 && (d.starts_with('8') || d.starts_with('7')) => Some(format!("7{}", &d[1..])),
        "ru" | "kz" if d.len() == 10 => Some(format!("7{d}")),
        // Беларусь: «8 029 …» и «8 017 …» внутри страны, 8-801 — бесплатный.
        "by" if d.len() == 11 && d.starts_with("8801") => Some(format!("375801{}", &d[4..])),
        "by" if d.len() == 11 && d.starts_with("80") => Some(format!("375{}", &d[2..])),
        "by" if d.len() == 12 && d.starts_with("375") => Some(d),
        "by" if d.len() == 9 => Some(format!("375{d}")),
        _ => None,
    }
}

/// Тип номера по плану нумерации России, Казахстана (+7) и Беларуси (+375).
pub(crate) fn phone_kind(intl: &str) -> Option<PhoneKind> {
    if let Some(n) = intl.strip_prefix("375") {
        // Бесплатные 8-801 — на цифру длиннее обычных номеров.
        if n.len() == 10 && n.starts_with("801") {
            return Some(PhoneKind::TollFree);
        }
        if n.len() != 9 {
            return None;
        }
        return match &n[..2] {
            "25" | "29" | "33" | "44" => Some(PhoneKind::Mobile),
            c if c.starts_with('1') || c.starts_with('2') => Some(PhoneKind::Landline),
            _ => None,
        };
    }
    let n = intl.strip_prefix('7').filter(|n| n.len() == 10)?;
    const KZ_MOBILE: &[&str] = &[
        "700", "701", "702", "705", "706", "707", "708", "747", "750", "751", "760", "761", "762", "763", "764", "771", "775",
        "776", "777", "778",
    ];
    match n.as_bytes()[0] {
        b'9' => Some(PhoneKind::Mobile),
        b'7' if KZ_MOBILE.contains(&&n[..3]) => Some(PhoneKind::Mobile),
        b'7' => Some(PhoneKind::Landline),
        b'8' if n.starts_with("800") || n.starts_with("804") => Some(PhoneKind::TollFree),
        b'8' if n.as_bytes()[1] == b'0' => None,
        b'3' | b'4' | b'8' => Some(PhoneKind::Landline),
        _ => None,
    }
}

fn r04_landline(v: &SiteView, market: bool, country: Option<&str>) -> MetricResult {
    let Some(c) = country.filter(|_| market) else {
        return MetricResult::na("R04", "Проверяется там, где Яндекс — основной поисковик (Россия, Беларусь, Казахстан)");
    };
    if !matches!(c, "ru" | "by" | "kz") {
        return MetricResult::na("R04", format!("План нумерации страны «{c}» не разобран"));
    }
    let tel: Vec<&str> = v.pages.iter().flat_map(|p| p.tel_links.iter().map(String::as_str)).collect();
    let mut seen = HashSet::new();
    let mut kinds: Vec<(PhoneKind, String)> = Vec::new();
    for raw in phone_matches(&v.text_all).iter().map(String::as_str).chain(tel.iter().map(|t| t.trim_start_matches("tel:"))) {
        if let Some(intl) = intl_digits(raw, c)
            && seen.insert(intl.clone())
            && let Some(k) = phone_kind(&intl)
        {
            kinds.push((k, format!("+{intl}")));
        }
    }
    if kinds.is_empty() {
        return MetricResult::na("R04", "Номеров на сайте не найдено — см. H01");
    }
    let fixed: Vec<String> =
        kinds.iter().filter(|(k, _)| *k != PhoneKind::Mobile).map(|(k, n)| if *k == PhoneKind::TollFree { format!("{n} (бесплатный)") } else { n.clone() }).collect();
    if fixed.is_empty() {
        let mobiles: Vec<String> = kinds.iter().map(|(_, n)| n.clone()).collect();
        MetricResult::warn("R04", format!("Только мобильные номера ({}) — Яндекс ждёт городской с кодом города или 8-800", list_short(&mobiles, 2))).with_value(0)
    } else {
        MetricResult::pass("R04", format!("Городской или бесплатный номер: {}", list_short(&fixed, 2))).with_value(fixed.len())
    }
}

// ---------- R05–R10: коммерческие факторы ----------

fn r05_contacts_page(v: &SiteView) -> MetricResult {
    let home_path = v.final_url.path().trim_end_matches('/').to_string();
    let mut anchor = false;
    for (href, text, internal) in all_links(v) {
        if !internal {
            continue;
        }
        let Ok(u) = url::Url::parse(&href) else { continue };
        let path = u.path().trim_end_matches('/').to_lowercase();
        let by_slug = CONTACT_SLUGS.iter().any(|s| path.contains(s));
        let by_text = CONTACT_TEXT.contains(&text.as_str());
        if (by_slug || by_text) && path != home_path && !path.is_empty() {
            return MetricResult::pass("R05", format!("Страница контактов: {}", crate::text::truncate(&href, 80)));
        }
        if by_text || u.fragment().is_some_and(|f| CONTACT_SLUGS.iter().any(|s| f.to_lowercase().contains(s))) {
            anchor = true;
        }
    }
    if anchor {
        MetricResult::warn("R05", "Отдельной страницы контактов нет — только блок на главной")
    } else {
        MetricResult::warn("R05", "Страницы «Контакты» не найдено")
    }
}

fn r06_channels(v: &SiteView) -> MetricResult {
    static EMAIL: OnceLock<Regex> = OnceLock::new();
    let phone = v.pages.iter().any(|p| !p.tel_links.is_empty()) || !crate::text::phones_in_text(&v.text_all).is_empty();
    let email = v.pages.iter().any(|p| !p.mailtos.is_empty())
        || re(&EMAIL, r"[a-z0-9._%+-]+@[a-z0-9-]+(?:\.[a-z0-9-]+)*\.[a-z]{2,}")
            .find_iter(&v.text_all)
            .any(|m| ![".png", ".jpg", ".jpeg", ".svg", ".webp", ".gif"].iter().any(|x| m.as_str().ends_with(x)));
    let messenger = crate::lexicon::MESSENGERS.iter().any(|(m, _)| v.html_all.contains(m));
    let form = v.pages.iter().flat_map(|p| p.forms.iter()).any(|f| f.personal && !f.not_lead);
    let chat = CHAT_WIDGETS.iter().any(|m| v.html_all.contains(m));
    let all = [("телефон", phone), ("email", email), ("мессенджер", messenger), ("форма заявки", form), ("онлайн-чат или обратный звонок", chat)];
    let has: Vec<&str> = all.iter().filter(|x| x.1).map(|x| x.0).collect();
    let miss: Vec<&str> = all.iter().filter(|x| !x.1).map(|x| x.0).collect();
    let n = has.len();
    let detail = format!(
        "{n} из 5: {}{}",
        if has.is_empty() { "ничего".to_string() } else { has.join(", ") },
        if miss.is_empty() { String::new() } else { format!("; нет: {}", miss.join(", ")) }
    );
    MetricResult::new("R06", match n { 4.. => Status::Pass, 2 | 3 => Status::Warn, _ => Status::Fail }, detail).with_value(n)
}

fn r07_payment(v: &SiteView) -> MetricResult {
    let words = lex_hits(&v.text_all, PAYMENT_TERMS);
    let logos: HashSet<&str> = PAYMENT_LOGOS.iter().filter(|m| v.html_all.contains(*m)).map(|m| m.split(['.', '-']).next().unwrap_or(m)).collect();
    if !words.is_empty() {
        let w: Vec<String> = words.iter().map(|w| w.trim_start_matches('=').to_string()).collect();
        MetricResult::pass("R07", format!("Способы оплаты указаны: {}", list_short(&w, 3)))
    } else if logos.len() >= 2 {
        MetricResult::pass("R07", "Логотипы платёжных систем на сайте")
    } else {
        MetricResult::warn("R07", "Способы оплаты не описаны (наличные, карта, СБП, рассрочка…)")
    }
}

fn r08_warranty(v: &SiteView) -> MetricResult {
    let words = lex_hits(&v.text_all, WARRANTY_TERMS);
    if words.is_empty() {
        MetricResult::warn("R08", "Гарантий и условий возврата на сайте нет")
    } else {
        let w: Vec<String> = words.iter().map(|w| w.trim_start_matches('=').to_string()).collect();
        MetricResult::pass("R08", format!("Есть: {}", list_short(&w, 3)))
    }
}

/// Широта предложения — по числу упоминаний цен, которое уже посчитал I08.
fn r09_offer(out: &[MetricResult]) -> MetricResult {
    let Some(i08) = out.iter().find(|r| r.id == "I08") else {
        return MetricResult::na("R09", "Цены не проверялись");
    };
    if i08.status == Status::NotMeasured {
        return MetricResult::na("R09", i08.detail.clone());
    }
    let n = i08.value.as_ref().and_then(|x| x.as_u64()).unwrap_or(0);
    // Цен нет вовсе — это провал I08; второй раз за то же не наказываем.
    if n == 0 {
        return MetricResult::na("R09", "Цен на сайте нет — см. I08");
    }
    let st = if n >= 10 { Status::Pass } else { Status::Warn };
    let note = if i08.detail.contains("поддомене") { format!(" ({})", i08.detail) } else { String::new() };
    MetricResult::new("R09", st, format!("Позиций с ценой на обойдённых страницах: {n}{note}")).with_value(n)
}

/// Профиль компании в соцсети (R10): хост и, где нужно, начало пути.
fn social_profile(href: &str) -> Option<&'static str> {
    let u = url::Url::parse(href).ok()?;
    let host = u.host_str()?.trim_start_matches("www.").trim_start_matches("m.").to_lowercase();
    let path = u.path().to_lowercase();
    let query = u.query().unwrap_or("").to_lowercase();
    if ["share", "sharer", "intent/", "/dialog/", "plugins/", "/embed", "widget", "/tr"].iter().any(|m| path.contains(m)) || query.contains("st.cmd=") {
        return None;
    }
    let p = |prefixes: &[&str]| prefixes.iter().any(|x| path.starts_with(x));
    Some(match host.as_str() {
        "vk.com" | "vk.ru" if path.len() > 1 => "VK",
        "ok.ru" if path.len() > 1 => "OK",
        "t.me" | "telegram.me" if path.len() > 1 => "Telegram",
        "dzen.ru" | "zen.yandex.ru" if path.len() > 1 => "Дзен",
        "youtube.com" if p(&["/@", "/channel/", "/c/", "/user/"]) => "YouTube",
        "rutube.ru" if p(&["/channel/", "/u/"]) => "Rutube",
        "instagram.com" if path.len() > 1 => "Instagram",
        "facebook.com" | "fb.com" if path.len() > 1 => "Facebook",
        "tiktok.com" if p(&["/@"]) => "TikTok",
        "x.com" | "twitter.com" if path.len() > 1 => "X",
        "linkedin.com" if p(&["/company/", "/in/"]) => "LinkedIn",
        "pinterest.com" | "pinterest.ru" if path.len() > 1 => "Pinterest",
        "threads.net" if p(&["/@"]) => "Threads",
        _ => return None,
    })
}

fn r10_social(v: &SiteView) -> MetricResult {
    let mut found: Vec<&str> = all_links(v).filter(|(_, _, internal)| !internal).filter_map(|(h, _, _)| social_profile(&h)).collect();
    found.sort();
    found.dedup();
    if found.is_empty() {
        MetricResult::warn("R10", "Ссылок на соцсети компании нет")
    } else {
        MetricResult::pass("R10", found.join(", ")).with_value(found.len())
    }
}

// ---------- R11–R15: нарушения и фильтры ----------

/// Самое частое значимое слово страницы: (основа, вхождений, слов всего).
/// Не считаются: служебные слова, слова короче 4 букв, слова из названия компании, тексты
/// ссылок и кнопок (меню, «Купить» у каждого товара) и слова рядом с числом
/// («30 минут», «1 500 рублей», «250 грамм» — строки прайса и меню).
pub(crate) fn keyword_density(text: &str, exclude_text: &str, own: &HashSet<String>, domain_label: &str) -> Option<(String, usize, usize)> {
    let stem = |w: &str| w.chars().take(6).collect::<String>();
    let numeric = |t: &str| t.chars().all(|c| c.is_ascii_digit());
    // Слово и признак «подпись поля»: сразу за ним двоеточие («Размер: M», «Цена: …» в карточках
    // товаров) — повтор подписей у каждого товара не SEO-текст.
    let mut tokens: Vec<(&str, bool)> = Vec::new();
    let mut start: Option<usize> = None;
    for (i, c) in text.char_indices().chain(std::iter::once((text.len(), ' '))) {
        match (c.is_alphanumeric(), start) {
            (true, None) => start = Some(i),
            (false, Some(st)) => {
                let label = text[i..].trim_start_matches([' ', '\u{a0}']).starts_with(':');
                tokens.push((&text[st..i], label));
                start = None;
            }
            _ => {}
        }
    }
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut total = 0usize;
    for (i, (t, label)) in tokens.iter().enumerate() {
        if numeric(t) {
            continue;
        }
        total += 1;
        let near_number = (i > 0 && numeric(tokens[i - 1].0)) || tokens.get(i + 1).is_some_and(|n| numeric(n.0));
        // Название компании латиницей при кириллическом названии в карточке («MONE» у «Моне»)
        // чаще всего есть в домене: salonmone.ru.
        let in_domain = t.chars().count() >= 4 && domain_label.contains(*t);
        if *label || in_domain || t.chars().count() < 4 || near_number || STOPWORDS.contains(t) || t.chars().any(|c| c.is_ascii_digit()) {
            continue;
        }
        let s = stem(t);
        if !own.contains(&s) {
            *counts.entry(s).or_default() += 1;
        }
    }
    // Тексты ссылок и кнопок вычитаются из счёта и из общего числа слов.
    for t in exclude_text.split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty() && !numeric(t)) {
        total = total.saturating_sub(1);
        if let Some(c) = counts.get_mut(&stem(t)) {
            *c = c.saturating_sub(1);
        }
    }
    let (word, n) = counts.into_iter().max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))?;
    Some((word, n, total))
}

fn r11_keyword_spam(ctx: &CheckCtx, v: &SiteView) -> MetricResult {
    let own: HashSet<String> = format!("{} {}", ctx.business.name, v.final_url.host_str().unwrap_or(""))
        .to_lowercase()
        .replace('ё', "е")
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.chars().count() >= 4)
        .map(|t| t.chars().take(6).collect())
        .collect();
    let host = v.final_url.host_str().unwrap_or("").to_lowercase();
    let domain_label = registrable_domain(&host).split('.').next().unwrap_or("").to_string();
    let mut worst: Option<(f64, String, usize, usize, String)> = None;
    let mut checked = 0;
    for p in &v.pages {
        // Политика, соглашения, контакты: «персональных данных» и «обращений» там повторяются по делу.
        let path = url::Url::parse(&p.url).map(|u| format!("{}?{}", u.path(), u.query().unwrap_or(""))).unwrap_or_default().to_lowercase();
        let title = p.title.as_deref().unwrap_or("").to_lowercase();
        if LEGAL_PAGES.iter().any(|m| path.contains(m) || title.contains(m)) {
            continue;
        }
        let exclude = format!("{} {}", p.links.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join(" "), p.buttons.join(" "))
            .to_lowercase()
            .replace('ё', "е");
        let Some((word, n, total)) = keyword_density(&p.text_lower(), &exclude, &own, &domain_label) else { continue };
        if total < 250 || LEGAL_STEMS.contains(&word.as_str()) {
            continue;
        }
        checked += 1;
        let d = n as f64 / total as f64;
        if n >= 15 && worst.as_ref().is_none_or(|w| d > w.0) {
            worst = Some((d, word, n, total, p.url.clone()));
        }
    }
    if checked == 0 {
        return MetricResult::na("R11", "Страниц с текстом от 250 слов нет — переспам не оценить");
    }
    let Some((d, word, n, total, url)) = worst else {
        return MetricResult::pass("R11", format!("Переспама нет: проверено страниц — {checked}")).with_value(0.0);
    };
    let pct = fmt_num((d * 1000.0).round() / 10.0);
    let detail = format!("«{word}…» — {pct} % ({n} из {total} слов) на {}", crate::text::truncate(&url, 70));
    // Тема страницы повторяется естественно (страница о сертификате — «сертификат» 6 %);
    // переспам — от 7 % и хотя бы 20 повторов.
    let st = if d <= 0.05 { Status::Pass } else if d <= 0.07 || n < 20 { Status::Warn } else { Status::Fail };
    let detail = if st == Status::Fail { format!("{detail}: похоже на SEO-текст, за такое Яндекс понижает") } else { detail };
    MetricResult::new("R11", st, detail).with_value((d * 1000.0).round() / 1000.0)
}

fn r12_hidden_text(v: &SiteView) -> MetricResult {
    let Some(p) = v.pages.iter().max_by_key(|p| p.trick_hidden_text.split_whitespace().count() + 20 * p.trick_hidden_links) else {
        return MetricResult::na("R12", "Страниц нет");
    };
    let words = p.trick_hidden_text.split_whitespace().count();
    let links = p.trick_hidden_links;
    let sample = crate::text::truncate(&p.trick_hidden_text, 80);
    let at = crate::text::truncate(&p.url, 60);
    if words >= 40 || links >= 3 {
        MetricResult::fail("R12", format!("Скрытый от посетителя текст ({words} слов, ссылок: {links}) на {at}: «{sample}»")).with_value(words)
    } else if words >= 10 {
        MetricResult::warn("R12", format!("Немного скрытого текста ({words} слов) на {at}: «{sample}»")).with_value(words)
    } else {
        MetricResult::pass("R12", "Скрытого текста и ссылок не найдено").with_value(0)
    }
}

fn r13_aggressive(v: &SiteView) -> MetricResult {
    let own = registrable_domain(v.final_url.host_str().unwrap_or(""));
    let other_site = |u: &str| url::Url::parse(u).ok().and_then(|x| x.host_str().map(registrable_domain)).filter(|d| *d != own);
    let mut fails: Vec<String> = Vec::new();
    let mut warns: Vec<String> = Vec::new();
    let pop: Vec<&str> = POPUNDER.iter().filter(|m| v.html_all.contains(*m)).copied().collect();
    if !pop.is_empty() {
        fails.push(format!("реклама popunder/clickunder ({})", pop.join(", ")));
    }
    let teasers: Vec<&str> = TEASERS.iter().filter(|m| v.html_all.contains(*m)).copied().collect();
    if !teasers.is_empty() {
        warns.push(format!("тизерная реклама ({})", teasers.join(", ")));
    }
    let frames: Vec<String> = v.pages.iter().flat_map(|p| p.invisible_iframes.iter().cloned()).collect();
    if !frames.is_empty() {
        fails.push(format!("прозрачный фрейм соцсети поверх страницы — кликджекинг ({})", list_short(&frames, 1)));
    }
    if let Some(d) = v.page.meta_refresh.as_deref().and_then(other_site) {
        warns.push(format!("главная сама перенаправляет на {d} (meta refresh)"));
    }
    if let Some(d) = v.snap.mobile_probe.as_ref().filter(|m| m.error.is_none()).and_then(|m| m.final_url.as_deref()).and_then(other_site) {
        warns.push(format!("мобильных посетителей перенаправляют на {d}"));
    }
    let all: Vec<String> = fails.iter().chain(warns.iter()).cloned().collect();
    if !fails.is_empty() {
        MetricResult::fail("R13", format!("Нарушение: {}", all.join("; ")))
    } else if !warns.is_empty() {
        MetricResult::warn("R13", format!("Проверьте: {}", all.join("; ")))
    } else {
        MetricResult::pass("R13", "Навязчивой рекламы, чужих перенаправлений и кликджекинга не найдено")
    }
}

fn r14_cloaking(v: &SiteView) -> MetricResult {
    let Some(b) = v.snap.yandex_bot.as_ref() else {
        return MetricResult::na("R14", "Проба робота Яндекса не выполнялась");
    };
    if let Some(e) = &b.error {
        return MetricResult::na("R14", format!("Запрос с User-Agent робота Яндекса не прошёл: {e}"));
    }
    let status = b.status.unwrap_or(0);
    if b.challenge || matches!(status, 401 | 403 | 429 | 451 | 503) {
        return MetricResult::na(
            "R14",
            format!(
                "Запрос с UA робота Яндекса не с его IP получил {status}{} — так защита и должна работать; ответ настоящему роботу смотрите в Вебмастере («Проверка ответа сервера»)",
                if b.challenge { " (заглушка защиты)" } else { "" }
            ),
        );
    }
    let own = registrable_domain(v.final_url.host_str().unwrap_or(""));
    if let Some(d) = b.final_url.as_deref().and_then(|u| url::Url::parse(u).ok()).and_then(|u| u.host_str().map(registrable_domain)).filter(|d| *d != own) {
        return MetricResult::fail("R14", format!("Робота Яндекса перенаправляют на другой сайт ({d}), посетителя — нет"));
    }
    if status >= 400 {
        return MetricResult::fail("R14", format!("Роботу Яндекса главная отвечает {status}, посетителю — {}", v.home.status));
    }
    let norm = |t: &Option<String>| t.as_deref().map(|x| x.to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ")).unwrap_or_default();
    let bot_title = norm(&b.title);
    let user_title = Some(norm(&v.page.title)).filter(|t| !t.is_empty()).unwrap_or_else(|| norm(&v.snap.js_title));
    let raw_words = v.snap.raw_words.unwrap_or(v.page.words);
    let rendered = v.page.words;
    let title_differs = !bot_title.is_empty() && !user_title.is_empty() && crate::text::similarity(&bot_title, &user_title) < 0.5;
    // Пререндер для роботов: робот получает то, что посетитель видит после выполнения скриптов.
    let prerender = v.snap.raw_words.is_some() && b.words as f64 >= rendered as f64 * 0.6;
    let more = b.words >= raw_words * 2 && b.words >= raw_words + 300 && !prerender;
    let less = raw_words >= 300 && b.words * 2 < raw_words;
    let detail = format!(
        "робот: «{}», {} слов; браузер: «{}», {} слов",
        crate::text::truncate(&bot_title, 50),
        b.words,
        crate::text::truncate(&user_title, 50),
        raw_words
    );
    if title_differs || more || less {
        MetricResult::warn("R14", format!("Роботу Яндекса отдаётся другое содержимое ({detail}) — похоже на клоакинг, проверьте в Вебмастере"))
    } else if prerender {
        MetricResult::pass("R14", format!("Роботу отдаётся готовая (пререндер) версия, совпадающая с тем, что видит посетитель ({detail})"))
    } else {
        MetricResult::pass("R14", format!("Робот и посетитель получают одно и то же ({detail})"))
    }
}

/// Слов вне повторяющихся на страницах сайта блоков (меню, подвал): пятисловные шинглы,
/// которые встречаются на половине страниц и больше, считаются шаблоном.
pub(crate) fn own_words(pages: &[Vec<String>]) -> Vec<usize> {
    let sets: Vec<HashSet<String>> = pages.iter().map(|t| t.windows(5).map(|w| w.join(" ")).collect()).collect();
    let mut freq: HashMap<&str, usize> = HashMap::new();
    for s in &sets {
        for sh in s {
            *freq.entry(sh.as_str()).or_default() += 1;
        }
    }
    let common = ((pages.len() as f64 * 0.5).ceil() as usize).max(2);
    pages
        .iter()
        .map(|t| {
            if t.len() < 5 {
                return t.len();
            }
            let boiler: Vec<bool> = t.windows(5).map(|w| freq.get(w.join(" ").as_str()).copied().unwrap_or(0) >= common).collect();
            // Слово шаблонное, если входит хоть в один шаблонный шингл.
            (0..t.len()).filter(|&i| !(i.saturating_sub(4)..=i.min(boiler.len() - 1)).any(|j| boiler[j])).count()
        })
        .collect()
}

fn r15_thin_pages(v: &SiteView) -> MetricResult {
    if v.pages.len() < 4 {
        return MetricResult::na("R15", "Обойдено меньше 4 страниц — шаблон сайта не выделить");
    }
    let all: Vec<Vec<String>> = v
        .pages
        .iter()
        .map(|p| p.text_lower().split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()).map(str::to_string).collect())
        .collect();
    let own = own_words(&all);
    let utility = |u: &str| {
        let path = url::Url::parse(u).map(|x| x.path().to_lowercase()).unwrap_or_default();
        crate::lexicon::UTILITY_SLUGS.iter().any(|s| path.contains(s))
    };
    let eligible: Vec<(usize, &str)> =
        v.pages.iter().zip(own.iter()).skip(1).filter(|(p, _)| !utility(&p.url) && !p.has_noindex()).map(|(p, n)| (*n, p.url.as_str())).collect();
    if eligible.len() < 3 {
        return MetricResult::na("R15", "Страниц услуг и разделов среди обойдённых меньше трёх");
    }
    let thin: Vec<String> = eligible.iter().filter(|(n, _)| *n < 80).map(|(n, u)| format!("{u} ({n} слов)")).collect();
    let share = thin.len() as f64 / eligible.len() as f64;
    let st = if share <= 0.2 { Status::Pass } else if share <= 0.4 { Status::Warn } else { Status::Fail };
    let detail = if thin.is_empty() {
        format!("У всех {} страниц есть свой текст", eligible.len())
    } else {
        format!("Почти без своего текста {} из {} страниц: {}", thin.len(), eligible.len(), list_short(&thin, 2))
    };
    MetricResult::new("R15", st, detail).with_value((share * 100.0).round() / 100.0)
}

// ---------- C19, R16: открытые данные Яндекса ----------

fn yandex_data(v: &SiteView, out: &mut Vec<MetricResult>) {
    match &v.snap.yandex {
        None => {
            let why = "Яндекс не спрашивался: страна скана и доменная зона — не рынок Яндекса";
            out.push(MetricResult::na("C19", why));
            out.push(MetricResult::na("R16", why));
        }
        Some(info) if info.error.is_some() => {
            let e = info.error.clone().unwrap_or_default();
            out.push(MetricResult::na("C19", e.clone()));
            out.push(MetricResult::na("R16", e));
        }
        Some(info) => {
            let sqi = info.sqi.unwrap_or(0);
            let detail = if sqi == 0 {
                format!("ИКС {}: 0 — ниже порога показа (у небольших и молодых сайтов это обычно)", info.host)
            } else {
                format!("ИКС {}: {sqi}", info.host)
            };
            out.push(MetricResult::info("C19", detail).with_value(sqi));
            let labels: Vec<String> = info.badges.iter().map(|b| b.label()).collect();
            out.push(MetricResult::info("R16", if labels.is_empty() { "Знаков Яндекса нет".to_string() } else { labels.join(", ") }).with_value(labels.len()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phone_kinds_by_numbering_plan() {
        let kind = |raw: &str, c: &str| intl_digits(raw, c).and_then(|d| phone_kind(&d));
        assert_eq!(kind("8 (495) 123-45-67", "ru"), Some(PhoneKind::Landline));
        assert_eq!(kind("+7 812 555-12-34", "ru"), Some(PhoneKind::Landline));
        assert_eq!(kind("8 800 100-20-30", "ru"), Some(PhoneKind::TollFree));
        assert_eq!(kind("+7 (916) 123-45-67", "ru"), Some(PhoneKind::Mobile));
        assert_eq!(kind("+375 29 123-45-67", "by"), Some(PhoneKind::Mobile));
        assert_eq!(kind("8 029 123-45-67", "by"), Some(PhoneKind::Mobile));
        assert_eq!(kind("+375 17 123-45-67", "by"), Some(PhoneKind::Landline));
        assert_eq!(kind("8 017 123-45-67", "by"), Some(PhoneKind::Landline));
        assert_eq!(kind("8 801 100-12-34", "by"), Some(PhoneKind::TollFree));
        assert_eq!(kind("+7 701 123 45 67", "kz"), Some(PhoneKind::Mobile));
        assert_eq!(kind("+7 727 123 45 67", "kz"), Some(PhoneKind::Landline));
        assert_eq!(kind("+49 30 1234567", "ru"), None);
    }

    #[test]
    fn lexicon_word_boundaries() {
        assert_eq!(lex_hits("оплата наличными или картой", PAYMENT_TERMS), vec!["оплата наличн", "наличными"]);
        assert!(lex_hits("cashback 5 %", PAYMENT_TERMS).is_empty());
        assert_eq!(lex_hits("we accept cash and cards", PAYMENT_TERMS), vec!["we accept", "=cash"]);
        assert!(lex_hits("presso il nostro studio", WARRANTY_TERMS).is_empty());
        assert_eq!(lex_hits("гарантия 2 года", WARRANTY_TERMS), vec!["гаранти"]);
        // Ложные срабатывания из живой выборки (24.11): испанское «ideal», «humo», «boleto»,
        // французское «retour à l'accueil» и «espèces» в значении «виды».
        assert!(lex_hits("el regalo ideal para tu piel, sin humo, compra tu boleto", PAYMENT_TERMS).is_empty());
        assert!(lex_hits("toutes les espèces de plantes", PAYMENT_TERMS).is_empty());
        assert!(lex_hits("retour à l'accueil", WARRANTY_TERMS).is_empty());
        assert_eq!(lex_hits("betalen met ideal of creditcard", PAYMENT_TERMS), vec!["met ideal", "creditcard"]);
    }

    #[test]
    fn keyword_density_ignores_prices_and_name() {
        let own: HashSet<String> = ["улыбка".to_string()].into();
        // Строки прайса: слово рядом с числом не считается.
        let price = "чистка зубов 30 минут 2500 рублей ".repeat(30);
        let (_, n, _) = keyword_density(&price, "", &own, "").unwrap();
        assert!(n <= 30, "{n}");
        // Название компании не считается переспамом.
        let name = "стоматология улыбка лучшая клиника города ".repeat(20);
        let (w, _, _) = keyword_density(&name, "", &own, "").unwrap();
        assert_ne!(w, "улыбка");
        // SEO-текст: «стоматология в минске» десятки раз.
        let spam = format!("{} {}", "стоматология в минске недорого ".repeat(40), "мы лечим зубы бережно и качественно ".repeat(20));
        let (w, n, total) = keyword_density(&spam, "", &own, "").unwrap();
        assert!(n as f64 / total as f64 > 0.06, "{w} {n}/{total}");
        // Ложные срабатывания из живой выборки (24.11): подписи в карточках товаров и бренд латиницей.
        let cards = format!("{} {}", "Quiksilver Mission Размер: S, M. Volcom Forty Размер: L. ".repeat(25), "прокат курток и брюк ".repeat(20));
        let (w, _, _) = keyword_density(&cards.to_lowercase(), "", &own, "").unwrap();
        assert_ne!(w, "размер");
        let brand = format!("{} {}", "салон mone на арбате ".repeat(25), "стрижка окрашивание маникюр педикюр ".repeat(10));
        let (w, _, _) = keyword_density(&brand, "", &HashSet::new(), "salonmone").unwrap();
        assert_ne!(w, "mone");
    }

    #[test]
    fn own_words_skip_template() {
        let menu: Vec<String> = "главная услуги цены контакты запись на прием телефон адрес".split(' ').map(String::from).collect();
        let page = |extra: &str| -> Vec<String> { menu.iter().cloned().chain(extra.split(' ').filter(|x| !x.is_empty()).map(String::from)).collect() };
        let pages = vec![page(""), page("лечение кариеса современными материалами под микроскопом без боли и страха"), page(""), page("")];
        let own = own_words(&pages);
        assert_eq!(own[0], 0);
        assert!(own[1] >= 10, "{own:?}");
    }

    #[test]
    fn social_profiles_not_share_buttons() {
        assert_eq!(social_profile("https://vk.com/zubmaster"), Some("VK"));
        assert_eq!(social_profile("https://vk.com/share.php?url=x"), None);
        assert_eq!(social_profile("https://www.facebook.com/sharer/sharer.php?u=x"), None);
        assert_eq!(social_profile("https://www.youtube.com/embed/abc"), None);
        assert_eq!(social_profile("https://www.youtube.com/@clinic"), Some("YouTube"));
        assert_eq!(social_profile("https://dropbox.com/s/file"), None);
        assert_eq!(social_profile("https://t.me/clinic_channel"), Some("Telegram"));
    }
}
