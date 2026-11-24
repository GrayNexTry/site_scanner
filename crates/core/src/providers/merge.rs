//! Слияние карточек разных провайдеров в компании: одинаковый телефон, свой домен сайта
//! или похожее название рядом (≤ 150 м).

use std::collections::HashMap;

use crate::model::{Business, Place, ProviderKind};
use crate::text::{classify_website, haversine_m, normalize_name, phone_key, registrable_domain, similarity, SiteKind};

struct Dsu(Vec<usize>);

impl Dsu {
    fn find(&mut self, x: usize) -> usize {
        let p = self.0[x];
        if p == x {
            return x;
        }
        let r = self.find(p);
        self.0[x] = r;
        r
    }
    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.0[rb] = ra;
        }
    }
}

fn own_domain(p: &Place) -> Option<String> {
    let w = p.website.as_deref()?;
    let (kind, host) = classify_website(w);
    (kind == SiteKind::Own).then(|| registrable_domain(&host))
}

/// Приоритет источника для полей компании: Яндекс (адреса по-русски), Google, OSM.
fn rank(p: &Place) -> u8 {
    match p.provider {
        Some(ProviderKind::Yandex) => 0,
        Some(ProviderKind::Google) => 1,
        Some(ProviderKind::Osm) => 2,
        Some(ProviderKind::Overture) => 3,
        _ => 4,
    }
}

pub fn merge(places: Vec<Place>) -> Vec<Business> {
    let n = places.len();
    let mut dsu = Dsu((0..n).collect());
    let names: Vec<String> = places.iter().map(|p| normalize_name(&p.name)).collect();

    let mut by_key: HashMap<String, usize> = HashMap::new();
    for (i, p) in places.iter().enumerate() {
        let keys = p
            .phone
            .as_deref()
            .and_then(phone_key)
            .map(|k| format!("tel:{k}"))
            .into_iter()
            .chain(own_domain(p).map(|d| format!("dom:{d}")));
        for k in keys {
            match by_key.get(&k) {
                Some(&j) => dsu.union(j, i),
                None => {
                    by_key.insert(k, i);
                }
            }
        }
    }

    // Похожие названия рядом: сортируем по широте и сравниваем только соседей в окне ~220 м.
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| places[a].lat.partial_cmp(&places[b].lat).unwrap_or(std::cmp::Ordering::Equal));
    for (oi, &i) in order.iter().enumerate() {
        for &j in &order[oi + 1..] {
            if places[j].lat - places[i].lat > 0.002 {
                break;
            }
            let d = haversine_m(places[i].lat, places[i].lon, places[j].lat, places[j].lon);
            if d > 150.0 {
                continue;
            }
            let sim = similarity(&names[i], &names[j]);
            if sim >= 0.82 || (sim >= 0.6 && d <= 40.0) {
                dsu.union(i, j);
            }
        }
    }

    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..n {
        let r = dsu.find(i);
        groups.entry(r).or_default().push(i);
    }
    let mut slots: Vec<Option<Place>> = places.into_iter().map(Some).collect();
    let mut out: Vec<Business> = groups
        .into_values()
        .map(|idx| {
            let mut sources: Vec<Place> = idx.iter().filter_map(|&i| slots[i].take()).collect();
            sources.sort_by_key(rank);
            let mut per_provider: HashMap<Option<ProviderKind>, u32> = HashMap::new();
            for s in &sources {
                *per_provider.entry(s.provider).or_default() += 1;
            }
            // Дубли считаем только у карт, где карточками управляет бизнес (Google, Яндекс):
            // повторы в OSM и Overture — особенности открытых данных, а не проблема компании.
            let duplicates = per_provider
                .iter()
                .filter(|(p, _)| matches!(p, Some(ProviderKind::Google | ProviderKind::Yandex)))
                .map(|(_, c)| c.saturating_sub(1))
                .sum();
            let first = &sources[0];
            let website = sources
                .iter()
                .filter_map(|s| s.website.clone())
                .find(|w| classify_website(w).0 == SiteKind::Own)
                .or_else(|| sources.iter().find_map(|s| s.website.clone()));
            Business {
                name: sources.iter().find(|s| s.provider == Some(ProviderKind::Google)).unwrap_or(first).name.clone(),
                address: sources.iter().find_map(|s| s.address.clone()),
                lat: first.lat,
                lon: first.lon,
                phone: sources.iter().find_map(|s| s.phone.clone()),
                website,
                category: sources.iter().find_map(|s| s.category.clone()),
                duplicates,
                chain_size: 0,
                sources,
            }
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(provider: ProviderKind, id: &str, name: &str, lat: f64, phone: Option<&str>, site: Option<&str>) -> Place {
        Place {
            provider: Some(provider),
            provider_id: id.into(),
            name: name.into(),
            lat,
            lon: 37.6,
            phone: phone.map(str::to_string),
            website: site.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn merges_by_phone_domain_and_name() {
        let list = vec![
            place(ProviderKind::Google, "g1", "Зуб-Мастер", 55.75, Some("+7 495 123-45-67"), Some("https://zub.ru")),
            place(ProviderKind::Yandex, "y1", "Стоматология Зуб Мастер", 55.7501, Some("8 (495) 123-45-67"), None),
            place(ProviderKind::Osm, "o1", "Зуб-Мастер", 55.75005, None, None),
            place(ProviderKind::Osm, "o2", "Колесо", 55.75, None, Some("https://vk.com/koleso")),
            place(ProviderKind::Google, "g2", "Шины", 55.76, None, Some("https://vk.com/shiny")),
        ];
        let b = merge(list);
        assert_eq!(b.len(), 3, "соцсеть не должна склеивать разные компании");
        let zub = b.iter().find(|x| x.sources.len() == 3).unwrap();
        assert_eq!(zub.website.as_deref(), Some("https://zub.ru"));
        assert_eq!(zub.duplicates, 0);
    }

    #[test]
    fn counts_duplicates_within_provider() {
        let list = vec![
            place(ProviderKind::Google, "g1", "Колесо", 55.75, Some("+7 495 000-00-01"), None),
            place(ProviderKind::Google, "g2", "Колесо шиномонтаж", 55.7502, Some("+7 495 000-00-01"), None),
        ];
        let b = merge(list);
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].duplicates, 1);
    }
}
