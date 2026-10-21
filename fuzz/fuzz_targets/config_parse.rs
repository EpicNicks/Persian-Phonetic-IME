#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| pime_fuzz::check_config(data));
