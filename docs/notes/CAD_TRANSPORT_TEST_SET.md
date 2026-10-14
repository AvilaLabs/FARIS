<!-- kind: explanation -->
# CAD transport on the four-model test set (2026-10-13/14)

What happened when four machines other than RM-M went through `faris design check` and
`faris design transport` with default settings, untuned. The models are built by
`integrations/openmc/risk_tests/build_test_set.py` (STEP, design file, operating history,
CAD self-check); runs live in `~/.cache/avila-night/test-set/<name>/`, outside git.

Machine: laptop, 4 cores (`taskset -c 4-7`), 8 GB cap for the check, 10 GB for transport.
Binaries: `design check` at 47d6ae1; transport at 47d6ae1 (no-port) and 78c66ff (bent-duct,
large). These designs predate the source-closure finding (06d6d86): their profiles imply less
than the declared fusion power (builder before that commit), so a check on current main would
block at check 9 until the designs are regenerated. Transport strength comes from the declared
power, so the numbers below are unaffected.

## Design check

| Model | Solids | Result | Wall time |
|---|---|---|---|
| st-compact (spherical tokamak, 200 MW) | 24 | all 9 PASS | 401 s |
| no-port (RM-M without a port duct) | 29 | all 9 PASS | 461 s |
| bent-duct (RM-M, dog-leg port duct) | 30 | all 9 PASS | 420 s |
| large (RM-M × 1.8, 2000 MW) | 30 | all 9 PASS | 469 s |

## Transport (TF-coil fast flux above 0.1 MeV, target 5 % on the mean, 10 % on the peak piece)

| Model | Mode chosen | Preparation | Sampling | Coil mean | Peak piece | Lost |
|---|---|---|---|---|---|---|
| st-compact | aimed | 653 s | 47 s, 0.6 M histories | 9.506e15 ± 1.5 % | inboard leg, r 0.30–0.40 m | 0 |
| no-port | windows-only (nothing in line of sight) | 318 s | 438 s, 12.5 M | 4.592e12 ± 0.8 % | 1.900e14 ± 15.2 % (see below), inboard leg r 1.25–1.30 m | 0 |
| bent-duct | aimed | 440 s | 29 s, 0.64 M | 6.550e12 ± 4.0 % | 2.976e15 ± 9.6 %, coil 00 at r 5.80–5.85 m, midplane | 0 |
| large | see addendum | | | | | |

Units: n/m²/s. All four reach the target; all four coil groups EXCEED the 3e22 n/m² fluence
limit of the test history (these are test machines, not designs).

## Findings

1. **No-port: aiming switched itself off, as designed.** The uncollided map found no part of
   the coils in sight of the source, so the run used the model's windows alone.

2. **No-port: the printed peak came from an old selection rule.** The binary was built before
   67340b7, which picks the leading piece across solids by flux less two standard errors.
   Recomputed from the run's `result.json` with the current rule: tf-coil-10, 9.46e-11 per
   source neutron per cm³ ± 9.7 %, not tf-coil-06 ± 15.2 %. Current main prints the former.

3. **No-port: the peak of a symmetric machine is biased high by selection.** All 18 coils have
   the same true peak (the machine has no port). Their leading pieces range 7.1e-11 to
   1.02e-10 per source neutron per cm³ (mean 8.44e-11, each ± 8–15 %), and the reported peak
   is the largest of 18 noisy estimates: 12 % above their average with the current rule
   (9.46e-11), 21 % with the old one. Each solid's leader is itself the largest of its
   pieces, so the average is not the true peak either. The bias is
   toward a shorter life (conservative) and lies inside the reported 2-sigma band. Not fixed;
   pooling symmetric copies would remove it, and FARIS has no symmetry declaration today.

4. **Bent duct: "straight channel" was false; the answer is right.** The builder's own
   ray check finds 0 of 20,016 void lines from plasma to coil through the duct (1,494 of
   20,016 with the straight-duct control). The aiming step decides from uncollided flux,
   which also crosses thin material, so it found the duct's last opening (face, exit and
   inlet at r 5.55–5.70 m) and aimed there; the old message called that a straight channel.
   An independent run on stock OpenMC (windows only, no aiming, seed 22) gives the same
   answer:

   | Run | Histories | Coil 00 mean (per source neutron) | Peak piece (per cm³) | All coils |
   |---|---|---|---|---|
   | aimed, seed 21 | 0.64 M | 2.568e-4 ± 8.9 % | 1.5985e-9 ± 9.6 % | 7.300e-4 ± 4.0 % |
   | stock, seed 22 | 2.44 M | 2.645e-4 ± 9.0 % | 1.5975e-9 ± 9.9 % | 7.676e-4 ± 3.5 % |

   z: peak 0.00, coil 00 −0.24, all coils −0.95. The aimed run is unbiased here too. The
   peak is high because the dog-leg's last leg opens 5 cm in front of coil 00 (the straight
   RM-M port passes between coils). Wording changed in this commit: the reason now says
   neutrons reach the response through a duct without a collision, through the void or thin
   material.

5. **Bent duct: aiming cost more than it saved.** Aimed: 440 s preparation (230 s of it the
   target search) + 29 s sampling = 469 s. Stock: 194 s preparation + 81 s sampling = 275 s.
   The search pays off only when sampling without it is long. Open: decide to aim from an
   estimate of the windows-only sampling time (for example a short windows-only pilot).

## Addendum: the large model (2026-10-14)

Densities closed first (factor 1.34154; 1111.27 MW implied before, 2000 MW declared). The recheck took 124 s
because it reused the earlier work.

Transport took four attempts. Each of the first three stopped on the job runner's disk limits, and each was fixed
in turn:

| Run | Binary | Stopped by | Fix |
|---|---|---|---|
| 1 | 78c66ff | EFBIG: 129 MB of windows written as text into `model.xml`, over the 256 MiB single-file limit | windows go to an HDF5 file named by relative path (9a93ef6) |
| 2 | 0295b75 | run folder over 512 MB: geometry, windows, a copy of the windows, and a 7.4 M-cell aiming map (194 MB) | no copy; map at most 2 M cells; the cap counts growth during the job, not what preparation left (9a93ef6) |
| 3 | a4f1eaa | run step over 512 MB: each of five aiming targets' visibility runs left about 90 MB | each run's model and statepoint removed once read (43db3b8) |
| 4 | 43db3b8 | no limit; cancelled by the 3400 s `timeout` wrapped around the command for this test | none needed |

Run 4 used 365 MB at most. Preparation and aiming took about 8 minutes. The plan chose an aimed run with four
targets. Sampling ran for 2.54 M histories in 2914 s.

**Finding: one heavy history in the aimed run.** Over that time the error on the coil mean fell steadily to 5.5 %
at 1.36 M histories. By 1.80 M it was 27.9 %, and it was still 20.5 % at 2.54 M. A single history or a few
histories carried a weight that dominated the sum. The highest piece stayed between 13 and 17 %.

The stop rule ends a run on the first batch at or below its target. It would have stopped this run correctly
had 5 % been reached before the event. Had the event come later, it would have stopped with a mean that is
biased low and an error that is too small.

The stop-rule test on RM-M showed no such bias (−2.2 % ± 1.6 %). This large machine with an aimed source is a
different case.

Open, in order:
1. Find the history and where it scored. That needs a rerun that keeps the per-batch tallies.
2. Decide whether the aimed windows on this geometry need a lower weight ceiling.
3. Make the stop rule refuse to stop while the batch series shows a recent jump. Candidates are the variance of
   the variance, or the largest single batch's share of the sum, the checks MCNP's ten statistical tests make.

No answer for this model is reported.

## Addendum 2026-10-14: FARIS-010 closed

The large model's heavy batch had two causes. First, the aiming patch let a particle headed at the duct skip the
windows inside a coil; the patch now skips them only in void. Second, with that fixed the same batch was still 18 times
the median: one source neutron struck the first wall 15 cm outside the port mouth, where the coarse windows are far too
high, survived roulette 210 times heavier and reached coils 00 and 17 split into 8,012 copies (every track of batch 73
recorded). Design transport now caps a roulette survivor at 5 times the weight it replaces, the MCNP limit for one
split or roulette.

| Run (seed 21 unless stated) | Histories | Mean error | Heaviest batch / median | VOV | Summed coil flux |
| --- | --- | --- | --- | --- | --- |
| Large, first fix only | 2.04 M | 13.9 % (jump at 1.4 M) | 18 | 0.80 | 3.20e15 ± 10 % |
| Large, first fix and cap | 2.76 M | 4.3 %, no jump | 3.4 | 0.027 | 3.00e15 ± 3.3 % |
| Bent duct, cap, seeds 21 and 22, against stock | 0.68 M, 0.60 M | 2.9 %, 2.9 % | | 0.098, 0.052 | pooled z = −1.45 |

The large run still ended at its 1 h limit: the peak piece was at 11.7 % against a 10 % target. Finer windows at the
port mouth (NUC-079) are the next step there.
