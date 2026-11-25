//! SQLite-хранилище: сканы, компании с результатами, настройки.
//! Одно соединение под мьютексом; вызовы — через spawn_blocking.

use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::Context;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const SCHEMA_VERSION: i64 = 2;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS scans (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at      TEXT NOT NULL,
    finished_at     TEXT,
    name            TEXT NOT NULL,
    kind            TEXT NOT NULL DEFAULT 'map',
    params          TEXT NOT NULL,
    status          TEXT NOT NULL,
    stage           TEXT,
    total           INTEGER NOT NULL DEFAULT 0,
    done            INTEGER NOT NULL DEFAULT 0,
    provider_errors TEXT,
    error           TEXT
);
CREATE TABLE IF NOT EXISTS businesses (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    scan_id      INTEGER NOT NULL REFERENCES scans(id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    address      TEXT,
    lat          REAL,
    lon          REAL,
    phone        TEXT,
    website      TEXT,
    category     TEXT,
    rating       REAL,
    reviews      INTEGER,
    data         TEXT NOT NULL,
    audit_status TEXT NOT NULL DEFAULT 'pending',
    lead         TEXT,
    overall      REAL,
    priority     REAL,
    report       TEXT,
    results      TEXT,
    site         TEXT,
    audited_at   TEXT,
    audit_ms     INTEGER,
    error        TEXT
);
CREATE INDEX IF NOT EXISTS idx_businesses_scan ON businesses(scan_id);
CREATE TABLE IF NOT EXISTS activities (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    business_id INTEGER NOT NULL REFERENCES businesses(id) ON DELETE CASCADE,
    at          TEXT NOT NULL,
    kind        TEXT NOT NULL,
    channel     TEXT,
    text        TEXT
);
CREATE INDEX IF NOT EXISTS idx_activities_business ON activities(business_id);
-- Сообщения клиентам: черновики нейросети, потом — отправленные и ответы (direction = 'in').
CREATE TABLE IF NOT EXISTS messages (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    business_id INTEGER NOT NULL REFERENCES businesses(id) ON DELETE CASCADE,
    created_at  TEXT NOT NULL,
    updated_at  TEXT,
    direction   TEXT NOT NULL DEFAULT 'out',
    kind        TEXT NOT NULL,
    status      TEXT NOT NULL DEFAULT 'draft',
    subject     TEXT,
    body        TEXT NOT NULL,
    ai          TEXT,
    sent_at     TEXT
);
CREATE INDEX IF NOT EXISTS idx_messages_business ON messages(business_id);
"#;

/// Колонки, добавленные после первой версии схемы: (имя, определение).
const ADDED_COLUMNS: &[(&str, &str)] = &[
    ("discovery", "TEXT"),
    ("website_override", "TEXT"),
    ("lead_status", "TEXT NOT NULL DEFAULT 'new'"),
    ("notes", "TEXT"),
    ("status_at", "TEXT"),
    ("site_url", "TEXT"),
    // Работа менеджера: цена сделки, следующий шаг, ЛПР, свои контакты, правки КП (JSON).
    ("deal_price", "REAL"),
    ("deal_currency", "TEXT"),
    ("next_action_at", "TEXT"),
    ("next_action", "TEXT"),
    ("contact_person", "TEXT"),
    ("extra_contacts", "TEXT"),
    ("proposal", "TEXT"),
    // Как клиент называет себя и свой адрес — для обращения, КП и отчёта (в карточках OSM белорусское или латинское написание).
    ("display_name", "TEXT"),
    ("display_address", "TEXT"),
];

fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let existing: Vec<String> =
        conn.prepare("PRAGMA table_info(businesses)")?.query_map([], |r| r.get::<_, String>(1))?.collect::<Result<_, _>>()?;
    for (name, def) in ADDED_COLUMNS {
        if !existing.iter().any(|c| c == name) {
            conn.execute_batch(&format!("ALTER TABLE businesses ADD COLUMN {name} {def}"))?;
        }
    }
    conn.execute_batch("CREATE INDEX IF NOT EXISTS idx_businesses_status ON businesses(lead_status)")?;
    Ok(())
}

/// Статусы работы с лидом.
pub const LEAD_STATUSES: &[&str] = &["new", "contacted", "talking", "proposal", "client", "rejected", "skip"];

/// Название статуса для журнала касаний.
pub fn status_label(s: &str) -> &'static str {
    match s {
        "contacted" => "Написали",
        "talking" => "Переговоры",
        "proposal" => "КП отправлено",
        "client" => "Клиент",
        "rejected" => "Отказ",
        "skip" => "Не работаем",
        _ => "Новый",
    }
}

/// Виды касаний, которые добавляет менеджер; `status` и `price` пишет сервер.
pub const ACTIVITY_KINDS: &[&str] = &["note", "message", "reply", "call", "meeting", "proposal"];
pub const ACTIVITY_CHANNELS: &[&str] = &["instagram", "email", "phone", "telegram", "whatsapp", "viber", "vk", "facebook", "visit", "other"];

/// Запись журнала касаний.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Activity {
    pub id: i64,
    pub business_id: i64,
    pub at: String,
    pub kind: String,
    pub channel: Option<String>,
    pub text: Option<String>,
}

/// Сообщение клиенту. `kind`: `email` (тема + письмо) или `short` (мессенджер); `status`: `draft`, `sent`;
/// `ai` — как составлено: профиль, модель, запрос, ход рассуждения, токены, цена, замечания проверки.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: i64,
    pub business_id: i64,
    pub created_at: String,
    pub updated_at: Option<String>,
    pub direction: String,
    pub kind: String,
    pub status: String,
    pub subject: Option<String>,
    pub body: String,
    pub ai: Value,
    pub sent_at: Option<String>,
}

const MESSAGE_COLS: &str = "id,business_id,created_at,updated_at,direction,kind,status,subject,body,ai,sent_at";

fn message_row(r: &rusqlite::Row) -> rusqlite::Result<Message> {
    Ok(Message {
        id: r.get(0)?,
        business_id: r.get(1)?,
        created_at: r.get(2)?,
        updated_at: r.get(3)?,
        direction: r.get(4)?,
        kind: r.get(5)?,
        status: r.get(6)?,
        subject: r.get(7)?,
        body: r.get(8)?,
        ai: r.get::<_, Option<String>>(9)?.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null),
        sent_at: r.get(10)?,
    })
}

/// Правки полей работы с лидом. `None` — не трогать; пустая строка / `Some(None)` — очистить.
#[derive(Debug, Default)]
pub struct MetaPatch {
    pub lead_status: Option<String>,
    pub notes: Option<String>,
    pub website_override: Option<String>,
    pub deal_price: Option<Option<f64>>,
    pub deal_currency: Option<String>,
    pub next_action_at: Option<String>,
    pub next_action: Option<String>,
    pub contact_person: Option<String>,
    pub extra_contacts: Option<Value>,
    pub proposal: Option<Value>,
    pub display_name: Option<String>,
    pub display_address: Option<String>,
}

/// Пересчитанная оценка: id, лид, общий балл, приоритет, отчёт и — если результаты привели
/// к новому каталогу — сами результаты.
pub type ReportRow = (i64, String, Option<f64>, f64, Value, Option<Value>);

#[derive(Clone)]
pub struct Db(Arc<Mutex<Connection>>);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanRow {
    pub id: i64,
    pub created_at: String,
    pub finished_at: Option<String>,
    pub name: String,
    pub kind: String,
    pub params: Value,
    pub status: String,
    pub stage: Option<String>,
    pub total: i64,
    pub done: i64,
    pub provider_errors: Value,
    pub error: Option<String>,
    /// Сводка по классам лидов (для списка сканов).
    #[serde(default)]
    pub leads: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusinessRow {
    pub id: i64,
    pub scan_id: i64,
    pub name: String,
    pub address: Option<String>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub phone: Option<String>,
    pub website: Option<String>,
    pub category: Option<String>,
    pub rating: Option<f64>,
    pub reviews: Option<i64>,
    pub data: Value,
    pub audit_status: String,
    pub lead: Option<String>,
    pub overall: Option<f64>,
    pub priority: Option<f64>,
    pub report: Value,
    #[serde(skip_serializing_if = "Value::is_null")]
    pub results: Value,
    pub site: Value,
    pub audited_at: Option<String>,
    pub audit_ms: Option<i64>,
    pub error: Option<String>,
    /// Поиск сайта в обход карт (scanner_core::discover::Discovery).
    pub discovery: Value,
    /// Сайт, указанный вручную: проверяется вместо найденного.
    pub website_override: Option<String>,
    /// Статус работы с лидом: new, contacted, talking, client, rejected, skip.
    pub lead_status: String,
    pub notes: Option<String>,
    pub status_at: Option<String>,
    /// Адрес, который проверялся (из карточки, найденный или указанный вручную).
    pub site_url: Option<String>,
    /// Цена сделки (итог КП), которую назначил менеджер.
    pub deal_price: Option<f64>,
    /// Валюта цены; пусто — валюта из настроек.
    pub deal_currency: Option<String>,
    /// Дата следующего шага, `YYYY-MM-DD`.
    pub next_action_at: Option<String>,
    pub next_action: Option<String>,
    /// ЛПР: имя, должность.
    pub contact_person: Option<String>,
    /// Контакты, добавленные вручную: `[{kind, value, note?}]`.
    pub extra_contacts: Value,
    /// Сохранённые правки КП (не собранное КП: оно приходит отдельно в `proposal` ответа компании).
    #[serde(skip_serializing_if = "Value::is_null")]
    pub proposal: Value,
    /// Название компании для документов клиенту, заданное вручную; пусто — из карточки.
    pub display_name: Option<String>,
    /// Адрес для документов клиенту, заданный вручную; пусто — из карточки.
    pub display_address: Option<String>,
    /// Название скана — в общем списке лидов.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scan_name: Option<String>,
    /// Время последнего касания — в списке лидов.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<String>,
    /// Есть ли у компании хоть один контакт (телефон, почта, мессенджер, соцсеть) — в списке лидов.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_contacts: Option<bool>,
    /// Виды контактов без повторов (phone, email, instagram…) — значки каналов в таблице скана.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contact_kinds: Option<Vec<String>>,
    /// Первый телефон в международном виде — для шапки отчёта, КП и страницы компании.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_phone: Option<String>,
}

/// Какие компании скана брать.
#[derive(Debug, Clone, Copy)]
pub enum Which {
    All,
    Pending,
    /// Сайт не открылся или не пустил сканер — после смены сети (VPN, прокси).
    Unreachable,
}

impl BusinessRow {
    /// Есть ли у компании контакты: карточки, сайт (если он принадлежит компании) и ручные.
    /// `h02` — значение метрики H02 («foreign»/«unverified» — контакты сайта чужие).
    pub fn contacts_of(&self, country: Option<&str>, h02: Option<&str>) -> Vec<scanner_core::sales::Contact> {
        use scanner_core::mini::SiteState;
        use scanner_core::sales::{contact_list, Contacts, ManualContact};
        let card: scanner_core::Business = serde_json::from_value(self.data.clone()).unwrap_or_default();
        let state: SiteState = serde_json::from_value(self.report["mini"]["state"].clone()).unwrap_or_default();
        let trusted = !matches!(state, SiteState::Stub | SiteState::Dead) && !matches!(h02, Some("foreign" | "unverified"));
        let site: Contacts = if trusted { serde_json::from_value(self.site["contacts"].clone()).unwrap_or_default() } else { Contacts::default() };
        let manual: Vec<ManualContact> = serde_json::from_value(self.extra_contacts.clone()).unwrap_or_default();
        contact_list(&card, &site, &manual, country)
    }

    /// Заполнить `has_contacts`, `contact_kinds` и `client_phone` по списку контактов.
    pub fn set_contact_summary(&mut self, country: Option<&str>, h02: Option<&str>) {
        let list = self.contacts_of(country, h02);
        let mut kinds: Vec<String> = Vec::new();
        for c in &list {
            if !kinds.contains(&c.kind) {
                kinds.push(c.kind.clone());
            }
        }
        self.has_contacts = Some(!list.is_empty());
        self.client_phone = scanner_core::sales::client_phone(&list, country);
        self.contact_kinds = Some(kinds);
    }

    /// Лёгкая строка для таблицы и событий: без исходных данных карточек и полного отчёта.
    /// У большого скана тысячи строк — полные весили бы десятки мегабайт.
    pub fn slim(mut self) -> Self {
        let take = |v: &Value, n: usize| v.as_array().map(|a| a.iter().take(n).cloned().collect::<Vec<_>>()).unwrap_or_default();
        let sources: Vec<Value> =
            self.data["sources"].as_array().into_iter().flatten().map(|p| json!({"provider": p["provider"], "card_url": p["card_url"]})).collect();
        self.data = json!({ "sources": sources });
        if !self.report.is_null() {
            let r = &self.report;
            let m = &r["mini"];
            self.report = json!({
                "lead_reasons": take(&r["lead_reasons"], 3),
                "mini": {"state": m["state"], "headline": m["headline"], "headline_en": m["headline_en"], "problems": m["problems"], "offer": m["offer"]},
            });
        }
        if !self.discovery.is_null() {
            self.discovery = json!({"found": self.discovery["found"], "suggestions": take(&self.discovery["suggestions"], 1)});
        }
        if !self.site.is_null() {
            let x = &self.site;
            self.site = json!({"final_url": x["final_url"], "error": x["error"], "status": x["status"], "via_proxy": x["via_proxy"]});
        }
        self.results = Value::Null;
        self.proposal = Value::Null;
        self
    }
}

fn json_col(s: Option<String>) -> Value {
    s.and_then(|x| serde_json::from_str(&x).ok()).unwrap_or(Value::Null)
}

pub fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

impl Db {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let conn = Connection::open(path).with_context(|| format!("не открыть {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.execute_batch(SCHEMA)?;
        migrate(&conn)?;
        let v: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if v < SCHEMA_VERSION {
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(Self(Arc::new(Mutex::new(conn))))
    }

    #[cfg(test)]
    pub fn in_memory() -> anyhow::Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(SCHEMA)?;
        migrate(&conn)?;
        Ok(Self(Arc::new(Mutex::new(conn))))
    }

    /// Выполнить работу с БД вне async-потока.
    pub async fn call<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Connection) -> rusqlite::Result<T> + Send + 'static,
    ) -> anyhow::Result<T> {
        let db = self.0.clone();
        let r = tokio::task::spawn_blocking(move || {
            let mut conn = db.lock().unwrap_or_else(|e| e.into_inner());
            f(&mut conn)
        })
        .await??;
        Ok(r)
    }

    // ---------- настройки ----------

    pub async fn get_setting(&self, key: &'static str) -> anyhow::Result<Option<String>> {
        self.call(move |c| c.query_row("SELECT value FROM settings WHERE key=?1", [key], |r| r.get(0)).optional()).await
    }

    pub async fn set_setting(&self, key: &'static str, value: String) -> anyhow::Result<()> {
        self.call(move |c| {
            c.execute("INSERT INTO settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![key, value])
                .map(|_| ())
        })
        .await
    }

    // ---------- сканы ----------

    /// Сканы, оставшиеся «в работе» после прошлого запуска, помечаются прерванными.
    pub async fn recover_interrupted(&self) -> anyhow::Result<usize> {
        self.call(|c| {
            c.execute("UPDATE businesses SET audit_status='pending' WHERE audit_status='running'", [])?;
            c.execute("UPDATE scans SET status='interrupted' WHERE status IN ('queued','running')", [])
        })
        .await
    }

    pub async fn insert_scan(&self, name: String, kind: &'static str, params: Value) -> anyhow::Result<i64> {
        self.call(move |c| {
            c.execute(
                "INSERT INTO scans(created_at,name,kind,params,status) VALUES(?1,?2,?3,?4,'queued')",
                params![now(), name, kind, params.to_string()],
            )?;
            Ok(c.last_insert_rowid())
        })
        .await
    }

    pub async fn update_scan(&self, id: i64, status: Option<&'static str>, stage: Option<&'static str>, error: Option<String>) -> anyhow::Result<()> {
        self.call(move |c| {
            if let Some(s) = status {
                let finished = matches!(s, "done" | "failed" | "cancelled").then(now);
                c.execute("UPDATE scans SET status=?2, finished_at=COALESCE(?3, finished_at) WHERE id=?1", params![id, s, finished])?;
                if s == "running" {
                    c.execute("UPDATE scans SET finished_at=NULL WHERE id=?1", [id])?;
                }
            }
            if let Some(st) = stage {
                c.execute("UPDATE scans SET stage=?2 WHERE id=?1", params![id, st])?;
            }
            if let Some(e) = error {
                c.execute("UPDATE scans SET error=?2 WHERE id=?1", params![id, e])?;
            }
            Ok(())
        })
        .await
    }

    pub async fn clear_scan_error(&self, id: i64) -> anyhow::Result<()> {
        self.call(move |c| c.execute("UPDATE scans SET error=NULL WHERE id=?1", [id]).map(|_| ())).await
    }

    pub async fn set_provider_errors(&self, id: i64, errors: Value) -> anyhow::Result<()> {
        self.call(move |c| c.execute("UPDATE scans SET provider_errors=?2 WHERE id=?1", params![id, errors.to_string()]).map(|_| ())).await
    }

    /// Пересчитать прогресс по компаниям.
    pub async fn refresh_progress(&self, id: i64) -> anyhow::Result<()> {
        self.call(move |c| {
            c.execute(
                "UPDATE scans SET total=(SELECT COUNT(*) FROM businesses WHERE scan_id=?1),
                 done=(SELECT COUNT(*) FROM businesses WHERE scan_id=?1 AND audit_status IN ('done','failed')) WHERE id=?1",
                [id],
            )
            .map(|_| ())
        })
        .await
    }

    fn scan_from_row(r: &rusqlite::Row) -> rusqlite::Result<ScanRow> {
        Ok(ScanRow {
            id: r.get(0)?,
            created_at: r.get(1)?,
            finished_at: r.get(2)?,
            name: r.get(3)?,
            kind: r.get(4)?,
            params: json_col(r.get(5)?),
            status: r.get(6)?,
            stage: r.get(7)?,
            total: r.get(8)?,
            done: r.get(9)?,
            provider_errors: json_col(r.get(10)?),
            error: r.get(11)?,
            leads: Value::Null,
        })
    }

    fn lead_counts(c: &Connection, scan_id: i64) -> rusqlite::Result<Value> {
        let mut st = c.prepare("SELECT COALESCE(lead,'pending'), COUNT(*) FROM businesses WHERE scan_id=?1 GROUP BY 1")?;
        let mut m = serde_json::Map::new();
        for row in st.query_map([scan_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
            let (k, n) = row?;
            m.insert(k, Value::from(n));
        }
        Ok(Value::Object(m))
    }

    const SCAN_COLS: &'static str = "id,created_at,finished_at,name,kind,params,status,stage,total,done,provider_errors,error";

    pub async fn get_scan(&self, id: i64) -> anyhow::Result<Option<ScanRow>> {
        self.call(move |c| {
            let row = c
                .query_row(&format!("SELECT {} FROM scans WHERE id=?1", Self::SCAN_COLS), [id], Self::scan_from_row)
                .optional()?;
            match row {
                Some(mut s) => {
                    s.leads = Self::lead_counts(c, id)?;
                    Ok(Some(s))
                }
                None => Ok(None),
            }
        })
        .await
    }

    pub async fn list_scans(&self) -> anyhow::Result<Vec<ScanRow>> {
        self.call(|c| {
            let mut st = c.prepare(&format!("SELECT {} FROM scans ORDER BY id DESC LIMIT 500", Self::SCAN_COLS))?;
            let mut rows: Vec<ScanRow> = st.query_map([], Self::scan_from_row)?.collect::<Result<_, _>>()?;
            for s in &mut rows {
                s.leads = Self::lead_counts(c, s.id)?;
            }
            Ok(rows)
        })
        .await
    }

    pub async fn delete_scan(&self, id: i64) -> anyhow::Result<bool> {
        self.call(move |c| {
            c.execute("DELETE FROM businesses WHERE scan_id=?1", [id])?;
            Ok(c.execute("DELETE FROM scans WHERE id=?1", [id])? > 0)
        })
        .await
    }

    // ---------- компании ----------

    pub async fn insert_businesses(&self, scan_id: i64, list: Vec<scanner_core::Business>) -> anyhow::Result<()> {
        self.call(move |c| {
            let tx = c.transaction()?;
            {
                let mut st = tx.prepare(
                    "INSERT INTO businesses(scan_id,name,address,lat,lon,phone,website,category,rating,reviews,data,audit_status)
                     VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,'pending')",
                )?;
                for b in &list {
                    st.execute(params![
                        scan_id,
                        b.name,
                        b.address,
                        b.lat,
                        b.lon,
                        b.phone,
                        b.website,
                        b.category,
                        b.rating(),
                        b.reviews().map(i64::from),
                        serde_json::to_string(b).unwrap_or_default(),
                    ])?;
                }
            }
            tx.commit()
        })
        .await
    }

    /// (id, Business) компаний скана; `only_pending` — только непроверенные.
    pub async fn scan_businesses(&self, scan_id: i64, only_pending: bool) -> anyhow::Result<Vec<(i64, scanner_core::Business)>> {
        self.scan_businesses_where(scan_id, if only_pending { Which::Pending } else { Which::All }).await
    }

    /// Компании скана по отбору: все, непроверенные или с недоступным сайтом.
    pub async fn scan_businesses_where(&self, scan_id: i64, which: Which) -> anyhow::Result<Vec<(i64, scanner_core::Business)>> {
        self.call(move |c| {
            let sql = match which {
                Which::Pending => "SELECT id,data FROM businesses WHERE scan_id=?1 AND audit_status IN ('pending','running') ORDER BY id",
                Which::All => "SELECT id,data FROM businesses WHERE scan_id=?1 ORDER BY id",
                Which::Unreachable => {
                    // Несуществующий домен от смены сети не оживёт — его не перепроверяем.
                    "SELECT id,data FROM businesses WHERE scan_id=?1 AND (audit_status='failed'
                       OR json_extract(report, '$.mini.state') = 'blocked'
                       OR (json_extract(report, '$.mini.state') = 'dead' AND json_extract(report, '$.mini.headline') NOT LIKE '%не существует%'))
                     ORDER BY id"
                }
            };
            let mut st = c.prepare(sql)?;
            let rows = st
                .query_map([scan_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
                .filter_map(|x| x.ok())
                .filter_map(|(id, d)| serde_json::from_str(&d).ok().map(|b| (id, b)))
                .collect();
            Ok(rows)
        })
        .await
    }

    /// Компании, проверенные с `since`, у которых главная не открылась по таймауту или обрыву
    /// соединения (не «домен не существует» и не отказ сервера).
    pub async fn network_failures(&self, scan_id: i64, since: String) -> anyhow::Result<Vec<i64>> {
        self.call(move |c| {
            let mut st = c.prepare(
                "SELECT b.id FROM businesses b WHERE b.scan_id=?1 AND b.audited_at >= ?2 AND (b.audit_status='failed' OR EXISTS (
                   SELECT 1 FROM json_each(b.results) j WHERE json_extract(j.value,'$.id')='B01' AND (
                     json_extract(j.value,'$.detail') LIKE '%таймаут%' OR json_extract(j.value,'$.detail') LIKE '%не удалось подключиться%'
                     OR json_extract(j.value,'$.detail') LIKE '%соединение оборвано%')))",
            )?;
            st.query_map(params![scan_id, since], |r| r.get(0))?.collect()
        })
        .await
    }

    /// Пометить выбранные компании скана непроверенными — их перепроверит скан в режиме «доделать».
    pub async fn mark_pending(&self, scan_id: i64, ids: Vec<i64>) -> anyhow::Result<usize> {
        self.call(move |c| {
            let tx = c.transaction()?;
            let mut n = 0;
            {
                let mut st = tx.prepare("UPDATE businesses SET audit_status='pending' WHERE scan_id=?1 AND id=?2")?;
                for id in &ids {
                    n += st.execute(params![scan_id, id])?;
                }
            }
            tx.commit()?;
            Ok(n)
        })
        .await
    }

    pub async fn set_audit_status(&self, id: i64, status: &'static str) -> anyhow::Result<()> {
        self.call(move |c| c.execute("UPDATE businesses SET audit_status=?2 WHERE id=?1", params![id, status]).map(|_| ())).await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn save_audit(
        &self,
        id: i64,
        lead: String,
        overall: Option<f64>,
        priority: f64,
        report: Value,
        results: Value,
        site: Value,
        ms: i64,
        discovery: Value,
        site_url: Option<String>,
    ) -> anyhow::Result<()> {
        self.call(move |c| {
            c.execute(
                "UPDATE businesses SET audit_status='done', lead=?2, overall=?3, priority=?4, report=?5, results=?6, site=?7,
                 audited_at=?8, audit_ms=?9, error=NULL, discovery=?10, site_url=?11 WHERE id=?1",
                params![id, lead, overall, priority, report.to_string(), results.to_string(), site.to_string(), now(), ms, discovery.to_string(), site_url],
            )
            .map(|_| ())
        })
        .await
    }

    /// Проверенные компании для пересчёта оценки без сети: (id, данные, результаты, поиск сайта).
    /// `scan_id = None` — все сканы; `only_missing` — только без мини-отчёта (старые проверки).
    /// Следующая пачка для пересчёта: записи с id больше `after_id`, не больше `limit` штук.
    /// Пачками, а не все сразу: 11 тыс. разобранных результатов занимали 4 ГБ памяти.
    pub async fn rows_to_rescore(&self, scan_id: Option<i64>, only_missing: bool, after_id: i64, limit: i64) -> anyhow::Result<Vec<(i64, Value, Value, Value)>> {
        self.call(move |c| {
            let mut st = c.prepare(
                "SELECT id, data, results, discovery FROM businesses
                 WHERE audit_status<>'running' AND results IS NOT NULL AND (?1 IS NULL OR scan_id=?1)
                   AND (?2 = 0 OR json_extract(report, '$.mini') IS NULL) AND id > ?3
                 ORDER BY id LIMIT ?4",
            )?;
            st.query_map(params![scan_id, only_missing as i64, after_id, limit], |r| Ok((r.get(0)?, json_col(r.get(1)?), json_col(r.get(2)?), json_col(r.get(3)?))))?
                .collect()
        })
        .await
    }

    /// Записать пересчитанные оценки одним махом.
    /// `results` — если результаты приведены к новому каталогу, их тоже записать.
    pub async fn update_reports(&self, rows: Vec<ReportRow>) -> anyhow::Result<()> {
        self.call(move |c| {
            let tx = c.transaction()?;
            {
                let mut st = tx.prepare("UPDATE businesses SET lead=?2, overall=?3, priority=?4, report=?5 WHERE id=?1")?;
                let mut res = tx.prepare("UPDATE businesses SET results=?2 WHERE id=?1")?;
                for (id, lead, overall, priority, report, results) in &rows {
                    st.execute(params![id, lead, overall, priority, report.to_string()])?;
                    if let Some(r) = results {
                        res.execute(params![id, r.to_string()])?;
                    }
                }
            }
            tx.commit()
        })
        .await
    }

    pub async fn fail_audit(&self, id: i64, error: String) -> anyhow::Result<()> {
        self.call(move |c| c.execute("UPDATE businesses SET audit_status='failed', error=?2 WHERE id=?1", params![id, error]).map(|_| ())).await
    }

    fn business_from_row(r: &rusqlite::Row, with_results: bool) -> rusqlite::Result<BusinessRow> {
        Ok(BusinessRow {
            id: r.get(0)?,
            scan_id: r.get(1)?,
            name: r.get(2)?,
            address: r.get(3)?,
            lat: r.get(4)?,
            lon: r.get(5)?,
            phone: r.get(6)?,
            website: r.get(7)?,
            category: r.get(8)?,
            rating: r.get(9)?,
            reviews: r.get(10)?,
            data: json_col(r.get(11)?),
            audit_status: r.get(12)?,
            lead: r.get(13)?,
            overall: r.get(14)?,
            priority: r.get(15)?,
            report: json_col(r.get(16)?),
            results: if with_results { json_col(r.get(17)?) } else { Value::Null },
            site: json_col(r.get(18)?),
            audited_at: r.get(19)?,
            audit_ms: r.get(20)?,
            error: r.get(21)?,
            discovery: json_col(r.get(22)?),
            website_override: r.get(23)?,
            lead_status: r.get::<_, Option<String>>(24)?.unwrap_or_else(|| "new".into()),
            notes: r.get(25)?,
            status_at: r.get(26)?,
            site_url: r.get(27)?,
            deal_price: r.get(28)?,
            deal_currency: r.get(29)?,
            next_action_at: r.get(30)?,
            next_action: r.get(31)?,
            contact_person: r.get(32)?,
            extra_contacts: json_col(r.get(33)?),
            proposal: json_col(r.get(34)?),
            display_name: r.get(35)?,
            display_address: r.get(36)?,
            scan_name: r.get::<_, Option<String>>(37).ok().flatten(),
            last_activity_at: r.get::<_, Option<String>>(38).ok().flatten(),
            has_contacts: None,
            contact_kinds: None,
            client_phone: None,
        })
    }

    const BIZ_COLS: &'static str = "id,scan_id,name,address,lat,lon,phone,website,category,rating,reviews,data,audit_status,lead,overall,priority,report,results,site,audited_at,audit_ms,error,discovery,website_override,lead_status,notes,status_at,site_url,deal_price,deal_currency,next_action_at,next_action,contact_person,extra_contacts,proposal,display_name,display_address";

    /// Компании скана без результатов метрик; `slim` — ещё и без тяжёлых JSON (для таблицы).
    pub async fn list_businesses(&self, scan_id: i64, slim: bool) -> anyhow::Result<Vec<BusinessRow>> {
        self.call(move |c| {
            let mut st = c.prepare(&format!(
                "SELECT {} FROM businesses WHERE scan_id=?1 ORDER BY COALESCE(priority,-1) DESC, name",
                Self::BIZ_COLS.replace("results", "NULL")
            ))?;
            let mut rows = st.query_map([scan_id], |r| Self::business_from_row(r, false))?.collect::<rusqlite::Result<Vec<_>>>()?;
            if slim {
                // Контакты для таблицы скана — как в «Лидах»: карточка, сайт (если он компании) и ручные.
                let country: Option<String> = c
                    .query_row("SELECT params FROM scans WHERE id=?1", [scan_id], |r| r.get::<_, Option<String>>(0))
                    .optional()?
                    .flatten()
                    .and_then(|p| serde_json::from_str::<Value>(&p).ok())
                    .and_then(|p| p["country"].as_str().map(str::to_string));
                // H02 нужен, только если у сайта есть сохранённые контакты: не разбирать результаты всех компаний.
                let mut h02q = c.prepare(
                    "SELECT json_extract(j.value, '$.value') FROM businesses b, json_each(b.results) j
                     WHERE b.id=?1 AND json_extract(j.value, '$.id')='H02' LIMIT 1",
                )?;
                for row in &mut rows {
                    let has_site_contacts = row.site["contacts"].as_object().is_some_and(|o| !o.is_empty());
                    let h02: Option<String> = if has_site_contacts { h02q.query_row([row.id], |r| r.get(0)).optional()?.flatten() } else { None };
                    row.set_contact_summary(country.as_deref(), h02.as_deref());
                }
            }
            Ok(rows.into_iter().map(|b| if slim { b.slim() } else { b }).collect())
        })
        .await
    }

    /// Для сводки по скану: статус проверки, лид, балл и мини-отчёт каждой компании.
    pub async fn scan_minis(&self, scan_id: i64) -> anyhow::Result<Vec<(String, Option<String>, Option<f64>, Value)>> {
        self.call(move |c| {
            let mut st = c.prepare("SELECT audit_status, lead, overall, json_extract(report, '$.mini') FROM businesses WHERE scan_id=?1")?;
            st.query_map([scan_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, json_col(r.get(3)?))))?.collect()
        })
        .await
    }

    pub async fn get_business(&self, id: i64) -> anyhow::Result<Option<BusinessRow>> {
        self.call(move |c| {
            c.query_row(&format!("SELECT {} FROM businesses WHERE id=?1", Self::BIZ_COLS), [id], |r| Self::business_from_row(r, true))
                .optional()
        })
        .await
    }

    /// Статус лида, заметки, сайт вручную. `website_override`: Some("") — убрать.
    /// Перезаписать исходные данные компании (например, уточнённый размер сети).
    /// Компании скана с сайтом, который сканер в итоге проверял (из карточки или найденный).
    pub async fn scan_site_urls(&self, scan_id: i64) -> anyhow::Result<Vec<(i64, Value, String)>> {
        self.call(move |c| {
            let mut st = c.prepare("SELECT id, data, site_url FROM businesses WHERE scan_id=?1 AND site_url IS NOT NULL")?;
            st.query_map([scan_id], |r| Ok((r.get(0)?, json_col(r.get(1)?), r.get(2)?)))?.collect()
        })
        .await
    }

    pub async fn update_business_data(&self, id: i64, data: Value) -> anyhow::Result<()> {
        self.call(move |c| c.execute("UPDATE businesses SET data=?2 WHERE id=?1", params![id, data.to_string()]).map(|_| ())).await
    }

    pub async fn update_business_meta(&self, id: i64, p: MetaPatch) -> anyhow::Result<bool> {
        self.call(move |c| {
            let mut n = 0;
            let blank = |t: String| (!t.trim().is_empty()).then(|| t.trim().to_string());
            if let Some(st) = p.lead_status {
                n += c.execute("UPDATE businesses SET lead_status=?2, status_at=?3 WHERE id=?1", params![id, st, now()])?;
            }
            if let Some(t) = p.notes {
                let t = (!t.trim().is_empty()).then_some(t);
                n += c.execute("UPDATE businesses SET notes=?2 WHERE id=?1", params![id, t])?;
            }
            if let Some(w) = p.website_override {
                n += c.execute("UPDATE businesses SET website_override=?2 WHERE id=?1", params![id, blank(w)])?;
            }
            if let Some(v) = p.deal_price {
                n += c.execute("UPDATE businesses SET deal_price=?2 WHERE id=?1", params![id, v])?;
            }
            if let Some(v) = p.deal_currency {
                n += c.execute("UPDATE businesses SET deal_currency=?2 WHERE id=?1", params![id, blank(v)])?;
            }
            if let Some(v) = p.next_action_at {
                n += c.execute("UPDATE businesses SET next_action_at=?2 WHERE id=?1", params![id, blank(v)])?;
            }
            if let Some(v) = p.next_action {
                n += c.execute("UPDATE businesses SET next_action=?2 WHERE id=?1", params![id, blank(v)])?;
            }
            if let Some(v) = p.contact_person {
                n += c.execute("UPDATE businesses SET contact_person=?2 WHERE id=?1", params![id, blank(v)])?;
            }
            if let Some(v) = p.extra_contacts {
                let v = (!v.is_null()).then(|| v.to_string());
                n += c.execute("UPDATE businesses SET extra_contacts=?2 WHERE id=?1", params![id, v])?;
            }
            if let Some(v) = p.proposal {
                let v = (!v.is_null()).then(|| v.to_string());
                n += c.execute("UPDATE businesses SET proposal=?2 WHERE id=?1", params![id, v])?;
            }
            if let Some(v) = p.display_name {
                n += c.execute("UPDATE businesses SET display_name=?2 WHERE id=?1", params![id, blank(v)])?;
            }
            if let Some(v) = p.display_address {
                n += c.execute("UPDATE businesses SET display_address=?2 WHERE id=?1", params![id, blank(v)])?;
            }
            Ok(n > 0)
        })
        .await
    }

    // ---------- журнал касаний ----------

    pub async fn list_activities(&self, business_id: i64) -> anyhow::Result<Vec<Activity>> {
        self.call(move |c| {
            let mut st = c.prepare("SELECT id,business_id,at,kind,channel,text FROM activities WHERE business_id=?1 ORDER BY at DESC, id DESC")?;
            st.query_map([business_id], |r| Ok(Activity { id: r.get(0)?, business_id: r.get(1)?, at: r.get(2)?, kind: r.get(3)?, channel: r.get(4)?, text: r.get(5)? }))?
                .collect()
        })
        .await
    }

    pub async fn add_activity(&self, business_id: i64, kind: String, channel: Option<String>, text: Option<String>, at: Option<String>) -> anyhow::Result<Activity> {
        self.call(move |c| {
            let at = at.unwrap_or_else(now);
            c.execute("INSERT INTO activities(business_id,at,kind,channel,text) VALUES(?1,?2,?3,?4,?5)", params![business_id, at, kind, channel, text])?;
            Ok(Activity { id: c.last_insert_rowid(), business_id, at, kind, channel, text })
        })
        .await
    }

    /// Удалить запись журнала; возвращает компанию, которой она принадлежала.
    pub async fn delete_activity(&self, id: i64) -> anyhow::Result<Option<i64>> {
        self.call(move |c| {
            let bid: Option<i64> = c.query_row("SELECT business_id FROM activities WHERE id=?1", [id], |r| r.get(0)).optional()?;
            if bid.is_some() {
                c.execute("DELETE FROM activities WHERE id=?1", [id])?;
            }
            Ok(bid)
        })
        .await
    }

    // ---------- сообщения ----------

    pub async fn insert_message(&self, business_id: i64, kind: &'static str, subject: Option<String>, body: String, ai: Value) -> anyhow::Result<Message> {
        self.call(move |c| {
            let at = now();
            c.execute(
                "INSERT INTO messages(business_id,created_at,direction,kind,status,subject,body,ai) VALUES(?1,?2,'out',?3,'draft',?4,?5,?6)",
                params![business_id, at, kind, subject, body, ai.to_string()],
            )?;
            let id = c.last_insert_rowid();
            c.query_row(&format!("SELECT {MESSAGE_COLS} FROM messages WHERE id=?1"), [id], message_row)
        })
        .await
    }

    pub async fn list_messages(&self, business_id: i64) -> anyhow::Result<Vec<Message>> {
        self.call(move |c| {
            let mut st = c.prepare(&format!("SELECT {MESSAGE_COLS} FROM messages WHERE business_id=?1 ORDER BY created_at DESC, id DESC"))?;
            st.query_map([business_id], message_row)?.collect()
        })
        .await
    }

    pub async fn get_message(&self, id: i64) -> anyhow::Result<Option<Message>> {
        self.call(move |c| c.query_row(&format!("SELECT {MESSAGE_COLS} FROM messages WHERE id=?1"), [id], message_row).optional()).await
    }

    /// Правка текста или статуса; `sent` ставит и время отправки. None — сообщения нет.
    pub async fn update_message(&self, id: i64, subject: Option<String>, body: Option<String>, status: Option<&'static str>) -> anyhow::Result<Option<Message>> {
        self.call(move |c| {
            let at = now();
            let sent_at = (status == Some("sent")).then(|| at.clone());
            let n = c.execute(
                "UPDATE messages SET subject=COALESCE(?2,subject), body=COALESCE(?3,body), status=COALESCE(?4,status),
                        sent_at=COALESCE(?5,sent_at), updated_at=?6 WHERE id=?1",
                params![id, subject, body, status, sent_at, at],
            )?;
            if n == 0 {
                return Ok(None);
            }
            c.query_row(&format!("SELECT {MESSAGE_COLS} FROM messages WHERE id=?1"), [id], message_row).optional()
        })
        .await
    }

    /// Удалить сообщение; возвращает компанию, которой оно принадлежало.
    pub async fn delete_message(&self, id: i64) -> anyhow::Result<Option<i64>> {
        self.call(move |c| {
            let bid: Option<i64> = c.query_row("SELECT business_id FROM messages WHERE id=?1", [id], |r| r.get(0)).optional()?;
            if bid.is_some() {
                c.execute("DELETE FROM messages WHERE id=?1", [id])?;
            }
            Ok(bid)
        })
        .await
    }

    /// Компании из списка, у которых уже есть исходящее сообщение этого вида (черновик или отправленное).
    pub async fn with_messages(&self, kind: &'static str, ids: Vec<i64>) -> anyhow::Result<std::collections::HashSet<i64>> {
        self.call(move |c| {
            let mut st = c.prepare("SELECT 1 FROM messages WHERE business_id=?1 AND kind=?2 AND direction='out' LIMIT 1")?;
            let mut out = std::collections::HashSet::new();
            for id in ids {
                if st.exists(params![id, kind])? {
                    out.insert(id);
                }
            }
            Ok(out)
        })
        .await
    }

    /// Лиды из всех сканов: с которыми идёт работа (статус не «новый»), с заметками, ценой,
    /// следующим шагом или касаниями.
    pub async fn list_leads(&self) -> anyhow::Result<Vec<BusinessRow>> {
        self.call(|c| {
            let cols = Self::BIZ_COLS.replace("results", "NULL").split(',').map(|x| format!("b.{x}")).collect::<Vec<_>>().join(",").replace("b.NULL", "NULL");
            let mut st = c.prepare(&format!(
                "SELECT {cols}, s.name, (SELECT MAX(a.at) FROM activities a WHERE a.business_id=b.id) AS last_at, s.params,
                        (SELECT json_extract(j.value, '$.value') FROM json_each(b.results) j WHERE json_extract(j.value, '$.id')='H02' LIMIT 1) AS h02
                 FROM businesses b JOIN scans s ON s.id=b.scan_id
                 WHERE b.lead_status <> 'new' OR (b.notes IS NOT NULL AND b.notes <> '')
                    OR b.deal_price IS NOT NULL OR (b.next_action_at IS NOT NULL AND b.next_action_at <> '')
                    OR (b.next_action IS NOT NULL AND b.next_action <> '')
                    OR EXISTS (SELECT 1 FROM activities a WHERE a.business_id=b.id)
                 ORDER BY MAX(COALESCE(last_at, ''), COALESCE(b.status_at, ''), COALESCE(b.audited_at, '')) DESC LIMIT 2000"
            ))?;
            st.query_map([], |r| {
                let mut row = Self::business_from_row(r, false)?;
                let params: Value = json_col(r.get(39)?);
                let h02: Option<String> = r.get(40)?;
                row.set_contact_summary(params["country"].as_str(), h02.as_deref());
                Ok(row.slim())
            })?
            .collect()
        })
        .await
    }

    /// Дополнить строку сводкой по контактам (для события о компании: у таблицы скана те же значки, что в списке).
    pub async fn with_contact_summary(&self, mut b: BusinessRow) -> anyhow::Result<BusinessRow> {
        self.call(move |c| {
            let country: Option<String> = c
                .query_row("SELECT params FROM scans WHERE id=?1", [b.scan_id], |r| r.get::<_, Option<String>>(0))
                .optional()?
                .flatten()
                .and_then(|p| serde_json::from_str::<Value>(&p).ok())
                .and_then(|p| p["country"].as_str().map(str::to_string));
            let h02: Option<String> = c
                .query_row(
                    "SELECT json_extract(j.value, '$.value') FROM businesses b, json_each(b.results) j
                     WHERE b.id=?1 AND json_extract(j.value, '$.id')='H02' LIMIT 1",
                    [b.id],
                    |r| r.get(0),
                )
                .optional()?
                .flatten();
            b.set_contact_summary(country.as_deref(), h02.as_deref());
            Ok(b)
        })
        .await
    }

    pub async fn business_brief(&self, id: i64) -> anyhow::Result<Option<BusinessRow>> {
        self.call(move |c| {
            c.query_row(&format!("SELECT {} FROM businesses WHERE id=?1", Self::BIZ_COLS.replace("results", "NULL")), [id], |r| {
                Self::business_from_row(r, false)
            })
            .optional()
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn scan_and_business_roundtrip() {
        let db = Db::in_memory().unwrap();
        let id = db.insert_scan("Тест".into(), "map", serde_json::json!({"query": "кафе"})).await.unwrap();
        let b = scanner_core::Business { name: "Кафе".into(), website: Some("https://cafe.ru".into()), ..Default::default() };
        db.insert_businesses(id, vec![b]).await.unwrap();
        db.refresh_progress(id).await.unwrap();
        let s = db.get_scan(id).await.unwrap().unwrap();
        assert_eq!((s.total, s.done, s.status.as_str()), (1, 0, "queued"));
        let list = db.scan_businesses(id, true).await.unwrap();
        assert_eq!(list.len(), 1);
        let bid = list[0].0;
        db.save_audit(bid, "hot".into(), Some(10.0), 320.0, serde_json::json!({}), serde_json::json!([]), serde_json::json!({}), 5, Value::Null, None).await.unwrap();
        assert!(db.update_business_meta(bid, MetaPatch { lead_status: Some("contacted".into()), notes: Some("позвонить в пятницу".into()), website_override: Some("https://x.ru".into()), ..Default::default() }).await.unwrap());
        let row = db.get_business(bid).await.unwrap().unwrap();
        assert_eq!((row.lead_status.as_str(), row.notes.as_deref(), row.website_override.as_deref()), ("contacted", Some("позвонить в пятницу"), Some("https://x.ru")));
        let leads = db.list_leads().await.unwrap();
        assert_eq!(leads.len(), 1);
        assert_eq!(leads[0].scan_name.as_deref(), Some("Тест"));
        // Работа менеджера: цена, шаг, журнал; в лиды попадает и «новый» с ценой.
        db.update_business_meta(bid, MetaPatch { deal_price: Some(Some(450.0)), next_action_at: Some("2026-12-01".into()), contact_person: Some("Анна, директор".into()), extra_contacts: Some(serde_json::json!([{"kind": "phone", "value": "+375 29 1"}])), ..Default::default() }).await.unwrap();
        let a = db.add_activity(bid, "call".into(), Some("phone".into()), Some("не взяли трубку".into()), None).await.unwrap();
        let row = db.get_business(bid).await.unwrap().unwrap();
        assert_eq!((row.deal_price, row.next_action_at.as_deref(), row.contact_person.as_deref()), (Some(450.0), Some("2026-12-01"), Some("Анна, директор")));
        assert_eq!(row.extra_contacts[0]["kind"], "phone");
        let leads = db.list_leads().await.unwrap();
        assert_eq!(leads[0].last_activity_at.as_deref(), Some(a.at.as_str()));
        db.update_business_meta(bid, MetaPatch { deal_price: Some(None), ..Default::default() }).await.unwrap();
        assert_eq!(db.get_business(bid).await.unwrap().unwrap().deal_price, None);
        assert_eq!(db.list_activities(bid).await.unwrap().len(), 1);
        assert_eq!(db.delete_activity(a.id).await.unwrap(), Some(bid));
        assert!(db.list_activities(bid).await.unwrap().is_empty());
        db.refresh_progress(id).await.unwrap();
        let s = db.get_scan(id).await.unwrap().unwrap();
        assert_eq!(s.done, 1);
        assert_eq!(s.leads["hot"], 1);
        assert!(db.delete_scan(id).await.unwrap());
        assert!(db.get_business(bid).await.unwrap().is_none());
    }
}
