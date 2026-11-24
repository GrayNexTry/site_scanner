//! M. Доверие и право, N. Конверсия, P. Аналитика, Q. Доступность.

use std::sync::OnceLock;

use regex::Regex;

use super::{grade_high, list_short, CheckCtx, SiteView};
use crate::model::{MetricResult, Status};
use crate::lexicon;
use crate::providers::presets::is_medical;
use crate::region::Jurisdiction;
use crate::text::registrable_domain;

/// Контрольная сумма ИНН (10 или 12 цифр).
pub fn inn_valid(inn: &str) -> bool {
    let d: Vec<u32> = inn.chars().filter_map(|c| c.to_digit(10)).collect();
    let ctrl = |w: &[u32], ds: &[u32]| w.iter().zip(ds).map(|(a, b)| a * b).sum::<u32>() % 11 % 10;
    match d.len() {
        10 => ctrl(&[2, 4, 10, 3, 5, 9, 4, 6, 8], &d) == d[9],
        12 => ctrl(&[7, 2, 4, 10, 3, 5, 9, 4, 6, 8], &d) == d[10] && ctrl(&[3, 7, 2, 4, 10, 3, 5, 9, 4, 6, 8], &d) == d[11],
        _ => false,
    }
}

/// Название страны для пояснений: код в верхнем регистре или «не задана».
fn country_label(c: &str) -> String {
    if c.is_empty() { "не задана".into() } else { c.to_uppercase() }
}

pub fn run(ctx: &CheckCtx, v: &SiteView, out: &mut Vec<MetricResult>) {
    let page = v.page;
    let text = &v.text_all;
    let html = &v.html_all;
    let links: Vec<(String, String)> =
        v.pages.iter().flat_map(|p| p.links.iter()).map(|l| (l.href.to_lowercase(), l.text.to_lowercase())).collect();
    let link_has = |needles: &[&str]| links.iter().any(|(h, t)| needles.iter().any(|n| h.contains(n) || t.contains(n)));
    let query = crate::providers::query_text(ctx.params);

    // ---------- M ----------
    let jur = ctx.params.jurisdiction();
    let country = ctx.params.country.clone().unwrap_or_default();
    let html_trackers = lexicon::ANALYTICS.iter().any(|(m, _)| html.contains(m)) || lexicon::PIXELS.iter().any(|(m, _)| html.contains(m));
    let privacy = link_has(lexicon::PRIVACY)
        || ["политика конфиденциальности", "политика обработки", "privacy policy", "datenschutzerklärung", "politique de confidentialité"].iter().any(|t| text.contains(t));
    let law = match jur {
        Jurisdiction::Ru => "152-ФЗ",
        Jurisdiction::Eu | Jurisdiction::Uk => "GDPR, ст. 13",
        Jurisdiction::Us => "CalOPPA/CCPA",
        Jurisdiction::Other => "",
    };
    out.push(if privacy {
        MetricResult::pass("M01", "Политика конфиденциальности опубликована")
    } else if jur == Jurisdiction::Other {
        MetricResult::warn("M01", "Политики конфиденциальности нет")
    } else {
        MetricResult::fail("M01", format!("Политики конфиденциальности нет — требование {law}"))
    });
    let personal: Vec<_> = v.pages.iter().flat_map(|p| p.forms.iter()).filter(|f| f.personal).collect();
    if personal.is_empty() {
        out.push(MetricResult::na("M02", "Форм с персональными данными не найдено"));
    } else {
        let prechecked = personal.iter().filter(|f| f.consent_checkbox == Some(true)).count();
        let bare = personal.iter().filter(|f| f.consent_checkbox.is_none() && !f.consent_text).count();
        let text_only = personal.iter().filter(|f| f.consent_checkbox.is_none() && f.consent_text).count();
        out.push(match jur {
            Jurisdiction::Ru if prechecked > 0 => MetricResult::fail("M02", format!("Галочка согласия стоит заранее в {prechecked} формах")),
            Jurisdiction::Ru if bare > 0 => MetricResult::fail("M02", format!("{bare} форм без согласия на обработку ПДн")),
            Jurisdiction::Ru if text_only > 0 => MetricResult::warn("M02", "Согласие только текстом, без отдельной галочки (152-ФЗ с 01.09.2025)"),
            Jurisdiction::Ru => MetricResult::pass("M02", "Согласие отдельной галочкой, не отмечено заранее"),
            Jurisdiction::Eu | Jurisdiction::Uk if prechecked > 0 => {
                MetricResult::fail("M02", format!("Галочка согласия стоит заранее в {prechecked} формах — по GDPR такое согласие недействительно"))
            }
            Jurisdiction::Eu | Jurisdiction::Uk if bare > 0 => MetricResult::warn("M02", format!("{bare} форм без ссылки на политику конфиденциальности (GDPR, ст. 13)")),
            Jurisdiction::Eu | Jurisdiction::Uk => MetricResult::pass("M02", "У форм есть согласие или ссылка на политику"),
            _ if bare > 0 => MetricResult::info("M02", format!("{bare} форм без ссылки на политику (в стране не обязательно)")),
            _ => MetricResult::info("M02", "У форм есть ссылка на политику"),
        });
    }
    let cmp = lexicon::any_in(html, lexicon::CMP);
    let cookie_text = text.contains("cookie") || text.contains("куки");
    out.push(match jur {
        Jurisdiction::Eu | Jurisdiction::Uk => {
            if !cmp.is_empty() {
                MetricResult::pass("M03", format!("Баннер согласия на cookie: {}", cmp[0]))
            } else if !html_trackers {
                MetricResult::pass("M03", "Трекеров нет — баннер не обязателен")
            } else if cookie_text {
                MetricResult::warn("M03", "Есть текст о cookie, но платформы согласия не видно — трекеры могут стартовать до согласия")
            } else {
                MetricResult::fail("M03", "Аналитика или пиксели стоят без баннера согласия — нарушение ePrivacy/GDPR")
            }
        }
        Jurisdiction::Us => {
            let pixels = lexicon::PIXELS.iter().any(|(m, _)| html.contains(m));
            let opt_out = link_has(&["do not sell", "do-not-sell", "your privacy choices", "opt-out", "opt out"]);
            if pixels && !opt_out {
                MetricResult::warn("M03", "Рекламные пиксели без ссылки «Do Not Sell or Share» (CCPA, если есть клиенты из Калифорнии)")
            } else {
                MetricResult::pass("M03", if opt_out { "Ссылка отказа от продажи данных есть" } else { "Рекламных пикселей нет" })
            }
        }
        _ => {
            let analytics = lexicon::ANALYTICS.iter().any(|(m, _)| html.contains(m));
            if cookie_text || !cmp.is_empty() {
                MetricResult::pass("M03", "Уведомление о cookie есть")
            } else if analytics {
                MetricResult::warn("M03", "Метрики стоят, а уведомления о cookie нет")
            } else {
                MetricResult::pass("M03", "Метрик нет — уведомление не обязательно")
            }
        }
    });
    if jur == Jurisdiction::Ru {
        let foreign = ["docs.google.com/forms", "forms.gle", "typeform.com", "jotform", "formspree.io", "tally.so", "forms.office.com", "wufoo.com"];
        let foreign_used: Vec<String> = v
            .pages
            .iter()
            .flat_map(|p| p.forms.iter().filter_map(|f| f.action.clone()).chain(p.iframes.iter().cloned()))
            .filter(|u| foreign.iter().any(|f| u.to_lowercase().contains(f)))
            .collect();
        out.push(if foreign_used.is_empty() {
            MetricResult::pass("M04", "Формы не отправляют данные в иностранные сервисы")
        } else {
            MetricResult::fail("M04", format!("Данные уходят в иностранные сервисы: {}", list_short(&foreign_used, 2)))
        });
    } else {
        out.push(MetricResult::na("M04", format!("Требование 152-ФЗ о локализации; для страны «{}» не применяется", country_label(&country))));
    }
    if jur == Jurisdiction::Ru {
        static INN: OnceLock<Regex> = OnceLock::new();
        static OGRN: OnceLock<Regex> = OnceLock::new();
        let inn_re = INN.get_or_init(|| Regex::new(r"инн[\s:№]*(\d{12}|\d{10})\b").unwrap());
        let ogrn_re = OGRN.get_or_init(|| Regex::new(r"огрн(?:ип)?[\s:№]*(\d{15}|\d{13})\b").unwrap());
        let inns: Vec<String> = inn_re.captures_iter(text).map(|c| c[1].to_string()).collect();
        let ogrn = ogrn_re.is_match(text);
        out.push(match (inns.iter().find(|i| inn_valid(i)), inns.first(), ogrn) {
            (Some(i), _, _) => MetricResult::pass("M05", format!("ИНН {i}{}", if ogrn { ", ОГРН есть" } else { "" })).with_value(i.clone()),
            (None, Some(i), _) => MetricResult::warn("M05", format!("ИНН {i} не проходит проверку контрольной суммы")),
            (None, None, true) => MetricResult::warn("M05", "ОГРН есть, ИНН нет"),
            (None, None, false) => MetricResult::warn("M05", "Реквизитов (ИНН, ОГРН) на сайте нет"),
        });
    } else if let Some((markers, what, required)) = lexicon::legal_notice(&country) {
        let found = markers.iter().any(|m| text.contains(m)) || link_has(markers);
        out.push(match (found, required) {
            (true, _) => MetricResult::pass("M05", format!("{what}: есть")),
            (false, true) => MetricResult::fail("M05", format!("Нет обязательных выходных данных: {what}")),
            (false, false) => MetricResult::warn("M05", format!("Не найдено: {what}")),
        });
    } else {
        let vat = ["vat", "tax id", "ein", "license #", "lic #", "licence"].iter().any(|m| text.contains(m));
        out.push(MetricResult::info("M05", if vat { "Регистрационные данные указаны" } else { "Реквизиты на сайте в этой стране не обязательны" }));
    }
    if jur == Jurisdiction::Ru {
        let licence = text.contains("лиценз");
        out.push(match (licence, is_medical(&query)) {
            (true, _) => MetricResult::pass("M06", "Лицензия упомянута (сверка с реестром — вручную)"),
            (false, true) => MetricResult::fail("M06", "Для медицинской деятельности лицензия обязательна, на сайте её нет"),
            (false, false) => MetricResult::na("M06", "Ниша без обязательной лицензии"),
        });
    } else {
        let licence = ["licens", "license", "approbation", "kammer", "board certified", "registered with", "gdc", "nhs", "ordre des"].iter().any(|m| text.contains(m));
        out.push(MetricResult::info("M06", if licence { "Лицензия или членство в палате упомянуты" } else { "Публикация лицензии на сайте в этой стране не обязательна" }));
    }
    let proof = lexicon::any_in(text, lexicon::PROOF);
    out.push(if proof.is_empty() { MetricResult::warn("M07", "Отзывов и примеров работ на сайте нет") } else { MetricResult::pass("M07", format!("Есть: {}", proof.into_iter().take(3).collect::<Vec<_>>().join(", "))) });
    let team = lexicon::any_in(text, lexicon::TEAM);
    out.push(if team.is_empty() { MetricResult::warn("M08", "Информации о команде нет") } else { MetricResult::pass("M08", "Есть информация о специалистах") });
    static EMAIL: OnceLock<Regex> = OnceLock::new();
    let email_re = EMAIL.get_or_init(|| Regex::new(r"[a-z0-9._%+\-]+@([a-z0-9\-]+(?:\.[a-z0-9\-]+)+)").unwrap());
    let site_domain = registrable_domain(v.final_url.host_str().unwrap_or(""));
    let mut domains: Vec<String> = page.mailtos.iter().filter_map(|m| m.split('@').nth(1)).map(|d| d.to_lowercase()).collect();
    domains.extend(email_re.captures_iter(text).map(|c| c[1].to_string()).filter(|d| !d.ends_with(".png") && !d.ends_with(".jpg")));
    out.push(if domains.is_empty() {
        MetricResult::na("M09", "Почты на сайте нет")
    } else if domains.iter().any(|d| registrable_domain(d) == site_domain) {
        MetricResult::pass("M09", "Почта на домене сайта")
    } else if let Some(d) = domains.iter().find(|d| lexicon::is_free_email(d)) {
        MetricResult::warn("M09", format!("Бесплатная почта: {d}"))
    } else {
        MetricResult::pass("M09", format!("Почта на домене {}", domains[0]))
    });
    // Магазин — по тому, что видит посетитель (кнопки «в корзину»), а не по служебному коду CMS
    // (`/cart/` в настройках шаблона есть и у сайтов без продаж).
    // Не по юридическим страницам (в политике cookie: «события… добавление в корзину») и не по
    // скрытым шаблонным окнам («Товар добавлен в корзину» есть и у сайтов без продаж).
    let legal = |u: &str| {
        let l = u.to_lowercase();
        [
            "privacy", "cookie", "polic", "polit", "konfid", "oferta", "offer", "agreement", "soglas", "terms", "pravila", "mentions",
            "cgv", "cgu", "confidentialit", "aviso-legal", "privacidad", "condiciones", "impressum", "datenschutz", "agb", "condizioni",
            "regulamin", "voorwaarden", "termos", "legal",
        ]
        .iter()
        .any(|w| l.contains(w))
    };
    let has_any = |ws: &[&str]| ws.iter().any(|w| text.contains(w));
    // Страница условий, на которую не ведут ссылки, но которая есть в sitemap (koluchka.by: /delivery-and-payment/).
    let terms_in_sitemap = v.snap.sitemap.urls.iter().any(|u| {
        let l = u.to_lowercase();
        [
            "delivery", "dostavka", "oplata", "payment", "vozvrat", "return", "refund", "shipping", "oferta", "terms", "agb", "widerruf",
            "livraison", "cgv", "envio", "devolucion", "spedizion", "dostawa", "zwrot",
        ]
        .iter()
        .any(|w| l.contains(w))
    });
    let shop = v.pages.iter().filter(|p| !legal(&p.url)).any(|p| {
        let (t, h) = (p.text_lower(), p.hidden_text.to_lowercase().replace('ё', "е"));
        lexicon::SHOP.iter().filter(|m| **m != "оформить заказ").any(|m| t.matches(m).count() > h.matches(m).count())
    });
    out.push(if !shop {
        MetricResult::na("M10", "Продаж на сайте нет")
    } else if link_has(lexicon::SHOP_TERMS) || (has_any(lexicon::DELIVERY) && has_any(lexicon::PAYMENT) && has_any(lexicon::RETURNS)) {
        MetricResult::pass("M10", "Условия продажи опубликованы")
    } else if terms_in_sitemap {
        MetricResult::warn("M10", "Страница условий продажи есть только в sitemap — с сайта на неё ссылок нет, покупатель её не найдёт")
    } else if has_any(lexicon::DELIVERY) && has_any(lexicon::PAYMENT) {
        MetricResult::warn("M10", "Доставка и оплата описаны, но условий возврата и публичной оферты нет")
    } else {
        MetricResult::fail("M10", "Есть корзина, но нет условий продажи, возврата и доставки")
    });

    // ---------- N ----------
    let ctas: Vec<&String> = page.buttons.iter().filter(|b| lexicon::CTA.iter().any(|c| b.to_lowercase().contains(c))).collect();
    let service = crate::providers::service_terms(ctx.params);
    let h1_service = page.h1s().iter().any(|h| {
        let l = h.to_lowercase().replace('ё', "е");
        service.iter().any(|s| l.contains(s.as_str()))
    });
    let approx = " (без рендера: проверено наличие, а не положение на экране)";
    out.push(match (!ctas.is_empty(), h1_service || service.is_empty()) {
        (true, true) => MetricResult::pass("N01", format!("Оффер в H1 и кнопка «{}»{approx}", ctas[0])),
        (true, false) => MetricResult::warn("N01", format!("Кнопка «{}» есть, но услуги нет в H1{approx}", ctas[0])),
        (false, true) => MetricResult::warn("N01", format!("Нет кнопки действия (записаться, заказать){approx}")),
        (false, false) => MetricResult::fail("N01", format!("Ни оффера в H1, ни кнопки действия{approx}")),
    });
    out.push(if page.header_tel {
        MetricResult::pass("N02", "Кликабельный телефон в шапке")
    } else if !page.tel_links.is_empty() {
        MetricResult::warn("N02", "Телефон кликабелен, но не в шапке")
    } else {
        MetricResult::fail("N02", "Кликабельного телефона нет")
    });
    let mut found: Vec<&str> = lexicon::MESSENGERS.iter().filter(|(m, _)| links.iter().any(|(h, _)| h.contains(m)) || html.contains(m)).map(|(_, n)| *n).collect();
    found.dedup();
    out.push(if found.is_empty() { MetricResult::warn("N03", "Кнопок мессенджеров нет") } else { MetricResult::pass("N03", found.join(", ")) });
    let booking_found = lexicon::any_in(html, lexicon::BOOKING_WIDGETS);
    out.push(if !booking_found.is_empty() {
        MetricResult::pass("N04", format!("Онлайн-запись: {}", booking_found.join(", ")))
    } else if lexicon::BOOKING_TEXT.iter().any(|t| text.contains(t)) {
        MetricResult::pass("N04", "Онлайн-запись есть")
    } else {
        MetricResult::warn("N04", "Онлайн-записи нет")
    });
    let lead_forms: Vec<usize> = page.forms.iter().filter(|f| f.personal && !f.not_lead).map(|f| f.fields.len()).collect();
    out.push(match lead_forms.iter().min() {
        None => MetricResult::warn("N05", "Формы заявки на главной нет"),
        Some(&n) => MetricResult::new("N05", if n <= 4 { Status::Pass } else if n <= 6 { Status::Warn } else { Status::Fail }, format!("Самая короткая форма: {n} полей")).with_value(n),
    });
    static RATING: OnceLock<Regex> = OnceLock::new();
    let rating_re = RATING.get_or_init(|| Regex::new(r"\b[45][.,]\d\b").unwrap());
    let page_text = page.text_lower();
    let widget = lexicon::REVIEW_WIDGETS.iter().any(|w| page.html_lower.contains(w));
    let review_word = lexicon::REVIEW_WORDS.iter().any(|w| page_text.contains(w));
    out.push(if widget || (review_word && rating_re.is_match(&page_text)) {
        MetricResult::pass("N06", "Рейтинг или виджет отзывов на главной")
    } else if review_word {
        MetricResult::warn("N06", "Отзывы упомянуты, рейтинга нет")
    } else {
        MetricResult::fail("N06", "Соцдоказательства на главной нет")
    });
    if v.snap.image_checks.is_empty() {
        out.push(MetricResult::na("N07", "Картинок для проверки нет"));
    } else {
        let urls = |pred: &dyn Fn(u16) -> bool| {
            let mut found: Vec<String> = v
                .snap
                .image_checks
                .iter()
                .filter(|c| c.final_status.is_some_and(pred) && !c.url.to_lowercase().contains("captcha"))
                .map(|c| c.url.clone())
                .collect();
            found.dedup();
            found
        };
        let broken = urls(&|s| matches!(s, 404 | 410));
        // 5xx — сервер картинок не справился (часто оптимизатор /_next/image на медленном сайте), сама картинка может быть цела.
        let server = urls(&|s| s >= 500 && s != 503);
        out.push(if !broken.is_empty() {
            MetricResult::fail("N07", format!("Битые картинки: {}", list_short(&broken, 2)))
        } else if !server.is_empty() {
            MetricResult::warn("N07", format!("Сервер картинок отвечал ошибкой (5xx), проверить повторно: {}", list_short(&server, 2)))
        } else {
            MetricResult::pass("N07", format!("Проверено {} картинок, битых нет (ошибки JS без рендера не проверялись)", v.snap.image_checks.len()))
        });
    }
    let card_addr = ctx.business.address.as_deref().is_some_and(|a| super::content::address_matches(a, &page_text));
    let schema_addr = page.jsonld.iter().any(|j| j.to_lowercase().contains("postaladdress") || j.to_lowercase().contains("streetaddress"));
    out.push(if super::content::address_re().is_match(&page_text) || card_addr || schema_addr {
        MetricResult::pass("N08", "Адрес на главной")
    } else {
        MetricResult::fail("N08", "Адреса на главной нет")
    });

    // ---------- P ----------
    let mut analytics: Vec<&str> = lexicon::ANALYTICS.iter().filter(|(m, _)| html.contains(m)).map(|(_, n)| *n).collect();
    analytics.dedup();
    // Идентификатор GTM во встроенных настройках («VITE_GTM_ID":"GTM-WD3NCM4») — адрес собирает бандл.
    static GTM: OnceLock<Regex> = OnceLock::new();
    if !analytics.contains(&"Google Tag Manager") && GTM.get_or_init(|| Regex::new(r#"["'=:]\s*["']?gtm-[a-z0-9]{6,8}\b"#).unwrap()).is_match(html) {
        analytics.push("Google Tag Manager");
    }
    // Счётчик, включаемый после согласия на cookie: <script type="text/plain" data-cc-src="/metrika.js">.
    // Счётчики, которые сервер вставляет только после согласия на cookie: их куки чистит баннер
    // («deleteCookie('_ga_HYBR6E5C5J')», «_ym_uid») — сам счётчик в коде появится после согласия.
    static GA_COOKIE: OnceLock<Regex> = OnceLock::new();
    if analytics.is_empty() && GA_COOKIE.get_or_init(|| Regex::new(r"_ga_[a-z0-9]{6,}").unwrap()).is_match(html) {
        analytics.push("Google Analytics (после согласия на cookie)");
    }
    let metrika = html.contains("mc.yandex.ru/metrika")
        || html.contains("ym(")
        || (html.contains("data-cc-src") && html.contains("metrika"))
        || html.contains("_ym_uid");
    // Wix подключает счётчики своим менеджером тегов (siteTags): в коде страницы их не видно.
    let wix = html.contains("static.parastorage.com") || html.contains("wix-code") || html.contains("x-wix-");
    out.push(if metrika {
        MetricResult::pass("P01", if html.contains("webvisor:true") || html.contains("webvisor: true") { "Яндекс Метрика с Вебвизором" } else { "Яндекс Метрика" })
    } else if analytics.is_empty() && wix {
        MetricResult::na("P01", "Сайт на Wix: счётчики подключаются через настройки Wix и в коде страницы не видны — проверить вручную")
    } else if analytics.is_empty() {
        MetricResult::fail("P01", "Веб-аналитики нет — результат работ не измерить")
    } else if jur == Jurisdiction::Ru {
        MetricResult::warn("P01", format!("Яндекс Метрики нет, стоит: {}", analytics.join(", ")))
    } else {
        MetricResult::pass("P01", analytics.join(", "))
    });
    let ga = lexicon::any_in(html, &["googletagmanager.com/gtag", "googletagmanager.com/gtm", "google-analytics.com"]);
    out.push(MetricResult::info("P02", if ga.is_empty() { "GA4/GTM нет".to_string() } else { "GA4/GTM есть".to_string() }).with_value(!ga.is_empty()));
    let mut ct_found: Vec<&str> = lexicon::CALLTRACKING.iter().filter(|(m, _)| html.contains(m)).map(|(_, n)| *n).collect();
    ct_found.dedup();
    out.push(MetricResult::info("P04", if ct_found.is_empty() { "Коллтрекинга нет".to_string() } else { ct_found.join(", ") }).with_value(ct_found.len()));
    let mut px_found: Vec<&str> = lexicon::PIXELS.iter().filter(|(m, _)| html.contains(m)).map(|(_, n)| *n).collect();
    px_found.dedup();
    out.push(MetricResult::info("P05", if px_found.is_empty() { "Рекламных пикселей нет".to_string() } else { format!("Рекламные пиксели: {}", px_found.join(", ")) }).with_value(!px_found.is_empty()));

    // ---------- Q ----------
    if let Some(a) = v.snap.psi.as_ref().filter(|p| p.error.is_none()).and_then(|p| p.a11y) {
        out.push(grade_high("Q01", a, 90.0, 50.0, format!("{a} из 100")));
    }
    let fields: Vec<_> = v.pages.iter().flat_map(|p| p.forms.iter()).flat_map(|f| f.fields.iter()).collect();
    if fields.is_empty() {
        out.push(MetricResult::na("Q03", "Форм нет"));
    } else {
        let labeled = fields.iter().filter(|f| f.has_label).count();
        let loose = v.pages.iter().flat_map(|p| p.forms.iter()).map(|f| f.labels).sum::<usize>();
        let detail = if labeled < fields.len() && loose >= fields.len() {
            format!("Подписи связаны с полями у {labeled} из {}: остальные подписи есть, но не привязаны (for/id) — экранный диктор их не прочитает", fields.len())
        } else {
            format!("Подписи у {labeled} из {} полей", fields.len())
        };
        out.push(grade_high("Q03", labeled as f64 / fields.len() as f64, 1.0, 0.5, detail));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inn_checksum() {
        assert!(inn_valid("7707083893")); // Сбербанк
        assert!(!inn_valid("7707083894"));
        assert!(inn_valid("500100732259"));
        assert!(!inn_valid("123"));
    }
}
