// Настройки → Нейросеть: ключ OpenRouter и профили — модель, инструкция, шаблоны запроса, рассуждение,
// параметры генерации, провайдеры и свои поля запроса. Проверка правок на живой компании до сохранения.
import { useEffect, useMemo, useState, type ReactNode } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { aiInfo, api, type AiKind, type AiModel, type AiPreview, type AiProfile, type Message } from "../api";
import { ConfirmButton } from "../components";
import { copyText, fmtMs } from "../format";
import { fmtCost, RequestView } from "./AiDraft";

export interface AiSettingsValue {
  ai_profiles: AiProfile[];
  ai_profile: string;
  ai_concurrency: number;
}

/** Последняя открытая компания — для пробы профиля (пишет страница компании). */
export const LAST_BUSINESS_KEY = "scanner:lastBusiness";

function lastBusiness(): { id: number; name: string } | null {
  try {
    const v = JSON.parse(localStorage.getItem(LAST_BUSINESS_KEY) ?? "null");
    return v && typeof v.id === "number" ? v : null;
  } catch {
    return null;
  }
}

function ErrorLine({ error }: { error: Error | null }) {
  return error ? <span className="small st-fail" role="alert">{error.message}</span> : null;
}

/** «$0.10 / $0.40 за 1 млн токенов», «бесплатно». */
function fmtPrice(m: AiModel): string {
  if (m.free) return "бесплатно";
  if (m.prompt_price == null || m.completion_price == null) return "цена зависит от выбранной модели";
  const f = (x: number) => `$${x < 0.1 ? x.toPrecision(2) : x.toFixed(2)}`;
  return `${f(m.prompt_price)} / ${f(m.completion_price)} за 1 млн токенов (запрос / ответ)`;
}

function fmtContext(n: number): string {
  if (!n) return "";
  return n >= 1_000_000 ? `${(n / 1_000_000).toFixed(n % 1_000_000 ? 1 : 0)} млн токенов` : `${Math.round(n / 1000)} тыс. токенов`;
}

function reasoningNote(m: AiModel | undefined): string {
  if (!m) return "";
  const r = m.reasoning;
  if (!r && !m.supported_parameters.includes("reasoning")) return "Модель не рассуждает — эти настройки на неё не повлияют.";
  if (!r) return "Модель умеет рассуждать.";
  const parts: string[] = [];
  if (r.mandatory) parts.push("рассуждение у модели не отключается");
  if (r.default_enabled === true) parts.push(`по умолчанию рассуждает${r.default_effort ? ` (уровень ${r.default_effort})` : ""}`);
  if (r.default_enabled === false) parts.push("по умолчанию не рассуждает");
  if (r.supported_efforts.length) parts.push(`уровни: ${r.supported_efforts.join(", ")}`);
  if (r.supports_max_tokens) parts.push("понимает бюджет токенов");
  return parts.length ? `Модель: ${parts.join("; ")}.` : "";
}

const EFFORT_LABEL: Record<string, string> = { max: "max — максимум", xhigh: "xhigh — очень много", high: "high — много", medium: "medium — средне", low: "low — мало", minimal: "minimal — чуть-чуть" };

/* ------------------------------------------------------------------ выбор модели */

type Sort = "name" | "cheap" | "new";

function ModelPicker({ value, onChange, models, loading, error, onRefresh }: { value: string; onChange: (id: string) => void; models: AiModel[]; loading: boolean; error: Error | null; onRefresh: () => void }) {
  const [open, setOpen] = useState(false);
  const [needle, setNeedle] = useState("");
  const [onlyFree, setOnlyFree] = useState(false);
  const [onlyReasoning, setOnlyReasoning] = useState(false);
  const [sort, setSort] = useState<Sort>("name");
  const current = models.find((m) => m.id === value);
  const list = useMemo(() => {
    const n = needle.trim().toLowerCase();
    const items = models.filter(
      (m) =>
        (!n || `${m.id} ${m.name}`.toLowerCase().includes(n)) &&
        (!onlyFree || m.free) &&
        (!onlyReasoning || !!m.reasoning || m.supported_parameters.includes("reasoning")),
    );
    const price = (m: AiModel) => (m.free ? 0 : (m.prompt_price ?? 1e9) + (m.completion_price ?? 1e9));
    const by: Record<Sort, (a: AiModel, b: AiModel) => number> = {
      name: (a, b) => a.name.localeCompare(b.name),
      cheap: (a, b) => price(a) - price(b),
      new: (a, b) => b.created - a.created,
    };
    return items.sort(by[sort]);
  }, [models, needle, onlyFree, onlyReasoning, sort]);

  return (
    <div className="field">
      <label htmlFor="ai-model">Модель</label>
      <div className="row">
        <input id="ai-model" className="input mono" style={{ flex: 1, width: "auto" }} value={value} onChange={(e) => onChange(e.target.value)} placeholder="поставщик/модель, например google/gemma-4-31b-it:free" />
        <button type="button" className="btn" aria-expanded={open} onClick={() => setOpen((v) => !v)}>{open ? "Скрыть каталог" : "Выбрать из каталога"}</button>
      </div>
      {current ? (
        <span className="hint">
          <b>{current.name}</b> · {fmtPrice(current)}
          {current.context_length ? ` · контекст ${fmtContext(current.context_length)}` : ""}
          {current.expiration_date ? ` · модель уберут ${current.expiration_date}` : ""}
        </span>
      ) : (
        value && models.length > 0 && <span className="hint st-warn">Такой модели нет в каталоге OpenRouter — проверьте название.</span>
      )}
      {open && (
        <div className="stack" style={{ gap: 8, border: "1px solid var(--line)", borderRadius: 8, padding: 10 }}>
          <div className="row" style={{ alignItems: "center" }}>
            <input className="input" style={{ flex: 1, minWidth: 180, width: "auto" }} placeholder="Поиск: claude, gemini, qwen, free…" value={needle} onChange={(e) => setNeedle(e.target.value)} aria-label="Поиск модели" />
            <label className="small" style={{ display: "flex", gap: 6, alignItems: "center" }}>
              <input type="checkbox" checked={onlyFree} onChange={(e) => setOnlyFree(e.target.checked)} /> только бесплатные
            </label>
            <label className="small" style={{ display: "flex", gap: 6, alignItems: "center" }}>
              <input type="checkbox" checked={onlyReasoning} onChange={(e) => setOnlyReasoning(e.target.checked)} /> только рассуждающие
            </label>
            <select className="select" style={{ width: 150, height: 34 }} value={sort} onChange={(e) => setSort(e.target.value as Sort)} aria-label="Сортировка моделей">
              <option value="name">по названию</option>
              <option value="cheap">сначала дешёвые</option>
              <option value="new">сначала новые</option>
            </select>
            <button type="button" className="btn btn-sm btn-ghost" onClick={onRefresh} disabled={loading}>{loading ? "Обновляю…" : "Обновить"}</button>
          </div>
          <ErrorLine error={error} />
          <span className="small faint">{loading && !models.length ? "Загружаю каталог OpenRouter…" : `Моделей: ${list.length} из ${models.length}`}</span>
          <div style={{ maxHeight: 380, overflow: "auto", display: "grid", gap: 2 }} role="listbox" aria-label="Модели OpenRouter">
            {list.map((m) => (
              <button
                key={m.id}
                type="button"
                role="option"
                aria-selected={m.id === value}
                className="btn btn-ghost"
                style={{ height: "auto", justifyContent: "flex-start", textAlign: "left", padding: "6px 8px", display: "grid", gap: 2, background: m.id === value ? "var(--surface-2)" : undefined }}
                title={m.description}
                onClick={() => {
                  onChange(m.id);
                  setOpen(false);
                }}
              >
                <span className="small">
                  <b>{m.name}</b> <span className={m.free ? "st-pass" : "faint"}>· {fmtPrice(m)}</span>
                  {m.reasoning || m.supported_parameters.includes("reasoning") ? <span className="faint"> · рассуждает</span> : null}
                </span>
                <span className="small faint mono">
                  {m.id}
                  {m.context_length ? ` · ${fmtContext(m.context_length)}` : ""}
                </span>
              </button>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

/* ------------------------------------------------------------------ поля */

function NumField({ id, label, hint, value, onChange, step = 0.05, min, max, model }: { id: keyof AiProfile; label: string; hint: string; value: number | null; onChange: (v: number | null) => void; step?: number; min?: number; max?: number; model?: AiModel }) {
  const unsupported = model && model.supported_parameters.length > 0 && !model.supported_parameters.includes(id);
  return (
    <div className="field">
      <label htmlFor={`ai-${id}`}>{label}</label>
      <input
        id={`ai-${id}`}
        type="number"
        className="input"
        step={step}
        min={min}
        max={max}
        placeholder="как у модели"
        value={value ?? ""}
        onChange={(e) => onChange(e.target.value === "" ? null : Number(e.target.value))}
      />
      <span className={`hint${unsupported ? " st-warn" : ""}`}>{unsupported ? "Эта модель параметр не поддерживает — OpenRouter его проигнорирует. " : ""}{hint}</span>
    </div>
  );
}

function ListField({ id, label, hint, value, onChange, placeholder }: { id: string; label: string; hint: string; value: string[]; onChange: (v: string[]) => void; placeholder?: string }) {
  const [text, setText] = useState(value.join(", "));
  useEffect(() => setText(value.join(", ")), [value.join(",")]);
  return (
    <div className="field">
      <label htmlFor={id}>{label}</label>
      <input
        id={id}
        className="input mono"
        placeholder={placeholder}
        value={text}
        onChange={(e) => setText(e.target.value)}
        onBlur={() => onChange(text.split(/[,\n]/).map((s) => s.trim()).filter(Boolean))}
      />
      <span className="hint">{hint}</span>
    </div>
  );
}

function PromptField({ id, label, hint, value, onChange, onReset, rows }: { id: string; label: string; hint: ReactNode; value: string; onChange: (v: string) => void; onReset?: () => void; rows: number }) {
  return (
    <div className="field">
      <div className="row" style={{ alignItems: "baseline" }}>
        <label htmlFor={id}>{label}</label>
        <span className="spacer" />
        {onReset && <button type="button" className="btn btn-sm btn-ghost" onClick={onReset}>Вернуть по умолчанию</button>}
      </div>
      <textarea id={id} className="input mono" rows={rows} value={value} onChange={(e) => onChange(e.target.value)} spellCheck />
      <span className="hint">{hint}</span>
    </div>
  );
}

/** Свои поля запроса: JSON-объект; неверный JSON не сохраняется, пока его не исправят. */
function ExtraField({ value, onChange }: { value: Record<string, unknown> | null; onChange: (v: Record<string, unknown> | null) => void }) {
  const initial = value && Object.keys(value).length ? JSON.stringify(value, null, 2) : "";
  const [text, setText] = useState(initial);
  const [err, setErr] = useState("");
  useEffect(() => setText(initial), [initial]);
  return (
    <div className="field">
      <label htmlFor="ai-extra">Свои параметры запроса (JSON)</label>
      <textarea
        id="ai-extra"
        className="input mono"
        rows={5}
        placeholder={'{\n  "plugins": [{"id": "response-healing"}],\n  "provider": {"max_price": {"prompt": 1, "completion": 4}}\n}'}
        value={text}
        onChange={(e) => {
          const t = e.target.value;
          setText(t);
          if (!t.trim()) {
            setErr("");
            onChange(null);
            return;
          }
          try {
            const v = JSON.parse(t);
            if (!v || typeof v !== "object" || Array.isArray(v)) throw new Error("нужен объект { … }");
            setErr("");
            onChange(v);
          } catch (x) {
            setErr(`Не JSON-объект: ${(x as Error).message}`);
          }
        }}
      />
      {err ? <span className="hint st-fail">{err}</span> : (
        <span className="hint">
          Любые поля запроса OpenRouter, которых нет выше (plugins, max_price, transforms…): накладываются на собранный запрос последними, объекты сливаются, <code>null</code> убирает поле.
          Справка: openrouter.ai/docs/api-reference/chat-completion.
        </span>
      )}
    </div>
  );
}

/* ------------------------------------------------------------------ проба на компании */

function TryOnBusiness({ profile }: { profile: AiProfile }) {
  const last = lastBusiness();
  const [bid, setBid] = useState(last ? String(last.id) : "");
  const [kind, setKind] = useState<AiKind>("email");
  const [preview, setPreview] = useState<AiPreview | null>(null);
  const [draft, setDraft] = useState<Message | null>(null);
  const id = Number(bid);
  const showReq = useMutation({ mutationFn: () => api.aiPreview(id, { kind, profile }), onSuccess: (p) => { setPreview(p); setDraft(null); } });
  const gen = useMutation({ mutationFn: () => api.createDraft(id, { kind, profile }), onSuccess: (m) => { setDraft(m); setPreview(null); } });
  const ok = Number.isInteger(id) && id > 0;
  const info = draft ? aiInfo(draft) : null;
  return (
    <div className="stack" style={{ gap: 8, border: "1px solid var(--line)", borderRadius: 8, padding: 12 }}>
      <b className="small">Проверить профиль на компании — с несохранёнными правками</b>
      <div className="row" style={{ alignItems: "center" }}>
        <input className="input" style={{ width: 130 }} inputMode="numeric" placeholder="№ компании" value={bid} onChange={(e) => setBid(e.target.value.replace(/\D/g, ""))} aria-label="Номер компании" />
        <select className="select" style={{ width: 180, height: 34 }} value={kind} onChange={(e) => setKind(e.target.value as AiKind)} aria-label="Что составить">
          <option value="email">Письмо</option>
          <option value="short">Сообщение в мессенджер</option>
        </select>
        <button type="button" className="btn" onClick={() => showReq.mutate()} disabled={!ok || showReq.isPending}>Показать запрос</button>
        <button type="button" className="btn btn-primary" onClick={() => gen.mutate()} disabled={!ok || gen.isPending}>{gen.isPending ? "Составляю…" : "Составить"}</button>
      </div>
      <span className="hint">
        Номер — из адреса страницы компании (/businesses/11236){last ? `; подставлена последняя открытая: ${last.name}` : ""}. «Составить» сохранит черновик у этой компании.
      </span>
      <ErrorLine error={showReq.error ?? gen.error} />
      {preview && (
        <>
          {preview.unknown_vars.length > 0 && <span className="small st-warn">Неизвестные переменные: {preview.unknown_vars.map((v) => `{{${v}}}`).join(", ")}</span>}
          {preview.unsupported.length > 0 && <span className="small st-warn">Модель не поддерживает: {preview.unsupported.join(", ")}</span>}
          <RequestView request={preview.request} maxHeight={420} />
        </>
      )}
      {draft && (
        <div className="stack" style={{ gap: 6 }}>
          <span className="small faint">
            {info?.model} · {fmtMs(info?.ms)} · {fmtCost(info?.cost)} · токены {info?.usage.prompt} + {info?.usage.completion}
          </span>
          {draft.subject && <div className="small"><b>Тема:</b> {draft.subject}</div>}
          <div className="small" style={{ whiteSpace: "pre-wrap" }}>{draft.body}</div>
          {(info?.lint ?? []).length > 0 && <ul className="small st-warn" style={{ margin: 0, paddingLeft: 18 }}>{info?.lint.map((x) => <li key={x}>{x}</li>)}</ul>}
          {info?.reasoning && (
            <details>
              <summary className="small">Ход рассуждения</summary>
              <div className="small muted" style={{ whiteSpace: "pre-wrap", maxHeight: 300, overflow: "auto" }}>{info.reasoning}</div>
            </details>
          )}
        </div>
      )}
    </div>
  );
}

/* ------------------------------------------------------------------ карточка настроек */

function newId(): string {
  return `p${Date.now().toString(36)}`;
}

export function AiSettingsCard({ value, onChange, keyInput }: { value: AiSettingsValue; onChange: (patch: Partial<AiSettingsValue>) => void; keyInput: ReactNode }) {
  const meta = useQuery({ queryKey: ["ai-meta"], queryFn: api.aiMeta, staleTime: Infinity });
  const models = useQuery({ queryKey: ["ai-models"], queryFn: () => api.aiModels(), staleTime: 3_600_000 });
  const refreshModels = useMutation({ mutationFn: () => api.aiModels(true), onSuccess: () => models.refetch() });
  const keyCheck = useMutation({ mutationFn: api.aiKey });
  const profiles = value.ai_profiles ?? [];
  const [editing, setEditing] = useState(value.ai_profile);
  const p = profiles.find((x) => x.id === editing) ?? profiles[0];
  const list = models.data?.models ?? [];
  const model = p ? list.find((m) => m.id === p.model) : undefined;
  const def = meta.data?.default_profile;

  if (!p) return null;
  const set = (patch: Partial<AiProfile>) => onChange({ ai_profiles: profiles.map((x) => (x.id === p.id ? { ...x, ...patch } : x)) });
  const setReasoning = (patch: Partial<AiProfile["reasoning"]>) => set({ reasoning: { ...p.reasoning, ...patch } });
  const setProvider = (patch: Partial<AiProfile["provider"]>) => set({ provider: { ...p.provider, ...patch } });
  const add = (base: AiProfile, name: string) => {
    const np = { ...structuredClone(base), id: newId(), name };
    onChange({ ai_profiles: [...profiles, np] });
    setEditing(np.id);
  };
  const efforts = (model?.reasoning?.supported_efforts ?? meta.data?.efforts ?? []).filter((e) => e !== "none");
  const key = keyCheck.data;

  return (
    <div className="card" id="ai">
      <div className="card-head"><h2>Нейросеть для писем (OpenRouter)</h2></div>
      <div className="card-pad stack" style={{ gap: 16 }}>
        <p className="small muted" style={{ margin: 0 }}>
          Любая модель из каталога OpenRouter составляет первое письмо и сообщение клиенту по фактам проверки — тем же, что берёт шаблон. Бесплатные модели (с «:free») —
          без оплаты: 20 запросов в минуту и 50 в день (1000 — после пополнения счёта на $10). Платные списывают кредиты по цене модели; цена каждого письма видна в черновике.
          Тексты компаний уходят в OpenRouter и к провайдеру модели.
        </p>
        {keyInput}
        <div className="row" style={{ alignItems: "center" }}>
          <button type="button" className="btn btn-sm" onClick={() => keyCheck.mutate()} disabled={keyCheck.isPending}>{keyCheck.isPending ? "Проверяю…" : "Проверить ключ"}</button>
          {key && (
            <span className="small st-pass">
              Ключ работает{key.label ? ` («${key.label}»)` : ""}: потрачено ${key.usage.toFixed(4)}
              {key.limit_remaining != null ? `, осталось $${key.limit_remaining.toFixed(2)}` : ""}
              {key.free_model_daily_requests ? `; бесплатных запросов сегодня: ${key.free_model_daily_requests.remaining} из ${key.free_model_daily_requests.limit}` : ""}
            </span>
          )}
          <ErrorLine error={keyCheck.error} />
        </div>
        <span className="hint">Ключ проверяется уже сохранённый — после замены сначала нажмите «Сохранить».</span>

        <div className="stack" style={{ gap: 8 }}>
          <div className="row" style={{ alignItems: "center" }}>
            <span className="label">Профили</span>
            <div className="chips">
              {profiles.map((x) => (
                <button key={x.id} type="button" className="chip" aria-pressed={x.id === p.id} onClick={() => setEditing(x.id)}>
                  {x.name}{x.id === value.ai_profile ? " — по умолчанию" : ""}
                </button>
              ))}
            </div>
            <span className="spacer" />
            {def && <button type="button" className="btn btn-sm" onClick={() => add(def, "Новый профиль")}>Новый</button>}
            <button type="button" className="btn btn-sm" onClick={() => add(p, `${p.name} (копия)`)}>Копия</button>
            {p.id !== value.ai_profile && <button type="button" className="btn btn-sm" onClick={() => onChange({ ai_profile: p.id })}>Сделать по умолчанию</button>}
            {profiles.length > 1 && (
              <ConfirmButton className="btn btn-sm btn-ghost btn-danger" confirm="Удалить профиль?" onConfirm={() => {
                const rest = profiles.filter((x) => x.id !== p.id);
                onChange({ ai_profiles: rest, ai_profile: value.ai_profile === p.id ? rest[0].id : value.ai_profile });
                setEditing(rest[0].id);
              }}>Удалить</ConfirmButton>
            )}
          </div>
          <span className="hint">Профиль — набор настроек модели. Основной используется по умолчанию; на странице компании и при массовой генерации можно выбрать другой.</span>
        </div>

        <div className="grid-2">
          <div className="field">
            <label htmlFor="ai-name">Название профиля</label>
            <input id="ai-name" className="input" value={p.name} onChange={(e) => set({ name: e.target.value })} />
          </div>
          <div className="field">
            <label htmlFor="ai-conc">Писем одновременно (массово)</label>
            <input id="ai-conc" type="number" className="input" min={1} max={8} value={value.ai_concurrency} onChange={(e) => onChange({ ai_concurrency: Number(e.target.value) })} />
            <span className="hint">Для бесплатных моделей — 1–2: у них 20 запросов в минуту.</span>
          </div>
        </div>

        <ModelPicker value={p.model} onChange={(m) => set({ model: m })} models={list} loading={models.isFetching || refreshModels.isPending} error={(models.error ?? refreshModels.error) as Error | null} onRefresh={() => refreshModels.mutate()} />
        <ListField
          id="ai-fallback"
          label="Запасные модели"
          hint="Через запятую, по порядку: если основная модель недоступна или перегружена, OpenRouter возьмёт следующую."
          value={p.fallback_models}
          onChange={(v) => set({ fallback_models: v })}
          placeholder="например, nvidia/nemotron-3-ultra-550b-a55b:free"
        />

        <PromptField
          id="ai-system"
          label="Инструкция (системное сообщение)"
          hint="Как писать: роль, тон, правила, запреты. Переменные {{…}} работают и здесь."
          rows={14}
          value={p.system}
          onChange={(v) => set({ system: v })}
          onReset={def ? () => set({ system: def.system }) : undefined}
        />
        <PromptField
          id="ai-email"
          label="Запрос: письмо"
          hint="Что написать и в каком виде вернуть. Ответ разбирается как JSON {subject, body}; если модель вернёт обычный текст, тема берётся из строки «Тема: …»."
          rows={16}
          value={p.email_prompt}
          onChange={(v) => set({ email_prompt: v })}
          onReset={def ? () => set({ email_prompt: def.email_prompt }) : undefined}
        />
        <PromptField
          id="ai-short"
          label="Запрос: сообщение в мессенджер"
          hint="Ответ — JSON {body} или просто текст."
          rows={12}
          value={p.short_prompt}
          onChange={(v) => set({ short_prompt: v })}
          onReset={def ? () => set({ short_prompt: def.short_prompt }) : undefined}
        />
        {meta.data && (
          <details>
            <summary className="small">Переменные для инструкции и запросов ({meta.data.vars.length}) — нажмите, чтобы скопировать</summary>
            <div className="stack" style={{ gap: 2, marginTop: 8 }}>
              {meta.data.vars.map((v) => (
                <button key={v.name} type="button" className="btn btn-ghost" style={{ height: "auto", justifyContent: "flex-start", textAlign: "left", padding: "3px 6px" }} onClick={() => copyText(`{{${v.name}}}`)}>
                  <span className="small"><span className="mono">{`{{${v.name}}}`}</span> <span className="muted">— {v.help}</span></span>
                </button>
              ))}
            </div>
          </details>
        )}

        <div className="stack" style={{ gap: 10 }}>
          <span className="label">Рассуждение</span>
          <div className="grid-2">
            <div className="field">
              <label htmlFor="ai-r-mode">Режим</label>
              <select id="ai-r-mode" className="select" style={{ height: 34 }} value={p.reasoning.mode} onChange={(e) => setReasoning({ mode: e.target.value as AiProfile["reasoning"]["mode"] })}>
                <option value="default">как у модели (не отправлять)</option>
                <option value="off" disabled={model?.reasoning?.mandatory}>выключить</option>
                <option value="effort">уровень усилий</option>
                <option value="budget">бюджет токенов</option>
              </select>
            </div>
            {p.reasoning.mode === "effort" && (
              <div className="field">
                <label htmlFor="ai-r-effort">Уровень</label>
                <select id="ai-r-effort" className="select" style={{ height: 34 }} value={p.reasoning.effort} onChange={(e) => setReasoning({ effort: e.target.value })}>
                  {efforts.map((e) => <option key={e} value={e}>{EFFORT_LABEL[e] ?? e}</option>)}
                </select>
                <span className="hint">Доля предела ответа на размышление: max и xhigh ≈ 95 %, high ≈ 80 %, medium ≈ 50 %, low ≈ 20 %, minimal ≈ 10 %.</span>
              </div>
            )}
            {p.reasoning.mode === "budget" && (
              <div className="field">
                <label htmlFor="ai-r-budget">Токенов на рассуждение</label>
                <input id="ai-r-budget" type="number" className="input" min={0} step={256} placeholder="2000" value={p.reasoning.max_tokens ?? ""} onChange={(e) => setReasoning({ max_tokens: e.target.value === "" ? null : Number(e.target.value) })} />
                <span className="hint">Для моделей Anthropic — от 1024. Остальным OpenRouter переводит бюджет в уровень.</span>
              </div>
            )}
          </div>
          <label className="check">
            <input type="checkbox" checked={p.reasoning.exclude} onChange={(e) => setReasoning({ exclude: e.target.checked })} />
            <span>
              <b>Не возвращать ход рассуждения</b>
              <span className="hint" style={{ display: "block" }}>Модель рассуждает, но текст размышлений в ответ не попадает (токены всё равно оплачиваются). Без галочки ход рассуждения виден в черновике.</span>
            </span>
          </label>
          {model && <span className="hint">{reasoningNote(model)}</span>}
        </div>

        <div className="stack" style={{ gap: 10 }}>
          <span className="label">Параметры генерации — пустое поле не отправляется (как у модели)</span>
          <div className="grid-3">
            <NumField id="temperature" label="Температура" hint="0–2: ниже — ровнее и предсказуемее, выше — разнообразнее." min={0} max={2} value={p.temperature} onChange={(v) => set({ temperature: v })} model={model} />
            <NumField id="top_p" label="top_p" hint="0–1: брать слова из верхней доли вероятности." min={0} max={1} value={p.top_p} onChange={(v) => set({ top_p: v })} model={model} />
            <NumField id="top_k" label="top_k" hint="Выбор из k самых вероятных слов." min={0} step={1} value={p.top_k} onChange={(v) => set({ top_k: v })} model={model} />
            <NumField id="min_p" label="min_p" hint="0–1: отсечь слова слабее этой доли от лучшего." min={0} max={1} value={p.min_p} onChange={(v) => set({ min_p: v })} model={model} />
            <NumField id="top_a" label="top_a" hint="0–1: динамическое отсечение по лучшему слову." min={0} max={1} value={p.top_a} onChange={(v) => set({ top_a: v })} model={model} />
            <NumField id="frequency_penalty" label="Штраф за частоту" hint="−2…2: меньше повторов одних и тех же слов." min={-2} max={2} value={p.frequency_penalty} onChange={(v) => set({ frequency_penalty: v })} model={model} />
            <NumField id="presence_penalty" label="Штраф за присутствие" hint="−2…2: чаще новые темы." min={-2} max={2} value={p.presence_penalty} onChange={(v) => set({ presence_penalty: v })} model={model} />
            <NumField id="repetition_penalty" label="Штраф за повтор" hint="1 — без штрафа, больше — меньше повторов." min={0} max={2} value={p.repetition_penalty} onChange={(v) => set({ repetition_penalty: v })} model={model} />
            <NumField id="seed" label="seed" hint="Одинаковый seed — повторяемый ответ (если модель умеет)." step={1} value={p.seed} onChange={(v) => set({ seed: v })} model={model} />
            <NumField id="max_tokens" label="Предел токенов ответа" hint="Вместе с рассуждением. Мало — ответ оборвётся." min={1} step={100} value={p.max_tokens} onChange={(v) => set({ max_tokens: v })} model={model} />
            <div className="field">
              <label htmlFor="ai-json">Ответ в JSON</label>
              <select id="ai-json" className="select" style={{ height: 34 }} value={p.json_mode} onChange={(e) => set({ json_mode: e.target.value as AiProfile["json_mode"] })}>
                <option value="auto">если модель умеет</option>
                <option value="on">всегда требовать</option>
                <option value="off">не требовать</option>
              </select>
              <span className="hint">Схема ответа (structured outputs): тема и текст приходят отдельными полями.</span>
            </div>
            <div className="field">
              <label htmlFor="ai-verb">Подробность (verbosity)</label>
              <select id="ai-verb" className="select" style={{ height: 34 }} value={p.verbosity} onChange={(e) => set({ verbosity: e.target.value as AiProfile["verbosity"] })}>
                <option value="">не отправлять</option>
                <option value="low">low</option>
                <option value="medium">medium</option>
                <option value="high">high</option>
              </select>
              <span className="hint">Понимают в основном модели OpenAI.</span>
            </div>
          </div>
          <div className="field">
            <label htmlFor="ai-stop">Стоп-последовательности</label>
            <textarea
              id="ai-stop"
              className="input mono"
              rows={2}
              placeholder="по одной в строке, \n — перевод строки"
              value={p.stop.map((s) => s.replace(/\n/g, "\\n")).join("\n")}
              onChange={(e) => set({ stop: e.target.value.split("\n").filter((l) => l !== "").map((l) => l.replace(/\\n/g, "\n")).slice(0, 4) })}
            />
            <span className="hint">До четырёх: модель остановится, дойдя до любой из них.</span>
          </div>
        </div>

        <div className="stack" style={{ gap: 10 }}>
          <span className="label">Провайдеры модели</span>
          <div className="grid-3">
            <div className="field">
              <label htmlFor="ai-dc">Хранение запросов</label>
              <select id="ai-dc" className="select" style={{ height: 34 }} value={p.provider.data_collection} onChange={(e) => setProvider({ data_collection: e.target.value as AiProfile["provider"]["data_collection"] })}>
                <option value="">как в аккаунте OpenRouter</option>
                <option value="deny">только не хранящие и не обучающиеся</option>
                <option value="allow">разрешить</option>
              </select>
              <span className="hint">Бесплатные модели часто доступны только с «разрешить».</span>
            </div>
            <div className="field">
              <label htmlFor="ai-sort">Выбор провайдера</label>
              <select id="ai-sort" className="select" style={{ height: 34 }} value={p.provider.sort} onChange={(e) => setProvider({ sort: e.target.value as AiProfile["provider"]["sort"] })}>
                <option value="">балансировка OpenRouter</option>
                <option value="price">дешевле</option>
                <option value="throughput">быстрее печатает</option>
                <option value="latency">быстрее отвечает</option>
              </select>
            </div>
            <div className="field">
              <label htmlFor="ai-fb">Другие провайдеры, если выбранные недоступны</label>
              <select id="ai-fb" className="select" style={{ height: 34 }} value={p.provider.allow_fallbacks == null ? "" : String(p.provider.allow_fallbacks)} onChange={(e) => setProvider({ allow_fallbacks: e.target.value === "" ? null : e.target.value === "true" })}>
                <option value="">по умолчанию (да)</option>
                <option value="true">да</option>
                <option value="false">нет</option>
              </select>
            </div>
          </div>
          <div className="grid-3">
            <ListField id="ai-order" label="Порядок провайдеров" hint="slug через запятую: deepinfra, together…" value={p.provider.order} onChange={(v) => setProvider({ order: v })} />
            <ListField id="ai-only" label="Только эти" hint="Остальных не использовать." value={p.provider.only} onChange={(v) => setProvider({ only: v })} />
            <ListField id="ai-ignore" label="Не использовать" hint="Исключить провайдеров." value={p.provider.ignore} onChange={(v) => setProvider({ ignore: v })} />
          </div>
          <div className="grid-2">
            <label className="check">
              <input type="checkbox" checked={p.provider.zdr} onChange={(e) => setProvider({ zdr: e.target.checked })} />
              <span><b>Только без хранения данных (ZDR)</b><span className="hint" style={{ display: "block" }}>Провайдеры с нулевым хранением запросов.</span></span>
            </label>
            <label className="check">
              <input type="checkbox" checked={p.provider.require_parameters} onChange={(e) => setProvider({ require_parameters: e.target.checked })} />
              <span><b>Только поддерживающие все параметры</b><span className="hint" style={{ display: "block" }}>Иначе неподдерживаемые параметры тихо игнорируются.</span></span>
            </label>
          </div>
        </div>

        <ExtraField key={p.id} value={p.extra} onChange={(v) => set({ extra: v })} />
        <TryOnBusiness profile={p} />
        <span className="hint">Правки профилей применяются после «Сохранить» внизу страницы; проба выше берёт их сразу.</span>
      </div>
    </div>
  );
}
