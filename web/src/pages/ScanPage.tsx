import { useEffect, useMemo, useState } from "react";
import { Link, useNavigate, useParams } from "react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api, type Business, type Lead, type ScanOverview, type SiteState } from "../api";
import { CardLinks, ConfirmButton, Empty, LeadBadge, Progress, ScanStatus, ScoreBar } from "../components";
import { useScanEvents } from "../hooks";
import { ResultsMap } from "../map";
import { absUrl, countryName, DISCOVERY_SOURCE, fmtDate, fmtRadius, hostOf, LEAD_LABEL, LEAD_STATUS_LABEL, plural, PROVIDER_LABEL, SITE_STATE_LABEL, SITE_STATE_TONE } from "../format";

/** Строк таблицы за раз: у большого скана их тысячи. */
const PAGE = 200;

/** Мини-отчёт по району: что с сайтами и частые проблемы. Плитки — фильтры таблицы. */
function Overview({ o, state, onState, onRecheck, rechecking }: { o: ScanOverview; state: SiteState | null; onState: (s: SiteState | null) => void; onRecheck?: () => void; rechecking: boolean }) {
  const unreachable = o.recheckable ?? (o.states.blocked ?? 0) + (o.states.dead ?? 0);
  const abroad = !!o.egress && !!o.country && o.egress.toLowerCase() !== o.country.toLowerCase();
  const tiles = (Object.keys(SITE_STATE_LABEL) as SiteState[]).filter((k) => (o.states[k] ?? 0) > 0);
  if (tiles.length === 0) return null;
  const sell = (["none", "social", "stub", "dead", "hidden", "weak"] as SiteState[]).reduce((n, k) => n + (o.states[k] ?? 0), 0);
  return (
    <div className="card card-pad stack" style={{ gap: 12 }}>
      <div className="row" style={{ alignItems: "baseline" }}>
        <h2 style={{ margin: 0 }}>Ситуация с сайтами</h2>
        <span className="small muted">
          {o.total} {plural(o.total, "компания", "компании", "компаний")}, проверено {o.audited}
          {o.avg_score != null && ` · средний балл сайта ${o.avg_score}`} · есть что предложить: <b>{sell}</b>
        </span>
      </div>
      <div className="tiles">
        {tiles.map((k) => (
          <button key={k} className={`tile tone-${SITE_STATE_TONE[k]}`} aria-pressed={state === k} onClick={() => onState(state === k ? null : k)}>
            <span className="tile-num">{o.states[k]}</span>
            <span className="tile-label">{SITE_STATE_LABEL[k]}</span>
          </button>
        ))}
      </div>
      {unreachable > 0 && (
        <div className={`banner small ${abroad ? "warn" : ""}`} style={{ alignItems: "center" }}>
          <span style={{ flex: 1 }}>
            {abroad ? (
              <>
                Скан идёт с IP в стране <b>{countryName(o.egress)}</b>, а район — <b>{countryName(o.country)}</b>. Сайты страны часто не
                отвечают на зарубежные адреса или показывают им защиту от ботов: {unreachable} {plural(unreachable, "сайт", "сайта", "сайтов")} не
                открылись или не пустили сканер. Выключите VPN (или пустите домены страны напрямую, или задайте свой прокси в настройках) и
                перепроверьте только их.
              </>
            ) : (
              <>
                {unreachable} {plural(unreachable, "сайт", "сайта", "сайтов")} не открылись или не пустили сканер. После смены сети или прокси
                их можно перепроверить отдельно.
              </>
            )}
          </span>
          {onRecheck && (
            <button className="btn btn-sm" onClick={onRecheck} disabled={rechecking}>
              Перепроверить недоступные
            </button>
          )}
        </div>
      )}
      {o.top_problems.length > 0 && (
        <details>
          <summary className="small">Частые проблемы сайтов ({o.sites_scored} {plural(o.sites_scored, "сайт проверен", "сайта проверено", "сайтов проверено")})</summary>
          <ol className="small problems-top">
            {o.top_problems.map((p) => (
              <li key={p.id}>
                {p.label} — <b>{p.share}%</b> <span className="muted">({p.count})</span>
              </li>
            ))}
          </ol>
        </details>
      )}
    </div>
  );
}

type Filter = "all" | Lead | "pending" | "nosite" | "found" | "work";
type Sort = "priority" | "score" | "reviews" | "name";

const FILTERS: { id: Filter; label: string }[] = [
  { id: "all", label: "Все" },
  { id: "hot", label: LEAD_LABEL.hot },
  { id: "warm", label: LEAD_LABEL.warm },
  { id: "cold", label: LEAD_LABEL.cold },
  { id: "nosite", label: "Без сайта" },
  { id: "found", label: "Сайт найден в обход карт" },
  { id: "work", label: "В работе" },
  { id: "pending", label: "Ждут проверки" },
  { id: "closed", label: LEAD_LABEL.closed },
];

function matches(b: Business, f: Filter): boolean {
  if (f === "all") return true;
  if (f === "pending") return b.audit_status !== "done";
  if (f === "nosite") return !b.website && !b.discovery?.found;
  if (f === "found") return !!b.discovery?.found;
  if (f === "work") return b.lead_status !== "new";
  return b.lead === f;
}

function SiteCell({ b }: { b: Business }) {
  const found = b.discovery?.found;
  const maybe = b.discovery?.suggestions?.[0];
  if (found && (!b.website || found.source === "manual" || b.site_url === found.url)) {
    return (
      <span className="small" style={{ display: "grid" }}>
        <a href={absUrl(found.url)} target="_blank" rel="noreferrer" onClick={(e) => e.stopPropagation()} className="ellipsis" style={{ maxWidth: 220 }}>
          {hostOf(found.url)} ↗
        </a>
        <span className="muted">{b.website ? `в карточке ${hostOf(b.website)}; ` : "не в карточке; "}{DISCOVERY_SOURCE[found.source]}</span>
      </span>
    );
  }
  if (!b.website)
    return (
      <span className="small" style={{ display: "grid" }}>
        <span className="st-fail">нет сайта</span>
        {maybe && <span className="muted">возможно, {hostOf(maybe.url)}</span>}
      </span>
    );
  const reason = b.report?.lead_reasons.find((r) => r.startsWith("Вместо сайта"));
  return (
    <span className="small" style={{ display: "grid" }}>
      <a href={absUrl(b.website)} target="_blank" rel="noreferrer" onClick={(e) => e.stopPropagation()} className="ellipsis" style={{ maxWidth: 220 }}>
        {hostOf(b.website)} ↗
      </a>
      {reason && <span className="st-warn">{reason.replace("Вместо сайта — ", "")}</span>}
    </span>
  );
}

export default function ScanPage() {
  const id = Number(useParams().id);
  const nav = useNavigate();
  const qc = useQueryClient();
  useScanEvents(id);
  const scanQ = useQuery({ queryKey: ["scan", id], queryFn: () => api.scan(id) });
  const listQ = useQuery({ queryKey: ["businesses", id], queryFn: () => api.businesses(id) });
  const [filter, setFilter] = useState<Filter>("all");
  const [sort, setSort] = useState<Sort>("priority");
  const [q, setQ] = useState("");
  const [selected, setSelected] = useState<number | null>(null);
  const [showMap, setShowMap] = useState(true);
  const [siteState, setSiteState] = useState<SiteState | null>(null);
  const [limit, setLimit] = useState(PAGE);
  const overviewQ = useQuery({
    queryKey: ["overview", id],
    queryFn: () => api.overview(id),
    // Пока идёт проверка — обновлять сводку раз в 5 с.
    refetchInterval: () => (scanQ.data?.scan.status === "running" ? 5000 : false),
  });

  const refresh = () => {
    qc.invalidateQueries({ queryKey: ["scan", id] });
    qc.invalidateQueries({ queryKey: ["businesses", id] });
  };
  const cancel = useMutation({ mutationFn: () => api.cancelScan(id), onSettled: refresh });
  // «Останавливаю…» держится до смены статуса; у следующего запуска кнопка снова активна.
  const status = scanQ.data?.scan.status;
  useEffect(() => {
    if (status !== "running" && status !== "queued") cancel.reset();
  }, [status]);
  const reaudit = useMutation({ mutationFn: (pending: boolean) => api.reaudit(id, pending), onSettled: refresh });
  const again = useMutation({ mutationFn: () => api.searchAgain(id), onSettled: refresh });
  const rescore = useMutation({
    mutationFn: () => api.rescore(id),
    onSettled: () => {
      refresh();
      qc.invalidateQueries({ queryKey: ["overview", id] });
    },
  });
  const del = useMutation({ mutationFn: () => api.deleteScan(id), onSuccess: () => nav("/") });
  const recheck = useMutation({ mutationFn: () => api.reauditUnreachable(id), onSettled: refresh });

  const list = listQ.data ?? [];
  const hasReviews = list.some((b) => b.reviews != null);
  const counts = useMemo(() => {
    const c: Record<Filter, number> = { all: list.length, hot: 0, warm: 0, cold: 0, closed: 0, pending: 0, nosite: 0, found: 0, work: 0 };
    for (const b of list) {
      if (b.lead) c[b.lead]++;
      if (b.audit_status !== "done") c.pending++;
      if (!b.website && !b.discovery?.found) c.nosite++;
      if (b.discovery?.found) c.found++;
      if (b.lead_status !== "new") c.work++;
    }
    return c;
  }, [list]);

  const shown = useMemo(() => {
    const needle = q.trim().toLowerCase();
    const items = list.filter(
      (b) =>
        matches(b, filter) &&
        (!siteState || (b.report?.mini?.state ?? "unknown") === siteState) &&
        (!needle || `${b.name} ${b.address ?? ""} ${b.category ?? ""} ${b.website ?? ""} ${b.site_url ?? ""} ${b.notes ?? ""}`.toLowerCase().includes(needle)),
    );
    const by: Record<Sort, (a: Business, b: Business) => number> = {
      priority: (a, b) => (b.priority ?? -1) - (a.priority ?? -1),
      score: (a, b) => (a.overall ?? 101) - (b.overall ?? 101),
      reviews: (a, b) => (b.reviews ?? -1) - (a.reviews ?? -1),
      name: (a, b) => a.name.localeCompare(b.name, "ru"),
    };
    return items.sort(by[sort]);
  }, [list, filter, sort, q, siteState]);
  useEffect(() => setLimit(PAGE), [filter, sort, q, siteState]);

  if (scanQ.error) return <div className="page"><div className="banner error">{scanQ.error.message}</div></div>;
  const scan = scanQ.data?.scan;
  if (!scan) return <div className="page muted">Загружаю…</div>;
  const running = scan.status === "running" || scan.status === "queued";
  const p = scan.params;
  const failed = scan.status === "failed";
  const providerErrors = scan.provider_errors ?? [];
  // Сбой поиска мест уже расписан по источникам ниже — общий текст ошибки не дублируем.
  const scanError = failed && providerErrors.length > 0 ? "Источники мест не ответили — подробности ниже." : scan.error;
  const searchAgainBtn = (primary: boolean) => (
    <button className={primary ? "btn btn-primary" : "btn"} onClick={() => again.mutate()} disabled={again.isPending}
      title="Найти места заново: новые компании добавятся к уже найденным, статусы и заметки сохранятся">
      Повторить поиск
    </button>
  );

  return (
    <div className="page">
      <div>
        <div className="crumbs"><Link to="/">Сканы</Link></div>
        <div className="page-head">
          <div>
            <h1>{scan.name}</h1>
            <p className="sub">
              {scan.kind === "manual"
                ? "Проверка одного сайта"
                : [p.providers.map((x) => PROVIDER_LABEL[x]).join(", "), `радиус ${fmtRadius(p.radius_m)}`, p.city, countryName(p.country), fmtDate(scan.created_at)].filter(Boolean).join(" · ")}
            </p>
          </div>
          <div className="row">
            {running && (
              <button className="btn" onClick={() => cancel.mutate()} disabled={cancel.isPending || cancel.isSuccess}>
                {cancel.isPending || cancel.isSuccess ? "Останавливаю…" : "Остановить"}
              </button>
            )}
            {!running && scan.status === "interrupted" && <button className="btn" onClick={() => reaudit.mutate(true)}>Продолжить</button>}
            {!running && scan.kind === "map" && searchAgainBtn(failed && list.length === 0)}
            {!running && list.length > 0 && <button className="btn" onClick={() => reaudit.mutate(false)} disabled={reaudit.isPending}>Перепроверить всё</button>}
            {!running && list.length > 0 && (
              <button className="btn" onClick={() => rescore.mutate()} disabled={rescore.isPending} title="Пересчитать баллы, лиды и мини-отчёты по уже собранным данным — без новых запросов к сайтам">
                {rescore.isPending ? "Пересчитываю…" : "Пересчитать оценки"}
              </button>
            )}
            <a className="btn" href={api.exportUrl(id)}>Экспорт CSV</a>
            {!running && <ConfirmButton confirm="Точно удалить?" onConfirm={() => del.mutate()}>Удалить</ConfirmButton>}
          </div>
        </div>
      </div>

      {(running || scan.status !== "done") && (
        <div className="card card-pad stack" style={{ gap: 8 }}>
          <div className="row"><ScanStatus scan={scan} /><span className="spacer" /><span className="small muted num">{scan.done} из {scan.total}</span></div>
          <Progress done={scan.done} total={scan.total} active={running} />
          {scanError && <span className="st-fail small">{scanError}</span>}
          {again.error && <span className="st-fail small">{again.error.message}</span>}
        </div>
      )}
      {providerErrors.length > 0 && (
        <div className="banner warn small">
          <span>
            {providerErrors.map((e) => (
              <span key={e.provider} style={{ display: "block" }}>
                <b>{PROVIDER_LABEL[e.provider]}{e.notice ? " (неполный список)" : ""}:</b> {e.error}
              </span>
            ))}
          </span>
        </div>
      )}

      {overviewQ.data && scan.kind === "map" && (
        <Overview o={overviewQ.data} state={siteState} onState={setSiteState} onRecheck={running ? undefined : () => recheck.mutate()} rechecking={recheck.isPending} />
      )}

      <div className="row">
        <div className="chips">
          {FILTERS.filter((f) => f.id === "all" || counts[f.id] > 0 || f.id === filter).map((f) => (
            <button key={f.id} className="chip" aria-pressed={filter === f.id} onClick={() => setFilter(f.id)}>
              {(f.id === "hot" || f.id === "warm" || f.id === "cold" || f.id === "closed") && <span className={`dot lead-${f.id}`} style={{ background: "currentColor" }} />}
              {f.label} <span className="count">{counts[f.id]}</span>
            </button>
          ))}
        </div>
        <span className="spacer" />
        <input className="input" style={{ width: 220 }} placeholder="Поиск по названию, адресу" value={q} onChange={(e) => setQ(e.target.value)} aria-label="Поиск" />
        <select className="select" style={{ width: 215 }} value={sort} onChange={(e) => setSort(e.target.value as Sort)} aria-label="Сортировка">
          <option value="priority">Сначала лучшие лиды</option>
          <option value="score">Сначала худшие сайты</option>
          <option value="reviews">Больше отзывов</option>
          <option value="name">По названию</option>
        </select>
        {scan.kind === "map" && <button className="btn" onClick={() => setShowMap((v) => !v)}>{showMap ? "Скрыть карту" : "Показать карту"}</button>}
      </div>

      <div className={showMap && scan.kind === "map" ? "split" : ""}>
        <div className="card">
          {listQ.isLoading && <div className="card-pad muted">Загружаю…</div>}
          {!listQ.isLoading && list.length === 0 && (
            <Empty
              title={running ? "Ищу компании…" : failed ? "Источники мест не ответили" : "Компаний не найдено"}
              action={!running && scan.kind === "map" ? searchAgainBtn(true) : undefined}
            >
              {running
                ? "Список появится, как только ответят источники. Если серверы OpenStreetMap перегружены, поиск займёт до двух минут."
                : failed
                  ? "Обычно это временная перегрузка серверов OpenStreetMap. Повторите поиск через пару минут или добавьте в настройках источник с ключом."
                  : "Попробуйте другой запрос, больший радиус или добавьте источник с ключом."}
            </Empty>
          )}
          {list.length > 0 && shown.length === 0 && <Empty title="Под фильтр ничего не попало" />}
          {shown.length > 0 && (
            <div className="table-wrap">
              <table className="table">
                <thead>
                  <tr>
                    <th>Компания</th>
                    <th>Сайт</th>
                    <th>Лид</th>
                    <th style={{ width: 140 }}>Балл сайта</th>
                    {hasReviews && <th>Отзывы</th>}
                    <th>Карточки</th>
                  </tr>
                </thead>
                <tbody>
                  {shown.slice(0, limit).map((b) => (
                    <tr
                      key={b.id}
                      className={`click${b.id === selected ? " selected" : ""}`}
                      onClick={() => nav(`/businesses/${b.id}`)}
                      onMouseEnter={() => setSelected(b.id)}
                    >
                      <td className="cell-name">
                        <Link to={`/businesses/${b.id}`} onClick={(e) => e.stopPropagation()}>{b.name}</Link>
                        <div className="small muted">{[b.category, b.address].filter(Boolean).join(" · ")}</div>
                        {b.report?.mini?.headline ? (
                          <>
                            <div className={`small lead-${b.lead}`} style={{ marginTop: 2 }}>{b.report.mini.headline}</div>
                            {b.report.mini.problems.length > 0 && (
                              <div className="small muted mini-problems">{b.report.mini.problems.slice(0, 2).join(" · ")}</div>
                            )}
                          </>
                        ) : (
                          b.report?.lead_reasons[0] && <div className={`small lead-${b.lead}`} style={{ marginTop: 2 }}>{b.report.lead_reasons[0]}</div>
                        )}
                      </td>
                      <td className="cell-site"><SiteCell b={b} /></td>
                      <td>
                        {b.audit_status === "running" ? <span className="small muted">проверяю…</span> : b.audit_status === "failed" ? <span className="small st-fail" title={b.error ?? ""}>ошибка</span> : <LeadBadge lead={b.lead} />}
                        {b.lead_status !== "new" && <div className="small muted" style={{ marginTop: 2 }}>{LEAD_STATUS_LABEL[b.lead_status]}</div>}
                      </td>
                      <td><ScoreBar value={b.overall} /></td>
                      {hasReviews && (
                        <td className="num small">
                          {b.reviews != null ? `${b.reviews}` : <span className="faint">—</span>}
                          {b.rating != null && <span className="muted"> · {b.rating.toFixed(1).replace(".", ",")}★</span>}
                        </td>
                      )}
                      <td><CardLinks sources={b.data.sources} /></td>
                    </tr>
                  ))}
                </tbody>
              </table>
              {shown.length > limit && (
                <div className="card-pad row" style={{ justifyContent: "center" }}>
                  <span className="small muted">Показано {limit} из {shown.length}</span>
                  <button className="btn btn-sm" onClick={() => setLimit((l) => l + PAGE)}>Показать ещё {Math.min(PAGE, shown.length - limit)}</button>
                  <button className="btn btn-sm btn-ghost" onClick={() => setLimit(shown.length)}>Все</button>
                </div>
              )}
            </div>
          )}
        </div>
        {showMap && scan.kind === "map" && (
          <div className="map-pane">
            <ResultsMap items={shown} center={p.center} radius={p.radius_m} selected={selected} onSelect={(bid) => nav(`/businesses/${bid}`)} />
          </div>
        )}
      </div>
    </div>
  );
}
