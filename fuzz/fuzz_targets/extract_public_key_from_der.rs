#![no_main]

use libfuzzer_sys::fuzz_target;
use openzeppelin_relayer::utils::der::extract_public_key_from_der;

fuzz_target!(|data: &[u8]| {
    let _ = extract_public_key_from_der(data);
});
