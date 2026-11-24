import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api, type KeyState, type Settings } from "../api";
import { useToast } from "../components";

type KeyField = "google_places_key" | "yandex_geosearch_key" | "psi_key" | "safe_browsing_key" | "brave_search_key" | "yandex_search_key" | "proxy_url";

const KEYS: { id: KeyField; label: string; how: string }[] = [
  { id: "google_places_key", label: "Google Places API (New)", how: "Google Cloud Console → APIs & Services → включить «Places API (New)» → Credentials → API key. Нужен платёжный аккаунт; бесплатный лимит есть." },
  { id: "yandex_geosearch_key", label: "Яндекс: API Поиска по организациям", how: "developer.tech.yandex.ru → Подключить API → «API Поиска по организациям». Условия хранения данных — по договору Яндекса." },
  { id: "psi_key", label: "PageSpeed Insights", how: "Google Cloud Console → включить «PageSpeed Insights API» → API key. Даёт Core Web Vitals (CrUX) и Lighthouse. Без ключа общий лимит Google обычно исчерпан." },
  { id: "safe_browsing_key", label: "Google Safe Browsing", how: "Google Cloud Console → включить «Safe Browsing API» → API key. Проверка сайта по спискам опасных (B13)." },
];

const SEARCH_KEYS: { id: KeyField; label: string; how: string }[] = [
  { id: "brave_search_key", label: "Brave Search API", how: "api-dashboard.search.brave.com → план Search → API key. Надёжный веб-поиск сайтов по названию в любой стране." },
  { id: "yandex_search_key", label: "Yandex Search API", how: "Yandex Cloud → сервисный аккаунт с ролью search-api.webSearch.user → API-ключ. Лучший поиск по России. Нужен и ID каталога ниже." },
];

function KeyInput({ id, label, how, state, value, onChange, placeholder = "Вставьте ключ" }: { id: KeyField; label: string; how: string; state: KeyState; value: string | undefined; onChange: (v: string | undefined) => void; placeholder?: string }) {
  const editing = value !== undefined;
  return (
    <div className="field">
      <label htmlFor={id}>{label}</label>
      <div className="row">
        {editing ? (
          <input id={id} className="input" style={{ flex: 1, width: "auto" }} type="password" autoComplete="off" placeholder={placeholder} value={value} onChange={(e) => onChange(e.target.value)} />
        ) : (
          <span className={`small ${state.set ? "st-pass" : "faint"}`} style={{ flex: 1 }}>{state.set ? `Задан ${state.hint}` : "Не задан"}</span>
        )}
        {!editing && <button type="button" className="btn btn-sm" onClick={() => onChange("")}>{state.set ? "Заменить" : "Добавить"}</button>}
        {!editing && state.set && <button type="button" className="btn btn-sm btn-ghost btn-danger" onClick={() => onChange("")}>Удалить</button>}
        {editing && <button type="button" className="btn btn-sm btn-ghost" onClick={() => onChange(undefined)}>Отмена</button>}
      </div>
      <span className="hint">{how}</span>
    </div>
  );
}

export default function SettingsPage() {
  const qc = useQueryClient();
  const [toast, show] = useToast();
  const q = useQuery({ queryKey: ["settings"], queryFn: api.settings });
  const meta = useQuery({ queryKey: ["meta"], queryFn: api.meta, staleTime: 60_000 });
  const [keys, setKeys] = useState<Partial<Record<KeyField, string>>>({});
  const [limits, setLimits] = useState<Partial<Settings>>({});
  const proxyCheck = useMutation({ mutationFn: api.netViaProxy });

  useEffect(() => {
    if (q.data) setLimits(q.data);
  }, [q.data]);

  const save = useMutation({
    mutationFn: () => {
      const patch: Record<string, unknown> = {
        max_pages: limits.max_pages,
        link_checks: limits.link_checks,
        concurrency: limits.concurrency,
        per_site_concurrency: limits.per_site_concurrency,
        timeout_secs: limits.timeout_secs,
        psi_enabled: limits.psi_enabled,
        discover_enabled: limits.discover_enabled,
        discover_guess: limits.discover_guess,
        search_free: limits.search_free,
        yandex_folder_id: limits.yandex_folder_id,
        render_js: limits.render_js,
        browser_path: limits.browser_path,
        proxy_always: limits.proxy_always,
        direct_network: limits.direct_network,
      };
      for (const [k, v] of Object.entries(keys)) if (v !== undefined) patch[k] = v;
      return api.saveSettings(patch);
    },
    onSuccess: (s) => {
      qc.setQueryData(["settings"], s);
      qc.invalidateQueries({ queryKey: ["meta"] });
      setKeys({});
      show("Настройки сохранены");
    },
  });

  if (!q.data) return <div className="page muted">{q.error ? q.error.message : "Загружаю…"}</div>;
  const num = (k: keyof Settings, label: string, min: number, max: number, hint: string) => (
    <div className="field">
      <label htmlFor={k}>{label}</label>
      <input id={k} type="number" className="input" min={min} max={max} value={Number(limits[k] ?? 0)} onChange={(e) => setLimits((l) => ({ ...l, [k]: Number(e.target.value) }))} />
      <span className="hint">{hint}</span>
    </div>
  );

  const flag = (k: "discover_enabled" | "discover_guess" | "search_free" | "render_js" | "proxy_always" | "direct_network", label: string, hint: string) => (
    <label className="check">
      <input type="checkbox" checked={!!limits[k]} onChange={(e) => setLimits((l) => ({ ...l, [k]: e.target.checked }))} />
      <span>
        <b>{label}</b>
        <span className="hint" style={{ display: "block" }}>{hint}</span>
      </span>
    </label>
  );

  return (
    <div className="page" style={{ maxWidth: 900 }}>
      <div className="page-head">
        <div>
          <h1>Настройки</h1>
          <p className="sub">Ключи хранятся только в локальной базе (права 0600) и наружу не отдаются</p>
        </div>
      </div>
      <form
        className="stack"
        onSubmit={(e) => {
          e.preventDefault();
          save.mutate();
        }}
      >
        <div className="card">
          <div className="card-head"><h2>Ключи API</h2></div>
          <div className="card-pad stack" style={{ gap: 18 }}>
            {KEYS.map((k) => (
              <KeyInput key={k.id} {...k} state={q.data[k.id]} value={keys[k.id]} onChange={(v) => setKeys((s) => ({ ...s, [k.id]: v }))} />
            ))}
          </div>
        </div>
        <div className="card">
          <div className="card-head"><h2>Поиск сайтов в обход карт</h2></div>
          <div className="card-pad stack" style={{ gap: 14 }}>
            <p className="small muted">
              Если в карточке нет сайта или там соцсеть, сканер ищет сайт сам: по почте из карточки, ссылкам из соцсетей и мультиссылок, доменам по
              названию и через поисковик. Сайт считается найденным, только если на нём есть телефон или адрес из карточки; совпадение одного названия
              показывается как «возможно, их сайт».
            </p>
            {flag("discover_enabled", "Искать сайт в обход карт", "Почта, ссылки из соцсетей, Wikidata бренда; +3–15 с на компанию без сайта")}
            {flag("discover_guess", "Угадывать домены по названию", "До 18 DNS-запросов и до 6 открытых главных страниц на компанию")}
            {flag("search_free", "Веб-поиск без ключа (DuckDuckGo, Bing)", "Против правил поисковиков; через VPN и с IP дата-центров почти всегда капча — тогда поисковик отключается до перезапуска")}
            {SEARCH_KEYS.map((k) => (
              <KeyInput key={k.id} {...k} state={q.data[k.id]} value={keys[k.id]} onChange={(v) => setKeys((s) => ({ ...s, [k.id]: v }))} />
            ))}
            <div className="field">
              <label htmlFor="yandex_folder_id">ID каталога Yandex Cloud</label>
              <input id="yandex_folder_id" className="input" placeholder="b1g…" value={limits.yandex_folder_id ?? ""} onChange={(e) => setLimits((l) => ({ ...l, yandex_folder_id: e.target.value }))} />
              <span className="hint">Нужен вместе с ключом Yandex Search API</span>
            </div>
          </div>
        </div>
        <div className="card">
          <div className="card-head"><h2>Сайты на скриптах (React, Vue…)</h2></div>
          <div className="card-pad stack" style={{ gap: 14 }}>
            {flag("render_js", "Рендерить в headless-браузере", "Только если в исходном HTML почти нет текста; +10–30 с на такой сайт, не больше двух одновременно")}
            <div className="field">
              <label htmlFor="browser_path">Путь к Chromium или Chrome</label>
              <input id="browser_path" className="input" placeholder="пусто — найти автоматически" value={limits.browser_path ?? ""} onChange={(e) => setLimits((l) => ({ ...l, browser_path: e.target.value }))} />
              <span className={`hint ${meta.data?.browser ? "" : "st-warn"}`}>
                {meta.data?.browser
                  ? `Найден: ${meta.data.browser}`
                  : "Браузер не найден — такие сайты проверяются по исходному HTML, а метрики по тексту помечаются «не измерено». Установите: sudo pacman -S chromium"}
              </span>
            </div>
          </div>
        </div>
        <div className="card">
          <div className="card-head"><h2>Свой прокси</h2></div>
          <div className="card-pad stack" style={{ gap: 14 }}>
            <p className="small muted">
              Нужен, когда сайты закрыты для страны вашего выхода в интернет: например, российские сайты отвечают ошибкой или молчат через зарубежный
              VPN. Если сайт напрямую не открылся или ответил отказом, вся его проверка повторяется через прокси, и в карточке видно, что сайт
              открыт так. Подойдёт свой сервер (VPS) в нужной стране — например SSH-туннель <code>ssh -D 1080 user@vps</code> → <code>socks5h://127.0.0.1:1080</code> —
              или платный прокси провайдера. Бесплатные публичные списки прокси сканер не поддерживает намеренно: обычно это чужие взломанные
              устройства, они подменяют страницы и видят весь трафик.
            </p>
            <KeyInput
              id="proxy_url"
              label="Адрес прокси"
              how="socks5h://, socks5:// или http://, можно с логином и паролем: socks5h://user:pass@host:1080. Хранится как ключ — наружу только последние символы."
              placeholder="socks5h://127.0.0.1:1080"
              state={q.data.proxy_url}
              value={keys.proxy_url}
              onChange={(v) => setKeys((s) => ({ ...s, proxy_url: v }))}
            />
            {flag("direct_network", "Сайты — напрямую, мимо VPN", "Если включён VPN, запросы к сайтам идут через Wi-Fi или Ethernet: быстрее и без гео-блоков для сайтов своей страны. Не открылся напрямую — повтор через VPN. Карты, поиск и API — как обычно")}
            {flag("proxy_always", "Всё через прокси", "Не только сайты, которые не открылись напрямую, но и карты, поиск и API — как будто сканер работает из страны прокси")}
            {q.data.proxy_url.set && (
              <div className="row">
                <button type="button" className="btn btn-sm" onClick={() => proxyCheck.mutate()} disabled={proxyCheck.isPending}>
                  {proxyCheck.isPending ? "Проверяю…" : "Проверить прокси"}
                </button>
                {proxyCheck.data && <span className="small st-pass">Выход через прокси: {proxyCheck.data.country ?? "?"} · {proxyCheck.data.ip}</span>}
                {proxyCheck.error && <span className="small st-fail">{proxyCheck.error.message}</span>}
              </div>
            )}
          </div>
        </div>
        <div className="card">
          <div className="card-head"><h2>Скорость и глубина</h2></div>
          <div className="card-pad grid-2">
            {num("concurrency", "Сайтов одновременно", 1, 32, "Больше — быстрее, но выше нагрузка на сеть")}
            {num("per_site_concurrency", "Запросов к одному сайту", 1, 16, "Не больше 4–6: сайты режут частые запросы")}
            {num("max_pages", "Страниц на сайт", 1, 200, "Обход внутренних страниц для проверок")}
            {num("link_checks", "Проверок ссылок на сайт", 0, 500, "Выборочная проверка битых ссылок")}
            {num("timeout_secs", "Таймаут, с", 5, 60, "Сколько ждать ответа главной")}
            <label className="check" style={{ alignSelf: "start" }}>
              <input type="checkbox" checked={!!limits.psi_enabled} onChange={(e) => setLimits((l) => ({ ...l, psi_enabled: e.target.checked }))} />
              <span>
                <b>PageSpeed Insights</b>
                <span className="hint" style={{ display: "block" }}>Core Web Vitals и Lighthouse; +10–30 с на сайт</span>
              </span>
            </label>
          </div>
        </div>
        {save.error && <div className="banner error small">{save.error.message}</div>}
        <div className="row"><button className="btn btn-primary" disabled={save.isPending}>{save.isPending ? "Сохраняю…" : "Сохранить"}</button></div>
      </form>
      {toast}
    </div>
  );
}
