import { useEffect, useMemo } from "react";
import { useParams } from "react-router";
import { useQuery } from "@tanstack/react-query";
import { api, type MetricDef, type MetricResult } from "../api";
import { hostOf, scoreTone, siteOf } from "../format";
import { CAT_NAMES, NO_SITE_STATES, WHY, atAddress, atAddressTech, fmtLong, headlineEn, labelOf, legalName, measureOf, okShown, strings, verdict, type Lang } from "../i18n";

type Row = { r: MetricResult; def: MetricDef };

/** Отчёт для клиента: печатается или сохраняется в PDF из браузера. Язык — по стране компании (lang). */
export default function ReportPage() {
  const id = Number(useParams().id);
  const meta = useQuery({ queryKey: ["meta"], queryFn: api.meta, staleTime: 60_000 });
  const q = useQuery({ queryKey: ["business", id], queryFn: () => api.business(id) });
  const lang: Lang = q.data?.lang === "en" ? "en" : "ru";
  const t = strings(lang);

  const rows: Row[] = useMemo(() => {
    const defs = new Map((meta.data?.catalog.metrics ?? []).map((m) => [m.id, m]));
    return (q.data?.business.results ?? []).flatMap((r) => {
      const def = defs.get(r.id);
      // В английских документах Яндекса нет: раздел R не показываем нигде.
      if (lang === "en" && r.id.startsWith("R")) return [];
      return def ? [{ r, def }] : [];
    });
  }, [q.data, meta.data, lang]);

  // Заголовок вкладки — он же имя файла и заголовок PDF (вместо «Сканер карт»).
  const titleBiz = q.data?.business;
  useEffect(() => {
    if (!titleBiz) return;
    const prev = document.title;
    const s = siteOf(titleBiz);
    document.title = `${strings(lang).docTitleReport} ${s ? hostOf(s) : titleBiz.name}`;
    return () => {
      document.title = prev;
    };
  }, [titleBiz, lang]);

  if (q.error) return <div className="report-page"><div className="report">{q.error.message}</div></div>;
  const b = q.data?.business;
  const phone = b ? (b.client_phone ?? b.phone) : null;
  if (!b || !meta.data) return <div className="report-page"><div className="report muted">{t.preparing}</div></div>;

  const m = meta.data;
  const report = b.report;
  const site = siteOf(b);
  const country = q.data?.scan?.params.country;
  const state = report?.mini?.state;
  const noSite = NO_SITE_STATES.includes(state ?? "");
  const defs = new Map(m.catalog.metrics.map((x) => [x.id, x]));
  const label = (r: MetricResult, def?: MetricDef) => labelOf(lang, r, def, m);
  const categories = m.catalog.categories;
  const scored = rows.filter((x) => x.def.kind === "check" && x.def.weight > 0 && ["pass", "warn", "fail"].includes(x.r.status));
  const fails = scored.filter((x) => x.r.status === "fail");
  const warns = scored.filter((x) => x.r.status === "warn");
  const passes = scored.filter((x) => x.r.status === "pass");
  // В «Что в порядке» — только пункты из разделов без провалов.
  const okList = okShown(rows.filter((x) => x.def.kind === "check" && x.def.weight > 0).map((x) => ({ ...x, id: x.r.id, status: x.r.status }))).filter((x) => x.status === "pass");
  const rank = (x: Row) => (x.def.stop_factor ? 100 : 0) + (x.r.status === "fail" ? 10 : 0) + x.def.weight;
  let top: { id: string; text: string; level: "fail" | "warn" }[] = [...fails, ...warns]
    .sort((a, c) => rank(c) - rank(a))
    .slice(0, 10)
    .map(({ r, def }) => ({ id: r.id, text: label(r, def), level: r.status === "fail" ? ("fail" as const) : ("warn" as const) }))
    // Разные метрики могут говорить одно и то же — в «Главном» фраза стоит один раз (первая, самая важная).
    .filter((x, i, all) => all.findIndex((y) => y.text === x.text) === i);
  // Нет разобранных замеров (заглушка, нет сайта) — берём номера проваленных пунктов из мини-отчёта.
  if (top.length === 0 && report?.mini?.fail_ids?.length) {
    top = report.mini.fail_ids
      .filter((fid) => !(lang === "en" && fid.startsWith("R")))
      .map((fid) => ({ id: fid, text: (lang === "en" ? m.labels_en : m.labels)?.[fid] ?? defs.get(fid)?.name ?? fid, level: "fail" as const }))
      .filter((x, i, all) => all.findIndex((y) => y.text === x.text) === i)
      .slice(0, 10);
  }
  const ai = noSite ? [] : okShown(rows.map((x) => ({ ...x, id: x.r.id, status: x.r.status }))).filter((x) => x.r.id.startsWith("I") && ["pass", "warn", "fail"].includes(x.r.status));
  const catRows = Object.entries(categories)
    .map(([letter, name]) => ({ letter, label: CAT_NAMES[lang][letter] ?? name, c: report?.categories[letter] }))
    .filter((x) => x.c && x.c.measured > 0 && (!noSite || x.letter === "A") && !(lang === "en" && x.letter === "R"));
  const tone = scoreTone(report?.overall);
  const headline = lang === "en" ? report?.mini?.headline_en ?? headlineEn(state, report?.overall, fails.length) : report?.mini?.headline;
  const here = state === "stub" || state === "dead" ? atAddress(lang, b.site, t, state) : null;

  // Приложение: все замеры по проблемным пунктам и по ИИ-разделу — ничего не прячем, но и не вставляем в основной текст.
  const tech = rows
    .filter((x) => ["fail", "warn"].includes(x.r.status) || (x.r.id.startsWith("I") && x.r.status === "pass"))
    .map((x) => ({ id: x.r.id, text: label(x.r, x.def), measure: measureOf(lang, x.r), status: x.r.status }))
    .filter((x) => x.measure);
  const techAddr = state === "stub" || state === "dead" ? atAddressTech(b.site) : null;
  if (techAddr) tech.unshift({ id: "_addr", text: t.techAddress, measure: techAddr, status: "info" });

  return (
    <div className="report-page" lang={lang}>
      <div className="report">
        <div className="report-actions no-print">
          <a className="btn btn-primary" href={`/api/businesses/${id}/report.pdf`}>{t.download}</a>
          <button className="btn" onClick={() => window.print()}>{t.print}</button>
          <span className="small muted">{t.reportHint}</span>
        </div>

        <header className="report-head">
          <div className="small muted">{t.reportKind} · {fmtLong(b.audited_at, lang, country)}</div>
          <h1>{q.data?.client_name ?? b.name}</h1>
          {!site || ["none", "social"].includes(state ?? "") ? (
            <>
              {(q.data?.client_address ?? b.address) && <div className="muted">{q.data?.client_address ?? b.address}</div>}
              {phone && <div className="muted">{phone}</div>}
            </>
          ) : (
            <div className="muted">{[site ? hostOf(site) : null, q.data?.client_address ?? b.address, phone].filter(Boolean).join(" · ")}</div>
          )}
        </header>

        <section className="report-score">
          {!noSite && <div className={`report-big st-${tone}`}>{report?.overall ?? "—"}<span>/100</span></div>}
          <div className="stack" style={{ gap: 6 }}>
            <b>{verdict(lang, state, report?.overall, fails.length)}</b>
            {noSite ? (
              <>
                {headline && lang === "ru" && !here && <span className="muted">{headline}</span>}
                {here && <span className="muted">{here}</span>}
              </>
            ) : (
              <span className="muted">
                {t.checked(scored.length)}:{" "}
                <span className="st-fail">{t.critical(fails.length)}</span>,{" "}
                <span className="st-warn">{t.attention(warns.length)}</span>, <span className="st-pass">{t.ok(passes.length)}</span>.
              </span>
            )}
          </div>
        </section>

        {top.length > 0 && (
          <section className="report-section">
            <h2>{t.topTitle}</h2>
            <ol className="report-list">
              {top.map((x) => (
                <li key={x.id}>
                  <div className="row" style={{ gap: 8 }}>
                    <b>{x.text}</b>
                    <span className={`report-tag st-${x.level}`}>{x.level === "fail" ? t.tagFail : t.tagWarn}</span>
                  </div>
                </li>
              ))}
            </ol>
          </section>
        )}

        {catRows.length > 0 && (
          <section className="report-section">
            <h2>{noSite ? t.mapsOnlyTitle : t.catTitle}</h2>
            <table className="report-table">
              <tbody>
                {catRows.map(({ letter, label: name, c }) => (
                  <tr key={letter}>
                    <td>
                      <b>{name}</b>
                      {WHY[lang][letter] && <div className="small muted">{WHY[lang][letter]}</div>}
                    </td>
                    <td className="num" style={{ width: 160 }}>
                      <div className="report-bar"><span className={`st-${scoreTone(c!.score)}`} style={{ width: `${c!.score ?? 0}%` }} /></div>
                      <span className="small">{c!.score != null ? Math.round(c!.score) : "—"} / 100</span>
                    </td>
                    <td className="small num" style={{ width: 120 }}>
                      {c!.fail > 0 && <span className="st-fail">{c!.fail} {t.catFail} </span>}
                      {c!.warn > 0 && <span className="st-warn">{c!.warn} {t.catWarn}</span>}
                      {c!.fail === 0 && c!.warn === 0 && <span className="st-pass">{t.catOk}</span>}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </section>
        )}

        {ai.length > 0 && (
          <section className="report-section">
            <h2>{t.aiTitle}</h2>
            <p className="small muted">{WHY[lang].I}</p>
            <ul className="report-checks">
              {ai.filter((x, i, all) => all.findIndex((y) => label(y.r, y.def) === label(x.r, x.def)) === i).map(({ r, def }) => (
                <li key={r.id}>
                  <span className={`dot st-${r.status}`} /> <b>{label(r, def)}</b>
                </li>
              ))}
            </ul>
          </section>
        )}

        {!noSite && okList.length > 0 && (
          <section className="report-section">
            <h2>{t.okTitle}</h2>
            <ul className="report-pass">
              {okList.filter((x, i, all) => all.findIndex((y) => label(y.r, y.def) === label(x.r, x.def)) === i).map(({ r, def }) => <li key={r.id}>{label(r, def)}</li>)}
            </ul>
          </section>
        )}

        <footer className="report-foot small muted">
          {t.footMeasured(fmtLong(b.audited_at, lang, country))}
          {country ? t.footLegal(legalName(lang, country)) : ""}
          {t.footRest}
        </footer>

        {tech.length > 0 && (
          <section className="report-section report-appendix">
            <h2>{t.techTitle}</h2>
            <p className="small muted">{t.techIntro}</p>
            <table className="report-table report-tech-table">
              <thead><tr><th>{t.techItem}</th><th>{t.techMeasure}</th></tr></thead>
              <tbody>
                {tech.map((x) => (
                  <tr key={x.id}>
                    <td><span className={`dot st-${x.status}`} /> {x.text}</td>
                    <td className="report-tech">{x.measure}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </section>
        )}
      </div>
    </div>
  );
}
