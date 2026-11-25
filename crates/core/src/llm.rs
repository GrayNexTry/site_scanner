//! Нейросеть для писем клиентам через OpenRouter (<https://openrouter.ai>): любая модель из каталога OpenRouter.
//!
//! Профиль нейросети (`AiProfile`) — всё, что менеджер может править: модель и запасные модели, инструкция,
//! шаблоны запроса для письма и для мессенджера, рассуждение, параметры генерации, маршрутизация по
//! провайдерам и любые другие поля запроса (`extra`, накладываются последними).
//!
//! Факты для письма берутся те же, что у шаблона (`sales::letter_facts`): только подтверждённые проблемы
//! простыми словами. Ответ модели разбирается (`parse_letter`) и проверяется по правилам стиля (`lint_letter`).
//! Сеть — только у `Client`; остальное — чистые функции с тестами.

use std::collections::BTreeMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::sales::{LetterFacts, Manager};

pub const DEFAULT_BASE: &str = "https://openrouter.ai/api/v1";

/// Модель профиля по умолчанию: бесплатная (`:free` — без оплаты, 50 запросов в день, 1000 — после пополнения на $10).
pub const DEFAULT_MODEL: &str = "google/gemma-4-31b-it:free";

/// Уровни рассуждения OpenRouter (`reasoning.effort`).
pub const EFFORTS: &[&str] = &["max", "xhigh", "high", "medium", "low", "minimal"];

/// Что составить: письмо (тема + текст с подписью) или короткое сообщение для мессенджера.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Email,
    Short,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Email => "email",
            Kind::Short => "short",
        }
    }
}

/// Рассуждение модели (`reasoning` в запросе OpenRouter).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Reasoning {
    /// `default` — как у модели (параметр не отправляется), `off` — выключить, `effort` — уровень, `budget` — бюджет токенов.
    pub mode: String,
    /// Уровень для `effort`: max, xhigh, high, medium, low, minimal.
    pub effort: String,
    /// Бюджет токенов на рассуждение для `budget`.
    pub max_tokens: Option<u32>,
    /// Модель рассуждает, но ход рассуждения не возвращается (токены всё равно оплачиваются).
    pub exclude: bool,
}

impl Default for Reasoning {
    fn default() -> Self {
        Self { mode: "default".into(), effort: "medium".into(), max_tokens: None, exclude: false }
    }
}

/// Выбор провайдера модели (`provider` в запросе OpenRouter). Пустые поля не отправляются.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct ProviderPrefs {
    /// Провайдеры по порядку предпочтения (slug: `deepinfra`, `google-vertex`…).
    pub order: Vec<String>,
    /// Только эти провайдеры.
    pub only: Vec<String>,
    /// Не использовать этих провайдеров.
    pub ignore: Vec<String>,
    /// Переходить к другим провайдерам, если выбранные недоступны (по умолчанию OpenRouter — да).
    pub allow_fallbacks: Option<bool>,
    /// `deny` — только провайдеры, которые не хранят запросы и не учатся на них; пусто — как в аккаунте OpenRouter.
    pub data_collection: String,
    /// Только провайдеры с нулевым хранением данных (Zero Data Retention).
    pub zdr: bool,
    /// `price`, `throughput`, `latency`; пусто — балансировка OpenRouter.
    pub sort: String,
    /// Только провайдеры, которые поддерживают все параметры запроса.
    pub require_parameters: bool,
}

/// Профиль нейросети: модель, инструкция, шаблоны запроса и все параметры генерации.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AiProfile {
    pub id: String,
    pub name: String,
    /// Модель OpenRouter: `google/gemma-4-31b-it:free`, `anthropic/claude-…`.
    pub model: String,
    /// Запасные модели по порядку: если основная недоступна, OpenRouter берёт следующую.
    pub fallback_models: Vec<String>,
    /// Инструкция (системное сообщение). Можно использовать переменные `{{…}}`.
    pub system: String,
    /// Шаблон запроса для письма.
    pub email_prompt: String,
    /// Шаблон запроса для сообщения в мессенджер.
    pub short_prompt: String,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub top_k: Option<u32>,
    pub min_p: Option<f64>,
    pub top_a: Option<f64>,
    pub frequency_penalty: Option<f64>,
    pub presence_penalty: Option<f64>,
    pub repetition_penalty: Option<f64>,
    pub seed: Option<i64>,
    /// Предел токенов ответа (вместе с рассуждением).
    pub max_tokens: Option<u32>,
    pub stop: Vec<String>,
    /// `low`, `medium`, `high` — подробность ответа (модели OpenAI); пусто — не отправлять.
    pub verbosity: String,
    /// Ответ JSON-объектом: `auto` — если модель это умеет, `on` — всегда, `off` — обычный текст.
    pub json_mode: String,
    pub reasoning: Reasoning,
    pub provider: ProviderPrefs,
    /// Любые другие поля запроса OpenRouter (JSON-объект): накладываются поверх собранного запроса последними,
    /// `null` убирает поле.
    pub extra: Value,
}

impl Default for AiProfile {
    fn default() -> Self {
        Self {
            id: "default".into(),
            name: "Основной".into(),
            model: DEFAULT_MODEL.into(),
            fallback_models: Vec::new(),
            system: DEFAULT_SYSTEM.into(),
            email_prompt: DEFAULT_EMAIL_PROMPT.into(),
            short_prompt: DEFAULT_SHORT_PROMPT.into(),
            temperature: Some(0.7),
            top_p: None,
            top_k: None,
            min_p: None,
            top_a: None,
            frequency_penalty: None,
            presence_penalty: None,
            repetition_penalty: None,
            seed: None,
            max_tokens: None,
            stop: Vec::new(),
            verbosity: String::new(),
            json_mode: "auto".into(),
            reasoning: Reasoning::default(),
            provider: ProviderPrefs::default(),
            extra: Value::Null,
        }
    }
}

pub const DEFAULT_SYSTEM: &str = "Ты пишешь первое сообщение владельцу местного бизнеса от имени специалиста, который делает сайты и занимается SEO. Задача — чтобы человек ответил, а не продать всё в первом же письме.

Правила:
— Язык текста — {{language}}.
— Коротко, вежливо и по делу: приветствие, одна фраза о себе, главные проблемы, предложение, контакты. Письмо — до 120 слов без подписи, сообщение в мессенджер — до 80 слов.
— Проблемы бери только из запроса. Не придумывай фактов, цифр, сроков и обещаний вроде «первое место в Google» или «рост продаж на 50 %».
— Пиши простыми словами, как для владельца бизнеса, без технических терминов (HTTPS, мета-теги, schema, robots.txt, Core Web Vitals и т. п.): что видит клиент компании и чем это мешает.
— Проблемы — списком, каждая строка начинается с «— ». Без markdown: никаких **, #, ссылок в скобках и эмодзи.
— От первого лица без глаголов прошедшего времени с родом: не «нашёл», «нашла», «посмотрел», а «нашлось», «вижу», «есть».
— Без давления и громких слов: без «срочно», «вы теряете деньги», капса и восклицательных знаков подряд.
— Не пиши, что текст составлен автоматически или нейросетью, не называй программы и сервисы проверки.
— Имя, компания и контакты отправителя — только те, что даны в запросе; ничего не добавляй от себя.";

pub const DEFAULT_EMAIL_PROMPT: &str = "Составь первое письмо компании.

Компания: {{company}}
Чем занимается: {{category}}
Город: {{city}}
Сайт: {{site}}
Состояние сайта: {{site_state}}

Главные проблемы, о которых стоит написать:
{{problems}}

Полный список найденных проблем (главные из него уже выше; остальные упоминай, только если главных меньше двух):
{{problems_more}}

Что предложить: {{offer}}
Пример запроса, по которому клиенты ищут такие компании: {{query}}
Первая фраза о себе (можно перефразировать): {{intro}}
В конце письма — подпись, ровно так:
{{signature}}

Образец, который собрал шаблон (сделай живее и убедительнее, но факты бери только из списков выше):
Тема: {{template_subject}}
{{template_body}}

Ответь JSON-объектом без пояснений: {\"subject\": \"тема письма\", \"body\": \"текст письма вместе с подписью\"}.";

pub const DEFAULT_SHORT_PROMPT: &str = "Составь короткое первое сообщение компании для мессенджера (WhatsApp, Telegram, Instagram).

Компания: {{company}}
Чем занимается: {{category}}
Город: {{city}}
Сайт: {{site}}
Состояние сайта: {{site_state}}

Главные проблемы, о которых стоит написать:
{{problems}}

Что предложить: {{offer}}
Первая фраза о себе (можно перефразировать): {{intro}}
В конце сообщения — контакты одной строкой, ровно так: {{contact_line}}

Образец, который собрал шаблон (сделай живее, но факты бери только из списков выше):
{{template_short}}

Ответь JSON-объектом без пояснений: {\"body\": \"текст сообщения\"}.";

/// Переменные шаблонов: имя и что в ней.
pub const VARS: &[(&str, &str)] = &[
    ("company", "Название компании (как для документов)"),
    ("category", "Чем занимается — категория из карточки"),
    ("city", "Город скана"),
    ("country", "Страна (код: by, ru, gb…)"),
    ("address", "Адрес компании"),
    ("site", "Сайт клиента без https:// или «сайта нет»"),
    ("site_state", "Состояние сайта одной строкой: «Слабый сайт: 38/100…», «Сайта нет»"),
    ("score", "Балл сайта из 100"),
    ("lang", "Язык письма: ru или en"),
    ("language", "Язык письма словом: русский или английский"),
    ("problems", "Главные проблемы для клиента, по строке с «— » (те же, что в шаблонном письме)"),
    ("problems_more", "Все подтверждённые проблемы простыми словами, главные — тоже (до 12)"),
    ("good", "Что на сайте в порядке"),
    ("offer", "Что предложить"),
    ("intro", "Первая фраза о себе (из настроек или по категории)"),
    ("query", "Пример поискового запроса клиентов: «стоматолог Минск»"),
    ("audience", "Кого теряет компания: пациентов, клиентов, гостей"),
    ("contact_person", "Контактное лицо (ЛПР), если указано"),
    ("notes", "Заметки менеджера о компании"),
    ("signature", "Подпись письма: «С уважением,» и контакты"),
    ("contact_line", "Контакты одной строкой — для мессенджера"),
    ("manager_name", "Ваше имя"),
    ("manager_email", "Ваша почта"),
    ("manager_phone", "Ваш телефон"),
    ("manager_telegram", "Ваш Telegram"),
    ("manager_company", "Ваша компания"),
    ("template_subject", "Тема письма по шаблону"),
    ("template_body", "Письмо по шаблону"),
    ("template_short", "Сообщение для мессенджера по шаблону"),
];

/// Привести профиль в порядок: обрезать пробелы, отбросить неизвестные значения, задать id и название.
pub fn normalize_profile(p: &AiProfile, fallback_id: &str) -> AiProfile {
    let mut p = p.clone();
    let slug: String = p.id.trim().chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')).take(40).collect();
    p.id = if slug.is_empty() { fallback_id.to_string() } else { slug };
    p.name = p.name.trim().chars().take(80).collect();
    if p.name.is_empty() {
        p.name = "Без названия".into();
    }
    p.model = p.model.trim().to_string();
    let clean_list = |v: &[String]| v.iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect::<Vec<_>>();
    p.fallback_models = clean_list(&p.fallback_models).into_iter().filter(|m| *m != p.model).collect();
    p.stop = p.stop.iter().filter(|s| !s.is_empty()).take(4).cloned().collect();
    if !["low", "medium", "high"].contains(&p.verbosity.as_str()) {
        p.verbosity.clear();
    }
    if !["auto", "on", "off"].contains(&p.json_mode.as_str()) {
        p.json_mode = "auto".into();
    }
    if !["default", "off", "effort", "budget"].contains(&p.reasoning.mode.as_str()) {
        p.reasoning.mode = "default".into();
    }
    if !EFFORTS.contains(&p.reasoning.effort.as_str()) {
        p.reasoning.effort = "medium".into();
    }
    p.provider.order = clean_list(&p.provider.order);
    p.provider.only = clean_list(&p.provider.only);
    p.provider.ignore = clean_list(&p.provider.ignore);
    if !["allow", "deny"].contains(&p.provider.data_collection.as_str()) {
        p.provider.data_collection.clear();
    }
    if !["price", "throughput", "latency"].contains(&p.provider.sort.as_str()) {
        p.provider.sort.clear();
    }
    if !(p.extra.is_object() || p.extra.is_null()) {
        p.extra = Value::Null;
    }
    p
}

/// Список профилей из настроек: хотя бы один, id без повторов.
pub fn normalize_profiles(list: &[AiProfile]) -> Vec<AiProfile> {
    let mut out: Vec<AiProfile> = Vec::new();
    for (n, p) in list.iter().enumerate() {
        let mut p = normalize_profile(p, &format!("p{}", n + 1));
        while out.iter().any(|x| x.id == p.id) {
            p.id = format!("{}-{}", p.id, n + 1);
        }
        out.push(p);
    }
    if out.is_empty() {
        out.push(AiProfile::default());
    }
    out
}

// ---------------------------------------------------------------- переменные и шаблоны

/// Всё о компании, что может понадобиться в запросе, кроме фактов шаблона.
pub struct LetterContext<'a> {
    pub facts: &'a LetterFacts,
    pub company: &'a str,
    pub category: Option<&'a str>,
    pub country: Option<&'a str>,
    pub address: Option<&'a str>,
    /// Одна строка мини-отчёта: «Слабый сайт: 38/100, 9 критичных проблем».
    pub site_state: &'a str,
    pub score: Option<f64>,
    /// Остальные подтверждённые проблемы простыми словами.
    pub more_problems: &'a [String],
    /// Что в порядке простыми словами.
    pub good: &'a [String],
    pub contact_person: Option<&'a str>,
    pub notes: Option<&'a str>,
    pub manager: &'a Manager,
}

fn dash_list(items: &[String]) -> String {
    items.iter().map(|s| format!("— {}", s.trim())).collect::<Vec<_>>().join("\n")
}

/// Значения переменных для шаблонов. Пустые — словами («нет», «не указано»), чтобы модель не гадала.
pub fn letter_vars(c: &LetterContext) -> BTreeMap<String, String> {
    let f = c.facts;
    let ru = f.lang == "ru";
    let none = if ru { "нет" } else { "none" };
    let unknown = if ru { "не указано" } else { "not specified" };
    let opt = |v: Option<&str>| v.map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let or_unknown = |v: Option<&str>| opt(v).unwrap_or_else(|| unknown.to_string());
    let list = |v: &[String]| if v.is_empty() { none.to_string() } else { dash_list(v) };
    let m = c.manager;
    let mut v = BTreeMap::new();
    let mut put = |k: &str, val: String| {
        v.insert(k.to_string(), val);
    };
    put("company", c.company.trim().to_string());
    put("category", or_unknown(c.category));
    put("city", or_unknown(f.city.as_deref()));
    put("country", or_unknown(c.country));
    put("address", or_unknown(c.address));
    put("site", f.host.clone().unwrap_or_else(|| if ru { "сайта нет".into() } else { "no website".into() }));
    put("site_state", or_unknown(Some(c.site_state)));
    put("score", c.score.map(|s| format!("{}", s.round())).unwrap_or_else(|| unknown.to_string()));
    put("lang", f.lang.to_string());
    put("language", if ru { "русский".into() } else { "английский (English)".into() });
    put("problems", list(&f.problems));
    put("problems_more", list(c.more_problems));
    put("good", list(c.good));
    put("offer", f.offer.clone());
    put("intro", f.intro.clone());
    put("query", f.query.clone().unwrap_or_else(|| unknown.to_string()));
    put("audience", f.audience.to_string());
    put("contact_person", or_unknown(c.contact_person));
    put("notes", opt(c.notes).unwrap_or_else(|| none.to_string()));
    let no_sign = if ru { "(подписи нет — ничего не добавляй)" } else { "(no signature — add nothing)" };
    put("signature", if f.signature.is_empty() { no_sign.to_string() } else { f.signature.clone() });
    put("contact_line", if f.contact_line.is_empty() { no_sign.to_string() } else { f.contact_line.clone() });
    put("manager_name", or_unknown(Some(&m.name)));
    put("manager_email", or_unknown(Some(&m.email)));
    put("manager_phone", or_unknown(Some(&m.phone)));
    put("manager_telegram", or_unknown(Some(&m.telegram)));
    put("manager_company", or_unknown(Some(&m.company)));
    put("template_subject", f.template.email_subject.clone());
    put("template_body", f.template.email_body.clone());
    put("template_short", f.template.short.clone());
    v
}

fn var_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\{\{\s*([A-Za-z_][A-Za-z0-9_]*)\s*\}\}").unwrap())
}

/// Подставить `{{имя}}`. Неизвестные переменные остаются как есть и возвращаются списком.
pub fn render(template: &str, vars: &BTreeMap<String, String>) -> (String, Vec<String>) {
    let mut unknown: Vec<String> = Vec::new();
    let out = var_re()
        .replace_all(template, |c: &regex::Captures| match vars.get(&c[1]) {
            Some(v) => v.clone(),
            None => {
                if !unknown.contains(&c[1].to_string()) {
                    unknown.push(c[1].to_string());
                }
                c[0].to_string()
            }
        })
        .into_owned();
    (out, unknown)
}

// ---------------------------------------------------------------- запрос

/// Что умеет модель — из каталога OpenRouter (`GET /models`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub context_length: u64,
    pub max_completion_tokens: Option<u64>,
    /// Цена в долларах за 1 млн токенов запроса и ответа; None — переменная (роутеры).
    pub prompt_price: Option<f64>,
    pub completion_price: Option<f64>,
    /// Бесплатная модель (`:free` или нулевая цена).
    pub free: bool,
    pub supported_parameters: Vec<String>,
    pub reasoning: Option<ReasoningCaps>,
    pub input_modalities: Vec<String>,
    /// Когда модель появилась в OpenRouter (unix-время).
    pub created: i64,
    pub expiration_date: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ReasoningCaps {
    pub supported_efforts: Vec<String>,
    pub default_effort: Option<String>,
    pub default_enabled: Option<bool>,
    /// Рассуждение не отключается (`effort: none` модель отвергнет).
    pub mandatory: bool,
    pub supports_max_tokens: bool,
}

impl ModelInfo {
    pub fn supports(&self, param: &str) -> bool {
        self.supported_parameters.iter().any(|p| p == param)
    }
}

/// Цена из строки «0.00000001» за токен → доллары за 1 млн токенов.
fn per_million(v: &Value) -> Option<f64> {
    let x: f64 = match v {
        Value::String(s) => s.trim().parse().ok()?,
        Value::Number(n) => n.as_f64()?,
        _ => return None,
    };
    (x >= 0.0).then(|| (x * 1_000_000.0 * 1e6).round() / 1e6)
}

/// Каталог моделей OpenRouter → модели, которые отвечают текстом.
pub fn parse_models(v: &Value) -> Vec<ModelInfo> {
    let strs = |x: &Value| x.as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect::<Vec<_>>()).unwrap_or_default();
    let mut out: Vec<ModelInfo> = v["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| {
            let outm = strs(&m["architecture"]["output_modalities"]);
            outm.is_empty() || outm.iter().any(|x| x == "text")
        })
        .filter_map(|m| {
            let id = m["id"].as_str()?.to_string();
            let pricing = &m["pricing"];
            let prompt_price = per_million(&pricing["prompt"]);
            let completion_price = per_million(&pricing["completion"]);
            let request = per_million(&pricing["request"]).unwrap_or(0.0);
            let free = id.ends_with(":free") || (prompt_price == Some(0.0) && completion_price == Some(0.0) && request == 0.0);
            let reasoning = m.get("reasoning").filter(|r| r.is_object()).map(|r| ReasoningCaps {
                supported_efforts: strs(&r["supported_efforts"]),
                default_effort: r["default_effort"].as_str().map(str::to_string),
                default_enabled: r["default_enabled"].as_bool(),
                mandatory: r["mandatory"].as_bool().unwrap_or(false),
                supports_max_tokens: r["supports_max_tokens"].as_bool().unwrap_or(false),
            });
            let description: String = m["description"].as_str().unwrap_or("").chars().take(400).collect();
            Some(ModelInfo {
                name: m["name"].as_str().unwrap_or(&id).to_string(),
                id,
                description,
                context_length: m["context_length"].as_u64().unwrap_or(0),
                max_completion_tokens: m["top_provider"]["max_completion_tokens"].as_u64(),
                prompt_price,
                completion_price,
                free,
                supported_parameters: strs(&m["supported_parameters"]),
                reasoning,
                input_modalities: strs(&m["architecture"]["input_modalities"]),
                created: m["created"].as_i64().unwrap_or(0),
                expiration_date: m["expiration_date"].as_str().map(str::to_string),
            })
        })
        .collect();
    out.sort_by_cached_key(|m| m.name.to_lowercase());
    out
}

/// Собранный запрос и замечания к нему.
#[derive(Debug, Clone, Serialize)]
pub struct BuiltRequest {
    /// Тело запроса к `/chat/completions` — ровно то, что уйдёт в OpenRouter (ключ — в заголовке, не здесь).
    pub body: Value,
    /// Переменные в шаблонах, которых нет в списке.
    pub unknown_vars: Vec<String>,
    /// Параметры, которые выбранная модель не поддерживает: OpenRouter их проигнорирует.
    pub unsupported: Vec<String>,
}

fn letter_schema(kind: Kind) -> Value {
    let (props, req) = match kind {
        Kind::Email => (json!({"subject": {"type": "string"}, "body": {"type": "string"}}), json!(["subject", "body"])),
        Kind::Short => (json!({"body": {"type": "string"}}), json!(["body"])),
    };
    json!({"type": "json_schema", "json_schema": {"name": "letter", "strict": true, "schema": {"type": "object", "properties": props, "required": req, "additionalProperties": false}}})
}

/// Наложить `extra` на запрос: объекты сливаются по ключам, остальное заменяется, `null` убирает поле.
pub fn merge_json(base: &mut Value, extra: &Value) {
    match (base, extra) {
        (Value::Object(b), Value::Object(e)) => {
            for (k, v) in e {
                if v.is_null() {
                    b.remove(k);
                } else if let Some(slot) = b.get_mut(k).filter(|s| s.is_object() && v.is_object()) {
                    merge_json(slot, v);
                } else {
                    b.insert(k.clone(), v.clone());
                }
            }
        }
        (b, e) if !e.is_null() => *b = e.clone(),
        _ => {}
    }
}

/// Собрать тело запроса к OpenRouter по профилю. `caps` — что умеет модель (для `json_mode: auto` и предупреждений).
pub fn build_request(p: &AiProfile, kind: Kind, vars: &BTreeMap<String, String>, caps: Option<&ModelInfo>) -> BuiltRequest {
    let (system, mut unknown) = render(&p.system, vars);
    let template = match kind {
        Kind::Email => &p.email_prompt,
        Kind::Short => &p.short_prompt,
    };
    let (user, u2) = render(template, vars);
    for u in u2 {
        if !unknown.contains(&u) {
            unknown.push(u);
        }
    }
    let mut messages: Vec<Value> = Vec::new();
    if !system.trim().is_empty() {
        messages.push(json!({"role": "system", "content": system}));
    }
    messages.push(json!({"role": "user", "content": user}));

    let mut b = Map::new();
    b.insert("model".into(), json!(p.model));
    if !p.fallback_models.is_empty() {
        let mut all = vec![p.model.clone()];
        all.extend(p.fallback_models.iter().cloned());
        b.insert("models".into(), json!(all));
    }
    b.insert("messages".into(), Value::Array(messages));
    let mut used: Vec<&str> = Vec::new();
    let mut num = |name: &'static str, v: Option<Value>| {
        if let Some(v) = v {
            b.insert(name.into(), v);
            used.push(name);
        }
    };
    num("temperature", p.temperature.map(|x| json!(x)));
    num("top_p", p.top_p.map(|x| json!(x)));
    num("top_k", p.top_k.map(|x| json!(x)));
    num("min_p", p.min_p.map(|x| json!(x)));
    num("top_a", p.top_a.map(|x| json!(x)));
    num("frequency_penalty", p.frequency_penalty.map(|x| json!(x)));
    num("presence_penalty", p.presence_penalty.map(|x| json!(x)));
    num("repetition_penalty", p.repetition_penalty.map(|x| json!(x)));
    num("seed", p.seed.map(|x| json!(x)));
    num("max_tokens", p.max_tokens.map(|x| json!(x)));
    num("stop", (!p.stop.is_empty()).then(|| json!(p.stop)));
    num("verbosity", (!p.verbosity.is_empty()).then(|| json!(p.verbosity)));

    let r = &p.reasoning;
    let reasoning = match r.mode.as_str() {
        "off" => Some(json!({"effort": "none"})),
        "effort" => Some(json!({"effort": r.effort, "exclude": r.exclude})),
        "budget" => Some(json!({"max_tokens": r.max_tokens.unwrap_or(2000), "exclude": r.exclude})),
        _ if r.exclude => Some(json!({"exclude": true})),
        _ => None,
    };
    if let Some(x) = reasoning {
        b.insert("reasoning".into(), x);
        used.push("reasoning");
    }

    let json_on = match p.json_mode.as_str() {
        "on" => true,
        "off" => false,
        _ => caps.is_some_and(|c| c.supports("structured_outputs") || c.supports("response_format")),
    };
    if json_on {
        let schema = caps.is_none_or(|c| c.supports("structured_outputs"));
        b.insert("response_format".into(), if schema { letter_schema(kind) } else { json!({"type": "json_object"}) });
        used.push("response_format");
    }

    let pr = &p.provider;
    let mut prov = Map::new();
    if !pr.order.is_empty() {
        prov.insert("order".into(), json!(pr.order));
    }
    if !pr.only.is_empty() {
        prov.insert("only".into(), json!(pr.only));
    }
    if !pr.ignore.is_empty() {
        prov.insert("ignore".into(), json!(pr.ignore));
    }
    if let Some(a) = pr.allow_fallbacks {
        prov.insert("allow_fallbacks".into(), json!(a));
    }
    if !pr.data_collection.is_empty() {
        prov.insert("data_collection".into(), json!(pr.data_collection));
    }
    if pr.zdr {
        prov.insert("zdr".into(), json!(true));
    }
    if !pr.sort.is_empty() {
        prov.insert("sort".into(), json!(pr.sort));
    }
    if pr.require_parameters {
        prov.insert("require_parameters".into(), json!(true));
    }
    if !prov.is_empty() {
        b.insert("provider".into(), Value::Object(prov));
    }

    let mut body = Value::Object(b);
    merge_json(&mut body, &p.extra);
    let unsupported = match caps {
        Some(c) if !c.supported_parameters.is_empty() => used
            .iter()
            .filter(|n| !c.supports(n) && !(**n == "response_format" && c.supports("structured_outputs")))
            .map(|n| n.to_string())
            .collect(),
        _ => Vec::new(),
    };
    BuiltRequest { body, unknown_vars: unknown, unsupported }
}

// ---------------------------------------------------------------- ответ

/// Что вернула модель.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChatResult {
    /// Текст ответа.
    pub content: String,
    /// Ход рассуждения, если модель его вернула.
    pub reasoning: Option<String>,
    /// Модель, которая ответила (может быть запасная).
    pub model: String,
    pub provider: Option<String>,
    pub finish_reason: Option<String>,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub reasoning_tokens: u64,
    pub cached_tokens: u64,
    /// Стоимость в кредитах OpenRouter (≈ доллары).
    pub cost: Option<f64>,
    pub ms: u64,
    pub id: Option<String>,
}

/// Ошибка OpenRouter понятными словами.
#[derive(Debug, Clone, Serialize)]
pub struct AiError {
    /// HTTP-код (0 — до OpenRouter не достучались).
    pub status: u16,
    pub message: String,
    /// Через сколько секунд можно повторить (429).
    pub retry_after: Option<u64>,
}

impl std::fmt::Display for AiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AiError {}

impl AiError {
    /// Стоит повторить через паузу: лимит частоты, перегрузка или сбой провайдера.
    pub fn retryable(&self) -> bool {
        matches!(self.status, 0 | 408 | 429 | 500 | 502 | 503 | 504 | 529)
    }
}

/// Сообщение об ошибке OpenRouter: `{"error": {"code", "message", "metadata"}}`.
pub fn explain_error(status: u16, body: &Value) -> AiError {
    let err = &body["error"];
    let mut msg = err["message"].as_str().unwrap_or("").trim().to_string();
    // У ошибки провайдера подробность — в metadata.raw.
    if let Some(raw) = err["metadata"]["raw"].as_str().map(str::trim).filter(|r| !r.is_empty()) {
        let raw: String = raw.chars().take(300).collect();
        msg = if msg.is_empty() { raw } else { format!("{msg} — {raw}") };
    }
    if let Some(p) = err["metadata"]["provider_name"].as_str() {
        msg = format!("{msg} (провайдер {p})");
    }
    let code = err["code"].as_u64().map(|c| c as u16).filter(|c| *c >= 400).unwrap_or(status);
    let why = match code {
        400 => "OpenRouter не принял запрос — проверьте параметры профиля и поле «Свои параметры (JSON)»",
        401 => "Ключ OpenRouter не подходит: проверьте его в настройках",
        402 => "На счёте OpenRouter не хватает кредитов: пополните счёт или выберите бесплатную модель (:free)",
        403 => "Запрос отклонён: модерация провайдера или модель недоступна из вашей страны",
        404 => "Нет подходящей модели или провайдера: проверьте имя модели, списки провайдеров и «не отдавать запросы на обучение» (бесплатным моделям это часто мешает)",
        408 => "Модель не успела ответить",
        413 => "Запрос слишком длинный для этой модели",
        429 => "Слишком много запросов: у бесплатных моделей 20 в минуту и 50 в день (1000 — после пополнения на $10)",
        502 | 503 | 504 | 529 => "Провайдер модели не ответил или перегружен — попробуйте ещё раз или добавьте запасные модели",
        _ => "Ошибка OpenRouter",
    };
    let message = if msg.is_empty() { format!("{why} ({code}).") } else { format!("{why} ({code}): {msg}") };
    AiError { status: code, message, retry_after: None }
}

/// Разобрать ответ `/chat/completions`.
pub fn parse_chat(v: &Value, ms: u64) -> Result<ChatResult, AiError> {
    if v.get("error").is_some_and(|e| !e.is_null()) {
        return Err(explain_error(200, v));
    }
    let choice = &v["choices"][0];
    if choice.get("error").is_some_and(|e| !e.is_null()) {
        return Err(explain_error(502, &json!({"error": choice["error"]})));
    }
    let msg = &choice["message"];
    let content = match &msg["content"] {
        Value::String(s) => s.clone(),
        // Часть провайдеров отдаёт содержимое списком частей.
        Value::Array(parts) => parts.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join(""),
        _ => String::new(),
    };
    let reasoning = msg["reasoning"].as_str().map(str::to_string).filter(|s| !s.trim().is_empty()).or_else(|| {
        let parts: Vec<&str> = msg["reasoning_details"].as_array().into_iter().flatten().filter_map(|d| d["text"].as_str().or(d["summary"].as_str())).collect();
        (!parts.is_empty()).then(|| parts.join("\n"))
    });
    let u = &v["usage"];
    let finish = choice["finish_reason"].as_str().map(str::to_string);
    let r = ChatResult {
        content,
        reasoning,
        model: v["model"].as_str().unwrap_or("").to_string(),
        provider: v["provider"].as_str().map(str::to_string),
        finish_reason: finish.clone(),
        prompt_tokens: u["prompt_tokens"].as_u64().unwrap_or(0),
        completion_tokens: u["completion_tokens"].as_u64().unwrap_or(0),
        reasoning_tokens: u["completion_tokens_details"]["reasoning_tokens"].as_u64().unwrap_or(0),
        cached_tokens: u["prompt_tokens_details"]["cached_tokens"].as_u64().unwrap_or(0),
        cost: u["cost"].as_f64(),
        ms,
        id: v["id"].as_str().map(str::to_string),
    };
    if r.content.trim().is_empty() {
        let message = if finish.as_deref() == Some("length") {
            "Ответ оборвался: модель израсходовала предел токенов (часто — на рассуждение). Увеличьте «Предел токенов ответа» или уменьшите рассуждение.".to_string()
        } else {
            format!("Модель вернула пустой ответ{}.", finish.map(|f| format!(" (finish_reason: {f})")).unwrap_or_default())
        };
        return Err(AiError { status: 502, message, retry_after: None });
    }
    Ok(r)
}

/// Письмо или сообщение из ответа модели.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Letter {
    pub subject: String,
    pub body: String,
}

fn strip_fences(s: &str) -> &str {
    let t = s.trim();
    let Some(rest) = t.strip_prefix("```") else { return t };
    let rest = rest.split_once('\n').map(|(_, r)| r).unwrap_or("");
    rest.trim_end().strip_suffix("```").unwrap_or(rest).trim()
}

fn json_field(o: &Map<String, Value>, names: &[&str]) -> Option<String> {
    names.iter().find_map(|n| o.iter().find(|(k, _)| k.to_lowercase() == *n).and_then(|(_, v)| v.as_str()).map(|s| s.trim().to_string()))
}

/// Разобрать ответ: JSON `{subject, body}` (в том числе внутри ```json```), иначе текст с первой строкой «Тема: …».
pub fn parse_letter(content: &str, kind: Kind) -> Letter {
    let t = strip_fences(content);
    if let (Some(a), Some(z)) = (t.find('{'), t.rfind('}'))
        && a < z
        && let Ok(Value::Object(o)) = serde_json::from_str::<Value>(&t[a..=z])
        && let Some(body) = json_field(&o, &["body", "text", "message", "email", "письмо", "текст"])
    {
        let subject = if kind == Kind::Email { json_field(&o, &["subject", "title", "тема"]).unwrap_or_default() } else { String::new() };
        return Letter { subject, body };
    }
    if kind == Kind::Email {
        let mut lines = t.lines();
        let first = lines.by_ref().find(|l| !l.trim().is_empty()).unwrap_or("");
        // «**Тема:** …», «Тема письма: …», «Subject: …».
        let head: String = first.replace("**", "").trim().trim_start_matches('#').trim().to_string();
        let lower = head.to_lowercase();
        for p in ["тема письма:", "тема:", "subject:"] {
            if lower.starts_with(p) {
                let subj: String = head.chars().skip(p.chars().count()).collect();
                let body = lines.collect::<Vec<_>>().join("\n").trim().to_string();
                return Letter { subject: subj.trim().to_string(), body };
            }
        }
    }
    Letter { subject: String::new(), body: t.to_string() }
}

fn re(pattern: &'static str, cell: &'static OnceLock<Regex>) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).unwrap())
}

/// Проверка текста по правилам стиля: что поправить перед отправкой. Пустой список — замечаний нет.
pub fn lint_letter(l: &Letter, kind: Kind, lang: &str, must_contain: &[String]) -> Vec<String> {
    static GENDER: OnceLock<Regex> = OnceLock::new();
    static MD: OnceLock<Regex> = OnceLock::new();
    static BRACKETS: OnceLock<Regex> = OnceLock::new();
    static AI: OnceLock<Regex> = OnceLock::new();
    let mut out: Vec<String> = Vec::new();
    let text = format!("{}\n{}", l.subject, l.body);
    if l.body.trim().is_empty() {
        out.push("Пустой текст.".into());
        return out;
    }
    if kind == Kind::Email && l.subject.trim().is_empty() {
        out.push("Нет темы письма.".into());
    }
    if text.contains("{{") {
        out.push("Остались незаполненные переменные {{…}}.".into());
    }
    if re(r"\*\*|__|(?m)^\s*#{1,6}\s|\[[^\]]+\]\(https?://", &MD).is_match(&text) {
        out.push("Есть разметка markdown (**, #, [текст](ссылка)) — в письме и мессенджере она видна как символы.".into());
    }
    if let Some(m) = re(r"\[[^\]\n]{2,40}\]", &BRACKETS).find(&text) {
        out.push(format!("Заготовка в квадратных скобках: {} — модель не подставила данные.", m.as_str()));
    }
    if lang == "ru" {
        let g = re(
            r"(?i)(?:^|[.!?\n]\s*|\bя\s+(?:\S+\s+)?)(наш[её]л|нашла|посмотрела?|проверила?|заметила?|увидела?|изучила?|проанализировала?|подготовила?|обратила?|решила?|сделала?)\b",
            &GENDER,
        );
        if let Some(c) = g.captures(&text) {
            out.push(format!("Глагол с родом от первого лица: «{}» — по правилам лучше «нашлось», «вижу», «есть».", &c[1]));
        }
    }
    if re(r"(?i)нейросет|искусственн\w* интеллект|chatgpt|\bgpt\b|as an ai|ai-generated|language model", &AI).is_match(&text) {
        out.push("Упоминается нейросеть или ИИ.".into());
    }
    let letters: Vec<char> = l.body.chars().filter(|c| c.is_alphabetic()).collect();
    if letters.len() > 40 {
        let cyr = letters.iter().filter(|c| matches!(c, 'а'..='я' | 'А'..='Я' | 'ё' | 'Ё')).count() as f64 / letters.len() as f64;
        if lang == "ru" && cyr < 0.5 {
            out.push("Текст не на русском, а клиенту нужно по-русски.".into());
        } else if lang == "en" && cyr > 0.2 {
            out.push("Текст не на английском, а клиенту нужно по-английски.".into());
        }
    }
    let words = l.body.split_whitespace().count();
    let limit = if kind == Kind::Email { 220 } else { 130 };
    if words > limit {
        out.push(format!("Длинно: {words} слов (для первого {} лучше до {limit}).", if kind == Kind::Email { "письма" } else { "сообщения" }));
    }
    let lower = text.to_lowercase();
    for want in must_contain.iter().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        if !lower.contains(&want.to_lowercase()) {
            out.push(format!("Нет ваших контактов: «{want}»."));
        }
    }
    out
}

// ---------------------------------------------------------------- сеть

/// Клиент OpenRouter. Ходит обычным маршрутом системы (через VPN, если он включён): часть провайдеров
/// моделей не обслуживает некоторые страны.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base: String,
}

impl Client {
    pub fn new(base: Option<&str>) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(300))
            .connect_timeout(Duration::from_secs(15))
            .user_agent(concat!("maps-seo-scanner/", env!("CARGO_PKG_VERSION")))
            .build()?;
        let base = base.map(str::trim).filter(|b| !b.is_empty()).unwrap_or(DEFAULT_BASE).trim_end_matches('/').to_string();
        Ok(Self { http, base })
    }

    async fn send(&self, req: reqwest::RequestBuilder) -> Result<(u16, Value, Option<u64>), AiError> {
        let resp = req.send().await.map_err(|e| AiError {
            status: 0,
            message: format!("Нет связи с OpenRouter: {}", if e.is_timeout() { "истекло время ожидания".to_string() } else { e.to_string() }),
            retry_after: None,
        })?;
        let status = resp.status().as_u16();
        let retry = resp.headers().get("retry-after").and_then(|h| h.to_str().ok()).and_then(|s| s.trim().parse::<u64>().ok());
        let text = resp.text().await.map_err(|e| AiError { status: 0, message: format!("Ответ OpenRouter оборвался: {e}"), retry_after: None })?;
        // Пока модель думает, OpenRouter держит соединение пробелами: JSON — после них.
        let v: Value = serde_json::from_str(text.trim()).unwrap_or_else(|_| json!({"error": {"message": text.trim().chars().take(300).collect::<String>()}}));
        Ok((status, v, retry))
    }

    /// Запрос к модели. Тело — из `build_request`.
    pub async fn chat(&self, key: &str, body: &Value) -> Result<ChatResult, AiError> {
        if key.trim().is_empty() {
            return Err(AiError { status: 401, message: "Не задан ключ OpenRouter: Настройки → Нейросеть.".into(), retry_after: None });
        }
        let t = Instant::now();
        let (status, v, retry) = self.send(self.http.post(format!("{}/chat/completions", self.base)).bearer_auth(key.trim()).json(body)).await?;
        if status >= 400 {
            let mut e = explain_error(status, &v);
            e.retry_after = retry;
            return Err(e);
        }
        parse_chat(&v, t.elapsed().as_millis() as u64)
    }

    /// Каталог моделей (ключ не нужен).
    pub async fn models(&self) -> Result<Vec<ModelInfo>, AiError> {
        let (status, v, _) = self.send(self.http.get(format!("{}/models", self.base))).await?;
        if status >= 400 {
            return Err(explain_error(status, &v));
        }
        Ok(parse_models(&v))
    }

    /// Сведения о ключе: остаток кредитов, бесплатные запросы на сегодня.
    pub async fn key_info(&self, key: &str) -> Result<Value, AiError> {
        if key.trim().is_empty() {
            return Err(AiError { status: 401, message: "Не задан ключ OpenRouter.".into(), retry_after: None });
        }
        let (status, v, _) = self.send(self.http.get(format!("{}/key", self.base)).bearer_auth(key.trim())).await?;
        if status >= 400 {
            return Err(explain_error(status, &v));
        }
        Ok(v["data"].clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sales::Outreach;

    fn facts() -> LetterFacts {
        LetterFacts {
            lang: "ru",
            no_site: false,
            problems: vec!["браузер пишет «Не защищено»".into(), "главная весит около 15 МБ".into()],
            offer: "Могу всё исправить.".into(),
            intro: "Я делаю сайты и SEO для клиник.".into(),
            query: Some("стоматолог Минск".into()),
            city: Some("Минск".into()),
            audience: "пациентов",
            host: Some("belodent.by".into()),
            signature: "С уважением,\nhello@example.com".into(),
            contact_line: "Почта для связи: hello@example.com".into(),
            template: Outreach { lang: "ru", short: "Здравствуйте! …".into(), email_subject: "Сайт belodent.by — что мешает пациентам вас найти".into(), email_body: "Здравствуйте! …".into() },
        }
    }

    fn vars() -> BTreeMap<String, String> {
        let f = facts();
        let m = Manager { email: "hello@example.com".into(), ..Default::default() };
        letter_vars(&LetterContext {
            facts: &f,
            company: "Белодент",
            category: Some("Стоматология"),
            country: Some("by"),
            address: None,
            site_state: "Слабый сайт: 38/100",
            score: Some(38.4),
            more_problems: &[],
            good: &["есть HTTPS".into()],
            contact_person: None,
            notes: None,
            manager: &m,
        })
    }

    #[test]
    fn every_documented_var_is_filled_and_defaults_render() {
        let v = vars();
        for (name, _) in VARS {
            assert!(v.contains_key(*name), "нет значения для {{{{{name}}}}}");
        }
        assert_eq!(v.len(), VARS.len(), "переменная без описания в VARS");
        assert_eq!(v["problems"], "— браузер пишет «Не защищено»\n— главная весит около 15 МБ");
        assert_eq!((v["problems_more"].as_str(), v["address"].as_str(), v["score"].as_str()), ("нет", "не указано", "38"));
        let p = AiProfile::default();
        for kind in [Kind::Email, Kind::Short] {
            let b = build_request(&p, kind, &v, None);
            assert!(b.unknown_vars.is_empty(), "в шаблоне по умолчанию неизвестные переменные: {:?}", b.unknown_vars);
            let user = b.body["messages"][1]["content"].as_str().unwrap();
            assert!(user.contains("belodent.by") && !user.contains("{{"));
            assert!(b.body["messages"][0]["content"].as_str().unwrap().contains("Язык текста — русский"));
        }
    }

    #[test]
    fn render_keeps_unknown() {
        let (s, u) = render("Привет, {{company}} и {{ nope }}{{nope}}", &vars());
        assert_eq!(s, "Привет, Белодент и {{ nope }}{{nope}}");
        assert_eq!(u, vec!["nope".to_string()]);
    }

    #[test]
    fn request_sends_only_set_params_and_merges_extra() {
        let caps = ModelInfo { supported_parameters: vec!["temperature".into(), "reasoning".into(), "response_format".into()], ..Default::default() };
        let mut p = AiProfile { temperature: Some(0.3), seed: Some(7), fallback_models: vec!["b/m".into()], ..Default::default() };
        p.reasoning = Reasoning { mode: "effort".into(), effort: "high".into(), max_tokens: None, exclude: true };
        p.provider = ProviderPrefs { data_collection: "deny".into(), sort: "price".into(), ..Default::default() };
        p.extra = json!({"provider": {"zdr": true}, "temperature": null, "plugins": [{"id": "response-healing"}]});
        let b = build_request(&p, Kind::Email, &vars(), Some(&caps));
        let body = &b.body;
        assert_eq!(body["models"], json!([DEFAULT_MODEL, "b/m"]));
        assert!(body.get("temperature").is_none(), "null в extra убирает поле");
        assert!(body.get("top_p").is_none(), "незаданное не отправляем");
        assert_eq!(body["seed"], 7);
        assert_eq!(body["reasoning"], json!({"effort": "high", "exclude": true}));
        assert_eq!(body["provider"], json!({"data_collection": "deny", "sort": "price", "zdr": true}));
        assert_eq!(body["response_format"], json!({"type": "json_object"}), "без structured_outputs — json_object");
        assert_eq!(body["plugins"][0]["id"], "response-healing");
        assert_eq!(b.unsupported, vec!["seed".to_string()], "seed модель не поддерживает");

        p.reasoning.mode = "off".into();
        p.json_mode = "off".into();
        let b = build_request(&p, Kind::Short, &vars(), Some(&caps));
        assert_eq!(b.body["reasoning"], json!({"effort": "none"}));
        assert!(b.body.get("response_format").is_none());
        p.reasoning = Reasoning { mode: "budget".into(), max_tokens: Some(4000), ..Default::default() };
        p.json_mode = "on".into();
        let b = build_request(&p, Kind::Short, &vars(), None);
        assert_eq!(b.body["reasoning"], json!({"max_tokens": 4000, "exclude": false}));
        assert_eq!(b.body["response_format"]["json_schema"]["schema"]["required"], json!(["body"]));
    }

    #[test]
    fn normalize_cleans_profiles() {
        let bad = AiProfile { id: " мой профиль! ".into(), name: "  ".into(), json_mode: "maybe".into(), extra: json!([1]), stop: vec!["".into(), "\n\n".into()], ..Default::default() };
        let list = normalize_profiles(&[bad.clone(), bad]);
        assert_eq!(list.len(), 2);
        assert_eq!((list[0].id.as_str(), list[1].id.as_str()), ("p1", "p2"));
        assert_eq!((list[0].name.as_str(), list[0].json_mode.as_str()), ("Без названия", "auto"));
        assert!(list[0].extra.is_null());
        assert_eq!(list[0].stop, vec!["\n\n".to_string()]);
        assert_eq!(normalize_profiles(&[]).len(), 1, "хотя бы один профиль");
    }

    #[test]
    fn parses_letters_in_any_shape() {
        let l = parse_letter("```json\n{\"subject\": \"Тема\", \"body\": \"Здравствуйте!\\n— раз\"}\n```", Kind::Email);
        assert_eq!(l, Letter { subject: "Тема".into(), body: "Здравствуйте!\n— раз".into() });
        let l = parse_letter("Вот письмо:\n{\"Subject\": \"S\", \"text\": \"B\"}", Kind::Email);
        assert_eq!((l.subject.as_str(), l.body.as_str()), ("S", "B"));
        let l = parse_letter("**Тема:** Сайт belodent.by\n\nЗдравствуйте!\nТекст", Kind::Email);
        assert_eq!((l.subject.as_str(), l.body.as_str()), ("Сайт belodent.by", "Здравствуйте!\nТекст"));
        let l = parse_letter("Subject: Hello\nBody text", Kind::Email);
        assert_eq!((l.subject.as_str(), l.body.as_str()), ("Hello", "Body text"));
        let l = parse_letter("{\"body\": \"Коротко\"}", Kind::Short);
        assert_eq!((l.subject.as_str(), l.body.as_str()), ("", "Коротко"));
        let l = parse_letter("Просто текст {без json}", Kind::Short);
        assert_eq!(l.body, "Просто текст {без json}");
    }

    #[test]
    fn lint_catches_style_breaks() {
        let must = vec!["hello@example.com".to_string()];
        let ok = Letter { subject: "Сайт belodent.by".into(), body: "Здравствуйте! На сайте нашлось несколько проблем:\n— раз\n— два\n\nhello@example.com".into() };
        assert!(lint_letter(&ok, Kind::Email, "ru", &must).is_empty(), "{:?}", lint_letter(&ok, Kind::Email, "ru", &must));
        let bad = Letter { subject: "".into(), body: "Я внимательно посмотрела ваш **сайт**. С уважением, [Ваше имя]. Пишу как нейросеть {{x}}".into() };
        let issues = lint_letter(&bad, Kind::Email, "ru", &must).join(" | ");
        for want in ["Нет темы", "посмотрела", "markdown", "[Ваше имя]", "нейросеть", "{{…}}", "hello@example.com"] {
            assert!(issues.contains(want), "нет замечания «{want}»: {issues}");
        }
        // «сделал» про третье лицо — не замечание; «Нашла» в начале предложения — замечание.
        let third = Letter { subject: "".into(), body: "Сайт сделал прошлый подрядчик, он работает медленно.".into() };
        assert!(lint_letter(&third, Kind::Short, "ru", &[]).is_empty());
        let start = Letter { subject: "".into(), body: "Здравствуйте! Нашла у вас ошибку.".into() };
        assert!(lint_letter(&start, Kind::Short, "ru", &[]).iter().any(|i| i.contains("Нашла")));
        let en = Letter { subject: "".into(), body: "Hello! I took a look at your website and found a few problems that cost you customers.".into() };
        assert!(lint_letter(&en, Kind::Short, "ru", &[]).iter().any(|i| i.contains("не на русском")));
        assert!(lint_letter(&en, Kind::Short, "en", &[]).is_empty());
    }

    #[test]
    fn parses_chat_response_and_errors() {
        let v = json!({
            "id": "gen-1", "model": "google/gemma-4-31b-it:free", "provider": "Google AI Studio",
            "choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "{\"body\":\"x\"}", "reasoning": "думаю…"}}],
            "usage": {"prompt_tokens": 900, "completion_tokens": 120, "completion_tokens_details": {"reasoning_tokens": 40}, "cost": 0.00012}
        });
        let r = parse_chat(&v, 1500).unwrap();
        assert_eq!((r.reasoning.as_deref(), r.prompt_tokens, r.reasoning_tokens, r.cost), (Some("думаю…"), 900, 40, Some(0.00012)));
        assert_eq!(r.provider.as_deref(), Some("Google AI Studio"));
        let details = json!({"choices": [{"message": {"content": "a", "reasoning_details": [{"type": "reasoning.summary", "summary": "кратко"}]}}]});
        assert_eq!(parse_chat(&details, 0).unwrap().reasoning.as_deref(), Some("кратко"));
        let cut = json!({"choices": [{"finish_reason": "length", "message": {"content": ""}}]});
        assert!(parse_chat(&cut, 0).unwrap_err().message.contains("предел токенов"));
        let e = explain_error(429, &json!({"error": {"code": 429, "message": "Rate limit exceeded: free-models-per-day"}}));
        assert!(e.retryable() && e.message.contains("50 в день") && e.message.contains("free-models-per-day"));
        let e = explain_error(502, &json!({"error": {"code": 502, "message": "Provider returned error", "metadata": {"raw": "overloaded", "provider_name": "Chutes"}}}));
        assert!(e.message.contains("overloaded") && e.message.contains("Chutes"));
        assert!(!explain_error(402, &json!({})).retryable());
    }

    #[test]
    fn parses_model_catalog() {
        let v = json!({"data": [
            {"id": "z/paid", "name": "Z Paid", "pricing": {"prompt": "0.000001", "completion": "0.000002"}, "context_length": 1000,
             "architecture": {"output_modalities": ["text"]}, "supported_parameters": ["reasoning"],
             "reasoning": {"mandatory": true, "supported_efforts": ["high", "low"], "default_effort": "high"}},
            {"id": "a/m:free", "name": "A Free", "pricing": {"prompt": "0", "completion": "0"}, "architecture": {"output_modalities": ["text"]}},
            {"id": "music/x", "name": "Music", "pricing": {"prompt": "0", "completion": "0"}, "architecture": {"output_modalities": ["audio"]}},
            {"id": "openrouter/auto", "name": "Auto Router", "pricing": {"prompt": "-1", "completion": "-1"}}
        ]});
        let m = parse_models(&v);
        assert_eq!(m.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), vec!["a/m:free", "openrouter/auto", "z/paid"]);
        assert!(m[0].free && !m[2].free && !m[1].free);
        assert_eq!((m[2].prompt_price, m[2].completion_price, m[1].prompt_price), (Some(1.0), Some(2.0), None));
        let caps = m[2].reasoning.as_ref().unwrap();
        assert!(caps.mandatory && caps.supported_efforts == vec!["high".to_string(), "low".to_string()]);
    }
}
