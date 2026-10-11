// Links the import library libopenmc.lib from OPENMC_LIB_DIR.
fn main() {
    let dir = std::env::var("OPENMC_LIB_DIR").expect("OPENMC_LIB_DIR must point at the folder holding libopenmc.lib");
    println!("cargo:rustc-link-search=native={dir}");
    println!("cargo:rerun-if-env-changed=OPENMC_LIB_DIR");
}
