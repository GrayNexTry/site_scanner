// Подписи, цвета и форматирование.
import type { DiscoverySource, Lead, LeadStatus, ProviderId, ScanStatus, SiteState, Status } from "./api";

export const LEAD_LABEL: Record<Lead, string> = {
  hot: "Горячий",
  warm: "Тёплый",
  cold: "Холодный",
  closed: "Закрыто",
};

export const LEAD_HINT: Record<Lead, string> = {
  hot: "Нет сайта, сайт не работает или вместо сайта соцсеть",
  warm: "Сайт есть, но слабый или с критичными ошибками",
  cold: "Сайт в порядке",
  closed: "Карточка помечена закрытой навсегда",
};

export const STATUS_LABEL: Record<Status, string> = {
  pass: "Норма",
  warn: "Внимание",
  fail: "Проблема",
  info: "Справка",
  not_measured: "Не измерено",
};

export const SCAN_STATUS: Record<ScanStatus, string> = {
  queued: "В очереди",
  running: "Идёт",
  done: "Готово",
  failed: "Ошибка",
  cancelled: "Остановлен",
  interrupted: "Прерван",
};

export const PROVIDER_LABEL: Record<ProviderId, string> = {
  osm: "OSM",
  overture: "Overture",
  google: "Google",
  yandex: "Яндекс",
  manual: "Вручную",
};

/** Хост без схемы и www. Без new URL(): тот переводит кириллические домены в punycode. */
export function hostOf(url: string | null | undefined): string {
  if (!url) return "";
  return url
    .trim()
    .replace(/^[a-z][a-z0-9+.-]*:\/\//i, "")
    .split(/[/?#]/)[0]
    .replace(/^www\./i, "")
    .toLowerCase();
}

export function absUrl(url: string): string {
  return url.includes("://") ? url : `https://${url}`;
}

const dateFmt = new Intl.DateTimeFormat("ru-RU", { day: "numeric", month: "short", hour: "2-digit", minute: "2-digit" });

export function fmtDate(iso: string | null | undefined): string {
  return iso ? dateFmt.format(new Date(iso)) : "—";
}

export function fmtMs(ms: number | null | undefined): string {
  if (ms == null) return "—";
  return ms < 1000 ? `${ms} мс` : `${(ms / 1000).toFixed(1).replace(".", ",")} с`;
}

export function fmtRadius(m: number): string {
  return m < 1000 ? `${Math.round(m)} м` : `${(m / 1000).toFixed(m % 1000 === 0 ? 0 : 1).replace(".", ",")} км`;
}

export function plural(n: number, one: string, few: string, many: string): string {
  const m10 = n % 10;
  const m100 = n % 100;
  if (m10 === 1 && m100 !== 11) return one;
  if (m10 >= 2 && m10 <= 4 && (m100 < 12 || m100 > 14)) return few;
  return many;
}

/** Класс цвета для балла 0–100. */
export function scoreTone(v: number | null | undefined): "pass" | "warn" | "fail" | "na" {
  if (v == null) return "na";
  if (v >= 75) return "pass";
  if (v >= 50) return "warn";
  return "fail";
}

export const LEAD_STATUS_LABEL: Record<LeadStatus, string> = {
  new: "Новый",
  contacted: "Написали",
  talking: "Переговоры",
  client: "Клиент",
  rejected: "Отказ",
  skip: "Не подходит",
};

export const DISCOVERY_SOURCE: Record<DiscoverySource, string> = {
  email: "по почте из карточки",
  card_link: "по ссылке из карточки",
  wikidata: "по Wikidata бренда",
  social_link: "по ссылке из соцсети",
  domain_guess: "по названию (домен)",
  web_search: "через поисковик",
  manual: "указан вручную",
};

const COUNTRY_NAMES = new Intl.DisplayNames(["ru"], { type: "region" });

/** «de» → «Германия». */
export function countryName(code: string | null | undefined): string {
  if (!code) return "";
  try {
    return COUNTRY_NAMES.of(code.toUpperCase()) ?? code.toUpperCase();
  } catch {
    return code.toUpperCase();
  }
}

/** Адрес сайта, который реально проверялся: вручную → найденный → из карточки. */
export function siteOf(b: { site_url: string | null; website: string | null; website_override: string | null; discovery: { found: { url: string } | null } | null }): string | null {
  return b.website_override ?? b.site_url ?? b.discovery?.found?.url ?? b.website ?? null;
}

/** Что с сайтом — подписи для сводки по району и фильтров. Порядок — порядок плиток. */
export const SITE_STATE_LABEL: Record<SiteState, string> = {
  none: "Нет сайта",
  social: "Соцсеть вместо сайта",
  stub: "Заглушка",
  dead: "Сайт не работает",
  hidden: "Сайт не в карточке",
  weak: "Слабый сайт",
  fair: "Средний сайт",
  good: "Хороший сайт",
  blocked: "Не пустил сканер",
  chain: "Сеть",
  closed: "Закрыто",
  unknown: "Не проверено",
};

/** Тон плитки: где есть что продать. */
export const SITE_STATE_TONE: Record<SiteState, "fail" | "warn" | "pass" | "muted"> = {
  none: "fail",
  social: "fail",
  stub: "fail",
  dead: "fail",
  hidden: "warn",
  weak: "warn",
  fair: "warn",
  good: "pass",
  blocked: "muted",
  chain: "muted",
  closed: "muted",
  unknown: "muted",
};
