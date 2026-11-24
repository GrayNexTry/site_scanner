//! Живая проверка поиска сайтов (сеть). Запуск: cargo test -p scanner-core --test live_discover -- --ignored --nocapture

use scanner_core::discover::{discover, DiscoverOptions};
use scanner_core::http::{FetchConfig, Fetcher};
use scanner_core::{Business, LatLon, Place, ProviderKind, ScanParams};

fn biz(name: &str, phone: Option<&str>, address: Option<&str>) -> Business {
    Business {
        name: name.into(),
        phone: phone.map(Into::into),
        address: address.map(Into::into),
        sources: vec![Place { provider: Some(ProviderKind::Osm), name: name.into(), ..Default::default() }],
        ..Default::default()
    }
}

#[tokio::test]
#[ignore]
async fn brooklyn_salons() {
    let f = Fetcher::new(FetchConfig::default()).unwrap().with_language("en-US,en;q=0.9".into());
    let params = ScanParams {
        query: String::new(),
        preset: Some("hairdresser".into()),
        city: Some("Нью-Йорк".into()),
        country: Some("us".into()),
        city_names: vec!["New York".into(), "Brooklyn".into()],
        center: LatLon { lat: 40.6782, lon: -73.9442 },
        radius_m: 1500.0,
        providers: vec![ProviderKind::Osm],
        max_places: 25,
        quick: false,
    };
    for b in [
        biz("GiGi Glam Box", Some("+1-347-799-8449"), Some("1251 Fulton Street")),
        biz("O Salon", Some("+1-718-604-2741"), None),
        biz("Fulton Barber Shop", Some("+1-929-413-9480"), Some("28 Arlington Place")),
        biz("Benny's Barber Shop", Some("+1-718-637-1200"), Some("809 Franklin Avenue")),
    ] {
        let d = discover(&f, &b, &params, &DiscoverOptions::default()).await;
        println!("{}: found={:?} suggestions={:?} guessed={} resolved={} checked={} {}ms",
            b.name,
            d.found.as_ref().map(|v| (&v.url, v.score, &v.evidence)),
            d.suggestions.iter().map(|v| (&v.url, v.score, &v.evidence)).collect::<Vec<_>>(),
            d.guessed, d.resolved, d.checked, d.elapsed_ms);
    }
}
