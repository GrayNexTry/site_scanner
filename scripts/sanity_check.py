#!/usr/bin/env python3
"""Поиск противоречий в выдаче скана — для итеративной правки проверок и логики лидов.

Запуск: python -I scripts/sanity_check.py [scan_id] [--db путь]
Читает базу сканера (по умолчанию ~/.local/share/maps-seo-scanner/scanner.db) только на чтение
и печатает правила, которые сработали, с примерами. Каждое правило — признак вероятной ошибки
проверки: «заглушка» при большом тексте, горячий лид при хорошем сайте и т. п. После правки
кода: перепроверить скан и запустить снова — список должен сокращаться.
"""

import argparse
import json
import os
import sqlite3
from collections import Counter, defaultdict


def load(db: str, scan_id: int):
    con = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
    con.row_factory = sqlite3.Row
    rows = con.execute(
        "SELECT id, name, website, site_url, lead, overall, report, results, site, discovery, data "
        "FROM businesses WHERE scan_id=? AND audit_status='done'",
        (scan_id,),
    ).fetchall()
    out = []
    for r in rows:
        j = lambda k: json.loads(r[k]) if r[k] else None  # noqa: E731
        results = {m["id"]: m for m in (j("results") or [])}
        out.append({
            "id": r["id"], "name": r["name"], "website": r["website"], "site_url": r["site_url"],
            "lead": r["lead"], "overall": r["overall"], "report": j("report") or {}, "results": results,
            "site": j("site") or {}, "discovery": j("discovery") or {}, "data": j("data") or {},
        })
    return out


def show(title: str, items: list, fmt, limit: int = 5):
    if not items:
        return
    print(f"\n## {title} — {len(items)}")
    for b in items[:limit]:
        print("  -", fmt(b))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("scan_id", type=int, nargs="?", default=None)
    ap.add_argument("--db", default=os.path.expanduser("~/.local/share/maps-seo-scanner/scanner.db"))
    a = ap.parse_args()
    con = sqlite3.connect(f"file:{a.db}?mode=ro", uri=True)
    scan_id = a.scan_id or con.execute("SELECT max(id) FROM scans WHERE kind='map'").fetchone()[0]
    bs = load(a.db, scan_id)
    print(f"# Скан {scan_id}: проверено компаний {len(bs)}")

    state = lambda b: (b["report"].get("mini") or {}).get("state")  # noqa: E731
    headline = lambda b: (b["report"].get("mini") or {}).get("headline", "")  # noqa: E731
    res = lambda b, m: b["results"].get(m) or {}  # noqa: E731
    label = lambda b: f"#{b['id']} {b['name']} · {b['site_url'] or b['website'] or '—'} · {headline(b)}"  # noqa: E731

    # 1. «Заглушка», хотя на главной много настоящего текста.
    show("Заглушка при большом тексте на главной (≥ 300 слов)",
         [b for b in bs if state(b) == "stub" and (b["site"].get("words") or 0) >= 300],
         lambda b: f"{label(b)} · слов {b['site'].get('words')}")

    # 2. «Не работает», хотя главная ответила 2xx.
    show("«Не работает» при ответе 2xx",
         [b for b in bs if state(b) == "dead" and 200 <= (b["site"].get("status") or 0) < 300], label)

    # 3. Горячий или тёплый лид при хорошем сайте — проверить, за что.
    show("Горячий/тёплый лид при балле сайта ≥ 75",
         [b for b in bs if b["lead"] in ("hot", "warm") and (b["overall"] or 0) >= 75],
         lambda b: f"{label(b)} · {(b['report'].get('lead_reasons') or [''])[0]}")

    # 4. Холодный лид без единой причины при слабом сайте.
    show("Холодный без причин при балле < 50",
         [b for b in bs if b["lead"] == "cold" and (b["overall"] if b["overall"] is not None else 100) < 50 and not b["report"].get("lead_reasons")],
         label)

    # 5. «Нет сайта», но сканер нашёл вероятный сайт — проверить вручную, не пропущен ли.
    show("Горячий «нет сайта», но есть предположение о сайте",
         [b for b in bs if state(b) == "none" and (b["discovery"].get("suggestions") or [])],
         lambda b: f"{label(b)} · {b['discovery']['suggestions'][0].get('domain')}")

    # 6. Один сайт у многих компаний, а сетью не помечены.
    by_host = defaultdict(list)
    for b in bs:
        u = b["site_url"] or ""
        host = u.split("://")[-1].split("/")[0].removeprefix("www.")
        if host:
            by_host[host].append(b)
    shared = [(h, g) for h, g in by_host.items() if len(g) >= 3 and any(state(b) != "chain" for b in g)]
    show("Общий сайт у ≥ 3 компаний, не все помечены сетью", shared,
         lambda hg: f"{hg[0]}: {len(hg[1])} компаний, сетью помечено {sum(state(b) == 'chain' for b in hg[1])}")

    # 7. Стоп-факторы: какие чаще всего опускают балл до 25 — кандидаты на ложные срабатывания.
    stops = Counter(s for b in bs for s in b["report"].get("stop_factors", []))
    if stops:
        print("\n## Стоп-факторы (балл ≤ 25)")
        for s, n in stops.most_common():
            ex = next(b for b in bs if s in b["report"].get("stop_factors", []))
            print(f"  - {s}: {n} · пример: {label(ex)} · {res(ex, s).get('detail', '')[:120]}")

    # 8. Метрики, проваленные у большинства проверенных сайтов: либо беда района, либо ошибка проверки.
    scored = [b for b in bs if b["overall"] is not None and state(b) != "chain"]
    fails = Counter()
    measured = Counter()
    for b in scored:
        for m, r in b["results"].items():
            if r.get("status") in ("pass", "warn", "fail"):
                measured[m] += 1
                if r["status"] == "fail":
                    fails[m] += 1
    high = sorted(((m, fails[m] / measured[m], fails[m]) for m in fails if measured[m] >= 20 and fails[m] / measured[m] >= 0.6),
                  key=lambda x: -x[1])
    if high:
        print(f"\n## Метрики с провалом у ≥ 60 % сайтов (из {len(scored)})")
        for m, share, n in high:
            details = Counter((b["results"][m].get("detail") or "")[:70] for b in scored if b["results"].get(m, {}).get("status") == "fail")
            print(f"  - {m}: {share:.0%} ({n}) · чаще всего: {details.most_common(1)[0][0]}")

    # 9. Сводка состояний и лидов.
    print("\n## Итог")
    print("  состояния:", dict(Counter(state(b) for b in bs).most_common()))
    print("  лиды:", dict(Counter(b["lead"] for b in bs).most_common()))


if __name__ == "__main__":
    main()
