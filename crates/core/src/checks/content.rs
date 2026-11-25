//! F. Содержимое, G. Разметка schema.org, H. Локальное SEO, I. Готовность к ИИ-поиску.

use std::collections::HashSet;
use std::sync::OnceLock;

use regex::Regex;
use serde_json::Value;

use super::{list_short, CheckCtx, SiteView};
use crate::model::{MetricResult, Status};
use crate::page::PageInfo;
use crate::robots;
use crate::text::{phone_key, phones_in_text, stems};

fn re(cell: &'static OnceLock<Regex>, p: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(p).unwrap())
}

pub use crate::lexicon::{address_re, price_re};

pub fn run(ctx: &CheckCtx, v: &SiteView, out: &mut Vec<MetricResult>) {
    let page = v.page;
    let text = &v.text_all;

    // ---------- F ----------
    // Рендер запускали и он не удался (проверка защиты, таймаут) — то, что ставит JS, неизвестно.
    let render_failed = v.snap.render_error.as_deref().is_some_and(|e| !e.contains("не прибавилось"));
    out.push(match (&page.title, &v.snap.js_title) {
        // Google рендерит страницу и title увидит, Яндекс и ИИ-боты читают исходный HTML.
        (None, Some(t)) => MetricResult::warn("F01", format!("title ставит JS: «{}» — Google увидит, Яндекс и ИИ-боты нет", crate::text::truncate(t, 70))),
        (None, None) if render_failed => MetricResult::na("F01", "title нет в исходном HTML, а рендер не удался — проверить вручную"),
        (None, None) => MetricResult::fail("F01", "Нет title"),
        (Some(t), _) => {
            let n = t.chars().count();
            MetricResult::new("F01", if (30..=65).contains(&n) { Status::Pass } else { Status::Warn }, format!("{n} симв.: {t}")).with_value(n)
        }
    });
    out.push(match &page.description {
        None if render_failed => MetricResult::na("F02", "description нет в исходном HTML, а рендер не удался — проверить вручную"),
        None => MetricResult::fail("F02", "Нет meta description"),
        Some(d) => {
            let n = d.chars().count();
            MetricResult::new("F02", if (70..=160).contains(&n) { Status::Pass } else { Status::Warn }, format!("{n} симв.: {}", crate::text::truncate(d, 120))).with_value(n)
        }
    });
    let h1 = page.h1s();
    out.push(match h1.len() {
        0 if v.snap.js_h1.is_some() => MetricResult::warn(
            "F03",
            format!("H1 ставит JS: «{}» — Google увидит, Яндекс и ИИ-боты нет", crate::text::truncate(v.snap.js_h1.as_deref().unwrap_or(""), 70)),
        ),
        0 if render_failed => MetricResult::na("F03", "H1 нет в исходном HTML, а рендер не удался — проверить вручную"),
        0 => MetricResult::fail("F03", "H1 нет"),
        1 => MetricResult::pass("F03", h1[0].to_string()),
        n => MetricResult::warn("F03", format!("H1: {n} шт. — {}", h1.iter().take(3).copied().collect::<Vec<_>>().join(" | "))),
    }
    .with_value(h1.len()));
    let mut prev = 0u8;
    let mut jump = None;
    for (l, t) in &page.headings {
        if prev > 0 && *l > prev + 1 && jump.is_none() {
            jump = Some(format!("после H{prev} сразу H{l} «{}»", crate::text::truncate(t, 40)));
        }
        prev = *l;
    }
    out.push(match (page.headings.is_empty(), jump) {
        (true, _) => MetricResult::warn("F04", "Заголовков нет"),
        (false, Some(j)) => MetricResult::warn("F04", format!("Пропуск уровня: {j}")),
        (false, None) => MetricResult::pass("F04", format!("{} заголовков без пропусков уровней", page.headings.len())),
    });
    out.push(MetricResult::info("F05", format!("{} слов на главной", page.words)).with_value(page.words));

    let service = crate::providers::service_terms(ctx.params);
    let city: Vec<String> = ctx.params.city_variants().iter().flat_map(|c| stems(c)).collect();
    let head = format!("{} {}", page.title.clone().unwrap_or_default(), h1.join(" ")).to_lowercase().replace('ё', "е");
    let in_head = |st: &[String]| !st.is_empty() && st.iter().any(|s| head.contains(s.as_str()));
    let in_text = |st: &[String]| !st.is_empty() && st.iter().any(|s| text.contains(s.as_str()));
    if service.is_empty() {
        out.push(MetricResult::na("F06", "Запрос скана не задан"));
    } else {
        let city_ok = city.is_empty() || in_head(&city) || in_text(&city);
        out.push(if in_head(&service) && city_ok {
            MetricResult::pass("F06", "Услуга есть в title/H1, город указан")
        } else if in_head(&service) {
            MetricResult::warn("F06", "Услуга в title/H1 есть, города нет на сайте")
        } else if in_text(&service) {
            MetricResult::warn("F06", "Услуга упомянута только в тексте, не в title/H1")
        } else {
            MetricResult::fail("F06", "Услуги из запроса нет ни в title/H1, ни в тексте")
        });
    }
    use crate::lexicon::SERVICE_SLUGS;
    let internal: HashSet<String> = v
        .pages
        .iter()
        .flat_map(|p| p.links.iter().filter(|l| l.internal).map(|l| l.href.to_lowercase()))
        .filter_map(|h| {
            // Путь без запроса и якоря; главная не считается.
            let u = url::Url::parse(&h).ok()?;
            let mut path = u.path().trim_end_matches('/').to_string();
            // WordPress без ЧПУ: /?page_id=289, /?p=2813 — это отдельные страницы.
            if let Some(q) = u.query().filter(|q| ["page_id=", "p=", "id=", "cat="].iter().any(|k| q.starts_with(k)))
                && (path.is_empty() || path.ends_with(".php"))
            {
                path = format!("{path}/?{q}");
            }
            (!path.is_empty()).then(|| format!("{}{path}", u.host_str().unwrap_or("")))
        })
        .collect();
    let service_urls: HashSet<&String> = internal.iter().filter(|h| SERVICE_SLUGS.iter().any(|s| h.contains(s))).collect();
    // Разделы и товары без «услуг» в адресе (магазин: /iphone-13/music/…, салон: /manikyur/):
    // всё, что не служебная страница и не файл.
    let content = internal
        .iter()
        .filter(|h| {
            let path = h.split_once('/').map(|(_, p)| p).unwrap_or("");
            !crate::lexicon::UTILITY_SLUGS.iter().any(|s| path.contains(s))
                && ![".pdf", ".jpg", ".jpeg", ".png", ".webp", ".doc", ".docx", ".xls", ".xlsx", ".zip", ".xml"].iter().any(|e| path.ends_with(e))
        })
        .count();
    let n = service_urls.len().max(content);
    // Магазин на Tilda Store: карточки товаров рисует скрипт.
    let tilda_store = v.html_all.contains("t-store") || v.html_all.contains("js-store-grid");
    out.push(if n == 0 && tilda_store {
        MetricResult::na("F07", "Каталог Tilda Store рисуется скриптом — страницы товаров без рендера не видны, проверить вручную")
    } else {
        MetricResult::new(
            "F07",
            if service_urls.len() >= 3 || content >= 5 {
                Status::Pass
            } else if n > 0 {
                Status::Warn
            } else {
                Status::Fail
            },
            if service_urls.len() >= content {
                format!("Страниц услуг и цен: {}", service_urls.len())
            } else {
                format!("Отдельных страниц услуг, разделов или товаров: {content}")
            },
        )
        .with_value(n)
    });
    let imgs: Vec<_> = page.images.iter().filter(|i| !i.tiny && i.src.as_deref().is_some_and(|s| !s.starts_with("data:"))).collect();
    if imgs.is_empty() {
        out.push(MetricResult::na("F10", "Картинок нет"));
    } else {
        let with_alt = imgs.iter().filter(|i| i.alt.as_deref().is_some_and(|a| !a.is_empty())).count();
        let share = with_alt as f64 / imgs.len() as f64;
        out.push(super::grade_high("F10", share, 0.9, 0.5, format!("alt у {with_alt} из {} картинок", imgs.len())));
    }
    out.push(match (page.og("og:title").is_some(), page.og("og:image").is_some()) {
        (true, true) => MetricResult::pass("F11", "og:title и og:image есть"),
        (false, false) => MetricResult::fail("F11", "Open Graph нет — ссылка в мессенджере будет без превью"),
        _ => MetricResult::warn("F11", "Open Graph заполнен частично"),
    });
    out.push(if page.icon_link || v.snap.favicon_status == Some(200) {
        MetricResult::pass("F12", "Фавиконка есть")
    } else {
        MetricResult::fail("F12", "Фавиконки нет")
    });
    static COPY: OnceLock<Regex> = OnceLock::new();
    let copy_re = re(&COPY, r"(?i)(?:©|\(c\)|&copy;|копирайт|copyright)\s*(?:\d{4}\s*[-–—]\s*)?(\d{4})");
    let years: Vec<i32> = copy_re.captures_iter(&page.text).filter_map(|c| c[1].parse().ok()).filter(|y| (1995..=v.now_year + 1).contains(y)).collect();
    // Свежий год в заголовке или тексте («2026-2027», новости от 22.11.2026) — сайт живой,
    // даже если в подвале забыли обновить ©.
    static YEAR: OnceLock<Regex> = OnceLock::new();
    static OPEN_RANGE: OnceLock<Regex> = OnceLock::new();
    let fresh = re(&YEAR, r"\b(20\d\d)\b")
        .captures_iter(&format!("{} {}", page.title.as_deref().unwrap_or(""), page.text))
        .filter_map(|c| c[1].parse::<i32>().ok())
        .any(|y| y >= v.now_year - 1 && y <= v.now_year + 1)
        // «© 2017-» и пусто: текущий год подставляет скрипт.
        || re(&OPEN_RANGE, r"(?:©|\(c\)|copyright)\s*\d{4}\s*[-–—]\s*(?:$|[^\d\s])").is_match(&page.text.to_lowercase())
        // Свежие даты обновления страниц в sitemap.
        || v.snap.sitemap.lastmods.iter().any(|l| l.get(..4).and_then(|y| y.parse::<i32>().ok()).is_some_and(|y| y >= v.now_year - 1));
    match years.iter().max() {
        None => out.push(MetricResult::na("F13", "Года в копирайте нет")),
        Some(&y) if y < v.now_year - 1 && fresh => {
            out.push(MetricResult::pass("F13", format!("© {y} в подвале не обновлён, но на странице есть свежие даты")).with_value(y))
        }
        Some(&y) => out.push(
            MetricResult::new(
                "F13",
                if y >= v.now_year - 1 { Status::Pass } else if y == v.now_year - 2 { Status::Warn } else { Status::Fail },
                if y >= v.now_year - 1 { format!("© {y}") } else { format!("© {y} — сайт давно не обновлялся") },
            )
            .with_value(y),
        ),
    }
    // Заглушка: шаблонный текст, который посетитель видит. В скрытых окнах (всплывающие, модальные)
    // шаблон часто стоит до подмены скриптом — это не заглушка. Сайт «по сути нет», только если
    // кроме шаблона на главной почти ничего нет; иначе — недоделка на рабочем сайте.
    let visible_hit = |text: &str, hidden: &str, d: &str| text.matches(d).count() > hidden.matches(d).count();
    let home_text = page.text_lower();
    let home_hidden = page.hidden_text.to_lowercase().replace('ё', "е");
    let mut dummy: Vec<&str> = crate::lexicon::DUMMY.iter().copied().filter(|d| visible_hit(&home_text, &home_hidden, d)).collect();
    if page.words < 200 {
        let extra: Vec<&str> =
            crate::lexicon::PLACEHOLDER.iter().copied().filter(|d| visible_hit(&home_text, &home_hidden, d) && !dummy.contains(d)).collect();
        dummy.extend(extra);
    }
    let hidden_only = crate::lexicon::DUMMY.iter().any(|d| home_text.contains(d)) && dummy.is_empty();
    let inner_dummy = v.snap.pages.iter().find_map(|cp| {
        let info = cp.info.as_ref()?;
        let (t, h) = (info.text_lower(), info.hidden_text.to_lowercase());
        crate::lexicon::DUMMY.iter().find(|d| visible_hit(&t, &h, d)).map(|d| (cp.final_url.clone(), *d))
    });
    // Слов настоящего текста на главной — без шаблонных абзацев (в среднем ~60 слов на вхождение).
    let real_words = page.words.saturating_sub(dummy.iter().map(|d| home_text.matches(d).count() * 60).sum::<usize>());
    out.push(match (dummy.is_empty(), inner_dummy) {
        (false, _) if real_words < 300 => MetricResult::fail("F14", format!("На главной шаблонный текст: «{}»", dummy.join("», «"))).with_value("home"),
        (false, _) => MetricResult::warn("F14", format!("На рабочей главной остался шаблонный текст: «{}»", dummy.join("», «"))).with_value("leftover"),
        (true, Some((url, d))) => MetricResult::fail("F14", format!("Текст-заглушка «{d}» на странице {url}")).with_value("inner"),
        (true, None) if hidden_only => MetricResult::pass("F14", "Шаблонный текст только в скрытом окне — посетитель его не видит"),
        (true, None) => MetricResult::pass("F14", "Заглушек и шаблонного текста нет"),
    });
    let inner: Vec<_> = v.inner_pages().collect();
    if inner.is_empty() {
        out.push(MetricResult::na("F17", "Внутренние страницы не обойдены"));
    } else {
        let with = inner.iter().filter(|p| p.breadcrumbs || p.jsonld.iter().any(|j| j.contains("BreadcrumbList"))).count();
        out.push(MetricResult::new("F17", if with > 0 { Status::Pass } else { Status::Warn }, format!("Хлебные крошки на {with} из {} страниц", inner.len())));
    }
    out.push(match &page.lang {
        Some(l) => MetricResult::pass("F18", format!("lang=\"{l}\"")),
        None => MetricResult::fail("F18", "Нет атрибута lang"),
    });
    let blog = page.links.iter().any(|l| {
        let h = l.href.to_lowercase();
        crate::lexicon::BLOG.iter().any(|s| h.contains(s))
    });
    out.push(MetricResult::info("F19", if blog { "Есть раздел статей или новостей" } else { "Раздела статей нет" }).with_value(blog));

    // ---------- G ----------
    schema_checks(ctx, v, out);

    // ---------- H ----------
    let site_phones: HashSet<String> = phones_in_text(text)
        .into_iter()
        .chain(v.pages.iter().flat_map(|p| p.tel_links.iter()).filter_map(|t| phone_key(t)))
        .collect();
    let has_addr = address_re().is_match(text);
    out.push(match (!site_phones.is_empty(), has_addr) {
        (true, true) => MetricResult::pass("H01", "Телефон и адрес указаны текстом"),
        (true, false) => MetricResult::warn("H01", "Телефон есть, адреса текстом нет"),
        (false, true) => MetricResult::warn("H01", "Адрес есть, телефона текстом нет"),
        (false, false) => MetricResult::fail("H01", "Ни телефона, ни адреса текстом"),
    });
    let b = ctx.business;
    let card_phone = b.phone.as_deref().and_then(phone_key);
    let domain_label = crate::text::registrable_domain(v.final_url.host_str().unwrap_or("")).split('.').next().unwrap_or("").to_string();
    let phone_match = card_phone.as_ref().map(|k| site_phones.contains(k));
    let addr_match = b.address.as_deref().map(|a| address_matches(a, text));
    // На сайте телефоны только другой страны (+7 у минской точки, +44 у парижской) — сайт сети из
    // другой страны (chopchop.me). Номера без кода страны считаются местными.
    let other_country = card_calling_code(b.phone.as_deref().unwrap_or(""), ctx.params.country.as_deref()).and_then(|card| {
        let (national, intl) = site_phone_countries(text, &v.html_all);
        if national + intl.get(&card).copied().unwrap_or(0) > 0 {
            return None;
        }
        intl.into_iter().filter(|(c, n)| *c != card && *n >= 2).max_by_key(|(_, n)| *n).map(|(c, _)| (card, c))
    });
    out.push(match (phone_match, addr_match) {
        (p, a) if p != Some(true) && a != Some(true) && other_country.is_some() => {
            let (card, site) = other_country.unwrap_or_default();
            MetricResult::fail("H02", format!("На сайте телефоны другой страны (+{site}), а в карточке +{card} — похоже, это сайт сети из другой страны"))
                .with_value("other_country")
        }
        // Домен занят казино/ставками или текстом на чужом языке — чужой, даже если в карточке нет контактов.
        (p, a) if p != Some(true) && a != Some(true) && (gambling_hijack(&b.name, b.category.as_deref(), text, page.title.as_deref()) || (mostly_cjk(text) && !asian_locale(&v.final_url) && !crate::region::cjk_country(ctx.params.country.as_deref()))) => {
            MetricResult::fail("H02", "На сайте чужое содержимое (казино, ставки или текст на другом языке) — домен, похоже, перехвачен или продан").with_value("foreign")
        }
        // Сверить не с чем, а названия компании на сайте не видно (luxtour.by у «Пончыкі» — туры в
        // Египет): не «чужой» наверняка, но отчёт с «сайт в порядке» отправлять нельзя — проверить.
        (None, None) if !name_on_site(&b.name, text, page.title.as_deref(), &domain_label) => MetricResult::warn(
            "H02",
            "В карточке нет телефона и адреса, а названия компании на сайте не видно — проверить, её ли это сайт",
        )
        .with_value("unverified"),
        (None, None) => MetricResult::na("H02", "В карточке нет телефона и адреса"),
        (Some(true), Some(true)) | (Some(true), None) | (None, Some(true)) => MetricResult::pass("H02", "Контакты сайта совпадают с карточкой"),
        (Some(false), Some(false)) if card_phone.as_ref().is_some_and(|k| crate::discover::raw_phone_keys(&v.home.body).contains(k)) => {
            MetricResult::warn("H02", "Телефон из карточки есть только в коде страницы, адреса на сайте нет")
        }
        (Some(false), Some(false)) if !name_on_site(&b.name, text, page.title.as_deref(), &domain_label) =>
        {
            MetricResult::fail(
            "H02",
                format!("На сайте нет ни названия, ни телефона ({}), ни адреса компании — похоже, это чужой или устаревший сайт", b.phone.clone().unwrap_or_default()),
            )
            .with_value("foreign")
        }
        (Some(false), Some(false)) => {
            let others = if site_phones.is_empty() { String::new() } else { format!("; на сайте другие номера ({})", site_phones.len()) };
            MetricResult::fail(
                "H02",
                format!("Телефона из карточки ({}) и её адреса ({}) на сайте нет{others}", b.phone.clone().unwrap_or_default(), b.address.clone().unwrap_or_default()),
            )
        }
        (Some(false), _) if card_phone.as_ref().is_some_and(|k| crate::discover::raw_phone_keys(&v.home.body).contains(k)) => {
            MetricResult::warn("H02", format!("Телефон из карточки ({}) есть только в коде страницы — посетитель его не видит", b.phone.clone().unwrap_or_default()))
        }
        (Some(false), _) => {
            let others = if site_phones.is_empty() { String::new() } else { format!(" — на сайте другие номера ({})", site_phones.len()) };
            MetricResult::warn("H02", format!("Телефона из карточки ({}) на сайте нет{others}", b.phone.clone().unwrap_or_default()))
        }
        (_, Some(false)) => MetricResult::warn("H02", "Адреса из карточки на сайте нет"),
    });
    let card_hours = b.sources.iter().any(|s| s.hours.as_deref().is_some_and(|h| !h.trim().is_empty()));
    let site_hours = crate::lexicon::hours_re().is_match(text);
    out.push(match (card_hours, site_hours) {
        // Сверять не с чем: часов нет в карточке — это не «совпадают» и не «не совпадают».
        (false, _) => MetricResult::na("H03", if site_hours { "В карточке часы не указаны — сверить не с чем (на сайте часы есть)" } else { "В карточке часы не указаны — сверить не с чем" }),
        (true, true) => MetricResult::pass("H03", "Часы работы на сайте указаны (сверка с картой — вручную)"),
        (true, false) => MetricResult::warn("H03", "Часов работы на сайте не найдено, а в карточке они есть"),
    });
    let html = &v.html_all;
    let map_widget = crate::lexicon::MAP_WIDGETS.iter().any(|m| html.contains(m));
    out.push(if map_widget { MetricResult::pass("H04", "Карта проезда встроена") } else { MetricResult::warn("H04", "Встроенной карты нет") });
    let card_links: Vec<String> = v
        .pages
        .iter()
        .flat_map(|p| p.links.iter())
        .map(|l| l.href.to_lowercase())
        .filter(|h| crate::lexicon::CARD_LINKS.iter().any(|m| h.contains(m)))
        .collect();
    out.push(if card_links.is_empty() {
        MetricResult::warn("H05", "Ссылок на карточки на картах нет")
    } else {
        MetricResult::pass("H05", list_short(&card_links.into_iter().collect::<HashSet<_>>().into_iter().collect::<Vec<_>>(), 2))
    });

    // ---------- I ----------
    ai_checks(ctx, v, out);
}

/// Слова адреса, которые не отличают одну улицу от другой.
const ADDR_SKIP: &[&str] = &[
    "улица", "проспект", "переулок", "шоссе", "бульвар", "набережная", "площадь", "москва", "россия", "город", "область",
    "район", "корпус", "строение", "street", "avenue", "road", "boulevard", "drive", "lane", "place", "court", "suite",
    "floor", "north", "south", "east", "west", "strasse", "straße", "platz", "allee", "gasse", "chemin", "avenida",
    "calle", "plaza", "piazza", "viale", "corso", "straat", "plein", "united", "states", "quai", "impasse", "allee",
    "carrer", "travessa", "avinguda", "paseo", "ulica", "aleja", "namesti", "square", "terrace", "parkway", "highway",
    "kingdom", "france", "espana", "deutschland", "italia", "nederland", "polska", "portugal",
];

/// Суффиксы составных названий улиц: «Oranienstraße» → «oranien».
const STREET_SUFFIXES: &[&str] = &["strasse", "straße", "str", "weg", "platz", "allee", "gasse", "damm", "ring", "ufer", "straat", "laan", "plein", "gracht"];

fn contains_word(text: &str, w: &str) -> bool {
    text.match_indices(w).any(|(i, _)| {
        let before = text[..i].chars().next_back();
        let after = text[i + w.len()..].chars().next();
        !before.is_some_and(|c| c.is_alphanumeric()) && !after.is_some_and(|c| c.is_alphanumeric())
    })
}

/// Улица и номер дома из адреса карточки найдены в тексте (текст — в нижнем регистре).
/// Улица берётся из части адреса с номером дома или из соседней с ней.
/// Белорусское написание → русское: карточки OSM и Overture в Беларуси часто по-белорусски,
/// а сайты — по-русски («вуліца Леніна» ↔ «ул. Ленина», «праспект Незалежнасці» ↔ «пр. Независимости»).
/// Применяется к обеим сторонам сравнения, поэтому русский текст портится одинаково и не мешает.
fn be_to_ru(s: &str) -> String {
    const WORDS: &[(&str, &str)] = &[
        ("вуліц", "улиц"), ("завулак", "переулок"), ("завулк", "переулк"), ("плошч", "площад"), ("набярэжн", "набережн"),
        ("шаша", "шоссе"), ("мікрараён", "микрорайон"), ("незалежнасц", "независимост"), ("пераможц", "победител"),
        ("перамог", "побед"), ("кастрычніцк", "октябрьск"), ("чырвонаармейск", "красноармейск"), ("савецк", "советск"),
        ("рэвалюцыйн", "революционн"), ("камсамольск", "комсомольск"), ("інтэрнацыянальн", "интернациональн"),
        ("старажоўск", "старожевск"), ("машэрав", "машеров"), ("сурганав", "сурганов"), ("багдановіч", "богданович"),
        ("няміг", "немиг"), ("маскоўск", "московск"), ("цэнтральн", "центральн"), ("гарадск", "городск"),
    ];
    let mut out = s.to_string();
    for (be, ru) in WORDS {
        out = out.replace(be, ru);
    }
    out.replace('і', "и").replace('ў', "в").replace('э', "е").replace(['\'', '’', 'ʼ'], "")
}

/// Белорусская латиница → кириллица: «Uĺjanaŭskaja» → «ульяноўская».
fn be_latin_to_cyr(s: &str) -> String {
    // По буквам с диакритикой — или по белорусским словам без неё («vulica Miasnikova 70», dsvd.by).
    let low = s.to_lowercase();
    let be_words = ["vulica", "vulitsa", "praspiekt", "prasp.", "zavulak", "ploshcha", "plošča", "tupik"].iter().any(|w| low.contains(w));
    if !be_words && !s.chars().any(|c| matches!(c, 'ĺ' | 'ŭ' | 'ž' | 'č' | 'š' | 'ć' | 'ś' | 'ń' | 'ź' | 'Ĺ' | 'Ŭ' | 'Ž' | 'Č' | 'Š')) {
        return s.to_string();
    }
    const PAIRS: &[(&str, &str)] = &[
        ("ja", "я"), ("ju", "ю"), ("je", "е"), ("ie", "е"), ("jo", "ё"), ("ia", "я"), ("iu", "ю"), ("io", "ё"), ("ĺ", "ль"), ("ŭ", "ў"), ("ž", "ж"), ("č", "ч"),
        ("š", "ш"), ("ć", "ць"), ("ś", "сь"), ("ń", "нь"), ("ź", "зь"), ("ch", "х"), ("a", "а"), ("b", "б"), ("v", "в"),
        ("h", "г"), ("g", "г"), ("d", "д"), ("e", "э"), ("z", "з"), ("i", "і"), ("j", "й"), ("k", "к"), ("l", "л"), ("m", "м"),
        ("n", "н"), ("o", "о"), ("p", "п"), ("r", "р"), ("s", "с"), ("t", "т"), ("u", "у"), ("f", "ф"), ("c", "ц"), ("y", "ы"),
    ];
    let lower = s.to_lowercase();
    let mut out = String::new();
    let mut rest = lower.as_str();
    'outer: while !rest.is_empty() {
        for (lat, cyr) in PAIRS {
            if let Some(r) = rest.strip_prefix(lat) {
                out.push_str(cyr);
                rest = r;
                continue 'outer;
            }
        }
        let c = rest.chars().next().unwrap();
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// Есть ли на сайте следы названия компании. Названия пишут по-разному: по-белорусски и
/// по-русски («Рускае радыё» ↔ «Русское радио»), кириллицей и латиницей («Місс Лі» ↔ «Miss Li»),
/// слитно и раздельно («ConcertHall»), а домен часто и есть название (respawn.by). Ложный флаг
/// «чужой домен» хуже пропущенного, поэтому сравнение снисходительное.
fn name_on_site(name: &str, text: &str, title: Option<&str>, domain_label: &str) -> bool {
    use crate::discover::translit_city as lat;
    let norm = |s: &str| crate::text::fold_diacritics(&be_to_ru(&s.to_lowercase().replace('ё', "е")).replace('о', "а"));
    let all: Vec<String> = crate::text::normalize_name(name).split_whitespace().filter(|w| w.chars().count() >= 4).map(norm).collect();
    if all.is_empty() {
        return true;
    }
    // Общие слова названия («туры», «мастерская», «клиника») есть на любом сайте отрасли — по ним
    // свой сайт не узнать (travelnews.by у «Мои Любимые Туры», ikona-skiniya.com у «Творческая
    // Мастерская Ирины Паньковой»). Сверяем отличительные слова, а если их нет — все.
    let generic = |t: &str| crate::discover::GENERIC.iter().chain(NAME_GENERIC).any(|g| norm(g) == t);
    let distinct: Vec<String> = all.iter().filter(|t| !generic(t)).cloned().collect();
    let tokens = if distinct.is_empty() { all } else { distinct };
    let multi = tokens.len() >= 2;
    let hay = norm(&format!("{} {}", title.unwrap_or(""), text));
    if tokens.iter().any(|t| hay.contains(t.as_str())) {
        return true;
    }
    // Латиницей: транслит слов текста и названия (а↔о уже сведены).
    let hay_words: Vec<String> = hay.split(|c: char| !c.is_alphanumeric()).filter(|w| w.chars().count() >= 3).map(|w| lat(w).replace('o', "a")).collect();
    let toks: Vec<String> = tokens.iter().map(|t| lat(t).replace('o', "a")).filter(|t| t.len() >= 4).collect();
    let compact_name: String = toks.concat();
    let compact_hay: String = hay_words.concat();
    if toks.iter().any(|t| hay_words.iter().any(|w| w == t)) || (compact_name.len() >= 5 && compact_hay.contains(&compact_name)) {
        return true;
    }
    // Домен похож на название: содержит слово названия или совпадает началом (belpost ↔ belposhta).
    // Если название — сам домен («Vegans.by»), совпадение с доменом ничего не доказывает.
    let label = domain_label.to_lowercase().replace('o', "a");
    let name_is_domain = name.to_lowercase().replace('o', "a").contains(&format!("{label}."));
    let prefix = |a: &str, b: &str| a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count();
    // В названии два отличительных слова и больше — одного совпавшего слова в домене мало
    // (zolotoi.by у «La Crête D'Or (Золотой Гребешок)» — сайт японских обоев).
    let label_hits = toks.iter().filter(|t| label.contains(t.as_str()) || t.contains(label.as_str())).count();
    if label.len() >= 4
        && !name_is_domain
        && (label_hits >= if multi { 2 } else { 1 }
            || prefix(&label, &compact_name) >= if multi { 8 } else { 5 }
            || (compact_name.contains(&label) && (!multi || label.len() >= 8))
            || (label.len() >= 6 && similar(&label, &compact_name) >= 0.55))
    {
        return true;
    }
    // Нечётко: слово текста отличается от слова названия парой букв («рускае» ≈ «русское»).
    toks.iter().any(|t| hay_words.iter().any(|w| w.len().abs_diff(t.len()) <= 2 && similar(w, t) >= 0.7))
}

/// Слова названий, которые есть на любом сайте отрасли (в дополнение к `discover::GENERIC`).
const NAME_GENERIC: &[&str] = &[
    "туры", "туров", "турагентство", "турфирма", "tours", "travel", "творческая", "агентство", "компания", "фирма", "ателье",
    "школа", "курсы", "клуб", "club", "school", "academy", "академия", "бюро", "кофейня", "coffee", "пиццерия", "пекарня",
    "отель", "гостиница", "hotel", "хостел", "hostel", "аптека", "оптика", "цветы", "flowers", "ремонт", "мебель", "маникюр",
    "массаж", "фитнес", "fitness", "йога", "yoga", "танцы", "dance", "фото", "photo", "печать", "print", "доставка", "delivery",
    "минск", "minsk", "беларусь", "belarus", "москва", "moscow", "london", "paris", "berlin", "madrid",
];

/// Домен перехвачен под азартные игры: на странице казино и ставки, а компания к ним отношения не имеет.
pub fn gambling_hijack(name: &str, category: Option<&str>, text: &str, title: Option<&str>) -> bool {
    const WORDS: &[&str] = &[
        "казино", "casino", "ставки", "букмекер", "слоты", "online slots", "slot machine", "1xbet", "фриспин", "покер", "poker", "рабочее зеркало",
        "фрибет", "коэффициенты", "беттинг", "betting", "игровые автоматы", "джекпот", "jackpot", "вейджер", "прогнозы на спорт",
        "бонус за регистрацию", "sportsbook", "bookmaker", "free spins", "no deposit bonus", "paris sportifs", "machines à sous",
        "apuestas", "tragamonedas", "tragaperras", "sportwetten", "spielautomaten", "freispiele", "wettanbieter", "scommesse",
        "giri gratis", "bukmacher", "zakłady", "automaty do gier", "apostas", "caça-níqueis", "gokken", "weddenschappen",
        // Индонезийский слот-спам на взломанных сайтах (Mother Dough: HAMTOTO, 7th Avenue Wine: JOS129 «gacor»).
        "gacor", "togel", "maxwin", "situs slot", "slot online", "judi online", "rtp live", "rtp slot", "slot88", "scatter hitam",
    ];
    let own = format!("{} {}", name, category.unwrap_or("")).to_lowercase();
    if WORDS.iter().any(|w| own.contains(w)) {
        return false;
    }
    let hay = format!("{} {}", title.unwrap_or(""), text).to_lowercase();
    // Три разных слова — или два, но много раз (boxberrypost.by: «ставки» ×13, «букмекер» ×2).
    let distinct = WORDS.iter().filter(|w| hay.contains(*w)).count();
    let total: usize = WORDS.iter().map(|w| hay.matches(*w).count()).sum();
    distinct >= 3 || (distinct >= 2 && total >= 8)
}

/// Код страны телефона из карточки: из «+33…»/«0033…», а для национального формата — по стране района.
fn card_calling_code(raw: &str, country: Option<&str>) -> Option<String> {
    let t = raw.trim();
    let digits: String = t.chars().filter(|c| c.is_ascii_digit()).collect();
    if t.starts_with('+') || t.starts_with("00") {
        let d = if t.starts_with("00") { &digits[2.min(digits.len())..] } else { digits.as_str() };
        (d.len() > 6).then(|| d[..crate::text::calling_code_len(d)].to_string())
    } else if digits.len() >= 7 {
        crate::region::calling_code(country).map(str::to_string)
    } else {
        None
    }
}

/// Телефоны сайта: сколько без кода страны (местные) и сколько с каждым кодом («+7», «+44») — по тексту и tel:.
fn site_phone_countries(text: &str, html: &str) -> (usize, std::collections::HashMap<String, usize>) {
    static TEL: OnceLock<Regex> = OnceLock::new();
    let mut intl: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut national = 0;
    let mut count = |raw: &str| {
        let t = raw.trim().trim_start_matches('(').replace("%2b", "+").replace("%2B", "+");
        let digits: String = t.chars().filter(|c| c.is_ascii_digit()).collect();
        if t.starts_with('+') || t.starts_with("00") {
            let d = if t.starts_with("00") { &digits[2.min(digits.len())..] } else { digits.as_str() };
            if d.len() > 6 {
                *intl.entry(d[..crate::text::calling_code_len(d)].to_string()).or_default() += 1;
            }
        } else if digits.len() >= 7 {
            national += 1;
        }
    };
    for m in crate::text::phone_matches(text) {
        count(&m);
    }
    for c in re(&TEL, r#"tel:([+0-9%][0-9%a-f()\-\s.]{5,30})"#).captures_iter(html) {
        count(&c[1]);
    }
    (national, intl)
}

/// Адрес — азиатская языковая версия (/ja-jp/, /jp/, .jp…): сайт бренда отдал её по IP выхода VPN.
fn asian_locale(u: &url::Url) -> bool {
    let host = u.host_str().unwrap_or("");
    // Языковой сегмент адреса в любом порядке: /ja/, /ja-jp/, /jp-ja/ (zegna.com), /zh_CN/, /ko-kr/.
    const ASIAN: &[&str] = &["ja", "jp", "zh", "cn", "ko", "kr", "tw", "hk"];
    let segment_is_asian = |seg: &str| {
        let parts: Vec<String> = seg.to_lowercase().split(['-', '_']).map(str::to_string).collect();
        parts.len() <= 2 && parts.iter().all(|p| p.len() == 2 || p.len() == 4) && parts.iter().any(|p| ASIAN.contains(&p.as_str()))
    };
    [".jp", ".cn", ".kr", ".tw", ".hk"].iter().any(|t| host.ends_with(t))
        || host.split('.').next().is_some_and(|sub| ["jp", "ja", "cn", "zh", "kr", "ko"].contains(&sub))
        || u.path_segments().into_iter().flatten().take(2).any(segment_is_asian)
}

/// Текст в основном на китайском, японском или корейском — у компании из Беларуси или России это чужой сайт.
fn mostly_cjk(text: &str) -> bool {
    let letters = text.chars().filter(|c| c.is_alphabetic()).count();
    let cjk = text.chars().filter(|c| matches!(*c as u32, 0x3040..=0x30FF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF)).count();
    letters > 50 && cjk * 3 > letters
}

/// Похожесть строк по расстоянию Левенштейна: 1 — одинаковые.
fn similar(a: &str, b: &str) -> f64 {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            cur[j + 1] = (prev[j] + usize::from(ca != cb)).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        prev = cur;
    }
    1.0 - prev[b.len()] as f64 / a.len().max(b.len()).max(1) as f64
}

pub fn address_matches(addr: &str, text: &str) -> bool {
    // Белорусская латиница в карточке («vulica Uĺjanaŭskaja 29») — второй попыткой в кириллице;
    // польские, чешские, литовские адреса с теми же буквами сверяем как есть.
    let be = be_latin_to_cyr(addr);
    // Русская транслитерация (Overture: «Ulitsa L'va Sapegi 9» ↔ «ул. Льва Сапеги, дом 9») — третьей.
    let ru = ru_latin_to_cyr(addr);
    address_matches_one(addr, text) || (be != addr && address_matches_one(&be, text)) || (ru != addr && address_matches_one(&ru, text))
}

/// Русская латиница (ГОСТ/BGN, как в Overture и Google) → кириллица. Только если в адресе есть
/// русское слово улицы: «Ulitsa», «Prospekt», «Pereulok»… — иначе латинский адрес не трогаем.
fn ru_latin_to_cyr(s: &str) -> String {
    let low = s.to_lowercase();
    const STREET: &[&str] = &["ulitsa", "ulitca", "prospekt", "pereulok", "ploshchad", "shosse", "bulvar", "naberezhnaya", "proyezd", "proezd", "tupik", "mikrorayon"];
    if !STREET.iter().any(|w| low.contains(w)) {
        return s.to_string();
    }
    const PAIRS: &[(&str, &str)] = &[
        ("shch", "щ"), ("sch", "щ"), ("sh", "ш"), ("ch", "ч"), ("zh", "ж"), ("kh", "х"), ("ts", "ц"), ("ya", "я"), ("yu", "ю"),
        ("yo", "е"), ("ye", "е"), ("iy", "ий"), ("yy", "ый"), ("'", "ь"), ("’", "ь"), ("a", "а"), ("b", "б"), ("v", "в"), ("g", "г"),
        ("d", "д"), ("e", "е"), ("z", "з"), ("i", "и"), ("y", "ы"), ("j", "й"), ("k", "к"), ("l", "л"), ("m", "м"), ("n", "н"),
        ("o", "о"), ("p", "п"), ("r", "р"), ("s", "с"), ("t", "т"), ("u", "у"), ("f", "ф"), ("h", "х"), ("c", "ц"),
    ];
    let mut out = String::new();
    let mut rest = low.as_str();
    'outer: while !rest.is_empty() {
        for (lat, cyr) in PAIRS {
            if let Some(r) = rest.strip_prefix(lat) {
                out.push_str(cyr);
                rest = r;
                continue 'outer;
            }
        }
        let c = rest.chars().next().unwrap();
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

fn address_matches_one(addr: &str, text: &str) -> bool {
    use crate::text::fold_diacritics as fold;
    // Белорусское аканье: «Кірава» ↔ «Кирова» — сравниваем без различия о/а (на обеих сторонах);
    // латиница — без диакритики («Élysées» ↔ «Elysees»).
    let lower = fold(&be_to_ru(&addr.to_lowercase().replace('ё', "е").replace('ß', "ss")).replace('о', "а"));
    let text = fold(&be_to_ru(&text.to_lowercase().replace('ß', "ss").replace('ё', "е")).replace('о', "а"));
    let parts: Vec<&str> = lower.split(',').map(str::trim).filter(|p| !p.is_empty()).collect();
    let is_house = |w: &str| w.chars().next().is_some_and(|c| c.is_ascii_digit()) && w.chars().count() <= 6;
    let words = |p: &str| p.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_string).collect::<Vec<_>>();
    let street_word = |p: &str| {
        words(p)
            .into_iter()
            .filter(|w| w.chars().count() >= 4 && !w.chars().any(|c| c.is_ascii_digit()) && !ADDR_SKIP.iter().any(|s| s.replace('о', "а") == *w))
            .max_by_key(|w| w.chars().count())
    };
    let house_idx = parts.iter().position(|p| words(p).iter().any(|w| is_house(w)));
    let (street, house) = match house_idx {
        Some(i) => {
            let house = words(parts[i]).into_iter().find(|w| is_house(w));
            let street = street_word(parts[i]).or_else(|| i.checked_sub(1).and_then(|j| street_word(parts[j])));
            (street, house)
        }
        None => (parts.iter().find_map(|p| street_word(p)), None),
    };
    let Some(street) = street else { return false };
    let mut base = street.clone();
    for suf in STREET_SUFFIXES {
        if let Some(b) = street.strip_suffix(suf)
            && b.chars().count() >= 4
        {
            base = b.to_string();
            break;
        }
    }
    let n = base.chars().count();
    let stem: String = base.chars().take(if n > 5 { n - 2 } else { n }).collect();
    // Дом «21А» на сайте могут написать латинской «21A»: тогда сверяем только цифры
    // (улица уже совпала, так что случайное совпадение номера маловероятно).
    let house_ok = |h: &str| {
        let digits: String = h.chars().take_while(|c| c.is_ascii_digit()).collect();
        let digits_ok = || {
            text.match_indices(digits.as_str()).any(|(i, _)| {
                let before = text[..i].chars().next_back();
                let after = text[i + digits.len()..].chars().next();
                !before.is_some_and(|c| c.is_alphanumeric()) && !after.is_some_and(|c| c.is_ascii_digit())
            })
        };
        contains_word(&text, h) || (digits.len() < h.chars().count() && !digits.is_empty() && digits_ok())
    };
    (text.contains(&stem) && house.as_deref().is_none_or(house_ok)) || house.as_deref().is_some_and(|h| street_near_house(&street, h, &text))
}

/// Костяк кириллического слова — согласные без повторов: белорусское и русское написание одной
/// улицы расходятся в гласных (ы/и, я/е, а/о, і/и), в «дз»/«д», «ц»/«т», «шч»/«щ», ў/в, а согласные
/// совпадают: «Прытыцкага» и «Притыцкого» → «прткг», «Дзяржынскага» и «Дзержинского» → «држнскг».
fn consonant_skeleton(w: &str) -> String {
    let w = w.replace("дз", "д").replace("шч", "щ").replace('ц', "т").replace('ў', "в");
    let mut out = String::new();
    for c in w.chars().filter(|c| c.is_alphabetic() && !"аеёиіоуыэюяьъй".contains(*c)) {
        if !out.ends_with(c) {
            out.push(c);
        }
    }
    out
}

/// Запасное сравнение кириллической улицы по костяку согласных — без словаря, для любых улиц.
/// Слабее точного, поэтому номер дома обязан быть первым числом после улицы, а в костяке —
/// не меньше трёх согласных.
fn street_near_house(street: &str, house: &str, text: &str) -> bool {
    if !street.chars().any(|c| matches!(c, 'а'..='я' | 'ё' | 'і' | 'ў')) {
        return false;
    }
    let sk = consonant_skeleton(street);
    let digits: String = house.chars().take_while(|c| c.is_ascii_digit()).collect();
    if sk.chars().count() < 3 || digits.is_empty() {
        return false;
    }
    // Первое число после улицы (не дальше 20 знаков: «, д. 39», « 39а») — номер дома.
    let house_after = |from: usize| {
        let first: String = text[from..].chars().take(20).skip_while(|c| !c.is_ascii_digit()).take_while(char::is_ascii_digit).collect();
        first == digits
    };
    let mut start = None;
    for (i, c) in text.char_indices().chain(std::iter::once((text.len(), ' '))) {
        if c.is_alphanumeric() {
            start.get_or_insert(i);
        } else if let Some(s) = start.take()
            && consonant_skeleton(&text[s..i]) == sk
            && house_after(i)
        {
            return true;
        }
    }
    false
}

const LOCAL_TYPES: &[&str] = &[
    "localbusiness", "dentist", "medicalclinic", "medicalbusiness", "medicalorganization", "physician", "hospital",
    "pharmacy", "optician", "veterinarycare", "autorepair", "autobodyshop", "autodealer", "autopartsstore",
    "autorental", "autowash", "automotivebusiness", "gasstation", "beautysalon", "hairsalon", "nailsalon", "dayspa",
    "healthandbeautybusiness", "healthclub", "exercisegym", "sportsclub", "sportsactivitylocation", "restaurant",
    "cafeorcoffeeshop", "foodestablishment", "bakery", "barorpub", "fastfoodrestaurant", "hotel", "hostel", "motel",
    "lodgingbusiness", "bedandbreakfast", "legalservice", "attorney", "notary", "realestateagent", "store", "artgallery",
    "nightclub", "movietheater", "bowlingalley", "amusementpark", "entertainmentbusiness", "comedyclub", "casino",
    "shoppingcenter", "brewery", "winery", "tattooparlor", "museum", "touristattraction", "sportsclub", "golfcourse",
    "florist", "furniturestore", "hardwarestore", "electronicsstore", "clothingstore", "homeandconstructionbusiness",
    "electrician", "plumber", "locksmith", "movingcompany", "generalcontractor", "professionalservice",
    "drycleaningorlaundry", "childcare", "travelagency", "financialservice", "educationalorganization",
    "entertainmentbusiness", "employmentagency", "tattooparlor",
];

fn types_of(v: &Value) -> Vec<String> {
    match &v["@type"] {
        Value::String(s) => vec![s.to_lowercase()],
        Value::Array(a) => a.iter().filter_map(|x| x.as_str()).map(str::to_lowercase).collect(),
        _ => vec![],
    }
}

fn flatten(v: &Value, out: &mut Vec<Value>) {
    match v {
        Value::Array(a) => a.iter().for_each(|x| flatten(x, out)),
        Value::Object(o) => {
            if let Some(g) = o.get("@graph") {
                flatten(g, out);
            }
            if o.contains_key("@type") {
                out.push(v.clone());
            }
            for (k, child) in o {
                if k != "@graph" && (child.is_object() || child.is_array()) {
                    flatten(child, out);
                }
            }
        }
        _ => {}
    }
}

fn is_local(types: &[String]) -> bool {
    // «…Shop» — не тип schema.org (там Florist), но так пишут часто (buket.by: FlowerShop) — считаем
    // местным бизнесом, а не «разметки нет».
    types.iter().any(|t| LOCAL_TYPES.contains(&t.as_str()) || t.ends_with("store") || t.ends_with("clinic") || t.ends_with("salon") || t.ends_with("shop"))
}

fn schema_checks(ctx: &CheckCtx, v: &SiteView, out: &mut Vec<MetricResult>) {
    let mut objects = Vec::new();
    let mut errors = 0;
    let mut no_context = 0;
    let mut blocks = 0;
    for p in &v.pages {
        for raw in &p.jsonld {
            // Пустой блок из шаблона CMS (Bitrix) — не ошибка разметки.
            if raw.trim().is_empty() {
                continue;
            }
            blocks += 1;
            match serde_json::from_str::<Value>(raw.trim()) {
                Ok(val) => {
                    let top_has_ctx = match &val {
                        Value::Array(a) => a.iter().all(|x| x.get("@context").is_some()),
                        x => x.get("@context").is_some(),
                    };
                    if !top_has_ctx {
                        no_context += 1;
                    }
                    flatten(&val, &mut objects);
                }
                Err(_) => errors += 1,
            }
        }
    }
    let microdata = v.pages.iter().any(|p| p.microdata);
    let micro_types: Vec<String> = v.pages.iter().flat_map(|p| p.microdata_types.iter().cloned()).collect();
    out.push(if blocks > 0 || microdata {
        MetricResult::pass("G01", format!("JSON-LD блоков: {blocks}{}", if microdata { ", есть микроданные" } else { "" }))
    } else {
        MetricResult::fail("G01", "Структурированной разметки нет")
    });
    // Разметки нет совсем: проверять нечего. Остальные метрики разметки — «не измерено», а не провал,
    // иначе клиенту пишется и «нет разметки», и «в разметке нет данных о компании» (belodent.by).
    if blocks == 0 && !microdata {
        for id in ["G02", "G03", "G04", "G05", "G06", "G07", "G08"] {
            out.push(MetricResult::na(id, "Разметки нет — см. G01"));
        }
        return;
    }
    let all_types: HashSet<String> = objects.iter().flat_map(types_of).collect();
    let local = objects.iter().find(|o| is_local(&types_of(o)));
    let org = local.or_else(|| objects.iter().find(|o| types_of(o).iter().any(|t| t == "organization" || t == "corporation")));
    match local {
        Some(o) => {
            let need = [("name", "name"), ("address", "address"), ("telephone", "telephone"), ("geo", "geo"), ("url", "url")];
            let mut missing: Vec<&str> = need.iter().filter(|(k, _)| o.get(*k).is_none()).map(|(_, l)| *l).collect();
            if o.get("openingHours").is_none() && o.get("openingHoursSpecification").is_none() {
                missing.push("openingHours");
            }
            let ty = types_of(o).join(", ");
            out.push(if missing.is_empty() {
                MetricResult::pass("G02", format!("{ty}: все ключевые поля заполнены"))
            } else {
                MetricResult::warn("G02", format!("{ty}: нет полей {}", missing.join(", ")))
            });
        }
        None if org.is_some() => out.push(MetricResult::warn("G02", "Есть только Organization, без LocalBusiness")),
        None if is_local(&micro_types) => out.push(MetricResult::warn("G02", "LocalBusiness — микроданными (itemtype), поля не проверялись; для ИИ-поиска лучше JSON-LD")),
        None => out.push(MetricResult::fail("G02", "Разметки LocalBusiness нет")),
    }
    out.push(match org {
        Some(o) => match (o.get("logo").is_some() || o.get("image").is_some(), o.get("sameAs").is_some()) {
            (true, true) => MetricResult::pass("G03", "Есть логотип и sameAs"),
            (_, false) => MetricResult::warn("G03", "Нет sameAs — ссылок на карточки и соцсети"),
            (false, true) => MetricResult::warn("G03", "Нет логотипа"),
        },
        None if micro_types.iter().any(|t| t == "organization" || t == "corporation") || is_local(&micro_types) => {
            MetricResult::warn("G03", "Организация размечена микроданными (itemtype) — логотип и sameAs не проверялись")
        }
        None => MetricResult::fail("G03", "Разметки организации нет"),
    });
    if blocks == 0 {
        out.push(MetricResult::na("G04", if microdata { "Микроданные не валидируются в этой версии" } else { "Разметки нет" }));
    } else {
        out.push(if errors > 0 {
            MetricResult::fail("G04", format!("JSON-LD с синтаксическими ошибками: {errors} из {blocks}"))
        } else if no_context > 0 {
            MetricResult::warn("G04", format!("Блоков без @context: {no_context}"))
        } else {
            MetricResult::pass("G04", "JSON-LD разбирается без ошибок")
        });
    }
    let card_phone = ctx.business.phone.as_deref().and_then(phone_key);
    let all_markup: Vec<String> = v
        .pages
        .iter()
        .flat_map(|p| p.jsonld.iter())
        .flat_map(|j| j.split("\"telephone\"").skip(1).filter_map(|rest| rest.split('"').nth(1).and_then(phone_key)).collect::<Vec<_>>())
        .collect();
    let markup_phone = match &card_phone {
        Some(c) if all_markup.contains(c) => Some(c.clone()),
        _ => local.and_then(|o| o["telephone"].as_str()).and_then(phone_key),
    };
    // Номера, видимые на сайте (текст и tel:-ссылки): если номер разметки на сайте есть, а номера
    // карточки нет — устарела карточка на картах (OSM, Overture), а не разметка (belstom.by, g-y.by).
    let site_phones: HashSet<String> = v
        .pages
        .iter()
        .flat_map(|p| crate::text::phones_in_text(&p.text).into_iter().chain(p.tel_links.iter().filter_map(|t| phone_key(t))))
        .collect();
    out.push(match (markup_phone, card_phone) {
        (Some(m), Some(c)) if m == c => MetricResult::pass("G05", "Телефон в разметке совпадает с карточкой"),
        (Some(m), Some(c)) if site_phones.contains(&m) && !site_phones.contains(&c) => {
            MetricResult::warn("G05", "В разметке и на сайте один номер, а в карточке на картах — другой: стоит обновить карточку")
        }
        // Оба номера на сайте есть (g-y.by: в карточке +375 29…, в разметке +375 44…): ошибки нет,
        // но поисковику и ИИ лучше видеть один основной номер везде.
        (Some(m), Some(c)) if site_phones.contains(&m) && site_phones.contains(&c) => {
            MetricResult::warn("G05", "В разметке и в карточке разные номера, оба есть на сайте — лучше указать один основной номер везде")
        }
        (Some(_), Some(_)) => MetricResult::fail("G05", "Телефон в разметке отличается от карточки"),
        _ => MetricResult::na("G05", "Нет телефона в разметке или в карточке"),
    });
    match org {
        Some(o) if o.get("aggregateRating").is_some() || o.get("review").is_some() => {
            out.push(MetricResult::warn("G06", "Отзывы о себе в разметке: Google не покажет за них звёзды"))
        }
        Some(_) => out.push(MetricResult::pass("G06", "Самоназначенных отзывов в разметке нет")),
        None => out.push(MetricResult::na("G06", "Разметки организации нет")),
    }
    let offers = objects.iter().any(|o| {
        let t = types_of(o);
        (t.iter().any(|x| matches!(x.as_str(), "offer" | "aggregateoffer" | "pricespecification")) && (o.get("price").is_some() || o.get("lowPrice").is_some()))
            || (t.iter().any(|x| matches!(x.as_str(), "product" | "service")) && o.get("offers").is_some())
    });
    let prices_in_text = price_re().is_match(&v.text_all);
    out.push(match (offers, prices_in_text) {
        (true, _) => MetricResult::pass("G07", "Цены размечены (Offer/Product/Service)"),
        (false, true) => MetricResult::warn("G07", "Цены на сайте есть, но не размечены"),
        (false, false) => MetricResult::na("G07", "Цен на сайте нет"),
    });
    out.push(if all_types.contains("breadcrumblist") {
        MetricResult::pass("G08", "BreadcrumbList есть")
    } else if v.pages.len() > 1 {
        MetricResult::warn("G08", "BreadcrumbList на страницах нет")
    } else {
        MetricResult::na("G08", "Внутренние страницы не обойдены")
    });
}

/// Поисковые ИИ-боты (I02): только те, что ищут и отвечают с источниками. Боты обучения моделей
/// (GPTBot, ClaudeBot, Google-Extended, CCBot…) — отдельная метрика I04: на поиск они не влияют.
const AI_SEARCH: &[&str] = &["OAI-SearchBot", "Claude-SearchBot", "PerplexityBot"];
const AI_USER: &[&str] = &["ChatGPT-User", "Claude-User", "Perplexity-User"];
const AI_TRAIN: &[&str] = &["GPTBot", "ClaudeBot", "Google-Extended", "Applebot-Extended", "CCBot", "Meta-ExternalAgent"];

/// Запреты вписал Cloudflare (Managed robots.txt), а не владелец сайта.
fn cloudflare_managed(robots: &str) -> bool {
    let l = robots.to_lowercase();
    l.contains("cloudflare managed") || l.contains("content-signal:")
}

fn ai_checks(ctx: &CheckCtx, v: &SiteView, out: &mut Vec<MetricResult>) {
    let page = v.page;
    let home_page = robots::page_url(&v.home.final_url);
    let home_url = home_page.as_str();
    let robots_body = v.robots();
    let blocked_for = |agents: &[&str]| -> Vec<String> {
        robots_body.map_or(vec![], |b| agents.iter().filter(|a| !robots::allowed(b, a, home_url)).map(|a| a.to_string()).collect())
    };

    let snippet_block = page.meta_robots.iter().any(|m| m.contains("nosnippet") || m.contains("max-snippet:0"))
        || v.home.headers_all("x-robots-tag").any(|x| x.to_lowercase().contains("nosnippet"));
    let google_ok = robots_body.is_none_or(|b| robots::allowed(b, "Googlebot", home_url));
    out.push(if !google_ok {
        MetricResult::fail("I01", "Googlebot закрыт в robots.txt — сайта не будет в AI Overviews и AI Mode").with_value("robots")
    } else if snippet_block {
        MetricResult::fail("I01", "nosnippet / max-snippet:0 — Google не сможет цитировать страницу").with_value("nosnippet")
    } else if page.data_nosnippet {
        MetricResult::warn("I01", "Часть текста помечена data-nosnippet").with_value("data_nosnippet")
    } else {
        MetricResult::pass("I01", "Googlebot пущен, сниппет не ограничен")
    });
    let search_blocked: Vec<String> = blocked_for(AI_SEARCH);
    let user_blocked = blocked_for(AI_USER);
    // Закрытые поисковые боты и боты по запросу пользователя — в значение: по ним клиентскому тексту
    // называют ровно те сервисы, чей поиск закрыт. Боты обучения (GPTBot, ClaudeBot) сюда не входят.
    let all_closed = [search_blocked.clone(), user_blocked.clone()].concat().join(", ");
    let cf_note = if robots_body.is_some_and(cloudflare_managed) {
        " — запрет вписал Cloudflare (управляемый robots.txt), снимается в панели Cloudflare"
    } else {
        ""
    };
    out.push(if !search_blocked.is_empty() {
        MetricResult::fail("I02", format!("Закрыты поисковые ИИ-боты: {}{cf_note}", search_blocked.join(", "))).with_value(all_closed.clone())
    } else if !user_blocked.is_empty() {
        MetricResult::warn("I02", format!("Закрыты боты по запросу пользователя: {}", user_blocked.join(", "))).with_value(all_closed.clone())
    } else {
        MetricResult::pass("I02", "Поисковые ИИ-боты не запрещены")
    });
    let yandex_ai = v.snap.robots_directives.as_ref().and_then(|d| {
        d.explicit_group("YandexAdditional").or_else(|| d.explicit_group("YandexAdditionalBot")).map(|g| g.disallow_all && !g.allow_root)
    });
    out.push(if !crate::region::yandex_market(ctx.params.country.as_deref()) {
        MetricResult::na("I03", "Яндекс в этой стране почти не используется")
    } else if yandex_ai == Some(true) {
        MetricResult::fail("I03", "YandexAdditional запрещён — сайта не будет в ответах Нейро и Алисы")
    } else {
        MetricResult::pass("I03", "YandexAdditional не запрещён")
    });
    let train = blocked_for(AI_TRAIN);
    out.push(MetricResult::info("I04", if train.is_empty() { "Обучающие боты не запрещены".to_string() } else { format!("Запрещены для обучения: {}{cf_note}", train.join(", ")) }).with_value(train.join(", ")));

    // Сравниваем с тем, что получил браузер: заглушка у бота считается, только если у браузера её нет.
    // Бота, закрытого в robots.txt, не проверяем: это I02, а не защита сайта.
    let browser_ok = v.home.ok();
    let browser_challenge = crate::site::looks_like_challenge(v.home);
    let open_bots: Vec<&crate::site::BotProbe> = v.snap.bots.iter().filter(|b| robots_body.is_none_or(|r| robots::allowed(r, &b.agent, home_url))).collect();
    // Любой ответ не 200 (403, 429, 503, нестандартные 490 и т. п. у Qrator/DDoS-Guard) — защита режет бота.
    let blocked: Vec<String> = open_bots
        .iter()
        .filter(|b| browser_ok && ((b.challenge && !browser_challenge) || b.status.is_some_and(|s| s >= 400)))
        .map(|b| format!("{} ({})", b.agent, if b.challenge { "заглушка WAF".to_string() } else { b.status.unwrap_or(0).to_string() }))
        .collect();
    let errors = open_bots.iter().filter(|b| b.error.is_some()).count();
    // Мы шлём User-Agent бота со своего IP. Крупные CDN (Cloudflare, Akamai) проверяют ботов по
    // IP и отказывают именно самозванцам, а настоящих пускают. Поэтому отказ — повод проверить
    // настройки, а не доказательство, что ИИ сайт не видит: «внимание», не стоп-фактор.
    out.push(if !blocked.is_empty() {
        MetricResult::warn(
            "I05",
            format!(
                "Запросам с User-Agent ИИ-ботов с чужого IP защита отказала: {}. Настоящих ботов (с их IP) она может пускать — проверьте настройки CDN или логи",
                blocked.join(", ")
            ),
        )
    } else if open_bots.is_empty() && !v.snap.bots.is_empty() {
        MetricResult::na("I05", "Боты закрыты в robots.txt — защиту сайта не проверить (см. I02)")
    } else if errors == open_bots.len() && errors > 0 {
        MetricResult::na("I05", "Пробы ботов не прошли (сеть)")
    } else {
        MetricResult::pass("I05", format!("Боты получают страницу: {}", open_bots.iter().map(|b| format!("{} {}", b.agent, b.status.unwrap_or(0))).collect::<Vec<_>>().join(", ")))
    });
    let spa_markers = ["<div id=\"root\"></div>", "<div id=\"app\"></div>", "id=\"__nuxt\"", "<app-root", "ng-version"];
    let spa = spa_markers.iter().any(|m| page.html_lower.contains(m));
    // JSON-LD появляется только после скриптов: ИИ-боты без JS не увидят цены, адрес и часы из разметки.
    let jsonld_by_js = v.snap.raw_jsonld == Some(0) && !page.jsonld.is_empty();
    let i06 = if let Some(raw) = v.snap.raw_words {
        // Без текста скрытых окон и cookie-баннеров, которые появляются после рендера.
        let hidden_words = page.hidden_text.split_whitespace().count();
        let rendered = page.words.saturating_sub(hidden_words).max(raw);
        let share = raw as f64 / rendered.max(1) as f64;
        if share >= 0.9 || rendered - raw < 100 {
            MetricResult::pass("I06", format!("В исходном HTML {raw} слов из {} после рендера", page.words))
        } else {
            MetricResult::fail(
                "I06",
                if raw >= 100 {
                    // 680 из 1606 слов (restoransvoi.by), 155 слов (flamingobabybk.com) — не «пустая», но часть текста боты не видят.
                    // «Почти пустая» — только когда в исходном HTML меньше 100 слов.
                    format!("Без выполнения скриптов видна только часть текста: {raw} слов из {} — остальное ИИ-боты (GPTBot, ClaudeBot, PerplexityBot) не прочитают", page.words)
                } else {
                    format!("В исходном HTML {raw} слов, после выполнения скриптов — {}: ИИ-боты (GPTBot, ClaudeBot, PerplexityBot) видят почти пустую страницу", page.words)
                },
            )
        }
        .with_value(share)
    } else if v.snap.js_shell && v.snap.render_error.as_deref().is_some_and(|e| e.contains("не прибавилось")) {
        // Рендер был, текста не прибавилось: страница просто почти без текста, скрипты ни при чём.
        MetricResult::warn("I06", format!("На странице всего {} слов и после выполнения скриптов", page.words))
    } else if v.snap.js_shell {
        MetricResult::na("I06", format!("В исходном HTML {} слов, рендер не удался — рисуют ли текст скрипты, не проверить", page.words))
    } else if page.words >= 150 {
        MetricResult::pass("I06", format!("В исходном HTML {} слов", page.words))
    } else if page.words < 50 && spa {
        MetricResult::fail("I06", format!("В исходном HTML {} слов и пустой контейнер приложения — текст рисуют скрипты, ИИ-боты его не увидят", page.words))
    } else {
        MetricResult::warn("I06", format!("В исходном HTML всего {} слов", page.words))
    }
    .with_value(page.words);
    out.push(if jsonld_by_js && i06.status == Status::Pass {
        MetricResult { status: Status::Warn, detail: format!("{}, но разметку JSON-LD добавляют скрипты — ИИ-боты без JS её не увидят", i06.detail), ..i06 }
    } else {
        i06
    });

    let text = &v.text_all;
    let phone_text = !phones_in_text(text).is_empty();
    let addr_text = address_re().is_match(text);
    let price_files: Vec<String> = v
        .pages
        .iter()
        .flat_map(|p| p.links.iter())
        .filter(|l| {
            let h = l.href.to_lowercase();
            let t = l.text.to_lowercase();
            (h.ends_with(".pdf") || h.ends_with(".jpg") || h.ends_with(".png") || h.ends_with(".doc") || h.ends_with(".docx") || h.ends_with(".xlsx"))
                && crate::lexicon::PRICE_FILE_WORDS.iter().any(|w| h.contains(w) || t.contains(w))
        })
        .map(|l| l.href.clone())
        .collect();
    out.push(if !phone_text && !addr_text {
        MetricResult::fail("I07", "Телефона и адреса нет в тексте")
    } else if !price_files.is_empty() {
        MetricResult::warn("I07", format!("Прайс выложен файлом: {}", list_short(&price_files, 2)))
    } else if phone_text && addr_text {
        MetricResult::pass("I07", "Телефон и адрес — текстом")
    } else {
        MetricResult::warn("I07", if phone_text { "Адреса текстом нет" } else { "Телефона текстом нет" })
    });
    let mut prices = price_re().find_iter(text).count();
    if prices < 3 {
        // В Беларуси и России меню и прайсы часто без знака валюты: «Брускетта … 19.00».
        static BARE: OnceLock<Regex> = OnceLock::new();
        let bare = re(&BARE, r"(?:^|[\s:|—–-])\d{1,5}[.,]\d{2}(?:$|[\s|;])").find_iter(text).count();
        let context = [
            "цен", "стоимост", "прейскурант", "прайс", "меню", "price", "menu", "prix", "tarif", "precio", "preis", "prezz", "carta",
            "speisekarte", "cennik", "prijs", "preço", "menú",
        ]
        .iter()
        .any(|w| text.contains(w));
        if context && bare >= 3 {
            prices += bare;
        }
    }
    if prices < 3 {
        // Валюта одна на столбец: «Цена, бел. руб.» — а в ячейках целые числа («45», «120»).
        static HEAD: OnceLock<Regex> = OnceLock::new();
        static INT: OnceLock<Regex> = OnceLock::new();
        if re(
            &HEAD,
            r"(?:цена|стоимость|price|prix|precio|preis|prezzo|cena|prijs|preço|preco|tarif)[,\s(:]*(?:in\s+|en\s+|в\s+)?(?:бел\.?\s*)?(?:руб|byn|br\b|₽|тенге|usd|\$|€|eur\b|£|gbp|chf|zł|pln)",
        )
        .is_match(text)
        {
            let ints = re(&INT, r"(?:^|[\s|:])\d{2,5}(?:$|[\s|;])").find_iter(text).count();
            if ints >= 3 {
                prices += ints;
            }
        }
    }
    if prices < 3 {
        // Страница цен (/ceny, /prices…) с таблицей чисел без знака валюты (bogemiastom.by/ceny:
        // «1200», «990», «2400»): валюта подразумевается — это прайс.
        static PRICE_INT: OnceLock<Regex> = OnceLock::new();
        const PRICE_SLUGS: &[&str] = &[
            "ceny", "tseny", "/cena", "price", "prais", "prajs", "praice", "stoimost", "tarif", "preise", "kosten", "prix", "precio",
            "prezzi", "cennik", "prijzen", "precos", "preco",
        ];
        let int_re = re(&PRICE_INT, r"(?:^|[\s|:])\d{2,5}(?:$|[\s|;])");
        if let Some(n) = v
            .pages
            .iter()
            .filter(|p| url::Url::parse(&p.url).is_ok_and(|u| PRICE_SLUGS.iter().any(|s| u.path().to_lowercase().contains(s))))
            .map(|p| int_re.find_iter(&p.text).count())
            .find(|n| *n >= 5)
        {
            prices += n;
        }
    }
    // Каталог с ценами — в магазине на поддомене (shop.…), куда обход сайта не заходит.
    let own_domain = crate::text::registrable_domain(v.final_url.host_str().unwrap_or(""));
    let shop_subdomain = v.pages.iter().flat_map(|p| p.links.iter()).find_map(|l| {
        let host = url::Url::parse(&l.href).ok()?.host_str()?.to_lowercase();
        let first = host.trim_start_matches("www.").split('.').next()?.to_string();
        (["shop", "store", "catalog", "market", "magazin", "eshop"].contains(&first.as_str()) && crate::text::registrable_domain(&host) == own_domain).then_some(host)
    });
    out.push(match (prices, shop_subdomain) {
        (0..=2, Some(h)) => MetricResult::warn("I08", format!("Цены — в магазине на поддомене {h} (не проверялся)")).with_value(prices),
        (0, None) => {
            let vague = crate::lexicon::any_in(text, crate::lexicon::VAGUE_PRICE);
            MetricResult::fail("I08", match vague.first() {
                Some(w) => format!("Цен цифрами нет, только «{w}»"),
                None => "Цен цифрами нет".to_string(),
            })
            .with_value(0)
        }
        _ => MetricResult::new("I08", match prices { 0 => Status::Fail, 1 | 2 => Status::Warn, _ => Status::Pass }, format!("Упоминаний цен: {prices}")).with_value(prices),
    });
    let faq_schema = v.pages.iter().any(|p| p.jsonld.iter().any(|j| j.contains("FAQPage")));
    // Раздел вопросов обычными абзацами («ВОПРОС — ОТВЕТ: Повреждается ли эмаль…?», narodent.by):
    // заголовка у раздела нет, но вопросы со знаком «?» под подписью раздела — это ответы на вопросы.
    const FAQ_MARKERS: &[&str] = &[
        "вопрос — ответ", "вопрос-ответ", "вопрос - ответ", "вопрос – ответ", "частые вопросы", "часто задаваемые", "пытанні і адказы",
        "frequently asked", "questions fréquentes", "preguntas frecuentes", "häufige fragen", "domande frequenti", "veelgestelde vragen",
        "perguntas frequentes", "najczęściej zadawane", "pytania i odpowiedzi", "faq",
    ];
    let plain_faq: usize = v
        .pages
        .iter()
        .filter(|p| p.question_headings + p.details_blocks == 0)
        .filter(|p| {
            // Подпись раздела вне меню: «Вопрос-ответ» пунктом меню есть на каждой странице.
            let t = p.text_lower();
            FAQ_MARKERS.iter().any(|m| {
                let in_links = p.links.iter().filter(|l| l.text.to_lowercase().replace('ё', "е").contains(m)).count();
                t.matches(m).count() > in_links
            })
        })
        .map(|p| p.text.matches('?').count())
        .filter(|n| *n >= 2)
        .map(|n| n.min(10))
        .sum();
    let questions: usize =
        v.pages.iter().map(|p| p.question_headings + p.details_blocks).sum::<usize>() + plain_faq + if faq_schema { 5 } else { 0 };
    out.push(MetricResult::new("I09", match questions { 0 => Status::Fail, 1..=4 => Status::Warn, _ => Status::Pass }, format!("Вопросов-ответов на страницах: {questions}")).with_value(questions));
    let about_link = v.pages.iter().flat_map(|p| p.links.iter()).any(|l| crate::lexicon::is_about_link(&l.href, &l.text));
    let about_pages: Vec<&PageInfo> = v.pages.iter().skip(1).copied().filter(|p| crate::lexicon::is_about_link(&p.url, "")).collect();
    let facts = company_facts(&v.pages.iter().take(1).chain(about_pages.iter()).map(|p| p.text.as_str()).collect::<Vec<_>>().join(" "));
    out.push(if about_link || !about_pages.is_empty() {
        MetricResult::pass("I10", if facts.is_empty() { "Есть страница «О компании»".to_string() } else { format!("Есть страница «О компании»; факты: {}", facts.join(", ")) })
    } else if facts.len() >= 2 {
        MetricResult::pass("I10", format!("Рассказ о компании с фактами есть на главной: {}", facts.join(", ")))
    } else {
        MetricResult::warn("I10", "Страницы «О компании» не найдено")
    });
    let articles: Vec<_> = v
        .snap
        .pages
        .iter()
        .filter(|p| crate::lexicon::BLOG.iter().any(|m| p.final_url.to_lowercase().contains(m)))
        .filter_map(|p| p.info.as_ref())
        .collect();
    if articles.is_empty() {
        out.push(MetricResult::na("I11", "Статей среди обойдённых страниц нет"));
    } else {
        let with = articles.iter().filter(|a| a.article_meta).count();
        out.push(MetricResult::new("I11", if with * 2 >= articles.len() { Status::Pass } else { Status::Warn }, format!("Автор/дата у {with} из {} статей", articles.len())));
    }
    let sem = &page.semantic;
    let kinds = [sem.main > 0, sem.header > 0, sem.footer > 0, sem.nav > 0].iter().filter(|x| **x).count();
    out.push(MetricResult::new("I12", match kinds { 0 => Status::Fail, 1 | 2 => Status::Warn, _ => Status::Pass }, format!("Семантических блоков (main/header/footer/nav): {kinds} из 4")).with_value(kinds));
    let same_as = v.pages.iter().any(|p| p.jsonld.iter().any(|j| j.contains("sameAs")));
    let card_links = v.pages.iter().flat_map(|p| p.links.iter()).any(|l| {
        let h = l.href.to_lowercase();
        // Свои адреса-переходы на соцсети: /social/instagram_by → 301 на instagram.com.
        let own_redirect = l.internal && {
            let path = url::Url::parse(&h).map(|u| u.path().to_string()).unwrap_or_default();
            path.contains("/social/") || ["instagram", "vkontakte", "facebook", "telegram", "viber", "youtube", "tiktok"].iter().any(|m| path.contains(m))
        };
        own_redirect || crate::lexicon::PROFILE_LINKS.iter().any(|m| h.contains(m))
    }) || v.pages.iter().flat_map(|p| p.iframes.iter()).any(|src| {
        // Виджет страницы в соцсети (facebook.com/plugins/page, likebox, vk.com/widget_community) — тоже связь.
        let s = src.to_lowercase();
        ["facebook.com/plugins", "vk.com/widget", "instagram.com/embed", "ok.ru/dk?st.cmd=widget", "youtube.com/embed"].iter().any(|m| s.contains(m))
    });
    out.push(match (same_as, card_links) {
        (true, _) => MetricResult::pass("I13", "sameAs в разметке связывает сайт с профилями"),
        (false, true) => MetricResult::warn("I13", "Ссылки на профили есть, но без sameAs в разметке"),
        (false, false) => MetricResult::fail("I13", "Сайт не связан с карточками и соцсетями"),
    });
    out.push(MetricResult::info("I14", match v.snap.llms_txt {
        Some(200) => "llms.txt есть (на Google не влияет)".to_string(),
        Some(st) if st >= 500 => format!("llms.txt отвечает {st}"),
        _ => "llms.txt нет (на Google не влияет)".to_string(),
    }));
    super::ai::run(ctx, v, prices, out);
}

/// Проверяемые факты о компании в тексте: год основания или опыт, лицензия с номером, команда.
pub(crate) fn company_facts(text: &str) -> Vec<&'static str> {
    use std::sync::OnceLock;
    static YEAR: OnceLock<regex::Regex> = OnceLock::new();
    static LIC: OnceLock<regex::Regex> = OnceLock::new();
    static TEAM: OnceLock<regex::Regex> = OnceLock::new();
    let year = YEAR.get_or_init(|| {
        regex::Regex::new(r"(?i)(?:с|since|seit|depuis|desde|dal|od|sinds|founded|established|основан\w*|gegründet|fondée?|fundad[oa]|fondat[oa]|założon\w*|opgericht)\s*(?:en\s+|in\s+|в\s*)?(?:19|20)\d{2}|(?:более|свыше|over|more than|mehr als|plus de|más de|più di|ponad|meer dan)\s*\d{1,2}\s*(?:лет|года|years|jahre|ans|años|anni|lat|jaar|anos)|\d{1,2}\s*(?:лет|years|jahre|ans|años|anni|lat|jaar|anos)\s*(?:на рынке|опыта|of experience|erfahrung|d.expérience|de experiencia|di esperienza|doświadczenia|ervaring|de experiência)").unwrap()
    });
    let lic = LIC.get_or_init(|| {
        regex::Regex::new(r"(?i)(?:лицензи\w*|свидетельств\w*|license|licence|licen[cs]e no|lizenz|licencia|licenza|licença|licentie|licencja|унп|огрн|инн|vat|siret|nip|ust-id|cif|p\.?\s?iva)\s*(?:№|no\.?|n°|nº|nr\.?|num\.?)?\s*[:#]?\s*[a-zа-я]{0,3}[\s-]?\d{4,}").unwrap()
    });
    let team = TEAM.get_or_init(|| {
        regex::Regex::new(r"(?i)\b(?:наша команда|наши (?:врачи|специалисты|мастера)|врачи клиники|our (?:team|doctors|dentists|staff)|meet the team|unser team|unsere (?:ärzte|zahnärzte)|notre équipe|nuestro equipo|il nostro team|nasz zespół|ons team|nossa equipe|nossa equipa)\b").unwrap()
    });
    let mut out = vec![];
    if year.is_match(text) {
        out.push("год или опыт");
    }
    if lic.is_match(text) {
        out.push("лицензия или реквизиты");
    }
    if team.is_match(text) {
        out.push("команда");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_matching() {
        let text = "наш адрес: ул. льва толстого, д. 5, москва";
        assert!(address_matches("Москва, улица Льва Толстого, 5", text));
        assert!(!address_matches("Москва, Тверская улица, 12", text));
        assert!(address_matches("624 Nostrand Avenue, Brooklyn", "visit us at 624 nostrand ave, brooklyn, ny"));
        assert!(address_matches("Nostrand Avenue, 624", "624 nostrand ave."));
        assert!(!address_matches("624 Nostrand Avenue, Brooklyn", "visit us at 62 nostrand ave"));
        assert!(address_matches("Berlin, Oranienstraße, 12", "praxis · oranienstr. 12 · 10999 berlin"));
        assert!(address_matches("12 Rue de Rivoli, Paris", "12 rue de rivoli, 75004 paris"));
        // Карточка по-белорусски, сайт по-русски.
        assert!(address_matches("Мінск, праспект Незалежнасці, 22", "минск, пр. независимости, 22, 2 этаж"));
        assert!(address_matches("Мінск, вуліца Леніна 8", "г. минск, ул. ленина, 8"));
        assert!(address_matches("вуліца Мяснікова, 29", "ул. мясникова 29"));
        assert!(!address_matches("Мінск, вуліца Леніна 8", "г. минск, ул. ленина, 18"));
        // Дом с латинской буквой на сайте.
        assert!(address_matches("Мінск, вуліца Сурганава, 57Б", "ул. сурганова 57b"));
        assert!(address_matches("Мінск, вуліца Кірава, 15", "г. минск, ул.кирова,15"), "аканье");
        assert!(address_matches("Москва, проспект Мира, 5", "пр-т мира, 5"), "служебные слова после о→а");
        assert!(address_matches("Minsk, vulica Uĺjanaŭskaja 29", "минск, ул. ульяновская, 29"), "белорусская латиница");
        // Без словаря — по согласным (belodent.by: «Прытыцкага» ↔ «Притыцкого»).
        assert!(address_matches("Мінск, вуліца Прытыцкага, 39", "минск, ул. притыцкого, 39, 150 м от ст. м. спортивная"));
        assert!(address_matches("Мінск, вуліца Дзяржынскага, 5", "г. минск, ул. дзержинского 5"));
        assert!(address_matches("Мінск, вуліца Будзённага, 12", "ул. буденного, д. 12"));
        assert!(address_matches("Мінск, вуліца Шчорса, 9", "ул. щорса 9"));
        assert!(address_matches("Мінск, вуліца Цімірязева, 67", "ул. тимирязева, 67"));
        // Русская транслитерация в карточке Overture (stomatologi.by).
        assert!(address_matches("Ulitsa L'va Sapegi 9", "минск, ул. льва сапеги, дом 9"));
        assert!(address_matches("Prospekt Pobediteley 119", "пр. победителей, 119, пом. 496"));
        assert!(!address_matches("Ulitsa L'va Sapegi 9", "ул. льва сапеги, дом 19"));
        // Номер дома должен стоять при улице, и номер чужой — не совпадение.
        assert!(!address_matches("Мінск, вуліца Прытыцкага, 39", "ул. притыцкого, 139"));
        assert!(!address_matches("Мінск, вуліца Прытыцкага, 39", "ул. притыцкого, 62. работаем с 9 до 21, тел. 39-00-00 и 39"));
        // Название компании на сайте записано иначе — это всё равно её сайт.
        assert!(name_on_site("Рэдакцыя радыёпраграмы «Рускае радыё»", "русское радио минск", None, "rusradio"));
        assert!(name_on_site("Місс Лі", "салон miss li", None, "missli"));
        assert!(name_on_site("ConcertHall", "concert hall group афиша", None, "concerthall"));
        assert!(name_on_site("Белпошта", "белпочта отделения", None, "belpost"));
        assert!(name_on_site("RESPAWN", "главная", None, "respawn"));
        assert!(!name_on_site("Респектор Тревел", "ставки и слоты в кыргызстане", None, "spinbetter"));
        // Общее слово названия на чужом сайте отрасли — не свой сайт (скан 18).
        assert!(!name_on_site("Мои Любимые Туры", "новости туризма беларуси: горящие туры в египет", None, "travelnews"));
        assert!(!name_on_site("La Crête D'Or (Золотой Гребешок)", "японские обои в минске", None, "zolotoi"));
        assert!(name_on_site("Клиника \"Стоматология Васильевых\"", "стоматология васильевых — лечение зубов", None, "stomatology"));
        assert!(!name_on_site("Бар Персонажи", "kalyan.by выставлен на продажу", None, "kalyan"));
    }

    #[test]
    fn price_and_address_patterns() {
        assert!(price_re().is_match("Лечение от 3 500 ₽"));
        assert!(price_re().is_match("стоимость 1200 руб."));
        assert!(!price_re().is_match("работаем с 2010 года"));
        assert!(address_re().is_match("пр-т Мира, д. 3"));
    }
}

