#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""CAPI negative controls for the NUC-013 tagged tally estimator guard.

Each case runs in a separate process, initializes the model and tally storage,
and checks the C API estimator setter. No transport batch is advanced.
"""
from __future__ import annotations
import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import xml.etree.ElementTree as ET

TARGET = "response-heating-total-whole-model"
FILES = ("geometry.xml", "materials.xml", "settings.xml", "tallies.xml")
GUARD = "NUC-013"
CE_ERROR = "requires continuous-energy tallies"
ESTIMATOR_ERROR = "supports only analog or collision estimators"
MG_UPSTREAM_ERROR = "Cannot tally heating reaction rate in multi-group mode"


def rejection_origin(mode: str, message: str) -> str | None:
    if mode == "mg" and MG_UPSTREAM_ERROR in message:
        return "openmc-multigroup-score-validation"
    expected = CE_ERROR if mode == "mg" else ESTIMATOR_ERROR
    if GUARD in message and expected in message:
        return "nuc013-accumulator-guard"
    return None


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def prepare(prefix: Path, src: Path, dst: Path, mode: str) -> None:
    for name in FILES:
        shutil.copy2(src / name, dst / name)
    tree = ET.parse(dst / "tallies.xml")
    matches = [t for t in tree.getroot().findall("tally") if t.get("name") == TARGET]
    if len(matches) != 1:
        raise RuntimeError(f"expected exactly one {TARGET}, got {len(matches)}")
    tally = matches[0]
    # The controls need one named scalar tally only. This removes unrelated
    # particle-filter tallies from the multi-group parsing path.
    for child in list(tree.getroot()):
        if child.tag == "tally" and child is not tally:
            tree.getroot().remove(child)
        elif child.tag == "filter":
            tree.getroot().remove(child)
    if mode == "xml-tracklength":
        estimator = tally.find("estimator")
        if estimator is None:
            estimator = ET.SubElement(tally, "estimator")
        estimator.text = "tracklength"
    if mode == "non-target":
        tally.set("name", "nuc013-neutral-control")
    tree.write(dst / "tallies.xml", encoding="utf-8", xml_declaration=True)
    if mode == "mg":
        # Build a valid synthetic 1-group macro library; no evaluated nuclear
        # data or transport calculations are involved.
        generator = dst / "make_mgxs.py"
        generator.write_text("\n".join([
            "import numpy as np", "import openmc",
            "groups = openmc.mgxs.EnergyGroups(group_edges=[0.0, 20.0e6])",
            "data = openmc.XSdata('NUC013_MACRO', groups)", "data.order = 0",
            "data.set_total([1.0])", "data.set_absorption([0.5])",
            "data.set_scatter_matrix(np.zeros((1, 1, 1)))",
            "lib = openmc.MGXSLibrary(groups)", "lib.add_xsdata(data)",
            "lib.export_to_hdf5('mgxs.h5')", "" ]), encoding="utf-8")
        subprocess.run([str(prefix / "bin/python"), str(generator)], cwd=dst,
                       check=True, timeout=30)
        mats_path = dst / "materials.xml"
        mats = ET.parse(mats_path)
        for material in mats.getroot().findall("material"):
            for child in list(material):
                if child.tag in ("nuclide", "element", "macroscopic", "density", "sab"):
                    material.remove(child)
            ET.SubElement(material, "density", {"value": "1.0", "units": "macro"})
            ET.SubElement(material, "macroscopic", {"name": "NUC013_MACRO"})
        mats.write(mats_path, encoding="utf-8", xml_declaration=True)
    if mode in ("xml-tracklength", "mg"):
        settings = ET.parse(dst / "settings.xml")
        root = settings.getroot()
        if mode == "mg":
            ET.SubElement(root, "energy_mode").text = "multi-group"
        photon = root.find("photon_transport")
        if photon is None:
            photon = ET.SubElement(root, "photon_transport")
        photon.text = "false"
        settings.write(dst / "settings.xml", encoding="utf-8", xml_declaration=True)


def error_message(dll) -> str:
    return (ctypes.c_char * 256).in_dll(dll, "openmc_err_msg").value.decode(errors="replace")


def run_case_inner(prefix: Path, source: Path, xs: Path, out: Path, mode: str) -> dict:
    work = out / mode
    work.mkdir()
    prepare(prefix, source, work, mode)
    library = prefix / "lib/libopenmc.so"
    executable = prefix / "bin/openmc"
    dll = ctypes.CDLL(str(library))
    dll.openmc_init.argtypes = [ctypes.c_int, ctypes.POINTER(ctypes.c_char_p), ctypes.c_void_p]
    dll.openmc_init.restype = ctypes.c_int
    dll.openmc_simulation_init.argtypes = []
    dll.openmc_simulation_init.restype = ctypes.c_int
    dll.openmc_simulation_finalize.argtypes = []
    dll.openmc_simulation_finalize.restype = ctypes.c_int
    dll.openmc_finalize.argtypes = []
    dll.openmc_finalize.restype = ctypes.c_int
    dll.openmc_tally_set_estimator.argtypes = [ctypes.c_int32, ctypes.c_char_p]
    dll.openmc_tally_set_estimator.restype = ctypes.c_int
    dll.openmc_tally_get_estimator.argtypes = [ctypes.c_int32, ctypes.POINTER(ctypes.c_int)]
    dll.openmc_tally_get_estimator.restype = ctypes.c_int
    argv = (ctypes.c_char_p * 2)(str(executable).encode(), None)
    old = Path.cwd()
    if mode == "mg":
        os.environ["OPENMC_MG_CROSS_SECTIONS"] = str(work / "mgxs.h5")
    else:
        os.environ.pop("OPENMC_MG_CROSS_SECTIONS", None)
    os.chdir(work)
    try:
        code = dll.openmc_init(1, argv, None)
        sim_code = dll.openmc_simulation_init() if code == 0 else None
        if mode == "xml-tracklength" or mode == "mg":
            msg = error_message(dll)
            # OpenMC rejects MG heating while parsing scores, before the
            # accumulator's defensive CE check can run.
            origin = rejection_origin(mode, msg)
            output_files = sorted(p.name for p in work.iterdir()
                                  if p.name.startswith("statepoint.")
                                  or p.name == "tallies.out")
            rejected = code != 0 or sim_code not in (None, 0)
            passed = rejected and origin is not None and not output_files
            return {"case": mode, "init_code": code, "simulation_init_code": sim_code,
                    "message": msg, "passed": passed, "transport_started": False,
                    "rejection_origin": origin,
                    "output_files": output_files}
        if code != 0:
            msg = error_message(dll)
            return {"case": mode, "init_code": code, "message": msg, "passed": False,
                    "simulation_init_completed": False, "transport_started": False}
        if sim_code != 0:
            msg = error_message(dll)
            return {"case": mode, "init_code": code, "simulation_init_code": sim_code,
                    "message": msg, "passed": False,
                    "simulation_init_completed": False, "transport_started": False}
        # Query the XML's explicit tally ID through the C API index lookup.
        xml_root = ET.parse(work / "tallies.xml").getroot()
        wanted = TARGET if mode == "late-setter" else "nuc013-neutral-control"
        target = next(t for t in xml_root.findall("tally") if t.get("name") == wanted)
        target_id = int(target.get("id"))
        find_index = dll.openmc_get_tally_index
        find_index.argtypes = [ctypes.c_int32, ctypes.POINTER(ctypes.c_int32)]
        find_index.restype = ctypes.c_int
        idx = ctypes.c_int32()
        find_code = find_index(target_id, ctypes.byref(idx))
        if find_code != 0:
            raise RuntimeError(f"openmc_get_tally_index failed: {error_message(dll)}")
        before = ctypes.c_int()
        get_before_code = dll.openmc_tally_get_estimator(idx.value, ctypes.byref(before))
        if get_before_code != 0:
            raise RuntimeError(f"initial estimator getter failed: {error_message(dll)}")
        set_code = dll.openmc_tally_set_estimator(idx.value, b"tracklength")
        setter_message = error_message(dll)
        after = ctypes.c_int()
        get_after_code = dll.openmc_tally_get_estimator(idx.value, ctypes.byref(after))
        if get_after_code != 0:
            raise RuntimeError(f"post-setter estimator getter failed: {error_message(dll)}")
        if mode == "late-setter":
            passed = (set_code != 0 and GUARD in setter_message and
                      ESTIMATOR_ERROR in setter_message and
                      before.value == 2 and after.value == before.value)
        else:
            passed = set_code == 0 and before.value == 2 and after.value == 1
        return {"case": mode, "init_code": code, "index_code": find_code,
                "get_before_code": get_before_code, "setter_code": set_code,
                "get_after_code": get_after_code,
                "setter_message": setter_message,
                "before_estimator_enum": before.value, "after_estimator_enum": after.value,
                "passed": passed, "simulation_init_code": sim_code,
                "simulation_init_completed": True, "transport_started": False}
    finally:
        # Release initialized storage without advancing a batch.
        if 'code' in locals() and code == 0:
            if 'sim_code' in locals() and sim_code == 0:
                dll.openmc_simulation_finalize()
            dll.openmc_finalize()
        os.chdir(old)



def run_case(prefix: Path, source: Path, xs: Path, out: Path, mode: str) -> dict:
    # Isolate each C API initialization in its own process so a rejected init
    # cannot leave global OpenMC state for the next control.
    cmd = [str(prefix / "bin/python"), str(Path(__file__).resolve()),
           "--worker", "--prefix", str(prefix), "--template", str(source),
           "--cross-sections", str(xs), "--out", str(out), "--mode", mode]
    environment = {"PATH": f"{prefix / 'bin'}:/usr/bin:/bin",
                   "OMP_NUM_THREADS": "1", "OPENBLAS_NUM_THREADS": "1",
                   "PYTHONDONTWRITEBYTECODE": "1",
                   "OPENMC_CROSS_SECTIONS": str(xs)}
    try:
        proc = subprocess.run(cmd, env=environment, check=False, text=True,
                              capture_output=True, timeout=90)
    except subprocess.TimeoutExpired:
        return {"case": mode, "passed": False, "reason": "initialization timed out"}
    (out / f"{mode}.stdout.log").write_text(proc.stdout)
    (out / f"{mode}.stderr.log").write_text(proc.stderr)
    if proc.returncode != 0:
        message = proc.stderr + "\n" + proc.stdout
        origin = rejection_origin(mode, message)
        outputs = sorted(p.name for p in (out / mode).iterdir()
                         if p.name.startswith("statepoint.")
                         or p.name == "tallies.out")
        # Native fatal_error currently aborts across the C API boundary. Keep
        # the subprocess result, and only pass on the guard's exact message.
        return {"case": mode, "process_returncode": proc.returncode,
                "message": message[-4000:], "passed": (mode in ("xml-tracklength", "mg")
                and origin is not None and not outputs),
                "rejection_origin": origin, "transport_started": False,
                "output_files": outputs}
    return json.loads((out / mode / "control-result.json").read_text())


def worker_main(argv: list[str]) -> int:
    p = argparse.ArgumentParser()
    p.add_argument("--worker", action="store_true")
    p.add_argument("--prefix", type=Path, required=True)
    p.add_argument("--template", type=Path, required=True)
    p.add_argument("--cross-sections", type=Path, required=True)
    p.add_argument("--out", type=Path, required=True)
    p.add_argument("--mode", required=True)
    a = p.parse_args(argv)
    prefix = a.prefix.resolve(strict=True)
    out = a.out.absolute()
    out.mkdir(parents=True, exist_ok=True)
    os.environ["OPENMC_CROSS_SECTIONS"] = str(a.cross_sections.resolve(strict=True))
    os.environ["OMP_NUM_THREADS"] = "1"
    record = run_case_inner(prefix, a.template.resolve(strict=True),
                            a.cross_sections.resolve(strict=True), out, a.mode)
    record["library_path"] = str(prefix / "lib/libopenmc.so")
    record["library_sha256"] = sha256(prefix / "lib/libopenmc.so")
    (out / a.mode / "control-result.json").write_text(json.dumps(record, indent=2) + "\n")
    return 0

def main() -> int:
    if "--worker" in sys.argv:
        return worker_main(sys.argv[1:])
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--prefix", type=Path, required=True,
                   help="candidate OpenMC CAPI prefix")
    p.add_argument("--template", type=Path, required=True)
    p.add_argument("--cross-sections", type=Path, required=True)
    p.add_argument("--out", type=Path, required=True)
    a = p.parse_args()
    prefix = a.prefix.resolve(strict=True)
    template = a.template.resolve(strict=True)
    xs = a.cross_sections.resolve(strict=True)
    if not (prefix / "bin/openmc").is_file() or not (prefix / "lib/libopenmc.so").is_file():
        raise RuntimeError("prefix must contain bin/openmc and lib/libopenmc.so")
    out = a.out.absolute()
    out.mkdir(parents=True, exist_ok=False)
    library_hash = sha256(prefix / "lib/libopenmc.so")
    records = []
    for mode in ("xml-tracklength", "mg", "late-setter", "non-target"):
        records.append(run_case(prefix, template, xs, out, mode))
    if sha256(prefix / "lib/libopenmc.so") != library_hash:
        raise RuntimeError("selected library changed during the control")
    report = {"schema": "faris-nuc013-tally-guard-control/v1",
              "prefix": str(prefix), "library_sha256": library_hash,
              "template": str(template), "records": records,
              "pass": all(r.get("passed", False) for r in records)}
    (out / "negative-control.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))
    return 0 if report["pass"] else 1

if __name__ == "__main__":
    raise SystemExit(main())
