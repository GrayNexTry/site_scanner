// Рассылка по почте: настройки SMTP и правил, страница «Рассылка» (очередь, ошибки, отправленное, пуск/пауза),
// постановка в очередь выбранных компаний и одного письма со страницы компании.
import { useEffect, useState, type ReactNode } from "react";
import { Link } from "react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api, type Contact, type MailSource, type OutboxRow, type Settings } from "../api";
import { useToast } from "../components";
import { LEAD_STATUS_LABEL } from "../format";
import { MessageStatus } from "./AiDraft";

function ErrorLine({ error }: { error: Error | null }) {
  return error ? <span className="small st-fail" role="alert">{error.message}</span> : null;
}

/* ------------------------------------------------------------------ настройки */

const PRESETS: { label: string; host: string; port: number; security: Settings["smtp_security"]; how: string }[] = [
  { label: "Gmail", host: "smtp.gmail.com", port: 465, security: "ssl", how: "Нужен пароль приложения: myaccount.google.com → Безопасность → двухэтапная аутентификация → Пароли приложений." },
  { label: "Яндекс", host: "smtp.yandex.ru", port: 465, security: "ssl", how: "Пароль приложения: id.yandex.ru → Безопасность → Пароли приложений → Почта; в настройках почты включите доступ по SMTP." },
  { label: "Mail.ru", host: "smtp.mail.ru", port: 465, security: "ssl", how: "Пароль для внешнего приложения: Настройки → Безопасность → Пароли для внешних приложений." },
];

type MailTextKey = "smtp_host" | "smtp_user" | "mail_from_name" | "mail_from_email" | "mail_reply_to";
type MailNumKey = "smtp_port" | "mail_interval_min" | "mail_interval_max" | "mail_daily_limit" | "mail_window_start" | "mail_window_end";

export function MailSettingsCard({ limits, setLimits, passwordInput }: { limits: Partial<Settings>; setLimits: (f: (l: Partial<Settings>) => Partial<Settings>) => void; passwordInput: ReactNode }) {
  const test = useMutation({ mutationFn: () => api.mailTest() });
  const set = (patch: Partial<Settings>) => setLimits((l) => ({ ...l, ...patch }));
  const text = (k: MailTextKey, label: string, placeholder: string, hint?: string) => (
    <div className="field">
      <label htmlFor={k}>{label}</label>
      <input id={k} className="input" placeholder={placeholder} value={(limits[k] as string) ?? ""} onChange={(e) => set({ [k]: e.target.value })} autoComplete="off" />
      {hint && <span className="hint">{hint}</span>}
    </div>
  );
  const num = (k: MailNumKey, label: string, hint: string, min: number, max: number) => (
    <div className="field">
      <label htmlFor={k}>{label}</label>
      <input id={k} type="number" className="input" min={min} max={max} value={Number(limits[k] ?? 0)} onChange={(e) => set({ [k]: Number(e.target.value) })} />
      <span className="hint">{hint}</span>
    </div>
  );
  const preset = PRESETS.find((p) => p.host === limits.smtp_host);
  return (
    <div className="card" id="mail">
      <div className="card-head"><h2>Почта для рассылки</h2></div>
      <div className="card-pad stack" style={{ gap: 16 }}>
        <p className="small muted" style={{ margin: 0 }}>
          Письма уходят с вашего ящика по одному, с паузами и дневным лимитом, только в окно отправки. Что и кому — решаете вы в скане и на странице «Рассылка»; рассылка
          запускается кнопкой. После отправки компания становится «Написали», в журнале — текст письма, следующий шаг — «Проверить ответ» через 3 дня.
        </p>
        <div className="row">
          <span className="label">Почтовый сервис</span>
          {PRESETS.map((p) => (
            <button key={p.host} type="button" className="chip" aria-pressed={limits.smtp_host === p.host} onClick={() => set({ smtp_host: p.host, smtp_port: p.port, smtp_security: p.security })}>{p.label}</button>
          ))}
        </div>
        {preset && <span className="hint">{preset.how}</span>}
        <div className="grid-3">
          {text("smtp_host", "SMTP-сервер", "smtp.example.com")}
          {num("smtp_port", "Порт", "465 — SSL, 587 — STARTTLS.", 1, 65535)}
          <div className="field">
            <label htmlFor="smtp_security">Шифрование</label>
            <select id="smtp_security" className="select" style={{ height: 34 }} value={limits.smtp_security ?? "ssl"} onChange={(e) => set({ smtp_security: e.target.value as Settings["smtp_security"] })}>
              <option value="ssl">SSL/TLS (обычно порт 465)</option>
              <option value="starttls">STARTTLS (обычно порт 587)</option>
              {limits.smtp_security === "plain" && <option value="plain">без шифрования (только локальный сервер)</option>}
            </select>
          </div>
        </div>
        <div className="grid-2">
          {text("smtp_user", "Логин", "you@example.com", "Обычно — адрес почты целиком.")}
          {passwordInput}
        </div>
        <div className="grid-3">
          {text("mail_from_name", "Имя отправителя", "Иван Петров", "Так письмо подписано в ящике получателя.")}
          {text("mail_from_email", "Адрес отправителя", "пусто — логин", "Должен принадлежать этому ящику, иначе письма попадут в спам.")}
          {text("mail_reply_to", "Куда отвечать", "пусто — на адрес отправителя")}
        </div>
        <div className="row" style={{ alignItems: "center" }}>
          <button type="button" className="btn btn-sm" onClick={() => test.mutate()} disabled={test.isPending}>{test.isPending ? "Отправляю…" : "Отправить пробное письмо себе"}</button>
          {test.data && <span className="small st-pass">Ушло на {test.data.to} — проверьте ящик (и «Спам»).</span>}
          <ErrorLine error={test.error} />
        </div>
        <span className="hint">Проверяется сохранённая настройка — после правок сначала «Сохранить».</span>

        <span className="label">Правила рассылки</span>
        <div className="grid-3">
          {num("mail_daily_limit", "Писем в день", "Новому ящику — 20–30, потом можно больше. Много сразу — письма начнут уходить в спам.", 1, 2000)}
          {num("mail_interval_min", "Пауза от, сек", "Между письмами — случайная пауза в этих пределах.", 10, 86400)}
          {num("mail_interval_max", "Пауза до, сек", "Например, 120–300: 2–5 минут.", 10, 86400)}
          {num("mail_window_start", "Отправлять с, час", "По вашему времени.", 0, 23)}
          {num("mail_window_end", "Отправлять до, час", "Конец не позже начала — круглые сутки.", 1, 24)}
          <label className="check">
            <input type="checkbox" checked={!!limits.mail_weekdays_only} onChange={(e) => set({ mail_weekdays_only: e.target.checked })} />
            <span><b>Только по будням</b></span>
          </label>
        </div>
        <label className="check">
          <input type="checkbox" checked={!!limits.mail_attach_report} onChange={(e) => set({ mail_attach_report: e.target.checked })} />
          <span>
            <b>Прикладывать PDF-разбор сайта</b>
            <span className="hint" style={{ display: "block" }}>Тот же отчёт, что «Скачать отчёт (PDF)». Компаниям без сайта не прикладывается. Нужен Chromium.</span>
          </span>
        </label>
        <div className="grid-2">
          <div className="field">
            <label htmlFor="mail_footer">Строка в конце письма</label>
            <textarea id="mail_footer" className="input" rows={2} value={limits.mail_footer ?? ""} onChange={(e) => set({ mail_footer: e.target.value })} />
            <span className="hint">Как отказаться от писем. Пусто — не добавлять.</span>
          </div>
          <div className="field">
            <label htmlFor="mail_footer_en">То же по-английски</label>
            <textarea id="mail_footer_en" className="input" rows={2} value={limits.mail_footer_en ?? ""} onChange={(e) => set({ mail_footer_en: e.target.value })} />
            <span className="hint">Для писем на английском.</span>
          </div>
        </div>
      </div>
    </div>
  );
}

/* ------------------------------------------------------------------ страница «Рассылка» */

type Tab = "queue" | "failed" | "sent";

/** «2–5 мин» или «10–12 с», если пауза меньше пары минут. */
function fmtPause(min: number, max: number): string {
  return max < 120 ? `${min}–${max} с` : `${Math.round(min / 60)}–${Math.round(max / 60)} мин`;
}

function timeOf(iso: string | null | undefined): string {
  if (!iso) return "";
  const d = new Date(iso);
  const hm = d.toLocaleTimeString("ru-RU", { hour: "2-digit", minute: "2-digit" });
  return d.toDateString() === new Date().toDateString() ? `сегодня в ${hm}` : `${d.toLocaleDateString("ru-RU", { day: "numeric", month: "short" })} в ${hm}`;
}

function OutboxItem({ m }: { m: OutboxRow }) {
  const qc = useQueryClient();
  const [to, setTo] = useState(m.to_addr ?? "");
  const [subject, setSubject] = useState(m.subject ?? "");
  const [body, setBody] = useState(m.body);
  const refresh = () => {
    qc.invalidateQueries({ queryKey: ["outbox"] });
    qc.invalidateQueries({ queryKey: ["mail-status"] });
  };
  const save = useMutation({ mutationFn: () => api.queueOne(m.business_id, { message_id: m.id, to, subject, body }), onSuccess: refresh });
  const remove = useMutation({ mutationFn: () => api.unqueue(m.id), onSuccess: refresh });
  const editable = m.status === "queued" || m.status === "failed";
  const changed = to !== (m.to_addr ?? "") || subject !== (m.subject ?? "") || body !== m.body;
  return (
    <details style={{ borderBottom: "1px solid var(--line)", padding: "8px 0" }}>
      <summary style={{ cursor: "pointer" }}>
        <span className="small">
          <Link to={`/businesses/${m.business_id}#outreach`} onClick={(e) => e.stopPropagation()}><b>{m.display_name || m.business_name}</b></Link>
          {" · "}
          <MessageStatus m={m} />
          {m.status === "sent" && m.lead_status !== "contacted" && <span className="faint"> · сейчас «{LEAD_STATUS_LABEL[m.lead_status]}»</span>}
          {m.status === "queued" && m.queued_at && <span className="faint"> · поставлено {timeOf(m.queued_at)}</span>}
        </span>
        <div className="small muted ellipsis">{m.subject}</div>
        {m.error && <div className="small st-fail">{m.error}</div>}
      </summary>
      <div className="stack" style={{ gap: 8, marginTop: 8 }}>
        {editable ? (
          <>
            <input className="input" value={to} onChange={(e) => setTo(e.target.value)} aria-label="Кому" placeholder="адрес получателя" />
            <input className="input" value={subject} onChange={(e) => setSubject(e.target.value)} aria-label="Тема" />
            <textarea className="input" rows={9} value={body} onChange={(e) => setBody(e.target.value)} aria-label="Текст письма" />
            <div className="row">
              <button type="button" className="btn btn-sm btn-primary" onClick={() => save.mutate()} disabled={save.isPending || (!changed && m.status === "queued")}>
                {m.status === "failed" ? "Повторить отправку" : "Сохранить правки"}
              </button>
              <span className="spacer" />
              <button type="button" className="btn btn-sm btn-ghost" onClick={() => remove.mutate()} disabled={remove.isPending}>Убрать в черновики</button>
            </div>
            <ErrorLine error={save.error ?? remove.error} />
          </>
        ) : (
          <>
            <div className="small"><b>Кому:</b> {m.to_addr} · <b>Тема:</b> {m.subject}</div>
            <div className="small" style={{ whiteSpace: "pre-wrap" }}>{m.body}</div>
          </>
        )}
      </div>
    </details>
  );
}

export default function MailPage() {
  const qc = useQueryClient();
  const status = useQuery({ queryKey: ["mail-status"], queryFn: api.mailStatus, refetchInterval: 5000 });
  const outbox = useQuery({ queryKey: ["outbox"], queryFn: api.outbox, refetchInterval: status.data?.running ? 5000 : 30000 });
  const [tab, setTab] = useState<Tab>("queue");
  const refresh = (d: unknown) => {
    qc.setQueryData(["mail-status"], d);
    qc.invalidateQueries({ queryKey: ["outbox"] });
    qc.invalidateQueries({ queryKey: ["settings"] });
  };
  const start = useMutation({ mutationFn: api.mailStart, onSuccess: refresh });
  const pause = useMutation({ mutationFn: api.mailPause, onSuccess: refresh });
  const s = status.data;
  const rows = outbox.data ?? [];
  const by: Record<Tab, OutboxRow[]> = {
    queue: rows.filter((m) => m.status === "queued" || m.status === "sending"),
    failed: rows.filter((m) => m.status === "failed"),
    sent: rows.filter((m) => m.status === "sent"),
  };
  const info = s?.info;
  return (
    <div className="page" style={{ maxWidth: 980 }}>
      <div className="page-head">
        <div>
          <h1>Рассылка</h1>
          <p className="sub">{s?.configured ? `Письма уходят с ${s.from} по одному` : "Почта ещё не настроена"}</p>
        </div>
      </div>
      {s && !s.configured && (
        <div className="banner warn small">
          <span>Укажите почту для рассылки: <Link to="/settings#mail">Настройки → Почта для рассылки</Link>.</span>
        </div>
      )}
      {s && (
        <div className="card card-pad stack" style={{ gap: 10 }}>
          <div className="row" style={{ alignItems: "center" }}>
            <b>{s.running ? "Рассылка идёт" : "Рассылка на паузе"}</b>
            {s.running && info?.waiting && <span className="small muted">— {info.waiting}{info.next_at ? `, следующее ${timeOf(info.next_at)}` : ""}</span>}
            <span className="spacer" />
            {s.running ? (
              <button type="button" className="btn" onClick={() => pause.mutate()} disabled={pause.isPending}>Пауза</button>
            ) : (
              <button type="button" className="btn btn-primary" onClick={() => start.mutate()} disabled={start.isPending || !s.configured}>Запустить рассылку</button>
            )}
          </div>
          <div className="small muted">
            Сегодня отправлено {s.sent_today} из {s.daily_limit} · в очереди {s.queued}
            {s.failed ? ` · не ушло ${s.failed}` : ""} · окно {s.window.end <= s.window.start ? "круглые сутки" : `${s.window.start}:00–${s.window.end}:00`}
            {s.window.weekdays_only ? ", по будням" : ""} · пауза {fmtPause(s.interval.min, s.interval.max)}
            {s.attach_report ? " · с PDF-разбором" : ""} · <Link to="/settings#mail">правила</Link>
          </div>
          {info?.stop_reason && <div className="banner warn small"><span>{info.stop_reason}</span></div>}
          {info?.last_error && !info.stop_reason && <div className="small st-warn">Последняя ошибка: {info.last_error}</div>}
          <ErrorLine error={start.error ?? pause.error} />
        </div>
      )}
      <div className="chips" role="tablist">
        <button type="button" className="chip" role="tab" aria-pressed={tab === "queue"} onClick={() => setTab("queue")}>В очереди <span className="count">{by.queue.length}</span></button>
        <button type="button" className="chip" role="tab" aria-pressed={tab === "failed"} onClick={() => setTab("failed")}>Не ушли <span className="count">{by.failed.length}</span></button>
        <button type="button" className="chip" role="tab" aria-pressed={tab === "sent"} onClick={() => setTab("sent")}>Отправлено <span className="count">{by.sent.length}</span></button>
      </div>
      <div className="card card-pad">
        {outbox.isLoading && <span className="muted">Загружаю…</span>}
        {!outbox.isLoading && by[tab].length === 0 && (
          <span className="small muted">
            {tab === "queue"
              ? "Очередь пуста. Отметьте компании в скане и нажмите «В рассылку» или поставьте письмо со страницы компании."
              : tab === "failed"
                ? "Ошибок нет."
                : "Рассылкой ещё ничего не отправлено."}
          </span>
        )}
        {by[tab].map((m) => <OutboxItem key={`${m.id}-${m.status}-${m.updated_at}`} m={m} />)}
      </div>
    </div>
  );
}

/* ------------------------------------------------------------------ в рассылку из скана */

export function MailQueueBar({ ids, label }: { ids: number[]; label: string }) {
  const qc = useQueryClient();
  const [open, setOpen] = useState(false);
  const [source, setSource] = useState<MailSource>("draft_or_template");
  const [skipContacted, setSkipContacted] = useState(true);
  const queue = useMutation({
    mutationFn: () => api.queueMail({ ids, source, skip_contacted: skipContacted, skip_sent: true }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["mail-status"] });
      qc.invalidateQueries({ queryKey: ["outbox"] });
    },
  });
  // Выбор поменялся — прошлый итог больше не про эти компании.
  const reset = queue.reset;
  useEffect(() => reset(), [ids.length, reset]);
  if (!open) {
    return <button type="button" className="btn" onClick={() => setOpen(true)} disabled={ids.length === 0}>В рассылку</button>;
  }
  const r = queue.data;
  return (
    <div className="card card-pad stack" style={{ gap: 10, width: "100%" }}>
      <div className="row" style={{ alignItems: "center" }}>
        <h2 style={{ margin: 0, fontSize: 16 }}>В рассылку — {label}</h2>
        <span className="spacer" />
        <button type="button" className="btn btn-sm btn-ghost" onClick={() => setOpen(false)}>Закрыть</button>
      </div>
      <div className="row" style={{ alignItems: "center" }}>
        <select className="select" style={{ width: 330, height: 34 }} value={source} onChange={(e) => setSource(e.target.value as MailSource)} aria-label="Какой текст отправить">
          <option value="draft_or_template">черновик нейросети, иначе шаблон</option>
          <option value="draft">только черновики нейросети</option>
          <option value="template">шаблон сканера</option>
        </select>
        <label className="small" style={{ display: "flex", gap: 6, alignItems: "center" }}>
          <input type="checkbox" checked={skipContacted} onChange={(e) => setSkipContacted(e.target.checked)} />
          пропускать компании, с которыми уже работаете
        </label>
        <span className="spacer" />
        <button type="button" className="btn btn-primary" onClick={() => queue.mutate()} disabled={queue.isPending || ids.length === 0}>Поставить в очередь ({ids.length})</button>
      </div>
      <span className="small faint">
        Письмо уходит на почту компании (не на адрес для вакансий). Без почты, без проверки сайта и тем, кому уже отправляли, — пропуск. Отправка начнётся после «Запустить
        рассылку» на странице <Link to="/mail">«Рассылка»</Link>; там же можно поправить любое письмо.
      </span>
      <ErrorLine error={queue.error} />
      {r && (
        <div className="stack" style={{ gap: 4 }}>
          <span className="small">
            В очередь: <b>{r.queued}</b>
            {r.skipped.length ? `, пропущено ${r.skipped.length}` : ""}. <Link to="/mail">Открыть рассылку</Link>
          </span>
          {r.skipped.length > 0 && (
            <details>
              <summary className="small">Почему пропущены</summary>
              <ul className="small muted" style={{ margin: "6px 0 0", paddingLeft: 18 }}>
                {r.skipped.map((x) => <li key={x.id}><Link to={`/businesses/${x.id}`}>{x.name}</Link>: {x.reason}</li>)}
              </ul>
            </details>
          )}
        </div>
      )}
    </div>
  );
}

/* ------------------------------------------------------------------ одно письмо со страницы компании */

export function QueueEmail({ businessId, contacts, draftId, subject, body, onQueued }: { businessId: number; contacts: Contact[]; draftId: number | null; subject: string; body: string; onQueued: (mid: number) => void }) {
  const qc = useQueryClient();
  const [toast, show] = useToast();
  // Сначала адреса без пометок (пометка — «вакансии»).
  const emails = contacts.filter((c) => c.kind === "email").sort((a, b) => Number(!!a.note) - Number(!!b.note));
  const [to, setTo] = useState(emails[0]?.value ?? "");
  const queue = useMutation({
    mutationFn: () => api.queueOne(businessId, { message_id: draftId ?? undefined, to, subject, body }),
    onSuccess: (m) => {
      onQueued(m.id);
      qc.invalidateQueries({ queryKey: ["business", businessId] });
      qc.invalidateQueries({ queryKey: ["mail-status"] });
      show("Письмо в очереди рассылки");
    },
  });
  return (
    <div className="row" style={{ alignItems: "center" }}>
      {emails.length > 1 ? (
        <select className="select" style={{ width: 230, height: 34 }} value={to} onChange={(e) => setTo(e.target.value)} aria-label="Кому">
          {emails.map((c) => <option key={c.value} value={c.value}>{c.value}{c.note ? ` (${c.note})` : ""}</option>)}
        </select>
      ) : (
        <input className="input" style={{ width: 230 }} placeholder="адрес почты" value={to} onChange={(e) => setTo(e.target.value)} aria-label="Кому" />
      )}
      <button type="button" className="btn" onClick={() => queue.mutate()} disabled={queue.isPending || !to.includes("@") || !subject.trim() || !body.trim()}>
        В рассылку
      </button>
      <Link className="small" to="/mail">Рассылка</Link>
      <ErrorLine error={queue.error} />
      {toast}
    </div>
  );
}
