# Performance

How fast FARIS starts, responds, renders, recalculates and computes. All interactive
targets are measured on the **reference laptop (RL)** unless stated; see the
[index](README.md#reference-hardware-and-models) for RL, RW and the reference models
RM-S, RM-M and RM-L. Percentiles are over at least 20 runs (startup) or 1,000 events
(interaction). Averages are never accepted as evidence: stutter lives in P99 and max.

## Startup, opening and saving

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| PERF-001 | FARIS shall show an interactive window on cold start (disk cache dropped). | First interactive frame P95 ≤ 1.0 s on RL. | Startup harness in CI on dedicated RL-class runner. | Zed markets sub-second start; VS Code/Blender typically 1–3 s [U, no primary cross-app benchmark] | F1 | Partial: first UI frame 0.23–2.3 s measured, cold P95 unmeasured |
| PERF-002 | FARIS shall start warm quickly. | First interactive frame P95 ≤ 0.5 s. | Same harness, warm cache. | As PERF-001 | F1 | Unmeasured |
| PERF-003 | Opening a study file shall be progressive. | UI usable ≤ 0.5 s after Open; RM-S study fully interactive ≤ 1.5 s; 1 GB study ≤ 3 s. | Open benchmark over fixture studies. | No published reference; FARIS choice | F1 | Partial: 6.1 MB study opens behind a modal; timing unmeasured |
| PERF-004 | Saving shall be incremental: unchanged blobs are not recompressed. | Save after a parameter edit ≤ 1.0 s for RM-S; first full save of 150 MB raw records ≤ 10 s. | Save benchmark; blob-reuse counter. | Measured 2026-10-01: full save 6.3 s at zstd 15 | F1 | Partial: full save 6.3 s, no incremental save |
| PERF-005 | Session restore shall reopen layout, camera, selection and open study. | ≤ 2 s on RL. | Restart test. | Blender saves and reloads workspace layout with the file [V](https://docs.blender.org/manual/en/latest/files/blend/open_save.html); 2 s is FARIS choice | F1 | No |

## Interaction and rendering

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| PERF-010 | Every direct input (click, key, hover, slider drag) shall produce visible feedback within one frame budget. | Input-to-present P50 ≤ 16.7 ms, P95 ≤ 50 ms, P99 ≤ 100 ms. | Instrumented input and present timestamps, 1,000-event script. | Nielsen 0.1 s "instant" [V]; INP "good" ≤ 200 ms [V] | F1 | Unmeasured |
| PERF-011 | The 3D viewport shall hold display rate while orbiting, panning and zooming RM-S. | Frame time P50 ≤ 8.3 ms, P99 ≤ 16.7 ms, no frame > 33 ms on RL. | Scripted camera path; wgpu timestamp queries; frame-time histogram. | 60 Hz = 16.7 ms [V] | F1 | Partial: P95 12.1 ms, mean 90 fps on RL (2026-09-30 profile); P99 and max unmeasured |
| PERF-012 | The viewport shall render large models interactively with level of detail. | RM-L geometry (≥ 5 M triangles) P99 ≤ 33 ms on RL and ≤ 16.7 ms on RW; ≥ 20 M triangles P99 ≤ 16.7 ms on RW. | Synthetic and RM-L scenes. | FARIS choice; no published single-GPU 1e7 limit found, to be settled by the F7 render benchmark; partly unverified [U] | F7 | No |
| PERF-013 | Drag-to-pixel latency for direct manipulation shall be at most two frames. | ≤ 33 ms at 60 Hz, ≤ 17 ms at 120 Hz. | Present-timestamp loop or high-speed camera. | Dragging latency: users detect about 2-11 ms (Ng 2012, via Jota 2013 [V](https://www.tactuallabs.com/papers/howFastIsFastEnoughCHI13.pdf)); throughput differs significantly between 10, 25 and 50 ms (Jota 2013); partly unverified [U] | F2 | Unmeasured |
| PERF-014 | The UI thread shall never block on computation or I/O. | No frame > 50 ms while any job runs; 0 occurrences in a 1-hour soak. | Frame log during soak with jobs. | RAIL: idle work chunks < 50 ms [V] | F1 | Partial: jobs run on workers; soak not run |
| PERF-015 | Idle FARIS shall not repaint or spin. | 0 repaints/s when nothing changes; ≤ 0.5 % of one core; ≤ 5 wakeups/s. | powertop / platform energy tools, scripted idle hour. | egui reactive repaint [V] | F1 | Unmeasured |
| PERF-016 | Selection, picking and linked highlighting shall be immediate. | Pick-to-highlight across 3D, tree, table and chart ≤ 100 ms on RM-L. | UI automation timing. | Nielsen 0.1 s [V] | F2 | Partial: picking works on RM-S; untimed |

## Recalculation and feedback on long work

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| PERF-020 | What-if edits that reuse recorded transport shall recalculate every displayed plant-life history at "instant" speed. | 30-year histories for 4 arrangements: P95 ≤ 100 ms on RL. Full coupled plant life (fuel cycle, activation decay, availability, power balance, cost): P95 ≤ 1 s. | Benchmark sweeping the whole parameter box. | Nielsen 0.1 s and 1 s limits [V]; FARIS today ≈ 1 s | F1 (history), F4 (coupled) | Partial: about 1 s for the history alone |
| PERF-021 | The UI shall state which edits reuse cached transport and which require new transport, before the edit is applied. | 100 % of editable parameters carry a cost class (instant / seconds / transport run). | Parameter registry lint. | Trap: "1 s recalculation" hides edits that need minutes [R3] | F1 | No |
| PERF-022 | Work over 1 s shall show progress; over 10 s, determinate progress with an estimate. | Busy state ≤ 100 ms after start for 100 % of operations > 1 s; operations > 10 s show stage, percent and ETA with \|error\| ≤ 30 % after the first 20 %. | Scripted audit of every long operation. | Nielsen 1 s and 10 s [V] | F1 | Partial: spinners and stage text; the Design flux window shows stage, elapsed time, error against target and a labelled estimate of time left; the Design check window shows elapsed time, the stage line, a bar for the current stage and, after 20 percent of a counted stage, a labelled estimate of the time left in that stage (test of the 30 percent bound at a varying pace); no whole-check estimate, STEP reading, conversion and audit steps have no estimate, and accuracy on real runs is unmeasured. |
| PERF-023 | Transport runs shall show progressive results with their current statistical error. | First partial tally ≤ 5 s after launch; refresh ≤ every 2 s; partial values labelled provisional. | Run log timing. | Seow 2008 (book) 7-10 s captive limit [U: book not read]; 5 s first partial and 2 s refresh are FARIS choices; partly unverified [U] | F2 | No |
| PERF-024 | Cancellation shall be prompt and complete. | UI shows "cancelling" ≤ 100 ms; job stopped and resources freed ≤ 2 s; external solver killed ≤ 5 s; 0 orphan processes. | Kill test across every adapter. | Measured 2026-10-01: run and cancel acknowledged ≤ 0.21 s | F1 | Partial: acknowledgement measured; the Design flux and Design check windows show "Cancelling" in the same call as the press (tested under 100 ms for both) and pass the cancel to the engine; kill times and orphan audit not run |
| PERF-025 | Every study shall offer a preview tier and a production tier. | Preview result with stated uncertainty ≤ 60 s on RL for RM-S and RM-M; production runs stop on a target error, not a fixed history count. | Timing test; trigger test. | OpenMC batch triggers [V]; laptop preview rule from the BNCT sprint [internal] | F2 | Partial: the reactor study (app Transport panel and `faris reactor run`) has a Preview tier (90,000 histories, 46 s on 2 threads on the demo; total tritium 0.4 %, magnet fast flux about 65 %, both errors stated) and a Production tier. With a window Python (`--window-python`, or Window Python in the app) Production runs two passes: an analogue pass for tritium, heating and first-wall responses, and a weight-window pass for the magnet fast flux. On the demo at 3 threads a complete Production run took 384 s with new windows (114 s of them making the windows) and 204 s with cached windows: magnet fast flux 4.7 to 4.9 % in 600,000 to 940,000 windowed histories, tritium 0.17 to 0.20 %, against an estimated 15 M analogue histories (about 2.4 h at 4 threads) for the magnet flux alone. Not met: only the demo scenario is measured (not RM-M), and deep-stack responses stay noisy (vessel flux 37 to 61 %) |
| PERF-026 | FARIS shall predict run time and cost to a target error before a run starts. | Prediction within ±30 % for 90 % of runs. | Predicted vs actual over the benchmark set. | FARIS choice | F2 | No |
| PERF-027 | Unchanged work shall never rerun. | Re-opening or re-running an unchanged study returns cached results in ≤ 1 s with 0 transport runs; 0 false cache hits (see AUTO-033). | Mutation test on cache keys. | Nix/Bazel content addressing [V] | F1 | Partial: recorded bundles reused; no general cache |

## Computation throughput and scaling

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| PERF-030 | FARIS shall publish transport throughput for every reference model and guard it. | Histories/s/core and parallel efficiency reported per release; > 5 % regression fails the release. | Performance CI on dedicated hardware. | FARIS choice; no published fusion throughput reference is claimed; partly unverified [U] | F2 | No |
| PERF-031 | Transport shall scale across local cores. | Parallel efficiency ≥ 80 % at 8 threads on RL and ≥ 70 % at 64 threads on RW. | Strong-scaling benchmark on RM-M. | Target is FARIS choice; OpenMC scaling literature: Siegel et al. 2014 [paywalled], Romano et al. 2015 [paywalled]; weak scaling on Mira per ANL deck; partly unverified [U] | F2 | Unmeasured: demo runs used 2 threads |
| PERF-032 | Transport shall scale on clusters through the adapter. | ≥ 80 % strong-scaling efficiency to 32 MPI ranks on RM-L. | Cluster benchmark. | OpenMC MPI+OpenMP [V] | F6 | No |
| PERF-033 | GPU transport shall be usable where the adapter supports it, with measured, labelled speedup. | Speedup vs RL-class CPU reported on RM-M at equal error; results within 3σ of CPU. | Paired runs. | OpenMC: one A100 ≈ 200 Xeon cores on a fission benchmark [V]; no fusion figure found [U] | F6 | No |
| PERF-034 | Embarrassingly parallel studies (sweeps, sensitivity, optimisation) shall scale. | ≥ 90 % efficiency to all cores on RL and RW; scheduler overhead ≤ 50 ms per case; 10,000 cached cases without RSS growth > 5 %. | Soak and scaling tests. | FARIS choice | F6 | No |
| PERF-035 | Field operations shall stay interactive on large meshes. | Slice ≤ 200 ms and isosurface ≤ 1 s on a 10⁷-cell mesh tally on RW; ≤ 1 s for both on 10⁶ cells on RL. | Benchmark on stored meshes. | ParaView feature set [V, no numbers published] | F2 | No |

## Resources and footprint

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| PERF-040 | Idle memory shall be small. | Empty-state RSS ≤ 250 MB; RM-S loaded ≤ 1.5 GB. | Scripted RSS measurement. | FARIS choice; measure VS Code on RL before citing a comparison; partly unverified [U] | F1 | Unmeasured |
| PERF-041 | Memory shall be budgeted, not discovered. | Default-size study peak ≤ 4 GB; configurable hard cap; a job projected to exceed free memory minus margin is refused with an explanation and an alternative. | Stress test inside a cgroup; admission test. | Three out-of-memory kills on the reference laptop [internal] | F1 | Partial: every external job runs under a memory cap (default 4 GiB). `--setting memory.cap_mb` and `memory.margin_mb`, the settings file and the environment variables set the cap and margin. A job refused by the free-memory check exits 5 with the numbers and what to do. A finished job records its peak memory (Linux, sampled every 50 ms). Not done: no per-job projection model, no stress test inside a cgroup, the default-size study's peak is not measured, and peak memory is sampled |
| PERF-042 | Long sessions shall not leak. | RSS drift ≤ 5 % after warm-up over an 8-hour soak. | Nightly soak. | FARIS choice | F1 | No |
| PERF-043 | Heavy external jobs shall be admitted by a memory- and core-aware queue. | Default 1 concurrent transport job; 0 out-of-memory kills in a 72-hour soak. | Soak with mixed jobs. | Laptop rule [internal] | F1 | Partial: one-at-a-time by convention |
| PERF-044 | Background compute shall yield to interaction and to battery. | Low priority by default; optional pause on battery; interactive targets PERF-010/011 still met while a job runs. | Measure PERF-010/011 under load. | FARIS choice | F2 | No |
| PERF-045 | The application shall stay small. | Installed app ≤ 150 MB excluding nuclear data and sample results. | Package check. | faris-app release binary is 31 MB today | F1 | Met: 31 MB binary |
| PERF-046 | Energy use shall be measured and guarded. | Power above idle during orbit and recalculation published per release; > 10 % regression fails. | Power measurement on RL. | FARIS choice | F4 | No |
| PERF-047 | Size limits shall be documented and enforced gracefully. | Published limits for triangles, cells, tally bins, history length and study size; at 110 % of a limit FARIS refuses with a message, never crashes. | Limit tests at 100 % and 110 %. | FARIS choice | F2 | No |

## Measurement discipline

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| PERF-050 | Every target in this file shall have an automated benchmark that runs on reference hardware. | 100 % of PERF requirements automated by their phase gate. | Requirement-to-benchmark trace (QA-002). | FARIS choice | F1 | No |
| PERF-051 | Performance regressions shall fail CI. | Wall-time regression > 5 % or instruction-count regression > 2 % fails, on dedicated runners. | Benchmark CI. | rustc-perf judges regressions with a per-benchmark IQR significance threshold, not a fixed percentage [V](https://github.com/rust-lang/rustc-perf/blob/master/docs/comparison-analysis.md); the 5 % / 2 % limits are FARIS choice | F1 | No |
| PERF-052 | FARIS shall include a performance overlay and trace export. | Overlay shows frame time, memory, job queue; traces open in Perfetto. | UI test; open trace in Perfetto. | Blender statistics overlay [V](https://docs.blender.org/manual/en/latest/editors/3dview/display/overlays.html); Perfetto opens Chrome JSON traces [V](https://perfetto.dev/docs/getting-started/other-formats) | F1 | No |

## Traps

- Mean frame rate. A 90 fps mean can hide 100 ms stalls; only P99 and max frame time count.
- "Startup time" without a definition. A splash screen or an empty window is not startup; the
  clock stops at the first frame that accepts input with the default study ready.
- Histories as the cost of a transport run. Cost is time to a stated error (figure of merit), so
  variance reduction and tally choice count, not raw histories per second.
- Speedups quoted from one case. GPU and variance-reduction gains are problem-specific; publish them
  per reference model, at equal error, never as a general factor.
- Warm-cache benchmarks only. A cache makes a re-run look instant; measure the cold path and the
  cache-miss path separately.
- Benchmarks on a developer workstation. Targets are judged on RL; a pass on faster hardware
  says nothing about the laptop a user actually has.
