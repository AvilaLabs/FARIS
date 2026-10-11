//! Builds OpenCASCADE (OCCT) from a pinned official source release as static libraries, then
//! compiles the C++ shim against it with the `cxx` crate.
//!
//! The source is the GitHub release tarball named below. Its sha256 is checked before anything
//! is unpacked; a different file stops the build.
//!
//! Environment (all optional):
//!
//! * `FARIS_OCCT_PREFIX`  where the static OCCT is installed and cached (default: inside the
//!   cargo output directory). A stamp file records the tarball hash and the CMake
//!   configuration, and a matching stamp means OCCT is not built again.
//! * `FARIS_OCCT_TARBALL` a local copy of the release tarball (default: downloaded with curl
//!   into `<prefix>.src`).
//! * `FARIS_CMAKE`        the cmake program (default: `cmake` on the path).
//!
//! The OCCT compile uses at most three parallel jobs and never writes to /tmp.

use sha2::{Digest, Sha256};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

const OCCT_TAG: &str = "V7_9_3";
const OCCT_URL: &str = "https://github.com/Open-Cascade-SAS/OCCT/archive/refs/tags/V7_9_3.tar.gz";
const OCCT_SHA256: &str = "5ecf094ec6b12d5413dfb851d8c3590c354058aee556e32e408bdfbf8c357d57";
/// Top-level directory inside the tarball.
const OCCT_TOP_DIR: &str = "OCCT-7_9_3";
const MAX_JOBS: usize = 3;

/// Toolkits asked for. CMake adds their dependencies (the XCAF document model pulls in
/// TKService, TKV3d, TKHLR and TKVCAF; none of them needs OpenGL, X11 or FreeType here).
const ROOT_TOOLKITS: &[&str] = &[
    "TKDESTEP",
    "TKXCAF",
    "TKBool",
    "TKMesh",
    "TKPrim",
    "TKTopAlgo",
];

/// Static libraries to link, dependants before the libraries they use.
const LINK_ORDER: &[&str] = &[
    "TKBool",
    "TKDESTEP",
    "TKXSBase",
    "TKXCAF",
    "TKVCAF",
    "TKV3d",
    "TKMesh",
    "TKHLR",
    "TKService",
    "TKDE",
    "TKCAF",
    "TKLCAF",
    "TKCDF",
    "TKBO",
    "TKShHealing",
    "TKPrim",
    "TKTopAlgo",
    "TKGeomAlgo",
    "TKBRep",
    "TKGeomBase",
    "TKG3d",
    "TKG2d",
    "TKMath",
    "TKernel",
];

fn main() {
    for variable in ["FARIS_OCCT_PREFIX", "FARIS_OCCT_TARBALL", "FARIS_CMAKE"] {
        println!("cargo:rerun-if-env-changed={variable}");
    }
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/ffi.rs");
    println!("cargo:rerun-if-changed=cpp/shim.h");
    println!("cargo:rerun-if-changed=cpp/shim.cc");

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let prefix = env::var_os("FARIS_OCCT_PREFIX")
        .map(PathBuf::from)
        .unwrap_or_else(|| out_dir.join("occt"));
    let args = cmake_arguments(&prefix);
    let stamp = format!(
        "occt={OCCT_TAG}\nsha256={OCCT_SHA256}\nconfiguration={}\n",
        hex(&Sha256::digest(args.join("\n").as_bytes()))
    );
    let stamp_path = prefix.join("faris-occt.stamp");
    if fs::read_to_string(&stamp_path).ok().as_deref() != Some(stamp.as_str()) {
        build_occt(&prefix, &args);
        fs::write(&stamp_path, &stamp).expect("write the OCCT stamp");
    }

    let include = prefix.join("include").join("opencascade");
    let lib = prefix.join("lib");
    assert!(
        include.join("Standard_Version.hxx").is_file(),
        "{} has no OCCT headers",
        include.display()
    );
    cxx_build::bridge("src/ffi.rs")
        .file("cpp/shim.cc")
        .include(&include)
        .std("c++17")
        .opt_level(2)
        .flag_if_supported("-w")
        .compile("faris_occt_shim");
    println!("cargo:rustc-link-search=native={}", lib.display());
    for name in LINK_ORDER {
        assert!(
            lib.join(format!("lib{name}.a")).is_file(),
            "static library lib{name}.a is missing from {}",
            lib.display()
        );
        println!("cargo:rustc-link-lib=static={name}");
    }
    println!("cargo:rustc-link-lib=dylib=pthread");
    println!("cargo:rustc-link-lib=dylib=dl");
    println!("cargo:rustc-link-lib=dylib=rt");
    println!("cargo:rustc-env=FARIS_OCCT_TAG={OCCT_TAG}");
    println!("cargo:rustc-env=FARIS_OCCT_SHA256={OCCT_SHA256}");
}

fn cmake_arguments(prefix: &Path) -> Vec<String> {
    let mut args = vec![
        "-DCMAKE_BUILD_TYPE=Release".to_string(),
        format!("-DCMAKE_INSTALL_PREFIX={}", prefix.display()),
        // CMake 4 refuses projects that declare an old minimum version unless told otherwise.
        "-DCMAKE_POLICY_VERSION_MINIMUM=3.5".into(),
        "-DCMAKE_POSITION_INDEPENDENT_CODE=ON".into(),
        "-DBUILD_LIBRARY_TYPE=Static".into(),
        "-DINSTALL_DIR_LAYOUT=Unix".into(),
        "-DBUILD_USE_PCH=OFF".into(),
        // Keep OpenCASCADE's argument and range checks: a bad input becomes an exception the
        // shim reports instead of undefined behaviour.
        "-DBUILD_RELEASE_DISABLE_EXCEPTIONS=OFF".into(),
        format!("-DBUILD_ADDITIONAL_TOOLKITS={}", ROOT_TOOLKITS.join(";")),
    ];
    for module in [
        "FoundationClasses",
        "ModelingData",
        "ModelingAlgorithms",
        "Visualization",
        "ApplicationFramework",
        "DataExchange",
        "Draw",
    ] {
        args.push(format!("-DBUILD_MODULE_{module}=OFF"));
    }
    for option in [
        "USE_TK",
        "USE_FREETYPE",
        "USE_FREEIMAGE",
        "USE_FFMPEG",
        "USE_OPENVR",
        "USE_RAPIDJSON",
        "USE_DRACO",
        "USE_TBB",
        "USE_EIGEN",
        "USE_VTK",
        "USE_XLIB",
        "USE_GLES2",
        "BUILD_DOC_Overview",
        "BUILD_Inspector",
        "BUILD_SAMPLES_QT",
        "INSTALL_SAMPLES",
        "INSTALL_TEST_CASES",
        "INSTALL_DOC_Overview",
    ] {
        args.push(format!("-D{option}=OFF"));
    }
    args
}

fn build_occt(prefix: &Path, args: &[String]) {
    let work = PathBuf::from(format!("{}.src", prefix.display()));
    let build = PathBuf::from(format!("{}.build", prefix.display()));
    fs::create_dir_all(&work).expect("create the OCCT work directory");
    let tarball = match env::var_os("FARIS_OCCT_TARBALL") {
        Some(path) => PathBuf::from(path),
        None => {
            let path = work.join(format!("{OCCT_TAG}.tar.gz"));
            if !path.is_file() || file_sha256(&path) != OCCT_SHA256 {
                run(
                    Command::new("curl")
                        .args(["-fsSL", "-o"])
                        .arg(&path)
                        .arg(OCCT_URL),
                    "download OCCT",
                );
            }
            path
        }
    };
    let found = file_sha256(&tarball);
    assert!(
        found == OCCT_SHA256,
        "{} has sha256 {found}, but this build pins {OCCT_SHA256} ({OCCT_URL})",
        tarball.display()
    );
    let source = work.join(OCCT_TOP_DIR);
    if !source.join("CMakeLists.txt").is_file() {
        run(
            Command::new("tar")
                .arg("xzf")
                .arg(&tarball)
                .arg("-C")
                .arg(&work),
            "unpack OCCT",
        );
    }
    let cmake = env::var_os("FARIS_CMAKE").unwrap_or_else(|| "cmake".into());
    let jobs = env::var("NUM_JOBS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(MAX_JOBS)
        .clamp(1, MAX_JOBS);
    let mut configure = Command::new(&cmake);
    configure.arg("-S").arg(&source).arg("-B").arg(&build);
    if Command::new("ninja").arg("--version").output().is_ok() {
        configure.args(["-G", "Ninja"]);
    }
    configure.args(args);
    run(&mut configure, "configure OCCT");
    let mut compile = Command::new(&cmake);
    compile
        .arg("--build")
        .arg(&build)
        .arg("--parallel")
        .arg(jobs.to_string())
        .env("CMAKE_BUILD_PARALLEL_LEVEL", jobs.to_string());
    run(&mut compile, "build OCCT");
    run(
        Command::new(&cmake).arg("--install").arg(&build),
        "install OCCT",
    );
    // The build tree is large; the install is what is cached.
    let _ = fs::remove_dir_all(&build);
}

fn run(command: &mut Command, what: &str) {
    eprintln!("faris-occt build: {what}");
    let status = command
        .status()
        .unwrap_or_else(|error| panic!("could not {what}: {error}"));
    assert!(status.success(), "could not {what}: {status}");
}

fn file_sha256(path: &Path) -> String {
    let mut digest = Sha256::new();
    let mut file = fs::File::open(path).unwrap_or_else(|e| panic!("open {}: {e}", path.display()));
    std::io::copy(&mut file, &mut digest).expect("hash the tarball");
    hex(&digest.finalize())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
