//! Многоязычные словари и шаблоны для проверок: ru, en, de, fr, es, it, nl, pl, pt.
//! Тексты сравниваются в нижнем регистре, ё заменена на е.

use std::sync::OnceLock;

use regex::Regex;

fn re(cell: &'static OnceLock<Regex>, p: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(p).unwrap())
}

pub fn any_in(hay: &str, needles: &[&str]) -> Vec<String> {
    needles.iter().filter(|n| hay.contains(*n)).map(|n| n.to_string()).collect()
}

/// Цена с валютой: «1 500 ₽», «€45», «45,00 €», «$120», «£30», «120 zł».
pub fn price_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(
        &R,
        r"(?i)(?:\d[\d\s\u{a0}.,]{0,9}\s?(?:₽|руб\b|руб\.|рубл\w*|р\.|р\b|€|eur\b|euros?\b|£|\$|usd\b|dollars?\b|chf\b|zł|pln\b|kč|czk\b|kr\b|sek\b|nok\b|dkk\b|₴|грн|₸|lei\b|ft\b|₺|tl\b|byn\b|br\b|бел\.?\s?руб|тенге|тг\b|kzt\b|rub\b|uah\b)|(?:€|£|\$|chf\s?|us\$|fr\.\s?)\s?\d)",
    )
}

/// Признаки адреса в тексте: «ул.», «Street», «Straße», «rue», «calle», «via», индексы.
pub fn address_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(
        &R,
        r"(?i)(\bул\.|\bулиц|\bпр-т|\bпр-кт|\bпросп\.|\bпр\.\s?[а-яё]{4,}[\s,]+(?:д\.\s?)?\d|\bпроспект|\bпер\.|\bпереул|\bшоссе|\bбульвар|\bб-р|\bнаб\.|\bнабережн|\bпл\.|\bплощад|\bмкр|\bмикрорайон|\bд\.\s*\d|\bдом\s+\d|\d+\s+[a-z0-9 .'-]{2,40}?\s(?:street|st|avenue|ave|road|rd|boulevard|blvd|lane|ln|drive|dr|place|pl|court|ct|parkway|pkwy|highway|hwy|way|square|sq|terrace)\b|\b(?:suite|ste\.)\s?\d|[a-zäöüß-]{3,}(?:straße|strasse|str\.|weg|allee|platz|gasse|damm|ufer|ring|chaussee)\s*\d|\b(?:rue|avenue|boulevard|bd|chemin|impasse|allée|place|quai)\s+(?:de\s|du\s|des\s|la\s|l')?\w|\b(?:calle|c/|avenida|avda\.?|plaza|paseo|carrer|rua|travessa|largo)\s+\w|\b(?:via|viale|piazza|corso|vicolo|largo)\s+[a-zà-ù]|\bul\.\s?\w|\baleja\b|[a-z]{3,}(?:straat|laan|weg|plein|gracht|kade|singel)\s*\d|\b[a-z]{2}\s\d{5}(?:-\d{4})?\b|\b\d{5}\s+[a-zäöüß][a-zäöüß-]{2,}|\b[a-z]{1,2}\d[a-z\d]?\s?\d[a-z]{2}\b)",
    )
}

/// Часы работы: «9:00–21:00», «9am–5pm», «круглосуточно», «24/7», «Mo–Fr».
pub fn hours_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(
        &R,
        r"(?i)(\b\d{1,2}[:.]\d{2}\s*(?:uhr|h)?\s*[-–—]\s*\d{1,2}[:.]\d{2}\b|\b\d{1,2}(?::\d{2})?\s?(?:am|pm)\s*[-–—]\s*\d{1,2}(?::\d{2})?\s?(?:am|pm)\b|\b\d{1,2}h\d{0,2}\s*[-–—à]\s*\d{1,2}h|круглосуточно|без выходных|24/7|open 24 hours|24 hours|rund um die uhr|öffnungszeiten|opening hours|business hours|horaires|horario|orari|openingstijden|godziny otwarcia|\bmo\.?\s*[-–]\s*fr\b|\bmon\.?\s*[-–]\s*fri\b|\blun\.?\s*[-–]\s*ven\b|\b\d{1,2}[:.]\d{2}\s*(?:до|to|bis|à|a|al|tot|do|às|till|until)\s*\d{1,2}[:.]\d{2}\b|\b(?:с|от|from|von|de|da|dalle|od)\s*\d{1,2}(?:[:.]\d{2})?\s*(?:до|to|bis|à|a|al|alle|tot|do|às|until)\s*\d{1,2}(?:[:.]\d{2})?\s*(?:ч\b|h\b|uhr|am|pm)?|\b(?:пн|пон\w*)\.?\s*[-–—]\s*(?:пт|птн|пятн\w*|сб|суб\w*|вс)\b|\b(?:mon\w*|montag|lundi|lunes|luned[iì]|poniedzia\w+|segunda|maandag)\s*(?:[-–—]|to|bis|au|a|al|t/m|do|até)\s*(?:fri\w*|sat\w*|freitag|samstag|vendredi|samedi|viernes|s[aá]bado|venerd[iì]|sabato|pi\w+tek|sobota|sexta|vrijdag|zaterdag)|время работы|режим работы|график работы|часы работы|часы приёма|часы приема|heures d.ouverture|öffnungszeit|horarios|hor[aá]rio de|godzin\w* pracy)",
    )
}

/// Ссылка или текст ведут на политику конфиденциальности.
pub const PRIVACY: &[&str] = &[
    "политик", "конфиденц", "персональн", "privacy", "datenschutz", "confidentialit", "privacidad", "riservatezza",
    "privacybeleid", "privacyverklaring", "prywatno", "rgpd", "gdpr", "dsgvo", "données personnelles",
    "donnees-personnelles", "protección de datos", "protezione dei dati", "politika", "policy",
];

/// Слова согласия рядом с формой или галочкой.
pub const CONSENT: &[&str] = &[
    "соглас", "персональн", "политик", "конфиденц", "consent", "agree", "i accept", "privacy", "datenschutz",
    "einwillig", "zustimm", "einverstanden", "j'accepte", "j’accepte", "consens", "acepto", "accetto", "akceptuj",
    "zgod", "akkoord", "gdpr", "dsgvo", "rgpd", "terms",
];

/// Платформы согласия на cookie (CMP) — по коду страницы.
pub const CMP: &[&str] = &[
    "cookiebot", "onetrust", "cookielaw.org", "usercentrics", "didomi", "quantcast.mgr", "quantcast choice", "trustarc",
    "iubenda", "complianz", "cookieyes", "cookie-script", "borlabs", "osano", "termly", "axeptio", "tarteaucitron",
    "klaro", "consentmanager", "cookiefirst", "cookiehub", "fundingchoicesmessages", "cookie-law-info",
    "cookie-notice", "moove_gdpr", "real-cookie-banner", "gdpr-cookie", "cookieconsent", "cookie-consent", "ccm19",
    "sirdata", "consentframework", "__tcfapi", "cmp-banner", "shopify-pc__banner", "wix-cookie",
];

/// Веб-аналитика (P01 вне России и сопутствующие проверки).
pub const ANALYTICS: &[(&str, &str)] = &[
    ("mc.yandex.ru", "Яндекс Метрика"),
    ("googletagmanager.com/gtag", "Google Analytics 4"),
    ("googletagmanager.com/ns.html", "Google Tag Manager"),
    ("tilda-stat", "Статистика Tilda"),
    ("bitrix/tools/conversion/ajax_counter", "Статистика Bitrix"),
    ("shopifyanalytics", "Shopify Analytics"),
    ("web-pixels-manager", "Shopify Analytics"),
    ("google-analytics.com", "Google Analytics"),
    ("googletagmanager.com/gtm", "Google Tag Manager"),
    ("matomo.js", "Matomo"),
    ("piwik.js", "Matomo"),
    ("plausible.io", "Plausible"),
    ("usefathom.com", "Fathom"),
    ("simpleanalytics", "Simple Analytics"),
    ("static.hotjar.com", "Hotjar"),
    ("clarity.ms", "Microsoft Clarity"),
    ("static.cloudflareinsights.com", "Cloudflare Web Analytics"),
    ("umami", "Umami"),
    ("pirsch.io", "Pirsch"),
    ("adobedtm.com", "Adobe Analytics"),
    ("omtrdc.net", "Adobe Analytics"),
    ("cdn.segment.com", "Segment"),
    ("heapanalytics", "Heap"),
    ("mixpanel", "Mixpanel"),
    ("counter.yadro.ru", "LiveInternet"),
    ("top-fwz1.mail.ru", "Top@Mail.ru"),
    ("stats.wp.com", "Jetpack Stats"),
    ("wix.com/_api/wix-analytics", "Wix Analytics"),
    ("squarespace.com/universal/scripts-compressed/performance", "Squarespace Analytics"),
];

/// Отзывы, кейсы, портфолио (M07).
pub const PROOF: &[&str] = &[
    "отзыв", "кейс", "портфолио", "наши работы", "до и после", "примеры работ", "reviews", "testimonial",
    "what our clients say", "what our customers say", "what our patients say", "bewertung", "kundenstimmen",
    "erfahrungen", "referenzen", "avis client", "témoignage", "temoignage", "reseñas", "opiniones", "testimonios",
    "recensioni", "opinie", "beoordelingen", "portfolio", "our work", "before and after", "vorher nachher",
    "avant après", "antes y después", "prima e dopo",
];

/// Команда и специалисты (M08).
pub const TEAM: &[&str] = &[
    "команда", "наши врачи", "наши специалисты", "специалист", "мастера", "сотрудник", "о враче", "our team",
    "meet the team", "our doctors", "our staff", "our stylists", "our barbers", "our dentists", "unser team",
    "das team", "unsere ärzte", "notre équipe", "nuestro equipo", "il nostro team", "ons team", "nasz zespół",
    "a nossa equipa", "nossa equipe",
];

/// Признаки интернет-магазина (M10).
pub const SHOP: &[&str] = &[
    "в корзину", "оформить заказ", "add to cart", "add to bag", "in den warenkorb", "warenkorb", "ajouter au panier",
    "añadir al carrito", "aggiungi al carrello", "do koszyka", "in winkelwagen", "adicionar ao carrinho",
];

/// Условия продажи текстом (M10): доставка, оплата, возврат — на основных языках.
pub const DELIVERY: &[&str] = &[
    "доставк", "delivery", "shipping", "versand", "lieferung", "livraison", "envío", "envio", "spedizion", "consegna", "dostaw",
    "verzend", "bezorg", "entrega",
];
pub const PAYMENT: &[&str] = &[
    "оплат", "payment", "zahlung", "bezahl", "paiement", "pago", "pagament", "płatno", "platno", "betal",
];
pub const RETURNS: &[&str] = &[
    "возврат", "обмен", "гаранти", "return", "refund", "exchange", "rückgabe", "ruckgabe", "widerruf", "umtausch", "retour",
    "rembours", "devoluci", "rimbors", "recesso", "zwrot", "reklamac", "garantie", "warranty", "garantía", "garanzia",
    "gwarancj", "troca", "devolução",
];

/// Ссылки на условия продажи (M10).
pub const SHOP_TERMS: &[&str] = &[
    "оферт", "возврат", "доставк", "oferta", "dostavka", "vozvrat", "terms", "conditions", "refund", "return",
    "shipping", "agb", "widerruf", "versand", "cgv", "livraison", "términos", "devoluci", "envío", "condizioni",
    "reso", "spedizion", "regulamin", "voorwaarden", "retour", "публичный договор", "договор публичн", "публічны дагавор",
    "dogovor", "условия продаж", "правила продаж", "pravila-prodazh", "гарантия", "обмен", "garantiya", "guarantee",
];

/// Кнопки действия (N01).
pub const CTA: &[&str] = &[
    "запис", "заказ", "заявк", "позвон", "рассчит", "получить", "купить", "бронир", "консультац", "вызвать",
    "связаться", "book", "appointment", "schedule", "call", "contact", "quote", "order", "reserve", "buy",
    "get started", "request", "termin", "buchen", "anrufen", "kontakt", "anfrage", "reservier", "bestell", "réserv",
    "rendez-vous", "appeler", "devis", "commander", "reservar", "cita", "llamar", "pedir", "prenot", "chiama",
    "preventivo", "umów", "zadzwoń", "afspraak", "bellen", "marcar", "agendar",
];

/// Виджеты онлайн-записи и бронирования (N04) — по коду страницы.
pub const BOOKING_WIDGETS: &[&str] = &[
    "yclients", "dikidi", "sonline", "medflex", "napopravku", "restoplace", "bookform", "online-zapis",
    "onlinezapis", "booksy", "prodoctorov.ru/widget", "zapis.online", "calendly.com", "acuityscheduling",
    "fresha.com", "treatwell", "vagaro", "mindbodyonline", "squareup.com/appointments", "square.site/book",
    "setmore", "simplybook", "gettimely", "schedulicity", "styleseat", "opentable", "resy.com", "sevenrooms",
    "thefork", "quandoo", "doctolib", "zocdoc", "jameda", "planity", "shore.com", "etermin", "terminland",
    "salonized", "phorest", "zenoti", "joinblvd", "glossgenius", "getsquire", "booker.com", "exploretock",
    "nexhealth", "localmed", "doctoralia", "miodottore", "znanylekarz", "samedi.de", "dentolo", "tock.com",
    "youcanbook.me", "appointy", "10to8", "bookeo", "picktime", "hubspot.com/meetings", "calendar.app.google",
];

/// Текст об онлайн-записи (N04).
pub const BOOKING_TEXT: &[&str] = &[
    "записаться онлайн", "онлайн-запись", "онлайн запись", "book online", "book now", "book an appointment",
    "schedule online", "schedule an appointment", "request an appointment", "online buchen", "termin online",
    "online-termin", "jetzt buchen", "termin buchen", "réserver en ligne", "prendre rendez-vous", "reservar online",
    "reserva online", "pide cita", "pedir cita", "prenota online", "umów wizytę", "online afspraak",
    "marcar consulta",
];

/// Сайт-заглушка или шаблонный текст (F14, B01).
pub const PLACEHOLDER: &[&str] = &[
    "сайт в разработке", "сайт находится в разработке", "находится в разработке", "в стадии разработки",
    "на стадии разработки", "сайт на реконструкции", "находится на реконструкции", "under construction",
    "we'll be back soon", "we will be back soon", "under maintenance", "maintenance mode", "ведутся технические работы",
    "сайт на техническом обслуживании", "крама не працуе", "сайт у распрацоўцы", "старонка ў распрацоўцы",
    "магазин не работает", "магазин временно не работает", "магазин закрыт", "coming soon", "website coming soon",
    "скоро открытие сайта", "сайт скоро заработает", "lorem ipsum", "site en construction", "bientôt disponible",
    "seite im aufbau", "website im aufbau", "webseite im aufbau", "in kürze verfügbar", "en construcción",
    "próximamente", "sitio en construcción", "in costruzione", "sito in costruzione", "prossimamente", "w budowie",
    "strona w budowie", "in aanbouw", "binnenkort online", "em construção", "launching soon", "blank page",
    // Заглушка Hover («… is a totally awesome idea still being worked on. Check back later.»).
    "still being worked on", "awesome idea still",
];

/// Ошибка движка или базы данных вместо главной (B01).
pub const SITE_ERRORS: &[&str] = &[
    "no connect!", "error establishing a database connection", "ошибка подключения к базе", "ошибка соединения с базой",
    "could not connect to database", "database connection error", "database error", "mysql_connect", "fatal error:",
    "parse error:", "service unavailable", "internal server error", "there has been a critical error on this website",
    "erreur de connexion à la base de données", "erreur lors de la connexion à la base de données",
    "il y a eu une erreur critique sur ce site", "error al establecer una conexión con la base de datos",
    "ha habido un error crítico en esta web", "fehler beim aufbau einer datenbankverbindung",
    "es gab einen kritischen fehler auf deiner website", "errore nello stabilire una connessione al database",
    "c'è stato un errore critico su questo sito", "błąd podczas nawiązywania połączenia z bazą danych",
];

/// Только шаблонный текст конструкторов и CMS (F14).
pub const DUMMY: &[&str] = &[
    "lorem ipsum", "здесь будет текст", "текст-рыба", "заголовок блока", "your title here", "sample page",
    "hello world!", "just another wordpress site", "ещё один сайт на wordpress", "еще один сайт на wordpress",
    "this is a sample", "add your text here", "click here to edit", "this is your website", "your business name",
    "ihr text hier", "votre texte ici", "tu texto aquí",
];

/// Парковка домена, заглушки хостинга (B01).
pub const PARKED: &[&str] = &[
    "домен продается", "домен продаётся", "this domain is for sale", "this domain may be for sale", "domain for sale",
    "domain is parked", "buy this domain", "купить домен", "account suspended", "аккаунт заблокирован",
    "сайт не обслуживается", "domain has been assigned", "please renew your subscription", "renew your subscription",
    "start.hoster.by", "сайт заблокирован за неуплату", "срок оплаты хостинга", "forsale.godaddy", "выставлен на продажу",
    "парковка доменов", "is for sale", "dan.com", "afternic", "sedo.com", "/lander",
    // GoDaddy Corporate Domains: «Protected Domain Holder» (gcd.com) — домен держат, сайта нет.
    "protected domain holder", "gcd.com/protected-domain", "protected by godaddy corporate domains",
    "default web page", "welcome to nginx", "apache2 ubuntu default page", "apache2 debian default page",
    "web server's default page", "no web site at this address", "test page for the apache", "iis windows server",
    "index of /", "сайт заблокирован", "срок регистрации домена истек", "срок регистрации домена истёк",
    "домен не делегирован", "хостинг заблокирован", "diese domain kaufen", "domain kaufen", "domain steht zum verkauf",
    "ce domaine est à vendre", "este dominio está a la venta", "questo dominio è in vendita", "parked free",
    "is parked for free", "hugedomains", "sedoparking", "this site can’t be reached", "website expired",
    "this account has been suspended", "future home of", "plesk default page", "cpanel default",
];

/// Страница «О компании» (I10) — в адресе или тексте ссылки.
pub const ABOUT: &[&str] = &[
    "о компании", "о нас", "о клинике", "о салоне", "/about", "o-kompanii", "o-nas", "o-klinike", "about-us",
    "about us", "über uns", "ueber-uns", "uber-uns", "/praxis", "a-propos", "à propos", "qui-sommes-nous",
    "qui sommes-nous", "sobre nosotros", "quienes-somos", "quiénes somos", "chi siamo", "chi-siamo", "over ons",
    "over-ons", "o nas", "sobre-nos", "sobre nós", "/team", "/equipe", "/equipo", "/company", "о-нас", "о-компании", "о-клинике",
    "/o-nas", "/o-firmie", "/sobre", "/nosotros", "/chi-siamo", "/over-ons", "/uber-uns", "/uberuns",
];

/// Адрес без %-кодирования («/%D0%BE-%D0%BD%D0%B0%D1%81» → «/о-нас»).
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Ссылка ведёт на страницу «О компании»: по адресу, тексту ссылки или короткой подписи «About».
pub fn is_about_link(href: &str, text: &str) -> bool {
    let t = text.trim().to_lowercase();
    let h = percent_decode(href).to_lowercase();
    let s = format!("{h} {t}");
    ABOUT.iter().any(|m| s.contains(m))
        || matches!(t.trim_end_matches(['.', '!']), "about" | "company" | "компания" | "о нас" | "über uns" | "à propos" | "acerca de" | "chi siamo" | "over ons" | "sobre nós" | "o nas" | "o firmie")
}

/// Разделы статей и новостей (F19, I11) — части адреса.
pub const BLOG: &[&str] = &[
    "/blog", "/news", "/novosti", "/stati", "/articles", "/article", "/statyi", "/journal", "/poleznoe", "/aktuelles",
    "/neuigkeiten", "/ratgeber", "/magazin", "/actualites", "/actualités", "/noticias", "/notizie", "/artikel",
    "/aktualnosci", "/nieuws", "/insights", "/tips",
];

/// Страницы услуг и цен (F07) — части адреса.
pub const SERVICE_SLUGS: &[&str] = &[
    "uslug", "service", "услуг", "price", "ceny", "tseny", "prais", "napravlen", "lechenie", "catalog", "katalog",
    "remont", "/vidy", "leistung", "behandlung", "preise", "tarif", "prestations", "soins", "servicios", "precios",
    "servizi", "prezzi", "uslugi", "cennik", "diensten", "treatment", "/menu", "pricing", "behandlungen",
    "angebot", "procedures", "tratamientos", "trattamenti", "menyu", "меню", "/rooms", "nomera", "номера", "products",
    "tovary", "товары", "assortiment", "ассортимент",
];

/// Служебные страницы — не услуга и не товар (F07).
pub const UTILITY_SLUGS: &[&str] = &[
    "contact", "kontakt", "контакт", "about", "o-nas", "o-kompanii", "о-нас", "о-компании", "company", "policy", "privacy",
    "politika", "конфиденц", "cookie", "agreement", "soglasie", "oferta", "оферт", "terms", "cart", "korzin", "корзин",
    "basket", "checkout", "order", "zakaz", "login", "auth", "register", "account", "personal", "/lk", "search", "poisk",
    "поиск", "blog", "news", "novosti", "новост", "stati", "статьи", "article", "tag/", "author", "page/", "feed", "sitemap",
    "wp-", "vacanc", "vakans", "ваканс", "career", "faq", "otzyv", "отзыв", "review", "impressum", "datenschutz",
    "rekvizit", "реквизит", "dostavka", "доставк", "delivery", "oplata", "оплат", "payment", "garant", "гарант",
    "mentions", "cgv", "panier", "connexion", "recherche", "carrito", "aviso-legal", "privacidad", "agb", "warenkorb",
    "carrello", "koszyk", "winkelwagen", "carrinho", "contacto", "contatti", "contato", "livraison", "envio", "versand",
];

/// Ссылки на файлы с прайсом или меню (I07): адрес или текст ссылки.
pub const PRICE_FILE_WORDS: &[&str] = &[
    "price", "prais", "прайс", "цен", "preis", "tarif", "prix", "precio", "prezz", "cennik", "menu", "меню", "speisekarte",
    "carta", "kaart",
];

/// Бесплатные почтовые сервисы (M09, поиск сайта по почте).
/// Домен бесплатной почты: из списка или страновой вариант крупного почтовика (yandex.by,
/// yahoo.es, hotmail.it…). Такой домен — не сайт компании.
pub fn is_free_email(domain: &str) -> bool {
    let d = domain.trim().to_lowercase();
    FREE_EMAIL.contains(&d.as_str())
        || ["yandex.", "ya.", "yahoo.", "hotmail.", "outlook.", "live.", "gmx.", "mail.", "msn.", "icloud.", "aol."]
            .iter()
            .any(|p| d.strip_prefix(p).is_some_and(|rest| !rest.contains('.') || rest.starts_with("co.") || rest.starts_with("com.")))
}

pub const FREE_EMAIL: &[&str] = &[
    "gmail.com", "googlemail.com", "mail.ru", "yandex.ru", "yandex.com", "ya.ru", "bk.ru", "list.ru", "inbox.ru",
    "rambler.ru", "internet.ru", "outlook.com", "hotmail.com", "live.com", "msn.com", "yahoo.com", "ymail.com",
    "icloud.com", "me.com", "mac.com", "aol.com", "gmx.de", "gmx.net", "gmx.com", "web.de", "t-online.de",
    "freenet.de", "orange.fr", "free.fr", "laposte.net", "sfr.fr", "wanadoo.fr", "libero.it", "virgilio.it",
    "alice.it", "tiscali.it", "wp.pl", "o2.pl", "onet.pl", "interia.pl", "seznam.cz", "btinternet.com",
    "sky.com", "virginmedia.com", "protonmail.com", "proton.me", "zoho.com", "comcast.net", "att.net",
    "verizon.net", "sbcglobal.net", "bluewin.ch", "ukr.net", "tut.by", "hotmail.de", "hotmail.fr", "yahoo.de",
    "yahoo.fr", "yahoo.co.uk", "outlook.de", "telefonica.net", "terra.es", "163.com", "126.com", "qq.com", "sina.com",
    "yeah.net", "mail.by", "rambler.by", "i.ua", "meta.ua", "bigmir.net", "inbox.lv", "naver.com", "daum.net", "hanmail.net",
    "posteo.de", "mailbox.org", "tutanota.com", "tuta.io", "fastmail.com", "aon.at", "a1.net", "rediffmail.com",
];

/// Коллтрекинг (P04).
pub const CALLTRACKING: &[(&str, &str)] = &[
    ("calltouch", "Calltouch"),
    ("comagic", "CoMagic"),
    ("uiscom", "UIS"),
    ("mango-office", "Mango Office"),
    ("callibri", "Callibri"),
    ("roistat", "Roistat"),
    ("alloka", "Alloka"),
    ("ringostat", "Ringostat"),
    ("callrail", "CallRail"),
    ("calltrackingmetrics", "CallTrackingMetrics"),
    ("tctm.co", "CallTrackingMetrics"),
    ("invoca", "Invoca"),
    ("whatconverts", "WhatConverts"),
    ("marchex", "Marchex"),
    ("ringba", "Ringba"),
    ("delacon", "Delacon"),
    ("matelso", "Matelso"),
    ("responsetap", "ResponseTap"),
    ("mediahawk", "Mediahawk"),
];

/// Рекламные пиксели (P05). top-fwz1.mail.ru не берём: его же использует обычный счётчик Top@Mail.ru.
pub const PIXELS: &[(&str, &str)] = &[
    ("vk.com/rtrg", "VK Реклама"),
    ("an.yandex.ru", "Яндекс Реклама"),
    ("connect.facebook.net", "Meta Pixel"),
    ("analytics.tiktok.com", "TikTok Pixel"),
    ("googleadservices", "Google Ads"),
    ("googleads.g.doubleclick", "Google Ads"),
    ("snap.licdn.com", "LinkedIn Insight"),
    ("s.pinimg.com/ct", "Pinterest Tag"),
    ("pintrk", "Pinterest Tag"),
    ("sc-static.net/scevent", "Snap Pixel"),
    ("static.ads-twitter.com", "X Ads"),
    ("bat.bing.com", "Microsoft Ads"),
    ("redditstatic.com/ads", "Reddit Pixel"),
    ("criteo", "Criteo"),
    ("taboola", "Taboola"),
    ("outbrain", "Outbrain"),
];

/// Мессенджеры (N03): часть ссылки → название.
pub const MESSENGERS: &[(&str, &str)] = &[
    ("t.me/", "Telegram"),
    ("telegram.me", "Telegram"),
    ("wa.me", "WhatsApp"),
    ("api.whatsapp.com", "WhatsApp"),
    ("whatsapp://", "WhatsApp"),
    ("max.ru/", "MAX"),
    ("vk.me", "VK"),
    ("viber://", "Viber"),
    ("m.me/", "Messenger"),
    ("line.me", "LINE"),
    ("signal.me", "Signal"),
];

/// Встроенные карты (H04) — по коду страницы.
pub const MAP_WIDGETS: &[&str] = &[
    "api-maps.yandex", "yandex.ru/map-widget", "yandex.ru/maps/?", "google.com/maps/embed", "maps.google",
    "maps.googleapis.com", "google.com/maps/d/embed", "2gis.ru/widget", "widgets.2gis", "openstreetmap.org/export",
    "api.mapbox.com", "leaflet", "maps.apple.com", "mapkit.js", "here.com/maps", "bing.com/maps/embed",
];

/// Ссылки на карточки на картах и отзовиках (H05).
pub const CARD_LINKS: &[&str] = &[
    "yandex.ru/maps/org", "yandex.ru/profile", "yandex.com/maps/org", "google.com/maps", "maps.app.goo.gl", "g.page",
    "goo.gl/maps", "search.google.com/local/writereview", "2gis.ru/", "2gis.com/", "zoon.ru", "prodoctorov.ru",
    "yelp.com/biz", "yelp.", "tripadvisor.", "trustpilot.com/review", "maps.apple.com", "jameda.de",
    "doctolib.", "zocdoc.com", "treatwell.", "fresha.com", "provenexpert.com", "golocal.de", "pagesjaunes.fr",
];

/// Профили компании, связывающие сайт с сущностью (I13).
pub const PROFILE_LINKS: &[&str] = &[
    "yandex.ru/maps", "google.com/maps", "maps.app.goo.gl", "2gis.", "vk.com", "t.me/", "facebook.com/",
    "instagram.com/", "yelp.com/biz", "tripadvisor.", "linkedin.com/company", "wikidata.org", "youtube.com/",
    "tiktok.com/@",
];

/// Виджеты отзывов и рейтингов (N06) — по коду страницы.
pub const REVIEW_WIDGETS: &[&str] = &[
    "maps-reviews-widget", "reviews-widget", "★", "elfsight", "trustindex", "trustpilot", "birdeye", "podium.com",
    "yotpo", "judge.me", "provenexpert", "ekomi", "trustedshops", "reviews.io", "grade.us", "embedsocial",
    "featurable", "widget.reviews", "sociablekit", "jameda-siegel", "google-reviews",
    // Бейдж рейтинга Яндекс Карт и виджеты 2ГИС (belstom.by: iframe yandex.ru/sprav/widget/rating-badge).
    "sprav/widget/rating-badge", "rating-badge", "widget.2gis", "2gis.ru/widget", "reviews-widget.2gis",
    // Самодельные блоки отзывов (brooklynburgersandbeer.com: div.reviewsBlock со звёздами).
    "reviewsblock", "reviews-block", "review-item", "reviews-list", "review-card", "testimonial-item", "otzyvy-block",
];

/// Слова «отзыв» на главной (N06).
pub const REVIEW_WORDS: &[&str] = &[
    "отзыв", "review", "testimonial", "bewertung", "kundenstimmen", "avis", "témoignage", "reseña", "opinion",
    "recension", "opinie", "beoordeling", "avaliaç",
];

/// Юридическая информация, обязательная в стране (M05): (страна, маркеры, что именно, обязательно ли).
pub fn legal_notice(country: &str) -> Option<(&'static [&'static str], &'static str, bool)> {
    Some(match country {
        "de" | "at" => (&["impressum"][..], "Impressum (§ 5 DDG / § 5 ECG)", true),
        "ch" => (&["impressum", "mentions légales", "note legali"][..], "Impressum (UWG)", true),
        "fr" | "lu" => (&["mentions légales", "mentions legales", "mentions-legales", "siret", "siren"][..], "Mentions légales (LCEN)", true),
        "be" => (&["mentions légales", "mentions legales", "btw", "tva be", "kbo", "bce", "ondernemingsnummer", "numéro d'entreprise"][..], "номер предприятия (KBO/BCE)", true),
        "it" => (&["p.iva", "p. iva", "partita iva", "p.i.", "c.f.", "codice fiscale"][..], "Partita IVA", true),
        "es" => (&["aviso legal", "aviso-legal", "cif", "nif", "razón social"][..], "Aviso legal (LSSI)", true),
        "pt" => (&["nif", "nipc", "termos e condições", "livro de reclamações"][..], "NIF и Livro de Reclamações", true),
        "nl" => (&["kvk", "kamer van koophandel", "btw-id", "btw nummer", "btw-nummer"][..], "номер KvK", true),
        "pl" => (&["nip", "regon", "krs"][..], "NIP / REGON / KRS", true),
        "cz" | "sk" => (&["ičo", "ico:", "dič", "ičo:"][..], "IČO", true),
        "gb" | "uk" | "ie" => (&["company number", "company no", "registered in england", "registered in scotland", "registered office", "company registration", "cro no", "registered in ireland", "vat no", "vat number", "vat reg"][..], "номер компании и юр. адрес (Companies Act)", false),
        "se" | "no" | "dk" | "fi" => (&["org.nr", "org. nr", "organisationsnummer", "organisasjonsnummer", "cvr", "y-tunnus", "momsnr", "vat"][..], "организационный номер", false),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prices_in_many_currencies() {
        for s in ["от 1 500 ₽", "€45", "45,00 €", "$120", "£30", "120 zł", "1500 руб.", "chf 80", "30 eur", "45 byn", "от 25,50 br", "120 бел. руб.", "3 500 тенге"] {
            assert!(price_re().is_match(s), "{s}");
        }
        assert!(!price_re().is_match("2019 год, 15 врачей"));
        // Сокращение «р» без точки (dentalelit.by: «от 40 р», «от 1500 р»).
        assert!(price_re().is_match("пломба от 40 р, коронка от 950 р"));
        assert!(!price_re().is_match("2 раза в год, 3 ребёнка"));
        assert!(PLACEHOLDER.iter().any(|m| "сайт mz.by находится в разработке".contains(m)), "заглушка с доменом в тексте");
        for d in ["yandex.by", "163.com", "tut.by", "yahoo.es", "hotmail.co.uk", "mail.ru", "gmail.com"] {
            assert!(is_free_email(d), "{d}");
        }
        for d in ["sosedi.by", "mailchimp.com", "yandexmaps.ru", "zub.ru", "mail.zub.ru"] {
            assert!(!is_free_email(d), "{d}");
        }
    }

    #[test]
    fn addresses_in_many_languages() {
        for s in [
            "ул. тверская, 1",
            "484 tompkins avenue, brooklyn",
            "1251 fulton st. brooklyn",
            "oranienstraße 12, 10999 berlin",
            "hauptstr. 5",
            "12 rue de rivoli",
            "calle mayor 5",
            "via roma 10",
            "keizersgracht 12",
            "brooklyn, ny 11216",
            "london sw1a 1aa",
            // kapadent.by: «пр.» с точкой вместо «пр-т».
            "г. минск, пр. дзержинского, 9, пом. 1037",
            "просп. независимости, 58",
        ] {
            assert!(address_re().is_match(s), "{s}");
        }
        assert!(!address_re().is_match("мы работаем с 2010 года"));
        assert!(!address_re().is_match("скидки на чистку и пр. услуги"));
    }

    #[test]
    fn opening_hours() {
        for s in [
            "9:00–21:00",
            "9am - 5pm",
            "mo-fr 8.00 - 18.00 uhr",
            "круглосуточно",
            "open 24 hours",
            "9h-19h",
            "Время работы: с 8:00 до 21:00 понедельник-пятница, суббота по графику",
            "пн-птн 10.00-20.00, сб: 10.00-15.00",
            "Mon–Fri 9am–6pm",
            "Monday to Friday, 9 to 5",
            "Lundi au vendredi 9h–18h",
            "Lunes a viernes de 9:00 a 18:00",
            "Montag bis Freitag 8 bis 17 Uhr",
            "Lunedì-venerdì dalle 9 alle 18",
            "Poniedziałek–piątek 9:00 do 17:00",
            "Segunda a sexta das 9h às 18h",
            "Maandag t/m vrijdag 9:00 tot 17:00",
        ] {
            assert!(hours_re().is_match(s), "{s}");
        }
    }

    #[test]
    fn legal_notice_by_country() {
        assert_eq!(legal_notice("de").unwrap().1, "Impressum (§ 5 DDG / § 5 ECG)");
        assert!(legal_notice("us").is_none());
        assert!(!legal_notice("gb").unwrap().2);
    }
}

// ---------- I17–I20: что помогает ИИ процитировать страницу ----------

/// Видимая дата обновления (I17): «обновлено 12.03.2026», «Stand: März 2026», «last updated May 4, 2026».
/// Ключевое слово, затем дата: число.месяц.год, ISO, «12 марта 2026», «March 2026» или год.
pub fn updated_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    const MONTH: &str = r"(?:январ|феврал|март|апрел|ма[йя]|июн|июл|август|сентябр|октябр|ноябр|декабр|jan|feb|mar|apr|may|jun|jul|aug|sep|oct|nov|dec|janv|f[eé]vr|avr|mai|juil|ao[uû]t|d[eé]c|ene|abr|ago|dic|m[aä]rz|mrz|okt|dez|gen|mag|giu|lug|set|ott|sty|lut|kwi|cze|lip|sie|wrz|pa[zź]|lis|gru|fev|out|mei|maa)[a-zа-яäöüéèêàáçãõóíúñßąćęłńśźż]*\.?";
    let date = format!(
        r"(?:\d{{1,2}}[./-]\d{{1,2}}[./-]\d{{2,4}}|\d{{4}}-\d{{2}}-\d{{2}}|\d{{1,2}}(?:st|nd|rd|th|er)?\.?\s+(?:de\s+)?{MONTH}\s+(?:de\s+)?\d{{4}}|{MONTH}\s+\d{{1,2}},?\s+\d{{4}}|{MONTH}\s+(?:de\s+)?\d{{4}}|20\d{{2}}\b)"
    );
    let p = format!(
        r"(?:обновлен[оаы]?|дата\s+обновления|последнее\s+обновление|актуальн\w*\s+на|(?:last\s+)?updated|last\s+modified|last\s+reviewed|valid\s+as\s+of|mis\s+à\s+jour|mise\s+à\s+jour|actualis[ée]e?|actualizado|última\s+actualización|aktualisiert|stand|aggiornat[oa]|ultimo\s+aggiornamento|zaktualizowano|aktualne\s+na|atualizado|última\s+atualização|bijgewerkt|laatst\s+gewijzigd)\s*[:\-–—]?\s*(?:on\s+|le\s+|am\s+|il\s+|em\s+|op\s+|el\s+|dnia\s+|от\s+|с\s+)?{date}"
    );
    re(&R, &format!("(?i){p}"))
}

/// Цены без цифр (I08, I20): «цена договорная», «price on request».
pub const VAGUE_PRICE: &[&str] = &[
    "цена договорная", "договорная цена", "цена по запросу", "стоимость по запросу", "цена по договоренности", "стоимость по договоренности",
    "цену уточняйте", "стоимость уточняйте", "price on request", "call for price", "call for pricing", "price upon request", "prix sur demande",
    "tarif sur demande", "sur devis", "precio a consultar", "consultar precio", "precio bajo consulta", "preis auf anfrage", "preise auf anfrage",
    "prezzo su richiesta", "prezzi su richiesta", "cena do uzgodnienia", "cena na zapytanie", "cena do negocjacji", "prijs op aanvraag",
    "prijzen op aanvraag", "preço sob consulta", "preço a consultar",
];

/// Что входит в цену (I20).
pub const PRICE_INCLUDED: &[&str] = &[
    "входит", "включен", "в стоимость", "в цену", "что входит", "included", "includes", "including", "inclusive", "inclus", "comprend",
    "incluido", "incluye", "incluida", "inklusive", "inkl.", "enthalten", "incluso", "inclusa", "compreso", "comprende", "w cenie",
    "zawiera", "inclusief", "inbegrepen", "incluído", "incluída", "inclui",
];

/// Что оплачивается отдельно (I20).
pub const PRICE_EXTRA: &[&str] = &[
    "не входит", "оплачивается отдельно", "оплачиваются отдельно", "доплат", "дополнительно оплач", "за отдельную плату",
    "not included", "extra charge", "additional cost", "additional fee", "charged separately", "non inclus", "en supplément", "en sus",
    "no incluido", "no incluye", "coste adicional", "nicht enthalten", "nicht inbegriffen", "zuzüglich", "zzgl", "aufpreis", "non incluso",
    "escluso", "supplemento", "nie zawiera", "dodatkowo płatne", "dopłata", "niet inbegrepen", "exclusief", "toeslag", "não incluído",
    "não inclui", "taxa adicional",
];

/// Сроки и условия рядом с ценой (I20).
pub const PRICE_TERMS: &[&str] = &[
    "срок", "длительност", "продолжительност", "гаранти", "duration", "turnaround", "takes about", "warranty", "guarantee", "délai",
    "durée", "garantie", "plazo", "duración", "garantía", "dauer", "lieferzeit", "gewährleistung", "durata", "tempi", "garanzia",
    "czas realizacji", "czas trwania", "gwarancj", "levertijd", "duur", "prazo", "duração", "garantia",
];

/// Категории, где цена — меню или товар: условия к ней не нужны (I20).
pub const MENU_OR_GOODS: &[&str] = &[
    "ресторан", "кафе", "бар", "фастфуд", "пекарн", "кофейн", "пиццер", "столов", "кондитер", "продукт", "одежд", "магазин", "ювелир",
    "обув", "цвет", "супермаркет", "выпечк", "сладост", "мороже", "суши", "напит", "табак", "косметик", "электроник", "подарк", "книг",
    "мебел", "спорттовар", "телефон", "оптик", "аптек", "часы", "сумки", "ткани", "канцтовар", "зоотовар", "товар", "торговый центр",
    "киоск", "всё по одной цене", "стройматериал", "pharmacy", "newsagent", "convenience", "toy store", "antique", "fabric store",
    "tea", "bubble tea", "food truck", "shopping mall", "fashion accessories", "restaurant", "cafe", "café", "bar", "pub", "fast food", "bakery", "coffee", "pizz", "bistro",
    "brasserie", "trattoria", "imbiss", "food", "shop", "store", "boutique", "clothing", "jewel", "supermarket", "grocery", "florist",
];

/// Слово «год» после числа: «с 2005 года», «since 2005 year» (I19 — иначе 2005 похоже на копирайт).
pub const YEAR_WORDS: &[&str] = &["год", "г.", "лет", "year", "an", "année", "año", "jahr", "anno", "rok", "ano", "jaar"];

/// Слова перед номером дома или офиса — это адрес, а не факт (I19).
pub const ADDRESS_BEFORE_NUMBER: &[&str] = &[
    "ул.", "д.", "дом", "кв.", "оф.", "офис", "корп.", "стр.", "пом.", "№", "#", "street", "st.", "ave", "suite", "apt", "no.", "nr.", "nr",
    "n°", "rue", "calle", "c/", "via", "piazza", "straße", "str.", "ul.", "rua", "av.", "avenida", "postcode", "tel.", "тел.", "тел",
];
