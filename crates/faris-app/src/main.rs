// The desktop executable. The browser build enters through `web::start_web`
// in the library; this target only exists natively.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(not(target_arch = "wasm32"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    faris_app::run_native()
}

#[cfg(target_arch = "wasm32")]
fn main() {}
