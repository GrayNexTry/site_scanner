#!/usr/bin/env python3
"""Досье для проверки критичных ошибок скана: по N случайных сайтов на каждую проваленную метрику.

Запуск: python -I scripts/verify_sample.py [scan_id] [--per 7] [--metrics C02,I01] [--db путь]

Быстрый цикл без полного пересканирования:
  1. --pick-only picks.json — только выбрать сайты (по --per на ошибку) и сохранить номера;
  2. POST /api/scans/<id>/reaudit {"ids": [...]} — перепроверить только их свежей сборкой;
  3. --picks picks.json — досье по тем же сайтам из свежих результатов; где провала больше
     нет, сайт идёт в список «исправлено», без досье.
Для каждой метрики со статусом «провал» берёт до N компаний (разные сайты, случайно), заново
скачивает с сайта независимые доказательства — заголовки ответа, meta robots и viewport,
canonical, robots.txt, http-версию, фрагмент видимого текста — и пишет markdown-досье: что
утверждает сканер и что на самом деле на сайте. Досье читает человек или агент и выносит вердикт
по каждому утверждению; ложные срабатывания — повод править проверку (см. скилл scan-verify).
Только чтение: база открывается read-only, к сайтам — обычные GET, как у посетителя.
"""

import argparse
import concurrent.futures as cf
import datetime as dt
import gzip
import zlib
import json
import os
import random
import re
import sqlite3
import ssl
import urllib.error
import urllib.parse
import urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
UA = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/157.0.0.0 Safari/537.36"
HEADERS = ["server", "content-type", "location", "strict-transport-security", "content-security-policy",
           "x-frame-options", "x-content-type-options", "referrer-policy", "x-robots-tag", "cache-control", "content-encoding"]


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *a, **k):
        return None


def fetch(url: str, follow=True, limit=600_000):
    """(статус, конечный адрес, заголовки, тело, пометка о сертификате или ошибке)."""
    note = ""
    # Сначала со строгой проверкой сертификата; если не прошла — без неё, но с пометкой.
    for ctx in (ssl.create_default_context(), ssl._create_unverified_context()):  # noqa: S323
        handlers = [urllib.request.HTTPSHandler(context=ctx)] + ([] if follow else [NoRedirect()])
        opener = urllib.request.build_opener(*handlers)
        req = urllib.request.Request(url, headers={"User-Agent": UA, "Accept": "text/html,*/*;q=0.8", "Accept-Language": "ru-RU,ru;q=0.9,en;q=0.6"})
        try:
            with opener.open(req, timeout=20) as r:
                raw = r.read(limit)
                enc = (r.headers.get("Content-Encoding") or "").lower()
                try:
                    if "gzip" in enc:
                        raw = gzip.decompress(raw)
                    elif "deflate" in enc:
                        raw = zlib.decompress(raw)
                except Exception:  # noqa: BLE001 — обрезанный поток: показываем как есть
                    pass
                body = raw.decode(r.headers.get_content_charset() or "utf-8", "replace")
                return r.status, r.geturl(), {k.lower(): v for k, v in r.headers.items()}, body, note
        except urllib.error.HTTPError as e:
            body = e.read(limit).decode("utf-8", "replace") if e.fp else ""
            return e.code, url, {k.lower(): v for k, v in (e.headers or {}).items()}, body, note
        except ssl.SSLError as e:
            note = f"сертификат не прошёл проверку: {e.reason or e}"
        except urllib.error.URLError as e:
            if isinstance(e.reason, ssl.SSLError):
                note = f"сертификат не прошёл проверку: {e.reason}"
                continue
            return None, url, {}, "", f"не открылся: {e.reason}"
        except Exception as e:  # noqa: BLE001 — досье собирается и при сбое одного сайта
            return None, url, {}, "", f"не открылся: {e}"
    return None, url, {}, "", note


def tag(html: str, pattern: str):
    m = re.search(pattern, html, re.I | re.S)
    return m.group(1).strip() if m else None


def visible_text(html: str) -> str:
    html = re.sub(r"(?is)<(script|style|noscript|template|svg)[^>]*>.*?</\1>", " ", html)
    text = re.sub(r"(?s)<[^>]+>", " ", html)
    text = re.sub(r"&nbsp;|&#160;", " ", text)
    return re.sub(r"\s+", " ", text).strip()


def evidence(url: str) -> dict:
    try:
        return collect(url)
    except Exception as e:  # noqa: BLE001 — один сайт не должен ронять досье
        return {"url": url, "status": None, "final": url, "note": f"досье не собрано: {e}", "headers": {}}


def collect(url: str) -> dict:
    if "://" not in url:
        url = "http://" + url
    clean = urllib.parse.urlunsplit(urllib.parse.urlsplit(url)._replace(query="", fragment=""))
    st, final, hdr, body, note = fetch(clean)
    ev = {"url": clean, "status": st, "final": final, "note": note, "headers": {k: hdr[k][:160] for k in HEADERS if k in hdr}}
    if body:
        text = visible_text(body)
        ev |= {
            "title": tag(body, r"<title[^>]*>(.*?)</title>"),
            "meta_robots": tag(body, r"<meta[^>]+name=[\"']robots[\"'][^>]*content=[\"']([^\"']*)"),
            "viewport": tag(body, r"<meta[^>]+name=[\"']viewport[\"'][^>]*content=[\"']([^\"']*)"),
            "canonical": tag(body, r"<link[^>]+rel=[\"']canonical[\"'][^>]*href=[\"']([^\"']*)"),
            "lang": tag(body, r"<html[^>]*\blang=[\"']([^\"']*)"),
            "description": (tag(body, r"<meta[^>]+name=[\"']description[\"'][^>]*content=[\"']([^\"']*)") or "")[:160] or None,
            "h1": len(re.findall(r"<h1[\s>]", body, re.I)),
            "tel_links": len(re.findall(r"href=[\"']tel:", body, re.I)),
            "jsonld_types": sorted(set(re.findall(r"\"@type\"\s*:\s*\"([^\"]+)\"", body)))[:12],
            "words": len(text.split()),
            "text": text[:700],
            "lorem": "lorem ipsum" in body.lower(),
        }
    sp = urllib.parse.urlsplit(final or clean)
    origin = f"{sp.scheme}://{sp.netloc}"
    rst, _, _, rbody, rnote = fetch(origin + "/robots.txt", limit=40_000)
    ev["robots"] = {"status": rst, "note": rnote, "head": "\n".join(rbody.splitlines()[:40]) if rst == 200 else ""}
    hst, _, hhdr, _, hnote = fetch(f"http://{sp.netloc}/", follow=False, limit=2_000)
    ev["http"] = {"status": hst, "location": hhdr.get("location"), "note": hnote}
    return ev


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("scan_id", type=int, nargs="?")
    ap.add_argument("--per", type=int, default=7, help="сайтов на одну ошибку (5–10)")
    ap.add_argument("--metrics", default="", help="только эти метрики, через запятую")
    ap.add_argument("--cats", default="", help="только эти категории метрик: BCDE, FGHI, MNOPQ…")
    ap.add_argument("--db", default=os.path.expanduser("~/.local/share/maps-seo-scanner/scanner.db"))
    ap.add_argument("--out", default="")
    ap.add_argument("--pick-only", default="", help="только выбрать сайты и записать их номера в этот JSON")
    ap.add_argument("--picks", default="", help="досье по номерам из JSON, записанного --pick-only")
    a = ap.parse_args()

    catalog = {m["id"]: m for m in json.load(open(os.path.join(ROOT, "docs/metrics.json"), encoding="utf-8"))["metrics"]}
    con = sqlite3.connect(f"file:{a.db}?mode=ro", uri=True)
    con.row_factory = sqlite3.Row
    scan_id = a.scan_id or con.execute("SELECT max(id) FROM scans WHERE kind='map'").fetchone()[0]
    rows = con.execute(
        "SELECT id, name, phone, address, coalesce(json_extract(site, '$.final_url'), site_url) AS site_url, results "
        "FROM businesses WHERE scan_id=? AND audit_status='done' AND site_url IS NOT NULL",
        (scan_id,),
    ).fetchall()

    wanted = {m.strip().upper() for m in a.metrics.split(",") if m.strip()}
    by_metric: dict[str, list] = {}
    for r in rows:
        for m in json.loads(r["results"] or "[]"):
            mid = m["id"]
            # Только утверждения о сайте: карточки (A) и замеры J/K/L сюда не входят.
            if m.get("status") != "fail" or mid[0] in "AJKL" or (wanted and mid not in wanted) or (a.cats and mid[0] not in a.cats.upper()):
                continue
            by_metric.setdefault(mid, []).append((r, m))

    fixed: list[tuple[str, int, str]] = []
    rng = random.Random()
    samples = []
    if a.picks:
        saved = json.load(open(a.picks, encoding="utf-8"))
        by_id = {r["id"]: r for r in rows}
        for mid, total, ids in saved["picks"]:
            if a.cats and mid[0] not in a.cats.upper():
                continue
            picked = []
            for bid in ids:
                r = by_id.get(bid)
                m = next((x for x in json.loads(r["results"] or "[]") if x["id"] == mid), None) if r else None
                if m and m.get("status") == "fail":
                    picked.append((r, m))
                else:
                    fixed.append((mid, bid, (m or {}).get("status", "нет результата")))
            if picked:
                samples.append((mid, total, picked))
        by_metric = {}
    for mid, items in by_metric.items():
        rng.shuffle(items)
        seen, picked = set(), []
        for r, m in items:
            host = urllib.parse.urlsplit(r["site_url"]).netloc.removeprefix("www.")
            if host in seen:
                continue
            seen.add(host)
            picked.append((r, m))
            if len(picked) == a.per:
                break
        samples.append((mid, len(items), picked))
    if a.pick_only:
        out = {"scan_id": scan_id, "picks": [[mid, total, [r["id"] for r, _ in picked]] for mid, total, picked in samples]}
        json.dump(out, open(a.pick_only, "w", encoding="utf-8"), ensure_ascii=False)
        print(a.pick_only, "сайтов:", len({i for _, _, ids in out["picks"] for i in ids}))
        print(",".join(str(i) for i in sorted({i for _, _, ids in out["picks"] for i in ids})))
        return
    d = catalog.get
    samples.sort(key=lambda x: (not (d(x[0]) or {}).get("stop_factor"), -((d(x[0]) or {}).get("weight") or 0), x[0]))

    urls = sorted({r["site_url"] for _, _, picked in samples for r, _ in picked})
    with cf.ThreadPoolExecutor(12) as ex:
        ev = dict(zip(urls, ex.map(evidence, urls)))

    suffix = f"-{a.cats.upper()}" if a.cats else ""
    out = a.out or os.path.expanduser(f"~/.local/state/maps-seo-scanner/verify/scan-{scan_id}-{dt.datetime.now():%Y%m%d-%H%M}{suffix}.md")
    cut = lambda v, n=300: (str(v)[:n] + "…") if len(str(v)) > n else str(v)  # noqa: E731
    os.makedirs(os.path.dirname(out), exist_ok=True)
    with open(out, "w", encoding="utf-8") as f:
        f.write(f"# Проверка критичных ошибок — скан {scan_id}\n\n")
        f.write(f"Сайтов в выборке: {len(urls)}; ошибок: {len(samples)}; до {a.per} случайных сайтов на ошибку.\n")
        f.write("Для каждого утверждения сканера — свежие доказательства с сайта. Вердикт: ВЕРНО / ЛОЖНО (почему) / НЕ ПРОВЕРИТЬ.\n")
        if fixed:
            f.write(f"\nПосле перепроверки свежей сборкой провала больше нет ({len(fixed)}): "
                    + ", ".join(f"{mid} #{bid} → {st}" for mid, bid, st in fixed) + "\n")
        for mid, total, picked in samples:
            c = d(mid) or {}
            stop = " · СТОП-ФАКТОР" if c.get("stop_factor") else ""
            f.write(f"\n## {mid} — {c.get('name', '')}{stop}\nНорма: {c.get('rule', '')}. Провалов в скане: {total}.\n")
            for r, m in picked:
                e = ev.get(r["site_url"], {})
                f.write(f"\n### #{r['id']} {r['name']} — {r['site_url']}\n")
                f.write(f"- Сканер: {cut(m.get('detail', ''), 400)}\n")
                if r["phone"] or r["address"]:
                    f.write(f"- Карточка: {r['phone'] or '—'} · {r['address'] or '—'}\n")
                f.write(f"- Ответ: {e.get('status')} {cut(e.get('final'), 200)} {cut(e.get('note') or '', 200)}\n")
                if e.get("headers"):
                    f.write(f"- Заголовки: {json.dumps(e['headers'], ensure_ascii=False)}\n")
                for k in ("title", "meta_robots", "viewport", "canonical", "lang", "description", "h1", "tel_links", "jsonld_types", "words", "lorem"):
                    if e.get(k) not in (None, "", []):
                        f.write(f"- {k}: {cut(e[k])}\n")
                f.write(f"- http://: {e.get('http', {})}\n")
                rb = e.get("robots", {})
                f.write(f"- robots.txt: {rb.get('status')} {rb.get('note') or ''}\n")
                if rb.get("head") and mid[0] in "CI":
                    f.write("```\n" + "\n".join(cut(line, 200) for line in rb["head"].splitlines()) + "\n```\n")
                if e.get("text"):
                    f.write(f"- Текст: {e['text']}\n")
    print(out)


if __name__ == "__main__":
    main()
