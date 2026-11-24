//! A. Карточки на картах.

use std::collections::BTreeSet;

use super::{fmt_num, CheckCtx};
use crate::model::{MetricResult, ProviderKind};
use crate::text::{classify_website, normalize_name, phone_key, registrable_domain, similarity, SiteKind};

/// Признаки в адресе страницы конкретной точки на сайте сети или производителя.
const BRAND_PAGE_MARKERS: &[&str] = &[
    "dealer", "locator", "storedetail", "store-detail", "/stores/", "/store/", "/shops/", "/boutique", "/locations/",
    "/branches/", "/filial", "/magaziny", "/salony", "/apteki", "/restaurants/", "/магазины", "/филиал", "/magasins",
    "/magasin/", "concession", "/tiendas", "/tienda/", "concesionario", "/filialen", "/filiale/", "haendler", "händler",
    "/negozi", "/negozio/", "rivenditor", "/sklepy", "/winkels", "/lojas", "/store-locator", "/find-a-store", "/standorte",
    "/standort/", "/agences", "/agence/", "/oficinas",
];

/// Ссылка из карточки ведёт на страницу точки на сайте бренда (дилер, магазин сети), а не на
/// свой сайт компании. Возвращает домен бренда.
/// `locales` — сколько языковых и страновых версий объявлено на главной (hreflang): у
/// международного сайта бренда (samsung.com, hugoboss.com) их десятки.
pub fn brand_page(card: &str, final_url: Option<&str>, city_names: &[String], locales: usize) -> Option<String> {
    let card_u = crate::text::parse_url_lenient(card)?;
    let final_u = final_url.and_then(crate::text::parse_url_lenient);
    let marked = |u: &url::Url| {
        let p = format!("{}?{}", u.path(), u.query().unwrap_or("")).to_lowercase();
        BRAND_PAGE_MARKERS.iter().any(|m| p.contains(m))
    };
    let card_host = crate::text::host_of(&card_u);
    let final_host = final_u.as_ref().map(crate::text::host_of).unwrap_or_else(|| card_host.clone());
    // minsk.brand.com → www.brand.com/…: поддомен-город уводит на общий сайт бренда.
    let first = card_host.trim_start_matches("www.").split('.').next().unwrap_or("").to_lowercase();
    let city_sub = card_host.split('.').count() >= 3
        && final_host.trim_start_matches("www.") != card_host.trim_start_matches("www.")
        && registrable_domain(&final_host) == registrable_domain(&card_host)
        && city_names.iter().any(|c| crate::discover::translit_city(c) == first);
    (marked(&card_u) || final_u.as_ref().is_some_and(marked) || city_sub || locales >= 15).then(|| registrable_domain(&final_host))
}

pub fn run(ctx: &CheckCtx, out: &mut Vec<MetricResult>) {
    let b = ctx.business;

    // A01: присутствие во всех выбранных источниках.
    let selected: BTreeSet<ProviderKind> =
        // Overture — набор данных, а не карта, на которой клиенты ищут компанию.
        ctx.params.providers.iter().copied().filter(|p| !matches!(p, ProviderKind::Manual | ProviderKind::Overture)).collect();
    let found: BTreeSet<ProviderKind> = b.sources.iter().filter_map(|s| s.provider).filter(|p| *p != ProviderKind::Overture).collect();
    let missing: Vec<&str> = selected.difference(&found).map(|p| p.label()).collect();
    let found_l: Vec<&str> = found.iter().map(|p| p.label()).collect();
    out.push(
        if missing.is_empty() {
            MetricResult::pass("A01", format!("Найдена в: {}", found_l.join(", ")))
        } else {
            MetricResult::warn("A01", format!("Нет в: {}. Найдена в: {}", missing.join(", "), found_l.join(", ")))
        }
        .with_value(found.len() as u64),
    );

    // A03, A04: сайт в карточке и его тип. Сайт, найденный в обход карт, упоминается в пояснении.
    let found = ctx.discovery.and_then(|d| d.found.as_ref());
    let found_note = found.map(|v| format!(" — а сайт есть: {} ({})", v.domain, v.evidence.first().cloned().unwrap_or_else(|| v.source.label().to_string())));
    match b.website.as_deref() {
        None => {
            out.push(
                MetricResult::fail("A03", format!("В карточке нет сайта{}", found_note.clone().unwrap_or_default()))
                    .with_value(found.map(|v| serde_json::json!({"found": v.domain})).unwrap_or(serde_json::Value::Null)),
            );
            out.push(MetricResult::na("A04", "Сайта в карточке нет"));
        }
        Some(w) => {
            out.push(MetricResult::pass("A03", w.to_string()));
            let (kind, host) = classify_website(w);
            let detail = format!("{}: {host}{}", kind.label(), if kind.is_auditable() { String::new() } else { found_note.clone().unwrap_or_default() });
            let final_url = ctx.site.and_then(|s| s.home.as_ref()).map(|h| h.final_url.as_str());
            let locales = ctx.site.and_then(|s| s.page.as_ref()).map(|p| p.hreflang.len()).unwrap_or(0);
            let brand = (kind == SiteKind::Own).then(|| brand_page(w, final_url, &ctx.params.city_names, locales)).flatten();
            out.push(
                match kind {
                    // Не свой сайт компании, а страница точки на сайте бренда: им занимается головная компания.
                    _ if brand.is_some() => {
                        MetricResult::pass("A04", format!("Страница точки на сайте бренда: {}", brand.clone().unwrap_or_default())).with_value("brand_page")
                    }
                    SiteKind::Own => MetricResult::pass("A04", detail).with_value(serde_json::to_value(kind).unwrap_or_default()),
                    SiteKind::ConstructorSubdomain => MetricResult::warn("A04", detail).with_value(serde_json::to_value(kind).unwrap_or_default()),
                    _ => MetricResult::fail("A04", detail).with_value(serde_json::to_value(kind).unwrap_or_default()),
                },
            );
        }
    }

    // A05, A06: рейтинг и отзывы против медианы конкурентов скана.
    match (b.rating(), ctx.market.rating_median) {
        (Some(r), Some(m)) => {
            let d = format!("{} при медиане конкурентов {}", fmt_num(r), fmt_num(m));
            out.push(super::grade_high("A05", r, m, m - 0.3, d));
        }
        (Some(r), None) => out.push(MetricResult::info("A05", format!("Рейтинг {}", fmt_num(r))).with_value(r)),
        _ => out.push(MetricResult::na("A05", "Рейтинг недоступен: его отдаёт только Google")),
    }
    match (b.reviews(), ctx.market.reviews_median) {
        (Some(n), Some(m)) => {
            let d = format!("{n} отзывов при медиане конкурентов {}", fmt_num(m));
            out.push(super::grade_high("A06", n as f64, m, m / 2.0, d));
        }
        (Some(n), None) => out.push(MetricResult::info("A06", format!("{n} отзывов")).with_value(n)),
        _ => out.push(MetricResult::na("A06", "Число отзывов недоступно: его отдаёт только Google")),
    }

    // A12: часы работы. В Overture часов нет вовсе, в OSM их заполняют волонтёры — отсутствие
    // там ничего не говорит о карточках Google и Яндекса, которыми управляет сам бизнес.
    let managed = b.sources.iter().any(|s| matches!(s.provider, Some(ProviderKind::Google | ProviderKind::Yandex)));
    let osm = b.sources.iter().any(|s| s.provider == Some(ProviderKind::Osm));
    out.push(match b.sources.iter().find_map(|s| s.hours.as_deref()) {
        Some(h) => MetricResult::pass("A12", crate::text::truncate(h, 160)),
        None if managed => MetricResult::fail("A12", "Часы работы в карточке не заполнены"),
        None if osm => MetricResult::warn("A12", "В OpenStreetMap часов нет; карточки Google и Яндекса не проверены"),
        None => MetricResult::na("A12", "В источнике нет часов работы — проверьте карточки Google и Яндекса"),
    });

    // A18: согласованность данных между картами. Overture — набор данных, а не карта: его
    // расхождения с OSM (белорусское и русское название, старый телефон) не ошибка компании.
    let providers: Vec<_> = b.sources.iter().filter(|s| !matches!(s.provider, Some(ProviderKind::Manual | ProviderKind::Overture))).collect();
    let distinct: BTreeSet<_> = providers.iter().filter_map(|s| s.provider).collect();
    if distinct.len() < 2 {
        out.push(MetricResult::na("A18", "Карточка найдена только в одном источнике"));
    } else {
        let mut issues = Vec::new();
        let phones: BTreeSet<String> = providers.iter().filter_map(|s| s.phone.as_deref().and_then(phone_key)).collect();
        if phones.len() > 1 {
            issues.push(format!("разные телефоны ({})", phones.len()));
        }
        let sites: BTreeSet<String> = providers
            .iter()
            .filter_map(|s| s.website.as_deref())
            .map(|w| registrable_domain(&classify_website(w).1))
            .collect();
        if sites.len() > 1 {
            issues.push(format!("разные сайты: {}", sites.into_iter().collect::<Vec<_>>().join(", ")));
        }
        let names: Vec<String> = providers.iter().map(|s| normalize_name(&s.name)).collect();
        if names.iter().any(|n| similarity(n, &names[0]) < 0.6) {
            issues.push("заметно разные названия".into());
        }
        let missing_site = providers.iter().any(|s| s.website.is_none()) && providers.iter().any(|s| s.website.is_some());
        if missing_site {
            issues.push("сайт указан не везде".into());
        }
        out.push(match issues.len() {
            0 => MetricResult::pass("A18", "Телефон, сайт и название совпадают"),
            1 => MetricResult::warn("A18", issues.join("; ")),
            _ => MetricResult::fail("A18", issues.join("; ")),
        });
    }

    // A19: дубли карточек — только в Google и Яндексе (см. merge).
    out.push(match b.duplicates {
        _ if !managed => MetricResult::na("A19", "Дубли проверяются по карточкам Google и Яндекса"),
        0 => MetricResult::pass("A19", "Дублей не найдено").with_value(0),
        n => MetricResult::fail("A19", format!("Найдено дублей: {n}")).with_value(n),
    });

    // A21: статус «закрыто».
    out.push(match b.sources.iter().find_map(|s| s.business_status.as_deref()) {
        Some("OPERATIONAL") => MetricResult::pass("A21", "Работает"),
        Some("CLOSED_TEMPORARILY") => MetricResult::warn("A21", "Помечена как временно закрытая").with_value("CLOSED_TEMPORARILY"),
        Some("CLOSED_PERMANENTLY") => {
            MetricResult::fail("A21", "Помечена как закрытая навсегда — проверить, правда ли").with_value("CLOSED_PERMANENTLY")
        }
        Some(other) => MetricResult::info("A21", other.to_string()),
        None => MetricResult::na("A21", "Статус отдаёт только Google"),
    });

    // A24: UTM в ссылке на сайт.
    if let Some(w) = b.website.as_deref() {
        out.push(if w.contains("utm_") {
            MetricResult::pass("A24", "Ссылка размечена UTM")
        } else {
            MetricResult::warn("A24", "Без UTM: переходы с карт не отличить от прочего трафика")
        });
    }
}

#[cfg(test)]
mod brand_tests {
    use super::brand_page;

    #[test]
    fn brand_store_pages() {
        let minsk = vec!["Мінск".to_string(), "Minsk".to_string(), "Минск".to_string()];
        assert_eq!(
            brand_page("http://www.minsk.bentleymotors.com/", Some("https://www.bentleymotors.com/en/apps/dealer-locator.html/region/Europe"), &minsk, 0).as_deref(),
            Some("bentleymotors.com")
        );
        assert_eq!(brand_page("https://www.hugoboss.com/storedetail?storeid=1032344", None, &minsk, 0).as_deref(), Some("hugoboss.com"));
        assert_eq!(brand_page("https://www.ovsfashion.com/en/es/stores/belarus/minsk-prosp-dzerzhinsky-104", None, &minsk, 0).as_deref(), Some("ovsfashion.com"));
        // Свой сайт салона — не бренд, даже с поддоменом.
        assert_eq!(brand_page("https://salon-lili.by/", Some("https://salon-lili.by/"), &minsk, 0), None);
        assert_eq!(brand_page("https://booking.salon-lili.by/", Some("https://salon-lili.by/"), &minsk, 0), None);
        // Поддомен-город, уводящий на общий сайт сети, — страница точки.
        // Международный сайт с десятками языковых версий — сайт бренда.
        assert_eq!(brand_page("https://www.samsung.com/by/", Some("https://www.samsung.com/jp/"), &minsk, 60).as_deref(), Some("samsung.com"));
        assert_eq!(brand_page("https://minsk.dentalclinic.ru/", Some("https://dentalclinic.ru/contacts/"), &minsk, 0).as_deref(), Some("dentalclinic.ru"));
    }
}
