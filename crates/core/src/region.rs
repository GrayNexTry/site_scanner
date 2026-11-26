//! Страна скана: юрисдикция для правовых проверок, язык запросов, формат адресов,
//! доменные зоны для поиска сайтов.

use serde::{Deserialize, Serialize};

/// Набор правовых норм, по которым проверяется сайт (M01–M06).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Jurisdiction {
    /// Россия: 152-ФЗ, реквизиты, лицензии.
    Ru,
    /// ЕС, ЕЭЗ, Швейцария: GDPR, ePrivacy (баннер cookie), выходные данные.
    Eu,
    /// Великобритания: UK GDPR, PECR, Companies Act.
    Uk,
    /// США: CCPA/CPRA (Калифорния), CalOPPA.
    Us,
    /// Остальные страны: только общие проверки.
    Other,
}

impl Jurisdiction {
    pub fn label(self) -> &'static str {
        match self {
            Jurisdiction::Ru => "Россия",
            Jurisdiction::Eu => "ЕС",
            Jurisdiction::Uk => "Великобритания",
            Jurisdiction::Us => "США",
            Jurisdiction::Other => "другая страна",
        }
    }
    /// Нормы о cookie-баннере и согласиях как в ЕС.
    pub fn gdpr(self) -> bool {
        matches!(self, Jurisdiction::Eu | Jurisdiction::Uk)
    }
}

const EU: &[&str] = &[
    "at", "be", "bg", "hr", "cy", "cz", "dk", "ee", "fi", "fr", "de", "gr", "hu", "ie", "it", "lv", "lt", "lu", "mt",
    "nl", "pl", "pt", "ro", "sk", "si", "es", "se", "is", "li", "no", "ch",
];

pub fn jurisdiction(country: Option<&str>) -> Jurisdiction {
    match country.map(str::to_ascii_lowercase).as_deref() {
        Some("ru") => Jurisdiction::Ru,
        Some("gb" | "uk") => Jurisdiction::Uk,
        Some("us") => Jurisdiction::Us,
        Some(c) if EU.contains(&c) => Jurisdiction::Eu,
        _ => Jurisdiction::Other,
    }
}

/// Страны, где запросы к картам лучше задавать по-русски.
pub fn russian_speaking(country: Option<&str>) -> bool {
    matches!(country.map(str::to_ascii_lowercase).as_deref(), None | Some("ru" | "by" | "kz" | "kg"))
}

/// Номер дома пишется перед улицей: «484 Tompkins Avenue», «12 rue de Rivoli».
pub fn house_number_first(country: Option<&str>) -> bool {
    matches!(
        country.map(str::to_ascii_lowercase).as_deref(),
        Some("us" | "ca" | "gb" | "uk" | "ie" | "au" | "nz" | "fr" | "lu" | "il" | "za" | "sg" | "my" | "ph" | "in")
    )
}

/// Доменные зоны, в которых ищем сайт по названию (самые вероятные — первыми).
pub fn guess_tlds(country: Option<&str>) -> Vec<String> {
    let v: &[&str] = match country.map(str::to_ascii_lowercase).as_deref() {
        None | Some("ru") => &["ru", "com", "рф"],
        Some("by") => &["by", "ru", "com"],
        Some("kz") => &["kz", "ru", "com"],
        Some("ua") => &["ua", "com.ua", "com"],
        Some("us") => &["com", "net", "us"],
        Some("ca") => &["ca", "com"],
        Some("gb" | "uk") => &["co.uk", "com", "uk"],
        Some("au") => &["com.au", "com"],
        Some("nz") => &["co.nz", "com"],
        Some("at") => &["at", "com", "de"],
        Some("tr") => &["com.tr", "com"],
        Some(c) if c.len() == 2 && c.chars().all(|x| x.is_ascii_lowercase()) => return vec![c.to_string(), "com".into()],
        _ => &["com"],
    };
    v.iter().map(|s| s.to_string()).collect()
}

/// Телефонный код страны района: для номеров в карточке без «+» (национальный формат).
pub fn calling_code(country: Option<&str>) -> Option<&'static str> {
    Some(match country?.to_ascii_lowercase().as_str() {
        "by" => "375",
        "ru" | "kz" => "7",
        "ua" => "380",
        "us" | "ca" => "1",
        "gb" | "uk" => "44",
        "ie" => "353",
        "fr" | "mc" => "33",
        "be" => "32",
        "lu" => "352",
        "nl" => "31",
        "de" => "49",
        "at" => "43",
        "ch" | "li" => "41",
        "es" => "34",
        "pt" => "351",
        "it" => "39",
        "pl" => "48",
        "cz" => "420",
        "sk" => "421",
        "hu" => "36",
        "ro" => "40",
        "bg" => "359",
        "gr" => "30",
        "se" => "46",
        "no" => "47",
        "dk" => "45",
        "fi" => "358",
        "ee" => "372",
        "lv" => "371",
        "lt" => "370",
        "hr" => "385",
        "si" => "386",
        "rs" => "381",
        "tr" => "90",
        "il" => "972",
        "ae" => "971",
        "ge" => "995",
        "am" => "374",
        "au" => "61",
        "nz" => "64",
        "jp" => "81",
        "kr" => "82",
        "cn" => "86",
        _ => return None,
    })
}

/// Страны, где сайт на китайском, японском или корейском — норма, а не признак чужого домена.
pub fn cjk_country(country: Option<&str>) -> bool {
    matches!(country.map(str::to_ascii_lowercase).as_deref(), Some("jp" | "cn" | "kr" | "tw" | "hk" | "mo" | "sg"))
}

/// Заголовок Accept-Language для сайтов страны: многоязычные сайты отдают основную версию.
pub fn accept_language(country: Option<&str>) -> String {
    let c = country.map(str::to_ascii_lowercase).unwrap_or_else(|| "ru".into());
    let lang = match c.as_str() {
        "ru" | "by" | "kz" | "kg" => "ru",
        "de" | "at" | "ch" | "li" => "de",
        "fr" | "be" | "lu" | "mc" => "fr",
        "es" | "mx" | "ar" | "co" | "cl" | "pe" => "es",
        "it" | "sm" => "it",
        "nl" => "nl",
        "pl" => "pl",
        "pt" | "br" => "pt",
        "ua" => "uk",
        "cz" => "cs",
        "sk" => "sk",
        "se" => "sv",
        "no" => "nb",
        "dk" => "da",
        "fi" => "fi",
        "tr" => "tr",
        "gr" => "el",
        "ro" => "ro",
        "hu" => "hu",
        "jp" => "ja",
        _ => "en",
    };
    if lang == "en" {
        format!("en-{},en;q=0.9", c.to_uppercase())
    } else {
        format!("{lang}-{},{lang};q=0.9,en;q=0.7", c.to_uppercase())
    }
}

/// Страны, где Яндекс — основной или заметный поисковик: там проверяются его региональные
/// сигналы (R01, R04) и запрашиваются ИКС и знаки (C19, R16).
pub fn yandex_market(country: Option<&str>) -> bool {
    matches!(country.map(str::to_ascii_lowercase).as_deref(), Some("ru" | "by" | "kz" | "uz" | "kg"))
}

/// Доменная зона сайта для R03.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Zone {
    /// Национальная зона страны (код ISO в нижнем регистре).
    Country(String),
    /// Общая зона (.com, .net, .org, .shop…): страну не задаёт.
    Generic,
}

/// Национальные зоны, которые на деле используют как общие (.me, .io, .co, .tv…).
const PSEUDO_GENERIC: &[&str] = &["me", "io", "co", "tv", "ai", "fm", "cc", "ws", "to", "ly", "gg", "su"];

/// Зона домена: национальная (со странами СНГ на кириллице) или общая.
pub fn zone_of(host: &str) -> Zone {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let tld = host.rsplit('.').next().unwrap_or("");
    let country = match tld {
        "ru" | "xn--p1ai" | "xn--80adxhks" | "xn--d1acj3b" | "moscow" | "tatar" => "ru", // .ru, .рф, .москва, .дети
        "by" | "xn--90ais" => "by",                                                     // .by, .бел
        "kz" | "xn--80ao21a" => "kz",                                                   // .kz, .қаз
        "ua" | "xn--j1amh" => "ua",
        "uk" => "gb",
        t if PSEUDO_GENERIC.contains(&t) => return Zone::Generic,
        t if t.len() == 2 && t.chars().all(|c| c.is_ascii_alphabetic()) => t,
        _ => return Zone::Generic,
    };
    Zone::Country(country.to_string())
}

/// Страна по доменной зоне сайта — для ручной проверки без карты.
pub fn country_from_host(host: &str) -> Option<String> {
    let tld = host.trim_end_matches('.').rsplit('.').next()?.to_ascii_lowercase();
    let c = match tld.as_str() {
        "ru" | "su" | "xn--p1ai" => "ru",
        "uk" => "gb",
        "eu" => return None,
        t if t.len() == 2 && t.chars().all(|c| c.is_ascii_alphabetic()) => t,
        _ => return None,
    };
    Some(c.to_string())
}

/// Часовой пояс IANA для страны скана и точки (координаты компании или центра скана) — чтобы письма
/// приходили в рабочее время получателя. Для стран с несколькими поясами (США, Канада, Россия, Австралия,
/// Бразилия, Мексика, Индонезия) пояс выбирается по долготе и широте с точностью до региона: на границах
/// поясов возможна ошибка в час. None — страна неизвестна (тогда время считается по долготе).
pub fn time_zone(country: Option<&str>, lat: f64, lon: f64) -> Option<&'static str> {
    let c = country?.trim().to_ascii_lowercase();
    let single = match c.as_str() {
        "by" => "Europe/Minsk",
        "ua" => "Europe/Kiev",
        "md" => "Europe/Chisinau",
        "lt" => "Europe/Vilnius",
        "lv" => "Europe/Riga",
        "ee" => "Europe/Tallinn",
        "pl" => "Europe/Warsaw",
        "de" => "Europe/Berlin",
        "fr" => "Europe/Paris",
        "es" if lon < -12.0 => "Atlantic/Canary",
        "es" => "Europe/Madrid",
        "pt" if lon < -20.0 => "Atlantic/Azores",
        "pt" => "Europe/Lisbon",
        "it" => "Europe/Rome",
        "nl" => "Europe/Amsterdam",
        "be" => "Europe/Brussels",
        "lu" => "Europe/Luxembourg",
        "ch" => "Europe/Zurich",
        "li" => "Europe/Vaduz",
        "at" => "Europe/Vienna",
        "cz" => "Europe/Prague",
        "sk" => "Europe/Bratislava",
        "hu" => "Europe/Budapest",
        "si" => "Europe/Ljubljana",
        "hr" => "Europe/Zagreb",
        "ba" => "Europe/Sarajevo",
        "rs" => "Europe/Belgrade",
        "me" => "Europe/Podgorica",
        "mk" => "Europe/Skopje",
        "al" => "Europe/Tirane",
        "gr" => "Europe/Athens",
        "bg" => "Europe/Sofia",
        "ro" => "Europe/Bucharest",
        "cy" => "Asia/Nicosia",
        "mt" => "Europe/Malta",
        "ie" => "Europe/Dublin",
        "gb" | "uk" => "Europe/London",
        "is" => "Atlantic/Reykjavik",
        "no" => "Europe/Oslo",
        "se" => "Europe/Stockholm",
        "fi" => "Europe/Helsinki",
        "dk" => "Europe/Copenhagen",
        "tr" => "Europe/Istanbul",
        "ge" => "Asia/Tbilisi",
        "am" => "Asia/Yerevan",
        "az" => "Asia/Baku",
        // С 2024 года весь Казахстан в одном поясе (UTC+5); правила — в базе часовых поясов системы.
        "kz" => "Asia/Almaty",
        "uz" => "Asia/Tashkent",
        "kg" => "Asia/Bishkek",
        "tj" => "Asia/Dushanbe",
        "tm" => "Asia/Ashgabat",
        "il" => "Asia/Jerusalem",
        "ae" => "Asia/Dubai",
        "sa" => "Asia/Riyadh",
        "qa" => "Asia/Qatar",
        "eg" => "Africa/Cairo",
        "ma" => "Africa/Casablanca",
        "za" => "Africa/Johannesburg",
        "ng" => "Africa/Lagos",
        "ke" => "Africa/Nairobi",
        "in" => "Asia/Kolkata",
        "pk" => "Asia/Karachi",
        "bd" => "Asia/Dhaka",
        "lk" => "Asia/Colombo",
        "np" => "Asia/Kathmandu",
        "cn" => "Asia/Shanghai",
        "hk" => "Asia/Hong_Kong",
        "tw" => "Asia/Taipei",
        "jp" => "Asia/Tokyo",
        "kr" => "Asia/Seoul",
        "th" => "Asia/Bangkok",
        "vn" => "Asia/Ho_Chi_Minh",
        "sg" => "Asia/Singapore",
        "my" => "Asia/Kuala_Lumpur",
        "ph" => "Asia/Manila",
        "nz" => "Pacific/Auckland",
        "ar" => "America/Argentina/Buenos_Aires",
        "cl" => "America/Santiago",
        "co" => "America/Bogota",
        "pe" => "America/Lima",
        "uy" => "America/Montevideo",
        "ve" => "America/Caracas",
        "ec" => "America/Guayaquil",
        "us" => return Some(us_zone(lat, lon)),
        "ca" => return Some(ca_zone(lon)),
        "ru" => return Some(ru_zone(lat, lon)),
        "au" => return Some(au_zone(lat, lon)),
        "br" => return Some(if lon < -66.5 { "America/Rio_Branco" } else if lon < -54.0 && lat > -24.0 { "America/Manaus" } else { "America/Sao_Paulo" }),
        "mx" => return Some(mx_zone(lat, lon)),
        "id" => return Some(if lon < 114.6 { "Asia/Jakarta" } else if lon < 124.0 { "Asia/Makassar" } else { "Asia/Jayapura" }),
        _ => return None,
    };
    Some(single)
}

fn us_zone(lat: f64, lon: f64) -> &'static str {
    if lon < -154.0 && lat < 23.0 {
        "Pacific/Honolulu"
    } else if lat > 51.0 && lon < -129.0 {
        "America/Anchorage"
    } else if (31.0..37.0).contains(&lat) && (-114.8..-109.05).contains(&lon) {
        // Аризона: без перехода на летнее время.
        "America/Phoenix"
    } else if lon >= -86.5 {
        "America/New_York"
    } else if lon >= -101.5 {
        "America/Chicago"
    } else if lon >= -114.5 || ((42.0..45.5).contains(&lat) && lon >= -117.0) {
        "America/Denver"
    } else {
        "America/Los_Angeles"
    }
}

fn ca_zone(lon: f64) -> &'static str {
    match lon {
        l if l < -120.0 => "America/Vancouver",
        l if l < -110.0 => "America/Edmonton",
        l if l < -102.0 => "America/Regina",
        l if l < -90.0 => "America/Winnipeg",
        l if l < -64.0 => "America/Toronto",
        l if l < -59.5 => "America/Halifax",
        _ => "America/St_Johns",
    }
}

fn ru_zone(lat: f64, lon: f64) -> &'static str {
    // Самарское время (UTC+4): Самара, Саратов, Ульяновск, Астрахань, Удмуртия; Татарстан и Киров — московское.
    let samara = ((45.5..54.0).contains(&lon) && lat < 54.0) || ((51.0..54.0).contains(&lon) && (56.0..58.5).contains(&lat));
    match lon {
        l if l < 23.0 => "Europe/Kaliningrad",
        _ if samara => "Europe/Samara",
        l if l < 54.0 => "Europe/Moscow",
        // Коми и Архангельская область севернее 59° — московское время.
        l if l < 60.0 && lat > 59.0 => "Europe/Moscow",
        l if l < 73.0 => "Asia/Yekaterinburg",
        l if l < 77.0 => "Asia/Omsk",
        l if l < 88.0 => "Asia/Novosibirsk",
        l if l < 100.0 => "Asia/Krasnoyarsk",
        l if l < 113.0 => "Asia/Irkutsk",
        l if l < 130.0 => "Asia/Yakutsk",
        l if l < 142.0 => "Asia/Vladivostok",
        l if l < 144.5 && lat < 54.5 => "Asia/Sakhalin",
        l if l > 155.0 && (lat > 50.0 || l > 165.0) => "Asia/Kamchatka",
        _ => "Asia/Magadan",
    }
}

fn au_zone(lat: f64, lon: f64) -> &'static str {
    if lon < 129.0 {
        "Australia/Perth"
    } else if lon < 138.0 {
        if lat > -26.0 { "Australia/Darwin" } else { "Australia/Adelaide" }
    } else if lon < 141.0 && lat < -26.0 {
        "Australia/Adelaide"
    } else if lat > -29.0 {
        "Australia/Brisbane"
    } else if lat < -39.5 {
        "Australia/Hobart"
    } else {
        "Australia/Sydney"
    }
}

fn mx_zone(lat: f64, lon: f64) -> &'static str {
    if lon < -114.7 && lat > 28.0 {
        "America/Tijuana"
    } else if lon < -108.5 && lat > 26.0 {
        "America/Hermosillo"
    } else if lon < -104.5 {
        "America/Mazatlan"
    } else if lon > -88.0 && lat < 22.0 {
        "America/Cancun"
    } else {
        "America/Mexico_City"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jurisdictions() {
        assert_eq!(jurisdiction(Some("DE")), Jurisdiction::Eu);
        assert_eq!(jurisdiction(Some("gb")), Jurisdiction::Uk);
        assert_eq!(jurisdiction(Some("us")), Jurisdiction::Us);
        assert_eq!(jurisdiction(Some("ru")), Jurisdiction::Ru);
        assert_eq!(jurisdiction(Some("jp")), Jurisdiction::Other);
        assert_eq!(jurisdiction(None), Jurisdiction::Other);
        assert!(house_number_first(Some("us")) && !house_number_first(Some("de")));
        assert_eq!(country_from_host("praxis.de").as_deref(), Some("de"));
        assert_eq!(country_from_host("xn--80aswg.xn--p1ai").as_deref(), Some("ru"));
        assert_eq!(country_from_host("shop.co.uk").as_deref(), Some("gb"));
        assert_eq!(country_from_host("example.com"), None);
        assert_eq!(guess_tlds(Some("jp")), vec!["jp", "com"]);
        assert_eq!(guess_tlds(Some("de")), vec!["de", "com"]);
        assert_eq!(accept_language(Some("de")), "de-DE,de;q=0.9,en;q=0.7");
        assert_eq!(accept_language(Some("us")), "en-US,en;q=0.9");
        assert_eq!(accept_language(None), "ru-RU,ru;q=0.9,en;q=0.7");
    }

    #[test]
    fn time_zones_by_country_and_point() {
        assert_eq!(time_zone(Some("BY"), 53.9, 27.56), Some("Europe/Minsk"));
        assert_eq!(time_zone(Some("gb"), 51.5, -0.12), Some("Europe/London"));
        // Москва, Казань (московское), Самара (+4), Екатеринбург, Новосибирск, Владивосток, Калининград.
        assert_eq!(time_zone(Some("ru"), 55.75, 37.62), Some("Europe/Moscow"));
        assert_eq!(time_zone(Some("ru"), 55.79, 49.12), Some("Europe/Moscow"));
        assert_eq!(time_zone(Some("ru"), 53.2, 50.15), Some("Europe/Samara"));
        assert_eq!(time_zone(Some("ru"), 56.84, 60.6), Some("Asia/Yekaterinburg"));
        assert_eq!(time_zone(Some("ru"), 55.03, 82.92), Some("Asia/Novosibirsk"));
        assert_eq!(time_zone(Some("ru"), 43.12, 131.9), Some("Asia/Vladivostok"));
        assert_eq!(time_zone(Some("ru"), 54.71, 20.51), Some("Europe/Kaliningrad"));
        // Нью-Йорк, Чикаго, Денвер, Финикс, Лос-Анджелес.
        assert_eq!(time_zone(Some("us"), 40.71, -74.0), Some("America/New_York"));
        assert_eq!(time_zone(Some("us"), 41.88, -87.63), Some("America/Chicago"));
        assert_eq!(time_zone(Some("us"), 39.74, -104.99), Some("America/Denver"));
        assert_eq!(time_zone(Some("us"), 33.45, -112.07), Some("America/Phoenix"));
        assert_eq!(time_zone(Some("us"), 34.05, -118.24), Some("America/Los_Angeles"));
        assert_eq!(time_zone(Some("au"), -33.87, 151.21), Some("Australia/Sydney"));
        assert_eq!(time_zone(Some("au"), -31.95, 115.86), Some("Australia/Perth"));
        assert_eq!(time_zone(Some("xx"), 0.0, 0.0), None);
        assert_eq!(time_zone(None, 53.9, 27.56), None);
    }

    #[test]
    fn zones_and_yandex_markets() {
        assert!(yandex_market(Some("BY")) && yandex_market(Some("ru")) && !yandex_market(Some("de")) && !yandex_market(None));
        assert_eq!(zone_of("zub.ru"), Zone::Country("ru".into()));
        assert_eq!(zone_of("xn--80aswg.xn--p1ai"), Zone::Country("ru".into()));
        assert_eq!(zone_of("kafe.xn--90ais"), Zone::Country("by".into()));
        assert_eq!(zone_of("shop.co.uk"), Zone::Country("gb".into()));
        assert_eq!(zone_of("praxis.de"), Zone::Country("de".into()));
        assert_eq!(zone_of("clinic.com"), Zone::Generic);
        assert_eq!(zone_of("salon.me"), Zone::Generic);
        assert_eq!(zone_of("studio.online"), Zone::Generic);
    }
}
