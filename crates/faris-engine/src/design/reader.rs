//! Which CAD reader runs the stages that have two: the native OpenCASCADE worker
//! (`faris-occt-worker`) or the Python helper. The choice is made once per run, and what ran is
//! recorded in the report, so a result is never taken for the other reader's.

use super::{CAD_HELPER, DesignError, external, sha256_file, sha256_hex};
use faris_occt_client::client::Worker;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// `--cad-reader`: which reader the STEP inspect and the per-solid tessellation use.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CadReader {
    /// The native worker when one is found and answers a ping; otherwise the Python helper.
    #[default]
    Auto,
    /// The native worker; an error when there is none.
    Native,
    /// The Python helper, whatever else is installed.
    Python,
}

impl CadReader {
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Native => "native",
            Self::Python => "python",
        }
    }

    pub fn from_label(label: &str) -> Option<Self> {
        match label {
            "auto" => Some(Self::Auto),
            "native" => Some(Self::Native),
            "python" => Some(Self::Python),
            _ => None,
        }
    }

    /// `FARIS_CAD_READER`; `None` when it is not set.
    pub fn from_environment() -> Result<Option<Self>, DesignError> {
        let Some(value) = std::env::var_os("FARIS_CAD_READER") else {
            return Ok(None);
        };
        let text = value.to_string_lossy();
        Self::from_label(text.trim()).map(Some).ok_or_else(|| {
            DesignError::Tool(format!(
                "FARIS_CAD_READER is {text:?}, which is not a CAD reader; set it to auto, native \
                 or python (or pass --cad-reader)"
            ))
        })
    }
}

/// What ran a stage: the reader and its identity, recorded in the report and in the reuse key.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ReaderStamp {
    /// `native` or `python`.
    pub reader: String,
    /// The Python helper's `HELPER_VERSION`, or the worker's.
    pub helper_version: String,
    /// sha256 of the embedded Python helper, or of the worker binary.
    pub helper_sha256: String,
    /// sha256 of the worker binary that ran; native only.
    #[serde(default)]
    pub worker_sha256: Option<String>,
    /// The OpenCASCADE release the worker answered its ping with; native only.
    #[serde(default)]
    pub occt_tag: Option<String>,
    /// Why `auto` fell back to Python although a worker was configured or found.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_unavailable: Option<String>,
}

impl ReaderStamp {
    pub fn python() -> Self {
        Self {
            reader: "python".into(),
            helper_version: external::helper_version(),
            helper_sha256: sha256_hex(CAD_HELPER.as_bytes()),
            worker_sha256: None,
            occt_tag: None,
            native_unavailable: None,
        }
    }

    pub fn is_native(&self) -> bool {
        self.reader == "native"
    }

    /// What a recorded product's reuse key holds for this reader. A Python key is the helper
    /// version alone, as before the native reader existed, so earlier records stay valid.
    pub fn identity(&self) -> String {
        match (&self.worker_sha256, &self.occt_tag) {
            (Some(sha), Some(tag)) if self.is_native() => format!("native:{sha}:{tag}"),
            _ => self.helper_version.clone(),
        }
    }
}

/// A worker that answered its ping.
#[derive(Clone, Debug)]
pub struct NativeReader {
    pub worker: Worker,
    pub stamp: ReaderStamp,
}

#[derive(Clone, Debug)]
pub enum ActiveReader {
    Native(NativeReader),
    Python(ReaderStamp),
}

impl ActiveReader {
    pub fn stamp(&self) -> &ReaderStamp {
        match self {
            Self::Native(n) => &n.stamp,
            Self::Python(stamp) => stamp,
        }
    }
}

/// Starts the worker once and asks it who it is. The worker file is hashed, so the report names
/// the binary that ran.
fn probe(worker: &Worker) -> Result<NativeReader, String> {
    let program: &Path = worker.program();
    let sha256 = sha256_file(program).map_err(|e| e.to_string())?;
    let pong = worker.ping().map_err(|e| e.to_string())?;
    let tag = pong.occt_tag.unwrap_or(pong.occt);
    let helper = pong.helper.unwrap_or_else(|| "?".into());
    Ok(NativeReader {
        worker: worker.clone(),
        stamp: ReaderStamp {
            reader: "native".into(),
            helper_version: helper,
            helper_sha256: sha256.clone(),
            worker_sha256: Some(sha256),
            occt_tag: Some(tag),
            native_unavailable: None,
        },
    })
}

/// Picks the reader. `worker` is what was found by `--occt-worker`, `FARIS_OCCT_WORKER` or the
/// file beside the program, or nothing.
pub fn resolve(choice: CadReader, worker: Option<Worker>) -> Result<ActiveReader, DesignError> {
    match choice {
        CadReader::Python => Ok(ActiveReader::Python(ReaderStamp::python())),
        CadReader::Native => {
            let Some(worker) = worker else {
                return Err(DesignError::Tool(
                    "the native CAD reader was asked for, but no faris-occt-worker was found. \
                     Pass --occt-worker PATH, set FARIS_OCCT_WORKER, or put faris-occt-worker \
                     next to the faris program (build it with `cargo build -p faris-occt \
                     --release`); or use --cad-reader python"
                        .into(),
                ));
            };
            probe(&worker).map(ActiveReader::Native).map_err(|why| {
                DesignError::Tool(format!(
                    "the native CAD reader was asked for, but {} could not be used: {why}. Check \
                     that it is the faris-occt-worker built with this FARIS and can be run from \
                     here, or use --cad-reader python",
                    worker.program().display()
                ))
            })
        }
        CadReader::Auto => {
            let mut stamp = ReaderStamp::python();
            if let Some(worker) = worker {
                match probe(&worker) {
                    Ok(native) => return Ok(ActiveReader::Native(native)),
                    Err(why) => {
                        stamp.native_unavailable = Some(format!(
                            "{} could not be used: {why}",
                            worker.program().display()
                        ));
                    }
                }
            }
            Ok(ActiveReader::Python(stamp))
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::super::testutil::stub_worker;
    use super::*;

    const FAILS: &str = "echo 'stub: not implemented' >&2; exit 5";

    fn broken_worker(dir: &Path) -> Worker {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("broken-worker");
        std::fs::write(
            &path,
            "#!/bin/sh\necho 'cannot load libstdc++' >&2\nexit 127\n",
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        Worker::new(path)
    }

    #[test]
    fn python_never_looks_at_the_worker() {
        let gone = Worker::new("/nonexistent/faris-occt-worker");
        let active = resolve(CadReader::Python, Some(gone)).unwrap();
        let stamp = active.stamp();
        assert_eq!(stamp.reader, "python");
        assert!(stamp.native_unavailable.is_none() && stamp.worker_sha256.is_none());
        assert_eq!(stamp.identity(), super::external::helper_version());
    }

    #[test]
    fn auto_without_a_worker_is_python() {
        let active = resolve(CadReader::Auto, None).unwrap();
        assert_eq!(active.stamp().reader, "python");
        assert!(active.stamp().native_unavailable.is_none());
    }

    #[test]
    fn auto_with_a_worker_that_answers_is_native_and_names_it() {
        let dir = tempfile::tempdir().unwrap();
        let worker = stub_worker(dir.path(), "w", "V7_9_3", FAILS);
        let sha = sha256_file(worker.program()).unwrap();
        let ActiveReader::Native(native) = resolve(CadReader::Auto, Some(worker)).unwrap() else {
            panic!("expected the native reader");
        };
        let stamp = native.stamp;
        assert_eq!(stamp.reader, "native");
        assert_eq!(stamp.worker_sha256.as_deref(), Some(sha.as_str()));
        assert_eq!(stamp.helper_sha256, sha);
        assert_eq!(stamp.occt_tag.as_deref(), Some("V7_9_3"));
        assert_eq!(stamp.helper_version, "faris-occt-1");
        assert_eq!(stamp.identity(), format!("native:{sha}:V7_9_3"));
    }

    #[test]
    fn auto_with_a_worker_that_fails_its_ping_falls_back_and_says_why() {
        let dir = tempfile::tempdir().unwrap();
        for worker in [
            broken_worker(dir.path()),
            Worker::new(dir.path().join("not-there")),
        ] {
            let path = worker.program().display().to_string();
            let active = resolve(CadReader::Auto, Some(worker)).unwrap();
            assert_eq!(active.stamp().reader, "python");
            let why = active.stamp().native_unavailable.clone().unwrap();
            assert!(why.contains(&path), "{why}");
        }
    }

    #[test]
    fn native_without_a_worker_says_where_to_put_one() {
        let error = resolve(CadReader::Native, None).unwrap_err().to_string();
        for needle in ["--occt-worker", "FARIS_OCCT_WORKER", "--cad-reader python"] {
            assert!(error.contains(needle), "{error}");
        }
    }

    #[test]
    fn native_with_a_worker_that_fails_its_ping_is_an_error_naming_it() {
        let dir = tempfile::tempdir().unwrap();
        let worker = broken_worker(dir.path());
        let path = worker.program().display().to_string();
        let error = resolve(CadReader::Native, Some(worker))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(&path) && error.contains("cannot load libstdc++"),
            "{error}"
        );
        assert!(error.contains("--cad-reader python"), "{error}");
    }

    #[test]
    fn the_reader_identity_follows_the_worker_binary_and_the_occt_tag() {
        let dir = tempfile::tempdir().unwrap();
        let identity = |name: &str, tag: &str, other: &str| {
            let worker = stub_worker(dir.path(), name, tag, other);
            resolve(CadReader::Native, Some(worker))
                .unwrap()
                .stamp()
                .identity()
        };
        let a = identity("a", "V7_9_3", FAILS);
        assert_eq!(
            a,
            identity("a", "V7_9_3", FAILS),
            "the same binary, the same identity"
        );
        assert_ne!(a, identity("b", "V7_9_3", "exit 6"), "another binary");
        assert_ne!(a, identity("a", "V7_9_4", FAILS), "another OpenCASCADE tag");
        assert_ne!(a, ReaderStamp::python().identity());
    }

    #[test]
    fn reader_labels_round_trip() {
        for reader in [CadReader::Auto, CadReader::Native, CadReader::Python] {
            assert_eq!(CadReader::from_label(reader.label()), Some(reader));
        }
        assert_eq!(CadReader::from_label("occt"), None);
    }
}
