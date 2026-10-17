#![no_main]
// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Avila Labs

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    faris_fuzz_entries::proc_and_config_text(data);
});
