#![no_main]
//! Feed arbitrary bytes to the length-prefixed frame reader. It must always
//! return `Ok` or `Err`, never panic, and never allocate on an oversized
//! length prefix.

use libfuzzer_sys::fuzz_target;
use qdrop_core::frame::read_message;

fuzz_target!(|data: &[u8]| {
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    rt.block_on(async {
        let mut cursor = std::io::Cursor::new(data);
        // Drain the whole stream; each call consumes one frame or errors.
        for _ in 0..64 {
            match read_message(&mut cursor).await {
                Ok(_) => {}
                Err(_) => break,
            }
        }
    });
});
