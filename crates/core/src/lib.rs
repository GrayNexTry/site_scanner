//! scanner-core: поиск компаний на картах, сбор данных о сайте, проверки по каталогу метрик
//! (docs/METRICS.md) и скоринг лидов. Сетевой код — async (tokio), проверки — чистые функции.

pub mod catalog;
pub mod checks;
pub mod discover;
pub mod domain_info;
pub mod http;
pub mod lexicon;
pub mod llm;
pub mod mini;
mod mini_en;
pub mod model;
pub mod page;
pub mod providers;
pub mod psi;
pub mod public_sector;
pub mod region;
pub mod render;
pub mod robots;
pub mod sales;
pub mod scoring;
pub mod search;
pub mod site;
pub mod sitemap;
pub mod text;
pub mod tls;
pub mod yandex_info;

pub use model::*;
