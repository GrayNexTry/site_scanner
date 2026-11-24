import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { api } from "../api";

const SOURCE: Record<string, string> = { ext: "Внешний скан", access: "Нужен доступ", both: "Грубо снаружи, точно с доступом" };
const STAGE: Record<string, string> = { lead: "Поиск лидов", audit: "Аудит", report: "Отчёт" };
const KIND: Record<string, string> = { check: "Проверка", kpi: "KPI", info: "Справка" };

export default function CatalogPage() {
  const meta = useQuery({ queryKey: ["meta"], queryFn: api.meta, staleTime: 60_000 });
  const [cat, setCat] = useState<string | null>(null);
  const [stage, setStage] = useState<string | null>(null);
  const [onlyStop, setOnlyStop] = useState(false);
  const [onlyReport, setOnlyReport] = useState(false);
  const [q, setQ] = useState("");

  const catalog = meta.data?.catalog;
  const list = useMemo(() => {
    if (!catalog) return [];
    const needle = q.trim().toLowerCase();
    return catalog.metrics.filter(
      (m) =>
        (!cat || m.category === cat) &&
        (!stage || m.stage === stage) &&
        (!onlyStop || m.stop_factor) &&
        (!onlyReport || m.in_report) &&
        (!needle || `${m.id} ${m.name} ${m.method} ${m.rule}`.toLowerCase().includes(needle)),
    );
  }, [catalog, cat, stage, onlyStop, onlyReport, q]);

  if (!catalog) return <div className="page muted">Загружаю…</div>;

  return (
    <div className="page">
      <div className="page-head">
        <div>
          <h1>Метрики</h1>
          <p className="sub">{catalog.metrics.length} показателей: что сканер меряет, чем и где порог. Источник — docs/METRICS.md</p>
        </div>
      </div>
      <div className="stack">
        <div className="chips">
          <button className="chip" aria-pressed={!cat} onClick={() => setCat(null)}>Все категории</button>
          {Object.entries(catalog.categories).map(([k, v]) => (
            <button key={k} className="chip" aria-pressed={cat === k} onClick={() => setCat(cat === k ? null : k)}>
              <span className="mono">{k}</span> {v}
            </button>
          ))}
        </div>
        <div className="row">
          <div className="chips">
            {Object.entries(STAGE).map(([k, v]) => (
              <button key={k} className="chip" aria-pressed={stage === k} onClick={() => setStage(stage === k ? null : k)}>{v}</button>
            ))}
            <button className="chip" aria-pressed={onlyStop} onClick={() => setOnlyStop((x) => !x)}>Стоп-факторы</button>
            <button className="chip" aria-pressed={onlyReport} onClick={() => setOnlyReport((x) => !x)}>В отчёте «до/после»</button>
          </div>
          <span className="spacer" />
          <input className="input" style={{ width: 260 }} placeholder="Поиск: robots, sitemap, LCP…" value={q} onChange={(e) => setQ(e.target.value)} aria-label="Поиск метрик" />
        </div>
      </div>
      <div className="card table-wrap">
        <table className="table">
          <thead>
            <tr>
              <th>ID</th>
              <th>Метрика</th>
              <th>Как мерить</th>
              <th>Норма / флаг</th>
              <th>Тип</th>
            </tr>
          </thead>
          <tbody>
            {list.map((m) => (
              <tr key={m.id}>
                <td className="mono">{m.id}</td>
                <td style={{ minWidth: 220 }}>
                  <b style={{ fontWeight: 550 }}>{m.name}</b>
                  <div className="row small" style={{ gap: 6, marginTop: 4 }}>
                    {m.stop_factor && <span className="badge lead-hot">стоп-фактор</span>}
                    {m.in_report && <span className="badge lead-cold">в отчёте</span>}
                  </div>
                </td>
                <td className="small muted" style={{ minWidth: 200 }}>{m.method}</td>
                <td className="small" style={{ minWidth: 200 }}>{m.rule}</td>
                <td className="small muted" style={{ whiteSpace: "nowrap" }}>
                  {KIND[m.kind]} · {STAGE[m.stage]}
                  <div>{SOURCE[m.source]}{m.weight > 0 && ` · вес ${String(m.weight).replace(".", ",")}`}</div>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}
