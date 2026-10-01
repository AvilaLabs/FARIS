# Working on FARIS

- FARIS is Rust-first and native desktop-first. Use egui/eframe for UI and wgpu
  for 3D. Python may support external scientific tools and independent controls;
  it must not become the authoritative core.
- Keep scenario semantics in `faris-model` and calculations/shared operations in
  `faris-engine`. The CLI and desktop are clients. The UI must not own physics.
- Maintain one scenario definition for geometry, materials, solver inputs, and
  displayed component identities. A display cutaway cannot change solver geometry.
- Preserve `PASS`, `FAIL`, `INCONCLUSIVE`, and `NOT_EVALUATED` with their scope.
  Never fill absent physical results with illustrative numbers or infer service
  life from a geometry or execution success.
- Keep authored assumptions, literature inputs, measurements, solver results,
  and model extrapolations distinguishable. Record units and normalization at
  adapter boundaries. Input hashes establish identity, not scientific correctness.
- Avila Core integration is not implemented. Its generated Compile study and
  evidence experience is required for the finished demo; the simulation remains
  independently usable. Use a few coarse stages and reusable/generated packaging;
  do not require hand-authored contracts for every component or time step.
- Follow `docs/DEMO_ROADMAP.md` for the functional demo boundary; `docs/ROADMAP.md`
  covers the full project. Show the small
  Powered by Avila Core attribution with actual Core compilation attempts;
  preserve the distinction between compilation, run readiness, and verdicts.
- Prefer small, useful increments. Avoid introducing process documents or
  approval requirements unless they resolve an actual project need.
- Keep solver jobs and substantial I/O off the egui thread. Bound work and support
  cancellation before adding external execution. Never install every potential
  scientific framework as part of routine startup.
- Use focused numerical tests and independent references where relevant. Before
  finishing a code change, run formatting, Clippy for changed targets, and
  appropriate tests. Compile the desktop when changing shared interfaces.
- Use `--jobs 1` for graphics builds on this workstation. Keep downloaded nuclear
  data, customer cases, and generated runs out of Git. Commit `Cargo.lock`.
- FARIS source code and documentation are licensed AGPL-3.0-only. New crates
  inherit the workspace license. Preserve project and third-party license notices.
  Do not publish packages or create/push a GitHub repository implicitly.
