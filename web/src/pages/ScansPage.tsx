import { useEffect, useState } from "react";
import { Link, useNavigate } from "react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api, type Scan } from "../api";
import { ConfirmButton, Empty, Progress, ScanStatus } from "../components";
import { fmtDate, fmtRadius, PROVIDER_LABEL } from "../format";

function LeadCounts({ scan }: { scan: Scan }) {
  const l = scan.leads;
  const parts: [string, number | undefined, string][] = [
    ["lead-hot", l.hot, "горячих"],
    ["lead-warm", l.warm, "тёплых"],
    ["lead-cold", l.cold, "холодных"],
  ];
  return (
    <span className="row small num" style={{ gap: 12 }}>
      {parts.map(([cls, n, label]) => (
        <span key={label} className={n ? cls : "faint"}>
          <b>{n ?? 0}</b> {label}
        </span>
      ))}
    </span>
  );
}

function scanSummary(s: Scan): string {
  if (s.kind === "manual") return "Проверка одного сайта";
  const p = s.params;
  return [p.providers.map((x) => PROVIDER_LABEL[x]).join(", "), fmtRadius(p.radius_m), p.city].filter(Boolean).join(" · ");
}

export default function ScansPage() {
  const nav = useNavigate();
  const qc = useQueryClient();
  const scans = useQuery({
    queryKey: ["scans"],
    queryFn: api.scans,
    refetchInterval: (q) => (q.state.data?.some((s) => s.status === "running" || s.status === "queued") ? 2000 : false),
  });
  const [url, setUrl] = useState("");
  const audit = useMutation({
    mutationFn: () => api.auditSite(url.trim()),
    onSuccess: (s) => nav(`/scans/${s.id}`),
  });
  const del = useMutation({
    mutationFn: (id: number) => api.deleteScan(id),
    onSettled: () => qc.invalidateQueries({ queryKey: ["scans"] }),
  });
  // Остановка не мгновенная (начатые сайты дорабатываются) — до смены статуса кнопка показывает,
  // что запрос принят.
  const [stopping, setStopping] = useState<Set<number>>(() => new Set());
  const stop = useMutation({
    mutationFn: (id: number) => api.cancelScan(id),
    onMutate: (id) => setStopping((s) => new Set(s).add(id)),
    onSettled: () => qc.invalidateQueries({ queryKey: ["scans"] }),
  });
  useEffect(() => {
    const active = new Set(scans.data?.filter((s) => s.status === "running" || s.status === "queued").map((s) => s.id));
    setStopping((s) => (Array.from(s).every((id) => active.has(id)) ? s : new Set(Array.from(s).filter((id) => active.has(id)))));
  }, [scans.data]);

  return (
    <div className="page">
      <div className="page-head">
        <div>
          <h1>Сканы</h1>
          <p className="sub">Компании в выбранном районе и состояние их сайтов</p>
        </div>
        <Link to="/new" className="btn btn-primary">Новый скан</Link>
      </div>

      <form
        className="card card-pad row"
        onSubmit={(e) => {
          e.preventDefault();
          if (url.trim()) audit.mutate();
        }}
      >
        <label htmlFor="quick-url" className="label" style={{ minWidth: 150 }}>Проверить один сайт</label>
        <input id="quick-url" className="input" style={{ flex: "1 1 260px", width: "auto" }} placeholder="zub-master.ru" value={url} onChange={(e) => setUrl(e.target.value)} />
        <button className="btn" disabled={!url.trim() || audit.isPending}>{audit.isPending ? "Запускаю…" : "Проверить"}</button>
        {audit.error && <span className="st-fail small">{audit.error.message}</span>}
      </form>

      <div className="card">
        {scans.isLoading && <div className="card-pad muted">Загружаю…</div>}
        {scans.error && <div className="card-pad st-fail">{scans.error.message}</div>}
        {scans.data && scans.data.length === 0 && (
          <Empty title="Сканов пока нет" action={<Link to="/new" className="btn btn-primary">Выбрать район</Link>}>
            Отметьте место на карте и категорию бизнеса — сканер найдёт компании и проверит их сайты по 214 метрикам.
          </Empty>
        )}
        {scans.data && scans.data.length > 0 && (
          <div className="table-wrap">
            <table className="table">
              <thead>
                <tr>
                  <th>Скан</th>
                  <th>Статус</th>
                  <th>Компаний</th>
                  <th>Лиды</th>
                  <th>Создан</th>
                  <th />
                </tr>
              </thead>
              <tbody>
                {scans.data.map((s) => {
                  const active = s.status === "running" || s.status === "queued";
                  return (
                    <tr key={s.id} className="click" onClick={() => nav(`/scans/${s.id}`)}>
                      <td className="cell-name">
                        <Link to={`/scans/${s.id}`} onClick={(e) => e.stopPropagation()}>{s.name}</Link>
                        <div className="small muted">{scanSummary(s)}</div>
                      </td>
                      <td style={{ minWidth: 170 }}>
                        <ScanStatus scan={s} />
                        {active && <div style={{ marginTop: 6 }}><Progress done={s.done} total={s.total} active={active} /></div>}
                      </td>
                      <td className="num">{s.total}</td>
                      <td><LeadCounts scan={s} /></td>
                      <td className="small muted num">{fmtDate(s.created_at)}</td>
                      <td onClick={(e) => e.stopPropagation()} style={{ textAlign: "right" }}>
                        {active ? (
                          <button className="btn btn-sm btn-ghost" disabled={stopping.has(s.id)} onClick={() => stop.mutate(s.id)}>
                            {stopping.has(s.id) ? "Останавливаю…" : "Остановить"}
                          </button>
                        ) : (
                          <ConfirmButton className="btn btn-sm btn-ghost btn-danger" confirm="Удалить?" onConfirm={() => del.mutate(s.id)}>
                            Удалить
                          </ConfirmButton>
                        )}
                      </td>
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
