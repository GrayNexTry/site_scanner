// Тексты материалов для клиента (отчёт и КП) на русском и английском.
import type { MetricDef, MetricResult, SiteSummary } from "./api";
import { countryName, fmtMoney, plural } from "./format";

export type Lang = "ru" | "en";

const CYRILLIC = /[Ѐ-ӿ]/;
export const hasCyrillic = (s: string) => CYRILLIC.test(s);

/** Локаль дат и денег: gb → en-GB, us → en-US, остальное по языку. */
export function localeOf(lang: Lang, country?: string | null): string {
  if (lang === "ru") return "ru-RU";
  return (country ?? "").toLowerCase() === "us" ? "en-US" : "en-GB";
}

export function fmtLong(d: Date | string | null | undefined, lang: Lang, country?: string | null): string {
  if (!d) return "—";
  return new Intl.DateTimeFormat(localeOf(lang, country), { day: "numeric", month: "long", year: "numeric" }).format(typeof d === "string" ? new Date(d) : d);
}

/** Деньги по валюте и локали: «870 £» по-русски, «£870» по-английски. */
export function money(v: number | null | undefined, currency: string | null | undefined, lang: Lang): string {
  if (v == null) return "—";
  if (lang === "ru" || !currency) return fmtMoney(v, currency);
  try {
    return new Intl.NumberFormat(currency === "USD" ? "en-US" : "en-GB", { style: "currency", currency, maximumFractionDigits: 2, minimumFractionDigits: Number.isInteger(v) ? 0 : 2 }).format(v);
  } catch {
    return fmtMoney(v, currency);
  }
}

export const CAT_NAMES: Record<Lang, Record<string, string>> = {
  ru: {
    A: "Карточки на картах",
    B: "Доступность сайта и домен",
    C: "Видимость в Google и Яндексе",
    D: "Скорость загрузки",
    E: "Мобильная версия",
    F: "Заголовки и тексты страниц",
    G: "Разметка для поисковиков и ИИ",
    H: "Контакты и совпадение с картами",
    I: "Готовность к поиску с ИИ",
    M: "Доверие и обязательные документы",
    N: "Заявки с сайта",
    O: "Безопасность",
    P: "Аналитика",
    Q: "Удобство для всех посетителей",
    R: "Яндекс: регион и коммерческие признаки",
  },
  en: {
    A: "Map listings",
    B: "Website availability and domain",
    C: "Visibility in Google",
    D: "Page speed",
    E: "Mobile experience",
    F: "Page titles and content",
    G: "Markup for search engines and AI",
    H: "Contact details and match with maps",
    I: "Readiness for AI search",
    M: "Trust and legal information",
    N: "Leads from the website",
    O: "Security",
    P: "Analytics",
    Q: "Accessibility for all visitors",
    R: "Regional and commercial signals",
  },
};

export const WHY: Record<Lang, Record<string, string>> = {
  ru: {
    A: "Через карты приходит большая часть клиентов локального бизнеса. Карточка должна быть полной и вести на сайт.",
    B: "Если сайт открывается долго или с ошибками, клиент уходит, а поисковики показывают сайт ниже.",
    C: "Чтобы сайт находили в Google и Яндексе, поисковые роботы должны видеть и правильно понимать его страницы.",
    D: "Медленные страницы теряют посетителей; Google учитывает скорость при ранжировании.",
    E: "Больше половины клиентов ищут услуги с телефона.",
    F: "Заголовки и тексты должны отвечать на запрос «услуга + город» — так сайт попадает в выдачу.",
    G: "Скрытая разметка на сайте сообщает поисковикам и ИИ-ассистентам, кто вы, где находитесь и когда работаете.",
    H: "Адрес, телефон и часы работы на сайте должны совпадать с картами — это сигнал доверия для поисковиков.",
    I: "ChatGPT, Gemini, Perplexity, Алиса и обзоры Google с ИИ советуют сайты, которые их роботы могут прочитать и в которых есть готовый ответ: что, где, почём, с датой обновления.",
    M: "Обязательные документы и реквизиты защищают от штрафов и вызывают доверие клиентов.",
    N: "Чем проще позвонить, написать или записаться, тем больше заявок с тех же посетителей.",
    O: "Защищённое соединение и базовая защита берегут посетителей и репутацию сайта.",
    P: "Без аналитики нельзя посчитать, сколько заявок приносит сайт и реклама.",
    Q: "Сайт должен быть удобен всем посетителям, включая людей с нарушениями зрения.",
  },
  en: {
    A: "Most customers of a local business come through maps. The listing should be complete and lead to your website.",
    B: "If the site is slow or shows errors, visitors leave and search engines rank it lower.",
    C: "For people to find the site in Google, search engines need to be able to see and correctly understand its pages.",
    D: "Slow pages lose visitors, and Google takes speed into account when ranking.",
    E: "More than half of customers look for services on their phone.",
    F: "Titles and text should answer searches like \"service + city\". That is how a site gets into the results.",
    G: "Hidden markup tells search engines and AI assistants who you are, where you are and when you are open.",
    H: "The address, phone number and opening hours on the site should match your map listings. It is a trust signal for search engines.",
    I: "ChatGPT, Gemini, Perplexity and Google's AI overviews recommend sites their crawlers can read and that contain a ready answer: what, where, how much, with an update date.",
    M: "Required legal pages and company details protect you from fines and build customer trust.",
    N: "The easier it is to call, message or book, the more enquiries you get from the same visitors.",
    O: "A secure connection and basic protection keep visitors safe and protect the site's reputation.",
    P: "Without analytics you cannot tell how many enquiries the site and your advertising bring in.",
    Q: "The site should be usable by every visitor, including people with visual impairments.",
  },
};

export type Strings = typeof S.ru;
const S = {
  ru: {
    reportKind: "Аудит сайта и карточек на картах",
    noSiteShort: "сайта нет",
    download: "Скачать PDF",
    print: "Печать",
    reportHint: "PDF собирается без служебных строк браузера (адрес, дата, номера страниц)",
    preparing: "Готовлю отчёт…",
    checked: (n: number) => `Проверено ${n} ${plural(n, "показатель", "показателя", "показателей")}`,
    critical: (n: number) => `${n} ${plural(n, "критичная проблема", "критичные проблемы", "критичных проблем")}`,
    attention: (n: number) => `${n} требуют внимания`,
    ok: (n: number) => `${n} в норме`,
    topTitle: "Главное, что стоит исправить",
    tagFail: "критично",
    tagWarn: "внимание",
    catTitle: "Оценка по разделам",
    catFail: "крит.",
    catWarn: "вним.",
    catOk: "в норме",
    mapsOnlyTitle: "Карточки на картах",
    aiTitle: "Готовность к поиску с ИИ",
    okTitle: "Что уже в порядке",
    atAddressStub: "По адресу сайта открывается заглушка хостинга",
    atAddressDead: "Сайт не открывается",
    techAddress: "Ответ по адресу сайта",
    noSiteNote: "Баллы по разделам не считаются: оценивать нечего.",
    techTitle: "Технические подробности для вашего разработчика",
    techIntro: "Приложение для того, кто будет вносить правки: что именно показал замер по каждому пункту.",
    techItem: "Пункт",
    techMeasure: "Замер",
    footMeasured: (d: string) => `Замеры сделаны автоматически ${d} по внешним признакам: без доступа к кабинетам сайта и карт.`,
    footLegal: (c: string) => ` Правовые проверки — по нормам страны: ${c}.`,
    footRest: " Позиции в поиске и ответы ИИ-ассистентов в этот отчёт не входят: они меряются отдельно, по методике с фиксированным набором запросов. Повторный замер после работ покажет изменения по тем же показателям.",
    // КП
    proposalKind: "Коммерческое предложение",
    proposalPreparing: "Готовлю предложение…",
    proposalHint: "Правки плана и цен — на странице компании; здесь показано сохранённое",
    now: "Что сейчас",
    scoreNote: (n: number) => `Оценка сайта по ${n} показателям: поиск, скорость, мобильная версия, заявки.`,
    fallbackHeadline: "Результат проверки сайта",
    fallbackHeadlineNone: "Сайт не удалось проверить",
    mainBlockers: "Главное, что мешает привлекать клиентов:",
    plan: "План работ",
    subtotal: "Стоимость работ",
    discount: "Скидка",
    total: "Итого",
    validFor: (n: number, until: string) => `Предложение действует ${n} ${plural(n, "день", "дня", "дней")} — до ${until}`,
    nothingPicked: "Ни один пакет работ не выбран: отметьте нужное на странице компании.",
    contacts: "Контакты",
    phone: "Телефон",
    email: "Почта",
    docTitleReport: "Аудит",
    docTitleProposal: "КП",
  },
  en: {
    reportKind: "Website and map listing audit",
    noSiteShort: "no website",
    download: "Download PDF",
    print: "Print",
    reportHint: "The PDF is generated without browser headers and footers (address, date, page numbers)",
    preparing: "Preparing the report…",
    checked: (n: number) => `${n} ${n === 1 ? "item" : "items"} checked`,
    critical: (n: number) => `${n} critical ${n === 1 ? "issue" : "issues"}`,
    attention: (n: number) => `${n} need attention`,
    ok: (n: number) => `${n} fine`,
    topTitle: "What to fix first",
    tagFail: "critical",
    tagWarn: "attention",
    catTitle: "Score by area",
    catFail: "critical",
    catWarn: "attention",
    catOk: "fine",
    mapsOnlyTitle: "Map listings",
    aiTitle: "Readiness for AI search",
    okTitle: "What is already fine",
    atAddressStub: "The website address opens a hosting placeholder page",
    atAddressDead: "The website does not open",
    techAddress: "Response at the website address",
    noSiteNote: "Area scores are not calculated: there is nothing to score.",
    techTitle: "Technical details for your developer",
    techIntro: "An appendix for whoever makes the changes: what the measurement showed for each item.",
    techItem: "Item",
    techMeasure: "Measurement",
    footMeasured: (d: string) => `Measurements were taken automatically on ${d} from publicly visible signals, without access to your website or map accounts.`,
    footLegal: (c: string) => ` Legal checks follow ${c}.`,
    footRest: " Search rankings and AI assistant answers are not part of this report: they are measured separately, with a fixed set of queries. A repeat measurement after the work will show the change on the same items.",
    proposalKind: "Commercial proposal",
    proposalPreparing: "Preparing the proposal…",
    proposalHint: "Edit the plan and prices on the company page; the saved version is shown here",
    now: "Current state",
    scoreNote: (n: number) => `Website score based on ${n} checks: search, speed, mobile experience, enquiries.`,
    fallbackHeadline: "Website check result",
    fallbackHeadlineNone: "The website could not be checked",
    mainBlockers: "The main things holding back new customers:",
    plan: "Work plan",
    subtotal: "Cost of work",
    discount: "Discount",
    total: "Total",
    validFor: (n: number, until: string) => `This proposal is valid for ${n} ${n === 1 ? "day" : "days"}, until ${until}`,
    nothingPicked: "No work package is selected: tick the ones you need on the company page.",
    contacts: "Contact",
    phone: "Phone",
    email: "Email",
    docTitleReport: "Audit",
    docTitleProposal: "Proposal",
  },
} as const;

export const strings = (lang: Lang): Strings => S[lang] as unknown as Strings;

/** Правовая строка подвала: gb → UK law, us → US law. */
export function legalName(lang: Lang, country?: string | null): string {
  const c = (country ?? "").toLowerCase();
  if (lang === "ru") return countryName(c);
  if (c === "gb") return "UK law";
  if (c === "us") return "US law";
  try {
    return `the law of ${new Intl.DisplayNames(["en"], { type: "region" }).of(c.toUpperCase()) ?? c.toUpperCase()}`;
  } catch {
    return "local law";
  }
}

export const NO_SITE_STATES = ["stub", "dead", "none", "social"];

/** Заголовок по состоянию сайта и баллу (по-английски; по-русски его даёт сервер). */
export function headlineEn(state: string | undefined, score: number | null | undefined, fails: number): string {
  const n = score != null ? Math.round(score) : null;
  switch (state) {
    case "none": return "The business has no website";
    case "social": return "A social media page is used instead of a website";
    case "stub": return "The website address shows a placeholder, not a real website";
    case "dead": return "The website does not open";
    case "hidden": return "The website is not linked from the map listing";
    case "blocked": return "The website could not be checked automatically";
    case "chain": return "Part of a chain";
    case "closed": return "Listed as closed";
    case "good": return n != null ? `The website is in good shape: ${n}/100` : "The website is in good shape";
    case "weak":
    case "fair": {
      const tail = fails > 0 ? `, ${fails} critical ${fails === 1 ? "issue" : "issues"}` : "";
      return n != null ? `The website scores ${n}/100${tail}` : "The website needs work";
    }
    default: return n != null ? `The website scores ${n}/100` : "The website could not be fully checked";
  }
}

/** Вердикт отчёта. */
export function verdict(lang: Lang, state: string | undefined, score: number | null | undefined, fails: number): string {
  const noSite = NO_SITE_STATES.includes(state ?? "");
  if (lang === "en") {
    if (noSite) return "There is effectively no website: customers from maps and search go to competitors.";
    if (score == null) return "The website could not be fully checked.";
    if (score >= 75 && fails >= 5) return `The website is generally fine, but ${fails} critical issues hurt search and enquiries.`;
    if (score >= 75) return "The website is in good shape; a few targeted improvements remain.";
    if (score >= 50) return "There are noticeable problems that cost the website customers from search and maps.";
    return "The website is losing customers: many critical problems with search, maps and enquiries.";
  }
  if (noSite) return "Сайта по факту нет: клиенты с карт и из поиска уходят к конкурентам.";
  if (score == null) return "Сайт не удалось проверить полностью.";
  if (score >= 75 && fails >= 5) return `Сайт в целом в порядке, но ${fails} ${plural(fails, "критичная проблема мешает", "критичные проблемы мешают", "критичных проблем мешают")} поиску и заявкам.`;
  if (score >= 75) return "Сайт в хорошем состоянии; есть точечные улучшения.";
  if (score >= 50) return "Есть заметные проблемы, из-за которых сайт теряет клиентов из поиска и карт.";
  return "Сайт теряет клиентов: много критичных проблем с поиском, картами и заявками.";
}

/** Формулировка пункта: проблема или «в порядке», на языке материалов. */
export function labelOf(
  lang: Lang,
  r: MetricResult,
  def: MetricDef | undefined,
  m: { labels?: Record<string, string>; labels_ok?: Record<string, string>; labels_en?: Record<string, string>; labels_ok_en?: Record<string, string> },
): string {
  const own = lang === "en" ? r.plain_en : r.plain;
  if (own) return own;
  const ok = r.status === "pass";
  const src = lang === "en" ? (ok ? m.labels_ok_en : m.labels_en) : ok ? m.labels_ok : m.labels;
  return src?.[r.id] ?? def?.name ?? r.id;
}

/** Замер для приложения. По-английски русский текст замера не показываем (английских замеров сервер пока не отдаёт). */
export function measureOf(lang: Lang, r: MetricResult): string | null {
  const d = (r.detail ?? "").trim();
  if (lang === "ru") return d || null;
  if (d && !hasCyrillic(d)) return d;
  return null;
}

/** Пройденные пункты, которые безопасно хвалить: не из раздела, где есть провалы (иначе «в порядке» рядом с «плохо»). Для раздела I — только провалы I01, I02, I03, I05, I06. */
export function okShown<T extends { id: string; status: string }>(rows: T[]): T[] {
  const failed = new Set(rows.filter((r) => r.status === "fail").map((r) => r.id[0]));
  const aiBlock = rows.some((r) => ["I01", "I02", "I03", "I05", "I06"].includes(r.id) && r.status === "fail");
  return rows.filter((r) => r.status !== "pass" || !(r.id[0] === "I" ? aiBlock : failed.has(r.id[0])));
}

/** Что открывается по адресу сайта, для заглушки и мёртвого сайта. Код ответа и адрес хостера — только в приложение (см. atAddressTech). */
export function atAddress(_lang: Lang, site: SiteSummary | null, t: Strings, state: string): string | null {
  if (!site) return null;
  return state === "stub" ? t.atAddressStub : t.atAddressDead;
}

/** Технические данные ответа для приложения «для разработчика». */
export function atAddressTech(site: SiteSummary | null): string | null {
  if (!site) return null;
  const url = site.final_url ?? site.input_url;
  return [site.status ? `HTTP ${site.status}` : null, url, site.error].filter(Boolean).join(" · ") || null;
}
