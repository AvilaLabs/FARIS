#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Software bill of materials in SPDX 2.3 and CycloneDX 1.5 JSON (requirement SEC-030).

    cargo metadata --locked --offline --format-version 1 > metadata.json
    python3 scripts/generate_sbom.py --metadata metadata.json --out sbom/

writes `faris.spdx.json` and `faris.cdx.json` and validates both. The components are every package of the locked
Cargo graph for all platforms (a superset of what one binary links: the notices in docs/THIRD_PARTY_NOTICES.md are
the per-platform subsets), the workspace's own binaries, and the font files bundled in crates/faris-report/fonts,
each with the SHA-256 of the file. Registry crates carry the checksum from Cargo.lock.

The validation here is structural: required fields of each format and the seven NTIA minimum elements (supplier,
component name, version, other unique identifier, dependency relationship, author of the SBOM data, timestamp).
It is not the official JSON-schema validator, which needs network access to fetch; run one before publishing.
A supplier Cargo does not record is written NOASSERTION, never guessed.

The timestamp is SOURCE_DATE_EPOCH when set (so a rebuild gives the same bytes), else the time of the run.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import sys
import tomllib
import uuid
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TOOL = "faris-generate-sbom"
FONT_DIR = Path("crates") / "faris-report" / "fonts"
FONT_LICENCES = {"Hack-Regular.ttf": "MIT", "Ubuntu-Light.ttf": "LicenseRef-Ubuntu-font-1.0"}
REGISTRY = "registry+https://github.com/rust-lang/crates.io-index"
NOASSERTION = "NOASSERTION"
NAMESPACE = uuid.UUID("5d3f1f2e-4b43-4a6e-9a27-6c2f0f0f7a11")


def normalise_licence(expression: str | None) -> str | None:
    """Cargo's older "A/B" and "A / B" separators written as SPDX "OR"; None if there is no declaration."""
    if not expression or not expression.strip():
        return None
    return re.sub(r"\s*/\s*", " OR ", expression.strip())


def timestamp() -> str:
    epoch = os.environ.get("SOURCE_DATE_EPOCH")
    moment = datetime.fromtimestamp(int(epoch), timezone.utc) if epoch else datetime.now(timezone.utc)
    return moment.strftime("%Y-%m-%dT%H:%M:%SZ")


def sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def lock_checksums(lock_text: str) -> dict[tuple[str, str], str]:
    packages = tomllib.loads(lock_text).get("package", [])
    return {(p["name"], p["version"]): p["checksum"] for p in packages if "checksum" in p}


def purl(package: dict) -> str:
    return f"pkg:cargo/{package['name']}@{package['version']}"


def gather(metadata: dict, root: Path, checksums: dict[tuple[str, str], str]) -> dict:
    """Components, binaries, fonts and dependency edges, independent of the output format."""
    members = set(metadata["workspace_members"])
    packages = {p["id"]: p for p in metadata["packages"]}
    components = []
    for pid, p in sorted(packages.items(), key=lambda kv: (kv[1]["name"], kv[1]["version"])):
        registry = p["source"] == REGISTRY
        components.append({
            "id": pid,
            "name": p["name"],
            "version": p["version"],
            "purl": purl(p),
            "licence": normalise_licence(p.get("license")),
            "supplier": ", ".join(p["authors"]) if p.get("authors") else None,
            "description": p.get("description"),
            "download": f"https://crates.io/api/v1/crates/{p['name']}/{p['version']}/download" if registry else None,
            "source": p["source"],
            "sha256": checksums.get((p["name"], p["version"])),
            "workspace": pid in members,
        })
    binaries = []
    for pid in sorted(members):
        p = packages[pid]
        for target in p["targets"]:
            if "bin" in target["kind"]:
                binaries.append({"name": target["name"], "version": p["version"], "crate": pid})
    fonts = []
    font_root = root / FONT_DIR
    if font_root.is_dir():
        for path in sorted(font_root.glob("*.ttf")):
            fonts.append({"name": path.name, "sha256": sha256_file(path),
                          "licence": FONT_LICENCES.get(path.name)})
    edges = {}
    for node in metadata["resolve"]["nodes"]:
        edges[node["id"]] = sorted({d["pkg"] for d in node["deps"]})
    return {"components": components, "binaries": binaries, "fonts": fonts, "edges": edges}


def spdx_id(*parts: str) -> str:
    return "SPDXRef-" + re.sub(r"[^A-Za-z0-9.-]", "-", "-".join(parts))


def build_spdx(data: dict, version: str, created: str) -> dict:
    refs = {c["id"]: spdx_id("Package", c["name"], c["version"]) for c in data["components"]}
    packages, relationships = [], []
    for c in data["components"]:
        package = {
            "SPDXID": refs[c["id"]],
            "name": c["name"],
            "versionInfo": c["version"],
            "supplier": f"Person: {c['supplier']}" if c["supplier"] else NOASSERTION,
            "downloadLocation": c["download"] or NOASSERTION,
            "filesAnalyzed": False,
            "licenseConcluded": NOASSERTION,
            "licenseDeclared": c["licence"] or NOASSERTION,
            "copyrightText": NOASSERTION,
            "externalRefs": [{"referenceCategory": "PACKAGE-MANAGER", "referenceType": "purl",
                              "referenceLocator": c["purl"]}],
        }
        if c["sha256"]:
            package["checksums"] = [{"algorithm": "SHA256", "checksumValue": c["sha256"]}]
        if c["description"]:
            package["description"] = c["description"]
        packages.append(package)
    for f in data["fonts"]:
        ident = spdx_id("Font", f["name"])
        packages.append({
            "SPDXID": ident, "name": f["name"], "versionInfo": NOASSERTION, "supplier": NOASSERTION,
            "downloadLocation": NOASSERTION, "filesAnalyzed": False, "licenseConcluded": NOASSERTION,
            "licenseDeclared": f["licence"] or NOASSERTION, "copyrightText": NOASSERTION,
            "checksums": [{"algorithm": "SHA256", "checksumValue": f["sha256"]}],
            "comment": "Font file bundled in crates/faris-report/fonts.",
        })
        relationships.append({"spdxElementId": "SPDXRef-DOCUMENT", "relationshipType": "DESCRIBES",
                              "relatedSpdxElement": ident})
    for pid, deps in data["edges"].items():
        for dep in deps:
            relationships.append({"spdxElementId": refs[pid], "relationshipType": "DEPENDS_ON",
                                  "relatedSpdxElement": refs[dep]})
    for c in data["components"]:
        if c["workspace"]:
            relationships.append({"spdxElementId": "SPDXRef-DOCUMENT", "relationshipType": "DESCRIBES",
                                  "relatedSpdxElement": refs[c["id"]]})
    return {
        "spdxVersion": "SPDX-2.3",
        "dataLicense": "CC0-1.0",
        "SPDXID": "SPDXRef-DOCUMENT",
        "name": f"FARIS-{version}",
        "documentNamespace": f"https://github.com/AvilaLabs/FARIS/sbom/{version}/{uuid.uuid5(NAMESPACE, version + created)}",
        "creationInfo": {"created": created, "creators": [f"Tool: {TOOL}", "Organization: Avila Labs"]},
        "hasExtractedLicensingInfos": [{
            "licenseId": "LicenseRef-Ubuntu-font-1.0", "name": "Ubuntu Font Licence 1.0",
            "extractedText": "See licenses/ubuntu-font-licence-1.0.txt in the repository.",
        }],
        "packages": packages,
        "relationships": relationships,
    }


def build_cyclonedx(data: dict, version: str, created: str) -> dict:
    refs = {c["id"]: f"{c['name']}@{c['version']}" for c in data["components"]}
    components = []
    for c in data["components"]:
        item = {
            "type": "library", "bom-ref": refs[c["id"]], "name": c["name"], "version": c["version"],
            "purl": c["purl"], "supplier": {"name": c["supplier"] or NOASSERTION},
            "licenses": [{"expression": c["licence"]}] if c["licence"] else [],
        }
        if c["sha256"]:
            item["hashes"] = [{"alg": "SHA-256", "content": c["sha256"]}]
        if c["description"]:
            item["description"] = c["description"]
        components.append(item)
    for b in data["binaries"]:
        components.append({"type": "application", "bom-ref": f"bin:{b['name']}", "name": b["name"],
                           "version": b["version"], "supplier": {"name": "Avila Labs"},
                           "licenses": [{"expression": "AGPL-3.0-only"}],
                           "purl": f"pkg:generic/{b['name']}@{b['version']}"})
    for f in data["fonts"]:
        components.append({"type": "data", "bom-ref": f"font:{f['name']}", "name": f["name"],
                           "version": NOASSERTION, "supplier": {"name": NOASSERTION},
                           "hashes": [{"alg": "SHA-256", "content": f["sha256"]}],
                           "licenses": [{"license": {"name": f["licence"] or NOASSERTION}}]})
    dependencies = [{"ref": refs[pid], "dependsOn": [refs[d] for d in deps]} for pid, deps in data["edges"].items()]
    for b in data["binaries"]:
        dependencies.append({"ref": f"bin:{b['name']}", "dependsOn": [refs[b["crate"]]]})
    return {
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "serialNumber": f"urn:uuid:{uuid.uuid5(NAMESPACE, 'cdx' + version + created)}",
        "version": 1,
        "metadata": {
            "timestamp": created,
            "tools": {"components": [{"type": "application", "name": TOOL}]},
            "authors": [{"name": "Avila Labs"}],
            "component": {"type": "application", "bom-ref": "faris", "name": "FARIS", "version": version,
                          "licenses": [{"expression": "AGPL-3.0-only"}]},
        },
        "components": components,
        "dependencies": dependencies,
    }


# --- validation ---------------------------------------------------------------------------------

def validate_spdx(doc: dict) -> list[str]:
    problems = []
    for key in ("spdxVersion", "dataLicense", "SPDXID", "name", "documentNamespace", "creationInfo", "packages"):
        if key not in doc:
            problems.append(f"SPDX: missing {key}")
    if doc.get("spdxVersion") != "SPDX-2.3":
        problems.append("SPDX: spdxVersion is not SPDX-2.3")
    info = doc.get("creationInfo", {})
    if not info.get("created") or not info.get("creators"):
        problems.append("SPDX: creationInfo needs created and creators (NTIA: author and timestamp)")
    seen = set()
    for p in doc.get("packages", []):
        for key in ("SPDXID", "name", "downloadLocation", "filesAnalyzed", "licenseConcluded", "licenseDeclared",
                    "copyrightText", "supplier", "versionInfo"):
            if key not in p:
                problems.append(f"SPDX: package {p.get('name')} lacks {key}")
        ident = p.get("SPDXID", "")
        if ident in seen:
            problems.append(f"SPDX: duplicate id {ident}")
        seen.add(ident)
        if not re.fullmatch(r"SPDXRef-[A-Za-z0-9.-]+", ident):
            problems.append(f"SPDX: bad id {ident}")
        if ident.startswith("SPDXRef-Package") and not any(
                r.get("referenceType") == "purl" for r in p.get("externalRefs", [])):
            problems.append(f"SPDX: package {p.get('name')} has no unique identifier (purl)")
    seen.add("SPDXRef-DOCUMENT")
    depends = 0
    for r in doc.get("relationships", []):
        if r["spdxElementId"] not in seen or r["relatedSpdxElement"] not in seen:
            problems.append(f"SPDX: relationship names an unknown element: {r}")
        depends += r["relationshipType"] == "DEPENDS_ON"
    if doc.get("packages") and not depends:
        problems.append("SPDX: no dependency relationships (NTIA)")
    return problems


def validate_cyclonedx(doc: dict) -> list[str]:
    problems = []
    if doc.get("bomFormat") != "CycloneDX" or doc.get("specVersion") != "1.5":
        problems.append("CycloneDX: bomFormat/specVersion are not CycloneDX 1.5")
    if not re.fullmatch(r"urn:uuid:[0-9a-f-]{36}", doc.get("serialNumber", "")):
        problems.append("CycloneDX: serialNumber is not a urn:uuid")
    meta = doc.get("metadata", {})
    if not meta.get("timestamp") or not meta.get("authors"):
        problems.append("CycloneDX: metadata needs timestamp and authors (NTIA)")
    refs = set()
    for c in doc.get("components", []):
        for key in ("type", "bom-ref", "name", "version", "supplier"):
            if key not in c:
                problems.append(f"CycloneDX: component {c.get('name')} lacks {key}")
        if c.get("type") in ("library", "application") and "purl" not in c:
            problems.append(f"CycloneDX: component {c.get('name')} has no unique identifier (purl)")
        if c.get("bom-ref") in refs:
            problems.append(f"CycloneDX: duplicate bom-ref {c.get('bom-ref')}")
        refs.add(c.get("bom-ref"))
    refs.add(meta.get("component", {}).get("bom-ref"))
    deps = doc.get("dependencies", [])
    for d in deps:
        for ref in [d["ref"], *d["dependsOn"]]:
            if ref not in refs:
                problems.append(f"CycloneDX: dependency names unknown ref {ref}")
    if doc.get("components") and not any(d["dependsOn"] for d in deps):
        problems.append("CycloneDX: no dependency relationships (NTIA)")
    return problems


def generate(metadata: dict, root: Path, lock_text: str, version: str, created: str | None = None):
    created = created or timestamp()
    data = gather(metadata, root, lock_checksums(lock_text))
    spdx = build_spdx(data, version, created)
    cdx = build_cyclonedx(data, version, created)
    return spdx, cdx, validate_spdx(spdx) + validate_cyclonedx(cdx)


def workspace_version(root: Path) -> str:
    cargo = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
    return cargo["workspace"]["package"]["version"]


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--metadata", required=True, type=Path, help="output of cargo metadata --locked --offline --format-version 1")
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--root", type=Path, default=ROOT)
    ap.add_argument("--version", help="release version (default: the workspace version)")
    args = ap.parse_args(argv)
    metadata = json.loads(args.metadata.read_text(encoding="utf-8"))
    lock = (args.root / "Cargo.lock").read_text(encoding="utf-8")
    spdx, cdx, problems = generate(metadata, args.root, lock, args.version or workspace_version(args.root))
    if problems:
        print("\n".join(problems), file=sys.stderr)
        return 1
    args.out.mkdir(parents=True, exist_ok=True)
    (args.out / "faris.spdx.json").write_text(json.dumps(spdx, indent=2) + "\n", encoding="utf-8")
    (args.out / "faris.cdx.json").write_text(json.dumps(cdx, indent=2) + "\n", encoding="utf-8")
    print(f"SBOM: {len(spdx['packages'])} SPDX packages, {len(cdx['components'])} CycloneDX components, written to {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
