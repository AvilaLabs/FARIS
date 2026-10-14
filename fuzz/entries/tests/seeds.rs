//! Every entry runs on a few plain inputs and on any seed corpus present, so a change that makes a
//! parser panic is caught by `cargo test` without the fuzzer (QA-033).

use faris_fuzz_entries::ENTRIES;

// Verifies: QA-033
#[test]
fn every_entry_survives_plain_inputs_and_the_seed_corpus() {
    let plain: &[&[u8]] = &[
        b"",
        b"{}",
        b"[]",
        b"null",
        b"\xff\xff\xff\xff\xff\xff\xff\xff",
        b"{\"schema_version\": 1, \"schema\": \"x\", \"steps\": [[1]], \"files\": {}}",
        b"\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0",
    ];
    for (name, entry) in ENTRIES {
        for input in plain {
            entry(input);
        }
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("corpus")
            .join(name);
        if let Ok(read) = std::fs::read_dir(dir) {
            for file in read.flatten() {
                entry(&std::fs::read(file.path()).unwrap());
            }
        }
    }
}

#[test]
fn the_entry_names_are_unique_and_every_one_has_a_fuzz_target_file() {
    let targets = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("fuzz_targets");
    let mut seen = std::collections::BTreeSet::new();
    for (name, _) in ENTRIES {
        assert!(seen.insert(*name), "{name} is listed twice");
        assert!(
            targets.join(format!("{name}.rs")).is_file(),
            "{name} has no fuzz_targets/{name}.rs"
        );
    }
}
