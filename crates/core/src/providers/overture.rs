//! Overture Maps Places — открытые данные мест (Meta, Microsoft, Amazon, TomTom и др.;
//! лицензия CDLA-Permissive-2.0, нужна атрибуция). Бесплатно, без ключа, весь мир; у мест
//! есть сайты, телефоны, почта и ссылки на соцсети.
//!
//! Данные — файлы GeoParquet на S3 (≈ 11 ГБ на выпуск). Читаем выборочно: служебные концовки
//! файлов (кэшируются в памяти), затем только группы строк, чей прямоугольник пересекает круг
//! поиска, и только нужные колонки; строки вне круга отсекаются при чтении.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use arrow_array::{Array, BooleanArray, Float64Array, RecordBatch, StructArray};
use bytes::Bytes;
use futures::future::BoxFuture;
use futures::{FutureExt, StreamExt};
use parquet::arrow::arrow_reader::{ArrowPredicateFn, ArrowReaderOptions, RowFilter};
use parquet::arrow::async_reader::{AsyncFileReader, ParquetRecordBatchStreamBuilder};
use parquet::arrow::ProjectionMask;
use parquet::errors::{ParquetError, Result as PqResult};
use parquet::file::metadata::{ParquetMetaData, ParquetMetaDataReader, RowGroupMetaData};
use parquet::file::statistics::Statistics;
use reqwest::header::RANGE;
use serde_json::Value;

use super::presets;
use crate::model::{Place, ProviderKind, ScanParams};
use crate::text::haversine_m;

const BASE: &str = "https://overturemaps-us-west-2.s3.us-west-2.amazonaws.com";
/// Ниже этой уверенности Overture места часто закрыты или выдуманы.
const MIN_CONFIDENCE: f64 = 0.5;
/// Соседние куски файла ближе этого склеиваются в один запрос.
const COALESCE: u64 = 512 * 1024;
/// Список файлов выпуска живёт столько, потом проверяем, не вышел ли новый (раз в месяц).
const RELEASE_TTL: Duration = Duration::from_secs(12 * 3600);

fn pq_err(e: impl std::fmt::Display) -> ParquetError {
    ParquetError::General(e.to_string())
}

fn tags<'a>(xml: &'a str, tag: &str) -> Vec<&'a str> {
    let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
    xml.split(open.as_str()).skip(1).filter_map(|x| x.split(close.as_str()).next()).collect()
}

/// Когда прочитан список, выпуск и его файлы (адрес, размер).
type ReleaseCache = Option<(Instant, String, Vec<(String, u64)>)>;

/// Последний выпуск и его файлы мест: (адрес, размер).
async fn release_files(api: &reqwest::Client) -> Result<(String, Vec<(String, u64)>), String> {
    static CACHE: OnceLock<tokio::sync::Mutex<ReleaseCache>> = OnceLock::new();
    let mut cache = CACHE.get_or_init(Default::default).lock().await;
    if let Some((at, release, files)) = cache.as_ref()
        && at.elapsed() < RELEASE_TTL
    {
        return Ok((release.clone(), files.clone()));
    }
    let list = |prefix: String, delimiter: bool| {
        let mut q = vec![("list-type", "2".to_string()), ("prefix", prefix)];
        if delimiter {
            q.push(("delimiter", "/".into()));
        }
        api.get(format!("{BASE}/")).query(&q).timeout(Duration::from_secs(30)).send()
    };
    let xml = list("release/".into(), true).await.map_err(|e| format!("S3: {e}"))?.text().await.map_err(|e| e.to_string())?;
    let release = tags(&xml, "Prefix")
        .into_iter()
        .filter(|p| p.len() > "release/".len())
        .max()
        .ok_or("Overture: не найден ни один выпуск")?
        .trim_start_matches("release/")
        .trim_end_matches('/')
        .to_string();
    let xml = list(format!("release/{release}/theme=places/type=place/"), false)
        .await
        .map_err(|e| format!("S3: {e}"))?
        .text()
        .await
        .map_err(|e| e.to_string())?;
    let files: Vec<(String, u64)> = tags(&xml, "Contents")
        .into_iter()
        .filter_map(|c| {
            let key = tags(c, "Key").into_iter().next()?;
            let size = tags(c, "Size").into_iter().next()?.parse().ok()?;
            key.ends_with(".parquet").then(|| (format!("{BASE}/{key}"), size))
        })
        .collect();
    if files.is_empty() {
        return Err(format!("Overture: в выпуске {release} нет файлов мест"));
    }
    *cache = Some((Instant::now(), release.clone(), files.clone()));
    Ok((release, files))
}

async fn fetch_range(api: &reqwest::Client, url: &str, r: Range<u64>) -> PqResult<Bytes> {
    let resp = api
        .get(url)
        .header(RANGE, format!("bytes={}-{}", r.start, r.end.saturating_sub(1)))
        .timeout(Duration::from_secs(120))
        .send()
        .await
        .map_err(pq_err)?;
    let status = resp.status().as_u16();
    let body = resp.bytes().await.map_err(pq_err)?;
    match status {
        206 => Ok(body),
        // Сервер проигнорировал Range и отдал файл целиком.
        200 if body.len() as u64 >= r.end => Ok(body.slice(r.start as usize..r.end as usize)),
        s => Err(pq_err(format!("Overture: ответ {s}"))),
    }
}

/// Файл на S3, читаемый кусками по HTTP Range.
struct Remote {
    api: reqwest::Client,
    url: String,
    size: u64,
}

fn meta_cache() -> &'static Mutex<HashMap<String, Arc<ParquetMetaData>>> {
    static META: OnceLock<Mutex<HashMap<String, Arc<ParquetMetaData>>>> = OnceLock::new();
    META.get_or_init(Default::default)
}

impl AsyncFileReader for Remote {
    fn get_bytes(&mut self, range: Range<u64>) -> BoxFuture<'_, PqResult<Bytes>> {
        let (api, url) = (self.api.clone(), self.url.clone());
        async move { fetch_range(&api, &url, range).await }.boxed()
    }

    /// Куски колонок одной группы строк лежат рядом: склеиваем и качаем параллельно.
    fn get_byte_ranges(&mut self, ranges: Vec<Range<u64>>) -> BoxFuture<'_, PqResult<Vec<Bytes>>> {
        let (api, url) = (self.api.clone(), self.url.clone());
        async move {
            let mut sorted: Vec<Range<u64>> = ranges.clone();
            sorted.sort_by_key(|r| r.start);
            let mut merged: Vec<Range<u64>> = Vec::new();
            for r in sorted {
                match merged.last_mut() {
                    Some(m) if r.start <= m.end + COALESCE => m.end = m.end.max(r.end),
                    _ => merged.push(r),
                }
            }
            let bodies = futures::future::try_join_all(merged.iter().map(|m| fetch_range(&api, &url, m.clone()))).await?;
            ranges
                .iter()
                .map(|r| {
                    let (i, m) = merged.iter().enumerate().find(|(_, m)| m.start <= r.start && r.end <= m.end).ok_or_else(|| pq_err("кусок вне диапазона"))?;
                    Ok(bodies[i].slice((r.start - m.start) as usize..(r.end - m.start) as usize))
                })
                .collect()
        }
        .boxed()
    }

    fn get_metadata<'a>(&'a mut self, _options: Option<&'a ArrowReaderOptions>) -> BoxFuture<'a, PqResult<Arc<ParquetMetaData>>> {
        async move {
            if let Some(m) = meta_cache().lock().unwrap().get(&self.url).cloned() {
                return Ok(m);
            }
            let size = self.size;
            let m = Arc::new(ParquetMetaDataReader::new().with_prefetch_hint(Some(1 << 20)).load_and_finish(&mut *self, size).await?);
            meta_cache().lock().unwrap().insert(self.url.clone(), m.clone());
            Ok(m)
        }
        .boxed()
    }
}

fn stat(rg: &RowGroupMetaData, path: &str, max: bool) -> Option<f64> {
    let c = rg.columns().iter().find(|c| c.column_path().string() == path)?;
    match c.statistics()? {
        Statistics::Double(s) => if max { s.max_opt() } else { s.min_opt() }.copied(),
        Statistics::Float(s) => if max { s.max_opt() } else { s.min_opt() }.map(|x| *x as f64),
        _ => None,
    }
}

/// Группы строк, чей охват пересекает прямоугольник (запад, юг, восток, север).
fn row_groups(meta: &ParquetMetaData, (w, s, e, n): (f64, f64, f64, f64)) -> Vec<usize> {
    meta.row_groups()
        .iter()
        .enumerate()
        .filter(|(_, rg)| match (stat(rg, "bbox.xmin", false), stat(rg, "bbox.xmax", true), stat(rg, "bbox.ymin", false), stat(rg, "bbox.ymax", true)) {
            (Some(x0), Some(x1), Some(y0), Some(y1)) => x0 <= e && x1 >= w && y0 <= n && y1 >= s,
            _ => true,
        })
        .map(|(i, _)| i)
        .collect()
}

/// Какие колонки читать: только то, что идёт в карточку места.
fn wanted(path: &str) -> bool {
    matches!(
        path,
        "id" | "confidence" | "operating_status" | "basic_category" | "taxonomy.primary" | "names.primary" | "brand.wikidata" | "brand.names.primary"
    ) || path.starts_with("bbox.")
        || ["websites.", "emails.", "socials.", "phones."].iter().any(|p| path.starts_with(p))
        || (path.starts_with("addresses.") && [".freeform", ".locality", ".postcode", ".country"].iter().any(|s| path.ends_with(s)))
}

fn f64_col<'a>(s: &'a StructArray, name: &str) -> Option<&'a Float64Array> {
    s.column_by_name(name)?.as_any().downcast_ref::<Float64Array>()
}

/// Строки одного файла внутри круга поиска — в виде JSON (как их отдаёт arrow-json).
async fn read_file(api: reqwest::Client, url: String, size: u64, p: &ScanParams, bbox: (f64, f64, f64, f64)) -> Result<Vec<Value>, String> {
    let reader = Remote { api, url, size };
    let builder = ParquetRecordBatchStreamBuilder::new(reader).await.map_err(|e| e.to_string())?;
    let groups = row_groups(builder.metadata(), bbox);
    if groups.is_empty() {
        return Ok(Vec::new());
    }
    let schema = builder.parquet_schema();
    let leaves: Vec<usize> = schema.columns().iter().enumerate().filter(|(_, c)| wanted(&c.path().string())).map(|(i, _)| i).collect();
    let bbox_leaves: Vec<usize> = schema.columns().iter().enumerate().filter(|(_, c)| c.path().string().starts_with("bbox.")).map(|(i, _)| i).collect();
    let projection = ProjectionMask::leaves(schema, leaves);
    let bbox_mask = ProjectionMask::leaves(schema, bbox_leaves);
    let (lat, lon, r) = (p.center.lat, p.center.lon, p.radius_m);
    // Отсев по кругу прямо при чтении: остальные колонки декодируются только для попавших строк.
    let inside = ArrowPredicateFn::new(bbox_mask, move |batch: RecordBatch| {
        let n = batch.num_rows();
        let bb = batch.column_by_name("bbox").and_then(|c| c.as_any().downcast_ref::<StructArray>());
        let cols = bb.and_then(|s| Some((f64_col(s, "xmin")?, f64_col(s, "xmax")?, f64_col(s, "ymin")?, f64_col(s, "ymax")?)));
        Ok(match cols {
            Some((x0, x1, y0, y1)) => (0..n)
                .map(|i| {
                    let (x, y) = ((x0.value(i) + x1.value(i)) / 2.0, (y0.value(i) + y1.value(i)) / 2.0);
                    Some(haversine_m(lat, lon, y, x) <= r)
                })
                .collect::<BooleanArray>(),
            None => BooleanArray::from(vec![false; n]),
        })
    });
    let mut stream = builder
        .with_projection(projection)
        .with_row_groups(groups)
        .with_row_filter(RowFilter::new(vec![Box::new(inside)]))
        .with_batch_size(8192)
        .build()
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    while let Some(batch) = stream.next().await {
        let batch = batch.map_err(|e| e.to_string())?;
        if batch.num_rows() == 0 {
            continue;
        }
        let mut buf = Vec::new();
        {
            let mut w = arrow_json::ArrayWriter::new(&mut buf);
            w.write(&batch).map_err(|e| e.to_string())?;
            w.finish().map_err(|e| e.to_string())?;
        }
        let rows: Vec<Value> = serde_json::from_slice(&buf).map_err(|e| e.to_string())?;
        out.extend(rows);
    }
    Ok(out)
}

/// Подходит ли место под запрос скана.
fn matches_scan(p: &ScanParams, name: &str, primary: &str, basic: &str, contact: bool) -> bool {
    if !p.query.trim().is_empty() {
        return name.to_lowercase().contains(&p.query.trim().to_lowercase());
    }
    if let Some(id) = p.preset.as_deref() {
        let pats = presets::overture_patterns(id);
        return pats.iter().any(|x| primary.contains(x) || basic.contains(x));
    }
    if basic.is_empty() && primary.is_empty() {
        // Без категории берём только то, с чем можно связаться: это почти всегда бизнес.
        return contact;
    }
    !presets::OVERTURE_NOT_BUSINESS.contains(&basic) && !primary.contains("worship")
}

/// Русское название категории Overture для колонки «категория».
fn label(primary: &str, basic: &str) -> Option<&'static str> {
    Some(match primary {
        "beauty_salon" => "Салон красоты",
        "hair_salon" => "Парикмахерская",
        "barber" => "Барбершоп",
        "nail_salon" => "Маникюр",
        "spa" => "Спа",
        "massage_therapy" => "Массаж",
        "tattoo_and_piercing" => "Тату",
        "coffee_shop" => "Кофейня",
        "pizza_restaurant" => "Пиццерия",
        "sushi_restaurant" => "Суши",
        "bakery" => "Пекарня",
        "dance_studio" => "Танцы",
        "yoga_studio" => "Йога",
        "language_school" => "Языковая школа",
        "driving_school" => "Автошкола",
        "car_wash" => "Автомойка",
        "veterinarian" => "Ветклиника",
        "florist" | "flower_shop" => "Цветы",
        "furniture_store" => "Мебель",
        "pharmacy" => "Аптека",
        "shopping_mall" => "Торговый центр",
        "gas_station" => "АЗС",
        "hostel" => "Хостел",
        "real_estate_agent" => "Недвижимость",
        "grocery_store" => "Продукты",
        "clothing_store" | "womens_clothing_store" | "mens_clothing_store" => "Одежда",
        "shoe_store" => "Обувь",
        "jewelry_store" => "Ювелирный",
        "eyewear_store" => "Оптика",
        "pet_store" => "Зоотовары",
        "bookstore" => "Книги",
        "tour_operator" | "travel_agent" => "Турагентство",
        "software_development" | "information_technology_company" => "IT-компания",
        "advertising_agency" | "marketing_agency" | "internet_marketing_service" => "Реклама, маркетинг",
        "insurance_agency" => "Страхование",
        "psychology" | "counseling" => "Психолог",
        "event_photography_service" => "Фотограф",
        "party_and_event_planning" | "wedding_planning" => "Праздники",
        "interior_design" => "Дизайн интерьера",
        "auto_parts_store" => "Автозапчасти",
        _ => match basic {
            "restaurant" => "Ресторан",
            "cafe" | "casual_eatery" => "Кафе",
            "coffee_shop" => "Кофейня",
            "bar" | "lounge" => "Бар",
            "fast_food_restaurant" => "Фастфуд",
            "personal_or_beauty_service" => "Красота",
            "wellness_service" => "Спа, массаж",
            "fashion_and_apparel_store" => "Одежда",
            "shopping" | "specialty_store" => "Магазин",
            "food_and_beverage_store" | "convenience_store" => "Продукты",
            "travel_service" => "Туризм",
            "professional_service" | "b2b_office_and_professional_service" => "Услуги",
            "event_or_party_service" => "Праздники",
            "real_estate_service" => "Недвижимость",
            "hardware_home_and_garden_store" => "Дом и ремонт",
            "specialty_school" | "educational_service" | "education" => "Обучение",
            "sport_or_fitness_facility" | "gym" | "fitness_studio" => "Фитнес, спорт",
            "bank_or_credit_union" => "Банк",
            "electronics_store" => "Электроника",
            "lodging" | "hotel" | "private_lodging" => "Гостиница",
            "flowers_and_gifts_store" => "Цветы, подарки",
            "automotive_service" => "Автосервис",
            "auto_dealer" | "vehicle_dealer" => "Автосалон",
            "dental_clinic" => "Стоматология",
            "pharmacy_and_drug_store" => "Аптека",
            "health_care" | "outpatient_care_facility" => "Медицина",
            "behavioral_or_mental_health_clinic" => "Психолог",
            "attorney_or_law_firm" | "legal_service" => "Юрист",
            "art_gallery" => "Галерея",
            "museum" => "Музей",
            "technical_service" => "IT и техника",
            "media_service" => "Медиа",
            "financial_service" => "Финансы",
            "home_service" => "Услуги для дома",
            "design_service" => "Дизайн",
            "printing_service" => "Печать",
            "animal_or_pet_service" | "animal_and_pet_store" => "Для животных",
            "sporting_goods_store" => "Спорттовары",
            "books_music_and_video_store" => "Книги, музыка",
            "personal_care_and_beauty_store" => "Косметика",
            "corporate_or_business_office" => "Офис компании",
            "building_or_construction_service" => "Строительство",
            "music_venue" | "dance_club" => "Клуб",
            "arts_and_entertainment" => "Развлечения",
            _ => return None,
        },
    })
}

/// Строка Overture → место; None — не подходит под скан или сомнительное.
fn to_place(v: &Value, p: &ScanParams) -> Option<Place> {
    if v["confidence"].as_f64().unwrap_or(0.0) < MIN_CONFIDENCE {
        return None;
    }
    let name = v["names"]["primary"].as_str()?.trim();
    if name.is_empty() {
        return None;
    }
    let bb = &v["bbox"];
    let lon = (bb["xmin"].as_f64()? + bb["xmax"].as_f64()?) / 2.0;
    let lat = (bb["ymin"].as_f64()? + bb["ymax"].as_f64()?) / 2.0;
    let primary = v["taxonomy"]["primary"].as_str().unwrap_or("");
    let basic = v["basic_category"].as_str().unwrap_or("");
    let list = |k: &str| -> Vec<String> { v[k].as_array().into_iter().flatten().filter_map(|x| x.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect() };
    let (websites, phones, emails, socials) = (list("websites"), list("phones"), list("emails"), list("socials"));
    let contact = !(websites.is_empty() && phones.is_empty() && socials.is_empty());
    if !matches_scan(p, name, primary, basic, contact) {
        return None;
    }
    let address = v["addresses"].as_array().and_then(|a| a.first()).and_then(|a| {
        let street = a["freeform"].as_str().map(str::trim).filter(|s| !s.is_empty())?;
        Some(match a["locality"].as_str().map(str::trim).filter(|l| !l.is_empty() && !street.contains(*l)) {
            Some(city) if crate::region::russian_speaking(p.country.as_deref()) => format!("{city}, {street}"),
            Some(city) => format!("{street}, {city}"),
            None => street.to_string(),
        })
    });
    let category = p
        .preset
        .as_deref()
        .and_then(presets::find)
        .map(|x| x.label.to_string())
        .or_else(|| label(primary, basic).map(str::to_string))
        .or_else(|| Some(if primary.is_empty() { basic } else { primary }.replace('_', " ")).filter(|s| !s.is_empty()));
    Some(Place {
        provider: Some(ProviderKind::Overture),
        provider_id: v["id"].as_str().unwrap_or_default().to_string(),
        name: name.to_string(),
        address,
        lat,
        lon,
        phone: phones.first().cloned(),
        website: websites.first().cloned(),
        category,
        rating: None,
        reviews: None,
        hours: None,
        business_status: match v["operating_status"].as_str() {
            Some("permanently_closed") => Some("CLOSED_PERMANENTLY".into()),
            Some("temporarily_closed") => Some("CLOSED_TEMPORARILY".into()),
            Some("open") => Some("OPERATIONAL".into()),
            _ => None,
        },
        card_url: None,
        email: emails.first().cloned(),
        links: socials,
        brand: v["brand"]["names"]["primary"].as_str().map(str::to_string),
        wikidata: v["brand"]["wikidata"].as_str().map(str::to_string),
    })
}

/// Места Overture в круге скана, ближние первыми.
pub async fn search(api: &reqwest::Client, p: &ScanParams) -> Result<Vec<Place>, String> {
    let (release, files) = release_files(api).await?;
    let dlat = p.radius_m / 111_320.0;
    let dlon = p.radius_m / (111_320.0 * p.center.lat.to_radians().cos().max(0.01));
    let bbox = (p.center.lon - dlon, p.center.lat - dlat, p.center.lon + dlon, p.center.lat + dlat);
    let t = Instant::now();
    let results = futures::future::join_all(files.iter().map(|(url, size)| read_file(api.clone(), url.clone(), *size, p, bbox))).await;
    let mut rows = Vec::new();
    let mut errors = Vec::new();
    for r in results {
        match r {
            Ok(v) => rows.extend(v),
            Err(e) => errors.push(e),
        }
    }
    if rows.is_empty() && !errors.is_empty() {
        return Err(format!("Overture {release}: {}", errors.first().cloned().unwrap_or_default()));
    }
    let mut out: Vec<Place> = rows.iter().filter_map(|v| to_place(v, p)).collect();
    tracing::info!(release, rows = rows.len(), places = out.len(), failed_files = errors.len(), ms = t.elapsed().as_millis() as u64, "Overture");
    out.sort_by(|a, b| {
        let d = |x: &Place| haversine_m(p.center.lat, p.center.lon, x.lat, x.lon);
        d(a).partial_cmp(&d(b)).unwrap_or(std::cmp::Ordering::Equal)
    });
    out.truncate(p.max_places);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::LatLon;

    fn params(preset: Option<&str>) -> ScanParams {
        ScanParams {
            query: String::new(),
            preset: preset.map(str::to_string),
            city: None,
            center: LatLon { lat: 53.9, lon: 27.56 },
            radius_m: 1500.0,
            providers: vec![ProviderKind::Overture],
            max_places: 100,
            country: Some("by".into()),
            city_names: vec![],
            quick: true,
        }
    }

    #[test]
    fn row_to_place_and_filters() {
        let row = serde_json::json!({
            "id": "abc", "confidence": 0.9, "names": {"primary": "Дорсум"}, "basic_category": "gym",
            "taxonomy": {"primary": "gym"}, "websites": ["http://dorsum.by/"], "phones": ["+375291452000"],
            "socials": ["https://www.facebook.com/337586576585877"], "operating_status": "open",
            "addresses": [{"freeform": "ул. Сурганова, 57Б", "locality": "Минск"}],
            "bbox": {"xmin": 27.56, "xmax": 27.56, "ymin": 53.9, "ymax": 53.9}
        });
        let pl = to_place(&row, &params(None)).unwrap();
        assert_eq!(pl.address.as_deref(), Some("Минск, ул. Сурганова, 57Б"));
        assert_eq!(pl.website.as_deref(), Some("http://dorsum.by/"));
        assert_eq!(pl.links.len(), 1);
        assert_eq!(pl.category.as_deref(), Some("Фитнес, спорт"));
        assert!(to_place(&row, &params(Some("fitness"))).is_some());
        assert!(to_place(&row, &params(Some("dentist"))).is_none());
        let park = serde_json::json!({"confidence": 0.9, "names": {"primary": "Парк"}, "basic_category": "park", "bbox": {"xmin": 27.56, "xmax": 27.56, "ymin": 53.9, "ymax": 53.9}});
        assert!(to_place(&park, &params(None)).is_none(), "парк — не бизнес");
        let low = serde_json::json!({"confidence": 0.2, "names": {"primary": "X"}, "basic_category": "cafe", "bbox": {"xmin": 27.56, "xmax": 27.56, "ymin": 53.9, "ymax": 53.9}});
        assert!(to_place(&low, &params(None)).is_none(), "низкая уверенность");
    }

    #[test]
    fn s3_listing_parse() {
        let xml = "<ListBucketResult><CommonPrefixes><Prefix>release/2026-10-21.0/</Prefix></CommonPrefixes><CommonPrefixes><Prefix>release/2026-11-18.0/</Prefix></CommonPrefixes>\
                   <Contents><Key>a.parquet</Key><Size>10</Size></Contents></ListBucketResult>";
        assert_eq!(tags(xml, "Prefix"), vec!["release/2026-10-21.0/", "release/2026-11-18.0/"]);
        assert_eq!(tags(tags(xml, "Contents")[0], "Size"), vec!["10"]);
    }

    /// Живой запрос к S3: `cargo test -p scanner-core overture_live -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn overture_live() {
        let api = reqwest::Client::builder().user_agent("maps-seo-scanner/0.1 (local tool)").build().unwrap();
        let mut p = params(None);
        p.max_places = 20_000;
        let t = Instant::now();
        let places = search(&api, &p).await.unwrap();
        println!("мест: {} за {:?}; с сайтом: {}", places.len(), t.elapsed(), places.iter().filter(|x| x.website.is_some()).count());
        for x in places.iter().take(5) {
            println!("{} | {:?} | {:?} | {:?}", x.name, x.category, x.address, x.website);
        }
        assert!(places.len() > 500);
        let t = Instant::now();
        let again = search(&api, &params(Some("dentist"))).await.unwrap();
        println!("стоматологий: {} за {:?} (метаданные из кэша)", again.len(), t.elapsed());
    }
}
