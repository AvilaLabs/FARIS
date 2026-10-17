#![no_main]
// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Avila Labs

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    faris_fuzz_entries::facets(data);
});
