import { useMemo } from "react";
import { useParams } from "react-router";
import { useQuery } from "@tanstack/react-query";
import { api, type MetricDef, type MetricResult } from "../api";
import { countryName, fmtDate, hostOf, plural, scoreTone, siteOf } from "../format";

/** Зачем клиенту каждая категория — простыми словами. */
const WHY: Record<string, string> = {
  A: "Через карты приходит большая часть клиентов локального бизнеса. Карточка должна быть полной и вести на сайт.",
  B: "Если сайт открывается долго или с ошибками, клиент уходит, а поисковики показывают сайт ниже.",
  C: "Чтобы сайт находили в Google и Яндексе, поисковые роботы должны видеть и правильно понимать его страницы.",
  D: "Медленные страницы теряют посетителей; Google учитывает скорость при ранжировании.",
  E: "Больше половины клиентов ищут услуги с телефона.",
  F: "Заголовки и тексты должны отвечать на запрос «услуга + город» — так сайт попадает в выдачу.",
  G: "Разметка schema.org сообщает поисковикам и ИИ-ассистентам, кто вы, где находитесь и когда работаете.",
  H: "Адрес, телефон и часы работы на сайте должны совпадать с картами — это сигнал доверия для поисковиков.",
  I: "ChatGPT, Gemini, Perplexity, Алиса и обзоры Google с ИИ советуют сайты, которые их роботы могут прочитать и в которых есть готовый ответ: что, где, почём, с датой обновления.",
  M: "Обязательные документы и реквизиты защищают от штрафов и вызывают доверие клиентов.",
  N: "Чем проще позвонить, написать или записаться, тем больше заявок с тех же посетителей.",
  O: "Защищённое соединение и заголовки безопасности берегут посетителей и репутацию сайта.",
  P: "Без аналитики нельзя посчитать, сколько заявок приносит сайт и реклама.",
  Q: "Сайт должен быть удобен всем посетителям, включая людей с нарушениями зрения.",
};

function verdict(score: number | null | undefined, hasSite: boolean, fails: number): string {
  if (!hasSite) return "У компании нет работающего сайта: клиенты с карт и из поиска уходят к конкурентам.";
  if (score == null) return "Сайт не удалось проверить полностью.";
  if (score >= 75 && fails >= 5) return `Сайт в целом в порядке, но ${fails} ${plural(fails, "критичная проблема мешает", "критичные проблемы мешают", "критичных проблем мешают")} поиску и заявкам.`;
  if (score >= 75) return "Сайт в хорошем состоянии; есть точечные улучшения.";
  if (score >= 50) return "Есть заметные проблемы, из-за которых сайт теряет клиентов из поиска и карт.";
  return "Сайт теряет клиентов: много критичных проблем с поиском, картами и заявками.";
}

type Row = { r: MetricResult; def: MetricDef };

/** Отчёт для клиента: печатается или сохраняется в PDF из браузера. */
export default function ReportPage() {
  const id = Number(useParams().id);
  const meta = useQuery({ queryKey: ["meta"], queryFn: api.meta, staleTime: 60_000 });
  const q = useQuery({ queryKey: ["business", id], queryFn: () => api.business(id) });

  const rows: Row[] = useMemo(() => {
    const defs = new Map((meta.data?.catalog.metrics ?? []).map((m) => [m.id, m]));
    return (q.data?.business.results ?? []).flatMap((r) => {
      const def = defs.get(r.id);
      return def ? [{ r, def }] : [];
    });
  }, [q.data, meta.data]);

  if (q.error) return <div className="report-page"><div className="report">{q.error.message}</div></div>;
  const b = q.data?.business;
  if (!b || !meta.data) return <div className="report-page"><div className="report muted">Готовлю отчёт…</div></div>;

  const report = b.report;
  const site = siteOf(b);
  const hasSite = !!b.site && !!b.site.final_url && !b.site.error;
  const categories = meta.data.catalog.categories;
  const scored = rows.filter((x) => x.def.kind === "check" && x.def.weight > 0 && ["pass", "warn", "fail"].includes(x.r.status));
  const fails = scored.filter((x) => x.r.status === "fail");
  const warns = scored.filter((x) => x.r.status === "warn");
  const passes = scored.filter((x) => x.r.status === "pass");
  const rank = (x: Row) => (x.def.stop_factor ? 100 : 0) + (x.r.status === "fail" ? 10 : 0) + x.def.weight;
  const top = [...fails, ...warns].sort((a, c) => rank(c) - rank(a)).slice(0, 10);
  const ai = rows.filter((x) => x.r.id.startsWith("I") && ["pass", "warn", "fail"].includes(x.r.status));
  const catRows = Object.entries(categories)
    .map(([letter, label]) => ({ letter, label, c: report?.categories[letter] }))
    .filter((x) => x.c && x.c.measured > 0);
  const tone = scoreTone(report?.overall);
  const country = q.data?.scan?.params.country;

  return (
    <div className="report-page">
      <div className="report">
        <div className="report-actions no-print">
          <button className="btn btn-primary" onClick={() => window.print()}>Печать или PDF</button>
          <span className="small muted">В окне печати выберите «Сохранить как PDF»</span>
        </div>

        <header className="report-head">
          <div className="small muted">Аудит сайта и карточек на картах · {fmtDate(b.audited_at)}</div>
          <h1>{b.name}</h1>
          <div className="muted">{[site ? hostOf(site) : "сайта нет", b.address, b.phone].filter(Boolean).join(" · ")}</div>
        </header>

        <section className="report-score">
          <div className={`report-big st-${tone}`}>{report?.overall ?? "—"}<span>/100</span></div>
          <div className="stack" style={{ gap: 6 }}>
            <b>{verdict(report?.overall, hasSite, fails.length)}</b>
            <span className="muted">
              Проверено {scored.length} {plural(scored.length, "показатель", "показателя", "показателей")}:{" "}
              <span className="st-fail">{fails.length} {plural(fails.length, "критичная проблема", "критичные проблемы", "критичных проблем")}</span>,{" "}
              <span className="st-warn">{warns.length} требуют внимания</span>, <span className="st-pass">{passes.length} в норме</span>.
            </span>
            {report && report.stop_factors.length > 0 && (
              <span className="small st-fail">Есть стоп-факторы — ошибки, которые перекрывают остальное: итоговая оценка ограничена 25 баллами.</span>
            )}
          </div>
        </section>

        {top.length > 0 && (
          <section className="report-section">
            <h2>Главное, что стоит исправить</h2>
            <ol className="report-list">
              {top.map(({ r, def }) => (
                <li key={r.id}>
                  <div className="row" style={{ gap: 8 }}>
                    <b>{def.name}</b>
                    <span className={`report-tag st-${r.status}`}>{r.status === "fail" ? "критично" : "внимание"}</span>
                  </div>
                  <div>Сейчас: {r.detail}</div>
                  {!def.rule.includes("→") && <div className="small muted">Норма: {def.rule}</div>}
                </li>
              ))}
            </ol>
          </section>
        )}

        {catRows.length > 0 && (
          <section className="report-section">
            <h2>Оценка по разделам</h2>
            <table className="report-table">
              <tbody>
                {catRows.map(({ letter, label, c }) => (
                  <tr key={letter}>
                    <td>
                      <b>{label}</b>
                      {WHY[letter] && <div className="small muted">{WHY[letter]}</div>}
                    </td>
                    <td className="num" style={{ width: 160 }}>
                      <div className="report-bar"><span className={`st-${scoreTone(c!.score)}`} style={{ width: `${c!.score ?? 0}%` }} /></div>
                      <span className="small">{c!.score != null ? Math.round(c!.score) : "—"} / 100</span>
                    </td>
                    <td className="small num" style={{ width: 120 }}>
                      {c!.fail > 0 && <span className="st-fail">{c!.fail} крит. </span>}
                      {c!.warn > 0 && <span className="st-warn">{c!.warn} вним.</span>}
                      {c!.fail === 0 && c!.warn === 0 && <span className="st-pass">в норме</span>}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </section>
        )}

        {ai.length > 0 && (
          <section className="report-section">
            <h2>Готовность к поиску с ИИ</h2>
            <p className="small muted">{WHY.I}</p>
            <ul className="report-checks">
              {ai.map(({ r, def }) => (
                <li key={r.id}>
                  <span className={`dot st-${r.status}`} /> <b>{def.name}.</b> <span className="muted">{r.detail}</span>
                </li>
              ))}
            </ul>
          </section>
        )}

        {passes.length > 0 && (
          <section className="report-section">
            <h2>Что уже в порядке</h2>
            <ul className="report-pass">
              {passes.map(({ r, def }) => <li key={r.id}>{def.name}</li>)}
            </ul>
          </section>
        )}

        <footer className="report-foot small muted">
          Замеры сделаны автоматически {fmtDate(b.audited_at)} по внешним признакам: без доступа к кабинетам сайта и карт.
          {country ? ` Правовые проверки — по нормам страны: ${countryName(country)}.` : ""} Позиции в поиске и ответы ИИ-ассистентов в этот отчёт не входят: они
          меряются отдельно, по методике с фиксированным набором запросов. Повторный замер после работ покажет изменения по тем же показателям.
        </footer>
      </div>
    </div>
  );
}
