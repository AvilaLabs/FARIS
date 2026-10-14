<!-- kind: explanation -->
# S0g pilot: the inboard-leg response (NUC-050)

Date: 2026-10-13. A pilot, not a protocol: single runs, one seed, no frozen rule. It sizes the measurement that
NUC-050 now asks for (at least 275× against analogue for nuclear heating in RM-M's inboard toroidal-field leg) and
shows where the difficulty is.

## Set-up

- Model: `base-model.xml` (sha256 c0c107f1f79c06d6…), stock OpenMC 0.15.3, 4 threads.
- Response region: toroidal-field coil material inside the box |x|, |y| < 130 cm, |z| < 150 cm. That is the
  inboard leg near the midplane.
- "Windows" is the model's current windows (F0 of S0f). "Analog" is the same model with windows off.
- Pilot script and outputs: `~/.cache/avila-night/s0g-inboard/`.

## Results

| Response | Histories | Run | Relative error | Time (s) | FOM |
| --- | --- | --- | --- | --- | --- |
| Fast flux (> 0.1 MeV), neutrons only | 3 M | Analog | 0.446 | 114 | 0.044 |
| Fast flux (> 0.1 MeV), neutrons only | 3 M | Windows | 0.028 | 92 | 14.0 |
| Heating, with photon transport | 0.3 M | Analog | 0.455 | 79 | 0.061 |
| Heating, with photon transport | 0.3 M | Windows | 0.129 | 120 | 0.50 |

- **Fast flux: about 320× in this pilot.** The current windows already do well on the inboard leg. The analog
  figure rests on a 45 % relative error, so the ratio is an indication only.
- **Heating: about 8× in this pilot,** with the same caveat on the analog side.
  - Heating in the leg is mostly photon energy: of 38.3 eV per source neutron with windows, 33.9 is scored by
    electrons, 1.8 by positrons and 2.5 by neutrons.
  - Photon transport makes the windowed run about 13× slower per history (120 s for 0.3 M against 92 s for 3 M).
    The windows are neutron-only; photons are followed without any.
- **Size of a proper measurement.** An analog reference at 5 % needs about 80 times these histories: roughly
  2.5 h for fast flux, and longer for heating. That needs Connor's approval before it runs.

## What follows

- NUC-050's response is heating, so the fast-flux result does not meet it.
- The heating gap is a photon problem, not a neutron one. Candidates, none tested: photon weight windows from a
  coupled adjoint; scoring photon heating at the production site (a kerma approximation) with its error stated;
  a photon energy cut-off with local deposition.
- A literature pass comes before building any of these.
