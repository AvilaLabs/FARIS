use super::*;
use serde_json::json;

fn key(kind: &str, a: Value, b: Value) -> CacheKey {
    cache_key(kind, [("model", a), ("settings", b)])
}

fn open() -> (tempfile::TempDir, ResultCache) {
    let dir = tempfile::tempdir().unwrap();
    let cache = ResultCache::open(dir.path().join("cache"));
    (dir, cache)
}

/// A small deterministic generator, so the property tests need no extra crate.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }
}

/// The same document written two ways: keys in another order, numbers spelled differently.
fn variant_pair(seed: u64) -> (String, String) {
    let mut rng = Lcg(seed);
    let n = 3 + (rng.next() % 6) as usize;
    let mut fields: Vec<(String, f64)> = (0..n)
        .map(|i| {
            let whole = (rng.next() % 1000) as f64;
            let fraction = (rng.next() % 8) as f64 / 8.0;
            (format!("field_{i}_m"), whole + fraction)
        })
        .collect();
    let plain = |fields: &[(String, f64)]| {
        let parts: Vec<String> = fields.iter().map(|(k, v)| format!("\"{k}\":{v}")).collect();
        format!("{{{}}}", parts.join(","))
    };
    let first = plain(&fields);
    fields.reverse();
    let spaced: Vec<String> = fields
        .iter()
        .map(|(k, v)| {
            // Whole numbers gain a ".0" or an exponent form; others a trailing zero.
            let text = if v.fract() == 0.0 {
                if rng.next().is_multiple_of(2) {
                    format!("{v}.0")
                } else {
                    format!("{}e0", *v as i64)
                }
            } else {
                format!("{v}0")
            };
            format!("  \"{k}\" :\n {text}")
        })
        .collect();
    (first, format!("{{\n{}\n}}", spaced.join(",\n")))
}

// Verifies: AUTO-034
#[test]
fn equal_inputs_hash_equally_over_200_reordered_and_reformatted_pairs() {
    for seed in 0..200 {
        let (a, b) = variant_pair(seed);
        let a: Value = serde_json::from_str(&a).unwrap();
        let b: Value = serde_json::from_str(&b).unwrap();
        let ka = key("test-kind", a, json!({"threads": 4}));
        let kb = key("test-kind", b, json!({"threads": 4.0}));
        assert_eq!(ka.digest(), kb.digest(), "pair {seed}");
    }
}

// Verifies: AUTO-034
#[test]
fn a_timestamp_path_or_host_name_that_is_not_a_component_never_changes_the_key() {
    // Components are only what the caller names; nothing ambient (time, working directory, host)
    // enters the digest, so two keys built at different moments are equal.
    let a = key("test-kind", json!({"x": 1}), json!(null));
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let b = key("test-kind", json!({"x": 1}), json!(null));
    assert_eq!(a, b);
}

// Verifies: AUTO-033, AUTO-034
#[test]
fn changing_any_one_component_changes_the_key() {
    let base: Vec<(&str, Value)> = vec![
        ("model", json!({"a": 1.5, "b": [1, 2, 3]})),
        ("data_library_sha256", json!("abc")),
        ("seed", json!(7)),
        ("adapter", json!("adapter/1.0")),
        ("threads", json!(4)),
    ];
    let reference = cache_key("test-kind", base.clone());
    for i in 0..base.len() {
        let mut changed = base.clone();
        changed[i].1 = match &changed[i].1 {
            Value::String(s) => json!(format!("{s}x")),
            Value::Number(n) => json!(n.as_f64().unwrap() + 1.0),
            _ => json!({"a": 1.5000000000000002, "b": [1, 2, 3]}),
        };
        assert_ne!(
            cache_key("test-kind", changed).digest(),
            reference.digest(),
            "component {} did not change the key",
            base[i].0
        );
    }
    assert_ne!(
        cache_key("other-kind", base.clone()).digest(),
        reference.digest()
    );
    let mut extra = base;
    extra.push(("one_more", json!(1)));
    assert_ne!(cache_key("test-kind", extra).digest(), reference.digest());
}

// Verifies: AUTO-032
#[test]
fn an_entry_records_every_key_component_readably() {
    let (_dir, cache) = open();
    let k = key("test-kind", json!({"radius_m": 3.3}), json!({"threads": 2}));
    cache.put_bytes(&k, "result.json", b"{}", 12.0).unwrap();
    let text = std::fs::read_to_string(cache.entry_dir(&k).join(ENTRY_FILE)).unwrap();
    for needle in [
        "radius_m",
        "3.3",
        "threads",
        "faris_version",
        "cpu_arch",
        "result.json",
    ] {
        assert!(text.contains(needle), "{needle} missing from {text}");
    }
    let record = cache.explain(&k.digest()[..10]).unwrap();
    assert_eq!(record.components, *k.components());
    assert_eq!(record.compute_seconds, 12.0);
}

// Verifies: AUTO-039
#[test]
fn a_hit_carries_when_it_was_made_by_which_version_and_under_which_key() {
    let (_dir, cache) = open();
    let k = key("test-kind", json!(1), json!(2));
    assert!(matches!(cache.lookup(&k), Lookup::Miss));
    cache.put_bytes(&k, "result.json", b"[1,2]", 31.0).unwrap();
    let (lookup, bytes) = cache.get_bytes(&k, "result.json");
    assert_eq!(bytes.unwrap(), b"[1,2]");
    let hit = lookup.hit().unwrap();
    assert_eq!(hit.provenance.key, k.digest());
    assert_eq!(hit.provenance.faris_version, FARIS_VERSION);
    assert!(hit.provenance.created_utc.ends_with('Z'));
    let text = hit.provenance.text();
    assert!(text.starts_with("reused from 20"), "{text}");
    assert!(text.contains("31 s to calculate"), "{text}");
    assert!(text.contains(&k.digest()[..12]), "{text}");
}

// Verifies: AUTO-035
#[test]
fn two_hundred_injected_corruptions_are_refused_and_reported_never_served() {
    let (_dir, cache) = open();
    let k = key("test-kind", json!({"v": 1}), json!(null));
    let payload: Vec<u8> = (0..4096u32).map(|i| (i * 7 % 251) as u8).collect();
    let mut rng = Lcg(99);
    let mut refused = 0;
    for round in 0..200 {
        cache.put_bytes(&k, "result.bin", &payload, 1.0).unwrap();
        let dir = cache.entry_dir(&k);
        let file = dir.join("result.bin");
        match round % 5 {
            0 => {
                let mut bytes = std::fs::read(&file).unwrap();
                let at = rng.next() as usize % bytes.len();
                bytes[at] ^= 1 << (rng.next() % 8);
                std::fs::write(&file, bytes).unwrap();
            }
            1 => {
                let bytes = std::fs::read(&file).unwrap();
                std::fs::write(
                    &file,
                    &bytes[..bytes.len() - 1 - (rng.next() as usize % 100)],
                )
                .unwrap();
            }
            2 => std::fs::remove_file(&file).unwrap(),
            3 => {
                // The record is edited: the components no longer hash to the key.
                let path = dir.join(ENTRY_FILE);
                let text = std::fs::read_to_string(&path).unwrap();
                std::fs::write(&path, text.replace("\"v\": 1", "\"v\": 2")).unwrap();
            }
            _ => std::fs::write(dir.join("extra.bin"), b"unlisted").unwrap(),
        }
        match cache.lookup(&k) {
            Lookup::Rejected(why) => {
                assert!(why.contains("refused"), "{why}");
                refused += 1;
            }
            other => panic!("round {round}: a damaged entry was not refused: {other:?}"),
        }
        cache.discard(&k);
    }
    assert_eq!(refused, 200);
    assert_eq!(cache.stats().corrupt, 200);
    assert_eq!(cache.stats().hits, 0);
}

// Verifies: AUTO-038
#[test]
fn hits_and_false_hits_are_counted_apart() {
    let (_dir, cache) = open();
    let k = key("test-kind", json!(1), json!(2));
    cache.put_bytes(&k, "r.json", b"{}", 0.0).unwrap();
    assert!(cache.lookup(&k).hit().is_some());
    assert!(cache.lookup(&k).hit().is_some());
    assert!(matches!(
        cache.lookup(&key("test-kind", json!(9), json!(9))),
        Lookup::Miss
    ));
    // An entry that is whole but whose stored components disagree with the key asked for: the
    // key claims the same digest as the stored entry but different components.
    let forged = CacheKey {
        kind: k.kind.clone(),
        digest: k.digest.clone(),
        components: key("test-kind", json!(5), json!(6)).components,
    };
    match cache.lookup(&forged) {
        Lookup::Rejected(why) => assert!(why.contains("false hit"), "{why}"),
        other => panic!("{other:?}"),
    }
    let stats = cache.stats();
    assert_eq!(
        (stats.hits, stats.misses, stats.false_hits, stats.corrupt),
        (2, 1, 1, 0)
    );
    assert_eq!(stats.hit_rate(), Some(0.5));
    assert_eq!(stats.false_hit_rate(), Some(1.0 / 3.0));
    assert_eq!(cache.session_stats(), stats);
    // The counters survive reopening the folder.
    assert_eq!(ResultCache::open(cache.root()).stats(), stats);
}

// Verifies: AUTO-036
#[test]
fn gc_removes_least_recently_used_first_and_never_a_pinned_entry() {
    let (_dir, cache) = open();
    let keys: Vec<CacheKey> = (0..4)
        .map(|i| key("test-kind", json!(i), json!(null)))
        .collect();
    for (i, k) in keys.iter().enumerate() {
        cache
            .put_bytes(k, "r.bin", &vec![i as u8; 1000], 0.0)
            .unwrap();
        // Distinct, ordered last-use times.
        std::fs::write(cache.entry_dir(k).join(USED_FILE), (1000 + i).to_string()).unwrap();
    }
    cache.set_pinned(&keys[0].digest()[..8], true).unwrap();
    // 4000 bytes of content; keep room for two entries.
    let (removed, left) = cache.gc(2100, None);
    assert_eq!(
        removed,
        vec![keys[1].dir_name(), keys[2].dir_name()],
        "oldest unpinned first; the pinned oldest stays"
    );
    assert_eq!(left, 2000);
    assert!(cache.lookup(&keys[0]).hit().is_some());
    assert!(cache.lookup(&keys[3]).hit().is_some());
    // Pinned entries alone can leave the cache above the limit; that is reported, not forced.
    cache.set_pinned(&keys[3].digest()[..8], true).unwrap();
    let (removed, left) = cache.gc(10, None);
    assert!(removed.is_empty());
    assert_eq!(left, 2000);
    // Unpinned, they go.
    cache.set_pinned(&keys[3].digest()[..8], false).unwrap();
    let (removed, _) = cache.gc(1000, None);
    assert_eq!(removed, vec![keys[3].dir_name()]);
}

// Verifies: AUTO-036
#[test]
fn the_size_limit_is_enforced_on_every_store() {
    let dir = tempfile::tempdir().unwrap();
    let cache = ResultCache::open(dir.path()).with_max_bytes(2500);
    for i in 0..6 {
        let k = key("test-kind", json!(i), json!(null));
        cache.put_bytes(&k, "r.bin", &vec![0u8; 1000], 0.0).unwrap();
        assert!(cache.list().iter().map(|e| e.bytes).sum::<u64>() <= 2500);
        // The entry just stored is always there.
        assert!(cache.lookup(&k).hit().is_some());
    }
    assert!(cache.stats().evicted >= 4);
}

// Verifies: AUTO-035, AUTO-036
#[test]
fn list_verify_and_clear_cover_entries_legacy_folders_and_interrupted_stores() {
    let (_dir, cache) = open();
    let good = key("test-kind", json!(1), json!(null));
    let bad = key("test-kind", json!(2), json!(null));
    cache.put_bytes(&good, "r.bin", b"good", 0.0).unwrap();
    cache.put_bytes(&bad, "r.bin", b"bad", 0.0).unwrap();
    std::fs::write(cache.entry_dir(&bad).join("r.bin"), b"BAD").unwrap();
    let legacy = cache
        .root()
        .join("reactor-windows-".to_string() + &"a".repeat(64));
    std::fs::create_dir_all(legacy.join("x")).unwrap();
    let design_legacy = cache.root().join("b".repeat(64));
    std::fs::create_dir_all(&design_legacy).unwrap();
    std::fs::create_dir_all(cache.root().join(".tmp-abc-1")).unwrap();
    std::fs::create_dir_all(cache.root().join("not-ours")).unwrap();
    assert_eq!(cache.list().len(), 2);
    let lines = cache.verify_all();
    let status = |name: &str| lines.iter().find(|l| l.dir_name == name).map(|l| l.status);
    assert_eq!(status(&good.dir_name()), Some("ok"));
    assert_eq!(status(&bad.dir_name()), Some("damaged"));
    assert_eq!(
        status(&legacy.file_name().unwrap().to_string_lossy()),
        Some("legacy")
    );
    assert_eq!(status(".tmp-abc-1"), Some("damaged"));
    assert_eq!(status("not-ours"), None);
    // A legacy folder is never a hit.
    assert!(matches!(
        cache.lookup(&key("test-kind", json!(3), json!(null))),
        Lookup::Miss
    ));
    cache.set_pinned(&good.digest()[..8], true).unwrap();
    let removed = cache.clear(false);
    assert_eq!(removed.len(), 4, "{removed:?}");
    assert!(cache.entry_dir(&good).is_dir());
    assert!(
        cache.root().join("not-ours").is_dir(),
        "only the cache's own folders go"
    );
    assert_eq!(cache.clear(true), vec![good.dir_name()]);
}

#[test]
fn an_entry_built_in_place_is_sealed_and_resealed_with_its_date_kept() {
    let (_dir, cache) = open();
    let k = key("test-kind", json!(1), json!(null));
    let dir = cache.entry_dir(&k);
    std::fs::create_dir_all(dir.join("windows")).unwrap();
    std::fs::write(dir.join("windows").join("filled.h5"), b"windows").unwrap();
    assert!(
        cache.lookup(&k).hit().is_none(),
        "unsealed folders are not entries"
    );
    cache.seal(&k, 5.0).unwrap();
    let first = cache.explain(k.digest()).unwrap();
    std::fs::create_dir_all(dir.join("peak")).unwrap();
    std::fs::write(dir.join("peak").join("volumes.npz"), b"volumes").unwrap();
    assert!(
        cache.lookup(&k).hit().is_none(),
        "a file added after sealing makes the entry refused until it is sealed again"
    );
    cache.seal(&k, 0.0).unwrap();
    let second = cache.explain(k.digest()).unwrap();
    assert_eq!(second.created_unix, first.created_unix);
    assert_eq!(second.compute_seconds, 5.0);
    assert_eq!(second.files.len(), 2);
    assert!(cache.lookup(&k).hit().is_some());
}

#[test]
fn utc_text_is_a_calendar_date() {
    assert_eq!(utc_text(0), "1970-01-01T00:00:00Z");
    assert_eq!(utc_text(1_792_000_000), "2026-10-14T17:46:40Z");
    assert_eq!(utc_text(951_782_400), "2000-02-29T00:00:00Z");
}

// Verifies: AUTO-031
#[test]
fn only_the_cache_module_builds_a_key() {
    // A static scan of the workspace sources: any function with `cache_key` in its name outside
    // this module must call `result_cache::cache_key`, and no other file may hash an "identity"
    // document itself.
    fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
        for item in std::fs::read_dir(dir).unwrap().flatten() {
            let path = item.path();
            if path.is_dir() {
                sources(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    for name in ["faris-engine", "faris-study", "faris-cli", "faris-model"] {
        sources(&crates.join(name).join("src"), &mut files);
    }
    assert!(files.len() > 50);
    for file in files {
        if file.ends_with("result_cache.rs") || file.to_string_lossy().contains("result_cache/") {
            continue;
        }
        let text = std::fs::read_to_string(&file).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            let defines = line.contains("fn ")
                && line.contains("cache_key")
                && line.contains('(')
                && !line.contains("fn fuzz_");
            let is_test = i > 0 && lines[i - 1].trim() == "#[test]";
            if defines && !is_test && !line.trim_start().starts_with("//") {
                let body = lines[i..(i + 40).min(lines.len())].join("\n");
                assert!(
                    body.contains("result_cache::cache_key("),
                    "{}:{} builds a key without the one function",
                    file.display(),
                    i + 1
                );
            }
            assert!(
                !line.contains("sha256_hex(identity")
                    && !line.contains("digest(identity.to_string"),
                "{}:{} hashes a key document itself",
                file.display(),
                i + 1
            );
        }
    }
}
