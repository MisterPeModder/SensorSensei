# Sensor Sensei

[Link to Working repository](https://github.com/MisterPeModder/T-IOT-902)  
[Link to Epitech repository](https://github.com/EpitechMscProPromo2025/T-IOT-902-NAN_10)

## Initial Setup

1. **Install PlatformIO**: [installation steps](https://platformio.org/install)
2. **Install Rust and Cargo**: Make sure you have Rust and Cargo installed on your machine. You can install them by following the instructions at [rust-lang.org](https://www.rust-lang.org/tools/install).
3. **Install Rust flashing tools**:

```shell
cargo install espup cargo-espflash espflash
espup install
```

## Building

### Gateway and Sensor Boards

Both boards are using cargo and can be built/runned with the same commands. The only difference being in the appropriate directory.

Build:

```shell
source ~/export-esp.sh
cd gateway-board
# or cd sensor-board
cargo build --release
```

Build and flash:

```shell
source ~/export-esp.sh
cd gateway-board
# or cd sensor-board
cargo run --release
```

## Simulating (Gateway Board)

You may use Wokwi to simulate the Gateway Board. To do so, follow these steps:

1. Install the Wokwi VSCode extension.
2. (Re)-build the Gateway Board (see above).
3. Open the `gateway-board/diagram.json` file in VSCode.
4. Click on the "Start the simulation" button
