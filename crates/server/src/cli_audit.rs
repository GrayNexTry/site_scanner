//! `maps-seo-scanner audit <url>`: проверка одного сайта из консоли, без сервера и базы.
//!
//! Нужна тем, кто делает сайт (в том числе агентам): прогнать те же метрики, что увидит
//! клиент в отчёте, и исправить провалы до сдачи. Работает и с сайтом в разработке
//! (`http://localhost:4321`): метрики домена, TLS и HSTS тогда уходят в «после выкладки».
//! Код выхода: 0 — критичных провалов нет, 1 — есть, 2 — сайт не открылся.

use std::net::IpAddr;
use std::path::Path;

use clap::Args;
use serde_json::{json, Value};

use scanner_core::catalog::catalog;
use scanner_core::checks::{self, CheckCtx};
use scanner_core::http::Fetcher;
use scanner_core::site::{self, CollectOptions};
use scanner_core::text::{host_of, parse_url_lenient};
use scanner_core::{scoring, Business, LatLon, MarketStats, MetricResult, Place, ProviderKind, ScanParams, Status};

use crate::settings::Settings;

#[derive(Args)]
pub struct AuditArgs {
    /// Адрес сайта: https://example.com или http://localhost:4321 для сайта в разработке.
    pub url: String,
    /// Страна (ru, by, de, us, gb…): от неё зависят правовые метрики и язык. По умолчанию — по домену.
    #[arg(long)]
    pub country: Option<String>,
    /// Город — для проверки «услуга + город» в заголовках и тексте.
    #[arg(long)]
    pub city: Option<String>,
    /// Услуга так, как её ищут клиенты: «стоматология», «dentist».
    #[arg(long)]
    pub service: Option<String>,
    /// Ниша из пресетов сканера (dentist, beauty, hairdresser, cafe…) вместо --service.
    #[arg(long)]
    pub preset: Option<String>,
    /// Название компании, как в карточке на картах.
    #[arg(long)]
    pub name: Option<String>,
    /// Телефон из карточки — сверка с сайтом (H02).
    #[arg(long)]
    pub phone: Option<String>,
    /// Адрес из карточки — сверка с сайтом (H02).
    #[arg(long)]
    pub address: Option<String>,
    /// Часы работы из карточки в формате OSM: "Mo-Fr 09:00-20:00; Sa 10:00-16:00" (H03).
    #[arg(long)]
    pub hours: Option<String>,
    /// Сколько страниц обойти (по умолчанию — из настроек сканера).
    #[arg(long)]
    pub pages: Option<usize>,
    /// Показать и пройденные метрики.
    #[arg(long)]
    pub all: bool,
    /// JSON вместо текста.
    #[arg(long)]
    pub json: bool,
}

/// Что на локальном сервере разработки не измерить честно: домен, сертификат, редиректы
/// на https, сжатие и кэш прокси, HTTP/2, HSTS, почтовые записи DNS, репутация, полевые данные.
const AFTER_DEPLOY: &[&str] = &["B02", "B03", "B04", "B05", "B06", "B08", "B09", "B10", "B11", "B13", "C21", "D01", "D02", "D03", "D04", "O01", "O05", "O06"];

fn is_local(host: &str) -> bool {
    let h = host.trim_matches(['[', ']']);
    h == "localhost"
        || h.ends_with(".localhost")
        || h.ends_with(".local")
        || h.parse::<IpAddr>().is_ok_and(|ip| match ip {
            IpAddr::V4(v) => v.is_loopback() || v.is_private() || v.is_link_local(),
            IpAddr::V6(v) => v.is_loopback() || v.is_unique_local(),
        })
}

/// Настройки сканера (ключ PSI, браузер, лимиты), если он уже запускался; иначе — по умолчанию.
async fn load_settings(dir: &Path) -> Settings {
    let path = dir.join("scanner.db");
    if !path.exists() {
        return Settings::default();
    }
    match crate::db::Db::open(&path) {
        Ok(db) => db.get_setting("settings").await.ok().flatten().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default(),
        Err(_) => Settings::default(),
    }
}

struct Row<'a> {
    r: &'a MetricResult,
    name: &'a str,
    rule: &'a str,
    category: &'a str,
    stop: bool,
    after_deploy: bool,
}

pub async fn run(a: AuditArgs, data_dir: &Path) -> anyhow::Result<i32> {
    let Some(url) = parse_url_lenient(&a.url) else {
        eprintln!("некорректный адрес: {}", a.url);
        return Ok(2);
    };
    let host = host_of(&url);
    let local = is_local(&host);
    let s = load_settings(data_dir).await;
    let country = a
        .country
        .map(|c| c.trim().to_lowercase())
        .filter(|c| c.len() == 2)
        .or_else(|| scanner_core::region::country_from_host(&host));
    let params = ScanParams {
        query: a.service.clone().unwrap_or_default(),
        preset: a.preset.clone(),
        city: a.city.clone(),
        country: country.clone(),
        city_names: a.city.iter().cloned().collect(),
        center: LatLon { lat: 0.0, lon: 0.0 },
        radius_m: 0.0,
        providers: vec![ProviderKind::Manual],
        max_places: 1,
        quick: false,
    };
    let name = a.name.clone().unwrap_or_else(|| host.clone());
    let place = Place {
        provider: Some(ProviderKind::Manual),
        name: name.clone(),
        website: Some(url.to_string()),
        phone: a.phone.clone(),
        address: a.address.clone(),
        hours: a.hours.clone(),
        ..Default::default()
    };
    let business = Business { name, website: Some(url.to_string()), phone: a.phone, address: a.address, sources: vec![place], ..Default::default() };
    let lang = scanner_core::region::accept_language(country.as_deref());
    let fetcher = Fetcher::new(s.fetch_config())?.with_language(lang);
    // Скан из другой страны (VPN): время ответа и таймауты — не про сайт.
    let egress = async {
        let t = fetcher.api.get("https://www.cloudflare.com/cdn-cgi/trace").timeout(std::time::Duration::from_secs(8)).send().await.ok()?.text().await.ok()?;
        t.lines().find_map(|l| l.strip_prefix("loc=")).map(|x| x.trim().to_lowercase())
    }
    .await;
    let abroad = !local && matches!((egress.as_deref(), country.as_deref()), (Some(e), Some(c)) if e != c);
    let opts = CollectOptions {
        abroad,
        max_pages: a.pages.unwrap_or(s.max_pages),
        link_checks: s.link_checks,
        concurrency: s.per_site_concurrency,
        psi_key: (!local && s.psi_enabled && !s.psi_key.is_empty()).then(|| s.psi_key.clone()),
        safe_browsing_key: (!local && !s.safe_browsing_key.is_empty()).then(|| s.safe_browsing_key.clone()),
        browser: s.browser(),
        // ИКС, знаки и Safe Browsing Яндекса есть только у сайта в интернете.
        yandex: !local && scanner_core::region::yandex_market(country.as_deref()),
        ..Default::default()
    };
    let snap = site::collect(&fetcher, url.as_str(), &opts).await;
    let market = MarketStats::default();
    let results = checks::run_all(&CheckCtx { business: &business, site: Some(&snap), params: &params, market: &market, discovery: None });
    let report = scoring::score(&results, &business, None);
    let summary = snap.summary();

    let cat = catalog();
    let mut rows: Vec<Row> = results
        .iter()
        .filter_map(|r| {
            let d = cat.get(&r.id)?;
            Some(Row { r, name: &d.name, rule: &d.rule, category: &d.category, stop: d.stop_factor, after_deploy: local && AFTER_DEPLOY.contains(&r.id.as_str()) })
        })
        .collect();
    // Сначала стоп-факторы, затем по номеру метрики.
    rows.sort_by(|x, y| (y.stop, &x.r.id).cmp(&(x.stop, &y.r.id)));
    let blocking = rows.iter().filter(|x| x.r.status == Status::Fail && !x.after_deploy).count();
    let unreachable = summary["final_url"].is_null();

    if a.json {
        let items: Vec<Value> = rows
            .iter()
            .map(|x| {
                json!({"id": x.r.id, "category": x.category, "name": x.name, "status": x.r.status, "detail": x.r.detail,
                       "rule": x.rule, "stop_factor": x.stop, "after_deploy": x.after_deploy})
            })
            .collect();
        let out = json!({"url": url.as_str(), "local": local, "country": country, "site": summary, "overall": report.overall,
                         "stop_factors": report.stop_factors, "categories": report.categories, "results": items});
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        print_text(&a.url, local, country.as_deref(), &summary, &report, &rows, a.all);
    }
    Ok(if unreachable { 2 } else if blocking > 0 { 1 } else { 0 })
}

fn print_text(input: &str, local: bool, country: Option<&str>, summary: &Value, report: &scoring::Report, rows: &[Row], all: bool) {
    let count = |st: Status| rows.iter().filter(|x| x.r.status == st && !x.after_deploy).count();
    println!("# Аудит сайта {input}");
    if let Some(f) = summary["final_url"].as_str() {
        println!("Открылся: {f}");
    }
    if let Some(e) = summary["error"].as_str() {
        println!("Ошибка главной: {e}");
    }
    if summary["via_proxy"].as_bool() == Some(true) {
        let direct = summary["direct_error"].as_str().map(|e| format!(" (напрямую: {e})")).unwrap_or_default();
        println!("Открыт через свой прокси{direct}: скорость и TLS измерены с поправкой на прокси");
    }
    println!(
        "Страна: {} · рендер скриптов: {}{}",
        country.unwrap_or("не задана (правовые метрики — общие)"),
        if summary["rendered"].as_bool() == Some(true) { "да" } else { "нет" },
        if local { " · локальный сайт: домен, TLS и HSTS — после выкладки" } else { "" }
    );
    println!(
        "Балл: {} · критично {} · внимание {} · в норме {} · не измерено {}",
        report.overall.map(|o| format!("{o:.0}/100")).unwrap_or_else(|| "—".into()),
        count(Status::Fail),
        count(Status::Warn),
        count(Status::Pass),
        count(Status::NotMeasured)
    );
    if !report.stop_factors.is_empty() {
        println!("Стоп-факторы (балл не выше {}): {}", scoring::STOP_CAP, report.stop_factors.join(", "));
    }
    // with_rule: печатать норму; без пояснения — для «после выкладки», где локальные замеры не о чём.
    let section = |title: &str, pick: &dyn Fn(&Row) -> bool, with_rule: bool| {
        let list: Vec<&Row> = rows.iter().filter(|x| pick(x)).collect();
        if list.is_empty() {
            return;
        }
        println!("\n## {title} — {}", list.len());
        for x in list {
            let stop = if x.stop { " [стоп-фактор]" } else { "" };
            if x.after_deploy {
                println!("- {} {}{stop}", x.r.id, x.name);
            } else {
                println!("- {} {}{stop}: {}", x.r.id, x.name, x.r.detail);
            }
            if with_rule && !x.rule.is_empty() {
                println!("  норма: {}", x.rule);
            }
        }
    };
    section("Критично — исправить до сдачи", &|x| x.r.status == Status::Fail && !x.after_deploy, true);
    section("Внимание", &|x| x.r.status == Status::Warn && !x.after_deploy, true);
    if local {
        section("Проверить после выкладки на домен", &|x| x.after_deploy && matches!(x.r.status, Status::Fail | Status::Warn | Status::NotMeasured), false);
    }
    // Не измеренных много и причины повторяются (нет карточки, нужен ключ) — одна строка на причину.
    let mut groups: Vec<(&str, Vec<&str>)> = Vec::new();
    for x in rows.iter().filter(|x| x.r.status == Status::NotMeasured && !x.after_deploy) {
        match groups.iter_mut().find(|(d, _)| *d == x.r.detail) {
            Some((_, ids)) => ids.push(&x.r.id),
            None => groups.push((&x.r.detail, vec![&x.r.id])),
        }
    }
    if !groups.is_empty() {
        println!("\n## Не измерено автоматически — {}", groups.iter().map(|(_, ids)| ids.len()).sum::<usize>());
        for (detail, ids) in groups {
            println!("- {}: {detail}", ids.join(", "));
        }
    }
    if all {
        section("В норме", &|x| x.r.status == Status::Pass, false);
        section("Справочно", &|x| x.r.status == Status::Info, false);
    }
}

#[cfg(test)]
mod tests {
    use super::is_local;

    #[test]
    fn local_hosts() {
        for h in ["localhost", "app.localhost", "127.0.0.1", "192.168.1.20", "10.0.0.5", "[::1]", "printer.local"] {
            assert!(is_local(h), "{h}");
        }
        for h in ["example.com", "8.8.8.8", "localhost.example.com"] {
            assert!(!is_local(h), "{h}");
        }
    }
}
