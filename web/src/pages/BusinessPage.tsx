import { useEffect, useMemo, useState } from "react";
import { Link, useLocation, useParams } from "react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api, type Business, type MetricDef, type MetricResult, type Place } from "../api";
import { LeadBadge, ScoreBar, StatusDot, useToast } from "../components";
import { useScanEvents } from "../hooks";
import { ActivityCard, ContactsCard, OutreachCard, ProposalCard, WorkCard } from "./SalesBlocks";
import { LAST_BUSINESS_KEY } from "./AiSettings";
import { absUrl, DISCOVERY_SOURCE, fmtDate, fmtMs, hostOf, PROVIDER_LABEL, scoreTone, siteOf } from "../format";

type View = "problems" | "all" | "na";

function Sources({ sources }: { sources: Place[] }) {
  const real = sources.filter((s) => s.provider !== "manual");
  if (real.length === 0) return null;
  return (
    <div className="card">
      <div className="card-head"><h2>Карточки на картах</h2></div>
      <div className="card-pad grid-3">
        {real.map((s) => (
          <div key={`${s.provider}-${s.provider_id}`} className="stack" style={{ gap: 6 }}>
            <div className="row">
              <b>{PROVIDER_LABEL[s.provider!]}</b>
              {s.card_url && <a className="small" href={s.card_url} target="_blank" rel="noreferrer">открыть ↗</a>}
            </div>
            <dl className="kv">
              <dt>Название</dt><dd>{s.name}</dd>
              {s.category && (<><dt>Категория</dt><dd>{s.category}</dd></>)}
              {s.address && (<><dt>Адрес</dt><dd>{s.address}</dd></>)}
              {s.phone && (<><dt>Телефон</dt><dd>{s.phone}</dd></>)}
              <dt>Сайт</dt><dd>{s.website ? <a href={absUrl(s.website)} target="_blank" rel="noreferrer">{hostOf(s.website)}</a> : <span className="st-fail">нет</span>}</dd>
              {s.rating != null && (<><dt>Рейтинг</dt><dd>{s.rating} · {s.reviews ?? 0} отзывов</dd></>)}
              {s.hours && (<><dt>Часы</dt><dd>{s.hours}</dd></>)}
              {s.business_status && s.business_status !== "OPERATIONAL" && (<><dt>Статус</dt><dd className="st-warn">{s.business_status}</dd></>)}
            </dl>
          </div>
        ))}
      </div>
    </div>
  );
}

/** Мини-отчёт: ситуация с сайтом, что мешает и что предложить — с копированием для письма. */
function MiniCard({ b }: { b: Business }) {
  const [toast, show] = useToast();
  const m = b.report?.mini;
  if (!m || !m.headline) return null;
  const text = [
    b.name,
    m.headline,
    m.problems.length ? `Что мешает:\n${m.problems.map((p) => `— ${p}`).join("\n")}` : "",
    m.offer.length ? `Что можно сделать: ${m.offer.join(", ").toLowerCase()}` : "",
  ]
    .filter(Boolean)
    .join("\n");
  return (
    <div className="card card-pad mini-card">
      <div className="row" style={{ alignItems: "baseline" }}>
        <h2 style={{ margin: 0 }}>Мини-отчёт</h2>
        <span className="spacer" />
        <button
          className="btn btn-sm"
          onClick={() =>
            navigator.clipboard.writeText(text).then(
              () => show("Скопировано"),
              () => show("Не удалось скопировать"),
            )
          }
        >
          Скопировать для письма
        </button>
      </div>
      <p className={`lead-${b.lead}`} style={{ margin: "8px 0 0", fontWeight: 600 }}>{m.headline}</p>
      {m.problems.length > 0 && (
        <>
          <div className="small muted" style={{ marginTop: 10 }}>Что мешает</div>
          <ul className="small">{m.problems.map((p) => <li key={p}>{p}</li>)}</ul>
        </>
      )}
      {m.detail && <p className="small faint" style={{ margin: "10px 0 0" }}>Для менеджера: {m.detail}</p>}
      {m.offer.length > 0 && (
        <>
          <div className="small muted" style={{ marginTop: 10 }}>Что предложить</div>
          <ul className="small">{m.offer.map((p) => <li key={p}>{p}</li>)}</ul>
        </>
      )}
      {toast}
    </div>
  );
}

/** Сайт в обход карт: найденный, возможные варианты и ручной ввод. */
function DiscoveryCard({ b }: { b: Business }) {
  const qc = useQueryClient();
  const [url, setUrl] = useState("");
  const set = useMutation({
    mutationFn: (website_override: string) => api.patchBusiness(b.id, { website_override }),
    onSuccess: () => {
      setUrl("");
      qc.invalidateQueries({ queryKey: ["business", b.id] });
    },
  });
  const d = b.discovery;
  const cardHasOwnSite = b.website && b.site_url === b.website && !b.website_override;
  if (cardHasOwnSite && !d) return null;
  const found = d?.found;
  return (
    <div className="card">
      <div className="card-head"><h2>Сайт в обход карт</h2>{d && d.elapsed_ms > 0 && <span className="small faint">поиск {fmtMs(d.elapsed_ms)}</span>}</div>
      <div className="card-pad stack">
        {found && (
          <div className="stack" style={{ gap: 4 }}>
            <div className="row">
              <b>{found.source === "manual" ? "Сайт указан вручную:" : "Найден сайт:"}</b>
              <a href={absUrl(found.url)} target="_blank" rel="noreferrer">{hostOf(found.url)} ↗</a>
              <span className="small muted">{DISCOVERY_SOURCE[found.source]}</span>
            </div>
            {found.source !== "manual" && <ul className="reasons small">{found.evidence.map((e) => <li key={e}>{e}</li>)}</ul>}
          </div>
        )}
        {d && d.suggestions.length > 0 && (
          <div className="stack" style={{ gap: 6 }}>
            <span className="label">Возможно, это их сайт — проверьте</span>
            {d.suggestions.map((v) => (
              <div key={v.url} className="row" style={{ alignItems: "flex-start" }}>
                <div style={{ flex: 1, minWidth: 0 }}>
                  <a href={absUrl(v.url)} target="_blank" rel="noreferrer">{hostOf(v.url)} ↗</a> <span className="small muted">{v.title ?? ""}</span>
                  <div className="small faint">{DISCOVERY_SOURCE[v.source]} · {v.evidence.join("; ")}</div>
                </div>
                <button className="btn btn-sm" onClick={() => set.mutate(v.url)} disabled={set.isPending}>Это их сайт</button>
              </div>
            ))}
          </div>
        )}
        {d && !found && d.suggestions.length === 0 && (
          <p className="small muted">
            Сайт не найден: проверено доменов по названию — {d.guessed} (существует {d.resolved}), открыто кандидатов — {d.checked}
            {d.search ? `; поиск: ${d.search}` : ""}.
          </p>
        )}
        {!d && !b.website && <p className="small muted">Поиск сайта выключен в настройках или ещё не запускался.</p>}
        <form
          className="row"
          onSubmit={(e) => {
            e.preventDefault();
            if (url.trim()) set.mutate(url.trim());
          }}
        >
          <input className="input" style={{ flex: 1, minWidth: 180 }} placeholder="Знаете сайт? Вставьте адрес — проверю его" value={url} onChange={(e) => setUrl(e.target.value)} aria-label="Сайт вручную" />
          <button className="btn" type="submit" disabled={!url.trim() || set.isPending}>Проверить этот сайт</button>
          {b.website_override && <button className="btn btn-ghost" type="button" onClick={() => set.mutate("")} disabled={set.isPending}>Убрать ручной сайт</button>}
        </form>
        {set.error && <span className="small st-fail">{set.error.message}</span>}
      </div>
    </div>
  );
}

export default function BusinessPage() {
  const id = Number(useParams().id);
  const qc = useQueryClient();
  const meta = useQuery({ queryKey: ["meta"], queryFn: api.meta, staleTime: 60_000 });
  const q = useQuery({ queryKey: ["business", id], queryFn: () => api.business(id) });
  useScanEvents(q.data?.business.scan_id ?? null);
  const audit = useMutation({ mutationFn: () => api.auditBusiness(id), onSettled: () => qc.invalidateQueries({ queryKey: ["business", id] }) });
  const [view, setView] = useState<View>("problems");
  const [cat, setCat] = useState<string | null>(null);

  const defs = useMemo(() => new Map((meta.data?.catalog.metrics ?? []).map((m) => [m.id, m])), [meta.data]);
  const b = q.data?.business;
  // Ссылка из списка скана («Написать») ведёт к блоку первого сообщения.
  const { hash } = useLocation();
  const loaded = !!b;
  useEffect(() => {
    if (hash === "#outreach" && loaded) document.getElementById("outreach")?.scrollIntoView({ block: "start" });
  }, [hash, loaded]);
  // Последняя открытая компания — для пробы профиля нейросети в настройках.
  useEffect(() => {
    if (!b) return;
    try {
      localStorage.setItem(LAST_BUSINESS_KEY, JSON.stringify({ id: b.id, name: q.data?.client_name || b.name }));
    } catch {
      /* хранилище недоступно — проба просто без подсказки */
    }
  }, [b?.id]);
  const results = b?.results ?? [];
  const stops = new Set(b?.report?.stop_factors ?? []);
  // Самые весомые проблемы: сначала стоп-факторы и «проблемы», затем «внимание».
  const topProblems = useMemo(
    () =>
      results
        .filter((r) => r.status === "fail" || r.status === "warn")
        .map((r) => ({ r, def: defs.get(r.id) }))
        .filter(({ def }) => (def?.weight ?? 0) > 0)
        .sort((a, b) => {
          const rank = (x: { r: MetricResult; def?: MetricDef }) => (x.def?.stop_factor ? 10 : 0) + (x.r.status === "fail" ? 5 : 0) + (x.def?.weight ?? 0);
          return rank(b) - rank(a);
        })
        .slice(0, 5),
    [results, defs],
  );

  const grouped = useMemo(() => {
    const keep = (r: MetricResult) =>
      (view === "all" || (view === "problems" ? r.status === "fail" || r.status === "warn" : r.status === "not_measured")) && (!cat || r.id.startsWith(cat));
    const out = new Map<string, { def: MetricDef | undefined; r: MetricResult }[]>();
    for (const r of results.filter(keep)) {
      const c = r.id[0];
      if (!out.has(c)) out.set(c, []);
      out.get(c)!.push({ def: defs.get(r.id), r });
    }
    return out;
  }, [results, view, cat, defs]);

  if (q.error) return <div className="page"><div className="banner error">{q.error.message}</div></div>;
  if (!b || !meta.data) return <div className="page muted">Загружаю…</div>;
  const report = b.report;
  const site = b.site;
  const categories = meta.data.catalog.categories;
  const busy = b.audit_status === "running" || audit.isPending;
  const counts = report?.counts;
  const siteUrl = siteOf(b);

  return (
    <div className="page">
      <div>
        <div className="crumbs">
          <Link to="/">Сканы</Link> / <Link to={`/scans/${b.scan_id}`}>{q.data?.scan?.name ?? "Скан"}</Link>
        </div>
        <div className="page-head">
          <div>
            <h1>{b.name}</h1>
            <p className="sub">{[b.category, b.address, b.client_phone ?? b.phone].filter(Boolean).join(" · ")}</p>
          </div>
          <div className="row">
            {siteUrl ? (
              <a className="btn btn-primary" href={absUrl(site?.final_url ?? siteUrl)} target="_blank" rel="noreferrer">Открыть сайт ↗</a>
            ) : (
              <span className="badge lead-hot">Сайта нет</span>
            )}
            {report && <a className="btn" href={`/report/${b.id}`} target="_blank" rel="noreferrer">Отчёт для клиента</a>}
            <button className="btn" onClick={() => audit.mutate()} disabled={busy}>{busy ? "Проверяю…" : "Перепроверить"}</button>
          </div>
        </div>
      </div>

      {b.audit_status === "failed" && <div className="banner error small">Проверка упала: {b.error}</div>}

      <div className="grid-2" style={{ alignItems: "start" }}>
        <WorkCard key={JSON.stringify([b.lead_status, b.notes, b.next_action, b.next_action_at, b.contact_person, b.deal_price, b.deal_currency, b.status_at, b.display_name, b.display_address, q.data?.name_suggestion])} b={b} clientName={q.data?.client_name} clientAddress={q.data?.client_address} nameSuggestion={q.data?.name_suggestion} />
        <OutreachCard key={`${b.id}-${q.data?.outreach?.short.length ?? 0}-${q.data?.contacts.length ?? 0}`} b={b} outreach={q.data?.outreach ?? null} contacts={q.data?.contacts ?? []} messages={q.data?.messages ?? []} />
      </div>

      <div className="grid-2" style={{ alignItems: "start" }}>
        <ContactsCard b={b} contacts={q.data?.contacts ?? []} />
        <ActivityCard b={b} activities={q.data?.activities ?? []} />
      </div>

      <ProposalCard b={b} proposal={q.data?.proposal ?? null} />

      <div className="grid-3">
        <div className="card card-pad stack">
          <span className="label">Лид</span>
          <div className="row"><LeadBadge lead={b.lead} pending={busy ? "Проверяю" : "Ждёт"} /></div>
          {report && report.lead_reasons.length > 0 && <ul className="reasons">{report.lead_reasons.map((r) => <li key={r}>{r}</li>)}</ul>}
          {!report && <p className="muted small">Проверка ещё не прошла</p>}
          {topProblems.length > 0 && (
            <>
              <span className="label" style={{ marginTop: 4 }}>Что предложить в первую очередь</span>
              <ul className="reasons small">
                {topProblems.map(({ def, r }) => (
                  <li key={r.id} title={def?.name}>
                    <span className={`st-${r.status}`}>{r.detail.length > 110 ? `${r.detail.slice(0, 110)}…` : r.detail}</span> <span className="faint mono">{r.id}</span>
                  </li>
                ))}
              </ul>
            </>
          )}
        </div>
        <div className="card card-pad stack">
          <span className="label">Балл сайта</span>
          <div className="big-score">
            <b className={`st-${scoreTone(report?.overall)}`}>{report?.overall ?? "—"}</b>
            <span className="muted">/ 100</span>
          </div>
          {counts && (
            <p className="small muted num">
              <span className="st-fail">{counts.fail} проблем</span> · <span className="st-warn">{counts.warn} замечаний</span> · <span className="st-pass">{counts.pass} в норме</span> · {counts.not_measured} не измерено
            </p>
          )}
          {stops.size > 0 && <p className="small st-fail">Стоп-факторы: {[...stops].join(", ")} — балл ограничен 25</p>}
        </div>
        <div className="card card-pad stack">
          <span className="label">Сайт</span>
          {site ? (
            <dl className="kv">
              <dt>Адрес</dt><dd>{site.final_url ? <a href={site.final_url} target="_blank" rel="noreferrer">{site.final_url}</a> : site.input_url}</dd>
              {site.error && (<><dt>Ошибка</dt><dd className="st-fail">{site.error}</dd></>)}
              {site.status != null && (<><dt>Ответ</dt><dd>{site.status} · {site.http_version} · TTFB {fmtMs(site.ttfb_ms)}</dd></>)}
              {site.server && (<><dt>Сервер</dt><dd>{site.server}</dd></>)}
              <dt>Обойдено</dt><dd>{site.pages_crawled} стр. · sitemap {site.sitemap_urls} URL</dd>
              {site.domain?.expires && (<><dt>Домен до</dt><dd>{site.domain.expires}</dd></>)}
              {site.tls?.days_left != null && (<><dt>Сертификат</dt><dd>{site.tls.days_left} дн. · {site.tls.protocol}</dd></>)}
              {site.psi?.perf != null && (<><dt>Lighthouse</dt><dd>{site.psi.perf} / 100</dd></>)}
              <dt>Проверено</dt><dd>{fmtDate(b.audited_at)} за {fmtMs(b.audit_ms)}</dd>
            </dl>
          ) : (
            <p className="muted small">{b.website ? "Своего сайта не найдено: в карточке соцсеть или агрегатор" : "Сайта нет ни в карточке, ни в обход карт — это и есть повод для предложения"}</p>
          )}
          {site?.throttled && <p className="small st-warn">Сайт перестал отвечать на частые запросы — часть проверок неполная</p>}
          {site?.via_proxy && (
            <p className="small muted">
              Сайт проверен через свой прокси{site.direct_error ? ` — напрямую: ${site.direct_error}` : ""}. Скорость ответа включает задержку прокси.
            </p>
          )}
          {site?.rendered && <p className="small muted">Сайт рисуется скриптами: проверки сделаны по странице после рендера в браузере</p>}
          {site?.js_shell && !site.rendered && <p className="small st-warn">Сайт рисуется скриптами, а рендер {site.render_error ? `не удался: ${site.render_error}` : "выключен или браузер не найден"} — проверки по тексту не измерены</p>}
        </div>
      </div>

      <MiniCard b={b} />

      <DiscoveryCard b={b} />

      {report && (
        <div className="grid-cats">
          <div className="card">
            <div className="card-head"><h2>Категории</h2>{cat && <button className="btn btn-sm btn-ghost" onClick={() => setCat(null)}>Все</button>}</div>
            <div className="card-pad cat-list">
              {Object.entries(categories).map(([letter, label]) => {
                const c = report.categories[letter];
                return (
                  <button key={letter} className="cat-row" aria-pressed={cat === letter} onClick={() => setCat(cat === letter ? null : letter)}>
                    <span className="letter">{letter}</span>
                    <span className="ellipsis">{label}</span>
                    <ScoreBar value={c?.score ?? null} />
                    <span className="counts">
                      {c && c.fail > 0 && <span className="st-fail">{c.fail}</span>}
                      {c && c.fail > 0 && c.warn > 0 && " · "}
                      {c && c.warn > 0 && <span className="st-warn">{c.warn}</span>}
                    </span>
                  </button>
                );
              })}
            </div>
          </div>

          <div className="card">
            <div className="card-head">
              <h2>{cat ? `${cat}. ${categories[cat]}` : "Метрики"}</h2>
              <div className="chips">
                <button className="chip" aria-pressed={view === "problems"} onClick={() => setView("problems")}>Проблемы</button>
                <button className="chip" aria-pressed={view === "all"} onClick={() => setView("all")}>Все</button>
                <button className="chip" aria-pressed={view === "na"} onClick={() => setView("na")}>Не измерено</button>
              </div>
            </div>
            {grouped.size === 0 && <div className="card-pad muted">{view === "problems" ? "Проблем в этом разделе нет" : "Пусто"}</div>}
            {[...grouped.entries()].map(([letter, rows]) => (
              <div key={letter}>
                {!cat && <div className="group-title"><span className="mono">{letter}</span> {categories[letter]}</div>}
                {rows.map(({ def, r }) => (
                  <div key={r.id} className={`metric${stops.has(r.id) ? " stop" : ""}`}>
                    <StatusDot status={r.status} />
                    <span className="id">{r.id}</span>
                    <div>
                      <div className="name">{def?.name ?? r.id}</div>
                      <div className={`detail small st-${r.status === "fail" || r.status === "warn" ? r.status : "info"}`} style={{ color: r.status === "pass" || r.status === "info" ? "var(--ink)" : undefined }}>
                        {r.detail}
                      </div>
                      {def && <div className="rule">Норма: {def.rule}</div>}
                    </div>
                  </div>
                ))}
              </div>
            ))}
          </div>
        </div>
      )}

      <Sources sources={b.data.sources} />
    </div>
  );
}
