//! Мини-отчёт о ситуации с сайтом: одна строка состояния, главные проблемы простыми словами
//! и что предложить. Для таблицы скана, сводки по району, выгрузки и первого письма клиенту.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::catalog::catalog;
use crate::discover::Discovery;
use crate::model::{MetricResult, Status};

/// Что с сайтом у компании — для сводки по району и фильтров.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SiteState {
    /// Не проверено.
    #[default]
    Unknown,
    /// Сайта нет.
    None,
    /// Вместо сайта соцсеть, мультиссылка или агрегатор.
    Social,
    /// Сайт есть, но в карточке не указан (найден в обход карт).
    Hidden,
    /// Сайт не открывается.
    Dead,
    /// Заглушка, парковка домена, шаблонный текст.
    Stub,
    /// Сайт не пустил сканер (защита от ботов, география).
    Blocked,
    /// Балл ниже 50.
    Weak,
    /// 50–74.
    Fair,
    /// 75 и выше.
    Good,
    /// Точка сети: сайтом занимается головная компания.
    Chain,
    Closed,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MiniReport {
    pub state: SiteState,
    /// Одна строка: «Сайта нет», «Слабый сайт: 38/100, 9 критичных проблем».
    pub headline: String,
    /// До пяти главных проблем — так, чтобы понял владелец бизнеса.
    pub problems: Vec<String>,
    /// Что предложить клиенту.
    pub offer: Vec<String>,
    /// Все проваленные метрики — для «частых проблем» по району.
    pub fail_ids: Vec<String>,
}

/// Что знает оценка к моменту мини-отчёта.
pub struct Context<'a> {
    pub overall: Option<f64>,
    pub closed: bool,
    pub chain: Option<String>,
    pub discovery: Option<&'a Discovery>,
}

/// Проблема простыми словами по номеру метрики — для сводки «частые проблемы».
pub fn label(id: &str) -> Option<&'static str> {
    Some(match id {
        "B04" => "Нет HTTPS — браузер пишет «Не защищено»",
        "B05" => "Нет перенаправления с http на https",
        "B06" => "Сайт открывается по двум адресам (с www и без) — дубль для поисковиков",
        "B08" => "Сервер отвечает медленно",
        "B12" => "Несуществующие страницы не отдают ошибку 404",
        "C02" => "robots.txt закрывает сайт от поисковиков",
        "C04" | "C06" => "Нет карты сайта (sitemap) для поисковиков",
        "C10" => "Главная закрыта от индексации (noindex)",
        "C12" => "На сайте битые ссылки",
        "D01" | "D05" => "Сайт медленно грузится на телефоне",
        "E01" => "Нет мобильной версии",
        "E05" => "Телефон нельзя нажать с мобильного",
        "F01" => "Заголовок страницы не говорит, что за услуга и где",
        "F02" => "Нет описания сайта для поисковой выдачи",
        "F03" => "Нет главного заголовка (H1)",
        "F06" => "В заголовках нет «услуга + город» — по таким запросам сайт не находится",
        "F07" => "Нет отдельных страниц под услуги",
        "F10" => "Картинки без подписей (alt)",
        "F11" => "Ссылка на сайт в мессенджерах без превью",
        "F12" => "Нет иконки сайта (favicon)",
        "F13" => "Сайт выглядит заброшенным",
        "G01" | "G02" => "Нет разметки для поисковиков и ИИ: адрес, часы, телефон",
        "H01" | "I07" => "Нет адреса и телефона текстом",
        "H02" => "Контакты на сайте не совпадают с картами",
        "H03" => "Часы работы на сайте не совпадают с картами",
        "I02" | "I03" => "Сайт закрыт от ИИ-поиска (ChatGPT, Gemini, Perplexity, Алиса) в robots.txt",
        "I05" => "Защита сайта может не пускать ИИ-ботов — проверить настройки CDN",
        "I06" => "Сайт рисуется скриптами — ИИ-ассистенты видят пустую страницу",
        "I08" => "Нет цен — ни клиент, ни ИИ не найдут ответ на «сколько стоит»",
        "I09" => "Нет ответов на частые вопросы клиентов",
        "I17" => "Не видно, когда обновлялись цены и услуги — ИИ берёт свежие источники",
        "I18" => "На первом экране нет готового ответа «что, где, почём» — ИИ нечего процитировать",
        "I19" => "Мало конкретики: сроков, гарантий, цифр — ИИ цитирует тех, у кого факты",
        "I20" => "Вместо цен «договорная» или без условий — ИИ ответит ценой конкурента",
        "I21" => "Страница тяжелее 2 МБ — Google читает только начало",
        "M01" => "Нет политики обработки персональных данных — риск штрафа",
        "M02" => "Формы без согласия на обработку данных",
        "M03" => "Нет уведомления о cookie",
        "M05" => "Нет обязательных реквизитов компании",
        "N01" => "На первом экране не видно услуги и кнопки записи",
        "N02" => "Телефона нет в шапке сайта",
        "N03" => "Нет кнопок мессенджеров",
        "N04" => "Нет онлайн-записи",
        "N06" => "Нет отзывов на первых экранах",
        "O01" => "Нет базовой защиты сайта (заголовки безопасности)",
        "O02" => "Часть ресурсов грузится по http — браузер ругается",
        "P01" => "Нет аналитики — не посчитать заявки с сайта",
        "Q03" => "Поля форм без подписей",
        "R01" => "Сайт не связан с карточкой в Яндекс Картах — Яндексу неясно, к какому городу его отнести",
        "R03" => "Домен в зоне другой страны — Яндекс отнесёт сайт не к тому региону",
        "R04" => "На сайте только мобильный номер — Яндекс ждёт городской или 8-800",
        "R05" => "Нет страницы «Контакты»",
        "R06" => "Мало способов связаться с компанией",
        "R07" => "Не указаны способы оплаты",
        "R08" => "Не указаны гарантии и условия возврата",
        "R09" => "Мало цен на сайте — для Яндекса предложение выглядит бедным",
        "R10" => "Нет ссылок на соцсети компании",
        "R11" => "SEO-текст с переспамом — за такое Яндекс понижает сайт",
        "R12" => "Скрытый текст или ссылки — нарушение правил Яндекса",
        "R13" => "Навязчивая реклама или чужие перенаправления — нарушение правил Яндекса",
        "R14" => "Роботу Яндекса показывается другое содержимое, чем посетителю",
        "R15" => "Много страниц почти без текста — Яндекс считает их малополезными",
        _ => return None,
    })
}

/// Проблема простыми словами с учётом найденного; None — брать пояснение проверки.
fn plain(id: &str, r: &MetricResult) -> Option<String> {
    let value = r.value.as_ref().and_then(|v| v.as_str());
    match id {
        "B04" if r.detail.starts_with("Сертификат") || r.detail.starts_with("HTTPS не работает") => Some(r.detail.clone()),
        "B05" if value == Some("no_redirect") => Some("Нет перенаправления на https — по ссылке из карточки браузер пишет «Не защищено»".into()),
        "B05" if value == Some("no_redirect_hsts") => {
            Some("Сервер не перенаправляет на https: новый посетитель по ссылке из карточки увидит «Не защищено» (кто уже заходил на https — браузер переключит сам)".into())
        }
        "F13" => Some(format!("Сайт выглядит заброшенным: {}", r.detail)),
        _ => label(id).map(str::to_string),
    }
}

/// Эти метрики уже сказаны в строке состояния или не про сайт.
fn skip_in_problems(id: &str, state: SiteState) -> bool {
    id.starts_with('A')
        || id == "C21"
        || (id == "B01" && matches!(state, SiteState::Dead | SiteState::Stub | SiteState::Blocked))
        || (id == "F14" && state == SiteState::Stub)
}

pub fn build(results: &[MetricResult], cx: &Context) -> MiniReport {
    let by_id: HashMap<&str, &MetricResult> = results.iter().map(|r| (r.id.as_str(), r)).collect();
    let st = |id: &str| by_id.get(id).map(|r| r.status);
    let detail = |id: &str| by_id.get(id).map(|r| r.detail.clone()).unwrap_or_default();
    let value = |id: &str| by_id.get(id).and_then(|r| r.value.as_ref()).and_then(|v| v.as_str().map(str::to_string));
    let found = cx.discovery.and_then(|d| d.found.as_ref());
    let suggestion = cx.discovery.and_then(|d| d.suggestions.first());
    let fails = results.iter().filter(|r| r.status == Status::Fail && !r.id.starts_with('A')).count();
    let crit = |n: usize| match n {
        0 => String::new(),
        n => format!(", {n} {}", crate::text::plural_ru(n, "критичная проблема", "критичные проблемы", "критичных проблем")),
    };

    let b01_text = detail("B01");
    let stub = st("B01") == Some(Status::Fail) && (b01_text.contains("заглушк") || b01_text.contains("парковк"))
        || (st("F14") == Some(Status::Fail) && value("F14").as_deref() == Some("home"));
    let (state, headline) = if cx.closed {
        (SiteState::Closed, "Компания закрыта".to_string())
    } else if let Some(brand) = &cx.chain {
        (SiteState::Chain, format!("Точка сети «{brand}» — сайтом занимается головная компания"))
    } else if stub {
        let what = if st("B01") == Some(Status::Fail) { b01_text.clone() } else { "на главной шаблонный текст".to_string() };
        (SiteState::Stub, format!("Вместо сайта заглушка: {what}"))
    } else if st("B01") == Some(Status::Fail) {
        (SiteState::Dead, format!("Сайт не открывается: {b01_text}"))
    } else if value("H02").as_deref() == Some("foreign") {
        (SiteState::Dead, "Сайт, похоже, не этой компании: нет ни названия, ни контактов — в карточке чужой или старый домен".to_string())
    } else if value("B01").as_deref() == Some("blocked") {
        // «… (ответ 403, защита: ddos-guard) …» → название защиты в строку состояния.
        let shield = b01_text.split("защита: ").nth(1).and_then(|x| x.split(')').next()).map(str::trim).filter(|x| !x.is_empty());
        let text = if b01_text.contains("подтверждением возраста") {
            "Сайт за подтверждением возраста (18+) — проверить вручную".to_string()
        } else if b01_text.starts_with("Сайт не ответил на зарубежный") {
            "Сайт не ответил на зарубежный IP — проверить без VPN".to_string()
        } else if let Some(s) = shield {
            format!("Защита от ботов ({s}) не пустила сканер — проверить вручную или без VPN")
        } else {
            "Сайт не пустил сканер — проверить вручную или без VPN".to_string()
        };
        (SiteState::Blocked, text)
    } else if let Some(v) = found.filter(|_| st("A03") == Some(Status::Fail) || st("A04") == Some(Status::Fail)) {
        let score = cx.overall.map(|o| format!(", оценка {o:.0}/100")).unwrap_or_default();
        (SiteState::Hidden, format!("Сайт {} есть, но не указан в карточке{score}", v.domain))
    } else if st("A03") == Some(Status::Fail) {
        let maybe = suggestion.map(|s| format!(" (возможно, {} — проверить)", s.domain)).unwrap_or_default();
        (SiteState::None, format!("Сайта нет{maybe}"))
    } else if st("A04") == Some(Status::Fail) {
        (SiteState::Social, format!("Вместо сайта — {}", detail("A04")))
    } else {
        match cx.overall {
            Some(o) if o < 50.0 => (SiteState::Weak, format!("Слабый сайт: {o:.0}/100{}", crit(fails))),
            Some(o) if o < 75.0 => (SiteState::Fair, format!("Сайт средний: {o:.0}/100{}", crit(fails))),
            Some(o) => (SiteState::Good, format!("Сайт в порядке: {o:.0}/100{}", crit(fails))),
            None => (SiteState::Unknown, "Сайт не проверен".to_string()),
        }
    };

    // Главные проблемы: стоп-факторы, затем провалы по весу, затем предупреждения.
    let cat = catalog();
    let mut ranked: Vec<(&MetricResult, f64)> = results
        .iter()
        .filter(|r| matches!(r.status, Status::Fail | Status::Warn) && !skip_in_problems(&r.id, state))
        .filter_map(|r| {
            let d = cat.get(&r.id)?;
            let rank = if d.stop_factor { 1000.0 } else { 0.0 } + if r.status == Status::Fail { 100.0 } else { 0.0 } + d.weight;
            Some((r, rank))
        })
        .collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let mut problems: Vec<String> = Vec::new();
    for (r, _) in ranked {
        let text = plain(&r.id, r).unwrap_or_else(|| r.detail.clone());
        if !text.is_empty() && text.chars().count() <= 140 && !problems.contains(&text) {
            problems.push(text);
        }
        if problems.len() == 5 {
            break;
        }
    }
    if matches!(state, SiteState::Chain | SiteState::Closed) {
        problems.clear();
    }

    let failed = |ids: &[&str]| ids.iter().any(|id| st(id) == Some(Status::Fail));
    let mut offer: Vec<&str> = Vec::new();
    match state {
        SiteState::None | SiteState::Social | SiteState::Stub | SiteState::Dead => {
            offer.push("Сайт с услугами, ценами и онлайн-записью");
            offer.push("Указать сайт в карточках на картах");
        }
        SiteState::Hidden => {
            offer.push("Указать сайт в карточках на картах");
            if cx.overall.is_some_and(|o| o < 50.0) {
                offer.push("Переделать сайт");
            }
        }
        SiteState::Weak => offer.push("Новый сайт или глубокая переделка"),
        SiteState::Fair => offer.push("Доработка сайта и локальное SEO"),
        SiteState::Blocked => offer.push("Проверить сайт вручную"),
        _ => {}
    }
    if !matches!(state, SiteState::Chain | SiteState::Closed | SiteState::None | SiteState::Social | SiteState::Stub | SiteState::Dead) {
        if failed(&["B04", "B05"]) {
            offer.push("HTTPS и перенаправление");
        }
        if failed(&["I02", "I03", "I05", "I06", "I21"]) {
            offer.push("Открыть сайт для ИИ-поиска");
        }
        if failed(&["I08", "I18", "I19", "I20"]) {
            offer.push("Тексты и цены под ответы ИИ");
        }
        if failed(&["F01", "F06", "F07", "G01", "G02"]) {
            offer.push("SEO под «услуга + город»");
        }
        if failed(&["E01", "D05", "D01"]) {
            offer.push("Мобильная версия и скорость");
        }
        if failed(&["M01", "M02", "M03", "M05"]) {
            offer.push("Правовые документы на сайте");
        }
        if failed(&["P01"]) {
            offer.push("Аналитика и учёт заявок");
        }
    }
    offer.dedup();
    offer.truncate(4);

    MiniReport {
        state,
        headline,
        problems,
        offer: offer.into_iter().map(str::to_string).collect(),
        fail_ids: results.iter().filter(|r| r.status == Status::Fail).map(|r| r.id.clone()).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(id: &str, s: Status, detail: &str) -> MetricResult {
        MetricResult::new(id, s, detail)
    }

    #[test]
    fn states_and_problems() {
        let cx = |o: Option<f64>| Context { overall: o, closed: false, chain: None, discovery: None };
        let none = build(&[r("A03", Status::Fail, "")], &cx(None));
        assert_eq!(none.state, SiteState::None);
        assert_eq!(none.headline, "Сайта нет");
        assert!(none.offer[0].starts_with("Сайт с услугами"));

        let weak = build(
            &[r("A03", Status::Pass, ""), r("B01", Status::Pass, ""), r("E01", Status::Fail, "нет viewport"), r("B04", Status::Fail, "Сайт работает без HTTPS"), r("P01", Status::Fail, "")],
            &cx(Some(31.0)),
        );
        assert_eq!(weak.state, SiteState::Weak);
        assert!(weak.headline.contains("31/100") && weak.headline.contains("3 критичные проблемы"), "{}", weak.headline);
        assert_eq!(weak.problems[0], "Нет мобильной версии", "стоп-фактор первым");
        assert!(weak.problems.contains(&"Нет HTTPS — браузер пишет «Не защищено»".to_string()));
        assert!(weak.offer.contains(&"HTTPS и перенаправление".to_string()));
        assert_eq!(weak.fail_ids, vec!["E01", "B04", "P01"]);

        let stub = build(&[r("B01", Status::Fail, "Вместо сайта — заглушка «в разработке»")], &cx(Some(20.0)));
        assert_eq!(stub.state, SiteState::Stub);

        let chain = build(&[r("E01", Status::Fail, "")], &Context { overall: Some(40.0), closed: false, chain: Some("Евроопт".into()), discovery: None });
        assert_eq!(chain.state, SiteState::Chain);
        assert!(chain.problems.is_empty() && chain.offer.is_empty());
    }
}
