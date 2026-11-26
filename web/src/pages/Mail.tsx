// Рассылка по почте: настройки SMTP и правил, страница «Рассылка» (очередь, ошибки, отправленное, пуск/пауза),
// постановка в очередь выбранных компаний и одного письма со страницы компании.
import { useEffect, useRef, useState, type ReactNode } from "react";
import { Link } from "react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api, type Contact, type MailSource, type OAuthProviderId, type OutboxRow, type Settings } from "../api";
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
  { label: "Mail.ru", host: "smtp.mail.ru", port: 465, security: "ssl", how: "Пароль для внешнего приложения: Настройки → Безопасность → Пароли для внешних приложений. Вход через аккаунт Mail.ru сторонним программам не даёт." },
];

type MailTextKey = "smtp_host" | "smtp_user" | "mail_from_name" | "mail_from_email" | "mail_reply_to";
type MailNumKey = "smtp_port" | "mail_interval_min" | "mail_interval_max" | "mail_daily_limit" | "mail_window_start" | "mail_window_end";

/** Как зарегистрировать своё приложение OAuth у каждого сервиса (один раз, бесплатно). */
function OAuthSteps({ provider, redirect }: { provider: OAuthProviderId; redirect: string | null }) {
  const uri = redirect ? <code style={{ userSelect: "all" }}>{redirect}</code> : <span className="st-warn">откройте сканер по адресу 127.0.0.1</span>;
  if (provider === "google")
    return (
      <ol className="small muted" style={{ margin: 0, paddingLeft: 18 }}>
        <li>console.cloud.google.com → создайте проект (любое имя) → Library → включите «Gmail API».</li>
        <li>Google Auth Platform → Get started: название, ваш адрес, аудитория — External.</li>
        <li>Data Access → Add or remove scopes → вручную добавьте <code>https://mail.google.com/</code>.</li>
        <li>
          Audience → <b>Publish app</b> (In production). В режиме «Testing» Google отключает вход через 7 дней. При входе Google покажет «приложение не проверено» — для своего
          ящика это нормально: «Дополнительно» → «Перейти».
        </li>
        <li>Clients → Create client → тип <b>Desktop app</b> → скопируйте сюда Client ID и Client secret. Адрес возврата для Desktop app указывать не нужно.</li>
      </ol>
    );
  if (provider === "yandex")
    return (
      <ol className="small muted" style={{ margin: 0, paddingLeft: 18 }}>
        <li>oauth.yandex.ru → «Создать приложение» (под тем ящиком, с которого пойдут письма).</li>
        <li>Платформа — «Веб-сервисы», Redirect URI: {uri}</li>
        <li>Доступы: «Почта» → отправка писем по SMTP (<code>mail:smtp</code>); «Яндекс ID» → доступ к адресу почты (<code>login:email</code>).</li>
        <li>Скопируйте сюда ClientID и Client secret.</li>
        <li>В Яндекс Почте: Все настройки → Почтовые программы → разрешите доступ к ящику с помощью почтовых клиентов.</li>
      </ol>
    );
  return (
    <ol className="small muted" style={{ margin: 0, paddingLeft: 18 }}>
      <li>entra.microsoft.com → App registrations → New registration (нужна бесплатная учётная запись Microsoft Entra / Azure).</li>
      <li>Supported account types — «Accounts in any organizational directory and personal Microsoft accounts».</li>
      <li>Redirect URI: платформа «Public client/native (mobile &amp; desktop)» → {uri}</li>
      <li>Скопируйте сюда Application (client) ID. Секрет не нужен, разрешения Microsoft спросит при входе.</li>
    </ol>
  );
}

/** Вход в почту через аккаунт: своё приложение OAuth, кнопка входа, состояние подключения. */
function OAuthLogin() {
  const qc = useQueryClient();
  const [waiting, setWaiting] = useState(false);
  const status = useQuery({ queryKey: ["oauth"], queryFn: api.oauthStatus, refetchInterval: waiting ? 2000 : false });
  const st = status.data;
  const [provider, setProvider] = useState<OAuthProviderId | null>(null);
  const current: OAuthProviderId = provider ?? (st?.provider || "google");
  const info = st?.providers.find((p) => p.id === current);
  const [ids, setIds] = useState<Partial<Record<OAuthProviderId, string>>>({});
  const [secret, setSecret] = useState("");
  const [blockedUrl, setBlockedUrl] = useState<string | null>(null);
  const tab = useRef<Window | null>(null);
  const clientId = ids[current] ?? info?.client_id ?? "";
  const refresh = () => {
    qc.invalidateQueries({ queryKey: ["oauth"] });
    qc.invalidateQueries({ queryKey: ["settings"] });
    qc.invalidateQueries({ queryKey: ["mail-status"] });
  };
  const start = useMutation({
    mutationFn: () => api.oauthStart({ provider: current, client_id: clientId.trim() || undefined, client_secret: secret.trim() || undefined }),
    onSuccess: (r) => {
      setSecret("");
      setBlockedUrl(null);
      if (tab.current && !tab.current.closed) tab.current.location.href = r.url;
      else setBlockedUrl(r.url);
      setWaiting(true);
      refresh();
    },
    onError: () => tab.current?.close(),
  });
  const logout = useMutation({ mutationFn: api.oauthDisconnect, onSuccess: refresh });
  // Вход завершён (успешно или с ошибкой) — перестать опрашивать и обновить настройки; через 5 минут — сдаться.
  const done = !!(st?.last_ok || st?.last_error);
  useEffect(() => {
    if (!waiting) return;
    if (done) {
      setWaiting(false);
      refresh();
      return;
    }
    const t = setTimeout(() => setWaiting(false), 5 * 60_000);
    return () => clearTimeout(t);
  }, [waiting, done]);
  const login = () => {
    // Вкладку открываем сразу по нажатию (иначе браузер заблокирует её), адрес входа подставим после ответа сервера.
    tab.current = window.open("about:blank", "_blank");
    start.mutate();
  };
  if (!st) return <span className="small muted">{status.error ? status.error.message : "Загружаю…"}</span>;
  const connectedHere = st.connected && st.provider === current;
  return (
    <div className="stack" style={{ gap: 12 }}>
      {st.connected && (
        <div className="row" style={{ alignItems: "center" }}>
          <span className="small st-pass">
            Подключено: <b>{st.email}</b> ({st.providers.find((p) => p.id === st.provider)?.name})
          </span>
          <span className="spacer" />
          <button type="button" className="btn btn-sm btn-ghost btn-danger" onClick={() => logout.mutate()} disabled={logout.isPending}>Выйти</button>
        </div>
      )}
      <div className="row">
        <span className="label">Сервис</span>
        {st.providers.map((p) => (
          <button key={p.id} type="button" className="chip" aria-pressed={current === p.id} onClick={() => setProvider(p.id)}>{p.name}</button>
        ))}
      </div>
      <details open={!info?.client_id}>
        <summary className="small">Как подключить {info?.name} — один раз, бесплатно</summary>
        <div style={{ marginTop: 8 }}>
          <OAuthSteps provider={current} redirect={st.redirect_uri} />
          <p className="hint" style={{ marginTop: 8 }}>
            Приложение — ваше: Google и другие сервисы не дают программам с открытым кодом общий ключ к чужой почте. Пароль ящика сканер не видит; доступ можно отозвать в
            настройках аккаунта.
          </p>
        </div>
      </details>
      <div className={info?.needs_secret ? "grid-2" : ""}>
        <div className="field">
          <label htmlFor={`oauth-${current}-id`}>Client ID</label>
          <input id={`oauth-${current}-id`} className="input" autoComplete="off" value={clientId} onChange={(e) => setIds((m) => ({ ...m, [current]: e.target.value }))} placeholder={current === "google" ? "….apps.googleusercontent.com" : ""} />
        </div>
        {info?.needs_secret && (
          <div className="field">
            <label htmlFor={`oauth-${current}-secret`}>Client secret</label>
            <input id={`oauth-${current}-secret`} className="input" type="password" autoComplete="off" value={secret} onChange={(e) => setSecret(e.target.value)} placeholder={info.secret_set ? "задан — оставьте пустым" : "вставьте секрет"} />
          </div>
        )}
      </div>
      <div className="row" style={{ alignItems: "center" }}>
        <button type="button" className="btn btn-primary" onClick={login} disabled={start.isPending || !clientId.trim() || !!st.redirect_error}>
          {connectedHere ? "Войти другим ящиком" : `Войти через ${info?.name ?? ""}`}
        </button>
        {waiting && <span className="small muted">Завершите вход во вкладке сервиса…</span>}
        {st.redirect_error && <span className="small st-warn">{st.redirect_error}</span>}
        <ErrorLine error={start.error ?? logout.error} />
      </div>
      {blockedUrl && (
        <span className="small">
          Браузер не открыл вкладку — <a href={blockedUrl} target="_blank" rel="noreferrer">откройте страницу входа</a>.
        </span>
      )}
      {st.last_error && !waiting && <div className="banner warn small"><span>{st.last_error}</span></div>}
    </div>
  );
}

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
  const oauth = limits.smtp_auth === "oauth";
  return (
    <div className="card" id="mail">
      <div className="card-head"><h2>Почта для рассылки</h2></div>
      <div className="card-pad stack" style={{ gap: 16 }}>
        <p className="small muted" style={{ margin: 0 }}>
          Письма уходят с вашего ящика по одному, с паузами и дневным лимитом, только по будням и днём по времени получателя. Что и кому — решаете вы в скане и на странице
          «Рассылка»; рассылка запускается кнопкой. После отправки компания становится «Написали», в журнале — текст письма, следующий шаг — «Проверить ответ» через 3 дня.
        </p>
        <div className="row">
          <span className="label">Как входить</span>
          <button type="button" className="chip" aria-pressed={oauth} onClick={() => set({ smtp_auth: "oauth" })}>Через аккаунт (Google, Яндекс, Microsoft)</button>
          <button type="button" className="chip" aria-pressed={!oauth} onClick={() => set({ smtp_auth: "password" })}>Пароль приложения</button>
        </div>
        {oauth ? (
          <OAuthLogin />
        ) : (
          <>
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
          </>
        )}
        <div className="grid-3">
          {text("mail_from_name", "Имя отправителя", "Иван Петров", "Так письмо подписано в ящике получателя.")}
          {text("mail_from_email", "Адрес отправителя", oauth ? "пусто — адрес входа" : "пусто — логин", "Должен принадлежать этому ящику, иначе письма попадут в спам.")}
          {text("mail_reply_to", "Куда отвечать", "пусто — на адрес отправителя")}
        </div>
        <div className="row" style={{ alignItems: "center" }}>
          <button type="button" className="btn btn-sm" onClick={() => test.mutate()} disabled={test.isPending}>{test.isPending ? "Отправляю…" : "Отправить пробное письмо себе"}</button>
          {test.data && <span className="small st-pass">Ушло на {test.data.to} — проверьте ящик (и «Спам»).</span>}
          <ErrorLine error={test.error} />
        </div>
        <span className="hint">Проверяется сохранённая настройка — после правок сначала «Сохранить».</span>

        <span className="label">Правила рассылки</span>
        <p className="small muted" style={{ margin: 0 }}>
          Без спама: одно письмо на сайт и на адрес — по всем сканам, повторных писем нет. Сетям (точкам одного бренда или сайта) и госорганизациям (школы, поликлиники,
          исполкомы, сайты на gov.by, gov.uk и т. п.) рассылка не пишет. Правила проверяются при постановке в очередь и ещё раз перед отправкой.
        </p>
        <div className="grid-3">
          {num("mail_daily_limit", "Писем в день", "Новому ящику — 20–30, потом можно больше. Много сразу — письма начнут уходить в спам.", 1, 2000)}
          {num("mail_interval_min", "Пауза от, сек", "Между письмами — случайная пауза в этих пределах.", 10, 86400)}
          {num("mail_interval_max", "Пауза до, сек", "Например, 120–300: 2–5 минут.", 10, 86400)}
          {num("mail_window_start", "Рабочее время с, час", "По местному времени получателя — часовой пояс по городу скана.", 6, 21)}
          {num("mail_window_end", "Рабочее время до, час", "Только днём (6–22) и только по будням.", 7, 22)}
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
          <p className="sub">{s?.configured ? `Письма уходят с ${s.from} по одному${s.auth !== "password" ? " · вход через аккаунт" : ""}` : "Почта ещё не настроена"}</p>
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
            {s.failed ? ` · не ушло ${s.failed}` : ""} · по будням {s.window.start}:00–{s.window.end}:00 по времени получателя · пауза {fmtPause(s.interval.min, s.interval.max)}
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
    mutationFn: () => api.queueMail({ ids, source, skip_contacted: skipContacted }),
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
        Письмо уходит на почту компании (не на адрес для вакансий). Пропуск: без почты, без проверки сайта, если этому сайту или адресу уже писали (одно письмо на сайт),
        сетям и госорганизациям — причина видна в списке. Отправка начнётся после «Запустить рассылку» на странице <Link to="/mail">«Рассылка»</Link>; там же можно
        поправить любое письмо.
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
