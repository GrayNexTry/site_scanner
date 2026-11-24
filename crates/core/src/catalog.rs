//! Каталог метрик: встраивается из docs/metrics.json (генерирует scripts/gen_metrics_yaml.py).

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

const CATALOG_JSON: &str = include_str!("../../../docs/metrics.json");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Threshold {
    pub op: String,
    pub value: serde_json::Value,
    #[serde(default)]
    pub unit: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricDef {
    pub id: String,
    pub category: String,
    pub name: String,
    pub method: String,
    pub rule: String,
    /// ext | access | both
    pub source: String,
    /// check | kpi | info
    pub kind: String,
    /// true | "semi"
    pub auto: serde_json::Value,
    /// lead | audit | report
    pub stage: String,
    pub weight: f64,
    pub pass: Option<Threshold>,
    pub warn: Option<Threshold>,
    pub stop_factor: bool,
    pub lead_signal: Option<String>,
    pub in_report: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Catalog {
    pub version: String,
    pub categories: BTreeMap<String, String>,
    pub category_weights: BTreeMap<String, f64>,
    pub metrics: Vec<MetricDef>,
}

impl Catalog {
    pub fn get(&self, id: &str) -> Option<&MetricDef> {
        self.metrics.iter().find(|m| m.id == id)
    }
}

/// Каталог, разобранный один раз на процесс.
pub fn catalog() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| serde_json::from_str(CATALOG_JSON).expect("docs/metrics.json повреждён"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_loads_all_metrics() {
        let c = catalog();
        assert_eq!(c.metrics.len(), 214);
        assert_eq!(c.categories.len(), 17);
        assert!(c.get("J01").is_none(), "раздел J (подсчёт упоминаний в ответах ИИ) убран");
        assert!(c.get("C02").unwrap().stop_factor);
    }

    /// Чеклист для разработки сайтов (скилл агента client-site-build) обязан упоминать каждую
    /// метрику каталога как `[ID]`: добавили метрику — добавьте пункт в docs/site-build/CHECKLIST.md.
    #[test]
    fn site_build_checklist_covers_catalog() {
        let text = include_str!("../../../docs/site-build/CHECKLIST.md");
        let missing: Vec<&str> = catalog().metrics.iter().map(|m| m.id.as_str()).filter(|id| !text.contains(&format!("[{id}]"))).collect();
        assert!(missing.is_empty(), "в docs/site-build/CHECKLIST.md нет метрик: {missing:?}");
    }
}
