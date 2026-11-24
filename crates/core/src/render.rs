//! Рендер страниц, которые рисуются скриптами (React, Vue и т. п.): headless Chromium
//! с `--dump-dom`. Процесс запускается на время одной страницы и убивается по таймауту;
//! одновременно — не больше двух. Без браузера такие сайты проверяются по исходному HTML,
//! а зависящие от текста метрики помечаются «не измерено».

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::sync::Semaphore;

const CANDIDATES: &[&str] = &[
    "chromium",
    "chromium-browser",
    "google-chrome-stable",
    "google-chrome",
    "chrome",
    "brave",
    "brave-browser",
    "microsoft-edge-stable",
    "chrome-headless-shell",
];

/// Найти браузер: путь из настроек, затем PATH, затем headless-сборка Playwright в ~/.cache.
pub fn find_browser(configured: Option<&str>) -> Option<PathBuf> {
    if let Some(p) = configured.map(str::trim).filter(|p| !p.is_empty()) {
        let p = PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for name in CANDIDATES {
            let p = dir.join(name);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    let cache = std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache/ms-playwright"))?;
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&cache).ok()?.filter_map(|e| e.ok().map(|e| e.path())).collect();
    dirs.sort();
    dirs.reverse();
    for d in dirs {
        let name = d.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let exe = if name.starts_with("chromium_headless_shell-") {
            d.join("chrome-headless-shell-linux64/chrome-headless-shell")
        } else if name.starts_with("chromium-") {
            d.join("chrome-linux64/chrome")
        } else {
            continue;
        };
        if exe.is_file() {
            return Some(exe);
        }
    }
    None
}

fn slots() -> &'static Semaphore {
    static S: OnceLock<Semaphore> = OnceLock::new();
    S.get_or_init(|| Semaphore::new(2))
}

/// HTML страницы после выполнения скриптов.
pub async fn dump_dom(browser: &Path, url: &str, user_agent: &str, accept_language: &str, proxy: Option<&str>, timeout: Duration) -> Result<String, String> {
    let _slot = slots().acquire().await.map_err(|e| e.to_string())?;
    let profile = std::env::temp_dir().join(format!("maps-seo-scanner-render-{}-{}", std::process::id(), rand_suffix()));
    let headless_shell = browser.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.contains("headless"));
    let lang = accept_language.split(',').next().unwrap_or("en").to_string();
    let mut cmd = tokio::process::Command::new(browser);
    if !headless_shell {
        cmd.arg("--headless=new");
    }
    cmd.args([
        "--disable-gpu",
        "--no-first-run",
        "--no-default-browser-check",
        "--disable-extensions",
        "--disable-background-networking",
        "--disable-sync",
        "--mute-audio",
        "--hide-scrollbars",
        "--window-size=1366,900",
        "--virtual-time-budget=6000",
        "--dump-dom",
    ])
    .arg(format!("--user-data-dir={}", profile.display()))
    .arg(format!("--user-agent={user_agent}"))
    .arg(format!("--lang={lang}"));
    if let Some(p) = proxy {
        cmd.arg(format!("--proxy-server={p}"));
    }
    cmd.arg(url)
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::null())
    .kill_on_drop(true);
    let result = async {
        let mut child = cmd.spawn().map_err(|e| format!("браузер не запустился: {e}"))?;
        let mut out = Vec::new();
        let mut stdout = child.stdout.take().ok_or("нет вывода браузера")?;
        let read = async {
            stdout.read_to_end(&mut out).await.map_err(|e| e.to_string())?;
            child.wait().await.map_err(|e| e.to_string())
        };
        match tokio::time::timeout(timeout, read).await {
            Ok(Ok(_)) => Ok(String::from_utf8_lossy(&out).into_owned()),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(format!("рендер дольше {} с", timeout.as_secs())),
        }
    }
    .await;
    let _ = tokio::fs::remove_dir_all(&profile).await;
    match result {
        Ok(html) if html.trim().is_empty() => Err("браузер вернул пустую страницу".into()),
        other => other,
    }
}

fn rand_suffix() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0)
}
