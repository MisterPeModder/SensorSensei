# Gateway Board Firmware

See the parent [README.md](../README.md) for more information.

## Debugging

### Enabling TCP dumps

Building and flashing the firmware with the `tcp-debug` feature will enable TCP dumps over `trace`-level logs.

```sh
cargo run --release --features tcp-debug
```

## Information

### Partition table

| Name     | Type | SubType | Offset  | Size               | Encrypted | Safe for config |
| -------- | ---- | ------- | ------- | ------------------ | --------- | --------------- |
| nvs      | data | nvs     | 0x9000  | 0x6000 (24KiB)     | no        | yes             |
| phy_init | data | phy     | 0xf000  | 0x1000 (4KiB)      | no        | no              |
| factory  | app  | factory | 0x10000 | 0x100000 (1024KiB) | no        | no              |
