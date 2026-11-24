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
"#;

/// Колонки, добавленные после первой версии схемы: (имя, определение).
const ADDED_COLUMNS: &[(&str, &str)] = &[
    ("discovery", "TEXT"),
    ("website_override", "TEXT"),
    ("lead_status", "TEXT NOT NULL DEFAULT 'new'"),
    ("notes", "TEXT"),
    ("status_at", "TEXT"),
    ("site_url", "TEXT"),
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
pub const LEAD_STATUSES: &[&str] = &["new", "contacted", "talking", "client", "rejected", "skip"];

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
    /// Название скана — в общем списке лидов.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scan_name: Option<String>,
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
                "mini": {"state": m["state"], "headline": m["headline"], "problems": m["problems"], "offer": m["offer"]},
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
            scan_name: r.get::<_, Option<String>>(28).ok().flatten(),
        })
    }

    const BIZ_COLS: &'static str = "id,scan_id,name,address,lat,lon,phone,website,category,rating,reviews,data,audit_status,lead,overall,priority,report,results,site,audited_at,audit_ms,error,discovery,website_override,lead_status,notes,status_at,site_url";

    /// Компании скана без результатов метрик; `slim` — ещё и без тяжёлых JSON (для таблицы).
    pub async fn list_businesses(&self, scan_id: i64, slim: bool) -> anyhow::Result<Vec<BusinessRow>> {
        self.call(move |c| {
            let mut st = c.prepare(&format!(
                "SELECT {} FROM businesses WHERE scan_id=?1 ORDER BY COALESCE(priority,-1) DESC, name",
                Self::BIZ_COLS.replace("results", "NULL")
            ))?;
            st.query_map([scan_id], |r| Self::business_from_row(r, false).map(|b| if slim { b.slim() } else { b }))?.collect()
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

    pub async fn update_business_meta(
        &self,
        id: i64,
        lead_status: Option<String>,
        notes: Option<String>,
        website_override: Option<String>,
    ) -> anyhow::Result<bool> {
        self.call(move |c| {
            let mut n = 0;
            if let Some(st) = lead_status {
                n += c.execute("UPDATE businesses SET lead_status=?2, status_at=?3 WHERE id=?1", params![id, st, now()])?;
            }
            if let Some(t) = notes {
                let t = (!t.trim().is_empty()).then_some(t);
                n += c.execute("UPDATE businesses SET notes=?2 WHERE id=?1", params![id, t])?;
            }
            if let Some(w) = website_override {
                let w = (!w.trim().is_empty()).then(|| w.trim().to_string());
                n += c.execute("UPDATE businesses SET website_override=?2 WHERE id=?1", params![id, w])?;
            }
            Ok(n > 0)
        })
        .await
    }

    /// Лиды из всех сканов, с которыми идёт работа (статус не «новый») или с заметками.
    pub async fn list_leads(&self) -> anyhow::Result<Vec<BusinessRow>> {
        self.call(|c| {
            let cols = Self::BIZ_COLS.replace("results", "NULL").split(',').map(|x| format!("b.{x}")).collect::<Vec<_>>().join(",").replace("b.NULL", "NULL");
            let mut st = c.prepare(&format!(
                "SELECT {cols}, s.name FROM businesses b JOIN scans s ON s.id=b.scan_id
                 WHERE b.lead_status <> 'new' OR (b.notes IS NOT NULL AND b.notes <> '')
                 ORDER BY COALESCE(b.status_at, b.audited_at) DESC LIMIT 2000"
            ))?;
            st.query_map([], |r| Self::business_from_row(r, false).map(BusinessRow::slim))?.collect()
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
        assert!(db.update_business_meta(bid, Some("contacted".into()), Some("позвонить в пятницу".into()), Some("https://x.ru".into())).await.unwrap());
        let row = db.get_business(bid).await.unwrap().unwrap();
        assert_eq!((row.lead_status.as_str(), row.notes.as_deref(), row.website_override.as_deref()), ("contacted", Some("позвонить в пятницу"), Some("https://x.ru")));
        let leads = db.list_leads().await.unwrap();
        assert_eq!(leads.len(), 1);
        assert_eq!(leads[0].scan_name.as_deref(), Some("Тест"));
        db.refresh_progress(id).await.unwrap();
        let s = db.get_scan(id).await.unwrap().unwrap();
        assert_eq!(s.done, 1);
        assert_eq!(s.leads["hot"], 1);
        assert!(db.delete_scan(id).await.unwrap());
        assert!(db.get_business(bid).await.unwrap().is_none());
    }
}
