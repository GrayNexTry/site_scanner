import { useMemo, useState } from "react";
import { Link, useNavigate } from "react-router";
import { useQuery } from "@tanstack/react-query";
import { api, type LeadStatus } from "../api";
import { Empty, LeadBadge, ScoreBar } from "../components";
import { absUrl, fmtDate, hostOf, LEAD_STATUS_LABEL, siteOf } from "../format";

type Filter = "all" | LeadStatus;

/** Лиды из всех сканов, с которыми идёт работа: статус не «новый» или есть заметки. */
export default function LeadsPage() {
  const nav = useNavigate();
  const q = useQuery({ queryKey: ["leads"], queryFn: api.leads });
  const [filter, setFilter] = useState<Filter>("all");
  const [needle, setNeedle] = useState("");
  const list = q.data ?? [];

  const counts = useMemo(() => {
    const c: Record<string, number> = { all: list.length };
    for (const b of list) c[b.lead_status] = (c[b.lead_status] ?? 0) + 1;
    return c;
  }, [list]);

  const shown = useMemo(() => {
    const n = needle.trim().toLowerCase();
    return list.filter(
      (b) => (filter === "all" || b.lead_status === filter) && (!n || `${b.name} ${b.notes ?? ""} ${b.address ?? ""} ${b.scan_name ?? ""}`.toLowerCase().includes(n)),
    );
  }, [list, filter, needle]);

  return (
    <div className="page">
      <div className="page-head">
        <div>
          <h1>Лиды в работе</h1>
          <p className="sub">Компании из всех сканов, которым поставлен статус или оставлена заметка</p>
        </div>
      </div>

      <div className="row">
        <div className="chips">
          <button className="chip" aria-pressed={filter === "all"} onClick={() => setFilter("all")}>
            Все <span className="count">{counts.all ?? 0}</span>
          </button>
          {(Object.keys(LEAD_STATUS_LABEL) as LeadStatus[])
            .filter((s) => s !== "new" || (counts.new ?? 0) > 0)
            .map((s) => (
              <button key={s} className="chip" aria-pressed={filter === s} onClick={() => setFilter(s)}>
                {LEAD_STATUS_LABEL[s]} <span className="count">{counts[s] ?? 0}</span>
              </button>
            ))}
        </div>
        <span className="spacer" />
        <input className="input" style={{ width: 240 }} placeholder="Поиск по названию и заметкам" value={needle} onChange={(e) => setNeedle(e.target.value)} aria-label="Поиск" />
      </div>

      <div className="card">
        {q.isLoading && <div className="card-pad muted">Загружаю…</div>}
        {q.error && <div className="card-pad st-fail">{q.error.message}</div>}
        {!q.isLoading && list.length === 0 && (
          <Empty title="Пока пусто">Откройте компанию в скане и поставьте статус «Написали» или оставьте заметку — она появится здесь.</Empty>
        )}
        {list.length > 0 && shown.length === 0 && <Empty title="Под фильтр ничего не попало" />}
        {shown.length > 0 && (
          <div className="table-wrap">
            <table className="table">
              <thead>
                <tr>
                  <th>Компания</th>
                  <th>Статус</th>
                  <th>Заметки</th>
                  <th>Сайт</th>
                  <th>Лид</th>
                  <th style={{ width: 130 }}>Балл</th>
                  <th>Изменён</th>
                </tr>
              </thead>
              <tbody>
                {shown.map((b) => {
                  const site = siteOf(b);
                  return (
                    <tr key={b.id} className="click" onClick={() => nav(`/businesses/${b.id}`)}>
                      <td className="cell-name">
                        <Link to={`/businesses/${b.id}`} onClick={(e) => e.stopPropagation()}>{b.name}</Link>
                        <div className="small muted">{[b.phone, b.address].filter(Boolean).join(" · ")}</div>
                        <div className="small faint">{b.scan_name}</div>
                      </td>
                      <td className="small">{LEAD_STATUS_LABEL[b.lead_status]}</td>
                      <td className="small cell-reason" style={{ whiteSpace: "pre-wrap" }}>{b.notes ?? <span className="faint">—</span>}</td>
                      <td className="small cell-site">
                        {site ? (
                          <a href={absUrl(site)} target="_blank" rel="noreferrer" onClick={(e) => e.stopPropagation()}>{hostOf(site)} ↗</a>
                        ) : (
                          <span className="st-fail">нет сайта</span>
                        )}
                      </td>
                      <td><LeadBadge lead={b.lead} /></td>
                      <td><ScoreBar value={b.overall} /></td>
                      <td className="small muted num">{fmtDate(b.status_at ?? b.audited_at)}</td>
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
