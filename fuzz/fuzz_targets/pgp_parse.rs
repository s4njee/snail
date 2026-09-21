#![no_main]

use libfuzzer_sys::fuzz_target;
use snail_core::pgp::KeyRing;

fuzz_target!(|data: &[u8]| {
    let ring = KeyRing::new();
    let _ = ring.verify_message(data, std::time::SystemTime::now());
    let _ = ring.decrypt(data, "fuzz-passphrase");
    let _ = snail_core::pgp_mime::envelope(data);
});
