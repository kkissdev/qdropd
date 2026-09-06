# qdrop fuzz targets

Excluded from the main workspace. Run with [`cargo-fuzz`](https://github.com/rust-fuzz/cargo-fuzz)
(nightly):

```bash
cargo install cargo-fuzz
cargo +nightly fuzz run frame     -- -max_total_time=60
cargo +nightly fuzz run message   -- -max_total_time=60
```

- **`frame`** — arbitrary bytes into `qdrop_core::frame::read_message`. Must
  always return `Ok`/`Err`, never panic, never allocate on an oversized length
  prefix.
- **`message`** — arbitrary bytes decoded as a msgpack `Message`; any value
  that decodes must round-trip through re-encode/re-decode.

CI runs both for 60 s on every push (`fuzz` job). A stable-Rust regression of
the same idea (`frame::tests::arbitrary_bytes_never_panic`) runs in the normal
test job too.
