import copy
import hashlib
import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).parent
REPO = HERE.parent


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, HERE / filename)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


MV = load("maintenance_validation", "maintenance_validation.py")
REFERENCE = REPO / "references" / "maintenance-coupling-test-a2.json"
ASSUMPTIONS = REPO / "scenarios" / "arc-inspired" / "demountable-magnet-assumptions.json"
DAY = MV.DAY_S
NAMES = MV.ARRANGEMENTS


def history(life, count, downtime=1.0e8):
    return {"outcome": "horizon_completed", "lifetime_net_electricity_mwh": life, "total_replacement_downtime_s": downtime,
            "replacements": [{"component": "blanket", "k": k, "start_s": 1.0, "end_s": 2.0} for k in range(count)]}


def d3_doc(ratio=14.0, considered=True, changed=True):
    rows = [{"contrast": label, "status": "EVALUATED", "fixed_difference_s": -3.0e6, "computed_difference_s": -3.0e6 * ratio,
             "ratio_computed_over_fixed": ratio, "considered": considered, "changed": changed}
            for label in ("breeder-minus-reference, no port", "breeder-minus-reference, port")]
    return {"status": "EVALUATED", "changed": changed, "contrasts": rows, "band": [0.8, 1.25], "min_fixed_difference_s": 2592000.0}


def synthetic(fixed_life=None, computed_life=None, fixed_counts=None, computed_counts=None, d3=None, variant=MV.BARE,
              statuses=None):
    """A minimal result with the structure of the amended run's record."""
    fixed_life = fixed_life or {"no-port/reference": 10.0, "no-port/breeder": 9.0, "port/reference": 8.0, "port/breeder": 7.0}
    computed_life = computed_life or {"no-port/reference": 9.0, "no-port/breeder": 10.0, "port/reference": 7.0,
                                      "port/breeder": 8.0}
    fixed_counts = fixed_counts or {n: 30 for n in NAMES}
    computed_counts = computed_counts or {n: 31 for n in NAMES}
    cases = {}
    for i, name in enumerate(NAMES):
        cases[name] = {"status": "EVALUATED", "durations_s": {"blanket": [1.0 + i], "magnets": [2.0 + i]},
                       "history": history(computed_life[name], computed_counts[name], 5.0 + i)}
        if statuses and name in statuses:
            cases[name] = {"status": "NOT_EVALUATED", "reason": statuses[name]}
    return {
        "computed_model": {variant: {"w": {"0.5": {
            "cases": cases, "q_star": {"blanket": {"q_star": 1.0}},
            "decisions": {"D1": {"status": "EVALUATED", "changed": True}, "D3": d3 or d3_doc()}}}}},
        "fixed_model": {n: {"status": "EVALUATED", "history": history(fixed_life[n], fixed_counts[n], 3.0)} for n in NAMES},
        "inputs_sha256": {"config": "x"},
    }


def outcomes(result, variant=MV.BARE):
    return {c: v["outcome"] for c, v in MV.evaluate_claims(result, variant).items()}


class ConfigTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.addCleanup(self.tmp.cleanup)
        base = self.root / "base"
        base.mkdir()
        (base / "assumptions.json").write_text(ASSUMPTIONS.read_text())
        arrangements = {}
        for name in NAMES:
            stem = name.replace("/", "_")
            arrangements[name] = {}
            for key in MV.ARRANGEMENT_KEYS:
                (base / f"{stem}.{key}.json").write_text(json.dumps({"k": key}))
                arrangements[name][key] = f"{stem}.{key}.json"
        self.base_cfg = {"faris": "/bin/faris", "actinv": "/bin/actinv", "data_dir": "data", "assumptions": "assumptions.json",
                         "output_dir": "out", "arrangements": arrangements, "sweep": {"a": {}}, "amendment": 2,
                         "w_values": [0.25, 0.5, 0.75], "f_values": [0.5, 1.0]}
        (base / "config.json").write_text(json.dumps(self.base_cfg))
        self.base = base / "config.json"
        self.out = self.root / "val"
        self.runs = self.root / "v4"
        for run in MV.V4_RUNS:
            (self.runs / run).mkdir(parents=True)
            (self.runs / run / "run.json").write_text("{}")

    def cfg(self, variant):
        return json.loads((self.out / variant / "config.json").read_text())

    def limits(self, variant):
        doc = json.loads((self.out / variant / "assumptions.json").read_text())
        return doc["service_limits"]

    def test_every_variant_has_the_common_keys(self):
        MV.make_configs(self.base, self.out, self.runs)
        for variant in MV.VARIANTS:
            cfg = self.cfg(variant)
            self.assertEqual(cfg["amendment"], 2, variant)
            self.assertIs(cfg["allow_reduced_grid"], True)
            self.assertEqual((cfg["w_values"], cfg["f_values"], cfg["sweep"]), ([0.5], [1.0], {}))
            self.assertEqual(sorted(cfg["arrangements"]), sorted(NAMES))
            self.assertEqual(cfg["output_dir"], str(self.out / variant / "out"))
            self.assertEqual(cfg["result"], str(self.out / variant / "result.json"))
            self.assertEqual(cfg["decay_cache"], str(self.out / "decay-cache"))
            self.assertTrue(Path(cfg["arrangements"]["port/breeder"]["scenario"]).is_absolute())
            info = json.loads((self.out / variant / "variant.json").read_text())
            self.assertEqual(info["variant"], variant)

    def test_baseline_changes_nothing_else(self):
        MV.make_configs(self.base, self.out, self.runs)
        cfg = self.cfg("B")
        for key in ("governing_quantity", "photon_response", "class_w", "impurities"):
            self.assertNotIn(key, cfg)
        self.assertEqual(cfg["assumptions"], str((self.base.parent / "assumptions.json").resolve()))
        self.assertEqual(json.loads((self.out / "B" / "variant.json").read_text())["derived_files"], {})

    def test_v1_and_v3_keys(self):
        MV.make_configs(self.base, self.out, self.runs)
        self.assertEqual(self.cfg("V1")["governing_quantity"], "dose")
        self.assertEqual(self.cfg("V1")["photon_response"], "/home/connoravila/nuclear-data/photon-response/nist-xcom-all.json")
        self.assertTrue(self.cfg("V3")["impurities"].endswith("scenarios/arc-inspired/impurities/specification-maximum.json"))

    def test_v2_blanket_durations_and_class_w(self):
        MV.make_configs(self.base, self.out, self.runs)
        for variant, months in (("V2-low", 3.2), ("V2-high", 5.9)):
            work = months * 30.4375 * 86400.0
            total = 30 * 86400.0 + work
            for entry in self.limits(variant):
                if entry["component_id"] == "blanket":
                    self.assertEqual(entry["replacement_duration_s"], total)
                else:
                    self.assertEqual(entry["replacement_duration_s"], 10368000)
            self.assertEqual(self.cfg(variant)["class_w"], {"blanket": work / total})
            self.assertEqual(self.cfg(variant)["assumptions"], str(self.out / variant / "assumptions.json"))

    def test_v5_limits_scaled(self):
        MV.make_configs(self.base, self.out, self.runs)
        base = {(e["component_id"], e["response_id"]): e["limit"] for e in json.loads(ASSUMPTIONS.read_text())["service_limits"]}
        for variant, comp, factor in (("V5a", "blanket", 0.8), ("V5b", "blanket", 1.2), ("V5c", "magnets", 0.8),
                                      ("V5d", "magnets", 1.2)):
            got = {(e["component_id"], e["response_id"]): e["limit"] for e in self.limits(variant)}
            for key, value in base.items():
                self.assertEqual(got[key], value * factor if key[0] == comp else value, (variant, key))
            self.assertEqual(sum(1 for k in got if k[0] == comp), 1 if comp == "blanket" else 3)

    def test_variant_json_records_derived_hashes(self):
        MV.make_configs(self.base, self.out, self.runs)
        info = json.loads((self.out / "V5a" / "variant.json").read_text())
        digest = hashlib.sha256((self.out / "V5a" / "assumptions.json").read_bytes()).hexdigest()
        self.assertEqual(info["derived_files"], {"assumptions.json": digest})
        self.assertTrue(info["changes"])

    def test_v4_replaces_spectrum_runs(self):
        MV.make_configs(self.base, self.out, self.runs)
        arr = self.cfg("V4")["arrangements"]
        for run, name in MV.V4_RUNS.items():
            self.assertEqual(arr[name]["spectrum_run"], str((self.runs / run / "run.json").resolve()))
        self.assertNotEqual(arr["port/breeder"]["spectrum_run"], self.cfg("B")["arrangements"]["port/breeder"]["spectrum_run"])

    def test_v4_refused_without_runs(self):
        with self.assertRaises(MV.Refused):
            MV.make_configs(self.base, self.out, None)
        (self.runs / "port-breeder" / "run.json").unlink()
        with self.assertRaises(MV.Refused):
            MV.make_configs(self.base, self.out, self.runs)
        self.assertFalse((self.out / "B").exists())


class ClaimTests(unittest.TestCase):
    def test_all_hold_on_the_swap(self):
        self.assertEqual(outcomes(synthetic()), {c: MV.HOLDS for c in MV.CLAIMS})

    def test_c1_no_swap_when_fixed_order_is_already_breeder_first(self):
        result = synthetic(fixed_life={"no-port/reference": 9.0, "no-port/breeder": 10.0, "port/reference": 8.0,
                                       "port/breeder": 7.0})
        claims = MV.evaluate_claims(result, MV.BARE)
        self.assertEqual(claims["C1"]["outcome"], MV.FAILS)
        self.assertTrue(claims["C1"]["reason"].startswith("no swap: fixed order already"))
        self.assertEqual(claims["C1"]["fixed_order"][0], "no-port/breeder")
        self.assertEqual(claims["C2"]["outcome"], MV.HOLDS)

    def test_c1_fails_when_computed_order_does_not_swap(self):
        result = synthetic(computed_life={"no-port/reference": 10.0, "no-port/breeder": 9.0, "port/reference": 7.0,
                                          "port/breeder": 8.0})
        claims = MV.evaluate_claims(result, MV.BARE)
        self.assertEqual(claims["C1"]["outcome"], MV.FAILS)
        self.assertEqual(claims["C1"]["computed_order"], ["no-port/reference", "no-port/breeder"])

    def test_gap_must_exceed_two_percent(self):
        result = synthetic(computed_life={"no-port/reference": 9.0, "no-port/breeder": 10.0, "port/reference": 9.85,
                                          "port/breeder": 10.0})
        claims = MV.evaluate_claims(result, MV.BARE)
        self.assertEqual(claims["C1"]["outcome"], MV.HOLDS)
        self.assertEqual(claims["C2"]["outcome"], MV.FAILS)
        self.assertAlmostEqual(claims["C2"]["computed_gap_fraction"], 0.015)
        self.assertIn("not above", claims["C2"]["reason"])

    def test_c2_records_replacement_counts(self):
        result = synthetic(fixed_counts={**{n: 30 for n in NAMES}, "port/reference": 32, "port/breeder": 39},
                           computed_counts={**{n: 31 for n in NAMES}, "port/reference": 25, "port/breeder": 26})
        counts = MV.evaluate_claims(result, MV.BARE)["C2"]["replacement_counts"]
        self.assertEqual(counts, {"port/reference": {"fixed": 32, "computed": 25}, "port/breeder": {"fixed": 39, "computed": 26}})
        self.assertNotIn("replacement_counts", MV.evaluate_claims(result, MV.BARE)["C1"])

    def test_not_evaluated_case_names_the_case_and_reason(self):
        result = synthetic(statuses={"port/breeder": "a curve stays above q* for 365 days"})
        claims = MV.evaluate_claims(result, MV.BARE)
        self.assertEqual(claims["C2"]["outcome"], MV.NOT_EVALUATED)
        self.assertIn("port/breeder", claims["C2"]["reason"])
        self.assertIn("365 days", claims["C2"]["reason"])
        self.assertEqual(claims["C1"]["outcome"], MV.HOLDS)

    def test_missing_computed_variant_is_not_evaluated(self):
        claims = MV.evaluate_claims(synthetic(), MV.WITH_IMPURITIES)
        self.assertTrue(all(c["outcome"] == MV.NOT_EVALUATED and "impurities" in c["reason"] for c in claims.values()))

    def test_impurity_variant_is_read_for_v3(self):
        self.assertEqual(outcomes(synthetic(variant=MV.WITH_IMPURITIES), MV.WITH_IMPURITIES), {c: MV.HOLDS for c in MV.CLAIMS})

    def test_c3_follows_d3_changed(self):
        self.assertEqual(outcomes(synthetic(d3=d3_doc(ratio=1.0, considered=True, changed=False)))["C3"], MV.FAILS)
        d3 = {"status": "NOT_EVALUATED", "reason": "a downtime is missing", "contrasts": []}
        claims = MV.evaluate_claims(synthetic(d3=d3), MV.BARE)
        self.assertEqual(claims["C3"]["outcome"], MV.NOT_EVALUATED)
        self.assertIn("a downtime is missing", claims["C3"]["reason"])

    def test_c4_ratio_threshold(self):
        self.assertEqual(outcomes(synthetic(d3=d3_doc(ratio=2.0)))["C4"], MV.HOLDS)
        low = MV.evaluate_claims(synthetic(d3=d3_doc(ratio=1.9)), MV.BARE)["C4"]
        self.assertEqual(low["outcome"], MV.FAILS)
        self.assertEqual(low["ratio_computed_over_fixed"], 1.9)

    def test_c4_not_considered_or_unevaluated(self):
        claim = MV.evaluate_claims(synthetic(d3=d3_doc(considered=False, changed=False)), MV.BARE)["C4"]
        self.assertEqual(claim["outcome"], MV.NOT_EVALUATED)
        self.assertIn("below the D3 minimum", claim["reason"])
        d3 = d3_doc()
        d3["contrasts"][0] = {"contrast": MV.C4_CONTRAST, "status": "NOT_EVALUATED"}
        claim = MV.evaluate_claims(synthetic(d3=d3), MV.BARE)["C4"]
        self.assertEqual(claim["outcome"], MV.NOT_EVALUATED)
        self.assertIn("missing", claim["reason"])


class ReproductionTests(unittest.TestCase):
    def test_identical_result_reproduces(self):
        ref = synthetic()
        self.assertEqual(MV.reproduce(copy.deepcopy(ref), ref), {"reproduced": True, "differences": []})

    def test_every_difference_is_listed(self):
        ref = synthetic()
        got = copy.deepcopy(ref)
        got["computed_model"][MV.BARE]["w"]["0.5"]["cases"]["port/reference"]["durations_s"]["blanket"][0] += 1e-9
        got["computed_model"][MV.BARE]["w"]["0.5"]["cases"]["no-port/breeder"]["history"]["lifetime_net_electricity_mwh"] += 1
        got["computed_model"][MV.BARE]["w"]["0.5"]["decisions"]["D3"]["changed"] = False
        got["fixed_model"]["port/breeder"]["history"]["replacements"].pop()
        out = MV.reproduce(got, ref)
        self.assertFalse(out["reproduced"])
        fields = {d["field"] for d in out["differences"]}
        self.assertEqual(fields, {"cases.port/reference.durations_s", "cases.no-port/breeder.history.lifetime_net_electricity_mwh",
                                  "decisions.D3", "fixed_model.port/breeder.history"})
        self.assertTrue(all({"reference", "got"} <= set(d) for d in out["differences"]))


class EvaluateTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name).resolve()
        self.addCleanup(self.tmp.cleanup)
        self.ref = synthetic()
        self.ref_path = self.root / "ref.json"
        self.ref_path.write_text(json.dumps(self.ref))
        self.dir = self.root / "val"

    def put(self, variant, result, info=None):
        (self.dir / variant).mkdir(parents=True, exist_ok=True)
        (self.dir / variant / "result.json").write_text(json.dumps(result))
        if info is not None:
            (self.dir / variant / "variant.json").write_text(json.dumps(info))

    def test_missing_variants_are_not_run(self):
        self.put("B", self.ref)
        out = MV.evaluate(self.dir, self.ref_path)
        self.assertTrue(out["baseline"]["reproduced"])
        self.assertEqual(out["variants"]["B"]["claims"]["C1"]["outcome"], MV.HOLDS)
        self.assertEqual(out["variants"]["V1"]["claims"]["C1"], {"outcome": MV.NOT_EVALUATED, "reason": "not run"})
        self.assertEqual(out["summary"]["C1"]["label"], "INCOMPLETE")
        self.assertEqual(out["summary"]["C1"]["holds_in"], ["B"])
        self.assertEqual(out["schema"], MV.SCHEMA)

    def test_failed_reproduction_blocks_the_others(self):
        bad = synthetic(computed_life={"no-port/reference": 9.0, "no-port/breeder": 10.5, "port/reference": 7.0,
                                       "port/breeder": 8.0})
        self.put("B", bad)
        self.put("V1", synthetic())
        out = MV.evaluate(self.dir, self.ref_path)
        self.assertFalse(out["baseline"]["reproduced"])
        for variant in ("B", "V1"):
            for claim in out["variants"][variant]["claims"].values():
                self.assertEqual(claim["outcome"], MV.NOT_EVALUATED)
                self.assertEqual(claim["reason"], "the baseline did not reproduce the amended run")

    def test_summary_labels(self):
        self.put("B", self.ref)
        failing = synthetic(fixed_life={"no-port/reference": 9.0, "no-port/breeder": 10.0, "port/reference": 8.0,
                                        "port/breeder": 7.0})
        self.put("V1", failing)
        for variant in MV.VARIANTS[2:]:
            self.put(variant, synthetic(variant=MV.WITH_IMPURITIES if variant == "V3" else MV.BARE))
        out = MV.evaluate(self.dir, self.ref_path)
        self.assertEqual(out["summary"]["C1"]["label"], "FRAGILE")
        self.assertEqual(out["summary"]["C1"]["fails_in"], ["V1"])
        self.assertEqual(out["summary"]["C2"]["label"], "ROBUST")
        self.assertEqual(out["summary"]["C3"]["label"], "ROBUST")
        self.assertEqual(set(out["summary"]), set(MV.CLAIMS))
        self.assertNotIn("verdict", out)
        # drop one variant: the robust claim becomes incomplete, the fragile one stays fragile
        (self.dir / "V5d" / "result.json").unlink()
        out = MV.evaluate(self.dir, self.ref_path)
        self.assertEqual(out["summary"]["C2"]["label"], "INCOMPLETE")
        self.assertEqual(out["summary"]["C2"]["not_evaluated_in"], ["V5d"])
        self.assertEqual(out["summary"]["C1"]["label"], "FRAGILE")

    def test_paths_are_replaced_and_output_is_sorted(self):
        info = {"variant": "B", "changes": [f"x {self.dir}/B/assumptions.json /home/connoravila/.local/bin/actinv "
                                            "/home/connoravila/Documents/actinv/actinv-data/a /home/connoravila/nuclear-data/b"]}
        self.put("B", self.ref, info)
        out = MV.evaluate(self.dir, self.ref_path)
        text = json.dumps(out)
        self.assertNotIn(str(self.dir), text)
        self.assertNotIn("/home/connoravila", text)
        self.assertEqual(out["variants"]["B"]["variant"]["changes"][0],
                         "x <validation-dir>/B/assumptions.json <actinv> <actinv-data>/a <nuclear-data>/b")
        target = self.root / "o.json"
        self.assertEqual(MV.main(["evaluate", "--dir", str(self.dir), "--reference", str(self.ref_path), "--out", str(target)]), 0)
        written = target.read_text()
        self.assertEqual(written, json.dumps(json.loads(written), indent=2, sort_keys=True) + "\n")

    def test_protocol_hash_mismatch_is_refused(self):
        altered = self.root / "protocol.md"
        data = (REPO / "docs" / "notes" / "MAINTENANCE_COUPLING_VALIDATION.md").read_bytes()
        altered.write_bytes(data.replace(b"Recorded 2026-10-06", b"Recorded 2026-10-07", 1))
        with self.assertRaises(MV.Refused):
            MV.evaluate(self.dir, self.ref_path, altered)
        # the hash covers the whole body, including the variants and the verdict rules
        rules = self.root / "rules.md"
        rules.write_bytes(data.replace(b"ROBUST", b"STRONG", 1))
        self.assertGreater(data.find(b"ROBUST"), 299)
        with self.assertRaises(MV.Refused):
            MV.evaluate(self.dir, self.ref_path, rules)
        # an amendment after the heading does not change the body hash
        amended = self.root / "amended.md"
        amended.write_bytes(data + b"\n### Amendment 1\nlater.\n")
        self.put("B", self.ref)
        self.assertTrue(MV.evaluate(self.dir, self.ref_path, amended)["baseline"]["reproduced"])
        self.assertEqual(MV.main(["evaluate", "--dir", str(self.dir), "--reference", str(self.ref_path),
                                  "--out", str(self.root / "x.json"), "--protocol", str(altered)]), 2)

    def test_recorded_protocol_has_the_recorded_hash(self):
        self.assertEqual(MV.protocol_body_sha256(MV.DEFAULT_PROTOCOL), MV.PROTOCOL_BODY_SHA256)


@unittest.skipUnless(REFERENCE.is_file(), "the amended run's record is not present")
class RealReferenceTests(unittest.TestCase):
    def test_amended_run_against_itself(self):
        with tempfile.TemporaryDirectory() as tmp:
            (Path(tmp) / "B").mkdir()
            (Path(tmp) / "B" / "result.json").write_bytes(REFERENCE.read_bytes())
            out = MV.evaluate(Path(tmp), REFERENCE)
        self.assertTrue(out["baseline"]["reproduced"])
        self.assertEqual({c: v["outcome"] for c, v in out["variants"]["B"]["claims"].items()}, {c: MV.HOLDS for c in MV.CLAIMS})
        self.assertAlmostEqual(out["variants"]["B"]["claims"]["C1"]["computed_gap_fraction"], 0.05611507764299334)
        self.assertEqual(out["variants"]["B"]["claims"]["C2"]["replacement_counts"]["port/reference"]["computed"], 32)


if __name__ == "__main__":
    unittest.main()
