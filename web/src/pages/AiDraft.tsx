// Нейросеть для писем на страницах компании и скана: черновики (с ходом рассуждения, запросом и ценой)
// и массовая генерация писем по списку компаний.
import { useEffect, useState, type CSSProperties } from "react";
import { Link } from "react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api, type AiBatch, type AiKind, type Message } from "../api";
import { fmtDate, fmtMs } from "../format";

/** «$0.0011», «бесплатно» (0), «—» (нет данных). */
export function fmtCost(v: number | null | undefined): string {
  if (v == null) return "—";
  if (v === 0) return "бесплатно";
  return `$${v < 0.01 ? v.toPrecision(2) : v.toFixed(v < 1 ? 3 : 2)}`;
}

/** Модель без префикса поставщика: «gemma-4-31b-it:free». */
function shortModel(id: string): string {
  return id.split("/").pop() ?? id;
}

const ROLE_LABEL: Record<string, string> = { system: "Инструкция (system)", user: "Запрос (user)", assistant: "Ответ (assistant)", developer: "Инструкция (developer)" };
const PRE: CSSProperties = { whiteSpace: "pre-wrap", margin: 0, padding: 10, background: "var(--surface-2)", borderRadius: 8, overflow: "auto" };

/** Запрос к модели по-человечески: сообщения — обычным текстом, остальные параметры — JSON. */
export function RequestView({ request, maxHeight = 360 }: { request: Record<string, unknown>; maxHeight?: number }) {
  const { messages, ...rest } = request as { messages?: { role: string; content: string }[] } & Record<string, unknown>;
  return (
    <div className="stack" style={{ gap: 8 }}>
      {(messages ?? []).map((m, i) => (
        <div key={i} className="stack" style={{ gap: 4 }}>
          <span className="small muted">{ROLE_LABEL[m.role] ?? m.role}</span>
          <pre className="small" style={{ ...PRE, maxHeight, fontFamily: "inherit" }}>{typeof m.content === "string" ? m.content : JSON.stringify(m.content, null, 2)}</pre>
        </div>
      ))}
      <div className="stack" style={{ gap: 4 }}>
        <span className="small muted">Параметры запроса</span>
        <pre className="mono" style={{ ...PRE, maxHeight: 260 }}>{JSON.stringify(rest, null, 2)}</pre>
      </div>
    </div>
  );
}

function ErrorLine({ error }: { error: Error | null }) {
  return error ? <span className="small st-fail" role="alert">{error.message}</span> : null;
}

/** Профили нейросети и выбранный по умолчанию — из настроек. */
function useAiProfiles() {
  const q = useQuery({ queryKey: ["settings"], queryFn: api.settings, staleTime: 30_000 });
  return { profiles: q.data?.ai_profiles ?? [], current: q.data?.ai_profile ?? "", keySet: !!q.data?.openrouter_key?.set, loaded: !!q.data };
}

/** Секундомер, пока идёт запрос: ответ с рассуждением может идти минуту. */
function useElapsed(active: boolean): number {
  const [s, setS] = useState(0);
  useEffect(() => {
    if (!active) return setS(0);
    const t0 = Date.now();
    const id = setInterval(() => setS(Math.round((Date.now() - t0) / 1000)), 1000);
    return () => clearInterval(id);
  }, [active]);
  return s;
}

/* ------------------------------------------------------------------ черновики на странице компании */

export function AiDraftPanel({
  businessId,
  kind,
  messages,
  activeId,
  onUse,
}: {
  businessId: number;
  kind: AiKind;
  messages: Message[];
  /** Черновик, который сейчас в поле текста. */
  activeId: number | null;
  onUse: (m: Message) => void;
}) {
  const qc = useQueryClient();
  const { profiles, current, keySet, loaded } = useAiProfiles();
  const [profileId, setProfileId] = useState("");
  const [showRequest, setShowRequest] = useState(false);
  const pid = profileId || current;
  const refresh = () => qc.invalidateQueries({ queryKey: ["business", businessId] });
  const gen = useMutation({
    mutationFn: () => api.createDraft(businessId, { kind, profile_id: pid || undefined }),
    onSuccess: (m) => {
      onUse(m);
      refresh();
    },
  });
  const preview = useQuery({
    queryKey: ["ai-preview", businessId, kind, pid],
    queryFn: () => api.aiPreview(businessId, { kind, profile_id: pid || undefined }),
    enabled: showRequest,
  });
  const del = useMutation({ mutationFn: (mid: number) => api.deleteMessage(mid), onSuccess: refresh });
  const elapsed = useElapsed(gen.isPending);
  const mine = messages.filter((m) => m.kind === kind && m.direction === "out");
  const profile = profiles.find((p) => p.id === pid);

  return (
    <div className="stack" style={{ gap: 8, borderTop: "1px solid var(--line)", paddingTop: 10 }}>
      <div className="row" style={{ alignItems: "center" }}>
        <b className="small">Нейросеть</b>
        {profiles.length > 1 && (
          <select className="select" style={{ width: 180, height: 30 }} value={pid} onChange={(e) => setProfileId(e.target.value)} aria-label="Профиль нейросети">
            {profiles.map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}
          </select>
        )}
        {profile && <span className="small faint mono" title={profile.model}>{shortModel(profile.model)}</span>}
        <span className="spacer" />
        <button type="button" className="btn btn-sm btn-ghost" aria-expanded={showRequest} onClick={() => setShowRequest((v) => !v)}>
          {showRequest ? "Скрыть запрос" : "Запрос к модели"}
        </button>
        <button type="button" className="btn btn-sm" onClick={() => gen.mutate()} disabled={gen.isPending || (loaded && !keySet)}>
          {gen.isPending ? `Составляю… ${elapsed} с` : kind === "email" ? "Составить письмо" : "Составить сообщение"}
        </button>
      </div>
      {loaded && !keySet && (
        <span className="small muted">
          Чтобы составлять тексты нейросетью, добавьте ключ OpenRouter в <Link to="/settings#ai">настройках</Link>.
        </span>
      )}
      <ErrorLine error={gen.error} />
      {showRequest && (
        <div className="stack" style={{ gap: 6 }}>
          {preview.isLoading && <span className="small muted">Собираю запрос…</span>}
          <ErrorLine error={preview.error} />
          {preview.data && (
            <>
              <span className="small muted">
                Ровно это уйдёт в OpenRouter (ключ — в заголовке, не здесь). Править — в Настройки → Нейросеть, профиль «{preview.data.profile.name}».
              </span>
              {preview.data.unknown_vars.length > 0 && <span className="small st-warn">Неизвестные переменные в шаблоне: {preview.data.unknown_vars.map((v) => `{{${v}}}`).join(", ")}</span>}
              {preview.data.unsupported.length > 0 && <span className="small st-warn">Модель не поддерживает и проигнорирует: {preview.data.unsupported.join(", ")}</span>}
              <RequestView request={preview.data.request} />
            </>
          )}
        </div>
      )}
      {mine.length > 0 && (
        <div className="stack" style={{ gap: 6 }}>
          {mine.map((m) => (
            <DraftItem key={m.id} m={m} active={m.id === activeId} onUse={() => onUse(m)} onDelete={() => del.mutate(m.id)} />
          ))}
        </div>
      )}
    </div>
  );
}

function DraftItem({ m, active, onUse, onDelete }: { m: Message; active: boolean; onUse: () => void; onDelete: () => void }) {
  const ai = m.ai;
  const lint = ai?.lint ?? [];
  const edited = !!ai && (ai.original.body !== m.body || (ai.original.subject || "") !== (m.subject || ""));
  return (
    <details style={{ border: "1px solid var(--line)", borderRadius: 8, padding: "6px 10px", background: active ? "var(--surface-2)" : undefined }}>
      <summary className="small" style={{ cursor: "pointer" }}>
        <span>{fmtDate(m.created_at)}</span>
        {" · "}
        <span className={m.status === "sent" ? "st-pass" : undefined}>{m.status === "sent" ? `отправлено ${fmtDate(m.sent_at)}` : "черновик"}</span>
        {ai && <span className="faint"> · {shortModel(ai.model || ai.model_requested)} · {fmtMs(ai.ms)} · {fmtCost(ai.cost)}</span>}
        {lint.length > 0 && <span className="st-warn"> · замечаний: {lint.length}</span>}
        {edited && <span className="faint"> · правлен</span>}
        {active && <span className="faint"> · в поле</span>}
      </summary>
      <div className="stack" style={{ gap: 8, marginTop: 8 }}>
        {m.subject && <div className="small"><b>Тема:</b> {m.subject}</div>}
        <div className="small" style={{ whiteSpace: "pre-wrap" }}>{m.body}</div>
        {lint.length > 0 && (
          <ul className="small st-warn" style={{ margin: 0, paddingLeft: 18 }}>
            {lint.map((x) => <li key={x}>{x}</li>)}
          </ul>
        )}
        {ai && (
          <span className="small faint">
            Профиль «{ai.profile_name}», модель {ai.model || ai.model_requested}{ai.provider ? ` (${ai.provider})` : ""}; токены: запрос {ai.usage.prompt}, ответ {ai.usage.completion}
            {ai.usage.reasoning ? `, из них рассуждение ${ai.usage.reasoning}` : ""}{ai.attempts > 1 ? `; попыток: ${ai.attempts}` : ""}
          </span>
        )}
        {ai?.reasoning && (
          <details>
            <summary className="small">Ход рассуждения</summary>
            <div className="small muted" style={{ whiteSpace: "pre-wrap", maxHeight: 320, overflow: "auto" }}>{ai.reasoning}</div>
          </details>
        )}
        {ai && (
          <details>
            <summary className="small">Запрос и ответ модели</summary>
            <div style={{ marginTop: 6 }}><RequestView request={ai.request} maxHeight={240} /></div>
            <div className="small muted" style={{ marginTop: 6 }}>Ответ модели как есть</div>
            <pre className="mono" style={{ ...PRE, maxHeight: 240 }}>{ai.content}</pre>
          </details>
        )}
        <div className="row">
          <button type="button" className="btn btn-sm" onClick={onUse} disabled={active}>{active ? "Уже в поле" : "Взять в текст"}</button>
          <span className="spacer" />
          <button type="button" className="btn btn-sm btn-ghost btn-danger" onClick={onDelete}>Удалить</button>
        </div>
      </div>
    </details>
  );
}

/* ------------------------------------------------------------------ массово на странице скана */

const KIND_LABEL: Record<AiKind, string> = { email: "Письма", short: "Сообщения для мессенджера" };

export function AiBatchBar({ items }: { items: { id: number; hasContacts: boolean }[] }) {
  const qc = useQueryClient();
  const { profiles, current, keySet, loaded } = useAiProfiles();
  const [open, setOpen] = useState(false);
  const [kind, setKind] = useState<AiKind>("email");
  const [profileId, setProfileId] = useState("");
  const [skip, setSkip] = useState(true);
  // Без контактов писать некуда, а бесплатный лимит (50 запросов в день) тратится — по умолчанию их не берём; галочку видно.
  const [withContacts, setWithContacts] = useState(true);
  const reachable = items.filter((x) => x.hasContacts).length;
  const ids = (withContacts ? items.filter((x) => x.hasContacts) : items).map((x) => x.id);
  const status = useQuery({
    queryKey: ["ai-batch"],
    queryFn: api.aiBatch,
    refetchInterval: (q) => (q.state.data?.running ? 1500 : false),
  });
  const b: AiBatch | undefined = status.data;
  const start = useMutation({
    mutationFn: () => api.startAiBatch({ ids, kind, profile_id: profileId || current || undefined, skip_existing: skip }),
    onSuccess: (d) => qc.setQueryData(["ai-batch"], d),
  });
  const stop = useMutation({ mutationFn: api.cancelAiBatch, onSuccess: (d) => qc.setQueryData(["ai-batch"], d) });
  const running = !!b?.running;
  const show = open || running;

  if (!show) {
    return (
      <button type="button" className="btn" onClick={() => setOpen(true)} disabled={items.length === 0} title="Составить нейросетью первое письмо или сообщение каждой компании из списка ниже">
        Письма нейросетью
      </button>
    );
  }
  const processed = b ? b.done + b.failed + b.skipped : 0;
  return (
    <div className="card card-pad stack" style={{ gap: 10, width: "100%" }}>
      <div className="row" style={{ alignItems: "center" }}>
        <h2 style={{ margin: 0, fontSize: 16 }}>Письма нейросетью</h2>
        <span className="spacer" />
        {!running && <button type="button" className="btn btn-sm btn-ghost" onClick={() => setOpen(false)}>Закрыть</button>}
      </div>
      {!running && (
        <>
          {loaded && !keySet && <span className="small st-warn">Нет ключа OpenRouter — добавьте его в <Link to="/settings#ai">настройках</Link>.</span>}
          <div className="row" style={{ alignItems: "center" }}>
            <select className="select" style={{ width: 230, height: 34 }} value={kind} onChange={(e) => setKind(e.target.value as AiKind)} aria-label="Что составить">
              <option value="email">Письма (тема и текст)</option>
              <option value="short">Сообщения для мессенджера</option>
            </select>
            {profiles.length > 1 && (
              <select className="select" style={{ width: 200, height: 34 }} value={profileId || current} onChange={(e) => setProfileId(e.target.value)} aria-label="Профиль нейросети">
                {profiles.map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}
              </select>
            )}
            <label className="small" style={{ display: "flex", gap: 6, alignItems: "center" }}>
              <input type="checkbox" checked={skip} onChange={(e) => setSkip(e.target.checked)} />
              пропускать компании, у которых такое уже есть
            </label>
            <label className="small" style={{ display: "flex", gap: 6, alignItems: "center" }}>
              <input type="checkbox" checked={withContacts} onChange={(e) => setWithContacts(e.target.checked)} />
              только с контактами ({reachable} из {items.length})
            </label>
            <span className="spacer" />
            <button type="button" className="btn btn-primary" onClick={() => start.mutate()} disabled={start.isPending || ids.length === 0 || (loaded && !keySet)}>
              Составить для {ids.length}
            </button>
          </div>
          <span className="small faint">
            Берутся компании из списка ниже с учётом фильтров. Компании без проверки, закрытые для сканера, и точки сетей пропускаются. Черновики появятся на страницах компаний; отправка
            пока вручную.
          </span>
          <ErrorLine error={start.error} />
        </>
      )}
      {b && b.total > 0 && (
        <div className="stack" style={{ gap: 6 }}>
          <div className="progress" aria-label="Ход генерации"><span style={{ width: `${Math.round((processed / Math.max(1, b.total)) * 100)}%` }} /></div>
          <div className="row small" style={{ alignItems: "center" }}>
            <span>
              {KIND_LABEL[b.kind as AiKind] ?? b.kind}: составлено {b.done} из {b.total}
              {b.skipped ? `, пропущено ${b.skipped}` : ""}
              {b.failed ? `, ошибок ${b.failed}` : ""} · {fmtCost(b.cost)} · {b.profile} ({shortModel(b.model)})
            </span>
            <span className="spacer" />
            {running && <button type="button" className="btn btn-sm" onClick={() => stop.mutate()} disabled={stop.isPending}>Остановить</button>}
          </div>
          {b.stop_reason && <span className="small st-warn">{b.stop_reason}</span>}
          {!running && b.finished_at && <span className="small faint">Готово {fmtDate(b.finished_at)}.</span>}
          {b.errors.length > 0 && (
            <details>
              <summary className="small">Пропуски и ошибки ({b.errors.length})</summary>
              <ul className="small" style={{ margin: "6px 0 0", paddingLeft: 18 }}>
                {b.errors.map((e, i) => (
                  <li key={`${e.id}-${i}`} className={e.skipped ? "muted" : "st-fail"}>
                    <Link to={`/businesses/${e.id}`}>{e.name}</Link>: {e.error}
                  </li>
                ))}
              </ul>
            </details>
          )}
        </div>
      )}
    </div>
  );
}
