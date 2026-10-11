#!/usr/bin/env python3
"""Reads an OpenMC statepoint with h5py only and writes every tally bin as JSON.

    summarize_statepoint.py STATEPOINT.h5 OUT.json

mean = sum / n, standard error = sqrt((sum_sq / n - mean^2) / (n - 1)), the same
definition OpenMC uses for tally results.
"""
import json
import sys

import h5py
import numpy as np


def summarize(path):
    out = {"file": str(path), "tallies": {}}
    with h5py.File(path, "r") as f:
        out["version"] = ".".join(str(v) for v in f.attrs["openmc_version"])
        out["n_batches"] = int(f["current_batch"][()])
        out["seed"] = int(f["seed"][()])
        out["n_particles"] = int(f["n_particles"][()])
        for key in sorted(f["tallies"].keys()):
            if not key.startswith("tally "):
                continue
            g = f["tallies"][key]
            n = int(g["n_realizations"][()])
            res = g["results"][()]  # (filter bins, nuclide x score, 2)
            scores = [s.decode() for s in g["score_bins"][()]]
            mean = res[..., 0] / n
            var = (res[..., 1] / n - mean**2) / max(n - 1, 1)
            std = np.sqrt(np.clip(var, 0.0, None))
            out["tallies"][key] = {
                "scores": scores,
                "n_realizations": n,
                "shape": list(mean.shape),
                "mean": mean.reshape(mean.shape[0], -1).tolist(),
                "std": std.reshape(std.shape[0], -1).tolist(),
            }
    return out


if __name__ == "__main__":
    json.dump(summarize(sys.argv[1]), open(sys.argv[2], "w"), indent=1)
