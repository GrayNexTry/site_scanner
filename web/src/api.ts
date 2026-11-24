// Типы и вызовы API сервера сканера (/api). Типы повторяют структуры Rust.

export type Status = "pass" | "warn" | "fail" | "info" | "not_measured";
export type Lead = "hot" | "warm" | "cold" | "closed";
export type ProviderId = "osm" | "overture" | "google" | "yandex" | "manual";

export interface Threshold {
  op: string;
  value: number | string | number[];
  unit?: string | null;
}

export interface MetricDef {
  id: string;
  category: string;
  name: string;
  method: string;
  rule: string;
  source: "ext" | "access" | "both";
  kind: "check" | "kpi" | "info";
  auto: boolean | string;
  stage: "lead" | "audit" | "report";
  weight: number;
  pass: Threshold | null;
  warn: Threshold | null;
  stop_factor: boolean;
  lead_signal: string | null;
  in_report: boolean;
}

export interface Catalog {
  version: string;
  categories: Record<string, string>;
  category_weights: Record<string, number>;
  metrics: MetricDef[];
}

export interface Preset {
  id: string;
  label: string;
  query: string;
  osm: [string, string][];
}

export interface ProviderInfo {
  id: "osm" | "google" | "yandex" | "apple";
  label: string;
  available: boolean;
  needs_key: boolean;
  note: string;
}

export interface Meta {
  version: string;
  catalog: Catalog;
  presets: Preset[];
  providers: ProviderInfo[];
  psi: boolean;
  browser: string | null;
}

export interface LatLon {
  lat: number;
  lon: number;
}

export interface ScanParams {
  query: string;
  preset?: string | null;
  city?: string | null;
  country?: string | null;
  city_names?: string[];
  center: LatLon;
  radius_m: number;
  providers: ProviderId[];
  max_places: number;
  quick?: boolean;
}

export type ScanStatus = "queued" | "running" | "done" | "failed" | "cancelled" | "interrupted";

export interface Scan {
  id: number;
  created_at: string;
  finished_at: string | null;
  name: string;
  kind: "map" | "manual";
  params: ScanParams;
  status: ScanStatus;
  stage: "places" | "audit" | null;
  total: number;
  done: number;
  /** Ошибки источников; `notice` — источник ответил, но с оговоркой (например, список неполный). */
  provider_errors: { provider: ProviderId; error: string; notice?: boolean }[] | null;
  error: string | null;
  leads: Partial<Record<Lead | "pending", number>>;
}

export interface MetricResult {
  id: string;
  status: Status;
  value?: unknown;
  detail: string;
}

export interface CategoryScore {
  score: number | null;
  pass: number;
  warn: number;
  fail: number;
  measured: number;
  total: number;
}

export interface Report {
  overall: number | null;
  categories: Record<string, CategoryScore>;
  stop_factors: string[];
  lead: Lead;
  lead_reasons: string[];
  priority: number;
  counts: { pass: number; warn: number; fail: number; info: number; not_measured: number };
  /** Ситуация с сайтом; в списке скана приходит без fail_ids. */
  mini?: MiniReport;
}

export type SiteState = "unknown" | "none" | "social" | "hidden" | "dead" | "stub" | "blocked" | "weak" | "fair" | "good" | "chain" | "closed";

export interface MiniReport {
  state: SiteState;
  headline: string;
  problems: string[];
  offer: string[];
  fail_ids?: string[];
}

/** Мини-отчёт по району: состояния сайтов и частые проблемы. */
export interface ScanOverview {
  total: number;
  audited: number;
  sites_scored: number;
  avg_score: number | null;
  states: Partial<Record<SiteState, number>>;
  leads: Partial<Record<Lead, number>>;
  top_problems: { id: string; label: string; count: number; share: number }[];
  /** Сайты, которые могут открыться после смены сети (без несуществующих доменов). */
  recheckable?: number;
  /** Страна района и страна, из которой сейчас идут запросы (VPN). */
  country?: string | null;
  egress?: string | null;
}

export interface Place {
  provider: ProviderId | null;
  provider_id: string;
  name: string;
  address?: string | null;
  lat: number;
  lon: number;
  phone?: string | null;
  website?: string | null;
  category?: string | null;
  rating?: number | null;
  reviews?: number | null;
  hours?: string | null;
  business_status?: string | null;
  card_url?: string | null;
  email?: string | null;
  links?: string[];
  brand?: string | null;
  wikidata?: string | null;
}

export type DiscoverySource = "email" | "card_link" | "wikidata" | "social_link" | "domain_guess" | "web_search" | "manual";

export interface Verdict {
  url: string;
  domain: string;
  source: DiscoverySource;
  via: string;
  score: number;
  confirmed: boolean;
  evidence: string[];
  title: string | null;
}

export interface Discovery {
  found: Verdict | null;
  suggestions: Verdict[];
  guessed: number;
  resolved: number;
  checked: number;
  search: string | null;
  brand: string | null;
  elapsed_ms: number;
}

export type LeadStatus = "new" | "contacted" | "talking" | "client" | "rejected" | "skip";

export interface SiteSummary {
  input_url: string;
  final_url: string | null;
  status: number | null;
  error: string | null;
  ttfb_ms: number | null;
  total_ms: number | null;
  http_version: string | null;
  server: string | null;
  throttled: boolean;
  input_broken: number | null;
  /** Проверен через свой прокси; direct_error — что было напрямую. */
  via_proxy?: boolean;
  direct_error?: string | null;
  js_shell: boolean;
  rendered: boolean;
  render_error: string | null;
  title: string | null;
  pages_crawled: number;
  sitemap_urls: number;
  links_checked: number;
  tls: { valid: boolean; days_left: number | null; issuer: string | null; protocol: string | null; error: string | null } | null;
  domain: { domain: string; created: string | null; expires: string | null; days_to_expiry: number | null; age_years: number | null; error: string | null } | null;
  psi: { perf: number | null; lcp_ms: number | null; inp_ms: number | null; cls: number | null; field: boolean; error: string | null } | null;
  elapsed_ms: number;
}

export interface Business {
  id: number;
  scan_id: number;
  name: string;
  address: string | null;
  lat: number | null;
  lon: number | null;
  phone: string | null;
  website: string | null;
  category: string | null;
  rating: number | null;
  reviews: number | null;
  data: { sources: Place[]; duplicates: number };
  audit_status: "pending" | "running" | "done" | "failed";
  lead: Lead | null;
  overall: number | null;
  priority: number | null;
  report: Report | null;
  results?: MetricResult[] | null;
  site: SiteSummary | null;
  audited_at: string | null;
  audit_ms: number | null;
  error: string | null;
  discovery: Discovery | null;
  website_override: string | null;
  lead_status: LeadStatus;
  notes: string | null;
  status_at: string | null;
  site_url: string | null;
  scan_name?: string;
}

export interface KeyState {
  set: boolean;
  hint: string;
}

export interface Settings {
  google_places_key: KeyState;
  yandex_geosearch_key: KeyState;
  psi_key: KeyState;
  safe_browsing_key: KeyState;
  max_pages: number;
  link_checks: number;
  concurrency: number;
  per_site_concurrency: number;
  timeout_secs: number;
  psi_enabled: boolean;
  discover_enabled: boolean;
  discover_guess: boolean;
  search_free: boolean;
  brave_search_key: KeyState;
  yandex_search_key: KeyState;
  yandex_folder_id: string;
  render_js: boolean;
  browser_path: string;
  proxy_url: KeyState;
  proxy_always: boolean;
  direct_network: boolean;
}

export interface GeoResult {
  label: string;
  lat: number;
  lon: number;
}

export type ScanEvent =
  | { type: "scan"; scan: Scan }
  | { type: "business"; scan_id: number; business: Business };

export class ApiError extends Error {}

async function call<T>(path: string, init?: RequestInit): Promise<T> {
  const resp = await fetch(`/api${path}`, {
    ...init,
    headers: { "content-type": "application/json", ...(init?.headers ?? {}) },
  });
  const text = await resp.text();
  const data = text ? JSON.parse(text) : null;
  if (!resp.ok) {
    throw new ApiError(data?.error ?? `Сервер ответил ${resp.status}`);
  }
  return data as T;
}

const post = <T>(path: string, body?: unknown) =>
  call<T>(path, { method: "POST", body: body === undefined ? undefined : JSON.stringify(body) });

export const api = {
  meta: () => call<Meta>("/meta"),
  net: () => call<{ ip: string | null; country: string | null }>("/net"),
  netViaProxy: () => call<{ ip: string | null; country: string | null }>("/net?via=proxy"),
  settings: () => call<Settings>("/settings"),
  saveSettings: (patch: Record<string, unknown>) => call<Settings>("/settings", { method: "PUT", body: JSON.stringify(patch) }),
  scans: () => call<Scan[]>("/scans"),
  scan: (id: number) => call<{ scan: Scan; running: boolean }>(`/scans/${id}`),
  createScan: (body: {
    name?: string;
    query: string;
    preset?: string | null;
    city?: string;
    center: LatLon;
    radius_m: number;
    providers: ProviderId[];
    max_places: number;
    quick?: boolean;
  }) => post<Scan>("/scans", body),
  overview: (id: number) => call<ScanOverview>(`/scans/${id}/overview`),
  rescore: (id: number) => post<{ rescored: number }>(`/scans/${id}/rescore`, {}),
  deleteScan: (id: number) => call<{ deleted: boolean }>(`/scans/${id}`, { method: "DELETE" }),
  cancelScan: (id: number) => post<{ cancelled: boolean }>(`/scans/${id}/cancel`),
  reaudit: (id: number, onlyPending = false) => post<{ started: boolean }>(`/scans/${id}/reaudit`, { only_pending: onlyPending }),
  reauditUnreachable: (id: number) => post<{ started: boolean }>(`/scans/${id}/reaudit`, { only_unreachable: true }),
  searchAgain: (id: number) => post<{ started: boolean }>(`/scans/${id}/search`, {}),
  businesses: (scanId: number) => call<Business[]>(`/scans/${scanId}/businesses`),
  business: (id: number) => call<{ business: Business; scan: Scan | null }>(`/businesses/${id}`),
  auditBusiness: (id: number) => post<{ started: boolean }>(`/businesses/${id}/audit`),
  patchBusiness: (id: number, patch: { lead_status?: LeadStatus; notes?: string; website_override?: string }) =>
    call<{ business: Business; reaudit: boolean }>(`/businesses/${id}`, { method: "PATCH", body: JSON.stringify(patch) }),
  leads: () => call<Business[]>("/leads"),
  auditSite: (url: string, query?: string, city?: string) => post<Scan>("/audit", { url, query, city }),
  geocode: (q: string) => call<GeoResult[]>(`/geocode?q=${encodeURIComponent(q)}`),
  reverse: (p: LatLon) => call<{ city: string | null; district: string | null }>(`/reverse?lat=${p.lat}&lon=${p.lon}`),
  exportUrl: (scanId: number) => `/api/scans/${scanId}/export.csv`,
};
