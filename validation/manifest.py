# SPDX-License-Identifier: AGPL-3.0-only
"""Benchmark manifest schema and validator (requirements VAL-001, 027, 029, 070, 075).

A manifest describes one validation case. It holds small experimental or
reference tables with their attribution, and the sha256 of the upstream input
files, which stay outside the repository (as for ITER_1D). Run results are never
stored in a manifest; they live in sealed run records (see scoring.py).
"""
from __future__ import annotations

import hashlib
import json
import re
from pathlib import Path

from .scoring import COVARIANCE_KINDS, EVIDENCE_CLASSES, LITERATURE_KIND, ValidationError

SCHEMA = "faris.validation-case/1.0.0"
SHA256 = re.compile(r"^[0-9a-f]{64}$")
COMMIT = re.compile(r"^[0-9a-f]{40}$")
DOI = re.compile(r"^10\.\d{4,9}/\S+$")
SPDX = re.compile(r"^[A-Za-z0-9][A-Za-z0-9.+-]*$")
DATE = re.compile(r"^\d{4}-\d{2}-\d{2}$")


class ManifestError(ValidationError):
    """A manifest that does not meet the schema; the message lists every problem."""

    def __init__(self, problems: list[str]):
        self.problems = problems
        super().__init__("invalid validation manifest:\n  - " + "\n  - ".join(problems))


def _finite(x) -> bool:
    return isinstance(x, (int, float)) and not isinstance(x, bool) and x == x and abs(x) != float("inf")


def validate_manifest(m: dict) -> list[str]:
    """Return every problem found (empty list when the manifest is valid)."""
    p: list[str] = []

    def need(obj, key, where, typ=None, nonempty=True):
        if not isinstance(obj, dict) or key not in obj:
            p.append(f"{where}: missing '{key}'")
            return None
        v = obj[key]
        if typ is not None and not isinstance(v, typ):
            p.append(f"{where}.{key}: expected {typ.__name__}")
            return None
        if nonempty and v in ("", [], {}):
            p.append(f"{where}.{key}: must not be empty")
            return None
        return v

    if not isinstance(m, dict):
        return ["manifest must be a JSON object"]
    if m.get("schema") != SCHEMA:
        p.append(f"schema: expected {SCHEMA!r}")
    case_id = need(m, "case_id", "manifest", str)
    if case_id and not re.fullmatch(r"[a-z0-9][a-z0-9-]*", case_id):
        p.append("case_id: lowercase letters, digits and hyphens only")
    need(m, "title", "manifest", str)
    ev = need(m, "evidence_class", "manifest", str)
    if ev is not None and ev not in EVIDENCE_CLASSES:
        p.append(f"evidence_class: must be one of {EVIDENCE_CLASSES}, got {ev!r}")

    src = need(m, "source", "manifest", dict)
    if src is not None:
        need(src, "url", "source", str)
        commit, doi = src.get("commit"), src.get("doi")
        if not commit and not doi:
            p.append("source: needs a pinned commit or a DOI")
        if commit and not COMMIT.match(str(commit)):
            p.append("source.commit: must be a 40-character lowercase hex commit")
        if doi and not DOI.match(str(doi)):
            p.append("source.doi: malformed DOI")
        d = need(src, "retrieved", "source", str)
        if d and not DATE.match(d):
            p.append("source.retrieved: expected YYYY-MM-DD")

    lic = need(m, "license", "manifest", dict)
    if lic is not None:
        spdx = need(lic, "spdx", "license", str)
        if spdx and not SPDX.match(spdx):
            p.append("license.spdx: not an SPDX identifier")
        need(lic, "attribution", "license", str)

    files = need(m, "files", "manifest", list)
    for i, f in enumerate(files or []):
        where = f"files[{i}]"
        need(f, "role", where, str)
        need(f, "path_in_source", where, str)
        h = need(f, "sha256", where, str)
        if h and not SHA256.match(h):
            p.append(f"{where}.sha256: must be 64 lowercase hex characters")
    if m.get("files_are_external_not_vendored") is not True:
        p.append("files_are_external_not_vendored: must be true (upstream input files stay outside the repository)")

    norm = need(m, "normalisation", "manifest", dict)  # VAL-029
    if norm is not None:
        st = need(norm, "status", "normalisation", str)
        if st and st not in ("confirmed", "inferred", "unconfirmed"):
            p.append("normalisation.status: confirmed, inferred or unconfirmed")
        if "blocks_scoring" in norm and not isinstance(norm["blocks_scoring"], bool):
            p.append("normalisation.blocks_scoring: must be true or false")
        for key in ("quantity", "units", "source_normalisation", "location", "reaction_or_particle", "energy_integration", "basis"):
            need(norm, key, "normalisation", str)

    unc = need(m, "uncertainty", "manifest", dict)  # VAL-029 components
    if unc is not None:
        need(unc, "components", "uncertainty", list)
        need(unc, "note", "uncertainty", str)

    comp = need(m, "compatibility", "manifest", dict)  # VAL-027
    classes = need(m, "response_classes", "manifest", dict)
    if comp is not None:
        k = comp.get("k")
        if not _finite(k) or k <= 0:
            p.append("compatibility.k: must be a positive number")
        need(comp, "k_basis", "compatibility", str)
        cov = need(comp, "covariance", "compatibility", dict)
        if cov is not None:
            if cov.get("kind") not in COVARIANCE_KINDS:
                p.append(f"compatibility.covariance.kind: must be one of {COVARIANCE_KINDS}")
            elif cov["kind"] == "correlation" and not (_finite(cov.get("rho")) and -1 <= cov["rho"] <= 1):
                p.append("compatibility.covariance.rho: needs a number in [-1, 1]")
            elif cov["kind"] == "shared_normalisation" and not (_finite(cov.get("shared_relative_u")) and cov["shared_relative_u"] >= 0):
                p.append("compatibility.covariance.shared_relative_u: needs a number >= 0")
            elif cov["kind"] == "explicit" and not _finite(cov.get("value")):
                p.append("compatibility.covariance.value: needs a finite number")
            need(cov, "basis", "compatibility.covariance", str)
        d = need(comp, "declared", "compatibility", str)  # VAL-005: rule declared before results
        if d and not DATE.match(d):
            p.append("compatibility.declared: expected YYYY-MM-DD")

    if classes is not None:
        for name, c in classes.items():
            need(c, "unit", f"response_classes.{name}", str)
            need(c, "description", f"response_classes.{name}", str)

    dets = need(m, "detectors", "manifest", list)
    seen: set[str] = set()
    for i, d in enumerate(dets or []):
        where = f"detectors[{i}]"
        did = need(d, "id", where, str)
        if did:
            if did in seen:
                p.append(f"{where}.id: duplicate detector id {did!r}")
            seen.add(did)
        rc = need(d, "response_class", where, str)
        if rc and classes is not None and rc not in classes:
            p.append(f"{where}.response_class: {rc!r} is not declared in response_classes")
        ref = d.get("reference") if isinstance(d, dict) else None
        if ref is None:
            if isinstance(d, dict):
                need(d, "reference_missing_why", where, str)
                need(d, "reference_missing_next_step", where, str)
        else:
            if not _finite(ref.get("value")):
                p.append(f"{where}.reference.value: must be a finite number")
            u = ref.get("u")
            if u is not None and (not _finite(u) or u < 0):
                p.append(f"{where}.reference.u: must be a finite number >= 0 or null")
            if u is None:
                p.append(f"{where}.reference.u: missing uncertainty (a detector without one cannot be scored)")
        blocked = d.get("blocked") if isinstance(d, dict) else None
        if blocked is not None:
            need(blocked, "why", f"{where}.blocked", str)
            need(blocked, "next_step", f"{where}.blocked", str)

    nc = need(m, "not_covered", "manifest", list)  # VAL-075
    for i, item in enumerate(nc or []):
        if not isinstance(item, str) or not item.strip():
            p.append(f"not_covered[{i}]: must be a non-empty string")

    qr = need(m, "qualified_range", "manifest", dict)  # VAL-070
    if qr is not None:
        for key in ("materials", "geometry_class", "spectrum_class", "cooling_time"):
            need(qr, key, "qualified_range", None)
        need(qr, "parameters", "qualified_range", list, nonempty=False)

    lit = m.get("literature_context", [])
    if not isinstance(lit, list):
        p.append("literature_context: must be a list")
    else:
        for i, item in enumerate(lit):
            if not isinstance(item, dict) or item.get("kind") != LITERATURE_KIND:
                p.append(f"literature_context[{i}]: kind must be {LITERATURE_KIND!r} (literature is context only, never scored)")
    return p


def check_manifest(m: dict) -> dict:
    problems = validate_manifest(m)
    if problems:
        raise ManifestError(problems)
    return m


def load_manifest(path: Path | str) -> dict:
    path = Path(path)
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as err:
        raise ManifestError([f"{path}: cannot read manifest: {err}"]) from err
    return check_manifest(data)


def manifest_sha256(m: dict) -> str:
    return hashlib.sha256(json.dumps(m, sort_keys=True, separators=(",", ":"), ensure_ascii=True).encode("utf-8")).hexdigest()


def verify_files(m: dict, source_root: Path | str) -> list[str]:
    """Compare a local checkout of the upstream source with the manifest hashes; returns mismatches."""
    root = Path(source_root)
    issues = []
    for f in m["files"]:
        path = root / f["path_in_source"]
        if not path.is_file():
            issues.append(f"{f['path_in_source']}: missing")
            continue
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        if digest != f["sha256"]:
            issues.append(f"{f['path_in_source']}: sha256 {digest[:12]} differs from manifest {f['sha256'][:12]}")
    return issues
