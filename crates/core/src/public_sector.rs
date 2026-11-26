//! Государственные и муниципальные организации — им рассылка не пишет: школы, поликлиники, исполкомы,
//! министерства, советы графств и т. п. Сайт им делает не владелец-предприниматель, а госзаказ.
//!
//! Признаки (достаточно одного):
//! - домен сайта: зона `.gov`/`.mil`, `gov.by`, `gov.uk`, `gouv.fr`, `gob.es`, `go.jp`, `edu.by`, `nhs.uk`,
//!   `gosuslugi.ru`, `mos.ru`, местные домены США (`ci.boston.ma.us`, `k12.ne.us`) и т. п.;
//! - почта компании на таком домене;
//! - метка OpenStreetMap `operator:type` (government, public, municipal…);
//! - организационно-правовая форма в названии: «УЗ», «ГУО», «ГБУЗ», «МБОУ», «КУП», «РУП», «КГУ»…;
//! - слова в названии: «исполком», «городская поликлиника», «средняя школа № 5», «city council», «Finanzamt»,
//!   «mairie», «ayuntamiento»… — кроме названий с частной формой («ООО», «ЧУП», «Ltd», «частная»).
//!
//! Правила намеренно узкие: лучше пропустить госучреждение (менеджер увидит его сам), чем не написать частной клинике.

use std::sync::LazyLock;

use regex::Regex;

use crate::text::registrable_domain;

/// Метки второго уровня перед национальной зоной: gov.by, gouv.fr, gob.es, govt.nz, gv.at, gc.ca, go.jp…
const GOV_SLD: &[&str] = &["gov", "mil", "gouv", "gob", "govt", "gv", "gub", "go", "gc", "lg"];

/// Образование в зонах стран СНГ (`school5.minsk.edu.by`) — почти всегда государственное.
const EDU_CIS: &[&str] = &["by", "kz", "uz", "kg", "tj", "ua", "ru", "am", "az", "ge", "md"];

/// Государственные домены и их поддомены.
const GOV_DOMAINS: &[&str] = &[
    "nhs.uk", "nhs.net", "nhs.scot", "hscni.net", "police.uk", "mod.uk", "parliament.uk", "gov.scot", "gov.wales", "llyw.cymru",
    "gosuslugi.ru", "mos.ru", "mskobr.ru", "egov.kz", "europa.eu", "bund.de", "admin.ch", "canada.ca", "overheid.nl",
    "rijksoverheid.nl", "belgium.be", "fgov.be", "service-public.fr", "nic.in",
];

/// Штаты США: местные домены `*.ma.us`, `*.k12.ne.us` принадлежат городам, округам и школьным округам.
const US_STATES: &[&str] = &[
    "al", "ak", "az", "ar", "ca", "co", "ct", "de", "dc", "fl", "ga", "hi", "id", "il", "in", "ia", "ks", "ky", "la", "me", "md",
    "ma", "mi", "mn", "ms", "mo", "mt", "ne", "nv", "nh", "nj", "nm", "ny", "nc", "nd", "oh", "ok", "or", "pa", "ri", "sc", "sd",
    "tn", "tx", "ut", "vt", "va", "wa", "wv", "wi", "wy",
];

/// Государственный домен (сайт или почта).
pub fn gov_domain(host: &str) -> bool {
    let h = host.trim().trim_end_matches('.').to_lowercase();
    let h = h.strip_prefix("www.").unwrap_or(&h);
    let labels: Vec<&str> = h.split('.').filter(|l| !l.is_empty()).collect();
    let n = labels.len();
    if n < 2 {
        return false;
    }
    let tld = labels[n - 1];
    if matches!(tld, "gov" | "mil") {
        return true;
    }
    let cc = tld.len() == 2;
    // gov.by, gouv.fr, go.jp, minsk.edu.by…
    if cc && (GOV_SLD.contains(&labels[n - 2]) || (labels[n - 2] == "edu" && EDU_CIS.contains(&tld))) {
        return true;
    }
    // Госзона глубже — в регистрируемой части: gov.spb.ru (spb.ru — публичная зона). Поддомен частного сайта
    // (edu.iqdata.by, gov.example.com) — не госзона.
    let reg = registrable_domain(h);
    let first = reg.split('.').next().unwrap_or("");
    if cc && (matches!(first, "gov" | "gouv" | "mil") || (first == "edu" && EDU_CIS.contains(&tld))) {
        return true;
    }
    if tld == "us" && n >= 3 && US_STATES.contains(&labels[n - 2]) {
        return true;
    }
    GOV_DOMAINS.iter().any(|d| h == *d || h.ends_with(&format!(".{d}")))
}

/// Формы госучреждений и госпредприятий — отдельным словом, заглавными буквами, как в названиях.
const GOV_FORMS: &[&str] = &[
    // Беларусь
    "УЗ", "ГУ", "ГУО", "ГУК", "ГУСО", "ГУДО", "ГУКДС", "КУП", "РУП", "КУПП", "КУКП", "КПУП", "ГЛХУ", "РНПЦ", "ГОКУ",
    // Россия
    "ГОУ", "ГБУ", "ГБУЗ", "ГАУЗ", "ГКУ", "ГКУЗ", "ГБОУ", "ГАОУ", "ГКОУ", "ГБПОУ", "ГАПОУ", "МБУ", "МАУ", "МКУ", "МБОУ",
    "МАОУ", "МКОУ", "МОУ", "МДОУ", "МБДОУ", "МАДОУ", "МКДОУ", "ГБДОУ", "МБУДО", "МАУДО", "ГБУДО", "МБУК", "МАУК", "ГБУК",
    "ФГБУ", "ФГБОУ", "ФГАОУ", "ФГУП", "ФГКУ", "ФКУ", "ФБУЗ", "ФГБУЗ", "ГУП", "МУП", "КГБУЗ", "ОГБУЗ", "ОБУЗ", "ГОБУЗ", "БУЗ",
    "КГБУ", "ОГБУ", "ОГАУЗ", "ГАУ", "КГАУ", "ГУЗ", "МУЗ", "ЦРБ", "ЦГБ", "ГКБ", "ДГКБ", "ГУВД", "РОВД", "РУВД", "МВД", "МЧС",
    "ФНС", "ИФНС", "ПФР", "СФР", "МФЦ", "ЗАГС", "МНС",
    // Казахстан, Украина
    "КГУ", "ГККП", "КГКП", "РГП", "РГУ", "ГКП", "КГП", "КНП", "КНМП",
    // Испания: государственные школы
    "CEIP", "IES",
    // США
    "DMV",
];

/// Частная форма в названии — словесные признаки госучреждения тогда не применяются.
const PRIVATE_FORMS: &[&str] = &[
    "ООО", "ОДО", "ЗАО", "АО", "ОАО", "ПАО", "ИП", "ЧУП", "ЧТУП", "ЧП", "ЧУ", "АНО", "ТОО", "ТОВ", "ФОП", "LLC", "LTD", "INC",
    "GMBH", "SARL", "SAS", "SRL", "PLC", "LLP",
];

/// Слова и сочетания (в нижнем регистре, «ё» → «е») с начала слова; пробел в конце — слово целиком.
const GOV_WORDS: &[&str] = &[
    // русский, белорусский, украинский
    "государствен", "дзяржаўн", "державн", "муниципальн", "бюджетное учреждение", "казенное учреждение",
    "коммунальное унитарное", "республиканское унитарное", "камунальнае ўнітарнае", "рэспубліканскае ўнітарнае",
    "учреждение здравоохранения", "установа аховы здароўя", "учреждение образования", "установа адукацыі",
    "исполнительный комитет", "исполком", "райисполком", "горисполком", "облисполком", "выканаўчы камітэт", "выканкам",
    "райвыканкам", "гарвыканкам", "администрация города", "администрация района", "администрация муниципальн",
    "администрация городск", "администрация сельск", "администрация поселения", "администрация округа", "сельсовет",
    "сельский совет", "министерство", "міністэрства", "міністерство", "управление образования", "отдел образования",
    "управление здравоохранения", "комитет по здравоохранению", "комитет по образованию", "налоговая инспекция",
    "инспекция мнс", "инспекция фнс", "пенсионный фонд", "социальный фонд", "фонд социальной защиты",
    "центр социального обслуживания", "территориальный центр социального", "военкомат", "военный комиссариат",
    "прокуратура", "районный суд", "городской суд", "областной суд", "мировой суд", "судебный участок", "отдел полиции",
    "отделение полиции", "управление внутренних дел", "пожарная часть", "пожарно-спасательн", "городская поликлиника",
    "городская детская поликлиника", "гарадская паліклініка", "городская больница", "городская клиническая",
    "районная больница", "центральная районная", "областная больница", "областная клиническая",
    "республиканская больница", "республиканский научно-практический", "станция скорой", "диспансер",
    // английский
    "city council", "county council", "borough council", "district council", "town council", "parish council",
    "city hall", "department of ", "ministry of ", "police department", "police station", "sheriff's office",
    "sheriff office", "fire department", "fire station", "public library", "public schools", "school district",
    "board of education", "county clerk", "courthouse", "magistrates court", "magistrates' court", "crown court",
    "county court", "jobcentre", "job centre plus", "hm revenue", "nhs trust", "nhs foundation", "veterans affairs",
    "social security administration",
    // немецкий
    "stadtverwaltung", "gemeindeverwaltung", "kreisverwaltung", "landratsamt", "bürgeramt", "finanzamt", "jobcenter",
    "agentur für arbeit", "polizeidirektion", "polizeirevier", "polizeiinspektion", "polizeipräsidium", "feuerwache",
    "berufsfeuerwehr", "ministerium", "bezirksamt", "amtsgericht", "landgericht", "standesamt", "gesundheitsamt",
    "städtisches klinikum", "städtische kliniken", "staatliche ",
    // французский
    "mairie", "hôtel de ville", "hotel de ville", "préfecture", "sous-préfecture", "ministère", "gendarmerie",
    "commissariat", "centre hospitalier", "conseil départemental", "conseil régional", "caisse primaire d'assurance",
    "france travail", "pôle emploi", "centre des finances publiques", "tribunal ", "école publique", "collège public",
    "lycée public",
    // испанский, итальянский
    "ayuntamiento", "ministerio", "gobierno de ", "diputación", "policía nacional", "policía local", "policía municipal",
    "guardia civil", "centro de salud", "consultorio local", "consejería", "juzgado", "seguridad social",
    "agencia tributaria", "colegio público", "comune di ", "ministero", "questura", "carabinieri", "polizia di stato",
    "polizia locale", "azienda sanitaria", "azienda ospedaliera", "agenzia delle entrate", "prefettura", "tribunale",
    // польский, чешский, нидерландский
    "urząd", "starostwo", "ministerstwo", "komenda", "straż pożarna", "samodzielny publiczny", "sp zoz", "spzoz",
    "szkoła podstawowa nr", "przedszkole nr", "městský úřad", "obecní úřad", "magistrát", "gemeente ", "politie ",
    "ministerie",
];

/// Нумерованные школы, сады, поликлиники и больницы («Средняя школа № 5», «Детский сад №457», «4-я городская
/// поликлиника») — муниципальные и государственные.
static NUMBERED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?:^|[^\p{L}])(?:средняя школа|школа|гимназия|лицей|детский сад|ясли-сад|ясли сад|детсад|поликлиника|больница|родильный дом|роддом|сш|сош|дши|детская школа искусств|центр детского творчества|сярэдняя школа|гімназія|дзіцячы сад|яслі-сад|паліклініка|бальніца)\s*№\s*\d+|(?:^|[^\p{L}\d])\d+\s*-?\s*я\s+(?:\p{L}+\s+){0,3}(?:поликлиника|больница|школа|гимназия|паліклініка|бальніца)",
    )
    .expect("regex")
});

/// Значения OSM `operator:type`, которые означают государство или муниципалитет.
const PUBLIC_OPERATORS: &[&str] = &["government", "public", "state", "municipal", "federal", "regional", "national", "county"];

fn normalize(s: &str) -> String {
    let low = s.to_lowercase().replace('ё', "е");
    let cleaned: String = low.chars().map(|c| if matches!(c, '«' | '»' | '"' | '“' | '”' | '„' | ',' | '(' | ')') { ' ' } else { c }).collect();
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Слова названия в исходном регистре (для форм вроде «УЗ», «ГБУЗ»).
fn tokens(s: &str) -> impl Iterator<Item = &str> {
    s.split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty())
}

/// Почему название — госорганизация; None — не похоже.
fn name_reason(name: &str) -> Option<String> {
    if tokens(name).any(|t| PRIVATE_FORMS.contains(&t.to_uppercase().as_str())) {
        return None;
    }
    let norm = normalize(name);
    if norm.contains("частн") || norm.contains("приватн") || norm.contains("private") {
        return None;
    }
    if let Some(form) = tokens(name).find(|t| GOV_FORMS.contains(t)) {
        return Some(format!("в названии «{form}»"));
    }
    let padded = format!(" {norm} ");
    if let Some(w) = GOV_WORDS.iter().find(|w| padded.contains(&format!(" {w}"))) {
        return Some(format!("в названии «{}»", w.trim_end()));
    }
    NUMBERED.find(&norm).map(|m| format!("в названии «{}»", m.as_str().trim_matches(|c: char| !c.is_alphanumeric())))
}

/// Почему компания — государственная или муниципальная организация (для пропуска в рассылке); None — нет признаков.
/// `host` — сайт компании, `emails` — её адреса, `names` — названия (карточки, для документов),
/// `operator_types` — OSM `operator:type` карточек.
pub fn public_sector(host: Option<&str>, emails: &[&str], names: &[&str], operator_types: &[&str]) -> Option<String> {
    if let Some(h) = host.filter(|h| gov_domain(h)) {
        return Some(format!("государственный сайт ({})", h.trim_start_matches("www.")));
    }
    if let Some(d) = emails.iter().filter_map(|e| e.rsplit_once('@').map(|(_, d)| d.trim())).find(|d| gov_domain(d)) {
        return Some(format!("почта на государственном домене ({d})"));
    }
    if operator_types.iter().any(|t| PUBLIC_OPERATORS.contains(&t.trim().to_lowercase().as_str())) {
        return Some("в OpenStreetMap владелец — государство или муниципалитет".into());
    }
    names.iter().find_map(|n| name_reason(n)).map(|why| format!("госорганизация: {why}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gov_domains() {
        for h in [
            "minzdrav.gov.by", "www.gov.uk", "leeds.gov.uk", "usa.gov", "army.mil", "service-public.fr", "interieur.gouv.fr",
            "sede.gob.es", "city.go.jp", "school5.minsk.edu.by", "gp45.mos.ru", "sh-1.gosuslugi.ru", "lincoln.k12.ne.us",
            "ci.boston.ma.us", "stmarys.nhs.uk", "gov.spb.ru", "dia.govt.nz", "www.bund.de", "canada.ca", "egov.kz",
        ] {
            assert!(gov_domain(h), "{h}");
        }
        for h in ["belodent.by", "govorun.by", "clinic.co.uk", "dental.us", "smile.com", "gov-dental.com", "edu-center.by", "goodent.jp", "spb.ru", "edu.iqdata.by", "gov.example.com", "gov.mysite.by"] {
            assert!(!gov_domain(h), "{h}");
        }
    }

    #[test]
    fn public_sector_names_and_contacts() {
        let ps = |n: &str| public_sector(None, &[], &[n], &[]);
        for n in [
            "УЗ «6-я городская клиническая больница»",
            "ГУО «Средняя школа № 5 г. Минска»",
            "ГБУЗ «Городская поликлиника № 2»",
            "МБОУ СОШ №12",
            "4-я городская детская поликлиника",
            "19-я поликлиника",
            "Детский сад №457",
            "Мінскі гарадскі выканаўчы камітэт",
            "Московский районный суд",
            "Leeds City Council",
            "Finanzamt Berlin-Mitte",
            "Mairie de Lyon",
            "Ayuntamiento de Madrid",
            "Centre Hospitalier de Valence",
            "Областной онкологический диспансер",
            "КУП «Минсккоммунтеплосеть»",
        ] {
            assert!(ps(n).is_some(), "{n}");
        }
        for n in [
            "Белодент",
            "ООО «Поликлиника № 1»",
            "Частная школа № 1",
            "Rathaus-Apotheke",
            "Smile Dental Practice — NHS & Private",
            "Стоматология «Гуру»",
            "Барбершоп Гуд",
            "Центр здоровья «Здравушка»",
            "City Dental Care",
            "Аптека",
            "Школа танцев Pulse",
        ] {
            assert!(ps(n).is_none(), "{n}: {:?}", ps(n));
        }
        assert_eq!(ps("МБОУ СОШ №12").as_deref(), Some("госорганизация: в названии «МБОУ»"));
        assert_eq!(ps("Детский сад №457").as_deref(), Some("госорганизация: в названии «детский сад №457»"));
        assert!(public_sector(Some("www.minzdrav.gov.by"), &[], &["Минздрав"], &[]).unwrap().contains("minzdrav.gov.by"));
        assert!(public_sector(None, &["info@school5.edu.by"], &["Школа"], &[]).unwrap().contains("school5.edu.by"));
        assert!(public_sector(None, &["info@gmail.com"], &["Кафе"], &["government"]).unwrap().contains("OpenStreetMap"));
        assert_eq!(public_sector(Some("belodent.by"), &["info@belodent.by"], &["Белодент"], &["private"]), None);
    }
}
