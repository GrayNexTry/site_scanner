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
