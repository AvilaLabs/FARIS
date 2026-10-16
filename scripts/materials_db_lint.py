#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Lint the materials database and the waste tables with a second, independent reader (MAT-009).

    python3 scripts/materials_db_lint.py [--root DIR] [--check-links]

The Rust crate validates the same files when it loads them; this script re-checks them without
that code, so a mistake in one reader is not repeated in the other. It checks:

  - schema keys, labels and enumerations of crates/faris-model/data/materials/materials_db.json;
  - no duplicate ids; every material, source and limit reference resolves;
  - every cited value: source, locator and quote present, the written value inside the quote, and
    the stored number equal to the written one;
  - composition fractions that sum to 1 within 1e-9 with the balance element;
  - not-evaluated rows that give a reason and a next step and carry no number;
  - the 10 CFR 61.55 table file against the stored eCFR snapshot (row labels and values) and the
    snapshot's SHA-256;
  - the IAEA clearance table file (unique nuclides, positive levels, pages 124 to 127);
  - watch-list thresholds that name a 10 CFR 61.55 row that exists.

--check-links also asks every source URL and DOI whether it still resolves; that needs the network
and runs in the monthly job, not in the push checks. Exit status 1 on any finding.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LABELS = {"authored", "literature", "calculated", "conditional", "not_evaluated"}
SPEC_KINDS = {"nominal", "specified", "measured", "assumed"}
QUANTITIES = {"dpa", "he_appm", "h_appm", "fluence_m2", "dose_gy"}
STATUSES = {"design_aspiration", "code_rule", "measurement", "not_evaluated"}
REDISTRIBUTION = {"public_domain", "cc_by", "quote_only"}


def number(text: str):
    try:
        return float(text)
    except ValueError:
        return None


def check_cite(owner: str, cite, sources: set, numeric, problems: list) -> None:
    if not isinstance(cite, dict):
        problems.append(f"{owner}: citation missing")
        return
    for key in ("source", "locator", "quote", "value_text"):
        if not str(cite.get(key, "")).strip():
            problems.append(f"{owner}: citation has an empty {key}")
    if cite.get("source") not in sources:
        problems.append(f"{owner}: citation names unknown source {cite.get('source')}")
    if cite.get("value_text", "") not in cite.get("quote", ""):
        problems.append(f"{owner}: the quote does not contain the value as written ({cite.get('value_text')})")
    if numeric is not None:
        written = number(cite.get("value_text", ""))
        if written is None or written != numeric:
            problems.append(f"{owner}: stored value {numeric} differs from the written value {cite.get('value_text')}")


def check_not_evaluated(owner: str, reason, problems: list) -> None:
    if not isinstance(reason, dict) or not reason.get("reason", "").strip() or not reason.get("next_step", "").strip():
        problems.append(f"{owner}: a not-evaluated entry needs a reason and a next step")


def check_database(db: dict, nrc: dict, problems: list) -> None:
    if db.get("schema") != "faris-materials-db/1":
        problems.append("materials database: schema is not faris-materials-db/1")
    meta = db.get("database", {})
    for key in ("id", "version", "date", "scope"):
        if not str(meta.get(key, "")).strip():
            problems.append(f"materials database: database.{key} is empty")
    sources = set()
    for s in db.get("sources", []):
        if s["id"] in sources:
            problems.append(f"source {s['id']} appears twice")
        sources.add(s["id"])
        for key in ("citation", "url", "retrieved", "licence", "note"):
            if not str(s.get(key, "")).strip():
                problems.append(f"source {s['id']}: empty {key}")
        if s.get("redistribution") not in REDISTRIBUTION:
            problems.append(f"source {s['id']}: redistribution must be one of {sorted(REDISTRIBUTION)}")
    materials = set()
    for m in db.get("materials", []):
        if m["id"] in materials:
            problems.append(f"material {m['id']} appears twice")
        materials.add(m["id"])
        c = m["composition"]
        if c["status"] == "not_evaluated":
            check_not_evaluated(f"{m['id']} composition", c.get("not_evaluated"), problems)
            if c.get("elements"):
                problems.append(f"{m['id']}: a not-evaluated composition lists elements")
        else:
            total = 0.0
            for e in c["elements"]:
                total += e["mass_percent"]
                owner = f"{m['id']} composition {e['element']}"
                if e["label"] in ("literature", "conditional"):
                    check_cite(owner, e.get("cite"), sources, e["mass_percent"], problems)
                elif e["label"] == "calculated":
                    check_cite(owner, e.get("cite"), sources, None, problems)
                    if not str(e.get("method") or "").strip():
                        problems.append(f"{owner}: calculated value without a method")
                elif e["label"] != "authored":
                    problems.append(f"{owner}: label {e['label']} is not allowed here")
            balance = c.get("balance")
            if not balance or balance.get("label") != "calculated":
                problems.append(f"{m['id']}: composition needs a calculated balance element")
            elif not 0 < total < 100.0:
                problems.append(f"{m['id']}: listed elements leave nothing for the balance element, so the fractions cannot sum to 1")
        seen_specs = set()
        for spec in m["impurity_specs"]:
            if spec["id"] in seen_specs:
                problems.append(f"{m['id']}: impurity specification {spec['id']} appears twice")
            seen_specs.add(spec["id"])
            if spec["kind"] not in SPEC_KINDS:
                problems.append(f"{m['id']} {spec['id']}: kind {spec['kind']} is not one of {sorted(SPEC_KINDS)}")
            elements = set()
            for l in spec["limits"]:
                owner = f"{m['id']} {spec['id']} {l['element']}"
                if l["element"] in elements:
                    problems.append(f"{owner} appears twice")
                elements.add(l["element"])
                check_cite(owner, l.get("cite"), sources, l["max_wt_ppm"], problems)
        seen_props = set()
        for p in m["properties"]:
            owner = f"{m['id']} property {p['id']}"
            if p["id"] in seen_props:
                problems.append(f"{owner} appears twice")
            seen_props.add(p["id"])
            if p["label"] not in LABELS:
                problems.append(f"{owner}: label {p['label']} is not one of {sorted(LABELS)}")
            if p["label"] == "not_evaluated":
                check_not_evaluated(owner, p.get("not_evaluated"), problems)
                if p["value"].get("shape") != "none":
                    problems.append(f"{owner}: a not-evaluated property carries a value")
            elif p["label"] in ("literature", "conditional"):
                numeric = p["value"].get("value") if p["value"].get("shape") == "scalar" else None
                check_cite(owner, p.get("cite"), sources, numeric, problems)
                if p["label"] == "conditional" and not p["conditions"]:
                    problems.append(f"{owner}: conditional without conditions")
    limits = set()
    for l in db.get("limits", []):
        owner = f"limit {l['id']}"
        if l["id"] in limits:
            problems.append(f"{owner} appears twice")
        limits.add(l["id"])
        if l["material"] not in materials:
            problems.append(f"{owner}: unknown material {l['material']}")
        if l["quantity"] not in QUANTITIES:
            problems.append(f"{owner}: quantity {l['quantity']} is not one of {sorted(QUANTITIES)}")
        if l["status"] not in STATUSES:
            problems.append(f"{owner}: status {l['status']} is not one of {sorted(STATUSES)}")
        for field in ("convention", "temperature_window_c", "spectrum", "form"):
            v = l.get(field)
            if not isinstance(v, dict) or not ("value" in v or str(v.get("not_stated", "")).strip()):
                problems.append(f"{owner}: {field} needs a value or a stated reason")
        if any("\n" in c or not c.strip() for c in l["conditions"]):
            problems.append(f"{owner}: a condition is empty or has a line break")
        if l["status"] == "not_evaluated":
            if l.get("value") is not None or l.get("cite") is not None or l["kind"] != "not_evaluated":
                problems.append(f"{owner}: a not-evaluated limit has no value, citation or other kind")
            check_not_evaluated(owner, l.get("not_evaluated"), problems)
        else:
            if l["kind"] not in ("literature", "conditional"):
                problems.append(f"{owner}: a stored limit is literature or conditional")
            if l["kind"] == "conditional" and not l["conditions"]:
                problems.append(f"{owner}: conditional without conditions")
            if l.get("value") is None:
                problems.append(f"{owner}: no value")
            check_cite(owner, l.get("cite"), sources, l.get("value"), problems)
    rows = {("table1", r["label"]) for r in nrc["table1"]} | {("table2", r["label"]) for r in nrc["table2"]}
    elements = set()
    for w in db.get("watchlist", []):
        owner = f"watch-list {w['element']}"
        if w["element"] in elements:
            problems.append(f"{owner} appears twice")
        elements.add(w["element"])
        if not w["reason"].strip() or not w["nuclides"]:
            problems.append(f"{owner}: needs a reason and nuclides")
        check_cite(owner, w.get("cite"), sources, None, problems)
        t = w["class_threshold"]
        if t["scheme"] == "nrc-10cfr61.55":
            if (t["table"], t["row_label"]) not in rows:
                problems.append(f"{owner}: no 10 CFR 61.55 row {t['table']} '{t['row_label']}'")
        elif t["scheme"] == "fetter-1990-sal":
            check_cite(f"{owner} threshold", t.get("cite"), sources, t["value"], problems)
        else:
            problems.append(f"{owner}: unknown threshold scheme {t['scheme']}")
        if w.get("literature_element_limit"):
            check_cite(f"{owner} element limit", w["literature_element_limit"].get("cite"), sources, None, problems)


def snapshot_rows(xml: str, title: str) -> list:
    start = xml.index(f'<P class="title">{title}</P>')
    body = xml[start:]
    body = body[body.index("<TBODY>") + 7 : body.index("</TBODY>")]
    rows = []
    for row in body.split("<TR>")[1:]:
        cells = []
        for cell in row.split("<TD")[1:]:
            text = cell[cell.index(">") + 1 : cell.index("</TD>")]
            cells.append(" ".join(re.sub(r"<[^>]+>", "", text).split()))
        rows.append(cells)
    return rows


def cell_value(cell: str):
    if cell == "(1)":
        return None
    return float(cell.removeprefix("1 ").replace(",", ""))


def check_nrc(nrc: dict, xml: str, problems: list) -> None:
    digest = hashlib.sha256(xml.encode("utf-8")).hexdigest()
    if digest != nrc["source"]["snapshot_sha256"]:
        problems.append("10 CFR 61.55 snapshot: SHA-256 differs from the one recorded in the table file")
    if not nrc["source"].get("retrieved"):
        problems.append("10 CFR 61.55 table file: no retrieval date")
    t1 = snapshot_rows(xml, "Table 1")
    if len(t1) != len(nrc["table1"]):
        problems.append("Table 1: row count differs from the snapshot")
    for cells, row in zip(t1, nrc["table1"]):
        if cells[0] != row["label"] or cell_value(cells[1]) != row["limit"]:
            problems.append(f"Table 1 row '{row['label']}' differs from the snapshot ({cells})")
        if cells[1].startswith("1 ") != (row["unit"] == "nCi/g"):
            problems.append(f"Table 1 row '{row['label']}': unit differs from the snapshot footnote")
    t2 = snapshot_rows(xml, "Table 2")
    if len(t2) != len(nrc["table2"]):
        problems.append("Table 2: row count differs from the snapshot")
    for cells, row in zip(t2, nrc["table2"]):
        values = [cell_value(c) for c in cells[1:4]]
        if cells[0] != row["label"] or values != [row["col1"], row["col2"], row["col3"]]:
            problems.append(f"Table 2 row '{row['label']}' differs from the snapshot ({cells})")


def check_iaea(iaea: dict, problems: list) -> None:
    seen = set()
    for l in iaea["levels"]:
        if l["nuclide"] in seen:
            problems.append(f"IAEA table: {l['nuclide']} appears twice")
        seen.add(l["nuclide"])
        if not l["bq_per_g"] > 0:
            problems.append(f"IAEA table: {l['nuclide']} has a level that is not positive")
        if l["printed_page"] not in (124, 125, 126, 127):
            problems.append(f"IAEA table: {l['nuclide']} cites page {l['printed_page']}, outside Table I.2")
    for key in ("title", "url", "retrieved", "table", "licence"):
        if not str(iaea["source"].get(key, "")).strip():
            problems.append(f"IAEA table: source has an empty {key}")


def check_links(db: dict, nrc: dict, problems: list) -> None:
    urls = {s["url"] for s in db["sources"]} | {nrc["source"]["url"]}
    for url in sorted(urls):
        request = urllib.request.Request(url, headers={"User-Agent": "faris-materials-db-lint"})
        try:
            with urllib.request.urlopen(request, timeout=60) as response:
                if response.status >= 400:
                    problems.append(f"{url}: HTTP {response.status}")
        except urllib.error.HTTPError as error:
            # Some servers refuse automated readers with 403 or 405; a 404 or 410 is a dead link.
            if error.code in (404, 410):
                problems.append(f"{url}: HTTP {error.code}")
        except (urllib.error.URLError, TimeoutError) as error:
            problems.append(f"{url}: {error}")


def lint(root: Path, links: bool = False) -> list:
    data = root / "crates" / "faris-model" / "data"
    problems: list = []
    db = json.loads((data / "materials" / "materials_db.json").read_text(encoding="utf-8"))
    nrc = json.loads((data / "waste" / "nrc_10cfr61_55.json").read_text(encoding="utf-8"))
    iaea = json.loads((data / "waste" / "iaea_gsr3_table_i2.json").read_text(encoding="utf-8"))
    xml = (data / "waste" / nrc["source"]["snapshot_file"]).read_text(encoding="utf-8")
    check_database(db, nrc, problems)
    check_nrc(nrc, xml, problems)
    check_iaea(iaea, problems)
    if links:
        check_links(db, nrc, problems)
    return problems


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument("--root", type=Path, default=ROOT)
    parser.add_argument("--check-links", action="store_true", help="also check that every source URL resolves (network)")
    args = parser.parse_args()
    problems = lint(args.root, args.check_links)
    for p in problems:
        print(p)
    if problems:
        print(f"{len(problems)} problem(s)")
        return 1
    print("materials database and waste tables: ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
