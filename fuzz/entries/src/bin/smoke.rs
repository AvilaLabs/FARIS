//! A blind mutation driver for the fuzz entries, for machines where `cargo fuzz` cannot be built.
//!
//! It is NOT coverage-guided and does not replace the libFuzzer targets; it mutates the seed corpus
//! at random and watches for panics, hangs and runaway memory, with the same limits the libFuzzer
//! run uses (10 s per input, 2 GiB). A parent process supervises a child that does the work, so an
//! abort, a hang or an out-of-memory stop still leaves the input that caused it.
//!
//!   smoke <target> <seconds> <corpus-dir> <artifact-dir>
//!
//! Prints one JSON line: target, seconds, executions, crashes.

use std::alloc::{GlobalAlloc, Layout, System};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const RSS_LIMIT: usize = 2048 << 20;
const TIMEOUT: Duration = Duration::from_secs(10);
const MAX_LEN: usize = 1 << 20;

/// Counts live heap bytes and stops the process when the limit is passed, like libFuzzer's
/// `-rss_limit_mb`.
struct Capped;
static LIVE: AtomicUsize = AtomicUsize::new(0);

// SAFETY: forwards every call to the system allocator and only adds a counter.
unsafe impl GlobalAlloc for Capped {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size() > RSS_LIMIT {
            eprintln!("out-of-memory: the heap passed {} MiB", RSS_LIMIT >> 20);
            std::process::exit(137);
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if new_size > layout.size()
            && LIVE.fetch_add(new_size - layout.size(), Ordering::Relaxed) + new_size
                - layout.size()
                > RSS_LIMIT
        {
            eprintln!("out-of-memory: the heap passed {} MiB", RSS_LIMIT >> 20);
            std::process::exit(137);
        }
        if new_size < layout.size() {
            LIVE.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOC: Capped = Capped;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }
}

const TOKENS: &[&[u8]] = &[
    b"null",
    b"true",
    b"false",
    b"{}",
    b"[]",
    b"\"\"",
    b"0",
    b"-1",
    b"1e308",
    b"1e-308",
    b"18446744073709551615",
    b"4294967295",
    b"NaN",
    b"\"schema\"",
    b"\"schema_version\"",
    b"\":\"",
    b",",
    b"\n",
    b"\0",
    b"\xff\xff\xff\xff",
    b"\xff\xff\xff\xff\xff\xff\xff\xff",
    b"\x00\x00\x00\x00\x00\x00\x00\x80",
    b"..",
    b"../",
    b"/",
    b"\\",
];

fn mutate(rng: &mut Rng, corpus: &[Vec<u8>]) -> Vec<u8> {
    let mut data = if corpus.is_empty() || rng.below(20) == 0 {
        (0..rng.below(64)).map(|_| rng.next() as u8).collect()
    } else {
        corpus[rng.below(corpus.len())].clone()
    };
    for _ in 0..1 + rng.below(4) {
        let len = data.len();
        match rng.below(9) {
            0 if len > 0 => data[rng.below(len)] ^= 1 << rng.below(8),
            1 if len > 0 => data[rng.below(len)] = rng.next() as u8,
            2 => {
                let at = rng.below(len + 1);
                data.insert(at, rng.next() as u8);
            }
            3 if len > 0 => {
                data.remove(rng.below(len));
            }
            4 if len > 0 => data.truncate(rng.below(len)),
            5 => {
                let t = TOKENS[rng.below(TOKENS.len())];
                let at = rng.below(len + 1);
                data.splice(at..at, t.iter().copied());
            }
            6 if len > 1 => {
                // Overwrite a stretch with a token (numbers and lengths become extreme values).
                let t = TOKENS[rng.below(TOKENS.len())];
                let at = rng.below(len);
                let end = (at + t.len()).min(len);
                data.splice(at..end, t.iter().copied());
            }
            7 if len > 1 => {
                // Duplicate a block.
                let a = rng.below(len);
                let b = (a + 1 + rng.below(64)).min(len);
                let block = data[a..b].to_vec();
                let at = rng.below(len + 1);
                data.splice(at..at, block);
            }
            8 if corpus.len() > 1 => {
                let other = &corpus[rng.below(corpus.len())];
                if !other.is_empty() {
                    let a = rng.below(other.len());
                    let b = (a + 1 + rng.below(256)).min(other.len());
                    let at = rng.below(len + 1);
                    data.splice(at..at, other[a..b].iter().copied());
                }
            }
            _ => {}
        }
        data.truncate(MAX_LEN);
    }
    data
}

fn load_corpus(dir: &Path) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    if let Ok(read) = std::fs::read_dir(dir) {
        for entry in read.flatten() {
            if let Ok(bytes) = std::fs::read(entry.path())
                && bytes.len() <= MAX_LEN
            {
                out.push(bytes);
            }
        }
    }
    out
}

fn child(target: &str, seconds: u64, corpus_dir: &Path, work: &Path) {
    let entry = faris_fuzz_entries::ENTRIES
        .iter()
        .find(|(name, _)| *name == target)
        .unwrap_or_else(|| panic!("no target named {target}"))
        .1;
    std::panic::set_hook(Box::new(|info| {
        eprintln!(
            "panic: {info}\n{}",
            std::backtrace::Backtrace::force_capture()
        );
        std::process::exit(101);
    }));
    // A watchdog: an input that runs for more than TIMEOUT stops the process.
    static STARTED_MS: AtomicU64 = AtomicU64::new(0);
    let clock = Instant::now();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(200));
            let started = STARTED_MS.load(Ordering::Relaxed);
            let now = clock.elapsed().as_millis() as u64 + 1;
            if started != 0 && now.saturating_sub(started) > TIMEOUT.as_millis() as u64 {
                eprintln!(
                    "timeout: one input ran for more than {} s",
                    TIMEOUT.as_secs()
                );
                std::process::exit(124);
            }
        }
    });
    let mut corpus = load_corpus(corpus_dir);
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(1, |d| d.as_nanos() as u64)
        | 1;
    let mut rng = Rng(seed);
    let current = work.join("current-input");
    // The seeds themselves first.
    let seeds = corpus.clone();
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut execs = 0u64;
    let mut last_report = Instant::now();
    let mut next_seed = 0;
    while Instant::now() < deadline {
        let data = if next_seed < seeds.len() {
            next_seed += 1;
            seeds[next_seed - 1].clone()
        } else {
            let data = mutate(&mut rng, &corpus);
            if corpus.len() < 4096 && rng.below(64) == 0 {
                corpus.push(data.clone());
            }
            data
        };
        let _ = std::fs::write(&current, &data);
        STARTED_MS.store(clock.elapsed().as_millis() as u64 + 1, Ordering::Relaxed);
        entry(&data);
        STARTED_MS.store(0, Ordering::Relaxed);
        execs += 1;
        if last_report.elapsed() > Duration::from_millis(500) {
            let _ = std::fs::write(work.join("execs"), execs.to_string());
            last_report = Instant::now();
        }
    }
    let _ = std::fs::write(work.join("execs"), execs.to_string());
}

fn read_execs(work: &Path) -> u64 {
    std::fs::read_to_string(work.join("execs"))
        .ok()
        .and_then(|t| t.trim().parse().ok())
        .unwrap_or(0)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--child") {
        child(
            &args[2],
            args[3].parse().unwrap(),
            Path::new(&args[4]),
            Path::new(&args[5]),
        );
        return;
    }
    if args.len() != 5 {
        eprintln!("usage: smoke <target> <seconds> <corpus-dir> <artifact-dir>");
        std::process::exit(2);
    }
    let (target, seconds) = (&args[1], args[2].parse::<u64>().expect("seconds"));
    let (corpus, artifacts) = (PathBuf::from(&args[3]), PathBuf::from(&args[4]));
    std::fs::create_dir_all(&artifacts).expect("artifact directory");
    let work = std::env::temp_dir().join(format!("faris-smoke-{}-{target}", std::process::id()));
    std::fs::create_dir_all(&work).expect("work directory");
    let start = Instant::now();
    let (mut execs, mut crashes) = (0u64, 0u32);
    while start.elapsed().as_secs() < seconds {
        let left = seconds - start.elapsed().as_secs();
        let _ = std::fs::remove_file(work.join("execs"));
        let log = work.join("stderr.txt");
        let status = Command::new(std::env::current_exe().unwrap())
            .args(["--child", target, &left.to_string()])
            .arg(&corpus)
            .arg(&work)
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(&log).unwrap())
            .status()
            .expect("child starts");
        execs += read_execs(&work);
        if status.success() {
            break;
        }
        crashes += 1;
        let kind = match status.code() {
            Some(124) => "timeout",
            Some(137) => "oom",
            Some(101) => "crash",
            _ => "abort",
        };
        let input = std::fs::read(work.join("current-input")).unwrap_or_default();
        let name = format!("{kind}-{target}-{crashes}");
        let _ = std::fs::write(artifacts.join(&name), &input);
        let _ = std::fs::copy(&log, artifacts.join(format!("{name}.log")));
        eprintln!("{name}: {} bytes, {:?}", input.len(), status);
        if crashes >= 5 {
            break; // a seed that crashes would otherwise restart the child over and over
        }
    }
    println!(
        "{{\"target\": \"{target}\", \"seconds\": {}, \"executions\": {execs}, \"crashes\": {crashes}}}",
        start.elapsed().as_secs()
    );
    let _ = std::fs::remove_dir_all(&work);
}
