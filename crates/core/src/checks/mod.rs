//! Проверки метрик каталога. Каждая метрика получает результат: измеренный или
//! «не измерено» с причиной. Проверки — чистые функции над собранными данными.

mod ai;
mod business;
mod card;
mod content;
mod tech;
mod yandex;

pub use content::address_matches;

use std::collections::HashSet;

use chrono::Datelike;
use url::Url;

use crate::catalog::{catalog, MetricDef};
use crate::http::Fetched;
use crate::model::{Business, MarketStats, MetricResult, ProviderKind, ScanParams, Status};
use crate::page::PageInfo;
use crate::site::SiteSnapshot;

pub struct CheckCtx<'a> {
    pub business: &'a Business,
    pub site: Option<&'a SiteSnapshot>,
    pub params: &'a ScanParams,
    pub market: &'a MarketStats,
    /// Сайт, найденный в обход карт (или указанный вручную), если в карточке своего нет.
    pub discovery: Option<&'a crate::discover::Discovery>,
}

/// Удобный доступ к данным сайта для проверок.
pub struct SiteView<'a> {
    pub snap: &'a SiteSnapshot,
    pub home: &'a Fetched,
    pub page: &'a PageInfo,
    pub final_url: Url,
    /// Главная и обойдённые страницы.
    pub pages: Vec<&'a PageInfo>,
    /// Текст всех страниц в нижнем регистре (ё → е).
    pub text_all: String,
    /// HTML всех страниц в нижнем регистре.
    pub html_all: String,
    pub now_year: i32,
}

impl SiteView<'_> {
    pub fn robots(&self) -> Option<&str> {
        self.snap.robots_body()
    }
    pub fn https(&self) -> bool {
        self.final_url.scheme() == "https"
    }
    /// Внутренние страницы (без главной).
    pub fn inner_pages(&self) -> impl Iterator<Item = &&PageInfo> {
        self.pages.iter().skip(1)
    }
}

/// Результаты по всем метрикам каталога для одной компании.
pub fn run_all(ctx: &CheckCtx) -> Vec<MetricResult> {
    let mut out: Vec<MetricResult> = Vec::new();
    let manual = ctx.business.sources.iter().all(|s| s.provider == Some(ProviderKind::Manual));
    if !manual {
        card::run(ctx, &mut out);
    }
    let empty = PageInfo::default();
    let mut site_state = SiteState::NoSite;
    if let Some(snap) = ctx.site {
        match &snap.home {
            Some(home) if crate::site::is_blocked(home) => {
                site_state = SiteState::Blocked;
                tech::run_blocked(snap, home, &mut out);
            }
            Some(home) => {
                site_state = SiteState::Ok;
                let page = snap.page.as_ref().unwrap_or(&empty);
                let pages: Vec<&PageInfo> = snap.all_pages().collect();
                let pages = if pages.is_empty() { vec![page] } else { pages };
                let mut text_all = String::new();
                let mut html_all = String::new();
                for p in &pages {
                    text_all.push_str(&p.text_lower());
                    text_all.push('\n');
                    if html_all.len() < 8_000_000 {
                        html_all.push_str(&p.html_lower);
                        html_all.push('\n');
                    }
                }
                // Свои скрипты сайта — для поиска счётчиков и пикселей (P01–P05).
                html_all.push_str(&snap.site_js_lower);
                let view = SiteView {
                    snap,
                    home,
                    page,
                    final_url: Url::parse(&home.final_url).unwrap_or_else(|_| Url::parse("https://invalid.local/").unwrap()),
                    pages,
                    text_all,
                    html_all,
                    now_year: chrono::Utc::now().year(),
                };
                tech::run(ctx, &view, &mut out);
                content::run(ctx, &view, &mut out);
                business::run(ctx, &view, &mut out);
                // После content: R09 берёт число цен из I08.
                yandex::run(ctx, &view, &mut out);
                if snap.js_shell && snap.raw_words.is_none() {
                    unrender(&mut out, snap.render_error.as_deref());
                }
                not_a_site(&mut out);
                foreign_site(&mut out);
            }
            None => {
                site_state = SiteState::Dead;
                tech::run_dead(snap, &mut out);
            }
        }
    }
    fill_missing(&mut out, site_state, manual, ctx.site.and_then(|s| s.psi.as_ref()).map(|p| p.error.clone()));
    let order: std::collections::HashMap<&str, usize> =
        catalog().metrics.iter().enumerate().map(|(i, m)| (m.id.as_str(), i)).collect();
    out.sort_by_key(|r| order.get(r.id.as_str()).copied().unwrap_or(usize::MAX));
    out
}

/// Главная не работает (ошибка, парковка, заглушка «в разработке», страница хостера) — оценивать
/// заголовки, sitemap, разметку и тексты такой страницы бессмысленно: это не сайт компании.
/// Остаются вердикт B01/F14, данные домена и карточки.
fn not_a_site(out: &mut [MetricResult]) {
    // Провал главной или окно, за которое сканер не заходит (возрастной барьер): остальное — по заглушке.
    let b01_fail = out.iter().any(|r| {
        r.id == "B01" && (r.status == Status::Fail || (r.status == Status::Warn && r.value.as_ref().and_then(|v| v.as_str()) == Some("blocked")))
    });
    let f14_home = out.iter().any(|r| r.id == "F14" && r.status == Status::Fail && r.value.as_ref().and_then(|v| v.as_str()) == Some("home"));
    if !b01_fail && !f14_home {
        return;
    }
    const KEEP: &[&str] = &["B01", "B02", "B03", "B13", "F14", "O06"];
    for r in out.iter_mut().filter(|r| !r.id.starts_with('A') && !KEEP.contains(&r.id.as_str()) && matches!(r.status, Status::Fail | Status::Warn | Status::Pass)) {
        *r = MetricResult::na(&r.id, "Сайт не работает или вместо него заглушка — остальное не проверялось");
    }
}

/// Домен занят чужим сайтом (казино, другой язык, не та компания): метрики содержимого
/// относились бы к чужому сайту — «не измерено», кроме самого вывода H02 и данных домена.
fn foreign_site(out: &mut [MetricResult]) {
    if !out.iter().any(|r| r.id == "H02" && r.value.as_ref().and_then(|v| v.as_str()) == Some("foreign")) {
        return;
    }
    const KEEP: &[&str] = &["B01", "B02", "B03", "B04", "B13", "H02", "O06"];
    for r in out.iter_mut().filter(|r| !r.id.starts_with('A') && !KEEP.contains(&r.id.as_str()) && matches!(r.status, Status::Fail | Status::Warn | Status::Pass)) {
        *r = MetricResult::na(&r.id, "На домене чужой сайт — остальное не проверялось");
    }
}

/// Метрики, которые по пустому исходному HTML дали бы ложный провал.
const TEXT_DEPENDENT: &[&str] = &[
    "E05", "F03", "F04", "F06", "F07", "F10", "F13", "F14", "F17", "F19", "H01", "H02", "H03", "H04", "H05", "I07", "I08", "I09",
    "I10", "I11", "I12", "I13", "M01", "M02", "M03", "M05", "M06", "M07", "M08", "M09", "M10", "N01", "N02", "N03", "N04", "N05",
    "N06", "N08", "Q03", "C12", "C13", "C16", "F16", "R01", "R04", "R05", "R06", "R07", "R08", "R09", "R10", "R11", "R15",
];

/// Сайт рисуется скриптами, а браузера для рендера нет: провалы по тексту не доказаны.
fn unrender(out: &mut [MetricResult], err: Option<&str>) {
    let why = match err {
        Some(e) => format!("Сайт рисуется скриптами, рендер не удался ({e}) — проверьте вручную"),
        None => "Сайт рисуется скриптами — для проверки нужен браузер (Chromium, см. Настройки)".to_string(),
    };
    // «H1 ставит JS» — вывод по рендеру, он остаётся.
    for r in out.iter_mut().filter(|r| TEXT_DEPENDENT.contains(&r.id.as_str()) && matches!(r.status, Status::Fail | Status::Warn) && !r.detail.contains("ставит JS")) {
        *r = MetricResult::na(&r.id, why.clone());
    }
}

#[derive(Clone, Copy, PartialEq)]
enum SiteState {
    NoSite,
    Dead,
    /// Сайт ответил, но не пустил сканер (403, капча, WAF).
    Blocked,
    Ok,
}

const RENDER_NEEDED: &[&str] = &["E02", "E03", "E04", "E06", "E07", "Q02"];
const PSI_METRICS: &[&str] = &["D01", "D02", "D03", "D04", "D05", "D06", "D07", "D08", "D10", "D11", "Q01"];

fn reason(def: &MetricDef, site: SiteState, manual: bool, psi_error: &Option<Option<String>>) -> String {
    let id = def.id.as_str();
    if def.category == "A" && manual {
        return "Ручная проверка сайта: карточки на картах нет".into();
    }
    if def.source == "access" {
        return "Нужен доступ к кабинетам клиента — этап отчёта".into();
    }
    if def.category != "A" && site == SiteState::NoSite {
        return "Нет сайта для проверки".into();
    }
    if def.category != "A" && site == SiteState::Dead && !matches!(id, "B02" | "B03" | "O06") {
        return "Сайт не открылся — проверка невозможна".into();
    }
    if def.category != "A" && site == SiteState::Blocked {
        return "Сайт не пустил сканер — проверьте вручную или без VPN".into();
    }
    if PSI_METRICS.contains(&id) {
        return match psi_error {
            Some(Some(e)) => format!("PageSpeed Insights не ответил: {e}"),
            _ => "Нужен ключ PageSpeed Insights (Настройки)".into(),
        };
    }
    if RENDER_NEEDED.contains(&id) {
        return "Нужна вёрстка в браузере (ширина экрана, шрифты, тап-цели) — в планах".into();
    }
    match (def.category.as_str(), id) {
        (_, "A20") => "Геосетка позиций — этап аудита (SERP API с координатами)".into(),
        ("K", _) => "Нужен SERP API для позиций в выдаче — этап аудита".into(),
        ("L", _) => "Нужен API ссылочного сервиса — этап аудита".into(),
        (_, "H06" | "H07" | "H08") => "Поиск по каталогам и отзовикам — этап аудита".into(),
        (_, "F09") => "Проверка уникальности текстов — этап аудита".into(),
        (_, "A02") => "API карт не сообщают, подтверждена ли карточка".into(),
        (_, "A07" | "A08" | "A09" | "A10" | "A13" | "A14" | "A15" | "A16" | "A17" | "A25") => {
            "Этих данных нет в API карт".into()
        }
        (_, "A11") => "Нужна ручная сверка категорий с лидерами".into(),
        (_, "C15" | "F16") => "Нужна разметка страниц услуг".into(),
        (_, "C19") => "ИКС доступен только в интерфейсе Яндекс Вебмастера".into(),
        (_, "C23") => "Снаружи IndexNow не определить".into(),
        (_, "F08") => "Нужны данные о филиалах".into(),
        (_, "F15") => "Проверка орфографии — в планах".into(),
        (_, "R20") => "Поиск других сайтов компании по телефону и адресу — этап аудита (нужен ключ веб-поиска)".into(),
        _ => "Не измеряется в этой версии".into(),
    }
}

/// Привести сохранённые результаты к текущему каталогу: убрать метрики, которых в нём больше
/// нет, и добавить новые как «не измерено» до перепроверки. true — что-то поменялось.
pub fn sync_with_catalog(results: &mut Vec<MetricResult>) -> bool {
    let known: HashSet<&str> = catalog().metrics.iter().map(|d| d.id.as_str()).collect();
    let before = results.len();
    results.retain(|r| known.contains(r.id.as_str()));
    let mut changed = results.len() != before;
    let have: HashSet<String> = results.iter().map(|r| r.id.clone()).collect();
    for def in &catalog().metrics {
        if !have.contains(&def.id) {
            results.push(MetricResult::na(&def.id, "Проверка добавлена после аудита — нажмите «Перепроверить»"));
            changed = true;
        }
    }
    changed
}

/// Отпечаток каталога: меняется, когда метрики добавили или убрали.
pub fn catalog_fingerprint() -> String {
    let ids: Vec<&str> = catalog().metrics.iter().map(|d| d.id.as_str()).collect();
    format!("{}:{}", ids.len(), ids.join(","))
}

fn fill_missing(out: &mut Vec<MetricResult>, site: SiteState, manual: bool, psi_error: Option<Option<String>>) {
    let have: HashSet<String> = out.iter().map(|r| r.id.clone()).collect();
    for def in &catalog().metrics {
        if !have.contains(&def.id) {
            out.push(MetricResult::na(&def.id, reason(def, site, manual, &psi_error)));
        }
    }
}

// ---------- Общие помощники ----------

pub(crate) fn fmt_num(x: f64) -> String {
    if (x - x.round()).abs() < 1e-9 {
        return format!("{}", x.round() as i64);
    }
    let s = format!("{x:.2}");
    s.trim_end_matches('0').trim_end_matches('.').replace('.', ",")
}

/// pass, если ≤ good; warn, если ≤ poor; иначе fail.
pub(crate) fn grade_low(id: &str, v: f64, good: f64, poor: f64, detail: String) -> MetricResult {
    let st = if v <= good { Status::Pass } else if v <= poor { Status::Warn } else { Status::Fail };
    MetricResult::new(id, st, detail).with_value(v)
}

/// pass, если ≥ good; warn, если ≥ poor; иначе fail.
pub(crate) fn grade_high(id: &str, v: f64, good: f64, poor: f64, detail: String) -> MetricResult {
    let st = if v >= good { Status::Pass } else if v >= poor { Status::Warn } else { Status::Fail };
    MetricResult::new(id, st, detail).with_value(v)
}

pub(crate) fn list_short(items: &[String], n: usize) -> String {
    let mut s = items.iter().take(n).cloned().collect::<Vec<_>>().join(", ");
    if items.len() > n {
        s.push_str(&format!(" и ещё {}", items.len() - n));
    }
    s
}

#[cfg(test)]
mod tests;
