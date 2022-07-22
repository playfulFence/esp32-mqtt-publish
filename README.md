# ESP32C3-RUST-BOARD MQTT Publish :crab:

This project is created for my [esp-clock](https://github.com/playfulFence/esp-clock) project. It has an option that requires measurements data to be received via **MQTT**.

So this project is MQTT publisher, which takes data from [RUST-BOARD's](https://github.com/esp-rs/esp-rust-board) sensors and publishes it MQTT-topic

> Both projects are dedicated to it, but you can easily adapt this code for your needs :wink:

<br>

## Short guide

1. Set all of "credentials" and the **topic name** in **`cfg.toml`** file.
2. Flash the code with [espflash](https://github.com/esp-rs/espflash/tree/master/espflash) and choose corresponding **serial port**
```bash
cargo espflash --release --monitor 
```
