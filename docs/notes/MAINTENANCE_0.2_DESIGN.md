# Computed maintenance durations in FARIS 0.2: design

Recorded 2026-10-08. Basis: the maintenance coupling test (`docs/notes/MAINTENANCE_COUPLING_RESULT.md`) and its
validation (`docs/notes/MAINTENANCE_COUPLING_VALIDATION_RESULT.md`).

## What the feature has to show

The finding that held in every evaluated validation variant: replacement outages computed from the activation
cooldown of the components around a replaced part differ between designs, and grow over a plant's life, by factors a
fixed-duration model cannot show (the no-port blanket contrast was 2 to 14 times its fixed value). Whether that
changes which design wins depends on outage length, so the feature reports downtime, availability and lifetime
electricity per design under both models and makes no ranking claim of its own.

For each design and each replacement over the plant's life, FARIS 0.2 shows:

1. the computed outage (cooldown plus work) beside the fixed one;
2. why the cooldown is what it is: each governing component's share of the decay heat at the end of the cooldown and
   how long that component had been in the machine;
3. per design, total replacement downtime, availability and lifetime net electricity, fixed and computed;
4. between designs, the downtime difference under each model and their ratio.

## Decisions

1. **Rust owns the coupling loop.** A new `faris_engine::maintenance` module runs history, collects decay curves,
   computes cooldowns and iterates to convergence. The Python driver `scripts/maintenance_coupling_test.py` stays as
   the frozen test harness and is the reference the Rust port is checked against. Reason: `AGENTS.md` keeps the
   authoritative model and time loop in Rust.
2. **Decay curves come through a trait.** The loop asks a `DecaySource` for, per replacement event and governing
   component, the decay curve of the installation in place at the event start. The first implementation runs
   `scripts/build_activation_inputs.py` (FARIS's documented ACTINV spec preparer) and `actinv run` through the
   bounded job boundary, with the content-addressed points cache shared with the driver
   (`faris-mct-actinv-points/v0.1`). Tests use an analytic source. Reason: the expensive part stays replaceable
   (restart method, `actinv mesh` batching) without touching the loop.
3. **Decay heat governs; dose does not yet.** Heat-governed durations held up in nine validation variants. The
   dose-governed variant is NOT EVALUATED: with a fixed threshold the never-replaced first wall keeps contact dose
   above it past a year at later events. The result records the governing quantity; asking for dose is refused with
   that reason and the next step (a remote-handling dose rule needs its own test).
4. **The rules are the tested ones.** Cooling grid 40 log-spaced times from 1 hour to 365 days; per event the governing
   set's curve is the sum of the components' heat divided by their total volume; interpolation linear in ln t and ln
   value; cooldown is the first crossing of q*; no crossing within 365 days is NOT EVALUATED; a crossing cut short by
   the next restart is window-limited. Iteration until no duration changes by more than 1 day, at most 10 iterations;
   no convergence is NOT EVALUATED with the last change.
5. **Threshold per class, two ways.** Either an explicit q* (W/m³), or calibration: the cooldown of the class's first
   replacement in a named design at the fixed durations equals a stated target, as in the test. Work time per class is
   explicit seconds. Reason: calibration is what was tested; an explicit q* lets a user apply a published or
   plant-specific threshold.
6. **One assumptions file, any number of designs.** `faris-maintenance-assumptions/v0.1` names the classes (replaced
   component, governing components, work time, threshold), the cooling grid and iteration limits. A design is a
   scenario, physics file, history run, 709-group spectrum run and operating-history assumptions; its fixed durations
   are those assumptions' replacement durations.
7. **Output `faris-maintenance-result/v0.1`**: inputs with SHA-256, per design the fixed and computed history
   summaries, every iteration's durations and the per-event records in point 2 above, contrasts between designs, and
   every NOT EVALUATED with its reason and next step.
8. **Delivery in four slices**: (A) model types and the engine loop with an analytic source; (B) the ACTINV source and
   `faris maintenance run|report` in the CLI; (C) the desktop panel that opens a result and shows points 1 to 4;
   (D) running the job from the desktop with progress and cancellation. A parity control runs the Rust command on the
   test's four arrangements at w = 0.5 and must reproduce the recorded computed durations within 1e-6 relative before
   the feature is described as working.

## Out of scope for 0.2

Dose-governed durations; crews, spares and queues (OPS-043); the restart continuation method as default; any claim
that a design ranks higher because of computed maintenance.
