//! Writes one real `.faris` study file for the fuzz seed corpus: `study_seed <out.faris>`.

use faris_study::{StudyDraft, ViewState, write_study};

fn main() {
    let out = std::path::PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("usage: study_seed <out.faris>"),
    );
    let dir = tempfile::tempdir().expect("scratch directory");
    let assumptions = dir.path().join("assumptions.json");
    std::fs::write(&assumptions, b"{\"a\":1}\n").expect("scratch file");
    let draft = StudyDraft {
        assumptions: Some(assumptions),
        view: ViewState {
            step: "evidence".into(),
            preset: Some("Loaded assumptions".into()),
            year: 12.5,
            field_view: "flux-slice".into(),
            history_tab: "tritium".into(),
            arrangement: "control".into(),
            allocation: "breeder-emphasis".into(),
            sweep_blanket_m: Some(0.55),
            what_if: None,
        },
        zstd_level: 3,
        ..StudyDraft::default()
    };
    write_study(&out, &draft).expect("the seed study saves");
}
