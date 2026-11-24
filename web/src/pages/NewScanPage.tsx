import { useEffect, useRef, useState } from "react";
import { Link, useNavigate } from "react-router";
import { useMutation, useQuery } from "@tanstack/react-query";
import { api, type GeoResult, type LatLon, type ProviderId } from "../api";
import { MapPicker } from "../map";
import { fmtRadius } from "../format";

const RADII = [300, 500, 1000, 2000, 5000];
const LAST_KEY = "scanner:last-center";

function loadLast(): LatLon {
  try {
    const v = JSON.parse(localStorage.getItem(LAST_KEY) ?? "");
    if (typeof v?.lat === "number" && typeof v?.lon === "number") return v;
  } catch {
    /* пусто — центр Москвы */
  }
  return { lat: 55.7558, lon: 37.6173 };
}

export default function NewScanPage() {
  const nav = useNavigate();
  const meta = useQuery({ queryKey: ["meta"], queryFn: api.meta, staleTime: 60_000 });
  const net = useQuery({ queryKey: ["net"], queryFn: api.net, staleTime: 5 * 60_000 });

  const [center, setCenter] = useState<LatLon>(loadLast);
  const [flyKey, setFlyKey] = useState(0);
  const [radius, setRadius] = useState(1000);
  const [preset, setPreset] = useState("dentist");
  const [query, setQuery] = useState("");
  const [city, setCity] = useState("");
  const cityTouched = useRef(false);
  // Оба бесплатных источника по умолчанию: OSM и Overture дополняют друг друга.
  const [providers, setProviders] = useState<ProviderId[]>(["osm", "overture"]);
  const [maxPlaces, setMaxPlaces] = useState(100);
  // Быстрая проверка: по умолчанию включается для больших сканов, можно переключить руками.
  const [quickTouched, setQuickTouched] = useState<boolean | null>(null);
  const quick = quickTouched ?? maxPlaces > 300;
  const [name, setName] = useState("");

  const [search, setSearch] = useState("");
  const [results, setResults] = useState<GeoResult[]>([]);
  const [searching, setSearching] = useState(false);

  // Источники с ключами включаем сами, как только узнаём, что они доступны.
  useEffect(() => {
    if (!meta.data) return;
    const avail = meta.data.providers.filter((p) => p.available && p.id !== "apple").map((p) => p.id as ProviderId);
    setProviders(avail.length ? avail : ["osm"]);
  }, [meta.data]);

  // Город/район по точке — пока пользователь не ввёл его сам.
  useEffect(() => {
    try {
      localStorage.setItem(LAST_KEY, JSON.stringify(center));
    } catch {
      /* хранилище недоступно — не страшно */
    }
    if (cityTouched.current) return;
    const t = setTimeout(() => {
      api.reverse(center).then((r) => {
        if (!cityTouched.current) setCity(r.district ?? r.city ?? "");
      }).catch(() => {});
    }, 700);
    return () => clearTimeout(t);
  }, [center]);

  // Поиск места (Nominatim просит не чаще раза в секунду).
  useEffect(() => {
    const q = search.trim();
    if (q.length < 3) {
      setResults([]);
      return;
    }
    const t = setTimeout(() => {
      setSearching(true);
      api.geocode(q).then(setResults).catch(() => setResults([])).finally(() => setSearching(false));
    }, 900);
    return () => clearTimeout(t);
  }, [search]);

  const create = useMutation({
    mutationFn: () =>
      api.createScan({
        name: name.trim() || undefined,
        query: query.trim(),
        preset: query.trim() ? null : preset || null,
        city: city.trim() || undefined,
        center,
        radius_m: radius,
        providers,
        max_places: maxPlaces,
        quick,
      }),
    onSuccess: (s) => nav(`/scans/${s.id}`),
  });

  const toggle = (id: ProviderId) => setProviders((p) => (p.includes(id) ? p.filter((x) => x !== id) : [...p, id]));
  const foreign = net.data?.country && net.data.country !== "RU";

  return (
    <div className="page">
      <div className="page-head">
        <div>
          <h1>Новый скан</h1>
          <p className="sub">Кликните по карте или перетащите метку — это центр района</p>
        </div>
      </div>

      <div className="new-scan">
        <form
          className="card card-pad stack"
          onSubmit={(e) => {
            e.preventDefault();
            create.mutate();
          }}
        >
          <div className="field">
            <label htmlFor="geo">Найти место</label>
            <input id="geo" className="input" placeholder="Москва, Хамовники" value={search} onChange={(e) => setSearch(e.target.value)} />
            {searching && <span className="hint">Ищу…</span>}
            {results.length > 0 && (
              <div className="geo-results">
                {results.map((r) => (
                  <button
                    type="button"
                    key={`${r.lat},${r.lon}`}
                    onClick={() => {
                      setCenter({ lat: r.lat, lon: r.lon });
                      setFlyKey((k) => k + 1);
                      setResults([]);
                      setSearch("");
                      cityTouched.current = false;
                    }}
                  >
                    {r.label}
                  </button>
                ))}
              </div>
            )}
          </div>

          <div className="field">
            <label htmlFor="radius">Радиус: {fmtRadius(radius)}</label>
            <input id="radius" type="range" min={200} max={10000} step={100} value={radius} onChange={(e) => setRadius(Number(e.target.value))} />
            <div className="chips">
              {RADII.map((r) => (
                <button type="button" key={r} className="chip" aria-pressed={radius === r} onClick={() => setRadius(r)}>
                  {fmtRadius(r)}
                </button>
              ))}
            </div>
          </div>

          <div className="field">
            <label htmlFor="preset">Категория</label>
            <select id="preset" className="select" value={preset} onChange={(e) => setPreset(e.target.value)} disabled={!!query.trim()}>
              {meta.data?.presets.map((p) => (
                <option key={p.id} value={p.id}>{p.label}</option>
              ))}
              <option value="">Все организации — магазины, кафе, услуги, офисы (OSM и Overture)</option>
            </select>
          </div>
          <div className="field">
            <label htmlFor="query">Или свой запрос</label>
            <input id="query" className="input" placeholder="например, «детская стоматология»" value={query} onChange={(e) => setQuery(e.target.value)} />
            <span className="hint">Свой запрос заменяет категорию. В OSM ищет по названию.</span>
          </div>
          <div className="field">
            <label htmlFor="city">Город или район</label>
            <input
              id="city"
              className="input"
              value={city}
              onChange={(e) => {
                cityTouched.current = true;
                setCity(e.target.value);
              }}
            />
            <span className="hint">Проверяем, упомянут ли он в title и тексте сайтов</span>
          </div>

          <div className="field">
            <span className="label">Источники</span>
            {meta.data?.providers.map((p) => {
              const usable = p.available && p.id !== "apple";
              return (
                <label key={p.id} className={`check${usable ? "" : " disabled"}`}>
                  <input type="checkbox" disabled={!usable} checked={usable && providers.includes(p.id as ProviderId)} onChange={() => toggle(p.id as ProviderId)} />
                  <span>
                    <b>{p.label}</b>
                    <span className="hint" style={{ display: "block" }}>
                      {p.note} {!p.available && p.needs_key && <Link to="/settings">Добавить ключ</Link>}
                    </span>
                  </span>
                </label>
              );
            })}
          </div>

          <div className="grid-2" style={{ gap: 12 }}>
            <div className="field">
              <label htmlFor="max">Максимум мест</label>
              <input id="max" type="number" className="input" min={1} max={20000} value={maxPlaces} onChange={(e) => setMaxPlaces(Math.min(20000, Number(e.target.value) || 1))} />
              <div className="chips">
                {[100, 500, 2000, 20000].map((n) => (
                  <button type="button" key={n} className="chip" aria-pressed={maxPlaces === n} onClick={() => setMaxPlaces(n)}>
                    {n === 20000 ? "все" : n}
                  </button>
                ))}
              </div>
              <span className="hint">Если мест больше, берутся ближайшие к центру</span>
            </div>
            <div className="field">
              <label htmlFor="name">Название</label>
              <input id="name" className="input" placeholder="по умолчанию — запрос" value={name} onChange={(e) => setName(e.target.value)} />
            </div>
          </div>

          <label className="check">
            <input type="checkbox" checked={quick} onChange={(e) => setQuickTouched(e.target.checked)} />
            <span>
              <b>Быстрая проверка</b>
              <span className="hint" style={{ display: "block" }}>
                Главная и пара страниц, без PageSpeed и угадывания доменов: тысяча сайтов — за минуты, а не часы. Полная проверка одной
                компании — кнопкой «Перепроверить» в её карточке.
              </span>
            </span>
          </label>

          {foreign && (
            <div className="banner warn small">
              <span>
                Скан идёт с IP в стране <b>{net.data!.country}</b>. Российские сайты часто закрыты для зарубежных адресов: часть сайтов
                попадёт в «не пустил сканер». Для точных результатов сканируйте без VPN.
              </span>
            </div>
          )}
          {create.error && <div className="banner error small">{create.error.message}</div>}
          <button className="btn btn-primary" disabled={providers.length === 0 || create.isPending}>
            {create.isPending ? "Запускаю…" : "Начать скан"}
          </button>
        </form>

        <MapPicker
          center={center}
          radius={radius}
          flyKey={flyKey}
          onPick={(p) => {
            setCenter(p);
            cityTouched.current = false;
          }}
        />
      </div>
    </div>
  );
}
