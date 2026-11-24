//! I17–I21: что помогает ИИ-ассистенту взять страницу в ответ, кроме доступа ботов (I01–I06):
//! видимая дата обновления, прямой ответ сразу после H1, плотность конкретных фактов, условия
//! к ценам и размер HTML. Упоминания в ответах ИИ сканер не считает — только эти условия.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use regex::Regex;

use super::{grade_high, list_short, CheckCtx, SiteView};
use crate::lexicon::{self, any_in};
use crate::model::{MetricResult, Status};
use crate::page::PageInfo;

/// Googlebot читает первые 2 МБ HTML без сжатия.
const HTML_LIMIT: usize = 2 * 1024 * 1024;

pub fn run(ctx: &CheckCtx, v: &SiteView, prices: usize, out: &mut Vec<MetricResult>) {
    let key = key_pages(v);
    out.push(updated_date(v, &key));
    out.push(answer_first(ctx, v, &key));
    out.push(fact_density(v));
    out.push(price_terms(ctx, v, prices));
    out.push(html_size(v));
}

/// Главная и страницы услуг и цен (по адресу), без служебных.
fn key_pages<'a>(v: &'a SiteView<'a>) -> Vec<&'a PageInfo> {
    let mut out = vec![v.page];
    for p in v.inner_pages() {
        let path = url::Url::parse(&p.url).map(|u| u.path().to_lowercase()).unwrap_or_default();
        let service = lexicon::SERVICE_SLUGS.iter().any(|s| path.contains(s));
        let utility = lexicon::UTILITY_SLUGS.iter().any(|s| path.contains(s));
        if service && !utility && !p.has_noindex() {
            out.push(*p);
        }
    }
    out
}

fn short_url(u: &str) -> String {
    url::Url::parse(u).map(|x| if x.path() == "/" { "главная".to_string() } else { x.path().to_string() }).unwrap_or_else(|_| u.to_string())
}

// ---------- I17: видимая дата обновления ----------

fn updated_date(v: &SiteView, key: &[&PageInfo]) -> MetricResult {
    static YEAR: OnceLock<Regex> = OnceLock::new();
    let year_re = YEAR.get_or_init(|| Regex::new(r"(?:19|20)\d{2}|[./-](\d{2})$").unwrap());
    let found = key.iter().find_map(|p| {
        let t = p.text_lower();
        let m = lexicon::updated_re().find(&t)?;
        let s = m.as_str().to_string();
        let year = year_re.captures(&s).and_then(|c| match c.get(1) {
            Some(yy) => yy.as_str().parse::<i32>().ok().map(|y| 2000 + y),
            None => c.get(0)?.as_str().parse().ok(),
        });
        Some((short_url(&p.url), s, year))
    });
    let markup = key.iter().any(|p| p.jsonld.iter().any(|j| j.contains("dateModified")) || p.html_lower.contains("article:modified_time"));
    match found {
        Some((page, s, Some(y))) if y < v.now_year - 1 => {
            MetricResult::warn("I17", format!("Дата обновления давняя: «{}» ({page}) — ИИ предпочитает свежие источники", crate::text::truncate(&s, 60)))
        }
        Some((page, s, _)) => MetricResult::pass("I17", format!("Дата обновления видна: «{}» ({page})", crate::text::truncate(&s, 60))),
        None if markup => MetricResult::warn("I17", "Дата обновления есть только в разметке (dateModified), на странице её не видно"),
        None => MetricResult::fail("I17", format!("Даты обновления не видно ни на главной, ни на страницах услуг ({} стр.)", key.len())),
    }
}

// ---------- I18: прямой ответ сразу после H1 ----------

struct Lead {
    words: usize,
    specific: bool,
    snippet: String,
}

/// Первые 60 слов после H1. Подзаголовок-слоган под H1 — часть первого экрана, поэтому
/// следующие заголовки не обрывают окно.
fn after_h1(p: &PageInfo) -> Option<String> {
    let norm = |s: &str| s.to_lowercase().replace('ё', "е").split_whitespace().collect::<Vec<_>>().join(" ");
    let text = norm(&p.text);
    let h1 = norm(p.h1s().first()?);
    if h1.is_empty() {
        return None;
    }
    let start = text.find(&h1).map(|i| i + h1.len()).or_else(|| {
        let head: String = h1.chars().take(30).collect();
        text.find(&head).map(|i| i + head.len())
    })?;
    Some(text[start..].split_whitespace().take(60).collect::<Vec<_>>().join(" "))
}

/// Слова, которые называют место компании: город скана и части адреса из карточки (район,
/// улица), кроме служебных («улица», «street», «avenue»).
fn place_terms(ctx: &CheckCtx) -> Vec<String> {
    const GENERIC: &[&str] = &[
        "улица", "проспект", "переулок", "бульвар", "шоссе", "площадь", "район", "город", "дом", "корпус", "street", "avenue", "road", "lane",
        "drive", "place", "square", "suite", "floor", "boulevard", "united", "states", "kingdom", "france", "espana", "españa", "spain",
        "deutschland", "germany", "italia", "polska", "беларусь", "россия", "calle", "avenida", "plaza", "piso", "strasse", "straße",
        "platz", "boulevard", "chemin", "allee", "allée", "piazza", "corso",
    ];
    let mut out: Vec<String> = ctx.params.city_variants().iter().flat_map(|c| crate::text::stems(c)).collect();
    if let Some(a) = ctx.business.address.as_deref() {
        let low = a.to_lowercase().replace('ё', "е");
        out.extend(
            low.split(|c: char| !c.is_alphabetic())
                .filter(|w| w.chars().count() >= 5 && !GENERIC.contains(w))
                .map(|w| w.chars().take(w.chars().count().saturating_sub(1).max(5)).collect::<String>()),
        );
    }
    out.retain(|w| w.chars().count() >= 4);
    out.sort();
    out.dedup();
    out
}

fn lead_of(ctx: &CheckCtx, places: &[String], p: &PageInfo) -> Option<Lead> {
    let t = after_h1(p)?;
    let words = t.split_whitespace().filter(|w| w.chars().any(char::is_alphabetic)).count();
    let service = crate::providers::service_terms(ctx.params);
    let specific = count_facts(&t) > 0
        || lexicon::price_re().is_match(&t)
        || places.iter().any(|c| t.contains(c.as_str()))
        || service.iter().any(|s| s.chars().count() >= 4 && t.contains(s.as_str()));
    Some(Lead { words, specific, snippet: crate::text::truncate(&t, 110) })
}

fn answer_first(ctx: &CheckCtx, v: &SiteView, key: &[&PageInfo]) -> MetricResult {
    let places = place_terms(ctx);
    let Some(home) = lead_of(ctx, &places, v.page) else {
        return MetricResult::na("I18", "На главной нет H1 (см. F03) — от чего отсчитывать первый экран, неясно");
    };
    let inner: Vec<Lead> = key.iter().skip(1).filter_map(|p| lead_of(ctx, &places, p)).collect();
    let good = |l: &Lead| l.words >= 25 && l.specific;
    let inner_good = inner.iter().filter(|l| good(l)).count();
    let inner_note = if inner.is_empty() { String::new() } else { format!("; страницы услуг: конкретика в начале у {inner_good} из {}", inner.len()) };
    if good(&home) {
        MetricResult::pass("I18", format!("Начало после H1: «{}»{inner_note}", home.snippet))
    } else if home.words >= 25 {
        MetricResult::warn("I18", format!("В начале страницы ни места (город, район, улица), ни цифр (цена, срок, опыт): «{}»{inner_note}", home.snippet))
    } else if inner_good * 2 >= inner.len().max(1) && !inner.is_empty() {
        MetricResult::warn("I18", format!("На главной после H1 почти нет текста ({} слов), на страницах услуг есть{inner_note}", home.words))
    } else {
        MetricResult::fail("I18", format!("После H1 всего {} слов текста — ИИ нечего процитировать с первого экрана{inner_note}", home.words))
    }
    .with_value(home.words)
}

// ---------- I19: плотность фактов ----------

/// Свой текст каждой страницы: без пятисловных фрагментов, которые повторяются на половине
/// страниц и больше (меню, подвал, баннеры).
pub(crate) fn own_texts(texts: &[String]) -> Vec<String> {
    let tokens: Vec<Vec<&str>> = texts.iter().map(|t| t.split_whitespace().collect()).collect();
    let sets: Vec<HashSet<String>> = tokens.iter().map(|t| t.windows(5).map(|w| w.join(" ")).collect()).collect();
    let mut freq: HashMap<&str, usize> = HashMap::new();
    for s in &sets {
        for sh in s {
            *freq.entry(sh.as_str()).or_default() += 1;
        }
    }
    let common = ((texts.len() as f64 * 0.5).ceil() as usize).max(2);
    tokens
        .iter()
        .map(|t| {
            if t.len() < 5 {
                return t.join(" ");
            }
            let boiler: Vec<bool> = t.windows(5).map(|w| freq.get(w.join(" ").as_str()).copied().unwrap_or(0) >= common).collect();
            (0..t.len())
                .filter(|&i| !(i.saturating_sub(4)..=i.min(boiler.len() - 1)).any(|j| boiler[j]))
                .map(|i| t[i])
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
}

/// Конкретные факты с цифрами: «3 года гарантии», «от 1 500 ₽», «за 40 минут», «98 %».
/// Не считаются: телефоны, даты, время, индексы, номера домов и годы без слова «год».
pub(crate) fn count_facts(text: &str) -> usize {
    static NUM: OnceLock<Regex> = OnceLock::new();
    // Число с пробелами между тысячами («1 500») — одно число; пробел перед следующим словом
    // берётся, только если дальше не цифра (иначе следующее число потеряет свою границу).
    let num = NUM.get_or_init(|| {
        Regex::new(r"(?:^|[\s(«„“])([€£$]\s?)?(\d{1,3}(?:[ \u{a0}]\d{3})+|\d[\d.,:/\-]*)(?:[\s\u{a0}]?([^\s\d][^\s]{0,19}))?").unwrap()
    });
    let mut n = 0;
    for c in num.captures_iter(text) {
        let prefix = c.get(1).map_or("", |m| m.as_str());
        let digits_raw = c.get(2).map_or("", |m| m.as_str()).trim_end_matches(['.', ',', '-', ':', '/']);
        let next = c.get(3).map_or("", |m| m.as_str()).to_lowercase();
        if digits_raw.contains(':') || digits_raw.contains('/') {
            continue; // время, дробь, дата
        }
        if digits_raw.matches('.').count() > 1 || digits_raw.matches('-').count() > 1 {
            continue; // дата 12.03.2026, телефон 123-45-67
        }
        let digits: String = digits_raw.chars().take_while(|ch| ch.is_ascii_digit()).collect();
        let money = !prefix.is_empty() || next.starts_with(['%', '€', '£', '$', '₽', '₸', '₴', '°']) || lexicon::price_re().is_match(&format!("1 {next}"));
        if money {
            n += 1;
            continue;
        }
        if next.is_empty() || !next.chars().next().is_some_and(char::is_alphabetic) {
            continue;
        }
        if digits.len() >= 5 {
            continue; // индекс, часть телефона, ИНН
        }
        if digits.len() == 4 && (digits.starts_with("19") || digits.starts_with("20")) && !lexicon::YEAR_WORDS.iter().any(|w| next.starts_with(w)) {
            continue; // «© 2024», «since 2005» без слова «год»
        }
        let at = c.get(2).map_or(0, |m| m.start());
        let before = text[..at].split_whitespace().last().unwrap_or("").to_lowercase();
        if lexicon::ADDRESS_BEFORE_NUMBER.iter().any(|w| before == *w || before.ends_with(w)) {
            continue;
        }
        // Номер дома перед улицей: «120 Main Street», «12 Rue de Rivoli» — адрес, а не факт.
        let tail: String = text[at..].chars().take(60).collect();
        if lexicon::address_re().find(&tail).is_some_and(|m| m.start() == 0) {
            continue;
        }
        n += 1;
    }
    n
}

fn fact_density(v: &SiteView) -> MetricResult {
    let pages: Vec<&&PageInfo> = v
        .pages
        .iter()
        .filter(|p| {
            let path = url::Url::parse(&p.url).map(|u| u.path().to_lowercase()).unwrap_or_default();
            path == "/" || !lexicon::UTILITY_SLUGS.iter().any(|s| path.contains(s))
        })
        .collect();
    let texts: Vec<String> = pages.iter().map(|p| p.text_lower()).collect();
    let own = own_texts(&texts);
    let words: usize = own.iter().map(|t| t.split_whitespace().filter(|w| w.chars().any(char::is_alphabetic)).count()).sum();
    if words < 150 {
        return MetricResult::na("I19", format!("Своего текста (без меню и подвала) всего {words} слов — плотность не посчитать, см. R15"));
    }
    let facts: usize = own.iter().map(|t| count_facts(t)).sum();
    let per = (facts as f64 * 1000.0 / words as f64 * 10.0).round() / 10.0;
    grade_high(
        "I19",
        per,
        10.0,
        4.0,
        format!("{facts} фактов с цифрами на {words} слов своего текста ({} стр.) — {} на 1000 слов, норма от 10", pages.len(), super::fmt_num(per)),
    )
}

// ---------- I20: цена с условиями ----------

fn price_terms(ctx: &CheckCtx, v: &SiteView, prices: usize) -> MetricResult {
    let text = &v.text_all;
    let vague = any_in(text, lexicon::VAGUE_PRICE);
    if prices == 0 {
        return if let Some(w) = vague.first() {
            MetricResult::fail("I20", format!("Вместо цен — «{w}»: на вопрос «сколько стоит» ИИ ответит ценой конкурента"))
        } else {
            MetricResult::na("I20", "Цен на сайте нет — см. I08")
        };
    }
    let category = ctx.business.category.as_deref().unwrap_or("").to_lowercase();
    if menu_or_goods(&category) || !any_in(text, lexicon::SHOP).is_empty() {
        return MetricResult::na("I20", "Цены — меню или товары: состав и сроки к ним не нужны");
    }
    let included = any_in(text, lexicon::PRICE_INCLUDED);
    let extra = any_in(text, lexicon::PRICE_EXTRA);
    let terms = any_in(text, lexicon::PRICE_TERMS);
    let have: Vec<&str> = [
        (!included.is_empty()).then_some("что входит"),
        (!extra.is_empty()).then_some("что отдельно"),
        (!terms.is_empty()).then_some("сроки/гарантия"),
    ]
    .into_iter()
    .flatten()
    .collect();
    let vague_note = vague.first().map(|w| format!("; рядом встречается «{w}»")).unwrap_or_default();
    let st = match (have.len(), vague.is_empty()) {
        (3, true) => Status::Pass,
        (2, true) if !included.is_empty() => Status::Pass,
        _ => Status::Warn,
    };
    let detail = if have.is_empty() {
        format!("Цены есть ({prices}), но не сказано, что в них входит, сроки и что оплачивается отдельно{vague_note}")
    } else {
        format!("К ценам указано: {}{vague_note}", have.join(", "))
    };
    MetricResult::new("I20", st, detail).with_value(have.len())
}

/// Категория карточки — еда или товары. Короткие слова («бар», «pub») совпадают только
/// целиком, чтобы «барбершоп» не стал баром; длинные — по началу слова («цветы», «товары»).
fn menu_or_goods(category: &str) -> bool {
    let words: Vec<&str> = category.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    lexicon::MENU_OR_GOODS.iter().any(|e| {
        if e.contains(' ') {
            category.contains(e)
        } else {
            words.iter().any(|w| *w == *e || (e.chars().count() >= 4 && w.starts_with(e)))
        }
    })
}

// ---------- I21: размер HTML ----------

fn html_size(v: &SiteView) -> MetricResult {
    let mut sizes: Vec<(String, usize, bool)> = vec![(v.home.final_url.clone(), v.home.body_bytes, v.home.truncated && v.home.body_bytes >= HTML_LIMIT)];
    sizes.extend(v.snap.pages.iter().filter(|p| p.bytes > 0).map(|p| (p.final_url.clone(), p.bytes, false)));
    let heavy: Vec<String> = sizes
        .iter()
        .filter(|(_, b, cut)| *b > HTML_LIMIT || *cut)
        .map(|(u, b, cut)| format!("{} — {}{} МБ", short_url(u), if *cut { "больше " } else { "" }, super::fmt_num((*b as f64 / 1048576.0 * 10.0).round() / 10.0)))
        .collect();
    let max = sizes.iter().map(|(_, b, _)| *b).max().unwrap_or(0);
    if heavy.is_empty() {
        MetricResult::pass("I21", format!("Самая тяжёлая страница — {} КБ HTML из {} проверенных", max / 1024, sizes.len()))
    } else {
        MetricResult::fail("I21", format!("HTML больше 2 МБ — Google прочтёт только начало: {}", list_short(&heavy, 3)))
    }
    .with_value((max as f64 / 1048576.0 * 100.0).round() / 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facts_skip_phones_dates_and_addresses() {
        assert_eq!(count_facts("гарантия 3 года, приём 40 минут, от 1 500 ₽, скидка 10 %"), 4);
        assert_eq!(count_facts("тел. +7 (495) 123-45-67, ул. ленина, д. 5, 220030 минск, © 2024 все права, 12.03.2026 9:00-21:00"), 0);
        assert_eq!(count_facts("с 2005 года на рынке, 12 врачей"), 2);
        assert_eq!(count_facts("3 years warranty, from €45, 98% satisfied, 120 Main Street"), 3);
        assert_eq!(count_facts("garantie 2 ans, 30 minutes, à partir de 50 €"), 3);
        assert_eq!(count_facts("seit 2010 Jahr, 5 Jahre Garantie, Preis 40 Euro"), 3);
    }

    #[test]
    fn updated_date_in_languages() {
        for t in [
            "цены обновлены: 01.11.2026",
            "обновлено 12 марта 2026",
            "last updated may 4, 2026",
            "stand: märz 2026",
            "mis à jour le 3 novembre 2026",
            "actualizado el 2026-11-02",
            "aggiornato il 5 ottobre 2026",
            "zaktualizowano 05.10.2026",
            "atualizado em 2026",
            "laatst gewijzigd 1 oktober 2026",
            "цены актуальны на 01.11.2026",
        ] {
            assert!(lexicon::updated_re().is_match(t), "{t}");
        }
        for t in ["обновленный интерьер и новые кресла", "we stand out from the rest", "updated equipment"] {
            assert!(!lexicon::updated_re().is_match(t), "{t}");
        }
    }

    #[test]
    fn menu_categories_by_words() {
        for c in ["бар", "ресторан", "выпечка, сладости", "цветы, подарки", "товары для дома", "wine bar", "tea room", "coffee", "bubble tea shop"] {
            assert!(menu_or_goods(c), "{c}");
        }
        for c in ["барбершоп", "парикмахерская, барбершоп", "салон красоты", "стоматология", "услуги", "theatre venue", "psychologist", "юрист"] {
            assert!(!menu_or_goods(c), "{c}");
        }
    }

    #[test]
    fn own_text_drops_menu_repeated_on_pages() {
        let menu = "главная услуги цены о нас контакты";
        let texts = vec![
            format!("{menu} лечение кариеса от 2 000 ₽ за 40 минут"),
            format!("{menu} отбеливание зубов 12 000 ₽ гарантия 1 год"),
            format!("{menu} имплантация под ключ 45 000 ₽"),
        ];
        let own = own_texts(&texts);
        assert!(own.iter().all(|t| !t.contains("контакты")), "{own:?}");
        assert!(own[0].contains("лечение кариеса"));
    }
}
