import { useEffect } from "react";
import { useParams } from "react-router";
import { useQuery } from "@tanstack/react-query";
import { api } from "../api";
import { hostOf, scoreTone, siteOf } from "../format";
import { fmtLong, headlineEn, money, strings, type Lang } from "../i18n";

/** Коммерческое предложение для клиента: печатается или сохраняется в PDF. Без технического жаргона. */
export default function ProposalPage() {
  const id = Number(useParams().id);
  const q = useQuery({ queryKey: ["business", id], queryFn: () => api.business(id) });
  const meta = useQuery({ queryKey: ["meta"], queryFn: api.meta, staleTime: 60_000 });
  const lang: Lang = (q.data?.proposal?.lang ?? q.data?.lang) === "en" ? "en" : "ru";
  const t = strings(lang);
  const st = useQuery({ queryKey: ["settings"], queryFn: api.settings, staleTime: 60_000 });

  const titleBiz = q.data?.business;
  useEffect(() => {
    if (!titleBiz) return;
    const prev = document.title;
    const s = siteOf(titleBiz);
    document.title = `${strings(lang).docTitleProposal} ${s ? hostOf(s) : titleBiz.name}`;
    return () => {
      document.title = prev;
    };
  }, [titleBiz, lang]);

  if (q.error) return <div className="report-page"><div className="report">{q.error.message}</div></div>;
  const b = q.data?.business;
  const phone = b ? (b.client_phone ?? b.phone) : null;
  const p = q.data?.proposal;
  if (!b || !st.data && !st.error) return <div className="report-page"><div className="report muted">{t.proposalPreparing}</div></div>;

  const s = st.data;
  const cur = p?.currency ?? "";
  const site = siteOf(b);
  const report = b.report;
  const mini = report?.mini;
  const country = q.data?.scan?.params.country;
  const failIds = mini?.fail_ids ?? [];
  // Главное — по факту результатов (plain); если разобранных результатов нет (заглушка, нет сайта) — из мини-отчёта.
  const byId = new Map((b.results ?? []).map((r) => [r.id, r]));
  const failRows = failIds.map((x) => byId.get(x)).filter((r): r is NonNullable<typeof r> => !!r);
  const fromResults = failRows.map((r) => (lang === "en" ? r.plain_en : r.plain)).filter((x): x is string => !!x);
  const fallback = lang === "en" ? failIds.filter((x) => !x.startsWith("R")).map((x) => meta.data?.labels_en?.[x]).filter((x): x is string => !!x) : mini?.problems ?? [];
  const problems = (fromResults.length > 0 ? fromResults : fallback).filter((x) => lang === "ru" || !/yandex/i.test(x)).filter((x, i, all) => all.indexOf(x) === i).slice(0, 5);
  // Число проверок — как в отчёте: измеримые пункты с весом; в английских документах без раздела R.
  const defs = new Map((meta.data?.catalog.metrics ?? []).map((x) => [x.id, x]));
  const scoredRows = (b.results ?? []).filter((r) => {
    const d = defs.get(r.id);
    return d && d.kind === "check" && d.weight > 0 && ["pass", "warn", "fail"].includes(r.status) && !(lang === "en" && r.id.startsWith("R"));
  });
  const nFails = scoredRows.length > 0 ? scoredRows.filter((r) => r.status === "fail").length : report?.counts.fail ?? failIds.length;
  const nChecks = scoredRows.length > 0 ? scoredRows.length : report ? report.counts.pass + report.counts.warn + report.counts.fail : 0;
  const headline = lang === "en" ? mini?.headline_en ?? headlineEn(mini?.state, report?.overall, nFails) : mini?.headline;
  const fm = (v: number | null | undefined) => money(v, cur, lang);
  const picked = (p?.packages ?? []).filter((x) => x.enabled);
  const until = p ? new Date(Date.now() + p.valid_days * 86_400_000) : null;
  const manager = [s?.manager_name, s?.company_name].filter(Boolean).join(", ");
  const hasContacts = !!(manager || s?.manager_email || s?.manager_phone || s?.manager_telegram);

  return (
    <div className="report-page" lang={lang}>
      <div className="report">
        <div className="report-actions no-print">
          <a className="btn btn-primary" href={`/api/businesses/${id}/proposal.pdf`}>{t.download}</a>
          <button className="btn" onClick={() => window.print()}>{t.print}</button>
          <span className="small muted">{t.proposalHint}</span>
        </div>

        <header className="report-head">
          <div className="small muted">{t.proposalKind} · {fmtLong(new Date(), lang, country)}</div>
          <h1>{q.data?.client_name ?? b.name}</h1>
          {site && !["none", "social"].includes(mini?.state ?? "") ? (
            <div className="muted">{[hostOf(site), q.data?.client_address ?? b.address, phone].filter(Boolean).join(" · ")}</div>
          ) : (
            <>
              {(q.data?.client_address ?? b.address) && <div className="muted">{q.data?.client_address ?? b.address}</div>}
              {phone && <div className="muted">{phone}</div>}
            </>
          )}
        </header>

        <section className="report-section">
          <h2>{t.now}</h2>
          {report?.overall != null ? (
            <div className="report-score">
              <div className={`report-big st-${scoreTone(report.overall)}`}>{Math.round(report.overall)}<span>/100</span></div>
              <div className="stack" style={{ gap: 6 }}>
                <b>{headline ?? t.fallbackHeadline}</b>
                <span className="small muted">{t.scoreNote(nChecks)}</span>
              </div>
            </div>
          ) : (
            <b>{headline ?? t.fallbackHeadlineNone}</b>
          )}
          {problems.length > 0 && (
            <>
              <div className="small muted">{t.mainBlockers}</div>
              <ul className="kp-problems">{problems.map((x) => <li key={x}>{x}</li>)}</ul>
            </>
          )}
        </section>

        {p && picked.length > 0 && (
          <section className="report-section">
            <h2>{t.plan}</h2>
            <div className="kp-list">
              {picked.map((pk) => {
                // «Сайта нет» уже сказано в «Что сейчас»; в плане — решение (summary пакета).
                const items = [...pk.items.filter((i) => i.level === "fail"), ...pk.items.filter((i) => i.level === "warn")]
                  .filter((i) => !(pk.id === "new_site" && i.id === "site"))
                  .filter((i, n, all) => all.findIndex((y) => y.text === i.text) === n)
                  .slice(0, 8);
                return (
                  <div key={pk.id} className="kp-pkg">
                    <div className="kp-row">
                      <b>{pk.title}</b>
                      <b className="num kp-price">{fm(pk.price)}</b>
                    </div>
                    <div className="small muted">{pk.summary}</div>
                    {items.length > 0 && <ul className="small kp-items">{items.map((i) => <li key={i.id}>{i.text}</li>)}</ul>}
                  </div>
                );
              })}
              {p.extra.map((e, i) => (
                <div key={`${e.title}-${i}`} className="kp-pkg">
                  <div className="kp-row">
                    <b>{e.title}</b>
                    <b className="num kp-price">{fm(e.price)}</b>
                  </div>
                </div>
              ))}
            </div>
            <div className="kp-total">
              {p.discount > 0 && (
                <>
                  <div className="kp-row small muted"><span>{t.subtotal}</span><span className="num">{fm(p.subtotal)}</span></div>
                  <div className="kp-row small muted"><span>{t.discount}</span><span className="num">−{fm(p.discount)}</span></div>
                </>
              )}
              <div className="kp-row kp-sum"><span>{t.total}</span><span className="num">{fm(p.total)}</span></div>
            </div>
            {p.note && <p className="small" style={{ whiteSpace: "pre-wrap" }}>{p.note}</p>}
            {until && (
              <p className="small muted">
                {t.validFor(p.valid_days, fmtLong(until, lang, country))}
              </p>
            )}
          </section>
        )}
        {p && picked.length === 0 && <section className="report-section"><h2>{t.plan}</h2><p className="muted">{t.nothingPicked}</p></section>}

        {hasContacts && (
          <section className="report-section">
            <h2>{t.contacts}</h2>
            <div className="stack" style={{ gap: 4 }}>
              {manager && <b>{manager}</b>}
              {s?.manager_phone && <span>{t.phone}: {s.manager_phone}</span>}
              {s?.manager_email && <span>{t.email}: {s.manager_email}</span>}
              {s?.manager_telegram && <span>Telegram: {s.manager_telegram}</span>}
            </div>
          </section>
        )}
      </div>
    </div>
  );
}
