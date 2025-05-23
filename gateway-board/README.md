# Gateway Board Firmware

See the parent [README.md](../README.md) for more information.

## Debugging

### Enabling TCP dumps

Building and flashing the firmware with the `tcp-debug` feature will enable TCP dumps over `trace`-level logs.

```sh
cargo run --release --features tcp-debug
```
