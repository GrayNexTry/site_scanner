// Рабочее место менеджера на странице компании: работа с клиентом, контакты, первое сообщение,
// журнал касаний, план работ и КП. Каждый блок сам ходит в API и обновляет кэш.
import { useMemo, useState } from "react";
import { Link } from "react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  api,
  type Activity,
  type ActivityKind,
  type Business,
  type BusinessPatch,
  type Channel,
  type Contact,
  type LeadStatus,
  type Message,
  type Outreach,
  type Proposal,
} from "../api";
import { ConfirmButton, useToast } from "../components";
import { AiDraftPanel } from "./AiDraft";
import { QueueEmail } from "./Mail";
import {
  ACTIVITY_LABEL,
  CHANNEL_LABEL,
  contactAction,
  CONTACT_LABEL,
  CONTACT_SOURCE_LABEL,
  copyText,
  FUNNEL,
  fmtDate,
  fmtMoney,
  hostOf,
  isOverdue,
  LEAD_STATUS_LABEL,
  plural,
  todayStr,
} from "../format";

const CURRENCIES = ["BYN", "RUB", "USD", "EUR", "GBP", "PLN"];

/** Обновить всё, что показывает эту компанию. */
function useRefresh(b: Business) {
  const qc = useQueryClient();
  return () => {
    qc.invalidateQueries({ queryKey: ["business", b.id] });
    qc.invalidateQueries({ queryKey: ["businesses", b.scan_id] });
    qc.invalidateQueries({ queryKey: ["leads"] });
  };
}

function ErrorLine({ error }: { error: Error | null }) {
  return error ? <span className="small st-fail" role="alert">{error.message}</span> : null;
}

function parseNum(s: string): number | null {
  const t = s.trim().replace(/\s/g, "").replace(",", ".");
  if (!t) return null;
  const n = Number(t);
  return Number.isFinite(n) ? n : null;
}

/* ------------------------------------------------------------------ Работа с клиентом */

const STATUS_ORDER: LeadStatus[] = ["new", "contacted", "talking", "proposal", "client", "rejected", "skip"];

export function WorkCard({ b, clientName, clientAddress, nameSuggestion }: { b: Business; clientName?: string; clientAddress?: string | null; nameSuggestion?: string | null }) {
  const refresh = useRefresh(b);
  const settings = useQuery({ queryKey: ["settings"], queryFn: api.settings, staleTime: 60_000 });
  const [notes, setNotes] = useState(b.notes ?? "");
  const [step, setStep] = useState(b.next_action ?? "");
  const [person, setPerson] = useState(b.contact_person ?? "");
  const [dispName, setDispName] = useState(b.display_name ?? "");
  const [dispAddr, setDispAddr] = useState(b.display_address ?? "");
  const [price, setPrice] = useState(b.deal_price != null ? String(b.deal_price) : "");
  const save = useMutation({ mutationFn: (p: BusinessPatch) => api.patchBusiness(b.id, p), onSuccess: refresh });
  const currency = b.deal_currency ?? settings.data?.currency ?? "BYN";
  const overdue = isOverdue(b.next_action_at);
  const setDate = (d: string) => save.mutate({ next_action_at: d });

  return (
    <div className="card card-pad stack">
      <div className="row" style={{ alignItems: "baseline" }}>
        <h2 style={{ margin: 0 }}>Работа с клиентом</h2>
        <span className="spacer" />
        <span className="small faint">{save.isPending ? "Сохраняю…" : b.status_at ? `Статус изменён ${fmtDate(b.status_at)}` : ""}</span>
      </div>

      <div className="stack" style={{ gap: 6 }}>
        <span className="label" id="st-label">Этап</span>
        <div className="chips" role="radiogroup" aria-labelledby="st-label">
          {STATUS_ORDER.map((st) => (
            <button
              key={st}
              type="button"
              className="chip"
              role="radio"
              aria-checked={b.lead_status === st}
              aria-pressed={b.lead_status === st}
              onClick={() => b.lead_status !== st && save.mutate({ lead_status: st })}
            >
              {LEAD_STATUS_LABEL[st]}
            </button>
          ))}
        </div>
      </div>

      <div className="stack" style={{ gap: 6 }}>
        <span className="label">Следующий шаг {overdue && <b className="st-fail">· просрочен</b>}</span>
        <div className="row">
          <input
            type="date"
            className={`input${overdue ? " st-fail" : ""}`}
            style={{ width: 160 }}
            value={b.next_action_at ?? ""}
            onChange={(e) => e.target.value && setDate(e.target.value)}
            aria-label="Дата следующего шага"
          />
          <input
            className="input"
            style={{ flex: 1, minWidth: 180, width: "auto" }}
            placeholder="Что сделать: написать повторно, позвонить…"
            value={step}
            onChange={(e) => setStep(e.target.value)}
            onBlur={() => step !== (b.next_action ?? "") && save.mutate({ next_action: step })}
            onKeyDown={(e) => e.key === "Enter" && (e.target as HTMLInputElement).blur()}
            aria-label="Что сделать на следующем шаге"
          />
        </div>
        <div className="row" style={{ gap: 6 }}>
          <button type="button" className="btn btn-sm" onClick={() => setDate(todayStr(1))}>Завтра</button>
          <button type="button" className="btn btn-sm" onClick={() => setDate(todayStr(3))}>Через 3 дня</button>
          <button type="button" className="btn btn-sm" onClick={() => setDate(todayStr(7))}>Через неделю</button>
          {b.next_action_at && <button type="button" className="btn btn-sm btn-ghost" onClick={() => setDate("")}>Сбросить дату</button>}
        </div>
      </div>

      <div className="grid-2" style={{ gap: 12 }}>
        <div className="field">
          <label htmlFor="deal-price">Цена сделки</label>
          <div className="row" style={{ flexWrap: "nowrap" }}>
            <input
              id="deal-price"
              className="input"
              inputMode="decimal"
              placeholder="не назначена"
              value={price}
              onChange={(e) => setPrice(e.target.value)}
              onBlur={() => {
                const n = parseNum(price);
                if (n !== (b.deal_price ?? null)) save.mutate({ deal_price: n });
              }}
              onKeyDown={(e) => e.key === "Enter" && (e.target as HTMLInputElement).blur()}
            />
            <select
              className="select"
              style={{ width: 90, height: 34 }}
              value={currency}
              onChange={(e) => save.mutate({ deal_currency: e.target.value })}
              aria-label="Валюта"
            >
              {(CURRENCIES.includes(currency) ? CURRENCIES : [currency, ...CURRENCIES]).map((c) => <option key={c}>{c}</option>)}
            </select>
          </div>
        </div>
        <div className="field">
          <label htmlFor="contact-person">Контактное лицо</label>
          <input
            id="contact-person"
            className="input"
            placeholder="Имя, должность"
            value={person}
            onChange={(e) => setPerson(e.target.value)}
            onBlur={() => person !== (b.contact_person ?? "") && save.mutate({ contact_person: person })}
            onKeyDown={(e) => e.key === "Enter" && (e.target as HTMLInputElement).blur()}
          />
        </div>
      </div>

      <div className="grid-2" style={{ gap: 12 }}>
        <div className="field">
          <label htmlFor="disp-name">Название для документов</label>
          <input
            id="disp-name"
            className="input"
            placeholder={clientName ?? b.name}
            value={dispName}
            onChange={(e) => setDispName(e.target.value)}
            onBlur={() => dispName.trim() !== (b.display_name ?? "") && save.mutate({ display_name: dispName.trim() })}
            onKeyDown={(e) => e.key === "Enter" && (e.target as HTMLInputElement).blur()}
          />
          {nameSuggestion && nameSuggestion !== (b.display_name ?? "") && (
            <button
              type="button"
              className="btn btn-sm"
              style={{ alignSelf: "flex-start" }}
              onClick={() => {
                setDispName(nameSuggestion);
                save.mutate({ display_name: nameSuggestion });
              }}
            >
              Взять с сайта: «{nameSuggestion}»
            </button>
          )}
        </div>
        <div className="field">
          <label htmlFor="disp-addr">Адрес для документов</label>
          <input
            id="disp-addr"
            className="input"
            placeholder={clientAddress ?? b.address ?? ""}
            value={dispAddr}
            onChange={(e) => setDispAddr(e.target.value)}
            onBlur={() => dispAddr.trim() !== (b.display_address ?? "") && save.mutate({ display_address: dispAddr.trim() })}
            onKeyDown={(e) => e.key === "Enter" && (e.target as HTMLInputElement).blur()}
          />
        </div>
      </div>

      <div className="field">
        <label htmlFor="biz-notes">Заметки</label>
        <textarea
          id="biz-notes"
          className="input"
          rows={3}
          placeholder="С кем говорили, что обещали, когда перезвонить…"
          value={notes}
          onChange={(e) => setNotes(e.target.value)}
          onBlur={() => notes !== (b.notes ?? "") && save.mutate({ notes })}
        />
      </div>
      <ErrorLine error={save.error} />
    </div>
  );
}

/* ------------------------------------------------------------------ Контакты */

const MANUAL_KINDS = ["phone", "email", "telegram", "whatsapp", "viber", "instagram", "vk", "facebook", "link"];

function shortValue(c: Contact): string {
  if (c.kind === "phone" || c.kind === "email") return c.value;
  const h = c.value.replace(/^[a-z][a-z0-9+.-]*:\/\//i, "").replace(/^www\./i, "").replace(/\/$/, "");
  return h || hostOf(c.value);
}

/** Заметное предупреждение: у компании нет ни одного контакта — сообщению некуда уйти. */
function NoContactsWarning() {
  return (
    <div className="banner warn small" role="alert" style={{ margin: "0 0 8px" }}>
      <b>Контактов нет — написать некуда; найдите их вручную и добавьте.</b>
    </div>
  );
}

export function ContactsCard({ b, contacts }: { b: Business; contacts: Contact[] }) {
  const refresh = useRefresh(b);
  const [toast, show] = useToast();
  const [kind, setKind] = useState("phone");
  const [value, setValue] = useState("");
  const [note, setNote] = useState("");
  const save = useMutation({
    mutationFn: (list: { kind: string; value: string; note?: string }[]) => api.patchBusiness(b.id, { extra_contacts: list.length ? list : null }),
    onSuccess: () => {
      setValue("");
      setNote("");
      refresh();
    },
  });
  const manual = b.extra_contacts ?? [];
  const add = () => {
    const v = value.trim();
    if (!v) return;
    save.mutate([...manual, { kind, value: v, ...(note.trim() ? { note: note.trim() } : {}) }]);
  };
  const remove = (c: Contact) => {
    const i = manual.findIndex((m) => m.kind === c.kind && m.value === c.value);
    if (i >= 0) save.mutate(manual.filter((_, j) => j !== i));
  };

  return (
    <div className="card">
      <div className="card-head">
        <h2>Контакты</h2>
        <span className="small faint">{contacts.length}</span>
      </div>
      {contacts.length === 0 && (
        <div className="card-pad">
          <NoContactsWarning />
          <div className="small muted">Контактов не найдено ни в карточках, ни на сайте. Добавьте вручную ниже.</div>
        </div>
      )}
      <ul className="contacts">
        {contacts.map((c, i) => (
          <li key={`${c.kind}-${c.value}-${i}`} className="contact">
            <div className="contact-main">
              <span className="small muted">{CONTACT_LABEL[c.kind] ?? c.kind}</span>
              <span className="contact-val" title={c.value}>{shortValue(c)}</span>
              {c.note && <span className={`small ${c.note === "вакансии" ? "contact-note" : "faint"}`}>{c.note === "вакансии" ? "· вакансии" : c.note}</span>}
            </div>
            <span className="small faint contact-src">{CONTACT_SOURCE_LABEL[c.source] ?? c.source}</span>
            <div className="contact-actions">
              <button
                type="button"
                className="btn btn-sm"
                onClick={() => copyText(c.value).then((ok) => show(ok ? "Скопировано" : "Не удалось скопировать"))}
              >
                Скопировать
              </button>
              {c.url && (
                <a className="btn btn-sm" href={c.url} target={c.url.startsWith("http") ? "_blank" : undefined} rel="noreferrer">
                  {contactAction(c.kind)}
                </a>
              )}
              {c.source === "manual" && (
                <ConfirmButton className="btn btn-sm btn-ghost btn-danger" confirm="Точно?" onConfirm={() => remove(c)}>
                  Удалить
                </ConfirmButton>
              )}
            </div>
          </li>
        ))}
      </ul>
      <form
        className="card-pad stack"
        style={{ borderTop: "1px solid var(--line)", gap: 8 }}
        onSubmit={(e) => {
          e.preventDefault();
          add();
        }}
      >
        <span className="label">Добавить контакт</span>
        <div className="row">
          <select className="select" style={{ width: 130, height: 34 }} value={kind} onChange={(e) => setKind(e.target.value)} aria-label="Вид контакта">
            {MANUAL_KINDS.map((k) => <option key={k} value={k}>{CONTACT_LABEL[k] ?? k}</option>)}
          </select>
          <input className="input" style={{ flex: 1, minWidth: 160, width: "auto" }} placeholder="Номер, ник или ссылка" value={value} onChange={(e) => setValue(e.target.value)} aria-label="Значение контакта" />
        </div>
        <div className="row">
          <input className="input" style={{ flex: 1, minWidth: 160, width: "auto" }} placeholder="Пометка: чей номер, когда звонить (необязательно)" value={note} onChange={(e) => setNote(e.target.value)} aria-label="Пометка к контакту" />
          <button className="btn" type="submit" disabled={!value.trim() || save.isPending}>Добавить</button>
        </div>
        <ErrorLine error={save.error} />
      </form>
      {toast}
    </div>
  );
}

/* ------------------------------------------------------------------ Первое сообщение */

type Tab = "messenger" | "email";
const MESSENGER_KINDS: Channel[] = ["whatsapp", "telegram", "viber", "instagram", "vk", "facebook"];

export function OutreachCard({ b, outreach, contacts, messages }: { b: Business; outreach: Outreach | null; contacts: Contact[]; messages: Message[] }) {
  const refresh = useRefresh(b);
  const [toast, show] = useToast();
  const firstMessenger = (MESSENGER_KINDS.find((k) => contacts.some((c) => c.kind === k)) ?? "other") as Channel;
  const [tab, setTab] = useState<Tab>("messenger");
  const [short, setShort] = useState(outreach?.short ?? "");
  const [subject, setSubject] = useState(outreach?.email_subject ?? "");
  const [body, setBody] = useState(outreach?.email_body ?? "");
  const [chan, setChan] = useState<Record<Tab, Channel>>({ messenger: firstMessenger, email: "email" });
  // Черновик нейросети в поле: при «отправлено» сохраняем итоговый текст и помечаем черновик отправленным.
  const [draft, setDraft] = useState<Record<Tab, number | null>>({ messenger: null, email: null });
  const takeDraft = (m: Message) => {
    if (m.kind === "short") {
      setShort(m.body);
      setDraft((d) => ({ ...d, messenger: m.id }));
    } else {
      setSubject(m.subject ?? "");
      setBody(m.body);
      setDraft((d) => ({ ...d, email: m.id }));
    }
  };
  const sent = useMutation({
    mutationFn: () =>
      api.markSent(b.id, {
        channel: chan[tab],
        text: tab === "messenger" ? short : `${subject}\n\n${body}`,
        message_id: draft[tab] ?? undefined,
        subject: tab === "email" ? subject : undefined,
        body: tab === "messenger" ? short : body,
      }),
    onSuccess: () => {
      refresh();
      show("Записано в журнал, шаг назначен");
    },
  });
  const copy = (text: string, what: string) => copyText(text).then((ok) => show(ok ? `${what} скопировано` : "Не удалось скопировать"));

  if (!outreach) {
    return (
      <div className="card card-pad stack" id="outreach">
        <h2 style={{ margin: 0 }}>Первое сообщение</h2>
        {contacts.length === 0 && <NoContactsWarning />}
        <p className="small muted">
          Писать пока не о чем: по сайту не нашлось подтверждённых проблем, которые можно назвать клиенту. Если проверка ещё не прошла, дождитесь её
          или нажмите «Перепроверить».
        </p>
      </div>
    );
  }

  return (
    <div className="card card-pad stack" id="outreach">
      {contacts.length === 0 && <NoContactsWarning />}
      <div className="row" style={{ alignItems: "baseline" }}>
        <h2 style={{ margin: 0 }}>Первое сообщение</h2>
        <span className="small faint">{outreach.lang === "ru" ? "по-русски" : "по-английски"}</span>
        <span className="spacer" />
        <div className="chips" role="tablist" aria-label="Вид сообщения">
          <button type="button" className="chip" role="tab" aria-selected={tab === "messenger"} aria-pressed={tab === "messenger"} onClick={() => setTab("messenger")}>Мессенджер</button>
          <button type="button" className="chip" role="tab" aria-selected={tab === "email"} aria-pressed={tab === "email"} onClick={() => setTab("email")}>Письмо</button>
        </div>
      </div>

      {tab === "messenger" ? (
        <textarea className="input" rows={10} value={short} onChange={(e) => setShort(e.target.value)} aria-label="Текст сообщения для мессенджера" />
      ) : (
        <>
          <input className="input" value={subject} onChange={(e) => setSubject(e.target.value)} aria-label="Тема письма" />
          <textarea className="input" rows={10} value={body} onChange={(e) => setBody(e.target.value)} aria-label="Текст письма" />
          <QueueEmail
            businessId={b.id}
            contacts={contacts}
            draftId={draft.email}
            subject={subject}
            body={body}
            onQueued={(mid) => setDraft((d) => ({ ...d, email: mid }))}
          />
        </>
      )}

      <div className="row">
        {tab === "messenger" ? (
          <button type="button" className="btn btn-primary" onClick={() => copy(short, "Сообщение")}>Скопировать</button>
        ) : (
          <>
            <button type="button" className="btn btn-primary" onClick={() => copy(body, "Письмо")}>Скопировать письмо</button>
            <button type="button" className="btn" onClick={() => copy(subject, "Тема")}>Скопировать тему</button>
          </>
        )}
        <span className="spacer" />
        <select
          className="select"
          style={{ width: 150, height: 34 }}
          value={chan[tab]}
          onChange={(e) => setChan((c) => ({ ...c, [tab]: e.target.value as Channel }))}
          aria-label="Канал, которым отправлено"
        >
          {(Object.keys(CHANNEL_LABEL) as Channel[]).map((c) => <option key={c} value={c}>{CHANNEL_LABEL[c]}</option>)}
        </select>
        <button type="button" className="btn" onClick={() => sent.mutate()} disabled={sent.isPending}>
          {sent.isPending ? "Записываю…" : "Отметить как отправленное"}
        </button>
      </div>
      <span className="small faint">
        «Отметить как отправленное» запишет касание в журнал, переведёт «Нового» в «Написали» и поставит шаг «Проверить ответ» через 3 дня.
      </span>
      <ErrorLine error={sent.error} />
      <AiDraftPanel businessId={b.id} kind={tab === "messenger" ? "short" : "email"} messages={messages} activeId={draft[tab]} onUse={takeDraft} />
      {toast}
    </div>
  );
}

/* ------------------------------------------------------------------ Журнал */

const QUICK: { kind: ActivityKind; label: string }[] = [
  { kind: "message", label: "Написали" },
  { kind: "reply", label: "Ответили" },
  { kind: "call", label: "Звонок" },
  { kind: "meeting", label: "Встреча" },
  { kind: "proposal", label: "Отправили КП" },
  { kind: "note", label: "Заметка" },
];

export function ActivityCard({ b, activities }: { b: Business; activities: Activity[] }) {
  const refresh = useRefresh(b);
  const [kind, setKind] = useState<ActivityKind | null>(null);
  const [channel, setChannel] = useState<Channel | "">("");
  const [text, setText] = useState("");
  const add = useMutation({
    mutationFn: async () => {
      if (!kind) return;
      await api.addActivity(b.id, { kind, channel: channel || undefined, text: text.trim() || undefined });
      // КП отправлено — статус не должен отставать, но и не откатываться назад по воронке.
      const at = FUNNEL.indexOf(b.lead_status);
      if (kind === "proposal" && at >= 0 && at < FUNNEL.indexOf("proposal")) await api.patchBusiness(b.id, { lead_status: "proposal" });
    },
    onSuccess: () => {
      setKind(null);
      setChannel("");
      setText("");
      refresh();
    },
  });
  const del = useMutation({ mutationFn: (id: number) => api.deleteActivity(id), onSuccess: refresh });

  return (
    <div className="card">
      <div className="card-head">
        <h2>Журнал</h2>
        <span className="small faint">{activities.length}</span>
      </div>
      <div className="card-pad stack" style={{ gap: 10 }}>
        <div className="chips" role="group" aria-label="Добавить запись">
          {QUICK.map((q) => (
            <button key={q.kind} type="button" className="chip" aria-pressed={kind === q.kind} onClick={() => setKind(kind === q.kind ? null : q.kind)}>
              {q.label}
            </button>
          ))}
        </div>
        {kind && (
          <form
            className="stack"
            style={{ gap: 8 }}
            onSubmit={(e) => {
              e.preventDefault();
              add.mutate();
            }}
          >
            <div className="row">
              <b className="small">{ACTIVITY_LABEL[kind]}</b>
              {kind !== "note" && (
                <select className="select" style={{ width: 150, height: 34 }} value={channel} onChange={(e) => setChannel(e.target.value as Channel | "")} aria-label="Канал">
                  <option value="">Канал не указан</option>
                  {(Object.keys(CHANNEL_LABEL) as Channel[]).map((c) => <option key={c} value={c}>{CHANNEL_LABEL[c]}</option>)}
                </select>
              )}
            </div>
            <textarea
              className="input"
              rows={2}
              autoFocus
              placeholder={kind === "note" ? "Что важно запомнить" : "Что сказали или решили (необязательно)"}
              value={text}
              onChange={(e) => setText(e.target.value)}
              aria-label="Текст записи"
            />
            <div className="row">
              <button className="btn btn-primary" type="submit" disabled={add.isPending || (kind === "note" && !text.trim())}>{add.isPending ? "Записываю…" : "Записать"}</button>
              <button className="btn btn-ghost" type="button" onClick={() => setKind(null)}>Отмена</button>
              {kind === "proposal" && FUNNEL.indexOf(b.lead_status) >= 0 && FUNNEL.indexOf(b.lead_status) < FUNNEL.indexOf("proposal") && (
                <span className="small faint">Этап сменится на «КП отправлено»</span>
              )}
            </div>
          </form>
        )}
        <ErrorLine error={add.error ?? del.error} />
      </div>
      {activities.length === 0 ? (
        <div className="card-pad small muted" style={{ borderTop: "1px solid var(--line)" }}>Касаний пока нет. Они появятся, когда отметите отправленное сообщение или добавите запись.</div>
      ) : (
        <ul className="feed">
          {activities.map((a) => (
            <li key={a.id} className={`feed-item${a.kind === "status" || a.kind === "price" ? " sys" : ""}`}>
              <div className="feed-meta small">
                <b>{ACTIVITY_LABEL[a.kind] ?? a.kind}</b>
                {a.channel && <span className="muted">· {CHANNEL_LABEL[a.channel] ?? a.channel}</span>}
                <span className="faint">{fmtDate(a.at)}</span>
              </div>
              {a.text && <div className="feed-text small">{a.text}</div>}
              <ConfirmButton className="btn btn-sm btn-ghost btn-danger feed-del" confirm="Точно?" onConfirm={() => del.mutate(a.id)}>
                Удалить
              </ConfirmButton>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

/* ------------------------------------------------------------------ План работ и КП */

interface Draft {
  enabled: Record<string, boolean>;
  price: Record<string, string>;
  extra: { title: string; price: string }[];
  discount: string;
  note: string;
  valid: string;
}

function draftOf(p: Proposal): Draft {
  return {
    enabled: Object.fromEntries(p.packages.map((x) => [x.id, x.enabled])),
    price: Object.fromEntries(p.packages.map((x) => [x.id, String(x.price)])),
    extra: p.extra.map((e) => ({ title: e.title, price: String(e.price) })),
    discount: p.discount ? String(p.discount) : "",
    note: p.note ?? "",
    valid: String(p.valid_days ?? ""),
  };
}

export function ProposalCard({ b, proposal }: { b: Business; proposal: Proposal | null }) {
  if (!proposal) {
    return (
      <div className="card card-pad stack">
        <h2 style={{ margin: 0 }}>План работ и КП</h2>
        <p className="small muted">План появится после проверки сайта: он собирается из найденных проблем.</p>
      </div>
    );
  }
  // key — чтобы после сохранения черновик пересобрался из ответа сервера.
  return <ProposalEditor key={JSON.stringify(proposal)} b={b} proposal={proposal} />;
}

function ProposalEditor({ b, proposal }: { b: Business; proposal: Proposal }) {
  const refresh = useRefresh(b);
  const settings = useQuery({ queryKey: ["settings"], queryFn: api.settings, staleTime: 60_000 });
  const initial = useMemo(() => JSON.stringify(draftOf(proposal)), [proposal]);
  const [d, setD] = useState<Draft>(() => draftOf(proposal));
  const dirty = JSON.stringify(d) !== initial;
  const cur = proposal.currency;

  const sum = proposal.packages.reduce((s, p) => s + (d.enabled[p.id] ? parseNum(d.price[p.id] ?? "") ?? 0 : 0), 0);
  const extraSum = d.extra.reduce((s, e) => s + (parseNum(e.price) ?? 0), 0);
  const discount = parseNum(d.discount) ?? 0;
  const total = Math.max(0, sum + extraSum - discount);

  const payload = () => ({
    packages: proposal.packages.map((p) => ({ id: p.id, enabled: !!d.enabled[p.id], price: parseNum(d.price[p.id] ?? "") ?? p.price })),
    extra: d.extra.filter((e) => e.title.trim()).map((e) => ({ title: e.title.trim(), price: parseNum(e.price) ?? 0 })),
    discount,
    note: d.note,
    ...(parseNum(d.valid) != null ? { valid_days: Math.round(parseNum(d.valid)!) } : {}),
  });
  const save = useMutation({ mutationFn: () => api.patchBusiness(b.id, { proposal: payload() }), onSuccess: refresh });
  const toDeal = useMutation({
    mutationFn: () => api.patchBusiness(b.id, { proposal: payload(), deal_price: total, deal_currency: cur }),
    onSuccess: refresh,
  });
  const reset = useMutation({ mutationFn: () => api.patchBusiness(b.id, { proposal: null }), onSuccess: refresh });

  const setPkg = (id: string, patch: Partial<{ enabled: boolean; price: string }>) =>
    setD((x) => ({
      ...x,
      enabled: patch.enabled === undefined ? x.enabled : { ...x.enabled, [id]: patch.enabled },
      price: patch.price === undefined ? x.price : { ...x.price, [id]: patch.price },
    }));
  const picked = proposal.packages.filter((p) => d.enabled[p.id]).length;

  return (
    <div className="card">
      <div className="card-head">
        <h2>План работ и КП</h2>
        <span className="small faint">выбрано {picked} из {proposal.packages.length}</span>
      </div>
      <div className="card-pad stack" style={{ gap: 14 }}>
        <p className="small">
          <b>КП {proposal.lang === "en" ? "на английском" : "на русском"}, {cur}</b>
        </p>
        {proposal.prices_unverified && (
          <p className="small st-warn">
            Для {cur} своих цен нет — показаны цены прайса {settings.data?.currency ?? "BYN"} как есть; задайте цены в Настройках → Прайс.
          </p>
        )}
        <p className="small muted">
          Пакеты собраны из найденных проблем; включённые по умолчанию — там, где есть критичные. Отметьте нужное, поправьте цену и сохраните — КП и PDF
          строятся из сохранённого.
        </p>
        <div className="pkgs">
          {proposal.packages.map((p) => {
            const on = !!d.enabled[p.id];
            const fails = p.items.filter((i) => i.level === "fail");
            const ordered = [...fails, ...p.items.filter((i) => i.level === "warn")].filter((i, n, all) => all.findIndex((y) => y.text === i.text) === n);
            return (
              <div key={p.id} className={`pkg${on ? "" : " off"}`}>
                <div className="pkg-head">
                  <label className="pkg-title">
                    <input type="checkbox" checked={on} onChange={(e) => setPkg(p.id, { enabled: e.target.checked })} />
                    <span>
                      <b>{p.title}</b>
                      <span className="small muted pkg-sum">{p.summary}</span>
                    </span>
                  </label>
                  <div className="pkg-price">
                    <input
                      className="input"
                      inputMode="decimal"
                      value={d.price[p.id] ?? ""}
                      onChange={(e) => setPkg(p.id, { price: e.target.value })}
                      aria-label={`Цена: ${p.title}`}
                    />
                    <span className="small muted">{cur}</span>
                  </div>
                </div>
                {ordered.length > 0 && (
                  <details className="pkg-items" open={ordered.length <= 4}>
                    <summary className="small muted">
                      Что сделаем: {ordered.length} {plural(ordered.length, "пункт", "пункта", "пунктов")}
                      {fails.length > 0 && <span className="st-fail"> · критичных {fails.length}</span>}
                    </summary>
                    <ul className="small">
                      {ordered.map((i) => (
                        <li key={i.id} className={i.level === "warn" ? "faint" : ""}>
                          {i.text}
                        </li>
                      ))}
                    </ul>
                  </details>
                )}
              </div>
            );
          })}
        </div>

        <div className="stack" style={{ gap: 8 }}>
          <span className="label">Дополнительные строки</span>
          {d.extra.map((e, i) => (
            <div key={i} className="row" style={{ flexWrap: "nowrap" }}>
              <input
                className="input"
                placeholder="Что ещё: например, фотосъёмка"
                value={e.title}
                onChange={(ev) => setD((x) => ({ ...x, extra: x.extra.map((r, j) => (j === i ? { ...r, title: ev.target.value } : r)) }))}
                aria-label={`Название дополнительной строки ${i + 1}`}
              />
              <input
                className="input"
                style={{ width: 110, flex: "none" }}
                inputMode="decimal"
                placeholder="Цена"
                value={e.price}
                onChange={(ev) => setD((x) => ({ ...x, extra: x.extra.map((r, j) => (j === i ? { ...r, price: ev.target.value } : r)) }))}
                aria-label={`Цена дополнительной строки ${i + 1}`}
              />
              <button type="button" className="btn btn-sm btn-ghost btn-danger" onClick={() => setD((x) => ({ ...x, extra: x.extra.filter((_, j) => j !== i) }))} aria-label={`Убрать строку ${i + 1}`}>✕</button>
            </div>
          ))}
          <div><button type="button" className="btn btn-sm" onClick={() => setD((x) => ({ ...x, extra: [...x.extra, { title: "", price: "" }] }))}>Добавить строку</button></div>
        </div>

        <div className="grid-2" style={{ gap: 12 }}>
          <div className="field">
            <label htmlFor="kp-discount">Скидка, {cur}</label>
            <input id="kp-discount" className="input" inputMode="decimal" placeholder="0" value={d.discount} onChange={(e) => setD((x) => ({ ...x, discount: e.target.value }))} />
          </div>
          <div className="field">
            <label htmlFor="kp-valid">Срок действия КП, дней</label>
            <input id="kp-valid" className="input" inputMode="numeric" value={d.valid} onChange={(e) => setD((x) => ({ ...x, valid: e.target.value }))} />
          </div>
        </div>
        <div className="field">
          <label htmlFor="kp-note">Примечание для клиента (в КП)</label>
          <textarea id="kp-note" className="input" rows={2} value={d.note} onChange={(e) => setD((x) => ({ ...x, note: e.target.value }))} placeholder="Сроки, условия оплаты, что не входит" />
        </div>

        <dl className="kv totals">
          <dt>Пакеты и строки</dt><dd className="num">{fmtMoney(sum + extraSum, cur)}</dd>
          {discount > 0 && (<><dt>Скидка</dt><dd className="num">−{fmtMoney(discount, cur)}</dd></>)}
          <dt><b>Итого</b></dt><dd className="num"><b>{fmtMoney(total, cur)}</b></dd>
        </dl>

        <div className="row">
          <button type="button" className="btn btn-primary" onClick={() => save.mutate()} disabled={!dirty || save.isPending}>
            {save.isPending ? "Сохраняю…" : "Сохранить"}
          </button>
          <button type="button" className="btn" onClick={() => toDeal.mutate()} disabled={toDeal.isPending}>Цену КП → в цену сделки</button>
          {b.proposal != null && (
            <ConfirmButton className="btn btn-ghost btn-danger" confirm="Сбросить правки?" onConfirm={() => reset.mutate()}>Сбросить правки</ConfirmButton>
          )}
          {dirty && <span className="small st-warn">Есть несохранённые правки — КП и PDF их пока не видят</span>}
        </div>
        <div className="row">
          <Link className="btn" to={`/proposal/${b.id}`} target="_blank">Открыть КП</Link>
          <a className="btn" href={`/api/businesses/${b.id}/proposal.pdf`}>Скачать КП (PDF)</a>
          <a className="btn" href={`/api/businesses/${b.id}/report.pdf`}>Скачать отчёт (PDF)</a>
        </div>
        <ErrorLine error={save.error ?? toDeal.error ?? reset.error} />
      </div>
    </div>
  );
}
