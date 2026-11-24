//! Скоринг: баллы категорий, общий балл, стоп-факторы и класс лида с причинами.
//! Правила описаны в docs/METRICS.md, разделы «Скоринг» и «Отбор лидов».

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use crate::catalog::catalog;
use crate::discover::Discovery;
use crate::model::{Business, MetricResult, Status};
use crate::text::classify_website;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeadClass {
    /// Нет сайта, сайт мёртв или вместо сайта соцсеть.
    Hot,
    /// Сайт есть, но слабый или с критичными ошибками.
    Warm,
    /// Сайт в порядке.
    Cold,
    /// Карточка помечена закрытой.
    Closed,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CategoryScore {
    pub score: Option<f64>,
    pub pass: usize,
    pub warn: usize,
    pub fail: usize,
    pub measured: usize,
    pub total: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Counts {
    pub pass: usize,
    pub warn: usize,
    pub fail: usize,
    pub info: usize,
    pub not_measured: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub overall: Option<f64>,
    pub categories: BTreeMap<String, CategoryScore>,
    pub stop_factors: Vec<String>,
    pub lead: LeadClass,
    pub lead_reasons: Vec<String>,
    /// Чем больше, тем выше в списке лидов.
    pub priority: f64,
    pub counts: Counts,
    /// Ситуация с сайтом для таблицы, сводки по району и письма клиенту.
    #[serde(default)]
    pub mini: crate::mini::MiniReport,
}

/// С этого числа точек с одним названием в скане компания считается сетью.
pub const CHAIN_MIN: u32 = 5;

/// Балл при сработавшем стоп-факторе не выше этого значения.
pub const STOP_CAP: f64 = 25.0;

/// Короткая формулировка стоп-фактора для списка причин.
pub fn stop_label(id: &str) -> &'static str {
    match id {
        "A21" => "карточка помечена закрытой",
        "B01" => "сайт не работает",
        "B13" => "сайт в списках опасных",
        "C02" => "robots.txt закрывает сайт от поиска",
        "C10" => "noindex на главной",
        "E01" => "нет мобильной версии",
        "I01" => "Google не может цитировать сайт",
        "I05" => "защита сайта не пускает ИИ-ботов",
        _ => "критичная ошибка",
    }
}

fn points(s: Status) -> Option<f64> {
    match s {
        Status::Pass => Some(100.0),
        Status::Warn => Some(50.0),
        Status::Fail => Some(0.0),
        _ => None,
    }
}

pub fn score(results: &[MetricResult], business: &Business, discovery: Option<&Discovery>) -> Report {
    let cat = catalog();
    let by_id: HashMap<&str, &MetricResult> = results.iter().map(|r| (r.id.as_str(), r)).collect();
    let mut categories: BTreeMap<String, CategoryScore> = cat.categories.keys().map(|k| (k.clone(), CategoryScore::default())).collect();
    let mut sums: HashMap<String, (f64, f64)> = HashMap::new();
    let mut counts = Counts::default();
    let mut stop_factors = Vec::new();

    for r in results {
        match r.status {
            Status::Pass => counts.pass += 1,
            Status::Warn => counts.warn += 1,
            Status::Fail => counts.fail += 1,
            Status::Info => counts.info += 1,
            Status::NotMeasured => counts.not_measured += 1,
        }
    }
    for def in &cat.metrics {
        let c = categories.entry(def.category.clone()).or_default();
        c.total += 1;
        let Some(r) = by_id.get(def.id.as_str()) else { continue };
        match r.status {
            Status::Pass => c.pass += 1,
            Status::Warn => c.warn += 1,
            Status::Fail => c.fail += 1,
            _ => {}
        }
        if def.kind != "check" || def.weight <= 0.0 {
            continue;
        }
        if let Some(p) = points(r.status) {
            c.measured += 1;
            let e = sums.entry(def.category.clone()).or_default();
            e.0 += def.weight * p;
            e.1 += def.weight;
        }
        if def.stop_factor && r.status == Status::Fail {
            stop_factors.push(def.id.clone());
        }
    }
    for (k, (num, den)) in &sums {
        if *den > 0.0
            && let Some(c) = categories.get_mut(k)
        {
            c.score = Some((num / den * 10.0).round() / 10.0);
        }
    }
    let (mut num, mut den) = (0.0, 0.0);
    for (k, c) in &categories {
        if let (Some(s), Some(w)) = (c.score, cat.category_weights.get(k)) {
            num += s * w;
            den += w;
        }
    }
    // Без проверенного сайта общего балла нет: одна карточка на карте — не оценка сайта.
    let site_measured = categories.iter().any(|(k, c)| k != "A" && c.score.is_some());
    let mut overall = (den > 0.0 && site_measured).then(|| (num / den).round());
    if !stop_factors.is_empty() {
        overall = overall.map(|o| o.min(STOP_CAP));
    }

    let st = |id: &str| by_id.get(id).map(|r| r.status);
    let detail = |id: &str| by_id.get(id).map(|r| r.detail.clone()).unwrap_or_default();
    let mut hot = Vec::new();
    let mut warm = Vec::new();
    let closed = by_id.get("A21").is_some_and(|r| r.value.as_ref().and_then(|v| v.as_str()) == Some("CLOSED_PERMANENTLY"));
    let found = discovery.and_then(|d| d.found.as_ref());
    let suggestion = discovery.and_then(|d| d.suggestions.first());
    let card_link = business.website.as_deref().map(|w| classify_website(w).1).unwrap_or_default();
    match found {
        // Сайт есть, но в карточку не попал: это не «сделать сайт», а связать карточку и сайт.
        Some(v) if st("A03") == Some(Status::Fail) => warm.push(format!("Сайт {} есть, но не указан в карточке — с карт на него не попасть", v.domain)),
        Some(v) if st("A04") == Some(Status::Fail) => {
            warm.push(format!("В карточке — {card_link}, а свой сайт {} не указан", v.domain))
        }
        _ => {
            if st("A03") == Some(Status::Fail) {
                hot.push("Нет сайта в карточке".to_string());
            }
            if st("A04") == Some(Status::Fail) {
                hot.push(format!("Вместо сайта — {}", detail("A04")));
            }
            if let Some(s) = suggestion
                && !hot.is_empty()
            {
                hot.push(format!("Возможно, сайт есть: {} — проверьте", s.domain));
            }
        }
    }
    if st("B01") == Some(Status::Fail) {
        hot.push(format!("Сайт не работает: {}", detail("B01")));
    }
    // По ссылке из карточки — сайт без названия и контактов компании: чужой или проданный домен.
    if by_id.get("H02").and_then(|r| r.value.as_ref()).and_then(|v| v.as_str()) == Some("foreign") {
        hot.push("В карточке ссылка на сайт без названия и контактов компании — похоже, чужой или проданный домен".to_string());
    }
    // Сайта по сути нет, только если на главной почти один шаблон; шаблонная фраза на рабочем
    // сайте (на главной или внутренней странице) — недоделка, тёплый лид.
    match (st("F14"), by_id.get("F14").and_then(|r| r.value.as_ref()).and_then(|v| v.as_str())) {
        (Some(Status::Fail), Some("home")) => hot.push("На главной шаблонный текст или заглушка — сайта по сути нет".to_string()),
        (Some(Status::Fail | Status::Warn), Some("inner" | "leftover")) => warm.push(detail("F14")),
        _ => {}
    }
    if st("A04") == Some(Status::Warn) {
        warm.push(format!("Сайт на конструкторе: {}", detail("A04")));
    }
    if let Some(o) = overall
        && o < 50.0
    {
        warm.push(format!("Слабый сайт: {o} из 100"));
    }
    let stop_names: Vec<&str> = stop_factors.iter().filter(|id| *id != "B01").map(|id| stop_label(id)).collect();
    if !stop_names.is_empty() {
        warm.push(format!("Критично: {}", stop_names.join("; ")));
    }
    let b01_blocked = by_id.get("B01").is_some_and(|r| r.status == Status::Warn && r.value.as_ref().and_then(|v| v.as_str()) == Some("blocked"));
    if b01_blocked {
        // Сайт есть, но мы его не видели: ни балла, ни повода считать лид тёплым.
        overall = None;
    }
    // Старый год в подвале хорошего сайта — косметика, а не повод для продажи.
    if st("F13") == Some(Status::Fail) && overall.is_none_or(|o| o < 75.0) {
        warm.push(format!("Сайт заброшен: {}", detail("F13")));
    }
    if by_id.get("B01").is_some_and(|r| r.value.as_ref().and_then(|v| v.as_str()) == Some("broken_link")) {
        warm.push(format!("{} — клиенты с карт попадают на ошибку", detail("B01").split(';').next().unwrap_or("Битая ссылка в карточке")));
    }
    if st("I06") == Some(Status::Fail) {
        warm.push("Сайт рисуется скриптами: ChatGPT, Perplexity и другие ИИ-ассистенты видят пустую страницу".to_string());
    }
    if st("B04") == Some(Status::Fail) {
        warm.push(format!("{} — браузер пишет «Не защищено»", detail("B04")));
    } else if by_id.get("B05").is_some_and(|r| r.value.as_ref().and_then(|v| v.as_str()) == Some("no_redirect")) {
        warm.push("HTTPS есть, но нет перенаправления с http:// — по ссылке из карточки браузер пишет «Не защищено»".to_string());
    } else if by_id.get("B05").is_some_and(|r| r.value.as_ref().and_then(|v| v.as_str()) == Some("no_redirect_hsts")) {
        warm.push("Сервер не перенаправляет http:// на https:// — новый посетитель по ссылке из карточки видит «Не защищено» (заходившим раньше браузер переключает сам)".to_string());
    }

    // Точка сети с брендом в Wikidata: сайт и маркетинг у головной компании.
    // Ссылка из карточки ведёт на страницу точки на сайте бренда (дилер, магазин сети).
    let brand_page = by_id
        .get("A04")
        .filter(|r| r.value.as_ref().and_then(|v| v.as_str()) == Some("brand_page"))
        .map(|r| r.detail.rsplit(": ").next().unwrap_or_default().to_string());
    // Или много точек с одним названием в районе.
    let foreign_site = by_id.get("H02").is_some_and(|r| r.value.as_ref().and_then(|v| v.as_str()) == Some("foreign"));
    let chain = brand_page.clone().or_else(|| business
        .brand()
        .filter(|_| business.wikidata().is_some() || discovery.is_some_and(|d| d.brand.is_some() && d.found.as_ref().is_some_and(|f| f.source == crate::discover::Source::Wikidata)))
        .map(str::to_string)
        .or_else(|| (business.chain_size >= CHAIN_MIN).then(|| business.name.clone()))
        // Сайт сети из другой страны (телефоны только +7 у минской точки).
        .or_else(|| by_id.get("H02").filter(|r| r.value.as_ref().and_then(|v| v.as_str()) == Some("other_country")).map(|_| business.name.clone())))
        .filter(|_| !foreign_site);
    let (lead, mut reasons) = if closed {
        (LeadClass::Closed, vec!["Карточка помечена закрытой навсегда".to_string()])
    } else if let Some(domain) = &brand_page {
        (LeadClass::Cold, vec![format!("Страница точки на сайте бренда {domain}: сайтом и маркетингом занимается головная компания")])
    } else if let Some(brand) = &chain {
        let n = if business.chain_size >= CHAIN_MIN { format!(" ({} точек в районе)", business.chain_size) } else { String::new() };
        (LeadClass::Cold, vec![format!("Точка сети «{brand}»{n}: сайтом и маркетингом занимается головная компания")])
    } else if !hot.is_empty() {
        (LeadClass::Hot, hot)
    } else if !warm.is_empty() && !b01_blocked {
        (LeadClass::Warm, warm)
    } else if b01_blocked {
        (LeadClass::Cold, vec!["Сайт не открылся сканеру — качество неизвестно, проверить вручную или без VPN".to_string()])
    } else {
        (LeadClass::Cold, vec![])
    };

    // Признаки живого и платёжеспособного бизнеса поднимают лид в списке.
    let reviews = business.reviews().unwrap_or(0) as f64;
    let rating = business.rating().unwrap_or(0.0);
    let pays = by_id.get("P05").and_then(|r| r.value.as_ref()).and_then(|v| v.as_bool()) == Some(true);
    let books = st("N04") == Some(Status::Pass);
    if st("A06") == Some(Status::Pass) && lead != LeadClass::Cold {
        reasons.push(format!("Живой бизнес: {} отзывов", reviews as u64));
    }
    if pays && lead != LeadClass::Cold {
        reasons.push("Стоит рекламный пиксель — похоже, вкладывается в рекламу".to_string());
    }
    let base = match lead {
        LeadClass::Hot => 300.0,
        LeadClass::Warm => 200.0,
        LeadClass::Cold => 100.0,
        LeadClass::Closed => 0.0,
    };
    let priority = base
        + (15.0 * (1.0 + reviews).log10()).min(40.0)
        + ((rating - 4.0) * 10.0).clamp(0.0, 10.0)
        + if pays { 15.0 } else { 0.0 }
        + if books { 5.0 } else { 0.0 }
        + (100.0 - overall.unwrap_or(0.0)) * 0.3
        - if suggestion.is_some() && found.is_none() && lead == LeadClass::Hot { 20.0 } else { 0.0 };

    let mini = crate::mini::build(results, &crate::mini::Context { overall, closed, chain, discovery });
    Report { overall, categories, stop_factors, lead, lead_reasons: reasons, priority: (priority * 10.0).round() / 10.0, counts, mini }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Place, ProviderKind};

    fn results(pairs: &[(&str, Status)]) -> Vec<MetricResult> {
        pairs.iter().map(|(id, s)| MetricResult::new(id, *s, "")).collect()
    }

    fn biz(reviews: Option<u32>) -> Business {
        Business { sources: vec![Place { provider: Some(ProviderKind::Google), reviews, rating: Some(4.8), ..Default::default() }], ..Default::default() }
    }

    #[test]
    fn no_site_is_hot() {
        let r = score(&results(&[("A03", Status::Fail), ("A06", Status::Pass)]), &biz(Some(250)), None);
        assert_eq!(r.lead, LeadClass::Hot);
        assert!(r.lead_reasons[0].contains("Нет сайта"));
        assert!(r.lead_reasons.iter().any(|x| x.contains("Живой бизнес")));
        assert!(r.priority > 300.0);
    }

    #[test]
    fn found_site_turns_hot_into_warm() {
        use crate::discover::{Discovery, Source, Verdict};
        let d = Discovery {
            found: Some(Verdict {
                url: "https://gigiglambox.com/".into(),
                domain: "gigiglambox.com".into(),
                source: Source::DomainGuess,
                via: String::new(),
                score: 1.0,
                confirmed: true,
                evidence: vec![],
                title: None,
            }),
            ..Default::default()
        };
        let r = score(&results(&[("A03", Status::Fail), ("F01", Status::Pass)]), &biz(None), Some(&d));
        assert_eq!(r.lead, LeadClass::Warm);
        assert!(r.lead_reasons[0].contains("gigiglambox.com"));
    }

    #[test]
    fn stop_factor_caps_score() {
        let r = score(&results(&[("C10", Status::Fail), ("F01", Status::Pass), ("F03", Status::Pass), ("B08", Status::Pass)]), &biz(None), None);
        assert_eq!(r.stop_factors, vec!["C10".to_string()]);
        assert!(r.overall.unwrap() <= STOP_CAP);
        assert_eq!(r.lead, LeadClass::Warm);
    }

    #[test]
    fn good_site_is_cold() {
        let r = score(&results(&[("A03", Status::Pass), ("A04", Status::Pass), ("B01", Status::Pass), ("F01", Status::Pass)]), &biz(Some(10)), None);
        assert_eq!(r.lead, LeadClass::Cold);
        assert_eq!(r.overall, Some(100.0));
        assert_eq!(r.categories["F"].score, Some(100.0));
        assert_eq!(r.categories["F"].total, 19);
    }

    #[test]
    fn info_and_not_measured_do_not_score() {
        let r = score(&results(&[("F05", Status::Info), ("F09", Status::NotMeasured), ("F01", Status::Warn)]), &biz(None), None);
        assert_eq!(r.categories["F"].score, Some(50.0));
        assert_eq!(r.counts.info, 1);
        assert_eq!(r.counts.not_measured, 1);
    }
}
