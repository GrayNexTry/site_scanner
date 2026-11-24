//! Разбор HTML-страницы в плоскую структуру для проверок (scraper/html5ever).

use std::collections::HashSet;
use std::sync::OnceLock;

use scraper::{ElementRef, Html, Node, Selector};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::text::{collapse_ws, same_site, truncate};

macro_rules! sel {
    ($s:expr) => {{
        static S: OnceLock<Selector> = OnceLock::new();
        S.get_or_init(|| Selector::parse($s).expect($s))
    }};
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Link {
    pub href: String,
    pub text: String,
    pub internal: bool,
    pub nofollow: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Image {
    pub src: Option<String>,
    pub alt: Option<String>,
    pub lazy: bool,
    /// Пиксели счётчиков и прочие 1×1.
    pub tiny: bool,
    /// Что браузер грузит на деле: `<source srcset>` внутри `<picture>` или `srcset` самой
    /// картинки. Запасной `src` при этом может быть битым — посетитель этого не увидит.
    #[serde(default)]
    pub shown: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScriptTag {
    pub src: Option<String>,
    pub in_head: bool,
    pub is_async: bool,
    pub is_defer: bool,
    pub is_module: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FormField {
    pub kind: String,
    pub name: String,
    pub has_label: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Form {
    pub action: Option<String>,
    pub fields: Vec<FormField>,
    /// Элементы <label> в форме — даже не связанные с полями через for/id.
    #[serde(default)]
    pub labels: usize,
    /// Some(true) — галочка согласия стоит заранее, Some(false) — есть и не стоит.
    pub consent_checkbox: Option<bool>,
    /// В форме есть текст о согласии на обработку персональных данных.
    pub consent_text: bool,
    /// Форма собирает персональные данные (имя, телефон, почта).
    pub personal: bool,
    /// Не заявка: поиск, корзина и оформление заказа, отзыв.
    #[serde(default)]
    pub not_lead: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Semantic {
    pub main: usize,
    pub header: usize,
    pub footer: usize,
    pub nav: usize,
    pub article: usize,
    pub section: usize,
    pub table: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PageInfo {
    pub url: String,
    pub lang: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    /// Содержимое meta robots/googlebot/yandex в нижнем регистре.
    pub meta_robots: Vec<String>,
    pub canonical: Option<String>,
    pub viewport: Option<String>,
    pub generator: Option<String>,
    pub verification: Vec<String>,
    pub og: Vec<(String, String)>,
    pub icon_link: bool,
    pub headings: Vec<(u8, String)>,
    pub links: Vec<Link>,
    pub images: Vec<Image>,
    pub scripts: Vec<ScriptTag>,
    pub stylesheets: Vec<(String, bool)>,
    pub iframes: Vec<String>,
    pub forms: Vec<Form>,
    pub jsonld: Vec<String>,
    pub microdata: bool,
    /// Типы микроданных (itemtype) в нижнем регистре: «organization», «restaurant»…
    #[serde(default)]
    pub microdata_types: Vec<String>,
    pub semantic: Semantic,
    /// Видимый текст без скриптов и стилей.
    pub text: String,
    pub words: usize,
    /// Текст скрытых блоков (всплывающие и модальные окна, hidden, display:none): шаблонный
    /// текст в них посетитель не видит, это не заглушка.
    #[serde(default)]
    pub hidden_text: String,
    pub tel_links: Vec<String>,
    pub mailtos: Vec<String>,
    pub header_tel: bool,
    pub hreflang: Vec<(String, String)>,
    pub breadcrumbs: bool,
    pub mixed_active: Vec<String>,
    pub mixed_passive: Vec<String>,
    pub question_headings: usize,
    pub details_blocks: usize,
    pub article_meta: bool,
    pub data_nosnippet: bool,
    /// Тексты кнопок и ссылок-кнопок (поиск CTA).
    pub buttons: Vec<String>,
    /// Текст, спрятанный приёмами скрытого текста: font-size:0, text-indent или позиция далеко
    /// за экраном (инлайн-стиль или правило в `<style>` по классу/id) — R12. Свёрнутые блоки
    /// (display:none у вкладок, спойлеров, попапов) сюда не входят: их посетитель может открыть.
    #[serde(default)]
    pub trick_hidden_text: String,
    /// Ссылок внутри такого текста.
    #[serde(default)]
    pub trick_hidden_links: usize,
    /// Адрес перехода из `<meta http-equiv="refresh">` с задержкой до 10 с (R13).
    #[serde(default)]
    pub meta_refresh: Option<String>,
    /// Прозрачные фреймы соцсетей (opacity:0) — приём кликджекинга (R13).
    #[serde(default)]
    pub invisible_iframes: Vec<String>,
    /// Весь HTML в нижнем регистре: поиск маркеров счётчиков и виджетов.
    #[serde(skip)]
    pub html_lower: String,
}

impl PageInfo {
    pub fn h1s(&self) -> Vec<&str> {
        self.headings.iter().filter(|(l, _)| *l == 1).map(|(_, t)| t.as_str()).collect()
    }
    pub fn text_lower(&self) -> String {
        self.text.to_lowercase().replace('ё', "е")
    }
    pub fn has_noindex(&self) -> bool {
        // Директивы целиком: «max-image-preview:none» — не «none».
        self.meta_robots.iter().any(|c| c.split(',').map(str::trim).any(|d| d == "noindex" || d == "none"))
    }
    pub fn og(&self, prop: &str) -> Option<&str> {
        self.og.iter().find(|(k, _)| k == prop).map(|(_, v)| v.as_str())
    }
}

const SKIP_TEXT: &[&str] = &["script", "style", "noscript", "template", "svg", "head", "iframe", "object"];
const BLOCK: &[&str] = &[
    "p", "div", "li", "br", "h1", "h2", "h3", "h4", "h5", "h6", "tr", "td", "th", "section", "article",
    "header", "footer", "nav", "ul", "ol", "table", "form", "label", "button", "a", "span", "dd", "dt",
];

fn collect_text(el: ElementRef, out: &mut String) {
    for child in el.children() {
        match child.value() {
            Node::Text(t) => out.push_str(t),
            Node::Element(e) => {
                let name = e.name();
                if SKIP_TEXT.contains(&name) {
                    continue;
                }
                if let Some(ch) = ElementRef::wrap(child) {
                    let block = BLOCK.contains(&name);
                    if block {
                        out.push(' ');
                    }
                    collect_text(ch, out);
                    if block {
                        out.push(' ');
                    }
                }
            }
            _ => {}
        }
    }
}

fn el_text(el: ElementRef) -> String {
    let mut s = String::new();
    collect_text(el, &mut s);
    collapse_ws(&s)
}

fn attr<'a>(el: &ElementRef<'a>, name: &str) -> Option<&'a str> {
    el.value().attr(name).map(str::trim).filter(|s| !s.is_empty())
}

/// Поле или его ближайшие предки скрыты (класс hidden/honeypot, display:none, hidden, aria-hidden).
fn is_hidden_field(el: &ElementRef) -> bool {
    std::iter::once(*el).chain(el.ancestors().filter_map(ElementRef::wrap).take(2)).any(|e| {
        let v = e.value();
        let class = v.attr("class").unwrap_or("").to_ascii_lowercase();
        let style = v.attr("style").unwrap_or("").to_ascii_lowercase().replace(' ', "");
        v.attr("hidden").is_some()
            || v.attr("aria-hidden") == Some("true")
            || style.contains("display:none")
            || style.contains("visibility:hidden")
            || ["hidden", "honeypot", "hp-field", "invisible", "d-none"].iter().any(|c| class.contains(c))
    })
}

fn has_label_ancestor(el: &ElementRef) -> bool {
    el.ancestors().filter_map(ElementRef::wrap).any(|a| a.value().name() == "label")
}

fn ancestor_text(el: &ElementRef, depth: usize) -> String {
    el.ancestors()
        .filter_map(ElementRef::wrap)
        .take(depth)
        .last()
        .map(el_text)
        .unwrap_or_default()
        .to_lowercase()
}

pub fn extract(html: &str, page_url: &str) -> PageInfo {
    let doc = Html::parse_document(html);
    let mut base = Url::parse(page_url).unwrap_or_else(|_| Url::parse("https://invalid.local/").unwrap());
    if let Some(b) = doc.select(sel!("base[href]")).next().and_then(|e| attr(&e, "href"))
        && let Ok(u) = base.join(b)
    {
        base = u;
    }
    let page = Url::parse(page_url).ok();
    let resolve = |h: &str| -> Option<Url> {
        base.join(h).ok().filter(|u| matches!(u.scheme(), "http" | "https"))
    };

    let mut p = PageInfo { url: page_url.to_string(), html_lower: html.to_lowercase(), ..Default::default() };

    p.lang = doc.select(sel!("html")).next().and_then(|e| attr(&e, "lang")).map(str::to_string);
    p.title = doc.select(sel!("title")).next().map(el_text).filter(|t| !t.is_empty());

    for m in doc.select(sel!("meta")) {
        let name = attr(&m, "name").map(str::to_ascii_lowercase);
        // Open Graph иногда пишут через name= вместо property= — соцсети это читают.
        let prop = attr(&m, "property").map(str::to_ascii_lowercase).or_else(|| name.clone().filter(|n| n.starts_with("og:")));
        let content = attr(&m, "content").unwrap_or("");
        match name.as_deref() {
            Some("description") if p.description.is_none() => {
                p.description = Some(collapse_ws(content)).filter(|s| !s.is_empty())
            }
            Some("robots" | "googlebot" | "yandex" | "yandexbot") => p.meta_robots.push(content.to_ascii_lowercase()),
            Some("viewport") => p.viewport = Some(content.to_string()),
            Some("generator") => p.generator = Some(content.to_string()),
            Some(n @ ("google-site-verification" | "yandex-verification" | "msvalidate.01" | "wmail-verification")) => {
                if !p.verification.iter().any(|v| v == n) {
                    p.verification.push(n.to_string());
                }
            }
            Some("author") => p.article_meta = true,
            _ => {}
        }
        // <meta http-equiv="refresh" content="0; url=…"> — перенаправление без участия посетителя.
        if attr(&m, "http-equiv").is_some_and(|h| h.eq_ignore_ascii_case("refresh")) && p.meta_refresh.is_none() {
            p.meta_refresh = meta_refresh_target(content, &base);
        }
        if let Some(pr) = prop {
            if pr.starts_with("og:") {
                p.og.push((pr.clone(), content.to_string()));
            }
            if pr == "article:published_time" || pr == "article:modified_time" {
                p.article_meta = true;
            }
        }
    }

    p.canonical = doc
        .select(sel!("link[rel]"))
        .find(|e| attr(e, "rel").is_some_and(|r| r.eq_ignore_ascii_case("canonical")))
        .and_then(|e| attr(&e, "href"))
        .and_then(|h| base.join(h).ok())
        .map(|u| u.to_string());
    p.icon_link = doc.select(sel!("link[rel]")).any(|e| {
        attr(&e, "rel").is_some_and(|r| r.to_ascii_lowercase().split_whitespace().any(|t| t == "icon" || t == "apple-touch-icon"))
    });
    for e in doc.select(sel!("link[rel='alternate'][hreflang]")) {
        if let (Some(l), Some(h)) = (attr(&e, "hreflang"), attr(&e, "href")) {
            p.hreflang.push((l.to_ascii_lowercase(), h.to_string()));
        }
    }
    for e in doc.select(sel!("link[rel~='stylesheet'][href]")) {
        if let Some(u) = attr(&e, "href").and_then(resolve) {
            let in_head = e.ancestors().filter_map(ElementRef::wrap).any(|a| a.value().name() == "head");
            p.stylesheets.push((u.to_string(), in_head));
        }
    }

    for h in doc.select(sel!("h1, h2, h3, h4, h5, h6")) {
        let level = h.value().name()[1..].parse::<u8>().unwrap_or(6);
        p.headings.push((level, truncate(&el_text(h), 200)));
    }
    // Вопросы FAQ: заголовки, термины, кнопки аккордеона — и жирные строки «<p><strong>…?</strong>» (mamboway.by).
    p.question_headings = doc
        .select(sel!("h2, h3, h4, dt, summary, button, p > strong, p > b, li > strong"))
        .filter(|e| {
            let t = el_text(*e);
            let t = t.trim_end();
            t.ends_with('?') && (8..=200).contains(&t.chars().count())
        })
        .count();
    p.details_blocks = doc.select(sel!("details")).count();

    // Ссылки, телефоны, почта.
    let tel_header: HashSet<String> = doc
        .select(sel!("header a[href^='tel:'], [class*='header'] a[href^='tel:'], [id*='header'] a[href^='tel:']"))
        .filter_map(|e| attr(&e, "href").map(str::to_string))
        .collect();
    p.header_tel = !tel_header.is_empty();
    // Заготовки ссылок в <template> («/{{li_buttonlink@url}}» у карточек Tilda) браузер не показывает.
    let in_template = |e: &ElementRef| e.ancestors().filter_map(ElementRef::wrap).any(|a| a.value().name() == "template");
    for (i, a) in doc.select(sel!("a[href]")).filter(|a| !in_template(a) && !attr(a, "href").unwrap_or("").contains("{{")).enumerate() {
        let href = attr(&a, "href").unwrap_or("");
        let lower = href.to_ascii_lowercase();
        if lower.starts_with("tel:") {
            p.tel_links.push(href[4..].to_string());
            if i < 25 {
                p.header_tel = true;
            }
            continue;
        }
        if lower.starts_with("mailto:") {
            p.mailtos.push(href[7..].split('?').next().unwrap_or("").to_string());
            continue;
        }
        if lower.starts_with('#') || lower.starts_with("javascript:") {
            continue;
        }
        let Some(u) = resolve(href) else { continue };
        let internal = page.as_ref().is_some_and(|pu| same_site(pu, &u));
        let nofollow = attr(&a, "rel").is_some_and(|r| r.to_ascii_lowercase().contains("nofollow"));
        p.links.push(Link { href: u.to_string(), text: truncate(&el_text(a), 120), internal, nofollow });
    }

    for img in doc.select(sel!("img")) {
        let src = attr(&img, "src")
            .filter(|s| !s.starts_with("data:"))
            .or_else(|| attr(&img, "data-src"))
            .or_else(|| attr(&img, "data-original"))
            .or_else(|| attr(&img, "src"));
        let lazy = attr(&img, "loading").is_some_and(|l| l.eq_ignore_ascii_case("lazy"))
            || (attr(&img, "src").is_none() && attr(&img, "data-src").is_some());
        let dim = |n: &str| attr(&img, n).and_then(|v| v.trim_end_matches("px").parse::<u32>().ok());
        // Иконки (мессенджеры, телефон, платёжные системы) — оформление, alt им не обязателен.
        let tiny = matches!((dim("width"), dim("height")), (Some(w), Some(h)) if w <= 48 && h <= 48)
            || src.is_some_and(|s| {
                let l = s.to_lowercase();
                // «Made on Tilda» (tildacopy.png) — значок конструктора, не картинка сайта.
                // svg в src — заглушка, если настоящая картинка в srcset (dili.by: nophoto.svg + jpg).
                let raster_in_srcset = attr(&img, "srcset").or_else(|| attr(&img, "data-srcset")).is_some_and(|x| !x.to_lowercase().contains(".svg"));
                l.contains("mc.yandex") || l.contains("counter") || l.contains("pixel") || l.contains("icon") || (l.ends_with(".svg") && !raster_in_srcset) || l.contains("tildacopy")
            });
        // Адрес в srcset может содержать запятые (Cloudinary «f_auto,q_auto»): кандидат — до пробела.
        // Если после адреса не дескриптор («640w», «2x»), а кусок имени файла с пробелом — браузер
        // такой кандидат отбрасывает и берёт src/data-src (thierry.by: «-2340 Хлеб_500x333.webP»).
        let first_url = |set: &str| {
            let mut tokens = set.split_whitespace();
            let url = tokens.next()?;
            if url.ends_with(',') {
                return Some(url.trim_end_matches(',').to_string());
            }
            let descriptor = tokens.next().map(|d| d.trim_end_matches(','));
            let valid = descriptor.is_none_or(|d| {
                let num = &d[..d.len().saturating_sub(1)];
                matches!(d.chars().last(), Some('w' | 'x' | 'h')) && !num.is_empty() && num.parse::<f64>().is_ok()
            });
            valid.then(|| url.to_string())
        };
        let picture_source = img
            .parent()
            .and_then(ElementRef::wrap)
            .filter(|p| p.value().name() == "picture")
            .and_then(|p| p.children().filter_map(ElementRef::wrap).find(|c| c.value().name() == "source"))
            .and_then(|s| attr(&s, "srcset").or_else(|| attr(&s, "data-srcset")))
            .and_then(first_url);
        let shown = picture_source
            .or_else(|| attr(&img, "srcset").and_then(first_url))
            .filter(|s| !s.starts_with("data:"))
            .and_then(|s| resolve(&s).map(|u| u.to_string()));
        p.images.push(Image {
            src: src.and_then(|s| if s.starts_with("data:") { Some(s.to_string()) } else { resolve(s).map(|u| u.to_string()) }),
            alt: img.value().attr("alt").map(|a| a.trim().to_string()),
            lazy,
            tiny,
            shown,
        });
    }

    for (sel_head, in_head) in [(sel!("head script"), true), (sel!("body script"), false)] {
        for s in doc.select(sel_head) {
            let ty = attr(&s, "type").unwrap_or("").to_ascii_lowercase();
            if ty == "application/ld+json" {
                p.jsonld.push(s.text().collect::<String>());
                continue;
            }
            p.scripts.push(ScriptTag {
                src: attr(&s, "src").and_then(resolve).map(|u| u.to_string()),
                in_head,
                is_async: s.value().attr("async").is_some(),
                is_defer: s.value().attr("defer").is_some(),
                is_module: ty == "module",
            });
        }
    }
    p.microdata = doc.select(sel!("[itemscope]")).next().is_some();
    p.microdata_types = doc
        .select(sel!("[itemtype]"))
        .filter_map(|e| attr(&e, "itemtype"))
        .flat_map(|t| t.split_whitespace())
        .filter_map(|t| t.trim_end_matches('/').rsplit('/').next().map(|x| x.to_lowercase()))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();

    for f in doc.select(sel!("iframe")) {
        if let Some(s) = attr(&f, "src").or_else(|| attr(&f, "data-src")) {
            p.iframes.push(resolve(s).map(|u| u.to_string()).unwrap_or_else(|| s.to_string()));
        }
    }

    // Формы.
    let label_for: HashSet<String> =
        doc.select(sel!("label[for]")).filter_map(|l| attr(&l, "for").map(str::to_string)).collect();
    for form in doc.select(sel!("form")) {
        let mut f = Form {
            action: attr(&form, "action").and_then(|a| base.join(a).ok()).map(|u| u.to_string()),
            ..Default::default()
        };
        // «Висящие» подписи — без for и без поля внутри (подписи радиокнопок и чекбоксов не в счёт).
        f.labels = form.select(sel!("label")).filter(|l| attr(l, "for").is_none() && l.select(sel!("input, select, textarea")).next().is_none()).count();
        let marks = format!(
            "{} {} {}",
            attr(&form, "class").unwrap_or(""),
            attr(&form, "id").unwrap_or(""),
            attr(&form, "action").unwrap_or("")
        )
        .to_lowercase();
        let head = el_text(form).to_lowercase();
        let parents = form
            .ancestors()
            .filter_map(ElementRef::wrap)
            .take(4)
            .map(|a| format!("{} {}", a.value().attr("class").unwrap_or(""), a.value().attr("id").unwrap_or("")))
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        // Заголовок формы отзыва часто стоит рядом, вне <form>: «Ваше мнение важно для нас».
        let around = ancestor_text(&form, 2);
        let around = if around.chars().count() < 600 { around } else { String::new() };
        const REVIEW: &[&str] = &[
            "оставить отзыв", "ваше мнение", "написать отзыв", "leave a review", "write a review", "leave feedback", "rate us",
            "your opinion", "laisser un avis", "donnez votre avis", "votre avis", "deja tu opinión", "deja tu reseña",
            "escribe una reseña", "tu opinión", "bewertung schreiben", "bewertung abgeben", "ihre meinung", "lascia una recensione",
            "la tua opinione", "napisz opinię", "dodaj opinię", "twoja opinia", "deixe a sua avaliação", "deixe sua avaliação",
            "schrijf een review", "uw mening",
        ];
        const NOT_LEAD_MARKS: &[&str] = &[
            "search", "poisk", "cart", "basket", "checkout", "order", "t706", "korzin", "panier", "recherche", "commande", "carrito",
            "buscar", "busca", "pedido", "suche", "kasse", "bestell", "carrello", "cerca", "ordine", "koszyk", "szukaj", "zamow",
            "winkelwagen", "zoek", "carrinho", "pesquis",
        ];
        f.not_lead = NOT_LEAD_MARKS.iter().any(|m| marks.contains(m))
            || [
                "t706", "cartpage", "orderform", "checkout", "review", "otzyv", "otziv", "testimonial", "avis", "resena", "reseña",
                "bewertung", "recensione", "opinia", "opinie", "avaliac",
            ]
            .iter()
            .any(|m| parents.contains(m))
            || REVIEW.iter().any(|m| head.contains(m) || around.contains(m));
        let form_text = el_text(form).to_lowercase();
        f.consent_text = (form_text.contains("соглас") && (form_text.contains("персональн") || form_text.contains("обработк")))
            || form_text.contains("политик")
            || crate::lexicon::PRIVACY.iter().any(|w| form_text.contains(w))
            || ["consent", "einwillig", "j'accepte", "acepto", "accetto"].iter().any(|w| form_text.contains(w));
        for input in form.select(sel!("input, textarea, select")) {
            let tag = input.value().name();
            let ty = if tag == "input" { attr(&input, "type").unwrap_or("text").to_ascii_lowercase() } else { tag.to_string() };
            if matches!(ty.as_str(), "hidden" | "submit" | "button" | "image" | "reset") {
                continue;
            }
            if ty == "checkbox" {
                let around = ancestor_text(&input, 2);
                if crate::lexicon::CONSENT.iter().any(|w| around.contains(w)) {
                    f.consent_checkbox = Some(input.value().attr("checked").is_some());
                }
                continue;
            }
            if ty == "radio" {
                continue;
            }
            // Поля-ловушки, скрытые служебные поля и условные группы CF7 посетитель не заполняет все разом.
            // Маркер условной группы CF7 до запуска JS лежит в data-class, а class ещё пустой.
            if is_hidden_field(&input)
                || input.ancestors().filter_map(ElementRef::wrap).take(4).any(|a| {
                    ["class", "data-class"].iter().any(|k| a.value().attr(k).is_some_and(|c| c.contains("wpcf7cf_group")))
                })
            {
                continue;
            }
            let name = attr(&input, "name").unwrap_or("").to_ascii_lowercase();
            let has_label = attr(&input, "id").is_some_and(|id| label_for.contains(id))
                || has_label_ancestor(&input)
                || attr(&input, "aria-label").is_some()
                || attr(&input, "aria-labelledby").is_some()
                || attr(&input, "title").is_some();
            if matches!(ty.as_str(), "tel" | "email")
                || ["name", "phone", "tel", "email", "mail", "имя", "телефон", "почт", "fio", "nombre", "nom", "vorname", "nachname", "telefon", "cognome", "imie", "nazwisko"].iter().any(|k| name.contains(k))
            {
                f.personal = true;
            }
            f.fields.push(FormField { kind: ty, name, has_label });
        }
        p.forms.push(f);
    }

    p.semantic = Semantic {
        main: doc.select(sel!("main")).count(),
        header: doc.select(sel!("header")).count(),
        footer: doc.select(sel!("footer")).count(),
        nav: doc.select(sel!("nav")).count(),
        article: doc.select(sel!("article")).count(),
        section: doc.select(sel!("section")).count(),
        table: doc.select(sel!("table")).count(),
    };
    p.breadcrumbs = doc
        .select(sel!("[class*='breadcrumb'], [id*='breadcrumb'], nav[aria-label*='readcrumb'], [itemtype*='BreadcrumbList']"))
        .next()
        .is_some();
    p.data_nosnippet = doc.select(sel!("[data-nosnippet]")).next().is_some();
    if doc.select(sel!("time[datetime], [itemprop='author'], [rel='author']")).next().is_some() {
        p.article_meta = true;
    }

    // Смешанный контент на https-странице.
    if page.as_ref().is_some_and(|u| u.scheme() == "https") {
        for e in doc.select(sel!("script[src^='http:'], iframe[src^='http:'], link[rel~='stylesheet'][href^='http:']")) {
            if let Some(s) = attr(&e, "src").or_else(|| attr(&e, "href")) {
                p.mixed_active.push(s.to_string());
            }
        }
        for e in doc.select(sel!("img[src^='http:'], video[src^='http:'], audio[src^='http:'], source[src^='http:']")) {
            if let Some(s) = attr(&e, "src") {
                p.mixed_passive.push(s.to_string());
            }
        }
    }

    for b in doc.select(sel!("button, input[type='submit'], a[class*='btn'], a[class*='button'], [role='button']")) {
        let t = if b.value().name() == "input" { attr(&b, "value").unwrap_or("").to_string() } else { el_text(b) };
        if !t.is_empty() && t.chars().count() <= 60 {
            p.buttons.push(t);
        }
    }

    if let Some(body) = doc.select(sel!("body")).next() {
        let mut s = String::new();
        collect_text(body, &mut s);
        p.text = truncate(&collapse_ws(&s), 200_000);
    }
    p.words = p.text.split_whitespace().filter(|w| w.chars().any(char::is_alphanumeric)).count();
    let mut hidden = String::new();
    for el in doc.select(sel!(
        "[hidden], [aria-hidden=true], [style*='display:none'], [style*='display: none'], [style*='visibility:hidden'],          [class*='popup'], [class*='modal'], [class*='mfp-hide'], [class~='hidden'], [id*='cookie' i], [class*='cookie' i], \
         [id*='consent' i], [class*='consent' i], [id*='Cookiebot']"
    )) {
        collect_text(el, &mut hidden);
        hidden.push(' ');
        if hidden.len() > 100_000 {
            break;
        }
    }
    p.hidden_text = collapse_ws(&hidden);
    trick_hidden(&doc, &mut p);
    for f in doc.select(sel!("iframe[src]")) {
        let src = attr(&f, "src").unwrap_or("").to_ascii_lowercase();
        let social = ["vk.com/", "facebook.com/", "ok.ru/", "instagram.com/", "twitter.com/", "x.com/"].iter().any(|d| src.contains(d));
        let transparent = std::iter::once(f).chain(f.ancestors().filter_map(ElementRef::wrap).take(2)).any(|e| transparent_style(e.value().attr("style").unwrap_or("")));
        if social && transparent {
            p.invisible_iframes.push(src);
        }
    }
    p
}

/// Цель `<meta http-equiv="refresh">`, если переход случится за 10 с или быстрее.
fn meta_refresh_target(content: &str, base: &Url) -> Option<String> {
    let (delay, rest) = content.split_once([';', ',']).unwrap_or((content, ""));
    let delay: f64 = delay.trim().parse().ok()?;
    let rest = rest.trim();
    let target = rest.get(..4).filter(|h| h.eq_ignore_ascii_case("url=")).map(|_| &rest[4..]).unwrap_or(rest);
    let target = target.trim().trim_matches(['\'', '"']);
    if delay > 10.0 || target.is_empty() {
        return None;
    }
    base.join(target).ok().map(|u| u.to_string())
}

/// Стиль без пробелов в нижнем регистре.
fn squash(style: &str) -> String {
    style.to_ascii_lowercase().chars().filter(|c| !c.is_whitespace()).collect()
}

/// Прозрачный слой: opacity:0 (кликджекинг — прозрачный фрейм поверх кнопки).
fn transparent_style(style: &str) -> bool {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"(?:^|[;{])opacity:0?\.?0*(?:[;}!]|$)|alpha\(opacity=0\)").unwrap()).is_match(&squash(style))
}

/// Приём скрытого текста: шрифт 0–1 px, text-indent или позиция дальше 900 px за экран.
fn trick_style(style: &str) -> bool {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let s = squash(style);
    let re = RE.get_or_init(|| {
        regex::Regex::new(
            r"(?:^|[;{])font-size:(?:0(?:\.0+)?|1px)(?:px|em|rem|pt|%)?(?:[;}!]|$)|text-indent:-(?:9\d\d|\d{4,})|(?:^|[;{])(?:left|top|margin-left):-(?:9\d\d|\d{4,})(?:px)?(?:[;}!]|$)",
        )
        .unwrap()
    });
    re.is_match(&s)
}

/// Скрытый текст (R12): элементы с приёмом в инлайн-стиле или в правиле `<style>` по классу/id.
/// Текст для экранных дикторов (sr-only, visually-hidden) и ссылки «к содержимому» — не спам.
fn trick_hidden(doc: &Html, p: &mut PageInfo) {
    let mut selectors: Vec<String> = Vec::new();
    static RULE: OnceLock<regex::Regex> = OnceLock::new();
    let rule = RULE.get_or_init(|| regex::Regex::new(r"([^{}]+)\{([^{}]*)\}").unwrap());
    for st in doc.select(sel!("style")) {
        let css: String = st.text().collect();
        for c in rule.captures_iter(&css) {
            if !trick_style(&c[2]) {
                continue;
            }
            for one in c[1].split(',').map(str::trim) {
                // Только простые селекторы: «.seo-text», «#links», «div.hide».
                let simple = one.len() < 80 && one.chars().all(|ch| ch.is_alphanumeric() || "-_.#".contains(ch)) && one.contains(['.', '#']);
                if simple && !selectors.iter().any(|x| x == one) {
                    selectors.push(one.to_string());
                }
            }
        }
    }
    let a11y = |e: &ElementRef| {
        std::iter::once(*e).chain(e.ancestors().filter_map(ElementRef::wrap).take(3)).any(|x| {
            let v = x.value();
            let marks = format!("{} {}", v.attr("class").unwrap_or(""), v.attr("id").unwrap_or("")).to_ascii_lowercase();
            ["sr-only", "visually-hidden", "visuallyhidden", "screen-reader", "screenreader", "skip", "a11y", "assistive", "honeypot"]
                .iter()
                .any(|m| marks.contains(m))
        })
    };
    let mut seen = HashSet::new();
    let mut take = |e: ElementRef, p: &mut PageInfo| {
        if a11y(&e) || e.ancestors().any(|a| seen.contains(&a.id())) || !seen.insert(e.id()) {
            return;
        }
        // Логотип с текстом за экраном, иконки соцсетей с подписью — короткие; это не спам.
        let t = el_text(e);
        if t.split_whitespace().count() < 10 {
            return;
        }
        if p.trick_hidden_text.len() < 20_000 {
            p.trick_hidden_text.push_str(&t);
            p.trick_hidden_text.push(' ');
        }
        p.trick_hidden_links += e.select(sel!("a[href]")).count();
    };
    for e in doc.select(sel!("[style]")) {
        if trick_style(e.value().attr("style").unwrap_or("")) {
            take(e, p);
        }
    }
    for s in &selectors {
        if let Ok(selector) = Selector::parse(s) {
            for e in doc.select(&selector) {
                take(e, p);
            }
        }
    }
    p.trick_hidden_text = collapse_ws(&p.trick_hidden_text);
}

#[cfg(test)]
mod tests {
    use super::*;

    const HTML: &str = r#"<!doctype html><html lang="ru"><head>
<title>Стоматология в Хамовниках — Зуб-Мастер</title>
<meta name="description" content="Лечение зубов без боли">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="yandex-verification" content="abc">
<meta property="og:title" content="Зуб-Мастер">
<link rel="canonical" href="/">
<link rel="icon" href="/favicon.svg">
<script src="http://cdn.example.com/a.js"></script>
<script type="application/ld+json">{"@context":"https://schema.org","@type":"Dentist","name":"Зуб-Мастер"}</script>
</head><body>
<header><a href="tel:+74951234567">+7 495 123-45-67</a></header>
<h1>Стоматология</h1><h3>Сколько стоит лечение?</h3>
<p>Лечение кариеса от 3 500 ₽. Адрес: ул. Льва Толстого, д. 5.</p>
<a href="/uslugi/">Услуги</a><a href="https://vk.com/zub">ВК</a>
<img src="/a.jpg" alt="Кабинет"><img src="/b.jpg">
<form action="/send"><input name="name"><input type="tel" name="phone" id="ph"><label for="ph">Телефон</label>
<label><input type="checkbox" checked> Я даю согласие на обработку персональных данных</label>
<button>Записаться</button></form>
<script>ym(1,'init',{webvisor:true})</script>
</body></html>"#;

    #[test]
    fn extracts_core_fields() {
        let p = extract(HTML, "https://zub.ru/");
        assert_eq!(p.lang.as_deref(), Some("ru"));
        assert!(p.title.as_deref().unwrap().starts_with("Стоматология"));
        assert_eq!(p.canonical.as_deref(), Some("https://zub.ru/"));
        assert_eq!(p.h1s(), vec!["Стоматология"]);
        assert_eq!(p.question_headings, 1);
        assert!(p.header_tel);
        assert_eq!(p.verification, vec!["yandex-verification"]);
        assert_eq!(p.jsonld.len(), 1);
        assert_eq!(p.links.iter().filter(|l| l.internal).count(), 1);
        assert_eq!(p.images.len(), 2);
        assert_eq!(p.mixed_active, vec!["http://cdn.example.com/a.js"]);
        assert!(p.icon_link);
        assert!(p.text.contains("3 500 ₽"));
        assert!(!p.text.contains("ym(1"));
    }

    #[test]
    fn picture_source_is_what_browser_loads() {
        let p = extract(
            r#"<html><body><picture><source srcset="/img/a.webp 1x, /img/a@2x.webp 2x" type="image/webp"><img src="/img/a.jpg" alt="a"></picture>
<img src="/b.jpg" srcset="/b-640.jpg 640w, /b-1280.jpg 1280w" alt="b"><img src="/c.jpg" alt="c"></body></html>"#,
            "https://x.by/",
        );
        let shown: Vec<_> = p.images.iter().map(|i| i.shown.as_deref()).collect();
        assert_eq!(shown, vec![Some("https://x.by/img/a.webp"), Some("https://x.by/b-640.jpg"), None]);
    }

    #[test]
    fn srcset_with_spaces_in_file_name_falls_back() {
        // Круг 5: thierry.by — «…/-2340 Хлеб_500x333_996.webP»: браузер отбрасывает кандидат и берёт src.
        let p = extract(r#"<html><body><img src="/a.jpg" srcset="/catalogs/-2340 Хлеб_500x333_996.webP"></body></html>"#, "https://x.by/");
        assert_eq!(p.images[0].shown, None);
    }

    #[test]
    fn radio_labels_are_not_loose_labels() {
        // Круг 5: monplatincenter.by — подписи вариантов доставки (радиокнопки) не «висящие» подписи.
        let p = extract(
            r#"<html><body><form><label><input type="radio" name="d"> Курьер</label><label for="pick">Самовывоз</label><input type="radio" id="pick" name="d">
            <input name="city" placeholder="Город"><input name="zip" placeholder="Индекс"></form></body></html>"#,
            "https://x.by/",
        );
        assert_eq!(p.forms[0].labels, 0);
    }

    #[test]
    fn template_links_are_not_links() {
        // Круг 5: бисквитофф.бел — «/{{li_buttonlink@url}}» в <template> карточки Tilda — не битая ссылка.
        let p = extract(r#"<html><body><a href="/about">О нас</a><template class="js-catalog-template-card"><a href="/{{li_buttonlink@url}}">Купить</a></template></body></html>"#, "https://x.by/");
        assert_eq!(p.links.len(), 1);
    }

    #[test]
    fn svg_placeholder_with_raster_srcset_and_bold_questions() {
        // Круг 6: dili.by — nophoto.svg в src, фото в srcset; mamboway.by — вопросы «<p><strong>…?</strong>».
        let p = extract(
            r#"<html><body><img src="/img/nophoto.svg" srcset="/upload/paris.jpg 1x" alt="Новый год в Париже">
            <div id="faq"><p><strong>Можно ли оплатить в рассрочку?</strong></p><p>Да.</p><p><strong>Нужна ли виза?</strong></p></div></body></html>"#,
            "https://x.by/",
        );
        assert!(!p.images[0].tiny);
        assert_eq!(p.question_headings, 2);
    }

    #[test]
    fn srcset_url_with_commas() {
        // Круг 4: armani.com — адрес Cloudinary с запятыми внутри разрезался по запятой и давал 404.
        let p = extract(
            r#"<html><body><img src="/x.jpg" srcset="https://res.cloudinary.com/a/image/upload/f_auto,q_auto,w_256/p.jpg 256w, https://res.cloudinary.com/a/image/upload/f_auto,q_auto,w_512/p.jpg 512w"></body></html>"#,
            "https://x.by/",
        );
        assert_eq!(p.images[0].shown.as_deref(), Some("https://res.cloudinary.com/a/image/upload/f_auto,q_auto,w_256/p.jpg"));
    }

    #[test]
    fn extracts_forms_and_consent() {
        let p = extract(HTML, "https://zub.ru/");
        let f = &p.forms[0];
        assert!(f.personal);
        assert_eq!(f.consent_checkbox, Some(true));
        assert!(f.consent_text);
        assert_eq!(f.fields.len(), 2);
        assert!(f.fields.iter().any(|x| x.kind == "tel" && x.has_label));
        assert!(f.fields.iter().any(|x| x.name == "name" && !x.has_label));
        assert!(p.buttons.contains(&"Записаться".to_string()));
    }
}

#[cfg(test)]
mod noindex_tests {
    #[test]
    fn directives_are_whole_words() {
        let p = super::extract("<html><head><meta name=\"robots\" content=\"index, nofollow, max-image-preview:none\"></head><body>x</body></html>", "https://a.by/");
        assert!(!p.has_noindex(), "max-image-preview:none — не noindex");
        let q = super::extract("<html><head><meta name=\"robots\" content=\"noindex, follow\"></head><body>x</body></html>", "https://a.by/");
        assert!(q.has_noindex());
    }

    #[test]
    fn hidden_text_tricks_refresh_and_clickjacking() {
        let seo = "стоматология минск недорого лечение зубов минск ".repeat(4);
        let html = format!(
            r#"<html><head><meta http-equiv="refresh" content="0; url=https://other.example/">
<style>.seo-block{{position:absolute;left:-9999px}} .logo{{text-indent:-9999px}}</style></head><body>
<h1 class="logo">Зуб-Мастер</h1>
<span class="sr-only">Перейти к основному содержимому страницы сайта стоматологии для экранных дикторов</span>
<div class="seo-block">{seo}</div>
<div style="font-size:0">{seo}<a href="https://a.ru/">a</a></div>
<div style="display:none">Свёрнутый блок с длинным текстом, который посетитель может открыть кнопкой «Подробнее» в любой момент</div>
<div style="opacity:0"><iframe src="https://vk.com/widget_like.php?app=1"></iframe></div>
</body></html>"#
        );
        let p = super::extract(&html, "https://zub.ru/");
        assert_eq!(p.meta_refresh.as_deref(), Some("https://other.example/"));
        let words = p.trick_hidden_text.split_whitespace().count();
        assert_eq!(words, 49, "два блока по 24 слова и текст скрытой ссылки: {}", p.trick_hidden_text);
        assert!(!p.trick_hidden_text.contains("Зуб-Мастер"), "логотип с text-indent — не спам");
        assert!(!p.trick_hidden_text.contains("дикторов"), "текст для экранных дикторов — не спам");
        assert!(!p.trick_hidden_text.contains("Свёрнутый"), "свёрнутый блок посетитель может открыть");
        assert_eq!(p.trick_hidden_links, 1);
        assert_eq!(p.invisible_iframes.len(), 1);
        let plain = super::extract("<html><head><meta http-equiv=refresh content=\"300\"></head><body>x</body></html>", "https://zub.ru/");
        assert!(plain.meta_refresh.is_none(), "обновление той же страницы раз в 5 минут — не перенаправление");
    }
}
