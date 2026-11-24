# Шаблоны для сайта локального бизнеса

Заготовки к `CHECKLIST.md`. Всё в фигурных скобках `{…}` заменить фактами клиента; если факта нет,
строку убрать, а не оставлять заглушку: сканер ловит заглушки (F14).

## `<head>` страницы

```html
<!doctype html>
<html lang="ru">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>{Услуга} в {Городе} — {Название}</title>                <!-- 30–65 символов -->
  <meta name="description" content="{70–160 символов: услуга, район, выгода, призыв}">
  <link rel="canonical" href="https://{домен}/{путь}/">
  <meta property="og:type" content="website">
  <meta property="og:locale" content="ru_RU">
  <meta property="og:title" content="{как title}">
  <meta property="og:description" content="{как description}">
  <meta property="og:url" content="https://{домен}/{путь}/">
  <meta property="og:image" content="https://{домен}/og/{страница}.jpg"> <!-- 1200×630 -->
  <link rel="icon" href="/favicon.ico" sizes="48x48">
  <link rel="icon" href="/favicon.svg" type="image/svg+xml">
  <link rel="apple-touch-icon" href="/apple-touch-icon.png">       <!-- 180×180 -->
  <link rel="preload" href="/fonts/{основной}.woff2" as="font" type="font/woff2" crossorigin>
  <link rel="preload" href="/img/{hero}.avif" as="image" fetchpriority="high">
  <!-- Подтверждение сайта — коды из кабинетов клиента (C21) -->
  <meta name="google-site-verification" content="{код}">
  <meta name="yandex-verification" content="{код}">
  <meta name="msvalidate.01" content="{код}">
  <script type="application/ld+json">{разметка ниже}</script>
</head>
```

Шрифт: `@font-face { font-display: swap; … }` и подмножество `unicode-range` для кириллицы.
Hero-картинка: `<img src=… width=… height=… fetchpriority="high" alt="…">` без `loading="lazy"`;
остальные — `loading="lazy" decoding="async"` с `srcset`/`sizes`.

## robots.txt

```text
User-agent: *
Allow: /

# Поисковые ИИ-боты — разрешены явно (I02, I03)
User-agent: OAI-SearchBot
User-agent: ChatGPT-User
User-agent: Claude-SearchBot
User-agent: Claude-User
User-agent: PerplexityBot
User-agent: Perplexity-User
User-agent: DuckAssistBot
User-agent: YandexAdditional
Allow: /

# Обучающие боты (GPTBot, ClaudeBot, Google-Extended, Applebot-Extended, CCBot) — решает клиент (I04).
# Если клиент против обучения — раскомментировать:
# User-agent: GPTBot
# User-agent: ClaudeBot
# User-agent: Google-Extended
# User-agent: Applebot-Extended
# User-agent: CCBot
# Disallow: /

User-agent: Yandex
Allow: /
Clean-param: utm_source&utm_medium&utm_campaign&utm_content&utm_term&yclid&gclid

Sitemap: https://{домен}/sitemap.xml
```

Без `Host:` и `Crawl-delay:` (C05). Стейджинг закрывается паролем, а не `Disallow: /` — иначе
закрытие «уедет» в прод (C02, C10).

## JSON-LD: организация, услуги, крошки

```json
{
  "@context": "https://schema.org",
  "@graph": [
    {
      "@type": "Dentist",
      "@id": "https://{домен}/#org",
      "name": "{Название как в картах}",
      "url": "https://{домен}/",
      "logo": "https://{домен}/logo.png",
      "image": ["https://{домен}/img/{фасад}.jpg"],
      "telephone": "+7{цифры}",
      "email": "info@{домен}",
      "priceRange": "{₽₽ или «от 2 000 ₽»}",
      "address": {
        "@type": "PostalAddress",
        "streetAddress": "{улица, дом}",
        "addressLocality": "{город}",
        "postalCode": "{индекс}",
        "addressCountry": "RU"
      },
      "geo": { "@type": "GeoCoordinates", "latitude": {широта}, "longitude": {долгота} },
      "openingHoursSpecification": [
        { "@type": "OpeningHoursSpecification", "dayOfWeek": ["Monday","Tuesday","Wednesday","Thursday","Friday"], "opens": "09:00", "closes": "21:00" },
        { "@type": "OpeningHoursSpecification", "dayOfWeek": "Saturday", "opens": "10:00", "closes": "18:00" }
      ],
      "sameAs": [
        "https://yandex.ru/maps/org/{slug}/{id}/",
        "https://maps.google.com/?cid={cid}",
        "https://2gis.ru/{город}/firm/{id}",
        "https://t.me/{канал}",
        "https://vk.com/{группа}"
      ],
      "hasOfferCatalog": {
        "@type": "OfferCatalog",
        "name": "Услуги",
        "itemListElement": [
          {
            "@type": "Offer",
            "itemOffered": { "@type": "Service", "name": "{Услуга}", "url": "https://{домен}/uslugi/{услуга}/" },
            "priceSpecification": { "@type": "PriceSpecification", "minPrice": {цена}, "priceCurrency": "RUB" }
          }
        ]
      }
    },
    {
      "@type": "BreadcrumbList",
      "itemListElement": [
        { "@type": "ListItem", "position": 1, "name": "Главная", "item": "https://{домен}/" },
        { "@type": "ListItem", "position": 2, "name": "Услуги", "item": "https://{домен}/uslugi/" },
        { "@type": "ListItem", "position": 3, "name": "{Услуга}" }
      ]
    }
  ]
}
```

Подтип: Dentist, MedicalClinic, BeautySalon, HairSalon, DaySpa, AutoRepair, AutoWash, TireShop,
Restaurant, CafeOrCoffeeShop, Hotel, LegalService, Notary, RealEstateAgent, Florist, Optician,
HardwareStore, FurnitureStore, DrivingSchool… Никаких `aggregateRating`/`review` о себе (G06).
Для сети — отдельный узел на каждый филиал со своим адресом, телефоном и `@id`.

## Заголовки ответа

Netlify, Cloudflare Pages — файл `_headers`:

```text
/*
  Strict-Transport-Security: max-age=31536000; includeSubDomains
  Content-Security-Policy: default-src 'self'; img-src 'self' data: https:; script-src 'self' https://mc.yandex.ru https://mc.yandex.com; connect-src 'self' https://mc.yandex.ru https://mc.yandex.com; frame-src https://yandex.ru https://www.google.com; frame-ancestors 'self'; base-uri 'self'; form-action 'self'
  X-Content-Type-Options: nosniff
  Referrer-Policy: strict-origin-when-cross-origin
  Permissions-Policy: camera=(), microphone=(), geolocation=()
/_astro/*
  Cache-Control: public, max-age=31536000, immutable
/img/*
  Cache-Control: public, max-age=2592000
```

nginx:

```nginx
server {
    listen 80;
    listen [::]:80;
    server_name {домен} www.{домен};
    return 301 https://{домен}$request_uri;                    # B05: один 301 сразу на итог
}
server {
    listen 443 ssl;
    listen [::]:443 ssl;
    http2 on;                                                   # B10
    server_name www.{домен};
    return 301 https://{домен}$request_uri;                    # B06: одно зеркало
}
server {
    listen 443 ssl;
    listen [::]:443 ssl;
    http2 on;
    server_name {домен};
    ssl_protocols TLSv1.2 TLSv1.3;                              # O05
    brotli on; gzip on; gzip_types text/css application/javascript application/json image/svg+xml;  # B09
    add_header Strict-Transport-Security "max-age=31536000; includeSubDomains" always;
    add_header X-Content-Type-Options nosniff always;
    add_header Referrer-Policy strict-origin-when-cross-origin always;
    add_header Content-Security-Policy "default-src 'self'; …; frame-ancestors 'self'" always;
    location ~* \.(css|js|woff2|avif|webp|jpg|png|svg)$ { expires 30d; add_header Cache-Control "public"; }  # B11
    error_page 404 /404.html;                                   # B12: статус 404
}
```

CSP дополнить хостами виджетов клиента (запись, чат, карта, аналитика) и проверить консоль.

## Проверка вёрстки в браузере (E02–E04, E06, E07, Q02)

Сканер эти метрики пока не меряет. Выполнить в headless-браузере (Playwright `page.evaluate` или
chrome-devtools MCP `evaluate_script`) на 360×780, затем на 1366×768. Видимое окно браузера не
открывать, пока пользователь в полноэкранной игре (`hyprctl activewindow -j`, fullscreen ≠ 0).

```js
(() => {
  const vw = document.documentElement.clientWidth;
  const visible = (e) => { const r = e.getBoundingClientRect(); return r.width > 0 && r.height > 0; };
  const out = { vw };
  // E02: горизонтальная прокрутка и что торчит за край
  out.horizontalScroll = document.documentElement.scrollWidth > vw + 1;
  out.wideElements = [...document.querySelectorAll("body *")]
    .filter((e) => visible(e) && e.getBoundingClientRect().right > vw + 1 && getComputedStyle(e).position !== "fixed")
    .slice(0, 5).map((e) => e.tagName.toLowerCase() + (e.id ? "#" + e.id : "") + (typeof e.className === "string" && e.className ? "." + e.className.split(" ")[0] : ""));
  // E03: базовый шрифт и абзацы мельче 16 px
  out.bodyFont = parseFloat(getComputedStyle(document.body).fontSize);
  const paras = [...document.querySelectorAll("main p, main li")].filter((e) => visible(e) && e.textContent.trim().length > 20);
  out.smallParagraphs = `${paras.filter((e) => parseFloat(getComputedStyle(e).fontSize) < 16).length} из ${paras.length}`;
  // E04: тап-цели меньше 24×24 (ссылки внутри текста WCAG 2.5.8 не считает)
  const small = [...document.querySelectorAll("a[href], button, input:not([type=hidden]), select, textarea, [role=button]")].filter((e) => {
    if (!visible(e)) return false;
    const r = e.getBoundingClientRect();
    const inline = e.tagName === "A" && getComputedStyle(e).display === "inline" && e.closest("p, li, td");
    return !inline && (r.width < 24 || r.height < 24);
  });
  out.smallTapTargets = small.length;
  out.smallTapExamples = small.slice(0, 5).map((e) => (e.getAttribute("aria-label") || e.textContent || e.tagName).trim().slice(0, 40));
  // E06: перекрывает ли что-то центр экрана сразу после входа
  const mid = document.elementFromPoint(innerWidth / 2, innerHeight / 2);
  out.overlayOnEntry = !!mid?.closest("[role=dialog], dialog[open], [aria-modal=true]");
  // E07: объём видимого текста — сравнить значения на 360 и 1366 px (норма ≥ 90 %)
  out.visibleTextLength = document.body.innerText.length;
  // Q02: контраст текста к фону (фон-картинки не учитываются — проверить глазами)
  const lum = (c) => {
    const [r, g, b] = c.match(/[\d.]+/g).slice(0, 3).map(Number).map((v) => { v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; });
    return 0.2126 * r + 0.7152 * g + 0.0722 * b;
  };
  const bg = (e) => {
    for (; e; e = e.parentElement) {
      const c = getComputedStyle(e).backgroundColor, a = c.match(/[\d.]+/g);
      if (a && (a.length < 4 || +a[3] > 0.5)) return c;
    }
    return "rgb(255, 255, 255)";
  };
  const low = [];
  for (const e of document.querySelectorAll("p, li, a, span, h1, h2, h3, h4, label, button, td, th, small")) {
    if (!visible(e) || ![...e.childNodes].some((n) => n.nodeType === 3 && n.textContent.trim())) continue;
    const s = getComputedStyle(e), l1 = lum(s.color), l2 = lum(bg(e));
    const ratio = (Math.max(l1, l2) + 0.05) / (Math.min(l1, l2) + 0.05);
    const large = parseFloat(s.fontSize) >= 24 || (parseFloat(s.fontSize) >= 18.66 && +s.fontWeight >= 700);
    if (ratio < (large ? 3 : 4.5)) low.push(`${ratio.toFixed(2)}: ${e.textContent.trim().slice(0, 40)}`);
  }
  out.lowContrast = low.length;
  out.lowContrastExamples = low.slice(0, 5);
  return out;
})()
```

Норма: `horizontalScroll: false`, `bodyFont ≥ 16`, `smallTapTargets: 0`, `overlayOnEntry: false`,
`lowContrast: 0`, текст на 360 px ≥ 90 % от 1366 px. Заодно — консоль без ошибок (N07).

## Поиск заглушек перед сдачей

```sh
grep -rniE "lorem ipsum|dolor sit|в разработке|ваш текст|текст-заглушка|coming soon|under construction|placeholder|example\.(com|org)|\{[а-яa-z ]+\}|TODO|FIXME|\+7 ?\(?000|123-45-67" {папка сборки}/
```

Каждое совпадение — либо факт клиента вместо заглушки, либо удалить строку.

## Боты после выкладки (I05)

```sh
for ua in "OAI-SearchBot/1.0" "ChatGPT-User/1.0" "Claude-SearchBot/1.0" "PerplexityBot/1.0" \
          "Mozilla/5.0 (compatible; bingbot/2.0)" "Mozilla/5.0 (compatible; YandexAdditional/3.0)"; do
  printf '%s  %s\n' "$(curl -s -o /dev/null -w '%{http_code}' -A "$ua" https://{домен}/)" "$ua"
done
```

Везде 200. 403 или страница-челлендж — правило WAF/CDN режет ботов, его отключают для поисковых.
