# winkeyer-rs

Async Rust library for K1EL WinKeyer 3 / WK3 Morse CW keyers.

## Example CLI

```sh
cargo run --example send_stdin -- --port /dev/ttyUSB0 --wpm 20 < message.txt
```

The library opens WK3 at its default serial settings: 1200 baud, 8N2, no parity, no flow control. It sends `Admin:Host Open`, waits for the revision byte, then exposes helpers such as `set_wpm`, `send_text`, `status`, `wait_until_idle`, and `close`.

Call `close()` before program exit so WK3 leaves host mode and restores standalone settings.
