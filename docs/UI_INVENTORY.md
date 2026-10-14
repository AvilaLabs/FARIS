<!-- kind: reference -->
# Interface inventory

The windows, steps and panels of the desktop app, with the handbook page that documents each and whether the handbook
shows it in a screenshot. This is a hand-made list read from `crates/faris-app/src`; the requirement (DOC-023) asks for
a list generated from a registry in the app and compared with the handbook, and the app has no such registry. Until it
does, this page is the inventory and it can go stale.

Last checked: 2026-10-14.

The handbook has no screenshot of any panel. Its only image is the animated tour in the welcome page. `docs/images`
holds seven screenshots used by the walkthrough and the README (flux slice, history events, history recalculation,
sensitivity, coupled workspace, component flux, interface size menu); none is a current capture of every panel.

| Part | Source file | Handbook page | Screenshot in the handbook |
| --- | --- | --- | --- |
| Step 1, Design: arrangement choice, 3D viewport, outliner, properties | `steps.rs`, `viewport.rs`, `design_shared.rs`, `design_tools.rs` | [Design](guide/design.md) | none |
| Step 2, Simulate: transport results and field views | `transport_panel.rs`, `steps.rs` | [Simulate](guide/simulate.md) | none |
| Step 3, Operate: timeline, what-if sliders, history tabs | `history_panel.rs`, `history_panel/` | [Operate](guide/operate.md) | none |
| Uncertainty ensembles | `uncertainty.rs`, `uncertainty/` | [Uncertainty ensembles](guide/uncertainty.md) | none |
| Step 4, Compare and the allocation sweep | `compare_panel.rs`, `sweep_panel.rs` | [Compare](guide/compare.md) | none |
| Step 5, Evidence and Core receipts | `study_panel.rs`, `archive_panel.rs` | [Evidence](guide/evidence.md) | none |
| Maintenance window | `maintenance_panel.rs`, `maintenance_run.rs` | [Computed maintenance durations](guide/maintenance.md) | none |
| Design flux window | `design_transport_panel.rs`, `design_transport_run.rs`, `design_transport_result.rs` | [The design file](guide/design-file.md), [Run your own transport](guide/transport.md) | none |
| Design check window | `design_check_panel.rs`, `design_check_run.rs`, `design_check_result.rs` | [The design file](guide/design-file.md) | none |
| Transport configuration | `transport_panel.rs`, `tool_finder.rs` | [Run your own transport](guide/transport.md) | none |
| File menu: open, save, include evidence | `study_file.rs` | [Study files](guide/study-files.md) | none |
| Export panel | `export_panel.rs` | [Export](guide/export.md) | none |
| Command palette | `commands.rs` | [Keyboard](guide/keyboard.md) | none |
| Tour | `tour.rs` | [Take the tour](guide/quick-start.md) | the animated tour |
| Interface size menu | `settings.rs` | [Take the tour](guide/quick-start.md) | `docs/images/interface-size-menu.png`, not in the handbook |
| Account chip and sign-in card | the `avila-account` library | [Avila Labs account](guide/account.md) | none |
| Browser notice (browser version only) | `browser.rs` | [The browser version](guide/browser.md) | none |

Result: every part named here has a handbook page; none has a current screenshot, and the list is not generated.
