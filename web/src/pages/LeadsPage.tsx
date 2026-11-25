import { useMemo, useState } from "react";
import { Link, useNavigate } from "react-router";
import { useQuery } from "@tanstack/react-query";
import { api, type Business, type LeadStatus } from "../api";
import { Empty, ScoreBar } from "../components";
import { absUrl, fmtDate, fmtDay, fmtMoney, hostOf, isOverdue, LEAD_STATUS_LABEL, siteOf, todayStr } from "../format";

type Filter = "all" | LeadStatus;

const ORDER: LeadStatus[] = ["new", "contacted", "talking", "proposal", "client", "rejected", "skip"];

/** Лиды из всех сканов, с которыми идёт работа: статус не «новый», заметка, цена или следующий шаг. */
export default function LeadsPage() {
  const nav = useNavigate();
  const q = useQuery({ queryKey: ["leads"], queryFn: api.leads });
  const settings = useQuery({ queryKey: ["settings"], queryFn: api.settings, staleTime: 60_000 });
  const defCur = settings.data?.currency ?? "BYN";
  const [filter, setFilter] = useState<Filter>("all");
  const [due, setDue] = useState(false);
  const [needle, setNeedle] = useState("");
  const list = q.data ?? [];
  const today = todayStr();

  const stats = useMemo(() => {
    const c: Record<string, { n: number; money: Record<string, number> }> = { all: { n: 0, money: {} } };
    const add = (k: string, b: Business) => {
      const e = (c[k] ??= { n: 0, money: {} });
      e.n++;
      if (b.deal_price) {
        const cur = b.deal_currency ?? defCur;
        e.money[cur] = (e.money[cur] ?? 0) + b.deal_price;
      }
    };
    for (const b of list) {
      add("all", b);
      add(b.lead_status, b);
    }
    return c;
  }, [list, defCur]);
  const dueCount = list.filter((b) => b.next_action_at && b.next_action_at <= today).length;

  const shown = useMemo(() => {
    const n = needle.trim().toLowerCase();
    const rows = list.filter(
      (b) =>
        (filter === "all" || b.lead_status === filter) &&
        (!due || (b.next_action_at && b.next_action_at <= today)) &&
        (!n || `${b.display_name ?? ""} ${b.name} ${b.notes ?? ""} ${b.address ?? ""} ${b.scan_name ?? ""} ${b.contact_person ?? ""} ${b.next_action ?? ""}`.toLowerCase().includes(n)),
    );
    const last = (b: Business) => b.last_activity_at ?? b.status_at ?? b.audited_at ?? "";
    // Сначала те, у кого есть дата шага (ближайшая и просроченные — вверху), потом остальные по свежести касания.
    return rows.sort((a, b) => {
      if (a.next_action_at && b.next_action_at) return a.next_action_at.localeCompare(b.next_action_at);
      if (a.next_action_at) return -1;
      if (b.next_action_at) return 1;
      return last(b).localeCompare(last(a));
    });
  }, [list, filter, due, needle, today]);

  const money = (m: Record<string, number>) =>
    Object.entries(m).map(([cur, v]) => fmtMoney(v, cur)).join(" + ");

  return (
    <div className="page">
      <div className="page-head">
        <div>
          <h1>Лиды в работе</h1>
          <p className="sub">Компании из всех сканов, по которым есть этап, заметка, цена или следующий шаг</p>
        </div>
        <a className="btn" href={api.leadsCsvUrl}>Скачать CSV</a>
      </div>

      <div className="row">
        <div className="chips">
          <button className="chip" aria-pressed={filter === "all"} onClick={() => setFilter("all")}>
            Все <span className="count">{stats.all.n}</span>
          </button>
          {ORDER.filter((s) => s !== "new" || (stats.new?.n ?? 0) > 0).map((s) => {
            const e = stats[s];
            return (
              <button key={s} className="chip" aria-pressed={filter === s} onClick={() => setFilter(s)}>
                {LEAD_STATUS_LABEL[s]} <span className="count">{e?.n ?? 0}</span>
                {e && Object.keys(e.money).length > 0 && <span className="count"> · {money(e.money)}</span>}
              </button>
            );
          })}
        </div>
      </div>
      <div className="row">
        <button className="chip" aria-pressed={due} onClick={() => setDue(!due)}>
          Сегодня и просрочено <span className="count">{dueCount}</span>
        </button>
        <span className="spacer" />
        <input className="input" style={{ maxWidth: 260 }} placeholder="Поиск: название, заметки, шаг" value={needle} onChange={(e) => setNeedle(e.target.value)} aria-label="Поиск" />
      </div>

      <div className="card">
        {q.isLoading && <div className="card-pad muted">Загружаю…</div>}
        {q.error && <div className="card-pad st-fail">{q.error.message}</div>}
        {!q.isLoading && list.length === 0 && (
          <Empty title="Пока пусто">Откройте компанию в скане, отметьте сообщение отправленным или поставьте этап, цену или шаг — она появится здесь.</Empty>
        )}
        {list.length > 0 && shown.length === 0 && <Empty title="Под фильтр ничего не попало" />}
        {shown.length > 0 && (
          <div className="table-wrap">
            <table className="table">
              <thead>
                <tr>
                  <th>Компания</th>
                  <th>Статус</th>
                  <th>Следующий шаг</th>
                  <th>Цена</th>
                  <th>Последнее касание</th>
                  <th style={{ width: 130 }}>Балл</th>
                </tr>
              </thead>
              <tbody>
                {shown.map((b) => {
                  const site = siteOf(b);
                  const late = isOverdue(b.next_action_at);
                  return (
                    <tr key={b.id} className="click" onClick={() => nav(`/businesses/${b.id}`)}>
                      <td className="cell-name">
                        <Link to={`/businesses/${b.id}`} onClick={(e) => e.stopPropagation()}>{b.display_name || b.name}</Link>
                        {b.contact_person && <div className="small">{b.contact_person}</div>}
                        {b.has_contacts === false && <div className="small st-warn" title="Найдите контакты вручную и добавьте на странице компании">Контактов нет — написать некуда</div>}
                        <div className="small muted">
                          {[b.phone, b.scan_name].filter(Boolean).join(" · ")}
                          {site && <> · <a href={absUrl(site)} target="_blank" rel="noreferrer" onClick={(e) => e.stopPropagation()}>{hostOf(site)} ↗</a></>}
                        </div>
                        {b.notes && <div className="small faint clamp-2">{b.notes}</div>}
                      </td>
                      <td className="small">{LEAD_STATUS_LABEL[b.lead_status]}</td>
                      <td className="small">
                        {b.next_action_at ? (
                          <span className={late ? "st-fail" : ""}>
                            <b>{fmtDay(b.next_action_at)}</b>{late && " · просрочено"}
                          </span>
                        ) : <span className="faint">—</span>}
                        {b.next_action && <div className={late ? "st-fail" : "muted"}>{b.next_action}</div>}
                      </td>
                      <td className="small num">{b.deal_price ? fmtMoney(b.deal_price, b.deal_currency ?? defCur) : <span className="faint">—</span>}</td>
                      <td className="small muted num">{b.last_activity_at ? fmtDate(b.last_activity_at) : <span className="faint">—</span>}</td>
                      <td><ScoreBar value={b.overall} /></td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}
      </div>
    </div>
  );
}
