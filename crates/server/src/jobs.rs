//! Фоновые сканы: поиск мест → слияние → аудит сайтов с ограничением параллельности.
//! Прогресс рассылается событиями (SSE).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::{broadcast, OnceCell, RwLock, Semaphore};
use tracing::{error, info, warn};

use scanner_core::checks::{self, CheckCtx};
use scanner_core::discover::{self, DiscoverOptions, Discovery, Source, Verdict};
use scanner_core::search::WebSearch;
use scanner_core::http::Fetcher;
use scanner_core::providers::{self, merge, Keys};
use scanner_core::site::{self, CollectOptions, SiteSnapshot};
use scanner_core::text::{classify_website, haversine_m, host_of, normalize_name, parse_url_lenient, registrable_domain, similarity};
use scanner_core::{scoring, Business, MarketStats, ScanParams};

use crate::db::{BusinessRow, Db, ScanRow, Which};
use crate::settings::Settings;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Scan { scan: Box<ScanRow> },
    Business { scan_id: i64, business: Box<BusinessRow> },
}

impl Event {
    pub fn scan_id(&self) -> i64 {
        match self {
            Event::Scan { scan } => scan.id,
            Event::Business { scan_id, .. } => *scan_id,
        }
    }
}

pub struct AppState {
    pub db: Db,
    pub settings: RwLock<Settings>,
    pub fetcher: RwLock<Arc<Fetcher>>,
    /// Веб-поиск для поиска сайтов: один на приложение (общий лимит частоты и память о капчах).
    pub search: RwLock<Arc<WebSearch>>,
    pub events: broadcast::Sender<Event>,
    cancels: Mutex<HashMap<i64, Arc<AtomicBool>>>,
    /// Нейросеть для писем: каталог моделей OpenRouter и массовая генерация.
    pub ai: crate::ai::AiState,
    /// Страна выхода в интернет и когда узнали (VPN меняет её на ходу).
    egress: tokio::sync::Mutex<Option<(Instant, Option<String>)>>,
}

pub type Shared = Arc<AppState>;

fn make_fetcher(s: &Settings) -> anyhow::Result<Arc<Fetcher>> {
    Ok(Arc::new(Fetcher::new(s.fetch_config())?))
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    /// Поиск мест и аудит.
    Full,
    /// Перепроверить все компании.
    AuditAll,
    /// Доделать непроверенные (после прерывания).
    AuditPending,
    /// Искать места заново: добавить новые компании к уже найденным и проверить только их.
    Search,
    /// Перепроверить сайты, которые не открылись или не пустили сканер.
    AuditUnreachable,
}

impl AppState {
    pub async fn new(db: Db) -> anyhow::Result<Shared> {
        // Адрес OpenRouter можно подменить для проверок (свой совместимый сервер).
        Self::with_ai(db, std::env::var("MAPS_SCANNER_OPENROUTER_URL").ok()).await
    }

    /// То же с заданным адресом API нейросети (None — OpenRouter).
    pub async fn with_ai(db: Db, ai_base: Option<String>) -> anyhow::Result<Shared> {
        let settings: Settings = db
            .get_setting("settings")
            .await?
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        // Прокси из настроек перестал разбираться — запускаемся без него, а не падаем.
        let fetcher = make_fetcher(&settings).or_else(|e| {
            warn!("загрузчик с настройками не собрался ({e:#}) — работаю без прокси");
            make_fetcher(&Settings { proxy_url: String::new(), ..settings.clone() })
        })?;
        let search = Arc::new(WebSearch::new(settings.search_config()));
        let (events, _) = broadcast::channel(1024);
        Ok(Arc::new(Self {
            db,
            settings: RwLock::new(settings),
            fetcher: RwLock::new(fetcher),
            search: RwLock::new(search),
            events,
            cancels: Mutex::new(HashMap::new()),
            ai: crate::ai::AiState::new(ai_base.as_deref())?,
            egress: tokio::sync::Mutex::new(None),
        }))
    }

    /// Страна, из которой идут запросы (ISO-код в нижнем регистре); кэш на 10 минут.
    pub async fn egress_country(&self) -> Option<String> {
        let mut g = self.egress.lock().await;
        if let Some((at, c)) = g.as_ref()
            && at.elapsed() < std::time::Duration::from_secs(600)
        {
            return c.clone();
        }
        let f = self.fetcher.read().await.clone();
        let c = async {
            let t = f.api.get("https://www.cloudflare.com/cdn-cgi/trace").timeout(std::time::Duration::from_secs(8)).send().await.ok()?.text().await.ok()?;
            t.lines().find_map(|l| l.strip_prefix("loc=")).map(|x| x.trim().to_lowercase())
        }
        .await;
        *g = Some((Instant::now(), c.clone()));
        c
    }

    pub async fn save_settings(&self, s: Settings) -> anyhow::Result<()> {
        // Сначала собрать загрузчик: он проверяет адрес прокси, и битый адрес не попадёт в базу.
        let fetcher = make_fetcher(&s)?;
        self.db.set_setting("settings", serde_json::to_string(&s)?).await?;
        *self.fetcher.write().await = fetcher;
        let old = self.settings.read().await.search_config();
        let new = s.search_config();
        if (old.brave_key, old.yandex_key, old.yandex_folder, old.free_engines) != (new.brave_key.clone(), new.yandex_key.clone(), new.yandex_folder.clone(), new.free_engines) {
            *self.search.write().await = Arc::new(WebSearch::new(new));
        }
        *self.settings.write().await = s;
        Ok(())
    }

    pub async fn emit_scan(&self, id: i64) {
        if let Ok(Some(scan)) = self.db.get_scan(id).await {
            let _ = self.events.send(Event::Scan { scan: Box::new(scan) });
        }
    }

    pub async fn emit_business(&self, id: i64) {
        if let Ok(Some(b)) = self.db.business_brief(id).await {
            let b = self.db.with_contact_summary(b.clone()).await.unwrap_or(b);
            let _ = self.events.send(Event::Business { scan_id: b.scan_id, business: Box::new(b.slim()) });
        }
    }

    pub fn cancel(&self, scan_id: i64) -> bool {
        match self.cancels.lock().unwrap().get(&scan_id) {
            Some(flag) => {
                flag.store(true, Ordering::Relaxed);
                true
            }
            None => false,
        }
    }

    pub fn is_running(&self, scan_id: i64) -> bool {
        self.cancels.lock().unwrap().contains_key(&scan_id)
    }
}

pub fn spawn_scan(state: Shared, scan_id: i64, mode: Mode) {
    tokio::spawn(async move {
        let flag = Arc::new(AtomicBool::new(false));
        {
            let mut c = state.cancels.lock().unwrap();
            if c.contains_key(&scan_id) {
                warn!(scan_id, "скан уже выполняется");
                return;
            }
            c.insert(scan_id, flag.clone());
        }
        let started = Instant::now();
        let result = run_scan(&state, scan_id, mode, &flag).await;
        state.cancels.lock().unwrap().remove(&scan_id);
        match result {
            Ok(()) => {
                let status = if flag.load(Ordering::Relaxed) { "cancelled" } else { "done" };
                let _ = state.db.update_scan(scan_id, Some(status), None, None).await;
                info!(scan_id, status, secs = started.elapsed().as_secs(), "скан завершён");
            }
            Err(e) => {
                error!(scan_id, "скан упал: {e:#}");
                let _ = state.db.update_scan(scan_id, Some("failed"), None, Some(format!("{e:#}"))).await;
            }
        }
        let _ = state.db.refresh_progress(scan_id).await;
        state.emit_scan(scan_id).await;
    });
}

/// Завершается, когда нажали «Остановить» (флаг проверяется 5 раз в секунду).
async fn cancelled(flag: &AtomicBool) {
    while !flag.load(Ordering::Relaxed) {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

/// Та же компания, что уже есть в скане: общая карточка провайдера или то же название рядом.
fn same_business(old: &Business, new: &Business) -> bool {
    let ids = |b: &Business| b.sources.iter().map(|p| (p.provider, p.provider_id.clone())).collect::<Vec<_>>();
    let old_ids = ids(old);
    ids(new).iter().any(|id| old_ids.contains(id))
        || (haversine_m(old.lat, old.lon, new.lat, new.lon) < 80.0 && similarity(&old.name, &new.name) >= 0.85)
}

async fn run_scan(state: &Shared, scan_id: i64, mode: Mode, cancel: &AtomicBool) -> anyhow::Result<()> {
    let scan = state.db.get_scan(scan_id).await?.ok_or_else(|| anyhow::anyhow!("скан {scan_id} не найден"))?;
    let params: ScanParams = serde_json::from_value(scan.params.clone())?;
    // Сеть могла смениться (VPN включили, другой Wi-Fi): пересобрать загрузчик под текущий маршрут.
    {
        let s = state.settings.read().await.clone();
        let want = s.fetch_config().interface;
        if state.fetcher.read().await.interface() != want.as_deref()
            && let Ok(f) = make_fetcher(&s)
        {
            info!(iface = want.as_deref().unwrap_or("—"), "сеть сменилась: загрузчик пересобран");
            *state.fetcher.write().await = f;
            *state.egress.lock().await = None;
        }
    }
    state.db.update_scan(scan_id, Some("running"), Some("places"), None).await?;
    if mode == Mode::Search {
        state.db.clear_scan_error(scan_id).await?;
    }
    state.emit_scan(scan_id).await;

    if matches!(mode, Mode::Full | Mode::Search) && scan.kind == "map" {
        let settings = state.settings.read().await.clone();
        let keys = Keys { google: Some(settings.google_places_key.clone()), yandex: Some(settings.yandex_geosearch_key.clone()) };
        let fetcher = state.fetcher.read().await.clone();
        // «Остановить» прерывает поиск мест сразу, не дожидаясь источников: перегруженный
        // Overpass иначе держит скан минутами. Скан получит статус «Остановлен».
        let outcomes = tokio::select! {
            o = providers::search_all(&fetcher.api, &params, &keys) => o,
            () = cancelled(cancel) => {
                info!(scan_id, "поиск мест остановлен");
                return Ok(());
            }
        };
        let mut places = Vec::new();
        let mut errors = Vec::new();
        for o in outcomes {
            match o.result {
                Ok(p) => {
                    info!(scan_id, provider = ?o.provider, found = p.len(), "провайдер ответил");
                    places.extend(p);
                    if let Some(note) = o.notice {
                        warn!(scan_id, provider = ?o.provider, "провайдер: {note}");
                        errors.push(json!({"provider": o.provider, "error": note, "notice": true}));
                    }
                }
                Err(e) => {
                    warn!(scan_id, provider = ?o.provider, "провайдер: {e}");
                    errors.push(json!({"provider": o.provider, "error": e}));
                }
            }
        }
        state.db.set_provider_errors(scan_id, Value::Array(errors.clone())).await?;
        if places.is_empty() && errors.iter().any(|e| e.get("notice").is_none()) {
            let msg = errors.iter().filter_map(|e| e["error"].as_str()).collect::<Vec<_>>().join("; ");
            anyhow::bail!("ни один источник не ответил: {msg}");
        }
        let mut businesses = merge::merge(places);
        // Много точек с одним названием в районе — сеть: сайт у головной компании.
        let names = name_chain_sizes(businesses.iter().map(|b| (b.name.as_str(), b.category.as_deref())));
        for b in &mut businesses {
            b.chain_size = name_chain_size(&names, &b.name, b.category.as_deref());
        }
        let c = params.center;
        businesses.sort_by(|a, b| {
            haversine_m(c.lat, c.lon, a.lat, a.lon).partial_cmp(&haversine_m(c.lat, c.lon, b.lat, b.lon)).unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut limit = params.max_places;
        if mode == Mode::Search {
            let existing: Vec<Business> = state.db.scan_businesses(scan_id, false).await?.into_iter().map(|(_, b)| b).collect();
            limit = limit.saturating_sub(existing.len());
            businesses.retain(|b| !existing.iter().any(|old| same_business(old, b)));
            info!(scan_id, new = businesses.len().min(limit), "повторный поиск");
        }
        businesses.truncate(limit);
        state.db.insert_businesses(scan_id, businesses).await?;
        state.db.refresh_progress(scan_id).await?;
    }
    if cancel.load(Ordering::Relaxed) {
        return Ok(());
    }
    state.db.update_scan(scan_id, None, Some("audit"), None).await?;
    state.emit_scan(scan_id).await;
    let which = match mode {
        Mode::AuditPending | Mode::Search => Which::Pending,
        Mode::AuditUnreachable => Which::Unreachable,
        _ => Which::All,
    };
    audit_scan(state, scan_id, &params, which, cancel, None).await
}

type SiteCache = Arc<Mutex<HashMap<String, Arc<OnceCell<Arc<SiteSnapshot>>>>>>;

/// Настройки аудита на время скана.
pub struct AuditCfg {
    collect: CollectOptions,
    discover: bool,
    guess: bool,
    search: Option<Arc<WebSearch>>,
    /// Клиент с Accept-Language страны скана.
    fetcher: Arc<Fetcher>,
    /// Один сайт домена за раз: десятки магазинов на одной платформе (deal.by, tilda.ws)
    /// иначе бьют в её защиту одновременно и получают 429.
    domains: Mutex<HashMap<String, Arc<Semaphore>>>,
}

async fn audit_cfg(state: &Shared, params: &ScanParams) -> AuditCfg {
    let s = state.settings.read().await.clone();
    let search = state.search.read().await.clone();
    let lang = scanner_core::region::accept_language(params.country.as_deref());
    // Быстрая проверка большого района: главная и пара страниц, без PageSpeed (10–30 с на сайт)
    // и без угадывания доменов и веб-поиска (десятки запросов на каждую компанию без сайта).
    // Полную проверку одной компании даёт кнопка «Перепроверить».
    let quick = params.quick;
    let egress = state.egress_country().await;
    let abroad = matches!((egress.as_deref(), params.country.as_deref()), (Some(e), Some(c)) if !e.eq_ignore_ascii_case(c));
    AuditCfg {
        collect: CollectOptions {
            max_pages: if quick { s.max_pages.min(3) } else { s.max_pages },
            link_checks: if quick { s.link_checks.min(5) } else { s.link_checks },
            concurrency: s.per_site_concurrency,
            psi_key: (!quick && s.psi_enabled && !s.psi_key.is_empty()).then(|| s.psi_key.clone()),
            safe_browsing_key: (!s.safe_browsing_key.is_empty()).then(|| s.safe_browsing_key.clone()),
            browser: s.browser(),
            abroad,
            yandex: scanner_core::region::yandex_market(params.country.as_deref()),
            ..Default::default()
        },
        discover: s.discover_enabled,
        guess: s.discover_guess && !quick,
        search: (search.cfg.any() && !quick).then_some(search),
        fetcher: Arc::new(state.fetcher.read().await.with_language(lang)),
        domains: Mutex::new(HashMap::new()),
    }
}

/// `limit` — сайтов одновременно вместо настроек (дообход таймаутов идёт по 4).
async fn audit_scan(state: &Shared, scan_id: i64, params: &ScanParams, which: Which, cancel: &AtomicBool, limit: Option<usize>) -> anyhow::Result<()> {
    let run_started = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let all = state.db.scan_businesses(scan_id, false).await?;
    let market = Arc::new(MarketStats::from_businesses(&all.iter().map(|(_, b)| b.clone()).collect::<Vec<_>>()));
    let todo = match which {
        Which::All => all,
        w => state.db.scan_businesses_where(scan_id, w).await?,
    };
    let concurrency = state.settings.read().await.concurrency.max(1);
    // Быстрая проверка лёгкая: сайтов одновременно можно вдвое больше.
    let concurrency = if params.quick { (concurrency * 2).clamp(8, 32) } else { concurrency };
    let concurrency = limit.unwrap_or(concurrency);
    let opts = Arc::new(audit_cfg(state, params).await);
    let sem = Arc::new(Semaphore::new(concurrency));
    let cache: SiteCache = Arc::default();
    let params = Arc::new(params.clone());
    let mut handles = Vec::new();
    // Пять и больше компаний скана ведут на один и тот же сайт (при разных названиях:
    // «Евроопт», «Хит Дискаунт») — это сеть, сайтом занимается головная компания.
    let own_host = |b: &Business| {
        let w = b.website.as_deref()?;
        let (kind, host) = classify_website(w);
        (kind == scanner_core::text::SiteKind::Own).then(|| host.trim_start_matches("www.").to_string())
    };
    let mut hosts: HashMap<String, u32> = HashMap::new();
    for (_, b) in &state.db.scan_businesses(scan_id, false).await? {
        if let Some(h) = own_host(b) {
            *hosts.entry(h).or_default() += 1;
        }
    }
    for (bid, mut b) in todo {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let shared = own_host(&b).and_then(|h| hosts.get(&h).copied()).unwrap_or(0);
        if shared > b.chain_size {
            b.chain_size = shared;
            let _ = state.db.update_business_data(bid, serde_json::to_value(&b)?).await;
        }
        let permit = sem.clone().acquire_owned().await?;
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let (st, params, market, opts, cache) = (state.clone(), params.clone(), market.clone(), opts.clone(), cache.clone());
        handles.push((
            bid,
            tokio::spawn(async move {
                let _permit = permit;
                audit_one(&st, scan_id, bid, &b, &params, &market, &opts, &cache).await
            }),
        ));
    }
    for (bid, h) in handles {
        match h.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                let _ = state.db.fail_audit(bid, format!("{e:#}")).await;
            }
            Err(e) => {
                let _ = state.db.fail_audit(bid, format!("внутренняя ошибка: {e}")).await;
            }
        }
    }
    // Таймауты и обрывы при массовой проверке — чаще перегруз своего канала (точка доступа
    // телефона, VPN), чем мёртвый сайт: перепроверяем их ещё раз по 4 сайта одновременно.
    if limit.is_none() && !cancel.load(Ordering::Relaxed) {
        let failed = state.db.network_failures(scan_id, run_started).await?;
        if !failed.is_empty() {
            info!(scan_id, n = failed.len(), "дообход таймаутов по 4 сайта");
            state.db.mark_pending(scan_id, failed).await?;
            Box::pin(audit_scan(state, scan_id, &params, Which::Pending, cancel, Some(4))).await?;
        }
    }
    // Сайты, найденные поиском, известны только после проверки — сети по ним досчитываем в конце.
    if mark_shared_sites(state, scan_id).await? > 0 {
        rescore(state, Some(scan_id), false).await?;
        state.emit_scan(scan_id).await;
    }
    Ok(())
}

/// Пять и больше компаний скана в итоге ведут на один сайт — из карточки или найденный поиском
/// (отделения «Белпошты» → belpost.by) — это сеть: сайтом занимается головная компания.
/// Пересчитывает `chain_size` (по названию и по общему сайту); возвращает, у скольких изменил.
async fn mark_shared_sites(state: &Shared, scan_id: i64) -> anyhow::Result<usize> {
    // Размер сети пересчитывается целиком — и вверх, и вниз: общий «сайт», который оказался
    // каталогом (kramba.by у 13 клиник), больше не держит у независимых клиник метку сети.
    let all = state.db.scan_businesses(scan_id, false).await?;
    let rows = state.db.scan_site_urls(scan_id).await?;
    let sizes = shared_site_sizes(rows.iter().map(|(id, _, u)| (*id, u.as_str())));
    let names = name_chain_sizes(all.iter().map(|(_, b)| (b.name.as_str(), b.category.as_deref())));
    let mut changed = 0;
    for (id, mut b) in all {
        let want = name_chain_size(&names, &b.name, b.category.as_deref()).max(sizes.get(&id).copied().unwrap_or(0));
        if b.chain_size == want {
            continue;
        }
        b.chain_size = want;
        state.db.update_business_data(id, serde_json::to_value(&b)?).await?;
        changed += 1;
    }
    Ok(changed)
}

/// Сколько точек района с тем же названием; общие названия-виды деятельности («Стаматалогія»,
/// «Аптека») не считаются.
fn name_chain_sizes<'a>(items: impl Iterator<Item = (&'a str, Option<&'a str>)>) -> HashMap<String, u32> {
    let mut names: HashMap<String, u32> = HashMap::new();
    for (name, category) in items {
        if !scanner_core::discover::generic_name(name, category) {
            *names.entry(normalize_name(name)).or_default() += 1;
        }
    }
    names
}

fn name_chain_size(names: &HashMap<String, u32>, name: &str, category: Option<&str>) -> u32 {
    if scanner_core::discover::generic_name(name, category) {
        1
    } else {
        names.get(&normalize_name(name)).copied().unwrap_or(1)
    }
}

#[allow(clippy::too_many_arguments)]
async fn audit_one(
    state: &Shared,
    scan_id: i64,
    bid: i64,
    b: &Business,
    params: &ScanParams,
    market: &MarketStats,
    cfg: &AuditCfg,
    cache: &SiteCache,
) -> anyhow::Result<()> {
    state.db.set_audit_status(bid, "running").await?;
    state.emit_business(bid).await;
    let t0 = Instant::now();
    let manual = state.db.business_brief(bid).await?.and_then(|r| r.website_override);
    let card_site = b.website.as_deref().filter(|w| classify_website(w).0.is_auditable());
    // Что проверяем: сайт, указанный вручную, → сайт из карточки → найденный в обход карт.
    let (target, discovery): (Option<String>, Option<Discovery>) = match (manual, card_site) {
        (Some(m), _) => {
            let domain = parse_url_lenient(&m).map(|u| registrable_domain(&host_of(&u))).unwrap_or_else(|| m.clone());
            let v = Verdict { url: m.clone(), domain, source: Source::Manual, via: String::new(), score: 1.0, confirmed: true, evidence: vec!["указан вручную".into()], title: None };
            (Some(m), Some(Discovery { found: Some(v), ..Default::default() }))
        }
        (None, Some(w)) => (Some(w.to_string()), None),
        (None, None) if cfg.discover => {
            let opt = DiscoverOptions { guess: cfg.guess, search: cfg.search.as_deref(), ..Default::default() };
            let d = discover::discover(&cfg.fetcher, b, params, &opt).await;
            info!(bid, name = %b.name, found = ?d.found.as_ref().map(|v| &v.domain), suggestions = d.suggestions.len(), ms = d.elapsed_ms, "поиск сайта");
            (d.found.as_ref().map(|v| v.url.clone()), Some(d))
        }
        (None, None) => (None, None),
    };
    let site = match target.as_deref() {
        Some(w) => {
            let key = parse_url_lenient(w).map(|u| host_of(&u)).unwrap_or_else(|| w.to_string());
            let lock = cfg.domains.lock().unwrap().entry(registrable_domain(&key)).or_insert_with(|| Arc::new(Semaphore::new(1))).clone();
            let cell = cache.lock().unwrap().entry(key).or_insert_with(|| Arc::new(OnceCell::new())).clone();
            Some(
                cell.get_or_init(|| async {
                    let _turn = lock.acquire().await;
                    Arc::new(site::collect(&cfg.fetcher, w, &cfg.collect).await)
                })
                .await
                .clone(),
            )
        }
        None => None,
    };
    let results = checks::run_all(&CheckCtx { business: b, site: site.as_deref(), params, market, discovery: discovery.as_ref() });
    let report = scoring::score(&results, b, discovery.as_ref());
    let lead = serde_json::to_value(report.lead)?.as_str().unwrap_or("cold").to_string();
    state
        .db
        .save_audit(
            bid,
            lead,
            report.overall,
            report.priority,
            serde_json::to_value(&report)?,
            serde_json::to_value(&results)?,
            site.map(|s| s.summary()).unwrap_or(Value::Null),
            t0.elapsed().as_millis() as i64,
            discovery.map(|d| serde_json::to_value(d).unwrap_or_default()).unwrap_or(Value::Null),
            target,
        )
        .await?;
    state.db.refresh_progress(scan_id).await?;
    state.emit_business(bid).await;
    state.emit_scan(scan_id).await;
    Ok(())
}

/// Сколько компаний делят свой сайт (без www), для тех, у кого таких не меньше `CHAIN_MIN`.
/// Соцсети, конструкторы и агрегаторы не в счёт — это не общий сайт.
fn shared_site_sizes<'a>(sites: impl Iterator<Item = (i64, &'a str)>) -> HashMap<i64, u32> {
    let hosts: Vec<(i64, String)> = sites
        .filter_map(|(id, u)| {
            let (kind, h) = classify_website(u);
            (kind == scanner_core::text::SiteKind::Own).then(|| (id, h.trim_start_matches("www.").to_string()))
        })
        .collect();
    let mut count: HashMap<&str, u32> = HashMap::new();
    for (_, h) in &hosts {
        *count.entry(h.as_str()).or_default() += 1;
    }
    hosts.iter().filter_map(|(id, h)| Some((*id, count[h.as_str()])).filter(|(_, n)| *n >= scoring::CHAIN_MIN)).collect()
}

/// Пересчитать оценки, лиды и мини-отчёты из сохранённых результатов — без сети.
/// Нужен после правок логики оценки и для старых проверок без мини-отчёта.
pub async fn rescore(state: &Shared, scan_id: Option<i64>, only_missing: bool) -> anyhow::Result<usize> {
    if let Some(id) = scan_id {
        mark_shared_sites(state, id).await?;
    }
    const BATCH: i64 = 500;
    let mut after = 0;
    let mut total = 0;
    loop {
        let rows = state.db.rows_to_rescore(scan_id, only_missing, after, BATCH).await?;
        let Some(last) = rows.last().map(|r| r.0) else { break };
        after = last;
        total += rescore_batch(state, rows).await?;
    }
    Ok(total)
}

async fn rescore_batch(state: &Shared, rows: Vec<(i64, Value, Value, Value)>) -> anyhow::Result<usize> {
    let mut out = Vec::with_capacity(rows.len());
    for (id, data, results, discovery) in rows {
        let (Ok(b), Ok(mut results)) = (serde_json::from_value::<Business>(data), serde_json::from_value::<Vec<scanner_core::MetricResult>>(results)) else {
            continue;
        };
        let synced = scanner_core::checks::sync_with_catalog(&mut results);
        let discovery: Option<Discovery> = serde_json::from_value(discovery).ok();
        let report = scoring::score(&results, &b, discovery.as_ref());
        let lead = serde_json::to_value(report.lead)?.as_str().unwrap_or("cold").to_string();
        let results = if synced { Some(serde_json::to_value(&results)?) } else { None };
        out.push((id, lead, report.overall, report.priority, serde_json::to_value(&report)?, results));
    }
    let n = out.len();
    if n > 0 {
        state.db.update_reports(out).await?;
    }
    Ok(n)
}

/// Перепроверить одну компанию (кнопка в карточке).
pub fn spawn_business_audit(state: Shared, bid: i64) {
    tokio::spawn(async move {
        let run = async {
            let row = state.db.get_business(bid).await?.ok_or_else(|| anyhow::anyhow!("компания не найдена"))?;
            let scan = state.db.get_scan(row.scan_id).await?.ok_or_else(|| anyhow::anyhow!("скан не найден"))?;
            let params: ScanParams = serde_json::from_value(scan.params)?;
            let b: Business = serde_json::from_value(row.data)?;
            let all = state.db.scan_businesses(row.scan_id, false).await?;
            let market = MarketStats::from_businesses(&all.into_iter().map(|(_, b)| b).collect::<Vec<_>>());
            // Одна компания по кнопке — всегда полная проверка, даже в быстром скане.
            let cfg = audit_cfg(&state, &ScanParams { quick: false, ..params.clone() }).await;
            audit_one(&state, row.scan_id, bid, &b, &params, &market, &cfg, &Arc::default()).await
        };
        if let Err(e) = run.await {
            error!(bid, "перепроверка: {e:#}");
            let _ = state.db.fail_audit(bid, format!("{e:#}")).await;
            state.emit_business(bid).await;
        }
    });
}

#[cfg(test)]
mod shared_site_tests {
    use super::shared_site_sizes;

    #[test]
    fn found_site_shared_by_five_is_a_chain() {
        // Скан 18: пять отделений «Белпошты» — одно из карточки, остальные найдены поиском сайта.
        let sites = [
            (617, "http://belpost.by"),
            (2774, "https://belpost.by/by"),
            (3140, "https://www.belpost.by/by"),
            (3271, "https://belpost.by/by"),
            (3318, "https://belpost.by/by"),
            (1, "https://cafe.by/"),
            (2, "https://vk.com/a"),
            (3, "https://vk.com/b"),
            (4, "https://vk.com/c"),
            (5, "https://vk.com/d"),
            (6, "https://vk.com/e"),
        ];
        let sizes = shared_site_sizes(sites.into_iter());
        assert_eq!(sizes.len(), 5);
        assert_eq!(sizes[&3140], 5);
        assert!(!sizes.contains_key(&1) && !sizes.contains_key(&2));
    }
}
