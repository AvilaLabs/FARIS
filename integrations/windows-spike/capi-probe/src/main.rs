//! Runs a model through the OpenMC C API (no openmc.exe) and writes tally 1 as JSON.
//!
//!     capi-probe MODEL_DIR OUT.json
//!
//! Checks three things the FARIS bundle relies on: the exported functions link and run, the
//! exported global `openmc_err_msg` can be read after a failing call, and tally results can be
//! read back through `openmc_tally_results`.

use std::ffi::{c_char, c_int, c_void, CStr, CString};

#[link(name = "libopenmc", kind = "dylib")]
extern "C" {
    fn openmc_init(argc: c_int, argv: *mut *mut c_char, intracomm: *const c_void) -> c_int;
    fn openmc_run() -> c_int;
    fn openmc_finalize() -> c_int;
    fn openmc_get_tally_index(id: i32, index: *mut i32) -> c_int;
    fn openmc_tally_get_n_realizations(index: i32, n: *mut i32) -> c_int;
    fn openmc_tally_results(index: i32, ptr: *mut *mut f64, shape: *mut usize) -> c_int;
    static openmc_err_msg: [c_char; 256];
}

fn err_msg() -> String {
    // SAFETY: openmc_err_msg is a NUL-terminated buffer owned by libopenmc.
    unsafe { CStr::from_ptr(openmc_err_msg.as_ptr()) }.to_string_lossy().into_owned()
}

fn check(code: c_int, what: &str) {
    if code != 0 {
        eprintln!("{what} failed with {code}: {}", err_msg());
        std::process::exit(2);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (model, out) = (&args[1], &args[2]);
    std::env::set_current_dir(model).expect("model directory");

    let prog = CString::new("openmc").unwrap();
    let mut argv = [prog.as_ptr() as *mut c_char, std::ptr::null_mut()];
    // SAFETY: argv outlives the call; a null communicator means no MPI.
    check(unsafe { openmc_init(1, argv.as_mut_ptr(), std::ptr::null()) }, "openmc_init");

    // A failing call must leave a readable message in the exported buffer.
    let mut missing = 0i32;
    let code = unsafe { openmc_get_tally_index(9999, &mut missing) };
    println!("probe: openmc_get_tally_index(9999) returned {code}, message: {}", err_msg());
    if code == 0 {
        eprintln!("expected an error for a tally id that does not exist");
        std::process::exit(3);
    }

    check(unsafe { openmc_run() }, "openmc_run");

    let mut idx = 0i32;
    check(unsafe { openmc_get_tally_index(1, &mut idx) }, "openmc_get_tally_index(1)");
    let mut n = 0i32;
    check(unsafe { openmc_tally_get_n_realizations(idx, &mut n) }, "openmc_tally_get_n_realizations");
    let mut ptr: *mut f64 = std::ptr::null_mut();
    let mut shape = [0usize; 3];
    check(unsafe { openmc_tally_results(idx, &mut ptr, shape.as_mut_ptr()) }, "openmc_tally_results");
    let (nf, ns, nr) = (shape[0], shape[1], shape[2]);
    println!("probe: tally 1 shape {nf} x {ns} x {nr}, {n} realizations");
    // SAFETY: libopenmc owns nf*ns*nr doubles at ptr until finalize.
    let data = unsafe { std::slice::from_raw_parts(ptr, nf * ns * nr) };

    let nn = n as f64;
    let mut means = Vec::new();
    let mut stds = Vec::new();
    for f in 0..nf {
        let (mut m, mut s) = (Vec::new(), Vec::new());
        for k in 0..ns {
            let base = (f * ns + k) * nr;
            let (sum, sum_sq) = (data[base + 1], data[base + 2]);
            let mean = sum / nn;
            let var = ((sum_sq / nn - mean * mean) / (nn - 1.0)).max(0.0);
            m.push(format!("{mean:e}"));
            s.push(format!("{:e}", var.sqrt()));
        }
        means.push(format!("[{}]", m.join(",")));
        stds.push(format!("[{}]", s.join(",")));
    }
    let json = format!(
        "{{\"version\":\"0.15.3 via C API\",\"tallies\":{{\"tally 1\":{{\"mean\":[{}],\"std\":[{}]}}}}}}",
        means.join(","),
        stds.join(",")
    );
    std::fs::write(out, json).expect("write json");
    check(unsafe { openmc_finalize() }, "openmc_finalize");
    println!("probe: ok");
}
