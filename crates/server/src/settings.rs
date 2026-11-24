//! Настройки: ключи API и лимиты сканирования. Хранятся в БД (файл 0600),
//! наружу ключи отдаются только маской.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub google_places_key: String,
    pub yandex_geosearch_key: String,
    pub psi_key: String,
    pub safe_browsing_key: String,
    /// Внутренних страниц на сайт.
    pub max_pages: usize,
    /// Выборочно проверяемых ссылок на сайт.
    pub link_checks: usize,
    /// Сайтов одновременно.
    pub concurrency: usize,
    /// Запросов одновременно к одному сайту.
    pub per_site_concurrency: usize,
    pub timeout_secs: u64,
    /// Запускать PageSpeed Insights (медленно: 10–30 с на сайт).
    pub psi_enabled: bool,
    /// Искать сайт в обход карт, если в карточке его нет или там соцсеть.
    pub discover_enabled: bool,
    /// Угадывать домены по названию (DNS + проверка главной).
    pub discover_guess: bool,
    /// Веб-поиск без ключа (DuckDuckGo, Bing): часто отвечает капчей, особенно через VPN.
    pub search_free: bool,
    pub brave_search_key: String,
    pub yandex_search_key: String,
    pub yandex_folder_id: String,
    /// Рендерить в headless-браузере сайты, которые рисуются скриптами.
    pub render_js: bool,
    /// Путь к Chromium/Chrome; пусто — найти автоматически.
    pub browser_path: String,
    /// Свой прокси для сайтов, закрытых по географии: socks5h://, http:// (секрет: может быть с паролем).
    pub proxy_url: String,
    /// Всё через прокси, а не только сайты, которые напрямую не открылись.
    pub proxy_always: bool,
    /// Сайты — напрямую через Wi-Fi/Ethernet, мимо VPN-туннеля (быстрее, без гео-блоков для сайтов
    /// своей страны); не открылся напрямую — повтор через VPN. Без VPN ничего не меняет.
    pub direct_network: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            google_places_key: String::new(),
            yandex_geosearch_key: String::new(),
            psi_key: String::new(),
            safe_browsing_key: String::new(),
            max_pages: 15,
            link_checks: 30,
            concurrency: 8,
            per_site_concurrency: 4,
            timeout_secs: 15,
            psi_enabled: true,
            discover_enabled: true,
            discover_guess: true,
            search_free: true,
            brave_search_key: String::new(),
            yandex_search_key: String::new(),
            yandex_folder_id: String::new(),
            render_js: true,
            browser_path: String::new(),
            proxy_url: String::new(),
            proxy_always: false,
            direct_network: true,
        }
    }
}

const KEY_FIELDS: &[&str] = &["google_places_key", "yandex_geosearch_key", "psi_key", "safe_browsing_key", "brave_search_key", "yandex_search_key", "proxy_url"];

fn mask(k: &str) -> Value {
    let tail: String = k.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
    json!({ "set": !k.is_empty(), "hint": if k.is_empty() { String::new() } else { format!("••••{tail}") } })
}

impl Settings {
    /// Настройки загрузчика: таймаут и свой прокси.
    pub fn fetch_config(&self) -> scanner_core::http::FetchConfig {
        scanner_core::http::FetchConfig {
            timeout: std::time::Duration::from_secs(self.timeout_secs),
            proxy: Some(self.proxy_url.clone()).filter(|p| !p.trim().is_empty()),
            proxy_always: self.proxy_always,
            interface: if self.direct_network { scanner_core::http::direct_interface() } else { None },
            ..Default::default()
        }
    }

    /// Браузер для рендера, если рендер включён и браузер найден.
    pub fn browser(&self) -> Option<std::path::PathBuf> {
        if !self.render_js {
            return None;
        }
        scanner_core::render::find_browser(Some(self.browser_path.as_str()).filter(|p| !p.trim().is_empty()))
    }

    pub fn search_config(&self) -> scanner_core::search::SearchConfig {
        let opt = |s: &str| (!s.is_empty()).then(|| s.to_string());
        scanner_core::search::SearchConfig {
            brave_key: opt(&self.brave_search_key),
            yandex_key: opt(&self.yandex_search_key),
            yandex_folder: opt(&self.yandex_folder_id),
            free_engines: self.search_free,
        }
    }

    /// Для интерфейса: ключи — только «задан/нет» и последние 4 символа.
    pub fn public(&self) -> Value {
        let mut v = serde_json::to_value(self).unwrap_or_default();
        for f in KEY_FIELDS {
            let k = v[f].as_str().unwrap_or("").to_string();
            v[f] = mask(&k);
        }
        v
    }

    /// Применить частичное обновление. Ключ: строка — заменить, "" — удалить, отсутствует — не трогать.
    pub fn apply(&mut self, patch: &Value) {
        let key = |name: &str, target: &mut String| {
            if let Some(s) = patch.get(name).and_then(Value::as_str) {
                *target = s.trim().to_string();
            }
        };
        key("google_places_key", &mut self.google_places_key);
        key("yandex_geosearch_key", &mut self.yandex_geosearch_key);
        key("psi_key", &mut self.psi_key);
        key("safe_browsing_key", &mut self.safe_browsing_key);
        key("brave_search_key", &mut self.brave_search_key);
        key("yandex_search_key", &mut self.yandex_search_key);
        key("yandex_folder_id", &mut self.yandex_folder_id);
        key("browser_path", &mut self.browser_path);
        key("proxy_url", &mut self.proxy_url);
        let num = |name: &str, lo: u64, hi: u64| patch.get(name).and_then(Value::as_u64).map(|x| x.clamp(lo, hi));
        if let Some(x) = num("max_pages", 1, 200) {
            self.max_pages = x as usize;
        }
        if let Some(x) = num("link_checks", 0, 500) {
            self.link_checks = x as usize;
        }
        if let Some(x) = num("concurrency", 1, 32) {
            self.concurrency = x as usize;
        }
        if let Some(x) = num("per_site_concurrency", 1, 16) {
            self.per_site_concurrency = x as usize;
        }
        if let Some(x) = num("timeout_secs", 5, 60) {
            self.timeout_secs = x;
        }
        let flag = |name: &str, target: &mut bool| {
            if let Some(b) = patch.get(name).and_then(Value::as_bool) {
                *target = b;
            }
        };
        flag("psi_enabled", &mut self.psi_enabled);
        flag("discover_enabled", &mut self.discover_enabled);
        flag("discover_guess", &mut self.discover_guess);
        flag("search_free", &mut self.search_free);
        flag("render_js", &mut self.render_js);
        flag("proxy_always", &mut self.proxy_always);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_and_patches() {
        let mut s = Settings::default();
        s.apply(&json!({"google_places_key": " AIzaSECRET1234 ", "max_pages": 1000, "psi_enabled": false}));
        assert_eq!(s.google_places_key, "AIzaSECRET1234");
        assert_eq!(s.max_pages, 200);
        assert!(!s.psi_enabled);
        let p = s.public();
        assert_eq!(p["google_places_key"]["hint"], "••••1234");
        assert!(!p.to_string().contains("SECRET"));
        s.apply(&json!({"max_pages": 20}));
        assert_eq!(s.google_places_key, "AIzaSECRET1234", "ключ без поля в патче не трогаем");
        s.apply(&json!({"google_places_key": ""}));
        assert!(s.google_places_key.is_empty());
        s.apply(&json!({"proxy_url": "socks5h://user:pass@10.0.0.2:1080"}));
        assert!(!s.public().to_string().contains("pass"), "пароль прокси наружу не отдаём");
        assert!(s.fetch_config().proxy.is_some());
    }
}
