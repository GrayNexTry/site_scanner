//! Сквозная проверка: синтетический сайт → все метрики каталога получают результат.

use super::*;
use crate::http::{Fetched, Hop};
use crate::model::{LatLon, Place};
use crate::page;
use crate::robots;
use crate::site::{BotProbe, RedirectProbe, SiteSnapshot, TextResource};

const HTML: &str = r#"<!doctype html><html lang="ru"><head>
<title>Стоматология в Хамовниках — Зуб-Мастер, Москва</title>
<meta name="description" content="Лечение и протезирование зубов в Хамовниках. Приём без очередей, гарантия на работы, рассрочка.">
<meta name="viewport" content="width=device-width, initial-scale=1">
<link rel="canonical" href="https://zub.ru/">
<script type="application/ld+json">{"@context":"https://schema.org","@type":"Dentist","name":"Зуб-Мастер","telephone":"+7 495 123-45-67","address":"Москва, ул. Льва Толстого, 5","url":"https://zub.ru/","sameAs":["https://yandex.ru/maps/org/1/"]}</script>
</head><body><header><a href="tel:+74951234567">+7 495 123-45-67</a></header>
<main><h1>Стоматология в Хамовниках</h1><p>Лечение кариеса от 3 500 ₽, имплантация от 25 000 ₽, чистка 4 000 ₽.</p>
<p>Адрес: ул. Льва Толстого, д. 5. Работаем 9:00–21:00. © 2019 Зуб-Мастер</p></main>
<footer><a href="/privacy">Политика конфиденциальности</a></footer></body></html>"#;

fn fetched(url: &str, body: &str) -> Fetched {
    Fetched {
        url: url.into(),
        final_url: url.into(),
        status: 200,
        chain: vec![Hop { url: "http://zub.ru/".into(), status: 301 }],
        headers: vec![("content-type".into(), "text/html".into())],
        body: body.into(),
        body_bytes: body.len(),
        truncated: false,
        ttfb_ms: 350,
        total_ms: 900,
        http_version: "HTTP/2.0".into(),
        content_type: "text/html".into(),
    }
}

fn business() -> Business {
    Business {
        name: "Зуб-Мастер".into(),
        address: Some("Москва, улица Льва Толстого, 5".into()),
        phone: Some("+7 (495) 123-45-67".into()),
        website: Some("https://zub.ru/".into()),
        sources: vec![Place { provider: Some(ProviderKind::Google), rating: Some(4.8), reviews: Some(120), business_status: Some("OPERATIONAL".into()), hours: Some("пн: 9–21".into()), ..Default::default() }],
        ..Default::default()
    }
}

fn params() -> ScanParams {
    ScanParams {
        query: "стоматология".into(),
        preset: None,
        city: Some("Хамовники".into()),
        center: LatLon { lat: 55.73, lon: 37.58 },
        radius_m: 1000.0,
        providers: vec![ProviderKind::Google],
        max_places: 10,
        quick: false,
        country: Some("ru".into()),
        city_names: vec![],
    }
}

#[test]
fn every_metric_gets_a_result_and_key_checks_fire() {
    let robots_body = "User-agent: *\nDisallow: /admin/\n\nUser-agent: YandexAdditional\nDisallow: /\n";
    let snap = SiteSnapshot {
        input_url: "https://zub.ru/".into(),
        home: Some(fetched("https://zub.ru/", HTML)),
        page: Some(page::extract(HTML, "https://zub.ru/")),
        robots: Some(TextResource { url: "https://zub.ru/robots.txt".into(), status: Some(200), body: robots_body.into(), bytes: robots_body.len(), error: None }),
        robots_directives: Some(robots::parse(robots_body)),
        http_probe: Some(RedirectProbe { url: "http://zub.ru/".into(), first_status: Some(301), final_url: Some("https://zub.ru/".into()), final_status: Some(200), hops: 1, error: None, hsts: false }),
        not_found: Some(RedirectProbe { url: "https://zub.ru/x".into(), first_status: Some(200), final_url: Some("https://zub.ru/x".into()), final_status: Some(200), hops: 0, error: None, hsts: false }),
        bots: vec![BotProbe { agent: "OAI-SearchBot".into(), status: Some(403), bytes: 10, challenge: false, error: None }],
        compression: Some("br".into()),
        ..Default::default()
    };
    let b = business();
    let p = params();
    let market = MarketStats { rating_median: Some(4.5), reviews_median: Some(80.0) };
    let results = run_all(&CheckCtx { business: &b, site: Some(&snap), params: &p, market: &market, discovery: None });

    assert_eq!(results.len(), catalog().metrics.len(), "каждая метрика должна получить результат");
    let st = |id: &str| results.iter().find(|r| r.id == id).unwrap().status;
    assert_eq!(st("A03"), Status::Pass);
    assert_eq!(st("A05"), Status::Pass);
    assert_eq!(st("B01"), Status::Pass);
    assert_eq!(st("B05"), Status::Pass);
    assert_eq!(st("B12"), Status::Fail, "soft 404");
    assert_eq!(st("C02"), Status::Pass);
    assert_eq!(st("C11"), Status::Pass);
    assert_eq!(st("F03"), Status::Pass);
    assert_eq!(st("F06"), Status::Pass);
    assert_eq!(st("F13"), Status::Fail, "© 2019 — заброшен");
    assert_eq!(st("G02"), Status::Warn, "нет geo и openingHours");
    assert_eq!(st("G05"), Status::Pass);
    assert_eq!(st("H02"), Status::Pass);
    assert_eq!(st("I03"), Status::Fail, "YandexAdditional закрыт");
    assert_eq!(st("I05"), Status::Warn, "WAF режет поддельного OAI-SearchBot — повод проверить, не стоп-фактор");
    assert_eq!(st("I08"), Status::Pass);
    assert_eq!(st("M01"), Status::Pass);
    assert_eq!(st("N02"), Status::Pass);
    assert_eq!(st("I16"), Status::NotMeasured, "переходы из ИИ — только с доступом к аналитике");
    assert_eq!(st("D01"), Status::NotMeasured);

    // Яндекс: ссылка на карточку в sameAs, городской номер, зона страны; ИКС не спрашивали.
    assert_eq!(st("R01"), Status::Pass);
    assert_eq!(st("R03"), Status::Pass);
    assert_eq!(st("R04"), Status::Pass);
    assert_eq!(st("R06"), Status::Fail, "только телефон");
    assert_eq!(st("R09"), Status::Warn, "три цены");
    assert_eq!(st("R12"), Status::Pass);
    assert_eq!(st("R13"), Status::Pass);
    assert_eq!(st("C19"), Status::NotMeasured);

    let report = crate::scoring::score(&results, &b, None);
    assert!(!report.stop_factors.contains(&"I05".to_string()), "отказ поддельному боту не стоп-фактор");
}

/// Сайт с нарушениями Яндекса: SEO-текст, скрытые ссылки, popunder, другое содержимое для робота.
#[test]
fn yandex_violations_and_signals() {
    let spam = "Стоматология в Минске недорого: стоматология в Минске для всей семьи. ".repeat(25);
    let story = "Мы лечим зубы бережно, объясняем каждый шаг лечения и подбираем план под бюджет пациента. ".repeat(10);
    let hidden = "лечение зубов минск имплантация минск протезирование минск ".repeat(8);
    let html = format!(
        r#"<!doctype html><html lang="ru"><head><title>Стоматология Минск — Зуб-Мастер</title>
<script src="https://cdn.propellerads.com/tag.js"></script></head><body>
<header><a href="tel:+375291234567">+375 29 123-45-67</a> <a href="/contacts/">Контакты</a></header>
<main><h1>Стоматология</h1><p>{spam}</p><p>{story}</p>
<p>Оплата наличными и банковской картой. Гарантия на пломбы 2 года.</p>
<div style="position:absolute; left:-9999px">{hidden}<a href="https://a.by/">1</a><a href="https://b.by/">2</a><a href="https://c.by/">3</a></div></main>
<footer><a href="https://yandex.by/maps/org/zub/123/">Мы на Яндекс Картах</a> <a href="https://vk.com/zubmaster">VK</a>
<a href="mailto:info@zub.ru">info@zub.ru</a></footer></body></html>"#
    );
    let snap = SiteSnapshot {
        input_url: "https://zub.ru/".into(),
        home: Some(fetched("https://zub.ru/", &html)),
        page: Some(page::extract(&html, "https://zub.ru/")),
        yandex_bot: Some(crate::site::BotView { status: Some(200), final_url: Some("https://zub.ru/".into()), title: Some("Купить диплом недорого".into()), words: 40, ..Default::default() }),
        mobile_probe: Some(RedirectProbe { url: "https://zub.ru/".into(), final_url: Some("https://m-dating.example/".into()), final_status: Some(200), ..Default::default() }),
        yandex: Some(crate::yandex_info::SiteInfo {
            host: "zub.ru".into(),
            sqi: Some(120),
            badges: vec![crate::yandex_info::Badge { kind: "GOODS".into(), value: serde_json::json!(true) }],
            error: None,
        }),
        yandex_safety: Some(Ok(vec!["malware".into()])),
        ..Default::default()
    };
    let b = business();
    let mut p = params();
    p.country = Some("by".into());
    let market = MarketStats::default();
    let results = run_all(&CheckCtx { business: &b, site: Some(&snap), params: &p, market: &market, discovery: None });
    let r = |id: &str| results.iter().find(|r| r.id == id).unwrap();
    assert_eq!(r("R01").status, Status::Pass, "{}", r("R01").detail);
    assert_eq!(r("R03").status, Status::Warn, ".ru у компании из Беларуси: {}", r("R03").detail);
    assert_eq!(r("R04").status, Status::Warn, "только мобильный: {}", r("R04").detail);
    assert_eq!(r("R05").status, Status::Pass);
    assert_eq!(r("R07").status, Status::Pass);
    assert_eq!(r("R08").status, Status::Pass);
    assert_eq!(r("R10").status, Status::Pass);
    assert_eq!(r("R11").status, Status::Fail, "{}", r("R11").detail);
    assert_eq!(r("R12").status, Status::Fail, "{}", r("R12").detail);
    assert_eq!(r("R13").status, Status::Fail, "{}", r("R13").detail);
    assert!(r("R13").detail.contains("m-dating.example"), "{}", r("R13").detail);
    assert_eq!(r("R14").status, Status::Warn, "{}", r("R14").detail);
    assert_eq!(r("B13").status, Status::Fail, "{}", r("B13").detail);
    assert_eq!(r("C19").value, Some(serde_json::json!(120)));
    assert_eq!(r("R16").detail, "Товары в поиске");
}

#[test]
fn blocked_site_is_not_reported_as_dead() {
    let mut home = fetched("https://zub.ru/", "<html><title>403 Forbidden.</title><b>Access denied</b></html>");
    home.status = 403;
    home.headers.push(("server".into(), "ddos-guard".into()));
    let snap = SiteSnapshot { input_url: "https://zub.ru/".into(), home: Some(home), ..Default::default() };
    let b = business();
    let p = params();
    let market = MarketStats::default();
    let results = run_all(&CheckCtx { business: &b, site: Some(&snap), params: &p, market: &market, discovery: None });
    let b01 = results.iter().find(|r| r.id == "B01").unwrap();
    assert_eq!(b01.status, Status::Warn);
    assert!(b01.detail.contains("ddos-guard"));
    assert!(results.iter().find(|r| r.id == "F01").unwrap().detail.contains("не пустил"));
    let report = crate::scoring::score(&results, &b, None);
    // Сайт не видели — качество неизвестно: не тёплый лид, а холодный с пометкой проверить.
    assert_eq!(report.lead, crate::scoring::LeadClass::Cold);
    assert!(report.overall.is_none());
    assert!(report.lead_reasons.iter().any(|r| r.contains("проверить вручную")));
}

#[test]
fn no_site_and_dead_site() {
    let mut b = business();
    b.website = None;
    let p = params();
    let market = MarketStats::default();
    let results = run_all(&CheckCtx { business: &b, site: None, params: &p, market: &market, discovery: None });
    assert_eq!(results.len(), catalog().metrics.len());
    assert_eq!(results.iter().find(|r| r.id == "A03").unwrap().status, Status::Fail);
    assert!(results.iter().find(|r| r.id == "F01").unwrap().detail.contains("Нет сайта"));

    let dead = SiteSnapshot { input_url: "https://zub.ru".into(), home_error: Some("домен не резолвится".into()), ..Default::default() };
    // Таймаут из-за границы — не смерть сайта, а «проверить без VPN».
    let far = SiteSnapshot { input_url: "https://nbrb.by".into(), home_error: Some("таймаут".into()), abroad: true, ..Default::default() };
    let mut out = Vec::new();
    super::tech::run_dead(&far, &mut out);
    assert_eq!(out[0].status, Status::Warn);
    assert!(out[0].detail.contains("без VPN"));
    let b = business();
    let results = run_all(&CheckCtx { business: &b, site: Some(&dead), params: &p, market: &market, discovery: None });
    assert_eq!(results.iter().find(|r| r.id == "B01").unwrap().status, Status::Fail);
    let report = crate::scoring::score(&results, &b, None);
    assert_eq!(report.lead, crate::scoring::LeadClass::Hot);
}

/// Шаблонный текст: в скрытом окне — не заглушка; на полноценной главной — недоделка (тёплый);
/// главная почти из одного шаблона — сайта по сути нет (горячий).
#[test]
fn placeholder_text_hidden_leftover_and_stub() {
    // Название компании на странице есть — иначе сработал бы флаг «чужой домен».
    let real: String = (0..80).map(|i| format!("<p>Зуб-Мастер открыт для посетителей, приём номер {i} проходит каждый день с утра.</p>")).collect();
    let f14 = |html: &str| {
        let snap = SiteSnapshot {
            input_url: "https://m.by/".into(),
            home: Some(fetched("https://m.by/", html)),
            page: Some(page::extract(html, "https://m.by/")),
            ..Default::default()
        };
        let (b, p, market) = (business(), params(), MarketStats::default());
        let results = run_all(&CheckCtx { business: &b, site: Some(&snap), params: &p, market: &market, discovery: None });
        let r = results.iter().find(|r| r.id == "F14").unwrap().clone();
        let report = crate::scoring::score(&results, &b, None);
        (r.status, r.value.and_then(|v| v.as_str().map(str::to_string)), report.lead)
    };
    let popup = "<div class=\"w-popup _js-pop-message\"><p>Lorem Ipsum is simply dummy text of the printing industry.</p></div>";
    let (st, _, _) = f14(&format!("<html><body>{real}{popup}</body></html>"));
    assert_eq!(st, Status::Pass, "шаблон только во всплывающем окне");
    let (st, v, lead) = f14(&format!("<html><body>{real}<p>Lorem ipsum dolor sit amet, ad mel ludus prodesset.</p></body></html>"));
    assert_eq!((st, v.as_deref()), (Status::Warn, Some("leftover")));
    assert_ne!(lead, crate::scoring::LeadClass::Hot, "рабочий сайт с недоделкой — не горячий лид");
    let (st, v, lead) = f14("<html><body><h1>Lorem ipsum</h1><p>Lorem ipsum dolor sit amet.</p></body></html>");
    assert_eq!((st, v.as_deref()), (Status::Fail, Some("home")));
    assert_eq!(lead, crate::scoring::LeadClass::Hot);
}

/// По ссылке из карточки — сайт без названия, телефона и адреса компании: чужой домен, горячий лид.
#[test]
fn foreign_domain_in_card() {
    let html = "<html><head><title>Лучшие онлайн-казино 2026</title></head><body><h1>Играйте онлайн</h1><p>Бонусы и фриспины для новых игроков каждый день.</p></body></html>";
    let snap = SiteSnapshot { input_url: "https://zub.ru/".into(), home: Some(fetched("https://zub.ru/", html)), page: Some(page::extract(html, "https://zub.ru/")), ..Default::default() };
    let (b, p, market) = (business(), params(), MarketStats::default());
    let results = run_all(&CheckCtx { business: &b, site: Some(&snap), params: &p, market: &market, discovery: None });
    let h02 = results.iter().find(|r| r.id == "H02").unwrap();
    assert_eq!(h02.value.as_ref().and_then(|v| v.as_str()), Some("foreign"), "{}", h02.detail);
    let report = crate::scoring::score(&results, &b, None);
    assert_eq!(report.lead, crate::scoring::LeadClass::Hot);
    assert_eq!(report.mini.state, crate::mini::SiteState::Dead);
}

fn run_page(html: &str, b: &Business, tweak: impl FnOnce(&mut SiteSnapshot)) -> Vec<MetricResult> {
    let mut snap = SiteSnapshot { input_url: "https://zub.ru/".into(), home: Some(fetched("https://zub.ru/", html)), page: Some(page::extract(html, "https://zub.ru/")), ..Default::default() };
    tweak(&mut snap);
    let (p, market) = (params(), MarketStats::default());
    run_all(&CheckCtx { business: b, site: Some(&snap), params: &p, market: &market, discovery: None })
}

/// Компания без телефона и адреса в карточке: синтетическая страница не про неё не должна стать «чужим сайтом».
fn anon() -> Business {
    Business { phone: None, address: None, ..business() }
}

fn result<'a>(results: &'a [MetricResult], id: &str) -> &'a MetricResult {
    results.iter().find(|r| r.id == id).unwrap()
}

/// Круг 4, скан 18: blissclinica.by — прайс целыми числами, валюта одна в заголовке столбца.
#[test]
fn prices_with_currency_in_column_header() {
    let html = "<html><head><title>Цены — клиника</title></head><body><h1>Цены</h1><table><tr><th>Услуга</th><th>Цена, бел.руб.</th></tr>\
        <tr><td>Консультация</td><td>45</td></tr><tr><td>УЗИ</td><td>60</td></tr><tr><td>Пилинг</td><td>120</td></tr></table></body></html>";
    let r = run_page(html, &anon(), |_| {});
    assert_eq!(result(&r, "I08").status, Status::Pass, "{}", result(&r, "I08").detail);
}

/// Круг 4: fashioncentre.by — каталог с ценами на shop.fashioncentre.by; rabota.by — соцсети через /social/…
#[test]
fn shop_subdomain_and_own_social_redirects() {
    let html = r#"<html><head><title>Fashion Centre</title></head><body><h1>Одежда</h1><p>Женская одежда белорусских фабрик.</p><a href="https://shop.zub.ru/catalog">Каталог</a>
        <a href="/social/instagram_by">Instagram</a></body></html>"#;
    let r = run_page(html, &anon(), |_| {});
    let i08 = result(&r, "I08");
    assert_eq!(i08.status, Status::Warn, "{}", i08.detail);
    assert!(i08.detail.contains("shop.zub.ru"));
    assert_eq!(result(&r, "I13").status, Status::Warn, "{}", result(&r, "I13").detail);
}

/// Круг 4: Nike, VFS Global, rabota.by — title и H1 ставит JS; Google их видит, Яндекс и ИИ-боты нет.
#[test]
fn title_and_h1_set_by_js_are_warnings() {
    let html = r#"<html><head><script src="/_next/static/main.js" defer></script></head><body><div id="__next"><p>Магазин</p></div></body></html>"#;
    let r = run_page(html, &anon(), |s| {
        s.js_shell = true;
        s.js_title = Some("Nike Store Minsk".into());
        s.js_h1 = Some("Поиск работы в Минске".into());
    });
    assert_eq!(result(&r, "F01").status, Status::Warn, "{}", result(&r, "F01").detail);
    assert_eq!(result(&r, "F03").status, Status::Warn, "{}", result(&r, "F03").detail);
    let r = run_page(html, &anon(), |s| s.js_shell = true);
    assert_eq!(result(&r, "F01").status, Status::Fail);
}

/// Круг 4: beldruk.by, dranikisvobody4.by — домен занят ставками, а в карточке нет телефона и адреса.
#[test]
fn betting_hijack_without_card_contacts() {
    let html = "<html><head><title>Ставки на спорт онлайн</title></head><body><h1>Лучшие букмекеры</h1>\
        <p>Фрибет за регистрацию, высокие коэффициенты, прогнозы на спорт каждый день.</p></body></html>";
    let b = Business { phone: None, address: None, ..business() };
    let r = run_page(html, &b, |_| {});
    let h02 = result(&r, "H02");
    assert_eq!(h02.value.as_ref().and_then(|v| v.as_str()), Some("foreign"), "{}", h02.detail);
}

/// Круг 4: nemiga3.by — «добавление в корзину» только в политике cookie; zoomarket.by — скрытый попап корзины.
#[test]
fn cart_words_in_policy_or_hidden_popup_are_not_a_shop() {
    let html = r#"<html><head><title>ТЦ</title></head><body><h1>Торговый центр</h1><p>Магазины и кафе.</p>
        <p class="popup add-cart">Товар добавлен в корзину</p></body></html>"#;
    let policy = "<html><body><p>Метрика собирает события электронной коммерции (просмотр, добавление в корзину).</p></body></html>";
    let r = run_page(html, &anon(), |s| {
        s.pages.push(crate::site::CrawledPage {
            url: "https://zub.ru/cookie-policy.html".into(),
            final_url: "https://zub.ru/cookie-policy.html".into(),
            depth: 1,
            status: Some(200),
            redirected: false,
            error: None,
            bytes: policy.len(),
            info: Some(page::extract(policy, "https://zub.ru/cookie-policy.html")),
        });
    });
    assert_eq!(result(&r, "M10").status, Status::NotMeasured, "{}", result(&r, "M10").detail);
}

/// Круг 4: vinarnya.by «No connect!», rcek.by «… is under maintenance» в title — сайт не работает.
#[test]
fn database_error_and_maintenance_title_are_not_a_working_site() {
    let r = run_page("<html><body>No connect!</body></html>", &business(), |_| {});
    assert_eq!(result(&r, "B01").status, Status::Fail, "{}", result(&r, "B01").detail);
    // Сайт на фреймах (trodat-bel.by): текста в самой странице нет, но это рабочий сайт.
    let frames = r#"<html><head><title>ТРОДАТ-Беларусь</title></head><frameset rows="95,*"><frame src="top-2.htm"><frame src="main.htm"></frameset></html>"#;
    let r = run_page(frames, &business(), |_| {});
    assert_ne!(result(&r, "B01").status, Status::Fail, "{}", result(&r, "B01").detail);
    let html = "<html><head><title>Центр экологии is under maintenance</title></head><body><h1>Сайт закрыт</h1><p>We are doing some work on our site.</p></body></html>";
    let r = run_page(html, &anon(), |_| {});
    let b01 = result(&r, "B01");
    assert_eq!(b01.status, Status::Fail);
    assert!(b01.detail.contains("under maintenance"), "{}", b01.detail);
}

/// Круг 4: rosting.by — http-скрипты при CSP upgrade-insecure-requests; veromoda.com — GTM-ID во встроенном JSON.
#[test]
fn upgrade_insecure_requests_and_gtm_id_in_json() {
    let html = r#"<html><head><meta http-equiv="Content-Security-Policy" content="upgrade-insecure-requests">
        <script src="http://zub.ru/bitrix/js/main.js"></script><title>Ростинг</title></head>
        <body><h1>Ростинг</h1><p>Хостинг и домены для бизнеса в Минске.</p><script>window.env={"VITE_GTM_ID":"GTM-WD3NCM4"}</script></body></html>"#;
    let r = run_page(html, &anon(), |_| {});
    assert_eq!(result(&r, "O02").status, Status::Pass, "{}", result(&r, "O02").detail);
    assert_ne!(result(&r, "P01").status, Status::Fail, "{}", result(&r, "P01").detail);
}

/// Круг 4: chopchop.me у минской точки — на сайте только российские номера: сайт сети из другой страны.
#[test]
fn other_country_chain_site() {
    let html = "<html><head><title>Chop-Chop — мужская парикмахерская</title></head><body><h1>Chop-Chop</h1>\
        <p>Челябинск: +7 (351) 200-00-00. Пенза: +7 (8412) 20-00-00. Москва: +7 (495) 100-00-00.</p></body></html>";
    let b = Business { name: "Chop-Chop".into(), phone: Some("+375 29 111-22-33".into()), address: Some("Минск, улица Немига, 3".into()), ..business() };
    let r = run_page(html, &b, |_| {});
    let h02 = result(&r, "H02");
    assert_eq!(h02.value.as_ref().and_then(|v| v.as_str()), Some("other_country"), "{}", h02.detail);
    let report = crate::scoring::score(&r, &b, None);
    assert_eq!(report.mini.state, crate::mini::SiteState::Chain);
}

/// Круг 5: 7fridays.by — вместо главной окно «Вы старше 18 лет?»: метрики по нему не считаем.
#[test]
fn age_gate_is_not_measured() {
    let html = r#"<html><head><title>7 Fridays</title></head><body><div class="age-gate-standard"><p>Вы старше 18 лет?</p><button>Да</button><button>Нет</button></div></body></html>"#;
    let r = run_page(html, &anon(), |_| {});
    let b01 = result(&r, "B01");
    assert_eq!((b01.status, b01.value.as_ref().and_then(|v| v.as_str())), (Status::Warn, Some("blocked")), "{}", b01.detail);
    assert_eq!(result(&r, "P01").status, Status::NotMeasured);
    let report = crate::scoring::score(&r, &business(), None);
    assert!(report.mini.headline.contains("возраста"), "{}", report.mini.headline);
}

/// Круг 5: avedress.by — статистика Tilda; kupala.by — GA после согласия на cookie; punktpitania.com — Wix.
#[test]
fn analytics_tilda_consent_and_wix() {
    let p01 = |html: &str| result(&run_page(html, &anon(), |_| {}), "P01").status;
    let tilda = r#"<html><head><title>Ave Dress</title><script src="https://static.tildacdn.com/js/tilda-stat-1.0.min.js"></script></head><body><h1>Платья</h1></body></html>"#;
    assert_ne!(p01(tilda), Status::Fail);
    let consent = r#"<html><head><title>Купала</title></head><body><h1>Театр</h1><script>function decline(){deleteCookie('_ga_HYBR6E5C5J')}</script></body></html>"#;
    assert_ne!(p01(consent), Status::Fail);
    let wix = r#"<html><head><title>Пункт питания</title><script src="https://static.parastorage.com/services/wix-thunderbolt/dist/main.js"></script></head><body><h1>Меню</h1></body></html>"#;
    assert_eq!(p01(wix), Status::NotMeasured);
}

/// Круг 5: lido.by — доставка и оплата описаны на главной, возврата и оферты нет: «внимание», не «ничего нет».
#[test]
fn delivery_and_payment_without_returns() {
    let html = "<html><head><title>Лидо</title></head><body><h1>Доставка еды</h1><button>В корзину</button>\
        <p>Доставка бесплатная, 165 минут. Оплата курьеру картой или наличными.</p></body></html>";
    let r = run_page(html, &anon(), |_| {});
    assert_eq!(result(&r, "M10").status, Status::Warn, "{}", result(&r, "M10").detail);
}

/// Круг 5: boxberrypost.by — портал о ставках: «ставки» много раз и «букмекер».
#[test]
fn repeated_betting_words_are_a_hijack() {
    let mut body = String::from("<html><head><title>Бокс</title></head><body><h1>Бокс</h1>");
    for _ in 0..10 {
        body.push_str("<p>Ставки на бокс: обзор боя и лучшие ставки.</p>");
    }
    body.push_str("<p>Надёжный букмекер.</p></body></html>");
    let b = Business { phone: None, address: None, ..business() };
    let r = run_page(&body, &b, |_| {});
    assert_eq!(result(&r, "H02").value.as_ref().and_then(|v| v.as_str()), Some("foreign"), "{}", result(&r, "H02").detail);
}

/// Круг 5: beldruk.by — домен сети из 7 киосков занят казино: это лид для головной компании, а не «сеть».
#[test]
fn hijacked_chain_domain_is_a_lead() {
    let html = "<html><head><title>Лучшие онлайн-казино</title></head><body><h1>Казино</h1><p>Слоты, фриспины, покер и казино онлайн.</p></body></html>";
    let b = Business { phone: None, address: None, chain_size: 7, ..business() };
    let r = run_page(html, &b, |_| {});
    let report = crate::scoring::score(&r, &b, None);
    assert_ne!(report.mini.state, crate::mini::SiteState::Chain, "{}", report.mini.headline);
    assert_eq!(report.lead, crate::scoring::LeadClass::Hot);
}

/// Круг 5: chehov.by, beldruk.by — на домене казино: I07, I08, I13 и прочее по чужому сайту — «не измерено».
#[test]
fn foreign_site_metrics_are_not_measured() {
    let html = "<html><head><title>Лучшие онлайн-казино</title></head><body><h1>Казино</h1><p>Слоты, фриспины, покер и казино онлайн для всех.</p></body></html>";
    let r = run_page(html, &business(), |_| {});
    assert_eq!(result(&r, "H02").value.as_ref().and_then(|v| v.as_str()), Some("foreign"));
    for id in ["I07", "I08", "I13", "F01", "G01"] {
        assert_eq!(result(&r, id).status, Status::NotMeasured, "{id}: {}", result(&r, id).detail);
    }
}

/// Круг 5: tamarind.by — Organization микроданными; 0km.by — «80 рублей»; wowcamp.by — значок «Made on Tilda».
#[test]
fn microdata_rubles_and_tilda_badge() {
    let html = r#"<html><head><title>Тамаринд — тайский массаж</title></head><body itemscope itemtype="https://schema.org/Organization"><h1>Тамаринд</h1>
        <p>Массаж всего 80 рублей, спа-программа от 33 рублей, пакет от 300 руб.</p>
        <img src="https://static.tildacdn.com/img/tildacopy.png"></body></html>"#;
    let r = run_page(html, &anon(), |_| {});
    assert_eq!(result(&r, "G03").status, Status::Warn, "{}", result(&r, "G03").detail);
    assert_eq!(result(&r, "I08").status, Status::Pass, "{}", result(&r, "I08").detail);
    assert_eq!(result(&r, "F10").status, Status::NotMeasured, "{}", result(&r, "F10").detail);
}

/// Круг 5: creativecenter.mgddm.by — страница веб-сервера по умолчанию; legendar.by — пустая Tilda «Blank page».
#[test]
fn default_server_page_and_blank_tilda() {
    let html = "<html><head><title>Web Server's Default Page</title></head><body><p>There is no Web site at this address.</p></body></html>";
    assert_eq!(result(&run_page(html, &anon(), |_| {}), "B01").status, Status::Fail);
    let blank = r#"<html><head><title>Blank page</title><script src="https://static.tildacdn.com/js/tilda-scripts.min.js"></script></head><body><div class="t-popup"><iframe src="https://booking.example/"></iframe></div></body></html>"#;
    assert_eq!(result(&run_page(blank, &anon(), |_| {}), "B01").status, Status::Fail);
}

/// Круг 5: pm.by — рендер упёрся в проверку Cloudflare: отсутствие title/H1 в исходном HTML не доказано.
#[test]
fn missing_title_after_failed_render_is_not_measured() {
    let html = r#"<html><head><script src="/app.js"></script></head><body><div id="app"><p>Загрузка страницы, подождите немного пожалуйста.</p></div></body></html>"#;
    let r = run_page(html, &anon(), |s| s.render_error = Some("рендер попал на проверку защиты от ботов".into()));
    assert_eq!(result(&r, "F01").status, Status::NotMeasured, "{}", result(&r, "F01").detail);
}

/// Круг 5: armani.com — из Японии сайт бренда отдал /ja-jp/: японский текст не делает его чужим.
#[test]
fn japanese_locale_of_brand_site_is_not_foreign() {
    let html = "<html><head><title>ジョルジオ アルマーニ</title></head><body><h1>ジョルジオ アルマーニ 公式オンラインストア</h1>\
        <p>新作コレクション、ウィメンズ、メンズ、バッグ、シューズ、アクセサリーをオンラインでご覧ください。送料無料でお届けします。</p></body></html>";
    let r = run_page(html, &business(), |s| s.home = Some(fetched("https://www.armani.com/ja-jp/", html)));
    assert_ne!(result(&r, "H02").value.as_ref().and_then(|v| v.as_str()), Some("foreign"), "{}", result(&r, "H02").detail);
    // zegna.com отдаёт /jp-ja/ — тот же случай.
    let r = run_page(html, &business(), |s| s.home = Some(fetched("https://www.zegna.com/jp-ja/", html)));
    assert_ne!(result(&r, "H02").value.as_ref().and_then(|v| v.as_str()), Some("foreign"), "{}", result(&r, "H02").detail);
}

/// Круг 5: kiseleva.store — каталог Tilda Store рисует скрипт: «страниц услуг 0» не доказано.
#[test]
fn tilda_store_catalog_is_not_measured() {
    let html = r#"<html><head><title>Kiseleva — одежда</title></head><body><h1>Одежда</h1><p>Платья и костюмы ручной работы из льна.</p><div class="t-store js-store-grid"></div></body></html>"#;
    assert_eq!(result(&run_page(html, &anon(), |_| {}), "F07").status, Status::NotMeasured);
}

// ---------- Многоязычие: те же правила на сайтах США и Европы ----------

fn run_in(html: &str, b: &Business, country: &str, url: &str) -> Vec<MetricResult> {
    let snap = SiteSnapshot { input_url: url.into(), home: Some(fetched(url, html)), page: Some(page::extract(html, url)), ..Default::default() };
    let p = ScanParams { country: Some(country.into()), ..params() };
    let market = MarketStats::default();
    run_all(&CheckCtx { business: b, site: Some(&snap), params: &p, market: &market, discovery: None })
}

/// Сайт парижской компании, где только британские номера, — сайт сети другой страны; с местным номером — нет.
#[test]
fn other_country_site_in_france() {
    let b = Business { name: "Maison Lumière".into(), phone: Some("+33 1 42 68 53 00".into()), address: Some("12 Rue de Rivoli, 75001 Paris".into()), ..business() };
    let uk = "<html><head><title>Maison Lumière — London</title></head><body><h1>Maison Lumière</h1><p>Call us: +44 20 7946 0958 or +44 20 7946 0959. Visit our shop at 5 Baker Street, London.</p></body></html>";
    let r = run_in(uk, &b, "fr", "https://maison-lumiere.example/");
    assert_eq!(result(&r, "H02").value.as_ref().and_then(|v| v.as_str()), Some("other_country"), "{}", result(&r, "H02").detail);
    let local = "<html><head><title>Maison Lumière</title></head><body><h1>Maison Lumière</h1><p>Tél. 01 42 68 53 00 — 12 rue de Rivoli, 75001 Paris. London: +44 20 7946 0958, +44 20 7946 0959.</p></body></html>";
    let r = run_in(local, &b, "fr", "https://maison-lumiere.example/");
    assert_eq!(result(&r, "H02").status, Status::Pass, "{}", result(&r, "H02").detail);
}

/// Адрес с диакритикой и без; чешский адрес с «Ž» не уходит в белорусскую кириллицу.
#[test]
fn european_addresses_match() {
    use super::content::address_matches;
    assert!(address_matches("52 Avenue des Champs-Élysées, 75008 Paris", "boutique au 52 avenue des champs-elysees, paris"));
    assert!(address_matches("Calle de Alcalá 45, Madrid", "estamos en calle alcala 45, madrid"));
    assert!(address_matches("Žitná 5, Praha", "najdete nás: žitná 5, praha 1"));
    assert!(address_matches("Oranienstraße 12, Berlin", "oranienstr. 12, 10999 berlin"));
    assert!(!address_matches("Calle de Alcalá 45, Madrid", "gran vía 12, madrid"));
}

/// Магазин по-английски: доставка, оплата, возврат текстом — условия есть; по-французски без возврата — «внимание».
#[test]
fn shop_terms_in_english_and_french() {
    let en = "<html><head><title>Brooklyn Candle Co.</title></head><body><h1>Candles</h1><button>Add to cart</button>\
        <p>Free shipping over $50. Payment by card or Apple Pay. 30-day returns and refunds.</p></body></html>";
    assert_eq!(result(&run_in(en, &anon(), "us", "https://candle.example/"), "M10").status, Status::Pass);
    let fr = "<html><head><title>Bougies de Paris</title></head><body><h1>Bougies</h1><button>Ajouter au panier</button>\
        <p>Livraison en 48 h partout en France. Paiement sécurisé par carte.</p></body></html>";
    assert_eq!(result(&run_in(fr, &anon(), "fr", "https://bougies.example/"), "M10").status, Status::Warn);
}

/// Возрастной барьер по-французски, ставки по-испански, цены «Prix (€)», ошибка WordPress по-французски.
#[test]
fn age_gate_betting_prices_errors_in_european_languages() {
    let gate = r#"<html><head><title>Cave à vins</title></head><body><div class="modal"><p>Avez-vous 18 ans ?</p><button>Oui</button></div></body></html>"#;
    assert_eq!(result(&run_in(gate, &anon(), "fr", "https://cave.example/"), "B01").value.as_ref().and_then(|v| v.as_str()), Some("blocked"));
    let bet = "<html><head><title>Apuestas deportivas</title></head><body><h1>Casino online</h1><p>Las mejores apuestas y tragamonedas con bono.</p></body></html>";
    let r = run_in(bet, &anon(), "es", "https://bar-pepe.example/");
    assert_eq!(result(&r, "H02").value.as_ref().and_then(|v| v.as_str()), Some("foreign"), "{}", result(&r, "H02").detail);
    let prices = "<html><head><title>Salon Belle</title></head><body><h1>Tarifs</h1><table><tr><th>Soin</th><th>Prix (€)</th></tr>\
        <tr><td>Coupe</td><td>45</td></tr><tr><td>Couleur</td><td>60</td></tr><tr><td>Brushing</td><td>30</td></tr></table></body></html>";
    assert_eq!(result(&run_in(prices, &anon(), "fr", "https://belle.example/"), "I08").status, Status::Pass);
    let wp = "<html><head><title>Erreur</title></head><body><p>Il y a eu une erreur critique sur ce site.</p></body></html>";
    assert_eq!(result(&run_in(wp, &anon(), "fr", "https://x.example/"), "B01").status, Status::Fail);
}

/// Японский сайт у компании из Токио — норма, а не чужой домен.
#[test]
fn japanese_site_in_japan_is_not_foreign() {
    let html = "<html><head><title>すし匠</title></head><body><h1>すし匠 銀座店</h1>\
        <p>銀座で江戸前寿司を楽しめるお店です。ご予約はお電話またはオンラインで承っております。季節の魚を使ったおまかせコースをご用意しています。</p></body></html>";
    let b = Business { name: "Sushi Sho".into(), phone: Some("+81 3-1234-5678".into()), address: Some("Ginza 5-1-1, Tokyo".into()), ..business() };
    let r = run_in(html, &b, "jp", "https://sushisho.example/");
    assert_ne!(result(&r, "H02").value.as_ref().and_then(|v| v.as_str()), Some("foreign"), "{}", result(&r, "H02").detail);
}

/// bonus-travel.by: http:// отдаёт 200 без перенаправления, а https-версия — HSTS. Посетитель, уже
/// открывавший https, переходит сам; новый по ссылке из карточки — нет. «Внимание», а не «провал».
#[test]
fn http_without_redirect_but_hsts_is_a_warning() {
    let html = "<html><head><title>Бонус Трэвел — туры из Минска</title></head><body><h1>Туры</h1><p>Горящие туры и путёвки из Минска.</p></body></html>";
    let probe = |hsts| RedirectProbe { url: "https://www.zub.ru/".into(), first_status: Some(200), final_url: Some("https://www.zub.ru/".into()), final_status: Some(200), hops: 0, error: None, hsts };
    let tls = crate::tls::TlsInfo { valid: true, ..Default::default() };
    let run = |hsts: bool| {
        run_page(html, &anon(), |s| {
            s.home = Some(fetched("http://www.zub.ru/", html));
            s.https_probe = Some(probe(hsts));
            s.tls = Some(tls.clone());
        })
    };
    let r = run(true);
    let b05 = result(&r, "B05");
    assert_eq!((b05.status, b05.value.as_ref().and_then(|v| v.as_str())), (Status::Warn, Some("no_redirect_hsts")), "{}", b05.detail);
    assert_eq!(result(&run(false), "B05").status, Status::Fail);
}

/// Лондон/Нью-Йорк: robots.txt закрывает только Яндекс — для страны вне его рынка это не стоп-фактор.
#[test]
fn yandex_block_outside_yandex_market_is_not_critical() {
    let html = "<html><head><title>Soho Records — vinyl shop in London</title></head><body><h1>Soho Records</h1><p>Vinyl records, turntables and more in Soho, London.</p></body></html>";
    let robots_body = "User-agent: Yandex\nDisallow: /\n\nUser-agent: *\nAllow: /\n";
    let url = "https://sohorecords.example/";
    let snap = SiteSnapshot {
        input_url: url.into(),
        home: Some(fetched(url, html)),
        page: Some(page::extract(html, url)),
        robots: Some(TextResource { url: format!("{url}robots.txt"), status: Some(200), body: robots_body.into(), bytes: robots_body.len(), error: None }),
        robots_directives: Some(robots::parse(robots_body)),
        ..Default::default()
    };
    let market = MarketStats::default();
    for (country, want) in [("gb", Status::Pass), ("by", Status::Fail)] {
        let p = ScanParams { country: Some(country.into()), ..params() };
        let r = run_all(&CheckCtx { business: &anon(), site: Some(&snap), params: &p, market: &market, discovery: None });
        assert_eq!(result(&r, "C02").status, want, "{country}: {}", result(&r, "C02").detail);
    }
}

/// Private Eye, internetlawcentre.co.uk: «renew your subscription», «account suspended» в длинном тексте — не парковка.
#[test]
fn parking_phrases_in_long_text_are_not_parking() {
    let mut body = String::from("<html><head><title>Private Eye — satirical magazine</title></head><body><h1>Private Eye</h1>");
    for _ in 0..60 {
        body.push_str("<p>The latest issue covers politics, media and the law in Britain with satire and investigation.</p>");
    }
    body.push_str("<p>Subscribers: renew your subscription online. If your account suspended notice appears, contact us.</p></body></html>");
    let r = run_in(&body, &anon(), "gb", "https://www.private-eye.example/");
    assert_ne!(result(&r, "B01").status, Status::Fail, "{}", result(&r, "B01").detail);
}

/// Круг 6: vegans.by — название в карточке и есть домен, а на домене новостной сайт: не «свой сайт».
#[test]
fn name_equal_to_domain_is_not_proof() {
    let html = "<html><head><title>Новости Бреста и Брестской области</title></head><body><h1>Новости Бреста</h1>\
        <p>Главные события дня в Бресте: происшествия, политика, спорт. Звоните в редакцию: +375 162 00-00-00.</p></body></html>";
    let b = Business { name: "Vegans.by".into(), phone: Some("+375 29 111-22-33".into()), address: Some("Минск, улица Немига, 3".into()), ..business() };
    let r = run_in(html, &b, "by", "https://vegans.by/");
    assert_eq!(result(&r, "H02").value.as_ref().and_then(|v| v.as_str()), Some("foreign"), "{}", result(&r, "H02").detail);
}

/// Круг 6: koluchka.by — страница доставки и оплаты есть только в sitemap: «внимание», а не «условий нет».
#[test]
fn sales_terms_only_in_sitemap() {
    let html = "<html><head><title>Колючка — кактусы</title></head><body><h1>Кактусы</h1><button>В корзину</button><p>Магазин кактусов и суккулентов.</p></body></html>";
    let r = run_page(html, &anon(), |s| s.sitemap.urls = vec!["https://zub.ru/".into(), "https://zub.ru/delivery-and-payment/".into()]);
    assert_eq!(result(&r, "M10").status, Status::Warn, "{}", result(&r, "M10").detail);
}

/// Круг 6: dsvd.by — адрес белорусской латиницей без диакритики; vikingvape.by — пустой блок JSON-LD.
#[test]
fn belarusian_latin_without_diacritics_and_empty_jsonld() {
    assert!(super::content::address_matches("vulica Miasnikova 70, Minsk", "наш адрес: г. минск, ул. мясникова 70, офис 24"));
    let html = r#"<html><head><title>Vape shop</title><script type="application/ld+json"></script>
        <script type="application/ld+json">{"@context":"https://schema.org","@type":"Store","name":"Viking"}</script></head><body><h1>Vape</h1><p>Магазин.</p></body></html>"#;
    let r = run_page(html, &anon(), |_| {});
    assert_ne!(result(&r, "G04").status, Status::Fail, "{}", result(&r, "G04").detail);
}
