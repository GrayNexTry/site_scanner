//! maps-seo-scanner: локальный сервер сканера (API + веб-интерфейс).
//!
//! Запуск: `maps-seo-scanner [--listen 127.0.0.1:8787] [--data-dir DIR] [--open]`.
//! Проверка одного сайта из консоли: `maps-seo-scanner audit <url> [--country de] …`.
//! Данные — в $XDG_DATA_HOME/maps-seo-scanner (по умолчанию ~/.local/share/maps-seo-scanner).

mod ai;
mod api;
mod assets;
mod cli_audit;
mod db;
mod jobs;
mod mail;
mod oauth;
mod settings;

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::Context;
use clap::Parser;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "maps-seo-scanner", version, about = "Сканер карт: компании без сайта или со слабым сайтом")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
    /// Адрес и порт. Только localhost: в настройках хранятся ключи API.
    #[arg(long, default_value = "127.0.0.1:8787")]
    listen: SocketAddr,
    /// Папка с базой данных.
    #[arg(long)]
    data_dir: Option<PathBuf>,
    /// Открыть интерфейс в браузере после запуска.
    #[arg(long)]
    open: bool,
}

#[derive(clap::Subcommand)]
enum Cmd {
    /// Проверить один сайт по каталогу метрик и вывести провалы (для разработки сайтов).
    Audit(cli_audit::AuditArgs),
}

fn data_dir(cli: &Cli) -> anyhow::Result<PathBuf> {
    let dir = match &cli.data_dir {
        Some(d) => d.clone(),
        None => std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
            .context("не задан HOME")?
            .join("maps-seo-scanner"),
    };
    std::fs::create_dir_all(&dir).with_context(|| format!("не создать {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    Ok(dir)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    if let Some(Cmd::Audit(args)) = cli.cmd {
        // Отчёт — в stdout, журнал — в stderr и только предупреждения.
        tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")))
            .with_writer(std::io::stderr)
            .with_target(false)
            .init();
        let dir = data_dir(&Cli { cmd: None, ..cli })?;
        std::process::exit(cli_audit::run(args, &dir).await?);
    }
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,hyper=warn,reqwest=warn")))
        .with_target(false)
        .init();
    if !cli.listen.ip().is_loopback() {
        warn!("сервер слушает {}: интерфейс и ключи API доступны из сети", cli.listen);
    }
    let dir = data_dir(&cli)?;
    let db = db::Db::open(&dir.join("scanner.db"))?;
    let interrupted = db.recover_interrupted().await?;
    if interrupted > 0 {
        info!("прерванных сканов: {interrupted} — их можно доделать кнопкой «Продолжить»");
    }
    let state = jobs::AppState::new(db).await?;
    // Рассылка по почте: фоновая задача, спит, пока её не запустят и пока в очереди нет писем.
    mail::spawn_worker(state.clone());
    // Старые проверки без мини-отчёта — пересчитать из сохранённых результатов (без сети).
    {
        let st = state.clone();
        tokio::spawn(async move {
            // Каталог поменялся (метрики добавили или убрали) — привести к нему все сохранённые проверки.
            let fp = scanner_core::checks::catalog_fingerprint();
            let changed = st.db.get_setting("catalog_fingerprint").await.ok().flatten().as_deref() != Some(fp.as_str());
            match jobs::rescore(&st, None, !changed).await {
                Ok(0) => {}
                Ok(n) => info!("пересчитаны оценки и мини-отчёты: {n}"),
                Err(e) => warn!("пересчёт оценок: {e:#}"),
            }
            if changed && let Err(e) = st.db.set_setting("catalog_fingerprint", fp).await {
                warn!("отпечаток каталога: {e:#}");
            }
        });
    }
    api::set_self_url(cli.listen);
    let app = api::router(state);
    let listener = tokio::net::TcpListener::bind(cli.listen).await.with_context(|| format!("порт {} занят?", cli.listen))?;
    let url = format!("http://{}", cli.listen);
    info!("сканер работает: {url}  (данные: {})", dir.display());
    if cli.open {
        let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
    }
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
            info!("остановка");
        })
        .await?;
    Ok(())
}
