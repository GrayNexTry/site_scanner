//! Общие типы: результаты метрик, места с карт, параметры скана.

use serde::{Deserialize, Serialize};

/// Итог одной метрики.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pass,
    Warn,
    Fail,
    /// Справочное значение, в балл не входит.
    Info,
    /// Не измерено: нет данных, ключа, доступа или неприменимо.
    NotMeasured,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricResult {
    pub id: String,
    pub status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,
    /// Пояснение для человека: что именно найдено.
    pub detail: String,
}

impl MetricResult {
    pub fn new(id: &str, status: Status, detail: impl Into<String>) -> Self {
        Self { id: id.to_string(), status, value: None, detail: detail.into() }
    }
    pub fn pass(id: &str, detail: impl Into<String>) -> Self {
        Self::new(id, Status::Pass, detail)
    }
    pub fn warn(id: &str, detail: impl Into<String>) -> Self {
        Self::new(id, Status::Warn, detail)
    }
    pub fn fail(id: &str, detail: impl Into<String>) -> Self {
        Self::new(id, Status::Fail, detail)
    }
    pub fn info(id: &str, detail: impl Into<String>) -> Self {
        Self::new(id, Status::Info, detail)
    }
    pub fn na(id: &str, detail: impl Into<String>) -> Self {
        Self::new(id, Status::NotMeasured, detail)
    }
    pub fn with_value(mut self, v: impl Into<serde_json::Value>) -> Self {
        self.value = Some(v.into());
        self
    }
}

/// Источник данных о месте.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Osm,
    Google,
    Yandex,
    /// Overture Maps: открытые данные мест (Meta, Microsoft и др.).
    Overture,
    /// Ручная проверка сайта без карт.
    Manual,
}

impl ProviderKind {
    pub fn label(self) -> &'static str {
        match self {
            ProviderKind::Osm => "OpenStreetMap",
            ProviderKind::Google => "Google Карты",
            ProviderKind::Yandex => "Яндекс Карты",
            ProviderKind::Overture => "Overture Maps",
            ProviderKind::Manual => "Вручную",
        }
    }
}

/// Карточка места у одного провайдера.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Place {
    pub provider: Option<ProviderKind>,
    pub provider_id: String,
    pub name: String,
    pub address: Option<String>,
    pub lat: f64,
    pub lon: f64,
    pub phone: Option<String>,
    pub website: Option<String>,
    pub category: Option<String>,
    pub rating: Option<f64>,
    pub reviews: Option<u32>,
    /// Часы работы текстом (если есть).
    pub hours: Option<String>,
    /// Статус: OPERATIONAL, CLOSED_TEMPORARILY, CLOSED_PERMANENTLY.
    pub business_status: Option<String>,
    /// Ссылка на карточку на карте.
    pub card_url: Option<String>,
    /// Почта из карточки (OSM): её домен часто и есть сайт.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Прочие ссылки карточки: соцсети, мессенджеры, мультиссылки.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<String>,
    /// Бренд сети (OSM brand) и его Wikidata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brand: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wikidata: Option<String>,
}

/// Компания после объединения карточек разных провайдеров.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Business {
    pub name: String,
    pub address: Option<String>,
    pub lat: f64,
    pub lon: f64,
    pub phone: Option<String>,
    pub website: Option<String>,
    pub category: Option<String>,
    pub sources: Vec<Place>,
    /// Сколько лишних карточек одного провайдера слилось в эту компанию (A19).
    pub duplicates: u32,
    /// Сколько точек с таким же названием в скане (сеть); 0 — не считали.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub chain_size: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl Business {
    pub fn rating(&self) -> Option<f64> {
        self.sources.iter().filter_map(|p| p.rating).fold(None, |acc: Option<f64>, r| {
            Some(acc.map_or(r, |a| a.max(r)))
        })
    }
    pub fn reviews(&self) -> Option<u32> {
        self.sources.iter().filter_map(|p| p.reviews).max()
    }
    pub fn source(&self, kind: ProviderKind) -> Option<&Place> {
        self.sources.iter().find(|p| p.provider == Some(kind))
    }
    /// Почты из всех карточек.
    pub fn emails(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self.sources.iter().filter_map(|p| p.email.as_deref()).collect();
        v.dedup();
        v
    }
    /// Ссылки из всех карточек: поле «сайт» и дополнительные (соцсети и т. п.).
    pub fn all_links(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self.website.as_deref().into_iter().collect();
        for p in &self.sources {
            v.extend(p.website.as_deref());
            v.extend(p.links.iter().map(String::as_str));
        }
        let mut seen = std::collections::HashSet::new();
        v.retain(|x| seen.insert(*x));
        v
    }
    /// Бренд сети, если карточка помечена брендом.
    pub fn brand(&self) -> Option<&str> {
        self.sources.iter().find_map(|p| p.brand.as_deref())
    }
    pub fn wikidata(&self) -> Option<&str> {
        self.sources.iter().find_map(|p| p.wikidata.as_deref())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct LatLon {
    pub lat: f64,
    pub lon: f64,
}

/// Параметры скана, которые влияют на проверки.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanParams {
    /// Текстовый запрос: «стоматология».
    pub query: String,
    /// Идентификатор пресета категории (см. providers::presets).
    #[serde(default)]
    pub preset: Option<String>,
    /// Город или район для проверки вхождений (F06).
    #[serde(default)]
    pub city: Option<String>,
    /// Страна скана, ISO 3166-1 alpha-2 в нижнем регистре («ru», «de», «us»).
    /// От неё зависят язык запросов, формат адресов и правовые проверки.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    /// Все известные названия города: местное, русское, английское (для F06 и поиска сайтов).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub city_names: Vec<String>,
    pub center: LatLon,
    pub radius_m: f64,
    pub providers: Vec<ProviderKind>,
    #[serde(default = "default_max_places")]
    pub max_places: usize,
    /// Быстрая проверка для больших сканов: главная и пара страниц, без PageSpeed и угадывания доменов.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub quick: bool,
}

fn default_max_places() -> usize {
    200
}

impl ScanParams {
    /// Названия города для поиска вхождений: заданное пользователем и известные варианты.
    pub fn city_variants(&self) -> Vec<String> {
        const PREFIXES: &[&str] = &[
            "Greater ", "City of ", "London Borough of ", "Royal Borough of ", "Borough of ", "County of ", "городской округ ", "город ",
            "Stadt ", "Landkreis ", "Ville de ", "Comune di ", "Gemeente ",
        ];
        let mut v: Vec<String> = Vec::new();
        for raw in self.city.iter().chain(self.city_names.iter()) {
            let t = raw.trim();
            if t.is_empty() {
                continue;
            }
            v.push(t.to_string());
            if let Some(rest) = PREFIXES.iter().find_map(|p| t.strip_prefix(p)) {
                v.push(rest.trim().to_string());
            }
        }
        let mut seen = std::collections::HashSet::new();
        v.retain(|x| seen.insert(x.to_lowercase()));
        v
    }
    pub fn jurisdiction(&self) -> crate::region::Jurisdiction {
        crate::region::jurisdiction(self.country.as_deref())
    }
}

/// Медианы по всем компаниям скана — база для относительных порогов.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MarketStats {
    pub rating_median: Option<f64>,
    pub reviews_median: Option<f64>,
}

pub fn median(mut v: Vec<f64>) -> Option<f64> {
    v.retain(|x| x.is_finite());
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    Some(if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 })
}

impl MarketStats {
    pub fn from_businesses(list: &[Business]) -> Self {
        Self {
            rating_median: median(list.iter().filter_map(|b| b.rating()).collect()),
            reviews_median: median(list.iter().filter_map(|b| b.reviews().map(f64::from)).collect()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_odd_even() {
        assert_eq!(median(vec![3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(vec![4.0, 1.0, 2.0, 3.0]), Some(2.5));
        assert_eq!(median(vec![]), None);
    }
}
