# Vehicle Auxiliary-Powered Subsystems

A modular vehicle electronics project providing an auxiliary power infrastructure
and a growing collection of subsystems that run on it: CAN bus telemetry,
keyless entry, power windows, and a wireless sensor/display network.

The hardware and firmware are designed to be **vehicle-agnostic** — compatible
with any OBD2-equipped vehicle (US market 1996 and later). Vehicle-specific
details such as wire colours, connector pinouts, and proprietary CAN IDs are
noted only where they arise.

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
│  Target Vehicle         │                              │  • ESP32 + TFT gauge │
│  OBD2 / CAN bus         │                              │  • Laptop dashboard  │
│  ISO 15765-4            │                              │  • Phone / tablet    │
│  500 kbaud, 11-bit ID   │                              └──────────────────────┘
└─────────────────────────┘
```

### Design Principles

- **Frozen hub firmware** — the CAN hub passively processes the raw CAN bus
  stream and publishes MQTT topics. It has no knowledge of displays, layouts,
  or rendering. Adding a new vehicle-specific parameter requires only a match
  arm in `decode_frame()` and `is_relevant()` — the publish layer and all
  display nodes are unaffected. OBD2 request/response is retained as an
  optional fallback for any parameter the ECU does not broadcast natively.
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

- Protocol: ISO 15765-4, 11-bit IDs, **500 kbps** (standard for OBD2 since 2008;
  some vehicles use 250 kbps — check your service manual)
- OBD2 broadcast request ID: `0x7DF`; typical ECM response ID: `0x7E8`
- The Waveshare board has an onboard isolated CAN transceiver — no external
  MCP2515/TJA1050 module needed.
- **Disable the onboard 120 Ω termination jumper** when splicing into an
  existing active network (e.g., via the OBD2 port).
- Proprietary (non-OBD2) CAN IDs are vehicle-specific and must be reverse-
  engineered or sourced from community databases for your make/model/year.

### CAN Bus Filtering

Frames are rejected at two levels to keep the receive loop fast:

| Level | Where | Mechanism | Cost |
|---|---|---|---|
| **Hardware filter** | TWAI peripheral | Code/mask register pair — frames are dropped before reaching the CPU FIFO | Zero CPU cycles |
| **Software allowlist** | `is_relevant()` in `mqtt_publisher.rs` | Explicit per-ID opt-in before any decode logic runs | A few comparisons per frame |

The hardware filter (configured in `peripherals_init.rs`) handles coarse
volume rejection (e.g. infotainment/radio bursts). The software allowlist is
the fine-grained complement: an ID must appear in `is_relevant()` **and** have
a corresponding arm in `decode_frame()` before any processing happens.
Systems with no decode arm (climate UI events, network management frames,
ABS wheel speed until you add a decode arm) never touch the telemetry struct.

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
The `vehicle/` prefix is configurable via `MQTT_TOPIC_PREFIX` in the firmware.

### Engine Performance

| Topic | Unit | OBD2 PID | Description |
|---|---|---|---|
| `vehicle/engine/rpm` | integer | 0x0C | Engine RPM (0–16 383) |
| `vehicle/engine/speed_kph` | integer | 0x0D | Vehicle speed km/h |
| `vehicle/engine/load_pct` | 0–100 | 0x04 | Calculated engine load % |
| `vehicle/engine/abs_load_pct` | 0–100 | 0x43 | Absolute load % |
| `vehicle/engine/runtime_s` | integer | 0x1F | Run time since engine start (s) |
| `vehicle/engine/maf_g_s` | float | 0x10 | Mass air flow rate (g/s) |
| `vehicle/engine/fuel_rate_l_h` | float | 0x5E | Engine fuel rate (L/h) |
| `vehicle/engine/module_voltage_mv` | integer | 0x42 | ECM supply voltage (mV) |

### Throttle / Pedal

| Topic | Unit | OBD2 PID | Description |
|---|---|---|---|
| `vehicle/engine/throttle_pct` | 0–100 | 0x11 | Absolute throttle position % |
| `vehicle/engine/throttle_rel_pct` | 0–100 | 0x45 | Relative throttle position % |
| `vehicle/engine/throttle_cmd_pct` | 0–100 | 0x4C | Commanded throttle actuator % |
| `vehicle/engine/accel_pedal_d_pct` | 0–100 | 0x49 | Accelerator pedal position D % |
| `vehicle/engine/accel_pedal_e_pct` | 0–100 | 0x4A | Accelerator pedal position E % |

### Temperature

| Topic | Unit | OBD2 PID | Description |
|---|---|---|---|
| `vehicle/engine/coolant_temp_c` | °C | 0x05 | Engine coolant temperature |
| `vehicle/engine/intake_air_temp_c` | °C | 0x0F | Intake air temperature |
| `vehicle/engine/ambient_temp_c` | °C | 0x46 | Ambient air temperature |
| `vehicle/engine/oil_temp_c` | °C | 0x5C | Engine oil temperature |

### Air / Fuel

| Topic | Unit | OBD2 PID | Description |
|---|---|---|---|
| `vehicle/engine/intake_map_kpa` | kPa | 0x0B | Intake manifold absolute pressure |
| `vehicle/engine/baro_kpa` | kPa | 0x33 | Barometric pressure |
| `vehicle/engine/timing_advance_deg` | ° BTDC | 0x0E | Ignition timing advance |
| `vehicle/engine/fuel_trim_short_b1` | % | 0x06 | Short-term fuel trim bank 1 |
| `vehicle/engine/fuel_trim_long_b1` | % | 0x07 | Long-term fuel trim bank 1 |
| `vehicle/engine/fuel_trim_short_b2` | % | 0x08 | Short-term fuel trim bank 2 |
| `vehicle/engine/fuel_trim_long_b2` | % | 0x09 | Long-term fuel trim bank 2 |
| `vehicle/engine/fuel_tank_pct` | 0–100 | 0x2F | Fuel tank level % |
| `vehicle/engine/fuel_pressure_kpa` | kPa | 0x0A | Fuel rail pressure (gauge) |

### Auxiliary Battery (I2C sensor — not OBD2)

| Topic | Unit | Description |
|---|---|---|
| `vehicle/battery/aux_voltage_mv` | mV | Auxiliary battery voltage |
| `vehicle/battery/aux_current_ma` | mA | Auxiliary battery current (negative = discharging) |

Vehicle-specific topics (e.g. wheel speeds, door states, HVAC setpoints)
are added to `decode_frame()` and the publish loop as native broadcast
frame IDs are confirmed via bus sniffing. Display nodes subscribe only
to the topics they need — adding topics is always non-breaking.

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

See [`firmware/glinet/`](firmware/glinet/) for the full setup script and
`mosquitto.conf`. Quick start:

```bash
scp firmware/glinet/setup_mqtt_broker.sh root@192.168.8.1:/tmp/
ssh root@192.168.8.1 ash /tmp/setup_mqtt_broker.sh
```

Default GL.iNet LAN IP: `192.168.8.1`

To monitor all live topics from the router once running:
```bash
mosquitto_sub -h 192.168.8.1 -t '#' -v
```

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

## Roadmap

| Item | Status | Notes |
|---|---|---|
| Fill in native broadcast IDs for target vehicle | Pending | Use SavvyCAN in passive mode; update `decode_frame()` and `is_relevant()` |
| Wire up TWAI driver in `main()` | Pending | Pass `TwaiDriver` from `peripherals_init.rs` into `can_rx` thread |
| Outboard Arduino I2C sensor nodes | Planned | INA219/226 for aux battery; additional analog sensors |
| **Mobile dashboard app** | Future | Android / iOS widget-based dashboard connecting to the in-car MQTT broker over Wi-Fi. Users compose custom gauge layouts from the published topic catalogue — on-par with Torque Pro but talking to this hub instead of a Bluetooth OBD2 dongle. Each widget subscribes to a single MQTT topic; layouts are stored on-device. Cross-platform target: Flutter or React Native. |
| `dashboard-rich.html` using canvas-gauges | Future | Alternate browser dashboard with richer visual gauge styles (needle, gradient fill, tick marks) |

---

## Reference

See [docs/canbus_hub_design_discussion.md](docs/canbus_hub_design_discussion.md)
for the original AI-assisted design conversation that informed this architecture.
