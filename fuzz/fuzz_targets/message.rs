#![no_main]
//! Decode arbitrary bytes as a msgpack `Message`. Must never panic; on success,
//! re-encoding and re-decoding must round-trip.

use libfuzzer_sys::fuzz_target;
use qdrop_core::proto::Message;

fuzz_target!(|data: &[u8]| {
    if let Ok(msg) = rmp_serde::from_slice::<Message>(data) {
        let bytes = rmp_serde::to_vec_named(&msg).expect("re-encode");
        let back: Message = rmp_serde::from_slice(&bytes).expect("re-decode");
        assert_eq!(msg, back);
    }
});
