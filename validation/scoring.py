# SPDX-License-Identifier: AGPL-3.0-only
"""Scoring rules for the validation suite (requirements VAL-001, 027-031, 035-037).

Standard library only. Every function is pure: the verdict of a row is derived
here from numbers and declared treatment, never entered by hand (VAL-006).

Verdicts keep their scope: PASS, FAIL, INCONCLUSIVE, NOT_EVALUATED, and STALE
(a row whose run identity no longer matches the current identity). Every
verdict other than PASS and FAIL carries a why and a next step.

Compatibility rule (VAL-027), declared per case before any result is read:

    |C - E| <= k * sqrt(u_C^2 + u_E^2 - 2 Cov(C, E))
"""
from __future__ import annotations

import hashlib
import json
import math
import random
from dataclasses import dataclass

PASS, FAIL, INCONCLUSIVE, NOT_EVALUATED, STALE = "PASS", "FAIL", "INCONCLUSIVE", "NOT_EVALUATED", "STALE"
VERDICTS = (PASS, FAIL, INCONCLUSIVE, NOT_EVALUATED, STALE)
EVIDENCE_CLASSES = ("verification", "code-to-code", "experiment")
COVARIANCE_KINDS = ("independent", "correlation", "shared_normalisation", "explicit", "unknown")
RUN_KIND, LITERATURE_KIND = "faris-run", "literature"
MIN_DETECTORS_FOR_BIAS = 5
MIN_BOOTSTRAP_RESAMPLES = 5000
DEFAULT_BOOTSTRAP_SEED = 20261009
# VAL-028: Monte Carlo error may not exceed this fraction of the reference uncertainty.
MC_DOMINANCE_RATIO = 0.5


class ValidationError(ValueError):
    """Input that the harness refuses to score."""


class AggregationError(ValidationError):
    """An aggregate across response classes, libraries or code versions (VAL-035)."""


@dataclass(frozen=True)
class Identity:
    """What a result was produced with (VAL-037)."""

    library_sha256: str
    code_version: str
    adapter_sha256: str

    def as_dict(self) -> dict:
        return {"library_sha256": self.library_sha256, "code_version": self.code_version, "adapter_sha256": self.adapter_sha256}

    @staticmethod
    def from_dict(raw: dict) -> "Identity":
        try:
            ident = Identity(str(raw["library_sha256"]), str(raw["code_version"]), str(raw["adapter_sha256"]))
        except (KeyError, TypeError) as err:
            raise ValidationError(f"identity needs library_sha256, code_version and adapter_sha256: {err}") from err
        if not all((ident.library_sha256, ident.code_version, ident.adapter_sha256)):
            raise ValidationError("identity fields must be non-empty")
        return ident

    def differences(self, current: "Identity") -> list[str]:
        return [name for name in ("library_sha256", "code_version", "adapter_sha256") if getattr(self, name) != getattr(current, name)]


def _finite(x) -> bool:
    return isinstance(x, (int, float)) and not isinstance(x, bool) and math.isfinite(x)


def covariance_term(spec: dict | None, c: float, e: float, u_c: float, u_e: float) -> float | None:
    """Cov(C, E) in the units of the response, or None when it is not known."""
    if not isinstance(spec, dict) or spec.get("kind") not in COVARIANCE_KINDS:
        raise ValidationError(f"covariance treatment must be an object with kind in {COVARIANCE_KINDS}")
    kind = spec["kind"]
    if kind == "unknown":
        return None
    if kind == "independent":
        return 0.0
    if kind == "correlation":
        rho = spec.get("rho")
        if not _finite(rho) or not -1.0 <= rho <= 1.0:
            raise ValidationError("correlation treatment needs rho in [-1, 1]")
        return rho * u_c * u_e
    if kind == "shared_normalisation":
        # Both values scale with one factor whose relative uncertainty is s: Cov = s^2 * C * E.
        s = spec.get("shared_relative_u")
        if not _finite(s) or s < 0:
            raise ValidationError("shared_normalisation treatment needs shared_relative_u >= 0")
        return s * s * c * e
    value = spec.get("value")
    if not _finite(value):
        raise ValidationError("explicit covariance treatment needs a finite value")
    return float(value)


def compatibility(c: float, e: float, u_c: float | None, u_e: float | None, k: float, covariance: dict,
                  u_c_mc: float | None = None) -> dict:
    """VAL-027 and VAL-028 for one scalar response. Returns verdict, numbers, why and next step."""
    if not _finite(k) or k <= 0:
        raise ValidationError("k must be a positive finite number")
    out: dict = {"verdict": None, "c": c, "e": e, "u_c": u_c, "u_e": u_e, "k": k, "covariance": covariance}

    def settle(verdict: str, why: str | None = None, next_step: str | None = None) -> dict:
        out.update(verdict=verdict, why=why, next_step=next_step)
        return out

    if not (_finite(c) and _finite(e)):
        return settle(NOT_EVALUATED, "a calculated or reference value is missing or not finite",
                      "supply the missing value from a run record or the case manifest")
    if not (_finite(u_c) and _finite(u_e)) or u_c < 0 or u_e < 0:
        return settle(INCONCLUSIVE, "a combined-uncertainty input is missing", "record both the calculated and the reference uncertainty")
    if u_c_mc is not None and _finite(u_c_mc) and u_c_mc > MC_DOMINANCE_RATIO * u_e:
        return settle(INCONCLUSIVE, f"Monte Carlo error {u_c_mc:.4g} exceeds {MC_DOMINANCE_RATIO} x reference uncertainty {u_e:.4g}",
                      f"rerun with more histories so the Monte Carlo error is at most {MC_DOMINANCE_RATIO * u_e:.4g}")
    cov = covariance_term(covariance, c, e, u_c, u_e)
    if cov is None:
        return settle(INCONCLUSIVE, "covariance between the calculated and reference value is unknown",
                      "state the covariance treatment for this case (independent, correlation, shared normalisation or explicit) with its basis")
    variance = u_c * u_c + u_e * u_e - 2.0 * cov
    if -1e-12 * (u_c * u_c + u_e * u_e) <= variance < 0:
        variance = 0.0  # round-off at perfect correlation
    out["covariance_value"] = cov
    if variance < 0:
        return settle(INCONCLUSIVE, "the stated covariance exceeds what the uncertainties allow (negative combined variance)",
                      "correct the covariance treatment or the uncertainties")
    allowed = k * math.sqrt(variance)
    out.update(difference=c - e, allowed=allowed)
    return settle(PASS if abs(c - e) <= allowed else FAIL)


def record_hash(record: dict) -> str:
    """sha256 of the canonical JSON of a record without its own hash field."""
    body = {key: value for key, value in record.items() if key != "record_sha256"}
    return hashlib.sha256(json.dumps(body, sort_keys=True, separators=(",", ":"), ensure_ascii=True).encode("utf-8")).hexdigest()


def seal_run_record(record: dict) -> dict:
    sealed = dict(record)
    sealed["record_sha256"] = record_hash(sealed)
    return sealed


def check_run_record(record: dict, case_id: str | None = None) -> Identity:
    """VAL-036: only hash-bound FARIS runs may feed a C/E table. Literature is refused here."""
    if not isinstance(record, dict):
        raise ValidationError("run record must be an object")
    kind = record.get("kind")
    if kind == LITERATURE_KIND:
        raise ValidationError("literature values are context only and are never scored (VAL-036)")
    if kind != RUN_KIND:
        raise ValidationError(f"run record kind must be {RUN_KIND!r}, got {kind!r}")
    for field in ("run_id", "case_id", "identity", "results", "record_sha256"):
        if field not in record:
            raise ValidationError(f"run record lacks {field}")
    if record["record_sha256"] != record_hash(record):
        raise ValidationError("run record hash does not match its content (edited after sealing)")
    if case_id is not None and record["case_id"] != case_id:
        raise ValidationError(f"run record is for case {record['case_id']!r}, not {case_id!r}")
    return Identity.from_dict(record["identity"])


def _percentile(sorted_values: list[float], q: float) -> float:
    """Linear interpolation between order statistics (q in [0, 100])."""
    if len(sorted_values) == 1:
        return sorted_values[0]
    pos = (len(sorted_values) - 1) * q / 100.0
    lo = math.floor(pos)
    hi = min(lo + 1, len(sorted_values) - 1)
    return sorted_values[lo] + (sorted_values[hi] - sorted_values[lo]) * (pos - lo)


def distribution(items: list[tuple[str, float]]) -> dict:
    """VAL-030: eight statistics over every detector, worst case named. items = [(detector_id, C/E)]."""
    if not items:
        raise ValidationError("a distribution needs at least one C/E value")
    values = sorted(v for _, v in items)
    n = len(values)
    mean = math.fsum(values) / n
    std = math.sqrt(math.fsum((v - mean) ** 2 for v in values) / (n - 1)) if n > 1 else None
    ordered = sorted(items, key=lambda it: (-abs(it[1] - 1.0), it[0]))
    return {
        "count": n, "mean": mean,
        "median": _percentile(values, 50.0), "std_dev": std,
        "p05": _percentile(values, 5.0), "p95": _percentile(values, 95.0),
        "min": values[0], "max": values[-1],
        "worst": {"detector_id": ordered[0][0], "ce": ordered[0][1], "abs_deviation_from_one": abs(ordered[0][1] - 1.0)},
        "worst_first": [{"detector_id": d, "ce": v} for d, v in ordered],
    }


def bias(items: list[tuple[str, float]], seed: int = DEFAULT_BOOTSTRAP_SEED, resamples: int = MIN_BOOTSTRAP_RESAMPLES) -> dict:
    """VAL-031: bias = mean log(C/E) with a 95 % percentile bootstrap interval over detectors."""
    if resamples < MIN_BOOTSTRAP_RESAMPLES:
        raise ValidationError(f"at least {MIN_BOOTSTRAP_RESAMPLES} bootstrap resamples are required")
    logs = [math.log(v) for _, v in items if v > 0 and math.isfinite(v)]
    excluded = len(items) - len(logs)
    out = {"n": len(logs), "excluded_non_positive": excluded, "seed": seed, "resamples": resamples, "method": "percentile bootstrap over detectors, 95 %"}
    if len(logs) < MIN_DETECTORS_FOR_BIAS:
        out.update(status="too few to estimate", bias=None, ci95=None, log_spread=None,
                   why=f"{len(logs)} usable detectors; at least {MIN_DETECTORS_FOR_BIAS} are needed for a bootstrap interval",
                   next_step="add detectors to the response class or report the individual C/E values only")
        return out
    n = len(logs)
    rng = random.Random(seed)
    means = sorted(math.fsum(rng.choices(logs, k=n)) / n for _ in range(resamples))
    mean = math.fsum(logs) / n
    spread = math.sqrt(math.fsum((x - mean) ** 2 for x in logs) / (n - 1))
    out.update(status="estimated", bias=mean, ci95=[_percentile(means, 2.5), _percentile(means, 97.5)], log_spread=spread)
    return out


def aggregate_ce(rows: list[dict]) -> dict:
    """The only C/E aggregate the harness makes: one response class, one library, one code version (VAL-035).

    Rows mixing classes, libraries or code versions raise; there is no headline number across them.
    """
    if not rows:
        raise ValidationError("nothing to aggregate")
    for field in ("response_class", "library_sha256", "code_version"):
        seen = sorted({str(r.get(field)) for r in rows})
        if len(seen) != 1:
            raise AggregationError(f"refusing to aggregate across {field}: {seen} (VAL-035)")
    return {"response_class": rows[0]["response_class"], "library_sha256": rows[0]["library_sha256"], "code_version": rows[0]["code_version"],
            "distribution": distribution([(r["detector_id"], r["ce"]) for r in rows])}


def score_case(manifest: dict, record: dict | None, current: Identity, bootstrap_seed: int = DEFAULT_BOOTSTRAP_SEED) -> dict:
    """Score one case. `manifest` is a validated manifest (see manifest.py); `record` a sealed FARIS run record or None."""
    case_id = manifest["case_id"]
    comp = manifest["compatibility"]
    base = {"case_id": case_id, "evidence_class": manifest["evidence_class"], "k": comp["k"], "covariance": comp["covariance"],
            "bootstrap_seed": bootstrap_seed, "current_identity": current.as_dict(), "rows": [], "classes": {}}
    if record is None:
        base["rows"] = [_row(manifest, d, None, None, NOT_EVALUATED,
                             "no FARIS run record exists for this case", "run the case runner and seal a run record") for d in manifest["detectors"]]
        return _finish(manifest, base, None)
    run_identity = check_run_record(record, case_id)
    base["run"] = {"run_id": record["run_id"], "record_sha256": record["record_sha256"], "identity": run_identity.as_dict()}
    differs = run_identity.differences(current)
    results = {r["detector_id"]: r for r in record["results"]}
    for det in manifest["detectors"]:
        res = results.get(det["id"])
        if det.get("blocked"):
            base["rows"].append(_row(manifest, det, res, run_identity, NOT_EVALUATED, det["blocked"]["why"], det["blocked"]["next_step"]))
        elif differs:
            base["rows"].append(_row(manifest, det, res, run_identity, STALE,
                                     f"run was made with a different {', '.join(differs)} than the current identity",
                                     "rerun the case with the current library, code and adapter, then reseal the record"))
        else:
            base["rows"].append(_score_detector(manifest, det, res, run_identity))
    return _finish(manifest, base, run_identity)


def _row(manifest, det, res, identity: Identity | None, verdict, why, next_step, extra: dict | None = None) -> dict:
    row = {
        "detector_id": det["id"], "response_class": det["response_class"], "evidence_class": manifest["evidence_class"],
        "verdict": verdict, "why": why, "next_step": next_step,
        "library_sha256": identity.library_sha256 if identity else None,
        "code_version": identity.code_version if identity else None,
        "adapter_sha256": identity.adapter_sha256 if identity else None,
        "c": res.get("value") if res else None, "e": (det.get("reference") or {}).get("value"), "ce": None,
    }
    if extra:
        row.update(extra)
    return row


def _score_detector(manifest: dict, det: dict, res: dict | None, identity: Identity) -> dict:
    ref = det.get("reference")
    if not ref:
        why = det.get("reference_missing_why") or "no reference value for this detector"
        return _row(manifest, det, res, identity, NOT_EVALUATED, why, det.get("reference_missing_next_step") or "obtain a reference value with provenance")
    if res is None:
        return _row(manifest, det, res, identity, NOT_EVALUATED, "the run record has no result for this detector", "rerun with this detector tallied")
    comp = manifest["compatibility"]
    c, e = res["value"], ref["value"]
    verdict = compatibility(c, e, res.get("u_total", res.get("u_mc")), ref.get("u"), comp["k"], comp["covariance"], res.get("u_mc"))
    ce = (c / e) if _finite(c) and _finite(e) and e != 0 else None
    why, nxt = verdict.get("why"), verdict.get("next_step")
    final = verdict["verdict"]
    if ce is None and final in (PASS, FAIL):
        final, why, nxt = NOT_EVALUATED, "C/E is undefined because the reference value is zero", "score this detector with the difference only, or merge it into a wider bin"
    # An unconfirmed normalisation keeps the C/E number visible but never lets it become PASS or FAIL.
    norm = manifest["normalisation"]
    if norm["status"] != "confirmed" and norm.get("blocks_scoring", False) and final in (PASS, FAIL):
        final = INCONCLUSIVE
        why = f"normalisation is {norm['status']}, not confirmed: {norm.get('why', norm['basis'])}"
        nxt = norm.get("next_step", "confirm the unit and normalisation of the reference values")
    row = _row(manifest, det, res, identity, final, why, nxt, {"u_c": res.get("u_total", res.get("u_mc")), "u_c_mc": res.get("u_mc"), "u_e": ref.get("u"),
                                                                  "difference": verdict.get("difference"), "allowed": verdict.get("allowed")})
    row["ce"] = ce
    return row


def _finish(manifest: dict, base: dict, identity: Identity | None) -> dict:
    """Per-class distributions and bias, computed from rows that have a C/E (any verdict) on one identity."""
    by_class: dict[str, list[dict]] = {}
    for row in base["rows"]:
        by_class.setdefault(row["response_class"], []).append(row)
    for name, rows in sorted(by_class.items()):
        counts = {v: sum(1 for r in rows if r["verdict"] == v) for v in VERDICTS}
        entry: dict = {"detectors": len(rows), "verdict_counts": counts}
        scored = [r for r in rows if r["ce"] is not None and r["verdict"] in (PASS, FAIL, INCONCLUSIVE)]
        if scored:
            agg = aggregate_ce(scored)
            entry["distribution"] = agg["distribution"]
            entry["bias"] = bias([(r["detector_id"], r["ce"]) for r in scored], base["bootstrap_seed"])
        else:
            entry["distribution"] = None
            entry["bias"] = None
            entry["why"] = "no detector in this class has a C/E value from a current run"
            entry["next_step"] = "run the case and resolve the blocked or missing detectors"
        base["classes"][name] = entry
    base["not_covered"] = manifest["not_covered"]
    base["qualified_range"] = manifest["qualified_range"]
    base["verdict_counts"] = {v: sum(1 for r in base["rows"] if r["verdict"] == v) for v in VERDICTS}
    return base
