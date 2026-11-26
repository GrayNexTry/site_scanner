//! B. Доступность, C. Индексация, D. Скорость, E. Мобильность, O. Безопасность.

use std::collections::{HashMap, HashSet};

use super::{fmt_num, grade_high, grade_low, list_short, CheckCtx, SiteView};
use crate::model::{MetricResult, Status};
use crate::robots;
use crate::site::SiteSnapshot;
use crate::text::{host_of, phones_in_text};

use crate::lexicon::{PARKED, PLACEHOLDER};

/// Сайт не открылся вовсе: B01 и данные домена.
pub fn run_dead(s: &SiteSnapshot, out: &mut Vec<MetricResult>) {
    let error = s.home_error.clone().unwrap_or_else(|| "не открывается".into());
    // Из-за границы (VPN) сайты страны часто молчат; мёртвым считаем только несуществующий домен.
    let surely_dead = error.contains("не существует") || error.contains("некорректный");
    if s.abroad && !surely_dead {
        out.push(MetricResult::warn("B01", format!("Сайт не ответил на зарубежный IP ({error}) — проверьте без VPN")).with_value("blocked"));
    } else {
        out.push(MetricResult::fail("B01", error.clone()));
    }
    domain_checks(s, out);
    // У несуществующего домена почтовых записей нет по определению — O06 к нему неприменим.
    if let Some(d) = s.dns.as_ref().filter(|_| !error.contains("не существует")) {
        out.push(dns_check(d));
    }
}

/// Сайт ответил, но не пустил сканер: B01 — предупреждение, плюс то, что видно без страницы.
pub fn run_blocked(s: &SiteSnapshot, home: &crate::http::Fetched, out: &mut Vec<MetricResult>) {
    let server = home.header("server").map(|x| format!(", защита: {x}")).unwrap_or_default();
    out.push(
        MetricResult::warn(
            "B01",
            format!("Сайт не пустил сканер (ответ {}{server}). Посетители, скорее всего, его видят — проверьте вручную или без VPN", home.status),
        )
        .with_value("blocked"),
    );
    domain_checks(s, out);
    tls_checks(s, home.final_url.starts_with("https://"), out);
    if let Some(d) = &s.dns {
        out.push(dns_check(d));
    }
}

fn tls_checks(s: &SiteSnapshot, https: bool, out: &mut Vec<MetricResult>) {
    // Открылся по http://, но HTTPS у хоста есть — сертификат оцениваем как у https-сайта.
    let alt = !https && (s.https_without_redirect() || s.tls.as_ref().is_some_and(|t| t.valid));
    match (&s.tls, https || alt) {
        (Some(t), false) if t.error.is_some() && t.issuer.is_some() => {
            out.push(MetricResult::fail("B04", format!("HTTPS не работает: {}", t.error.clone().unwrap_or_default())))
        }
        // Сертификат не проверялся (сайт не пустил сканер по http) — не «без HTTPS», а не измерено.
        (None, false) => out.push(MetricResult::na("B04", "HTTPS не проверялся: сайт не пустил сканер")),
        (_, false) => out.push(MetricResult::fail("B04", "Сайт работает без HTTPS")),
        (Some(t), true) => {
            let days = t.days_left.unwrap_or(0);
            let note = match (alt, s.https_hsts()) {
                (true, true) => " — но ссылка ведёт на http://, а сервер на https не перенаправляет (выручает только HSTS у тех, кто уже заходил)",
                (true, false) => " — но ссылка ведёт на http://, перенаправления на https нет",
                _ => "",
            };
            out.push(if s.tls_unread() && !s.tls_lenient {
                // Проба не достучалась (сеть, геоблок), а страница по https открылась со строгой
                // проверкой сертификата: сертификат рабочий, но срок и издателя не узнать.
                MetricResult::na(
                    "B04",
                    format!("Сертификат прочитать не удалось ({}), но сайт по https открывается с проверкой сертификата{note}", t.error.clone().unwrap_or_default()),
                )
            } else if !t.valid {
                MetricResult::fail("B04", format!("Сертификат не проходит проверку: {}", t.error.clone().unwrap_or_default())).with_value(days)
            } else if days < 14 {
                MetricResult::warn("B04", format!("Сертификат истекает через {days} дн.{note}")).with_value(days)
            } else {
                MetricResult::pass("B04", format!("Валиден, {days} дн. до истечения ({}){note}", t.issuer.clone().unwrap_or_default()))
                    .with_value(days)
            });
        }
        (None, true) => out.push(MetricResult::na("B04", "TLS-проба не выполнена")),
    }
    match (&s.tls, https || alt) {
        (Some(t), true) => out.push(match t.protocol.as_deref() {
            Some(p @ ("TLS 1.3" | "TLS 1.2")) => MetricResult::pass("O05", p.to_string()),
            Some(p) => MetricResult::fail("O05", p.to_string()),
            // Протокол не определился (необычный сертификат, сбой рукопожатия) — это не доказательство слабого TLS.
            None => MetricResult::na("O05", "Версию TLS определить не удалось — проверьте вручную"),
        }),
        _ => out.push(MetricResult::na("O05", "Нет HTTPS")),
    }
}

fn domain_checks(s: &SiteSnapshot, out: &mut Vec<MetricResult>) {
    if let Some(d) = &s.domain {
        match d.days_to_expiry {
            Some(days) if days < 0 => out.push(MetricResult::fail("B02", format!("Регистрация истекла {} дн. назад", -days)).with_value(days)),
            Some(days) => out.push(
                MetricResult::new("B02", if days >= 30 { Status::Pass } else { Status::Fail }, format!("{days} дн. до окончания ({})", d.expires.clone().unwrap_or_default()))
                    .with_value(days),
            ),
            None => out.push(MetricResult::na("B02", d.error.clone().unwrap_or_else(|| "Дата окончания неизвестна".into()))),
        }
        if let Some(age) = d.age_years {
            out.push(MetricResult::info("B03", format!("{} лет (с {})", fmt_num((age * 10.0).round() / 10.0), d.created.clone().unwrap_or_default())).with_value(age));
        }
    }
}

/// B13: чёрные списки Google Safe Browsing (нужен ключ) и Яндекса (без ключа).
fn blacklists(s: &SiteSnapshot) -> MetricResult {
    let sources = [("Google Safe Browsing", &s.safe_browsing, "нужен ключ (Настройки)"), ("Яндекс", &s.yandex_safety, "не спрашивался")];
    let mut threats = Vec::new();
    let mut clean = Vec::new();
    let mut unchecked = Vec::new();
    for (name, r, why_none) in sources {
        match r {
            Some(Ok(m)) if m.is_empty() => clean.push(name.to_string()),
            Some(Ok(m)) => threats.push(format!("{name}: {}", m.join(", "))),
            Some(Err(e)) => unchecked.push(format!("{name} — {e}")),
            None => unchecked.push(format!("{name} — {why_none}")),
        }
    }
    let rest = if unchecked.is_empty() { String::new() } else { format!(". Не проверено: {}", unchecked.join("; ")) };
    if !threats.is_empty() {
        MetricResult::fail("B13", format!("В чёрном списке: {}{rest}", threats.join("; ")))
    } else if !clean.is_empty() {
        MetricResult::pass("B13", format!("В списках нет: {}{rest}", clean.join(", ")))
    } else {
        MetricResult::na("B13", format!("Не проверено: {}", unchecked.join("; ")))
    }
}

fn dns_check(d: &crate::domain_info::DnsInfo) -> MetricResult {
    // Поддомен конструктора (wixsite.com, tilda.ws): почтой и DNS управляет платформа.
    if d.platform {
        return MetricResult::na("O06", "Сайт на поддомене конструктора — SPF и DMARC настраивает платформа");
    }
    if let Some(e) = &d.error {
        return MetricResult::na("O06", format!("DNS не ответил: {e}"));
    }
    match (&d.spf, &d.dmarc) {
        (Some(_), Some(_)) => MetricResult::pass("O06", "Есть SPF и DMARC"),
        (Some(_), None) => MetricResult::warn("O06", "Есть SPF, нет DMARC"),
        (None, Some(_)) => MetricResult::warn("O06", "Есть DMARC, нет SPF"),
        (None, None) => MetricResult::fail("O06", "Нет SPF и DMARC: письма с форм могут уходить в спам"),
    }
}

/// Вместо главной — окно «Вам есть 18?» (плагин Age Gate и т. п.): за ним настоящий сайт,
/// а метрики по окну были бы ложными.
fn age_gate(page: &crate::page::PageInfo) -> bool {
    let h = &page.html_lower;
    page.words < 150
        && (["age-gate", "agegate", "age_gate", "age-verification", "ageverification"].iter().any(|m| h.contains(m))
            || [
                "вам есть 18", "вам исполнилось 18", "вы старше 18", "are you over 18", "are you 18", "are you over 21", "are you 21",
                "legal drinking age", "avez-vous 18 ans", "plus de 18 ans", "êtes-vous majeur", "mayor de edad", "eres mayor de 18",
                "tienes 18", "sind sie 18", "über 18", "mindestens 18", "volljährig", "maggiorenne", "hai 18 anni", "pełnoletni",
                "masz 18 lat", "maior de 18", "tem 18 anos", "18 jaar of ouder",
            ]
            .iter()
            .any(|m| page.text_lower().contains(m)))
}

pub fn run(ctx: &CheckCtx, v: &SiteView, out: &mut Vec<MetricResult>) {
    let s = v.snap;
    let home = v.home;
    let page = v.page;
    let text = page.text_lower();

    // ---------- B ----------
    let parked = PARKED
        .iter()
        // Фразы парковки в тексте («domain for sale», «account suspended», «renew your subscription»)
        // бывают и в настоящих статьях — по тексту только на короткой странице; по заголовку — всегда.
        .find(|m| (page.words < 300 && text.contains(*m)) || page.title.as_deref().is_some_and(|t| t.to_lowercase().contains(*m)))
        .or_else(|| ["forsale.godaddy", "window.location.href=\"/lander\"", "/lander\""].iter().find(|m| page.html_lower.contains(*m)));
    let title = page.title.as_deref().unwrap_or("").to_lowercase();
    let placeholder = (page.words < 120).then(|| PLACEHOLDER.iter().find(|m| text.contains(*m) || title.contains(*m))).flatten();
    // Вместо главной — ошибка движка или базы («No connect!», «Error establishing a database connection»).
    let site_error = (page.words < 60)
        .then(|| crate::lexicon::SITE_ERRORS.iter().find(|m| text.contains(*m) || title.contains(*m)))
        .flatten()
        .map(|m| m.to_string())
        .or_else(|| {
            // Сайт на фреймах (trodat-bel.by) — содержимое во фреймах, главная не пустая. Скрипты
            // и ссылки не оправдание: почти пустую главную рендерим, и если текста не прибавилось — пустая.
            let rendered_something = s.js_title.is_some() || s.js_h1.is_some() || s.raw_words.is_some() || page.html_lower.contains("<frame");
            // Заставка «Enter» со ссылкой внутрь сайта — не пустая главная (обход идёт по ссылке дальше).
            let internal_links = page.links.iter().filter(|l| l.internal).count();
            (page.words < 5 && internal_links == 0 && page.links.len() < 3 && !s.js_shell && !rendered_something).then(|| page.text.trim().to_string())
        });
    out.push(if !home.ok() && parked.is_some() && page.words < 120 {
        // Адрес из карточки ведёт на страницу хостера («Сайт не обслуживается», 404): сайта по факту нет — заглушка.
        MetricResult::fail("B01", format!("Вместо сайта — заглушка хостинга «{}» (ответ {})", parked.unwrap_or(&""), home.status)).with_value(home.status)
    } else if !home.ok() {
        MetricResult::fail("B01", format!("Главная отвечает {}", home.status)).with_value(home.status)
    } else if let Some(m) = parked {
        MetricResult::fail("B01", format!("Похоже на заглушку хостинга или парковку домена: «{m}»"))
    } else if let Some(m) = placeholder {
        MetricResult::fail("B01", format!("Вместо сайта — заглушка «{m}»"))
    } else if age_gate(page) {
        MetricResult::warn("B01", "Сайт закрыт подтверждением возраста (18+) — содержимое не проверялось, проверить вручную").with_value("blocked")
    } else if let Some(e) = site_error {
        MetricResult::fail("B01", if e.is_empty() { "Главная открывается пустой".to_string() } else { format!("Вместо главной — ошибка сайта: «{}»", crate::text::truncate(&e, 60)) })
    } else if let Some(code) = s.input_broken {
        MetricResult::warn("B01", format!("Ссылка из карточки ведёт на страницу с ошибкой {code}; главная сайта открывается")).with_value("broken_link")
    } else if home.total_ms > 5000 && !s.abroad && !s.via_proxy {
        MetricResult::warn("B01", format!("Открывается, но медленно: {} мс", home.total_ms)).with_value(home.total_ms)
    } else {
        MetricResult::pass("B01", format!("200 за {} мс", home.total_ms)).with_value(home.total_ms)
    });
    domain_checks(s, out);
    tls_checks(s, v.https(), out);

    // B05: http → https.
    if !v.https() && s.https_without_redirect() && s.https_hsts() {
        out.push(
            MetricResult::warn(
                "B05",
                "Сервер не перенаправляет http:// на https://: браузеры, уже открывавшие https-версию, переходят сами (HSTS), а новый посетитель по ссылке из карточки увидит «Не защищено»",
            )
            .with_value("no_redirect_hsts"),
        );
    } else if !v.https() && s.https_without_redirect() {
        out.push(
            MetricResult::fail("B05", "HTTPS есть, но http:// не перенаправляет на него — по такой ссылке (в том числе из карточки) браузер пишет «Не защищено»")
                .with_value("no_redirect"),
        );
    } else if !v.https() && s.tls.as_ref().is_some_and(|t| t.error.is_some() && t.issuer.is_some()) {
        // HTTPS отвечает, но сертификат просрочен или чужой — «только по HTTP» звучало бы неверно.
        out.push(MetricResult::fail("B05", "Перенаправления на https нет, а сам HTTPS не работает — сертификат не проходит проверку (см. B04)"));
    } else if !v.https() {
        out.push(MetricResult::fail("B05", "Сайт отдаётся только по HTTP"));
    } else if let Some(p) = &s.http_probe {
        let to_https = p.final_url.as_deref().is_some_and(|u| u.starts_with("https://"));
        out.push(match (p.first_status, &p.error) {
            (_, Some(e)) => MetricResult::warn("B05", format!("http:// не отвечает: {e}")),
            (Some(301 | 308), _) if to_https => MetricResult::pass("B05", "Постоянный редирект на HTTPS"),
            (Some(302 | 303 | 307), _) if to_https => MetricResult::warn("B05", "Редирект на HTTPS временный (302/307), нужен 301"),
            (Some(st), _) => MetricResult::fail("B05", format!("http:// отвечает {st} без перехода на HTTPS")),
            _ => MetricResult::na("B05", "Нет данных"),
        });
    }

    // B06: зеркала www / без www.
    if let Some(p) = &s.mirror_probe {
        let main_host = host_of(&v.final_url);
        let lands_main = p.final_url.as_deref().and_then(|u| url::Url::parse(u).ok()).is_some_and(|u| {
            u.host_str().unwrap_or("") == v.final_url.host_str().unwrap_or("")
        });
        out.push(if p.error.is_some() {
            MetricResult::pass("B06", "Второе зеркало не отвечает — дублей нет")
        } else if lands_main {
            MetricResult::pass("B06", format!("Зеркало склеено с {main_host}"))
        } else if p.final_status == Some(200) {
            MetricResult::fail("B06", format!("{} отдаёт 200 отдельно от основного адреса — дубль сайта", p.url))
        } else {
            MetricResult::warn("B06", format!("{} отвечает {:?}", p.url, p.final_status))
        });
    }

    // B07: цепочка редиректов главной.
    let hops = home.chain.len();
    let chain_desc = home.chain.iter().map(|h| format!("{} {}", h.status, h.url)).collect::<Vec<_>>().join(" → ");
    out.push(
        MetricResult::new(
            "B07",
            match hops { 0 | 1 => Status::Pass, 2 => Status::Warn, _ => Status::Fail },
            if hops == 0 { "Без редиректов".into() } else { format!("{hops}: {chain_desc} → {}", home.final_url) },
        )
        .with_value(hops),
    );
    // Через VPN или из другой страны к времени ответа добавляется дорога: меряем не сайт.
    out.push(if s.abroad || s.via_proxy {
        MetricResult::na("B08", format!("{} мс, но скан шёл через VPN или из другой страны — замер недостоверен", home.ttfb_ms))
    } else {
        grade_low("B08", home.ttfb_ms as f64, 800.0, 1800.0, format!("{} мс", home.ttfb_ms))
    });
    match s.compression.as_deref() {
        Some(e) if ["br", "gzip", "zstd", "deflate"].iter().any(|x| e.contains(x)) => {
            out.push(MetricResult::pass("B09", e.to_string()))
        }
        Some(_) if home.body_bytes > 2048 => out.push(MetricResult::fail("B09", "HTML отдаётся без сжатия")),
        Some(_) => out.push(MetricResult::pass("B09", "Страница слишком мала для сжатия")),
        None => out.push(MetricResult::na("B09", "Не удалось проверить")),
    }
    let h2 = home.http_version.contains("HTTP/2") || home.http_version.contains("HTTP/3");
    let alt_svc_h3 = home.header("alt-svc").is_some_and(|a| a.contains("h3"));
    out.push(if !h2 && !home.final_url.starts_with("https://") {
        // HTTP/2 в браузерах бывает только по https: на http-ссылке его не измерить.
        MetricResult::na("B10", "Сайт открылся по http:// — HTTP/2 проверяется только по https")
    } else if h2 {
        MetricResult::pass("B10", format!("{}{}", home.http_version, if alt_svc_h3 { ", объявлен HTTP/3" } else { "" }))
    } else {
        MetricResult::fail("B10", format!("{} — нет HTTP/2", home.http_version))
    });
    if s.assets.is_empty() {
        out.push(MetricResult::na("B11", "Статика не найдена"));
    } else {
        let month = 30 * 86_400;
        let good = s.assets.iter().filter(|a| a.max_age.unwrap_or(0) >= month).count();
        let some = s.assets.iter().filter(|a| a.max_age.unwrap_or(0) > 0).count();
        let detail = s
            .assets
            .iter()
            .map(|a| format!("{}: {}", a.url.rsplit('/').next().unwrap_or(""), a.cache_control.clone().unwrap_or_else(|| "без Cache-Control".into())))
            .collect::<Vec<_>>()
            .join("; ");
        out.push(MetricResult::new(
            "B11",
            if good == s.assets.len() { Status::Pass } else if some > 0 { Status::Warn } else { Status::Fail },
            crate::text::truncate(&detail, 300),
        ));
    }
    if let Some(p) = &s.not_found {
        out.push(match (p.final_status, p.hops, &p.error) {
            (_, _, Some(e)) => MetricResult::na("B12", e.clone()),
            (Some(404 | 410), _, _) => MetricResult::pass("B12", "Несуществующая страница отдаёт 404"),
            (Some(200), 0, _) => MetricResult::fail("B12", "Несуществующая страница отдаёт 200 (soft 404)"),
            (Some(200), _, _) => MetricResult::warn("B12", "Несуществующая страница перенаправляется на существующую"),
            (Some(st), _, _) => MetricResult::warn("B12", format!("Несуществующая страница отдаёт {st}")),
            _ => MetricResult::na("B12", "Нет данных"),
        });
    }
    out.push(blacklists(s));

    // ---------- C ----------
    let robots_body = v.robots();
    let dirs = s.robots_directives.as_ref();
    match &s.robots {
        Some(r) => out.push(match r.status {
            Some(200) if r.body.trim_start().starts_with('<') => MetricResult::warn("C01", "Вместо robots.txt отдаётся HTML"),
            Some(200) if r.bytes > 500 * 1024 => MetricResult::fail("C01", format!("{} КиБ — Google читает только 500 КиБ", r.bytes / 1024)),
            Some(200) => {
                let unknown = dirs.map(|d| d.unknown_lines).unwrap_or(0);
                if unknown > 3 {
                    MetricResult::warn("C01", format!("Есть {unknown} непонятных строк"))
                } else {
                    MetricResult::pass("C01", format!("{} байт", r.bytes))
                }
            }
            Some(404 | 410) => MetricResult::warn("C01", "robots.txt нет (всё разрешено)"),
            Some(st) if st >= 500 => MetricResult::fail("C01", format!("robots.txt отвечает {st}: Google считает сайт закрытым")),
            Some(st) => MetricResult::warn("C01", format!("robots.txt отвечает {st}")),
            None => MetricResult::fail("C01", format!("robots.txt не загрузился: {}", r.error.clone().unwrap_or_default())),
        }),
        None => out.push(MetricResult::na("C01", "Нет данных")),
    }
    let home_page = robots::page_url(&home.final_url);
    let home_url = home_page.as_str();
    match robots_body {
        Some(body) => {
            let mut blocked = Vec::new();
            // Яндекс важен в русскоязычных странах и Турции; в остальных — Bing (на нём поиск ChatGPT, DuckDuckGo).
            let agents: &[&str] = if crate::region::yandex_market(ctx.params.country.as_deref()) { &["Googlebot", "Yandex"] } else { &["Googlebot", "Bingbot"] };
            for &agent in agents {
                if !robots::allowed(body, agent, home_url) {
                    blocked.push(agent);
                }
            }
            out.push(if blocked.is_empty() {
                MetricResult::pass("C02", format!("Главная открыта для {}", agents.join(" и ")))
            } else {
                MetricResult::fail("C02", format!("Главная закрыта для: {}", blocked.join(", ")))
            });
            let assets: Vec<String> = page
                .stylesheets
                .iter()
                .map(|(u, _)| u.clone())
                .chain(page.scripts.iter().filter_map(|x| x.src.clone()))
                .filter(|u| u.contains(v.final_url.host_str().unwrap_or("-")))
                .take(12)
                .collect();
            // Стили для печати отрисовке страницы не нужны.
            let blocked_assets: Vec<String> =
                assets.into_iter().filter(|u| !u.to_lowercase().contains("print") && !robots::allowed(body, "Googlebot", u)).collect();
            out.push(if blocked_assets.is_empty() {
                MetricResult::pass("C03", "CSS и JS доступны Googlebot")
            } else {
                MetricResult::fail("C03", format!("Закрыто от Googlebot: {}", list_short(&blocked_assets, 3)))
            });
        }
        None => {
            out.push(MetricResult::pass("C02", "robots.txt нет или недоступен — индексация не запрещена"));
            out.push(MetricResult::pass("C03", "Ресурсы не закрыты"));
        }
    }
    if let Some(d) = dirs {
        out.push(if d.sitemaps.is_empty() {
            MetricResult::warn("C04", "В robots.txt нет строки Sitemap:")
        } else {
            MetricResult::pass("C04", list_short(&d.sitemaps, 2))
        });
        let mut old = Vec::new();
        if d.host {
            old.push("Host (Яндекс не читает с 2018)");
        }
        if d.crawl_delay {
            old.push("Crawl-delay (Google игнорирует)");
        }
        let clean = if d.clean_param { " Clean-param есть." } else { "" };
        // Внутренние ссылки с метками: Яндекс без Clean-param считает такие адреса дублями.
        // Страница, открытая по ссылке с метками (из карточки), передаёт их относительным ссылкам
        // сама — такие страницы не считаем.
        let has_tag = |h: &str| ["utm_", "yclid=", "gclid=", "fbclid=", "_openstat="].iter().any(|m| h.to_lowercase().contains(m));
        let tagged: Vec<String> = v
            .pages
            .iter()
            .filter(|p| !has_tag(&p.url))
            .flat_map(|p| p.links.iter().filter(|l| l.internal))
            .map(|l| l.href.clone())
            .filter(|h| has_tag(h))
            .collect();
        out.push(if !old.is_empty() {
            MetricResult::warn("C05", format!("Устаревшие директивы: {}.{clean}", old.join(", ")))
        } else if !d.clean_param && !tagged.is_empty() {
            MetricResult::warn("C05", format!("Внутренние ссылки с метками ({}) — без Clean-param Яндекс считает их дублями страниц", list_short(&tagged, 1)))
        } else {
            MetricResult::pass("C05", format!("Устаревших директив нет.{clean}"))
        });
    } else {
        out.push(MetricResult::warn("C04", "robots.txt нет — Sitemap не объявлен"));
        out.push(MetricResult::na("C05", "robots.txt нет"));
    }

    let sm = &s.sitemap;
    out.push(if sm.too_big {
        MetricResult::fail("C06", "В одном sitemap больше 50 000 URL")
    } else if sm.found() && !sm.foreign.is_empty() {
        MetricResult::warn("C06", format!("Свой sitemap есть ({} URL), но robots.txt указывает на чужой: {}", sm.urls.len(), sm.foreign.join(", ")))
    } else if sm.nested_in_urlset && !sm.urls.is_empty() {
        MetricResult::warn("C06", format!("{} URL, но индекс оформлен как urlset со ссылками на другие sitemap — поисковики его так не читают, нужен sitemapindex", sm.urls.len()))
            .with_value(sm.urls.len())
    } else if sm.found() {
        MetricResult::pass("C06", format!("{} URL в {} файлах", sm.urls.len(), sm.fetched.len())).with_value(sm.urls.len())
    } else if let Some((u, st, _)) = sm.fetched.first() {
        match st {
            Some(200) => MetricResult::fail("C06", format!("{u}: не похоже на sitemap (ошибка XML)")),
            Some(st) => MetricResult::fail("C06", format!("{u} отвечает {st}")),
            None => MetricResult::fail("C06", format!("{u} не загрузился")),
        }
    } else {
        MetricResult::fail("C06", "sitemap.xml не найден")
    });
    // 403/429/503 — антибот и перегрузка, а не плохой адрес в sitemap.
    let responded: Vec<_> = s.sitemap_checks.iter().filter(|c| !c.unreachable() && !matches!(c.final_status, Some(403 | 429 | 503))).collect();
    let silent = s.sitemap_checks.len() - responded.len();
    if responded.is_empty() {
        out.push(MetricResult::na("C07", if s.sitemap_checks.is_empty() { "Нет URL для проверки".to_string() } else { format!("Сайт не ответил на {silent} проверок — похоже на защиту от частых запросов") }));
    } else {
        let ok = responded.iter().filter(|c| c.final_status == Some(200) && !c.redirected).count();
        let share = ok as f64 / responded.len() as f64;
        let bad: Vec<String> = responded
            .iter()
            .filter(|c| !(c.final_status == Some(200) && !c.redirected))
            .map(|c| format!("{} → {}{}", c.url, c.final_status.unwrap_or(0), if c.redirected { " (редирект)" } else { "" }))
            .collect();
        let detail = format!(
            "{ok} из {} отвечают 200 без редиректа{}{}",
            responded.len(),
            if silent > 0 { format!(", ещё {silent} не ответили") } else { String::new() },
            if bad.is_empty() { String::new() } else { format!(". Например: {}", list_short(&bad, 2)) }
        );
        out.push(grade_high("C07", share, 0.95, 0.8, detail));
    }
    if sm.urls.is_empty() {
        out.push(MetricResult::na("C08", "sitemap пуст или не найден"));
    } else {
        let share = sm.lastmods.len() as f64 / sm.urls.len() as f64;
        let distinct: HashSet<&str> = sm.lastmods.iter().map(|l| l.get(..10).unwrap_or(l)).collect();
        let today = chrono::Utc::now().date_naive().to_string();
        out.push(if sm.lastmods.is_empty() {
            MetricResult::warn("C08", "lastmod не указан")
        } else if sm.urls.len() > 20 && distinct.len() == 1 {
            let d = distinct.iter().next().copied().unwrap_or("");
            MetricResult::warn("C08", if d == today { "У всех URL сегодняшняя дата — lastmod фиктивный".to_string() } else { format!("У всех URL одна дата: {d}") })
        } else if share < 0.5 {
            MetricResult::warn("C08", format!("lastmod есть у {:.0}% URL", share * 100.0))
        } else {
            MetricResult::pass("C08", format!("lastmod у {:.0}% URL, {} разных дат", share * 100.0, distinct.len()))
        });
    }
    // Сравниваем без схемы, www, слеша на конце и с одинаковой кодировкой кириллицы.
    let norm = |u: &str| match url::Url::parse(u.trim()) {
        Ok(p) => {
            let query: Vec<String> = p
                .query_pairs()
                .filter(|(k, _)| !k.starts_with("utm_") && !["gclid", "yclid", "fbclid", "_openstat", "roistat"].contains(&k.as_ref()))
                .map(|(k, v)| format!("{k}={v}"))
                .collect();
            let query = if query.is_empty() { String::new() } else { format!("?{}", query.join("&")) };
            format!("{}{}{}", p.host_str().unwrap_or("").trim_start_matches("www."), p.path(), query)
                .trim_end_matches('/')
                .to_lowercase()
        }
        Err(_) => u.trim().trim_end_matches('/').to_lowercase(),
    };
    let sm_set: HashSet<String> = sm.urls.iter().map(|u| norm(u)).collect();
    // Страница с canonical на другой адрес (фильтр каталога «/catalog?tfc_sort=…» → /catalog) —
    // в sitemap должен быть адрес из canonical.
    let crawled_ok: Vec<String> = s
        .pages
        .iter()
        .filter(|p| p.status == Some(200) && !p.redirected && p.info.as_ref().is_some_and(|i| !i.has_noindex()))
        .filter(|p| !["cart", "basket", "korzin", "wishlist", "compare", "login", "auth", "account", "search", "checkout"].iter().any(|k| p.final_url.to_lowercase().contains(k)))
        .map(|p| {
            let own = |c: &String| url::Url::parse(c).ok().and_then(|u| u.host_str().map(|h| h.trim_start_matches("www.").to_string()))
                == url::Url::parse(&p.final_url).ok().and_then(|u| u.host_str().map(|h| h.trim_start_matches("www.").to_string()));
            p.info.as_ref().and_then(|i| i.canonical.clone()).filter(own).unwrap_or_else(|| p.final_url.clone())
        })
        .collect();
    if sm_set.is_empty() || crawled_ok.is_empty() {
        out.push(MetricResult::na("C09", "Нет sitemap или обойдённых страниц"));
    } else if sm.partial {
        out.push(MetricResult::na("C09", format!("sitemap прочитан частично ({} URL из первых файлов индекса) — покрытие не оценить", sm.urls.len())));
    } else {
        let inside = crawled_ok.iter().filter(|u| sm_set.contains(&norm(u))).count();
        let share = inside as f64 / crawled_ok.len() as f64;
        out.push(grade_high("C09", share, 0.95, 0.7, format!("{inside} из {} найденных страниц есть в sitemap", crawled_ok.len())));
    }

    let x_robots: Vec<&str> = home.headers_all("x-robots-tag").collect();
    let home_noindex = page.has_noindex() || x_robots.iter().any(|x| x.to_lowercase().contains("noindex"));
    let inner_noindex: Vec<String> =
        s.pages.iter().filter(|p| p.info.as_ref().is_some_and(|i| i.has_noindex())).map(|p| p.final_url.clone()).collect();
    out.push(if home_noindex {
        MetricResult::fail("C10", "На главной стоит noindex — сайт выпадет из поиска")
    } else if !inner_noindex.is_empty() {
        MetricResult::warn("C10", format!("noindex на страницах: {}", list_short(&inner_noindex, 3)))
    } else {
        MetricResult::pass("C10", "noindex на важных страницах нет")
    });

    out.push(match &page.canonical {
        None => MetricResult::warn("C11", "На главной нет canonical"),
        Some(c) => {
            // Ссылка с метками (?utm_…) и canonical на ту же страницу без них — так и должно быть.
            let same = norm(c) == norm(&home.final_url) || norm(&robots::page_url(c)) == norm(&robots::page_url(&home.final_url));
            let other_host = url::Url::parse(c).ok().map(|u| host_of(&u)) != Some(host_of(&v.final_url));
            if same {
                MetricResult::pass("C11", "canonical указывает на саму страницу")
            } else if other_host {
                MetricResult::fail("C11", format!("canonical ведёт на другой домен: {c}"))
            } else {
                MetricResult::warn("C11", format!("canonical указывает на другую страницу: {c}"))
            }
        }
    });

    // Битая ссылка — только ответ 4xx/5xx; таймауты и обрывы считаем отдельно, это не ошибка сайта.
    let mut broken: Vec<String> = s
        .pages
        .iter()
        // 429 и 503 — «слишком много запросов» и временная перегрузка, а не битая ссылка;
        // 401 — страница под паролем (belodent.by/prices/): закрыта намеренно, не битая.
        .filter(|p| p.status.is_some_and(|st| st >= 400 && !matches!(st, 401 | 403 | 429 | 503)) && !p.url.contains("/cdn-cgi/"))
        .map(|p| format!("{} ({})", p.url, p.status.unwrap_or(0)))
        .collect();
    broken.extend(s.link_checks.iter().filter(|c| c.http_error() && !matches!(c.final_status, Some(401 | 403 | 429 | 503)) && !c.url.contains("/cdn-cgi/")).map(|c| format!("{} ({})", c.url, c.final_status.unwrap_or(0))));
    let silent = s.pages.iter().filter(|p| p.status.is_none()).count() + s.link_checks.iter().filter(|c| c.unreachable()).count();
    let checked = s.pages.len() + s.link_checks.len() - silent;
    let silent_note = match (silent, s.throttled) {
        (0, _) => String::new(),
        (n, true) => format!(" Сайт перестал отвечать на частые запросы ({n} без ответа) — обход остановлен."),
        (n, false) => format!(" Без ответа: {n}."),
    };
    if checked == 0 {
        let why = if silent > 0 { format!("Ссылки не ответили.{silent_note}") } else { "Внутренних ссылок не найдено".to_string() };
        out.push(MetricResult::na("C12", why.clone()));
        out.push(MetricResult::na("C13", why));
    } else {
        out.push(if broken.is_empty() {
            MetricResult::pass("C12", format!("Проверено {checked} ссылок, битых нет.{silent_note}")).with_value(0)
        } else {
            MetricResult::fail("C12", format!("Битых: {} из {checked}: {}.{silent_note}", broken.len(), list_short(&broken, 3))).with_value(broken.len())
        });
        let redirected = s.pages.iter().filter(|p| p.redirected).count() + s.link_checks.iter().filter(|c| c.redirected).count();
        let share = redirected as f64 / checked as f64;
        out.push(
            MetricResult::new("C13", if share < 0.05 { Status::Pass } else { Status::Warn }, format!("{redirected} из {checked} ссылок ведут через редирект"))
                .with_value(share),
        );
    }

    let mut titles: HashMap<String, usize> = HashMap::new();
    let mut descs: HashMap<String, usize> = HashMap::new();
    for p in &v.pages {
        if let Some(t) = &p.title {
            *titles.entry(t.to_lowercase()).or_default() += 1;
        }
        if let Some(d) = &p.description {
            *descs.entry(d.to_lowercase()).or_default() += 1;
        }
    }
    let dup_t = titles.values().filter(|c| **c > 1).map(|c| c - 1).sum::<usize>();
    let dup_d = descs.values().filter(|c| **c > 1).map(|c| c - 1).sum::<usize>();
    // Сколько страниц делят повторяющиеся заголовки / описания (с «первой» из каждой группы).
    let pages_t = titles.values().filter(|c| **c > 1).sum::<usize>();
    let pages_d = descs.values().filter(|c| **c > 1).sum::<usize>();
    if v.pages.len() < 2 {
        out.push(MetricResult::na("C14", "Обойдена одна страница"));
    } else {
        out.push(
            MetricResult::new(
                "C14",
                match dup_t + dup_d { 0 => Status::Pass, 1..=2 => Status::Warn, _ => Status::Fail },
                format!("Повторов title: {dup_t}, description: {dup_d} на {} страницах (одинаковый title у {pages_t} стр., description у {pages_d} стр.)", v.pages.len()),
            )
            .with_value(dup_t + dup_d),
        );
    }

    // C16: сиротские страницы — только если сайт обойдён целиком.
    if !sm.urls.is_empty() && sm.urls.len() <= s.pages.len() + 1 {
        let linked: HashSet<String> = v.pages.iter().flat_map(|p| p.links.iter().filter(|l| l.internal).map(|l| norm(&l.href))).collect();
        let orphans: Vec<String> = sm.urls.iter().filter(|u| !linked.contains(&norm(u)) && norm(u) != norm(&home.final_url)).cloned().collect();
        out.push(if orphans.is_empty() {
            MetricResult::pass("C16", "Все страницы из sitemap доступны по ссылкам")
        } else {
            MetricResult::warn("C16", format!("Без внутренних ссылок: {}", list_short(&orphans, 3))).with_value(orphans.len())
        });
    }

    let yandex_market = crate::region::yandex_market(ctx.params.country.as_deref());
    out.push(if page.verification.is_empty() {
        MetricResult::warn("C21", "Метатегов подтверждения Вебмастера и Search Console нет")
    } else if yandex_market && !page.verification.iter().any(|x| x == "yandex-verification") {
        MetricResult::warn(
            "C21",
            format!("Есть {}, но нет yandex-verification: Яндекс Вебмастер, похоже, не подключён (если подтверждали файлом или DNS — проверьте вручную)", page.verification.join(", ")),
        )
    } else {
        MetricResult::pass("C21", page.verification.join(", "))
    });
    if page.hreflang.is_empty() {
        out.push(MetricResult::na("C24", "Не применимо: hreflang нет"));
    } else {
        let has_self = page.hreflang.iter().any(|(l, h)| l == "x-default" || norm(h) == norm(&home.final_url));
        out.push(if has_self {
            MetricResult::pass("C24", format!("{} языковых версий", page.hreflang.len()))
        } else {
            MetricResult::warn("C24", "hreflang не ссылается на саму страницу и нет x-default")
        });
    }
    let urls: Vec<&str> = s.pages.iter().map(|p| p.final_url.as_str()).chain(sm.urls.iter().map(String::as_str)).collect();
    if !urls.is_empty() {
        let dirty = urls
            .iter()
            .filter(|u| u.contains('?') || u.contains("jsessionid") || u.contains("index.php") || u.contains("&id=") || u.contains("?id="))
            .count();
        let share = dirty as f64 / urls.len() as f64;
        // Адреса «нечитаемые» — когда таких большинство (порог 50 %); меньше — только у части страниц.
        out.push(
            MetricResult::new("C25", if share < 0.1 { Status::Pass } else { Status::Warn }, format!("С параметрами: {dirty} из {}", urls.len()))
                .with_value(share),
        );
    }

    // ---------- D ----------
    speed(v, out);

    // ---------- E ----------
    out.push(match page.viewport.as_deref() {
        None => MetricResult::fail("E01", "Нет meta viewport — на телефоне будет мелкая десктопная версия"),
        Some(vp) => {
            let vp_l = vp.to_lowercase().replace(' ', "");
            if vp_l.contains("user-scalable=no") || vp_l.contains("user-scalable=0") || vp_l.contains("maximum-scale=1,") || vp_l.ends_with("maximum-scale=1") || vp_l.contains("maximum-scale=1.0") {
                // viewport есть, мобильная версия работает — запрещён только щипок-масштаб.
                MetricResult::warn("E01", format!("Масштабирование запрещено: {vp}")).with_value("no_zoom")
            } else if vp_l.contains("width=device-width") {
                MetricResult::pass("E01", vp.to_string())
            } else {
                MetricResult::warn("E01", format!("Нестандартный viewport: {vp}")).with_value("odd_viewport")
            }
        }
    });
    out.push(if !page.tel_links.is_empty() {
        MetricResult::pass("E05", format!("tel: {}", page.tel_links[0]))
    } else if !phones_in_text(&page.text).is_empty() {
        MetricResult::fail("E05", "Телефон на странице есть, но не кликабелен")
    } else {
        // Нет телефона вовсе — это H01 и N02; E05 оценивает только кликабельность.
        MetricResult::na("E05", "Телефона на главной нет — см. N02")
    });

    // ---------- O ----------
    let has = |h: &str| home.header(h).is_some();
    let csp = home.header("content-security-policy").unwrap_or("");
    // Сайт открылся по http:// без перенаправления, а https-версия отдаёт HSTS (belarusmodels.by,
    // centercomfort.by): по http браузер HSTS не принимает, но заголовок у сайта есть — беда в
    // перенаправлении (B05), а не в заголовке.
    let hsts = has("strict-transport-security") || (!v.https() && s.https_hsts());
    let checks = [
        ("HSTS", hsts),
        ("CSP", !csp.is_empty()),
        ("X-Content-Type-Options", has("x-content-type-options")),
        ("X-Frame-Options/frame-ancestors", has("x-frame-options") || csp.contains("frame-ancestors")),
        ("Referrer-Policy", has("referrer-policy")),
    ];
    let present = checks.iter().filter(|c| c.1).count();
    let missing: Vec<&str> = checks.iter().filter(|c| !c.1).map(|c| c.0).collect();
    let grade = ["F", "E", "D", "C", "B", "A"][present];
    out.push(
        MetricResult::new(
            "O01",
            match present { 4 | 5 => Status::Pass, 2 | 3 => Status::Warn, _ => Status::Fail },
            if missing.is_empty() { format!("{grade}: все 5 заголовков").to_string() } else { format!("{grade}: нет {}", missing.join(", ")) },
        )
        .with_value(grade),
    );
    // CSP upgrade-insecure-requests: браузер сам переводит http-адреса на https — смешанного контента нет.
    let upgrade = page.html_lower.contains("upgrade-insecure-requests")
        || home.headers.iter().any(|(k, val)| k == "content-security-policy" && val.to_lowercase().contains("upgrade-insecure-requests"));
    if v.https() && upgrade && !(page.mixed_active.is_empty() && page.mixed_passive.is_empty()) {
        out.push(MetricResult::pass("O02", "Ссылки по http:// есть, но CSP upgrade-insecure-requests переводит их на https"));
    } else if v.https() {
        out.push(if !page.mixed_active.is_empty() {
            MetricResult::fail("O02", format!("Скрипты/стили по http://: {}", list_short(&page.mixed_active, 2)))
        } else if !page.mixed_passive.is_empty() {
            MetricResult::warn("O02", format!("Картинки по http://: {}", list_short(&page.mixed_passive, 2)))
        } else {
            MetricResult::pass("O02", "Смешанного контента нет")
        });
    }
    out.push(cms_check(v));
    // Слова, которые есть в названии или категории самой компании (ресторан при казино Royal),
    // спамом не считаются.
    let own = format!(
        "{} {} {} {}",
        ctx.business.name,
        ctx.business.category.as_deref().unwrap_or(""),
        page.title.as_deref().unwrap_or(""),
        v.final_url.host_str().unwrap_or("")
    )
    .to_lowercase();
    let spam_words: Vec<&str> =
        ["casino", "казино", "1xbet", "вулкан", "viagra", "виагра", "porn", "порно", "букмекер", "slot"]
            .into_iter()
            .filter(|w| !own.contains(w) && !((*w == "casino" && own.contains("казино")) || (*w == "казино" && own.contains("casino"))))
            .collect();
    let spam: Vec<String> = page
        .links
        .iter()
        .filter(|l| !l.internal)
        .filter(|l| {
            let t = format!("{} {}", l.href, l.text).to_lowercase();
            spam_words.iter().any(|w| t.contains(w))
        })
        .map(|l| l.href.clone())
        .collect();
    out.push(if spam.len() >= 2 {
        MetricResult::fail("O04", format!("Подозрительные ссылки: {}", list_short(&spam, 3)))
    } else {
        MetricResult::pass("O04", "Спам-ссылок не найдено")
    });
    if let Some(d) = &s.dns {
        out.push(dns_check(d));
    }
}

fn speed(v: &SiteView, out: &mut Vec<MetricResult>) {
    let page = v.page;
    let host = v.final_url.host_str().unwrap_or("");
    let ext_hosts: HashSet<String> = page
        .scripts
        .iter()
        .filter_map(|x| x.src.as_deref())
        .filter_map(|u| url::Url::parse(u).ok())
        .filter_map(|u| u.host_str().map(str::to_string))
        .filter(|h| h != host)
        .collect();
    let psi = v.snap.psi.as_ref().filter(|p| p.error.is_none());
    if let Some(p) = psi {
        let scope = p.field_scope.clone().unwrap_or_default();
        match p.lcp_ms {
            Some(x) => out.push(grade_low("D01", x, 2500.0, 4000.0, format!("{} с ({scope})", fmt_num(x / 1000.0)))),
            None => out.push(MetricResult::na("D01", "Нет полевых данных CrUX")),
        }
        match p.inp_ms {
            Some(x) => out.push(grade_low("D02", x, 200.0, 500.0, format!("{} мс ({scope})", fmt_num(x)))),
            None => out.push(MetricResult::na("D02", "Нет полевых данных CrUX")),
        }
        match p.cls {
            Some(x) => out.push(grade_low("D03", x, 0.1, 0.25, format!("{} ({scope})", fmt_num(x)))),
            None => out.push(MetricResult::na("D03", "Нет полевых данных CrUX")),
        }
        out.push(MetricResult::info("D04", if p.field { format!("Есть: {scope}") } else { "Нет — у сайта мало трафика".into() }).with_value(p.field));
        if let Some(x) = p.perf {
            out.push(grade_high("D05", x, 90.0, 50.0, format!("{} из 100", fmt_num(x))));
        }
        if let Some(x) = p.fcp_ms {
            out.push(grade_low("D06", x, 1800.0, 3000.0, format!("{} с", fmt_num(x / 1000.0))));
        }
        if let Some(x) = p.tbt_ms {
            out.push(grade_low("D07", x, 200.0, 600.0, format!("{} мс", fmt_num(x.round()))));
        }
        if let Some(x) = p.total_bytes {
            out.push(grade_low("D08", x / 1_048_576.0, 3.0, 5.0, format!("{} МБ", fmt_num((x / 104_857.6).round() / 10.0))));
        }
        if let Some(x) = p.unused_js_bytes {
            out.push(MetricResult::info("D10", format!("{} КБ неиспользуемого JS", (x / 1024.0).round())).with_value(x));
        }
        if let Some(x) = p.modern_images {
            out.push(grade_high("D11", x, 0.9, 0.5, if x >= 0.9 { "Форматы и размеры картинок в порядке".into() } else { "Картинки можно ужать (WebP/AVIF, размеры)".into() }));
        }
    }
    out.push(
        MetricResult::info(
            "D09",
            format!(
                "Запросов: {}; сторонних доменов скриптов: {}",
                psi.and_then(|p| p.requests).map(|x| x.to_string()).unwrap_or_else(|| "—".into()),
                ext_hosts.len()
            ),
        )
        .with_value(ext_hosts.len()),
    );
    // D12–D15: по Lighthouse, если есть; иначе приблизительно по HTML.
    let approx = " (оценка по HTML, без рендера)";
    match psi.and_then(|p| p.lcp_lazy) {
        // Google PageSpeed смотрит отрисованную страницу: отложенную загрузку часто ставит скрипт
        // (Tilda, Swiper), в исходном HTML атрибута loading=lazy может не быть.
        Some(x) => out.push(MetricResult::new(
            "D12",
            if x >= 0.9 { Status::Pass } else { Status::Fail },
            if x >= 0.9 {
                "Главная картинка первого экрана грузится сразу (Google PageSpeed)"
            } else {
                "Главная картинка первого экрана грузится отложенно — появляется позже остального (Google PageSpeed, ленивая загрузка)"
            },
        )),
        None => {
            let first_lazy = page.images.iter().find(|i| !i.tiny).is_some_and(|i| i.lazy);
            out.push(if first_lazy {
                MetricResult::warn("D12", format!("Первая картинка страницы с отложенной загрузкой{approx}"))
            } else {
                MetricResult::pass("D12", format!("Первая картинка грузится сразу{approx}"))
            });
        }
    }
    match psi.and_then(|p| p.render_blocking) {
        Some(x) => out.push(grade_high("D13", x, 0.9, 0.5, if x >= 0.9 { "Блокирующих ресурсов почти нет".into() } else { "Есть ресурсы, блокирующие отрисовку".into() })),
        None => {
            let blocking = page.scripts.iter().filter(|x| x.in_head && x.src.is_some() && !x.is_async && !x.is_defer && !x.is_module).count();
            let css = page.stylesheets.iter().filter(|(_, h)| *h).count();
            out.push(
                MetricResult::new("D13", if blocking <= 2 { Status::Pass } else { Status::Warn }, format!("В <head> {blocking} синхронных скриптов и {css} CSS{approx}"))
                    .with_value(blocking),
            );
        }
    }
    match psi.and_then(|p| p.font_display) {
        Some(x) => out.push(MetricResult::new("D14", if x >= 0.9 { Status::Pass } else { Status::Warn }, if x >= 0.9 { "Шрифты не задерживают текст" } else { "Текст ждёт загрузки шрифтов" })),
        None => {
            let gf: Vec<&String> = page.stylesheets.iter().map(|(u, _)| u).filter(|u| u.contains("fonts.googleapis.com")).collect();
            out.push(if gf.is_empty() {
                MetricResult::pass("D14", format!("Внешних веб-шрифтов Google нет{approx}"))
            } else if gf.iter().all(|u| u.contains("display=swap") || u.contains("display=optional")) {
                MetricResult::pass("D14", format!("Google Fonts с display=swap{approx}"))
            } else {
                MetricResult::warn("D14", format!("Google Fonts без display=swap{approx}"))
            });
        }
    }
    const WIDGETS: &[(&str, &str)] = &[
        ("jivosite", "JivoSite"), ("bitrix24", "Битрикс24"), ("carrotquest", "Carrot quest"), ("marquiz", "Marquiz"),
        ("envybox", "Envybox"), ("callbackhunter", "CallbackHunter"), ("leadhit", "LeadHit"), ("talk-me", "Talk-Me"),
        ("chatra", "Chatra"), ("livetex", "LiveTex"), ("redhelper", "RedHelper"), ("callibri", "Callibri"),
        ("calltouch", "Calltouch"), ("comagic", "CoMagic"), ("uiscom", "UIS"), ("roistat", "Roistat"),
    ];
    let found: Vec<&str> = WIDGETS.iter().filter(|(m, _)| page.html_lower.contains(m)).map(|(_, n)| *n).collect();
    match psi.and_then(|p| p.third_party_ms) {
        Some(ms) => out.push(grade_low("D15", ms, 250.0, 1000.0, format!("Сторонний код блокирует {} мс{}", ms.round(), if found.is_empty() { String::new() } else { format!(": {}", found.join(", ")) }))),
        None => out.push(
            MetricResult::new("D15", if found.len() <= 2 { Status::Pass } else { Status::Warn }, if found.is_empty() { format!("Тяжёлых виджетов не найдено{approx}") } else { format!("Виджеты: {}{approx}", found.join(", ")) })
                .with_value(found.len()),
        ),
    }
}

fn cms_check(v: &SiteView) -> MetricResult {
    let h = &v.page.html_lower;
    let generator = v.page.generator.clone().unwrap_or_default();
    let gl = generator.to_lowercase();
    let known: &[(&str, &str)] = &[
        ("wp-content", "WordPress"), ("/bitrix/", "1С-Битрикс"), ("tildacdn", "Tilda"), ("static.wixstatic", "Wix"),
        ("joomla", "Joomla"), ("drupal", "Drupal"), ("modx", "MODX"), ("catalog/view/theme", "OpenCart"),
        ("nethouse", "Nethouse"), ("ucoz", "uCoz"), ("insales", "InSales"), ("cdn.shopify", "Shopify"),
        ("__next_data__", "Next.js"), ("__nuxt", "Nuxt"), ("webflow", "Webflow"), ("craftum", "Craftum"),
        ("umi-cms", "UMI.CMS"), ("netcat", "NetCat"), ("cs-cart", "CS-Cart"), ("flexbe", "Flexbe"),
        ("squarespace", "Squarespace"), ("img1.wsimg.com", "GoDaddy Website Builder"), ("weebly", "Weebly"),
        ("jimdo", "Jimdo"), ("duda", "Duda"), ("strikingly", "Strikingly"), ("square.site", "Square Online"),
        ("editmysite", "Square Online"), ("site123", "SITE123"), ("webnode", "Webnode"), ("hubspot", "HubSpot CMS"),
        ("ghost", "Ghost"), ("typo3", "TYPO3"), ("contao", "Contao"), ("prestashop", "PrestaShop"),
        ("magento", "Magento"), ("framerusercontent", "Framer"), ("readymag", "Readymag"),
    ];
    let cms = known.iter().find(|(m, _)| h.contains(m) || gl.contains(&m.to_lowercase())).map(|(_, n)| *n);
    if gl.starts_with("wordpress") {
        let ver: Vec<u32> = gl.trim_start_matches("wordpress").trim().split('.').filter_map(|x| x.parse().ok()).collect();
        if let (Some(&maj), Some(&min)) = (ver.first(), ver.get(1))
            && (maj, min) < (6, 4)
        {
            return MetricResult::warn("O03", format!("{generator}: устаревшая версия WordPress")).with_value(generator);
        }
    }
    match cms {
        Some(name) => MetricResult::pass("O03", if generator.is_empty() { name.to_string() } else { format!("{name} ({generator})") }).with_value(name),
        None if !generator.is_empty() => MetricResult::pass("O03", generator.clone()).with_value(generator),
        None => MetricResult::pass("O03", "CMS не определена").with_value("unknown"),
    }
}
