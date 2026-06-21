# Vehicle Auxiliary Power — KiCad Project

A modular vehicle electronics project for a **2008 Chevy Cobalt**, adding an
auxiliary battery bank, CAN bus telemetry, keyless entry, power windows, and a
wireless sensor/display network.

---

## Repository Structure

```
hardware/        KiCad schematics and project files
firmware/        Firmware source snippets and project scaffolding
  canbus_hub/    Rust code for the Waveshare ESP32-S3 CAN hub
docs/            Design notes and reference documentation
```

---

## System Architecture

### High-Level Overview

```
┌─────────────────────────┐        Wi-Fi (station)       ┌──────────────────────┐
│  Waveshare ESP32-S3     │ ───────────────────────────► │  GL.iNet Router      │
│  CAN Hub                │                              │  (OpenWrt)           │
│                         │                              │  Mosquitto MQTT      │
│  • TWAI ← OBD2/CAN bus  │                              │  broker :1883        │
│  • Publishes telemetry  │                              └──────────┬───────────┘
│    topics to GL.iNet    │                                         │ publish/subscribe
│  • Firmware is frozen   │                              ┌──────────▼───────────┐
│    once deployed        │                              │  Display / Sensor    │
└─────────────────────────┘                              │  Nodes (any count)   │
                                                         │                      │
┌─────────────────────────┐                              │  • Arduino + OLED    │
│  2008 Chevy Cobalt      │                              │  • ESP32 + TFT gauge │
│  OBD2 / CAN bus         │                              │  • Laptop dashboard  │
│  ISO 15765-4            │                              │  • Phone / tablet    │
│  500 kbaud, 11-bit ID   │                              └──────────────────────┘
└─────────────────────────┘
```

### Design Principles

- **Frozen hub firmware** — the CAN hub only polls OBD2 data and publishes MQTT
  topics. It has no knowledge of displays, layouts, or rendering. It never needs
  recompiling when the UI changes.
- **GL.iNet as broker host** — Mosquitto runs on the GL.iNet router (OpenWrt),
  which acts as the in-car Wi-Fi access point and message broker. The hub connects
  as a plain Wi-Fi client.
- **Decoupled display nodes** — any device that can connect to Wi-Fi and speak
  MQTT can become a display peripheral. Nodes subscribe to only the topics they
  need and render data however they choose.
- **Scalable I2C sensor network** — outboard Arduino/Pico nodes act as I2C
  sub-stations, reading local analog sensors and forwarding data to the hub over
  the shared bus. Up to 3–5 nodes are practical before requiring a TCA9548A
  I2C multiplexer.

---

## Hardware

### Key Components

| Component | Role |
|---|---|
| Waveshare ESP32-S3-RS485-CAN | CAN bus hub, MQTT publisher |
| GL.iNet router | In-car Wi-Fi AP, Mosquitto MQTT broker |
| INA219 / INA226 (I2C) | Auxiliary battery voltage & current sensing |
| Outboard Arduino / Pico nodes | Local analog sensor sub-stations (I2C slaves) |
| SSD1306 OLED or I2C LCD | Dashboard display (attached to display node, not hub) |
| Auxiliary battery bank | Secondary power for electronics |

### CAN Bus Notes

- Protocol: ISO 15765-4, 11-bit IDs, **500 kbps**
- OBD2 request ID: `0x7DF`; ECM response ID: `0x7E8`
- The Waveshare board has an onboard isolated CAN transceiver — no external
  MCP2515/TJA1050 module needed.
- **Disable the onboard 120 Ω termination jumper** when splicing into the
  vehicle's existing network (e.g., via the OBD2 port).

### I2C Pin Mapping (Waveshare JST SH1.0 connector)

| JST Pin | Signal | Use |
|---|---|---|
| 1 | 3V3 | Logic power for I2C peripherals |
| 2 | GND | Shared reference ground |
| 3 | GPIO1 | SDA |
| 4 | GPIO2 | SCL |

---

## MQTT Topics

All values are published as plain UTF-8 strings at ~10 Hz.

| Topic | Type | Description |
|---|---|---|
| `cobalt/engine/rpm` | u16 | Engine RPM (0–16383) |
| `cobalt/engine/coolant_temp` | i8 | Coolant temperature °C |
| `cobalt/engine/speed_kph` | u8 | Vehicle speed km/h |
| `cobalt/battery/aux_voltage_mv` | u16 | Aux battery voltage in millivolts |
| `cobalt/battery/aux_current_ma` | i32 | Aux battery current in milliamps |

---

## Firmware

The hub firmware is written in **Rust** using the `esp-idf-hal` / `esp-idf-svc`
crates (standard library / `std` path — not bare-metal `no_std`).

### Snippets in `firmware/canbus_hub/`

| File | Description |
|---|---|
| `peripherals_init.rs` | TWAI (CAN) and I2C peripheral initialization |
| `telemetry.rs` | `CarTelemetry` struct — fixed data payload sent to broker |
| `mqtt_publisher.rs` | Main publish loop; connects to GL.iNet Mosquitto broker |

### Toolchain Setup

```bash
cargo install espup
espup install
cargo generate esp-rs/esp-idf-template cargo   # select ESP32-S3 and std
```

Flash via USB-C using `espflash` or `probe-rs`.

### GL.iNet Broker Setup (one-time, via SSH)

```bash
opkg update && opkg install mosquitto-nossl
# Edit /etc/mosquitto/mosquitto.conf:
#   listener 1883
#   allow_anonymous true
/etc/init.d/mosquitto enable
/etc/init.d/mosquitto start
```

Default GL.iNet LAN IP: `192.168.8.1`

---

## KiCad Schematics

All schematics are in `hardware/`. Open `vehicle_aux_power.kicad_pro` in KiCad
to load the full project.

Sub-sheets:
- `vehicle_aux_power.kicad_sch` — top-level sheet
- `CAN_bus_hub.kicad_sch`
- `Keyless_Entry_Subsystem.kicad_sch`
- `Power_Window_Subsystem.kicad_sch`
- `low_voltage_cutoff.kicad_sch`
- `solar_charge_controller.kicad_sch`
- `meshtastic_node.kicad_sch`
- `GL_iNet_router.kicad_sch`
- `12V_Relay_Module.kicad_sch`

---

## Reference

See [docs/canbus_hub_design_discussion.md](docs/canbus_hub_design_discussion.md)
for the original AI-assisted design conversation that informed this architecture.
