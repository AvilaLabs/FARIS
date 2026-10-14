#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    faris_fuzz_entries::nuclear_data_audit(data);
});
