//! Провайдеры карт. OpenStreetMap работает без ключа; Google Places (New) и
//! Яндекс «Поиск по организациям» — с ключом. У Apple нет публичного API поиска
//! организаций с сайтами, поэтому он только перечисляется как недоступный.

pub mod google;
pub mod merge;
pub mod osm;
pub mod overture;
pub mod presets;
pub mod yandex;

use serde::Serialize;

use crate::model::{Place, ProviderKind, ScanParams};

#[derive(Debug, Clone, Serialize)]
pub struct ProviderInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub available: bool,
    pub needs_key: bool,
    pub note: &'static str,
}

/// Описание провайдеров для интерфейса с учётом заданных ключей.
pub fn describe(google_key: bool, yandex_key: bool) -> Vec<ProviderInfo> {
    vec![
        ProviderInfo {
            id: "osm",
            label: "OpenStreetMap",
            available: true,
            needs_key: false,
            note: "Бесплатно, без ключа. Покрытие сайтов и телефонов неполное, рейтингов нет.",
        },
        ProviderInfo {
            id: "overture",
            label: "Overture Maps",
            available: true,
            needs_key: false,
            note: "Бесплатно, без ключа: открытые данные Meta, Microsoft, Amazon, TomTom. Много мест с сайтами, телефонами и соцсетями; обновляется раз в месяц.",
        },
        ProviderInfo {
            id: "google",
            label: "Google Карты",
            available: google_key,
            needs_key: true,
            note: "Places API (New): сайт, рейтинг, отзывы, часы, статус. До 60 мест на запрос, большие районы делятся на квадраты.",
        },
        ProviderInfo {
            id: "yandex",
            label: "Яндекс Карты",
            available: yandex_key,
            needs_key: true,
            note: "API «Поиск по организациям»: сайт, телефоны, часы, рубрики. Рейтингов в API нет.",
        },
        ProviderInfo {
            id: "apple",
            label: "Apple Карты",
            available: false,
            needs_key: false,
            note: "У Apple нет публичного API поиска организаций с сайтами и рейтингами.",
        },
    ]
}

/// Текст запроса: пользовательский или из пресета на языке страны скана.
pub fn query_text(p: &ScanParams) -> String {
    if !p.query.trim().is_empty() {
        return p.query.trim().to_string();
    }
    let ru = crate::region::russian_speaking(p.country.as_deref());
    p.preset
        .as_deref()
        .and_then(presets::find)
        .map(|x| if ru { x.query } else { x.query_en }.to_string())
        .unwrap_or_default()
}

/// Признаки ниши для поиска на сайте: слова пресета на разных языках или основы слов запроса.
pub fn service_terms(p: &ScanParams) -> Vec<String> {
    if p.query.trim().is_empty()
        && let Some(preset) = p.preset.as_deref().and_then(presets::find)
    {
        return preset.keywords.iter().map(|k| k.to_string()).collect();
    }
    crate::text::stems(&query_text(p))
}

/// Язык ответа провайдеров карт: русский в русскоязычных странах, иначе английский.
pub fn answer_lang(p: &ScanParams) -> &'static str {
    if crate::region::russian_speaking(p.country.as_deref()) { "ru" } else { "en" }
}

pub struct Keys {
    pub google: Option<String>,
    pub yandex: Option<String>,
}

/// Результат одного провайдера: места или ошибка (скан продолжается с остальными).
pub struct ProviderOutcome {
    pub provider: ProviderKind,
    pub result: Result<Vec<Place>, String>,
    /// Места есть, но с оговоркой: например, собраны запасным путём и могут быть неполными.
    pub notice: Option<String>,
}

pub async fn search_all(api: &reqwest::Client, params: &ScanParams, keys: &Keys) -> Vec<ProviderOutcome> {
    let tasks = params.providers.iter().map(|kind| async move {
        let mut notice = None;
        let result = match kind {
            ProviderKind::Osm => osm::search(api, params).await.map(|(places, note)| {
                notice = note;
                places
            }),
            ProviderKind::Google => match keys.google.as_deref().filter(|k| !k.is_empty()) {
                Some(k) => google::search(api, k, params).await,
                None => Err("не задан ключ Google Places".into()),
            },
            ProviderKind::Yandex => match keys.yandex.as_deref().filter(|k| !k.is_empty()) {
                Some(k) => yandex::search(api, k, params).await,
                None => Err("не задан ключ Яндекс «Поиск по организациям»".into()),
            },
            ProviderKind::Overture => overture::search(api, params).await,
            ProviderKind::Manual => Ok(vec![]),
        };
        ProviderOutcome { provider: *kind, result, notice }
    });
    futures::future::join_all(tasks).await
}
