#!/usr/bin/env python3
"""Генерирует docs/metrics.yaml из таблиц docs/METRICS.md.

METRICS.md — источник правды для текста (название, способ замера, правило, где).
Машинные поля (веса, числовые пороги, стоп-факторы, этап, признаки лида, отчёт)
задаются словарями ниже. Запуск: python -I scripts/gen_metrics_yaml.py
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "docs" / "METRICS.md"
OUT = ROOT / "docs" / "metrics.yaml"
OUT_JSON = ROOT / "docs" / "metrics.json"

# Веса категорий для общего балла (нормируются при расчёте).
CATEGORY_WEIGHTS = {
    "A": 20, "B": 6, "C": 12, "D": 10, "E": 5, "F": 8, "G": 4, "H": 5, "I": 10,
    "K": 6, "L": 3, "M": 3, "N": 6, "O": 2, "P": 2, "Q": 2,
    # Факторы Яндекса: коммерческие, региональные, фильтры (раздел R).
    "R": 8,
}

# Вес проверки (kind=check). По умолчанию 1, «(мелкий)» в правиле — 0.5.
WEIGHT_CRITICAL = {"A03", "A20", "A21", "B01", "B13", "C02", "C10", "E01", "I01", "I05", "I06"}
WEIGHT_IMPORTANT = {
    "A02", "A05", "A06", "A08", "A18", "B04", "B05", "C06", "C07", "C11", "C17", "C18",
    "D01", "D02", "D03", "D05", "E02", "F01", "F03", "F06", "F07", "F14", "G02", "H01",
    "H02", "I02", "I03", "I07", "I08", "K03", "M01", "M02", "M04", "N01", "N02", "N04",
    # Яндекс: нарушения, за которые понижают весь сайт.
    "R11", "R12", "R13", "R14",
}

# Стоп-факторы перекрывают общий балл.
STOP_FACTORS = {"A21", "B01", "B13", "C02", "C10", "E01", "I01", "I05"}

# Признаки лида: hot — горячий, warm — тёплый, alive — бизнес живой, solvent — платёжеспособен.
LEAD_SIGNALS = {
    "A03": "hot", "A04": "hot", "B01": "hot", "F14": "hot",
    "A20": "warm", "F13": "warm",
    "A06": "alive", "A07": "alive",
    "P05": "solvent", "N04": "solvent",
}

# Ядро отчёта «до/после».
IN_REPORT = {
    "A06", "A08", "A18", "A20", "A22", "A23",
    "C02", "C10", "C07", "C17", "C18", "D01", "D02", "D03", "D05",
    "I02", "I05", "I06", "I08", "I16", "I18", "I19", "I20",
    "K01", "K02", "K03", "K07", "L01", "N09", "N10",
    "C19", "R17", "R19",
}

# Дорогие внешние замеры — только для отобранных лидов.
AUDIT_STAGE = {
    "A20", "F09", "H06", "H07", "H08",
    "K01", "K02", "K03", "K04", "K05", "K06", "K08",
    "L01", "L02", "L03", "L04", "L05", "L06", "L07", "L08",
    "R20",
}
# Метрики с источником «Д (В≈)»: точное значение только с доступом.
REPORT_STAGE = {"C17", "C18"}

# Требуют проверки человеком или нечёткого сопоставления.
SEMI_AUTO = {"A11", "A13", "F07", "H06", "H08", "I10", "L04", "M06", "M07", "M08", "N01", "N06", "R14", "R20"}


def t(op: str, value, unit: str | None = None) -> dict:
    """Числовой порог: op ∈ {<=, <, >=, >, ==, between}."""
    d = {"op": op, "value": value}
    if unit:
        d["unit"] = unit
    return d


# Числовые пороги: (pass, warn). warn=None — промежуточной зоны нет.
THRESHOLDS = {
    "A07": (t(">=", 1, "reviews_90d"), None),
    "A08": (t(">=", 0.8, "ratio"), t(">", 0, "ratio")),
    "A09": (t("<=", 2, "days"), t("<=", 7, "days")),
    "A16": (t("<=", 30, "days"), t("<=", 90, "days")),
    "A17": (t(">=", 0.5, "ratio"), None),
    "A18": (t("==", 1.0, "ratio"), None),
    "A19": (t("==", 0, "count"), None),
    "B01": (t("<=", 5, "s"), None),
    "B02": (t(">=", 30, "days"), None),
    "B04": (t(">=", 14, "days"), None),
    "B07": (t("<=", 1, "hops"), None),
    "B08": (t("<=", 0.8, "s"), t("<=", 1.8, "s")),
    "B11": (t(">=", 30, "days"), None),
    "B14": (t(">=", 99.9, "percent"), None),
    "C01": (t("<=", 500, "KiB"), None),
    "C06": (t("<=", 50000, "urls"), None),
    "C07": (t(">=", 0.95, "ratio"), t(">=", 0.8, "ratio")),
    "C09": (t(">=", 0.95, "ratio"), None),
    "C12": (t("==", 0, "count"), None),
    "C13": (t("<", 0.05, "ratio"), None),
    "C14": (t("==", 0, "count"), None),
    "C15": (t("<=", 3, "clicks"), None),
    "C16": (t("==", 0, "count"), None),
    "C17": (t(">=", 0.9, "ratio"), None),
    "C18": (t(">=", 0.9, "ratio"), None),
    "C20": (t("==", 0, "count"), None),
    "C25": (t("<", 0.1, "ratio"), None),
    "D01": (t("<=", 2.5, "s"), t("<=", 4.0, "s")),
    "D02": (t("<=", 200, "ms"), t("<=", 500, "ms")),
    "D03": (t("<=", 0.1), t("<=", 0.25)),
    "D05": (t(">=", 90, "score"), t(">=", 50, "score")),
    "D06": (t("<=", 1.8, "s"), t("<=", 3.0, "s")),
    "D07": (t("<=", 200, "ms"), t("<=", 600, "ms")),
    "D08": (t("<=", 3, "MB"), None),
    "D11": (t(">=", 0.8, "ratio"), None),
    "D15": (t("<=", 250, "ms"), None),
    "E03": (t(">=", 16, "px"), None),
    "E04": (t(">=", 24, "px"), None),
    "E07": (t(">=", 0.9, "ratio"), None),
    "F01": (t("between", [30, 65], "chars"), None),
    "F02": (t("between", [70, 160], "chars"), None),
    "F03": (t("==", 1, "count"), None),
    "F09": (t(">=", 0.8, "ratio"), None),
    "F10": (t(">=", 0.9, "ratio"), None),
    "F15": (t("<=", 3, "per_1000_words"), None),
    "F16": (t(">=", 3, "links"), None),
    "G04": (t("==", 0, "errors"), None),
    "H02": (t("==", 1.0, "ratio"), None),
    "H07": (t(">=", 0.95, "ratio"), None),
    "I06": (t(">=", 0.9, "ratio"), None),
    "I09": (t(">=", 5, "count"), None),
    "I19": (t(">=", 10, "per_1000_words"), t(">=", 4, "per_1000_words")),
    "I21": (t("<=", 2, "MB"), None),
    "L03": (t("<", 0.1, "ratio"), None),
    "L06": (t("<", 0.3, "ratio"), None),
    "N05": (t("<=", 4, "fields"), t("<=", 6, "fields")),
    "N07": (t("==", 0, "count"), None),
    "O01": (t(">=", "B", "grade"), None),
    "O02": (t("==", 0, "count"), None),
    "O04": (t("==", 0, "count"), None),
    "Q01": (t(">=", 90, "score"), None),
    "Q02": (t(">=", 4.5, "contrast"), None),
    "Q03": (t("==", 1.0, "ratio"), None),
    "R06": (t(">=", 4, "channels"), t(">=", 2, "channels")),
    "R09": (t(">=", 10, "prices"), t(">=", 1, "prices")),
    "R11": (t("<=", 0.05, "ratio"), t("<=", 0.07, "ratio")),
    "R12": (t("==", 0, "count"), None),
    "R15": (t("<=", 0.2, "ratio"), t("<=", 0.4, "ratio")),
}

HEADING_RE = re.compile(r"^## ([A-R])\. (.+?)\s*$")
ROW_RE = re.compile(r"^\| ([A-R]\d{2}) \|(.+)\|\s*$")
KPI_PREFIXES = ("рост", "для отчёта", "vs ", "сравнить", "средняя", "основа плана")


def clean(s: str) -> str:
    return s.replace("**", "").strip()


def source_of(where: str) -> str:
    w = where.replace(" ", "")
    if w == "В":
        return "ext"
    if w == "Д":
        return "access"
    return "both"  # «Д/В», «Д (В≈)»


def kind_of(rule: str) -> str:
    low = rule.lower()
    if "информ" in low:
        return "info"
    if low.startswith(KPI_PREFIXES):
        return "kpi"
    return "check"


def parse(text: str) -> tuple[dict[str, str], list[dict]]:
    categories: dict[str, str] = {}
    metrics: list[dict] = []
    current = None
    for line in text.splitlines():
        if m := HEADING_RE.match(line):
            current = m.group(1)
            categories[current] = m.group(2)
            continue
        if m := ROW_RE.match(line):
            mid = m.group(1)
            cells = [c.strip() for c in m.group(2).split("|")]
            if len(cells) != 4:
                sys.exit(f"{mid}: ожидалось 4 колонки после ID, получено {len(cells)}")
            if current is None or not mid.startswith(current):
                sys.exit(f"{mid}: строка вне своей категории ({current})")
            name, method, rule, where = (clean(c) for c in cells)
            metrics.append({"id": mid, "name": name, "method": method, "rule": rule, "where": where})
    return categories, metrics


def enrich(m: dict) -> dict:
    mid, rule = m["id"], m["rule"]
    kind = kind_of(rule)
    source = source_of(m["where"])
    if kind != "check":
        weight = 0
    elif mid in WEIGHT_CRITICAL:
        weight = 3
    elif mid in WEIGHT_IMPORTANT:
        weight = 2
    elif "мелкий" in rule:
        weight = 0.5
    else:
        weight = 1
    if source == "access" or mid in REPORT_STAGE:
        stage = "report"
    elif mid in AUDIT_STAGE:
        stage = "audit"
    else:
        stage = "lead"
    pass_t, warn_t = THRESHOLDS.get(mid, (None, None))
    return {
        "id": mid,
        "category": mid[0],
        "name": m["name"],
        "method": m["method"],
        "rule": rule,
        "source": source,
        "kind": kind,
        "auto": "semi" if mid in SEMI_AUTO else True,
        "stage": stage,
        "weight": weight,
        "pass": pass_t,
        "warn": warn_t,
        "stop_factor": mid in STOP_FACTORS,
        "lead_signal": LEAD_SIGNALS.get(mid),
        "in_report": mid in IN_REPORT,
    }


def validate(categories: dict[str, str], metrics: list[dict]) -> None:
    ids = [m["id"] for m in metrics]
    dup = sorted({i for i in ids if ids.count(i) > 1})
    if dup:
        sys.exit(f"повторяются ID: {dup}")
    known = set(ids)
    for label, keys in (
        ("WEIGHT_CRITICAL", WEIGHT_CRITICAL), ("WEIGHT_IMPORTANT", WEIGHT_IMPORTANT),
        ("STOP_FACTORS", STOP_FACTORS), ("LEAD_SIGNALS", set(LEAD_SIGNALS)),
        ("IN_REPORT", IN_REPORT), ("AUDIT_STAGE", AUDIT_STAGE), ("REPORT_STAGE", REPORT_STAGE),
        ("SEMI_AUTO", SEMI_AUTO), ("THRESHOLDS", set(THRESHOLDS)),
    ):
        missing = sorted(keys - known)
        if missing:
            sys.exit(f"{label}: нет в каталоге {missing}")
    if set(categories) != set(CATEGORY_WEIGHTS):
        sys.exit(f"категории каталога {sorted(categories)} ≠ весам {sorted(CATEGORY_WEIGHTS)}")
    for m in metrics:
        if not m["name"] or not m["method"] or not m["rule"]:
            sys.exit(f"{m['id']}: пустое поле")
    # Нумерация внутри категории без пропусков: A01, A02, …
    for cat in categories:
        nums = [int(i[1:]) for i in ids if i[0] == cat]
        if nums != list(range(1, len(nums) + 1)):
            sys.exit(f"категория {cat}: нумерация с пропусками {nums}")


def main() -> None:
    categories, raw = parse(SRC.read_text(encoding="utf-8"))
    validate(categories, raw)
    metrics = [enrich(m) for m in raw]
    for m in metrics:
        if m["stop_factor"] and m["kind"] != "check":
            sys.exit(f"{m['id']}: стоп-фактор должен быть проверкой")
    doc = {
        "version": "2026-11-24",
        "source": "docs/METRICS.md",
        "legend": {
            "source": {"ext": "внешний скан", "access": "нужен доступ клиента", "both": "грубо внешне, точно с доступом"},
            "kind": {"check": "pass/warn/fail, входит в балл", "kpi": "значение во времени", "info": "справочно"},
            "stage": {"lead": "массовый поиск лидов", "audit": "дорогие внешние замеры", "report": "отчёт с доступом"},
            "lead_signal": {"hot": "горячий", "warm": "тёплый", "alive": "бизнес живой", "solvent": "платёжеспособен"},
            "score": {"pass": 100, "warn": 50, "fail": 0},
        },
        "categories": categories,
        "category_weights": CATEGORY_WEIGHTS,
        "metrics": metrics,
    }
    header = "# Сгенерировано scripts/gen_metrics_yaml.py из docs/METRICS.md — не править вручную.\n"
    body = yaml.safe_dump(doc, allow_unicode=True, sort_keys=False, width=120)
    OUT.write_text(header + body, encoding="utf-8")
    # Тот же каталог в JSON: его встраивает в бинарник scanner-core (include_str!).
    OUT_JSON.write_text(json.dumps(doc, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")

    by_cat = {c: sum(m["category"] == c for m in metrics) for c in categories}
    count = lambda key, val: sum(m[key] == val for m in metrics)  # noqa: E731
    print(f"метрик: {len(metrics)}  " + " ".join(f"{c}{n}" for c, n in by_cat.items()))
    print(f"source: ext {count('source', 'ext')}, access {count('source', 'access')}, both {count('source', 'both')}")
    print(f"kind: check {count('kind', 'check')}, kpi {count('kind', 'kpi')}, info {count('kind', 'info')}")
    print(f"stage: lead {count('stage', 'lead')}, audit {count('stage', 'audit')}, report {count('stage', 'report')}")
    print(f"с числовым порогом: {sum(m['pass'] is not None for m in metrics)}, "
          f"стоп-факторов: {count('stop_factor', True)}, в отчёте: {count('in_report', True)}")
    print(f"записано: {OUT.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
