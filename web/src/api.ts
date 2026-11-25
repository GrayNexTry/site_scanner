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
  /** Проблема простыми словами по номеру метрики — для отчёта клиенту. */
  labels?: Record<string, string>;
  labels_ok?: Record<string, string>;
  labels_en?: Record<string, string>;
  labels_ok_en?: Record<string, string>;
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
  /** Формулировка ПО ФАКТУ этого результата простыми словами (ru / en). */
  plain?: string;
  plain_en?: string;
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
  headline_en?: string;
  /** Технический текст для менеджера (заглушка / мёртвый сайт); клиенту не показывать. */
  detail?: string | null;
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

export type LeadStatus = "new" | "contacted" | "talking" | "proposal" | "client" | "rejected" | "skip";

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
  /** Работа менеджера: цена сделки, следующий шаг, ЛПР, ручные контакты. */
  deal_price?: number | null;
  deal_currency?: string | null;
  next_action_at?: string | null;
  next_action?: string | null;
  contact_person?: string | null;
  /** Ручные значения менеджера для документов. */
  display_name?: string | null;
  display_address?: string | null;
  extra_contacts?: ExtraContact[] | null;
  last_activity_at?: string | null;
  /** Только в списке лидов и ответе компании: false — контактов нет совсем. */
  has_contacts?: boolean;
  /** Виды контактов без повторов (phone, email, instagram…): в списке компаний скана и лидов. */
  contact_kinds?: string[];
  /** Первый телефон в международном виде (в ответе компании и списках). */
  client_phone?: string | null;
  /** Сохранённые правки КП (если есть). */
  proposal?: unknown;
}

export interface ExtraContact {
  kind: string;
  value: string;
  note?: string;
}

export interface Contact extends ExtraContact {
  url?: string | null;
  source: "card" | "site" | "manual";
}

export type ActivityKind = "note" | "message" | "reply" | "call" | "meeting" | "proposal" | "status" | "price";
export type Channel = "instagram" | "email" | "phone" | "telegram" | "whatsapp" | "viber" | "vk" | "facebook" | "visit" | "other";

export interface Activity {
  id: number;
  business_id: number;
  at: string;
  kind: ActivityKind;
  channel: Channel | null;
  text: string | null;
}

export interface ProposalItem {
  id: string;
  text: string;
  level: "fail" | "warn";
}

export interface ProposalPackage {
  id: string;
  title: string;
  summary: string;
  price: number;
  enabled: boolean;
  default_enabled: boolean;
  items: ProposalItem[];
}

export interface Proposal {
  packages: ProposalPackage[];
  extra: { title: string; price: number }[];
  discount: number;
  subtotal: number;
  total: number;
  note: string;
  valid_days: number;
  currency: string;
  /** Язык текстов КП. */
  lang?: "ru" | "en";
  /** Для этой валюты своих цен нет: показаны цены настроек как есть. */
  prices_unverified?: boolean;
}

/** Правки КП, которые уходят на сервер (PATCH proposal). */
export interface ProposalPatch {
  packages: { id: string; enabled?: boolean; price?: number; title?: string }[];
  extra: { title: string; price: number }[];
  discount?: number;
  note?: string;
  valid_days?: number;
}

export interface Outreach {
  lang: "ru" | "en";
  short: string;
  email_subject: string;
  email_body: string;
}

/* ---------------------------------------------------------------- нейросеть (OpenRouter) */

export type AiKind = "email" | "short";

export interface AiReasoning {
  /** default — как у модели, off — выключить, effort — уровень, budget — бюджет токенов. */
  mode: "default" | "off" | "effort" | "budget";
  effort: string;
  max_tokens: number | null;
  /** Не возвращать ход рассуждения (токены всё равно оплачиваются). */
  exclude: boolean;
}

export interface AiProviderPrefs {
  order: string[];
  only: string[];
  ignore: string[];
  allow_fallbacks: boolean | null;
  data_collection: "" | "allow" | "deny";
  zdr: boolean;
  sort: "" | "price" | "throughput" | "latency";
  require_parameters: boolean;
}

export interface AiProfile {
  id: string;
  name: string;
  model: string;
  fallback_models: string[];
  system: string;
  email_prompt: string;
  short_prompt: string;
  temperature: number | null;
  top_p: number | null;
  top_k: number | null;
  min_p: number | null;
  top_a: number | null;
  frequency_penalty: number | null;
  presence_penalty: number | null;
  repetition_penalty: number | null;
  seed: number | null;
  max_tokens: number | null;
  stop: string[];
  verbosity: "" | "low" | "medium" | "high";
  json_mode: "auto" | "on" | "off";
  reasoning: AiReasoning;
  provider: AiProviderPrefs;
  /** Любые другие поля запроса OpenRouter; накладываются последними. */
  extra: Record<string, unknown> | null;
  /** Есть замечания проверки — попросить модель исправить их (ещё один запрос). */
  fix_lint: boolean;
}

export interface AiModel {
  id: string;
  name: string;
  description: string;
  context_length: number;
  max_completion_tokens: number | null;
  /** Доллары за 1 млн токенов; null — переменная цена. */
  prompt_price: number | null;
  completion_price: number | null;
  free: boolean;
  supported_parameters: string[];
  reasoning: { supported_efforts: string[]; default_effort: string | null; default_enabled: boolean | null; mandatory: boolean; supports_max_tokens: boolean } | null;
  input_modalities: string[];
  created: number;
  expiration_date: string | null;
}

export interface AiMeta {
  vars: { name: string; help: string }[];
  default_profile: AiProfile;
  efforts: string[];
}

export interface AiKeyInfo {
  label?: string;
  limit: number | null;
  limit_remaining: number | null;
  usage: number;
  usage_daily?: number;
  usage_monthly?: number;
  is_free_tier: boolean;
  free_model_daily_requests?: { used: number; limit: number; remaining: number } | null;
}

export interface AiPreview {
  request: Record<string, unknown>;
  unknown_vars: string[];
  unsupported: string[];
  vars: Record<string, string>;
  lang: "ru" | "en";
  model: AiModel | null;
  profile: { id: string; name: string };
}

/** Как составлен черновик. */
export interface MessageAi {
  profile_id: string;
  profile_name: string;
  model_requested: string;
  model: string;
  provider: string | null;
  request: Record<string, unknown>;
  unsupported: string[];
  unknown_vars: string[];
  content: string;
  reasoning: string | null;
  usage: { prompt: number; completion: number; reasoning: number; cached: number };
  cost: number | null;
  ms: number;
  finish_reason: string | null;
  lint: string[];
  lang: "ru" | "en";
  attempts: number;
  original: { subject: string; body: string };
  /** Первый вариант до исправления по замечаниям. */
  first?: { content: string; lint: string[]; cost: number | null; ms: number } | null;
}

/** Сведения нейросети о письме; у шаблона и своего текста — null. */
export function aiInfo(m: { ai: Message["ai"] }): MessageAi | null {
  return m.ai && "model_requested" in m.ai ? m.ai : null;
}

export interface Message {
  id: number;
  business_id: number;
  created_at: string;
  updated_at: string | null;
  direction: "out" | "in";
  kind: AiKind;
  /** draft — черновик, queued — в рассылке, sending — уходит, sent — отправлено, failed — не ушло. */
  status: "draft" | "queued" | "sending" | "sent" | "failed";
  subject: string | null;
  body: string;
  /** Как составлено (у шаблона и своего текста — только source). */
  ai: MessageAi | { source: string } | null;
  sent_at: string | null;
  to_addr: string | null;
  queued_at: string | null;
  message_id: string | null;
  error: string | null;
}

/** Письмо в рассылке с компанией. */
export interface OutboxRow extends Message {
  business_name: string;
  display_name: string | null;
  scan_id: number;
  lead_status: LeadStatus;
}

export interface MailStatus {
  configured: boolean;
  running: boolean;
  from: string;
  queued: number;
  failed: number;
  sent_today: number;
  daily_limit: number;
  in_window: boolean;
  window: { start: number; end: number; weekdays_only: boolean };
  interval: { min: number; max: number };
  attach_report: boolean;
  info: { next_at: string | null; waiting: string | null; stop_reason: string | null; last_error: string | null; sending: number | null };
}

export type MailSource = "draft" | "template" | "draft_or_template";

export interface AiBatch {
  running: boolean;
  kind: string;
  profile: string;
  model: string;
  total: number;
  done: number;
  failed: number;
  skipped: number;
  cost: number;
  started_at: string | null;
  finished_at: string | null;
  stopped: boolean;
  stop_reason: string | null;
  errors: { id: number; name: string; error: string; skipped: boolean }[];
  drafted: number[];
  /** Поставлено в рассылку (опция «сразу в рассылку»). */
  queued: number;
}

/** Что отправить вместо сохранённого профиля: несохранённые правки из настроек. */
export type DraftRequest = { kind: AiKind; profile_id?: string; profile?: AiProfile };

export interface BusinessDetail {
  business: Business;
  /** Сообщения клиенту: черновики нейросети и отправленные. */
  messages?: Message[];
  scan: Scan | null;
  contacts: Contact[];
  activities: Activity[];
  proposal: Proposal | null;
  outreach: Outreach | null;
  /** Язык материалов для клиента. */
  lang?: "ru" | "en";
  /** Имя и адрес для шапки документов клиенту. */
  client_name?: string;
  client_address?: string | null;
  /** Название с сайта, если отличается от карточки. */
  name_suggestion?: string | null;
  /** Есть ли хоть один контакт (телефон, почта, мессенджер, соцсеть). */
  has_contacts?: boolean;
}

export interface BusinessPatch {
  display_name?: string;
  display_address?: string;
  lead_status?: LeadStatus;
  notes?: string;
  website_override?: string;
  deal_price?: number | null;
  deal_currency?: string;
  next_action_at?: string;
  next_action?: string;
  contact_person?: string;
  extra_contacts?: ExtraContact[] | null;
  proposal?: ProposalPatch | null;
}

export interface PriceItem {
  id: string;
  title: string;
  price: number;
  enabled: boolean;
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
  manager_name: string;
  manager_email: string;
  manager_phone: string;
  manager_telegram: string;
  company_name: string;
  intro: string;
  currency: string;
  price_list: PriceItem[];
  /** Свои прайсы по другим валютам. */
  price_lists?: Record<string, PriceItem[]>;
  portfolio_url: string;
  about_me: string;
  smtp_host: string;
  smtp_port: number;
  smtp_security: "ssl" | "starttls" | "plain";
  smtp_user: string;
  smtp_password: KeyState;
  mail_from_name: string;
  mail_from_email: string;
  mail_reply_to: string;
  mail_interval_min: number;
  mail_interval_max: number;
  mail_daily_limit: number;
  mail_window_start: number;
  mail_window_end: number;
  mail_weekdays_only: boolean;
  mail_attach_report: boolean;
  mail_footer: string;
  mail_footer_en: string;
  mail_running: boolean;
  openrouter_key: KeyState;
  ai_profiles: AiProfile[];
  ai_profile: string;
  ai_concurrency: number;
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
  business: (id: number) => call<BusinessDetail>(`/businesses/${id}`),
  auditBusiness: (id: number) => post<{ started: boolean }>(`/businesses/${id}/audit`),
  patchBusiness: (id: number, patch: BusinessPatch) =>
    call<{ business: Business; reaudit: boolean; activities?: Activity[] }>(`/businesses/${id}`, { method: "PATCH", body: JSON.stringify(patch) }),
  addActivity: (id: number, body: { kind: ActivityKind; channel?: Channel; text?: string; at?: string }) =>
    post<{ activity: Activity; activities: Activity[] }>(`/businesses/${id}/activities`, body),
  deleteActivity: (aid: number) => call<{ deleted: boolean; activities: Activity[] }>(`/activities/${aid}`, { method: "DELETE" }),
  markSent: (id: number, body: { channel: Channel; text?: string; message_id?: number; subject?: string; body?: string }) =>
    post<{ activity: Activity; business: Business; activities: Activity[] }>(`/businesses/${id}/sent`, body),
  leads: () => call<Business[]>("/leads"),
  auditSite: (url: string, query?: string, city?: string) => post<Scan>("/audit", { url, query, city }),
  geocode: (q: string) => call<GeoResult[]>(`/geocode?q=${encodeURIComponent(q)}`),
  reverse: (p: LatLon) => call<{ city: string | null; district: string | null }>(`/reverse?lat=${p.lat}&lon=${p.lon}`),
  aiMeta: () => call<AiMeta>("/ai/meta"),
  aiModels: (refresh = false) => call<{ models: AiModel[] }>(`/ai/models${refresh ? "?refresh=true" : ""}`),
  aiKey: () => call<AiKeyInfo>("/ai/key"),
  aiPreview: (id: number, body: DraftRequest) => post<AiPreview>(`/businesses/${id}/ai/preview`, body),
  createDraft: (id: number, body: DraftRequest) => post<Message>(`/businesses/${id}/drafts`, body),
  patchMessage: (mid: number, body: { subject?: string; body?: string }) => call<Message>(`/messages/${mid}`, { method: "PATCH", body: JSON.stringify(body) }),
  deleteMessage: (mid: number) => call<{ ok: boolean }>(`/messages/${mid}`, { method: "DELETE" }),
  aiBatch: () => call<AiBatch>("/ai/batch"),
  startAiBatch: (body: { ids: number[]; kind: AiKind; profile_id?: string; skip_existing?: boolean; enqueue?: boolean }) => post<AiBatch>("/ai/batch", body),
  mailStatus: () => call<MailStatus>("/mail/status"),
  mailStart: () => post<MailStatus>("/mail/start"),
  mailPause: () => post<MailStatus>("/mail/pause"),
  mailTest: (to?: string) => post<{ ok: boolean; to: string }>("/mail/test", { to }),
  outbox: () => call<OutboxRow[]>("/mail/outbox"),
  queueMail: (body: { ids: number[]; source: MailSource; skip_contacted?: boolean; skip_sent?: boolean }) =>
    post<{ queued: number; skipped: { id: number; name: string; reason: string }[] }>("/mail/queue", body),
  queueOne: (id: number, body: { message_id?: number; to: string; subject: string; body: string }) => post<Message>(`/businesses/${id}/mail`, body),
  unqueue: (mid: number) => post<{ ok: boolean }>(`/messages/${mid}/unqueue`),
  cancelAiBatch: () => post<AiBatch>("/ai/batch/cancel"),
  leadsCsvUrl: "/api/leads.csv",
  exportUrl: (scanId: number) => `/api/scans/${scanId}/export.csv`,
};
