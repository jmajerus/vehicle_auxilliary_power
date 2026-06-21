// MQTT publisher loop for the Waveshare CAN hub
//
// Architecture: "headless hub" — passive CAN stream processing
// ─────────────────────────────────────────────────────────────
//  Waveshare ESP32-S3 (CAN hub)
//    └─ Wi-Fi client (station mode) ──► GL.iNet router (OpenWrt)
//                                           └─ Mosquitto MQTT broker
//                                                ├─ Display node A  (OLED)
//                                                ├─ Display node B  (TFT gauge)
//                                                └─ Laptop / phone  (MQTT Explorer)
//
// Primary data path — passive stream processing
// ─────────────────────────────────────────────
// The can_rx task never sends any requests. It sits in a tight receive loop,
// dispatching every frame the bus produces to decode_frame(). On a live
// automotive CAN network the ECM, ABS module, BCM, HVAC controller, and other
// nodes all broadcast their state continuously — typically at 10 to 100 Hz per
// frame ID. This gives you far higher resolution and lower latency than the
// OBD2 request/response cycle, and it exposes vehicle-specific parameters that
// have no standard OBD2 PID mapping at all (e.g. individual wheel speeds, door
// ajar bits, HVAC set-points, window motor current).
//
// To add new data points:
//   1. Sniff the bus with SavvyCAN or similar (OBD2 port, passive mode).
//   2. Identify the frame ID and byte positions for the value you want.
//   3. Add a match arm to decode_frame() — no other code changes needed.
//   4. Add a publish line in the publish loop if you want it in MQTT.
//
// Secondary data path — OBD2 request/response (optional)
// ───────────────────────────────────────────────────────
// A small number of parameters (e.g. calculated load, long-term fuel trim)
// are only available by asking for them. The optional obd2_requester() helper
// fires periodic requests at a low rate (≈ 1 Hz) for any PID not available
// as a native broadcast. It is a separate concern from the receive path.
//
// MQTT topic layout (prefix is configurable — change MQTT_TOPIC_PREFIX below):
//   vehicle/engine/rpm              — u16, raw RPM
//   vehicle/engine/coolant_temp     — i8,  degrees C
//   vehicle/engine/speed_kph        — u8,  km/h
//   vehicle/battery/aux_voltage_mv  — u16, millivolts (e.g. 13400 = 13.4 V)
//   vehicle/battery/aux_current_ma  — i32, milliamps  (negative = discharging)
//
//   Vehicle-specific topics can be added freely — display nodes subscribe only
//   to the topics they care about, so adding new ones is non-breaking.
//
// GL.iNet broker setup (one-time, via SSH):
//   opkg update && opkg install mosquitto-nossl
//   # Edit /etc/mosquitto/mosquitto.conf — listener 1883, allow_anonymous true
//   /etc/init.d/mosquitto enable && /etc/init.d/mosquitto start
//
// Cargo.toml dependencies:
//   esp-idf-hal = "0.43"
//   esp-idf-svc = "0.48"
//   anyhow      = "1"

use crate::telemetry::CarTelemetry;   // see telemetry.rs — derives Clone, Default
use esp_idf_svc::mqtt::client::{EspMqttClient, MqttClientConfiguration, QoS};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::thread;

// ── Software allowlist (second line of defence after hardware filter) ─────────
//
// The hardware TWAI filter in peripherals_init.rs provides coarse rejection at
// zero CPU cost. This allowlist is the fine-grained complement: it gates every
// individual frame ID before any decode logic runs, keeping the hot receive
// loop fast even when the hardware filter admits a broad ID range.
//
// Design rules:
//   • List only IDs (or ID ranges) you actively decode in decode_frame().
//   • Add a new arm here whenever you add a match arm in decode_frame().
//   • Prefer specific IDs over ranges — ranges should only cover broadcast
//     groups where you genuinely intend to decode every member (e.g. all
//     OBD2 ECU responses 0x7E8–0x7EF for multi-ECU vehicles).
//   • Comment each arm with the system it represents so future you knows
//     what breaks if you remove it.
//
// Systems explicitly excluded (do not add without a decode arm to match):
//   0x500–0x5FF  Infotainment bus (radio, XM, HMI events)
//   0x600–0x6FF  Climate control UI frames
//   0x700–0x77E  Network management / node wakeup frames
//   0x7DF        Our own OBD2 requests — no need to hear our own transmissions
//
#[inline(always)]
fn is_relevant(id: u32) -> bool {
    matches!(id,
        // ── OBD2 ECU responses ─────────────────────────────────────────
        0x7E8           // primary ECM response (standard single-ECU vehicles)
        // | 0x7E9..=0x7EF  // additional ECU responses — uncomment if multi-ECU

        // ── 2008 Cobalt native broadcast frames (fill in after sniffing) ──
        // Uncomment each ID range as you confirm it in SavvyCAN.
        // Until an ID is confirmed, leave it commented — unknown frames
        // consume decode effort for no benefit.
        //
        // | 0x0C9   // ECM: engine RPM + throttle
        // | 0x3E9   // ECM: vehicle speed
        // | 0x1A1   // ECM: coolant temp + engine load
        // | 0x2C1   // BCM: door ajar, window motor current
        // | 0x4D1   // ABS: individual wheel speeds
    )
}

// ── Frame dispatcher ──────────────────────────────────────────────────────────
// Called for every frame received off the bus, regardless of source.
//
// Two categories of frame are handled here:
//
//   Vehicle-specific broadcast frames (primary path)
//   ─────────────────────────────────────────────────
//   The ECU and other modules transmit these on their own schedule. No request
//   is ever sent. Frame IDs and byte layouts are specific to the make/model/year.
//   Use SavvyCAN to discover them: connect in listen-only mode while the engine
//   runs and correlate value changes with known sensor readings.
//   Example tool: https://www.savvycan.com/
//
//   OBD2 response frames (secondary path, ID 0x7E8)
//   ────────────────────────────────────────────────
//   Arrive only when obd2_requester() has sent a request. The decoding is
//   standard across all OBD2-compliant vehicles (SAE J1979 service 0x01).
//   Only needed for parameters not available as native broadcasts.
fn decode_frame(id: u32, data: &[u8], telem: &mut CarTelemetry) {
    match id {

        // ── 2008 Cobalt — native broadcast frames (TODO: fill in after sniffing) ──
        //
        // Step 1: Connect a USB-CAN adapter (or the Waveshare board itself in
        //         passive mode) and capture frames with SavvyCAN while the engine
        //         idles, then accelerates.
        // Step 2: Watch for IDs whose byte values correlate with RPM/speed/temp
        //         changes. The Cobalt's GMLAN (single-wire CAN variant) has IDs
        //         typically in the 0x100–0x4FF range for high-speed bus traffic.
        // Step 3: Replace the example patterns below with real values.
        //
        // Example pattern (byte positions are illustrative — verify with sniffer):
        //
        // 0x0C9 if data.len() >= 4 => {
        //     // Engine RPM, big-endian u16 in bytes 2–3, raw / 4
        //     telem.engine_rpm = (((data[2] as u16) << 8) | data[3] as u16) / 4;
        // }
        // 0x3E9 if data.len() >= 2 => {
        //     // Vehicle speed, single byte, km/h
        //     telem.vehicle_speed_kph = data[1];
        // }
        // 0x1A1 if data.len() >= 3 => {
        //     // Coolant temp in raw ECU counts: (A - 40) °C
        //     telem.coolant_temp_c = (data[2] as i16 - 40) as i8;
        // }
        //
        // Vehicle-specific extras (no OBD2 PID equivalent):
        //
        // 0x2C1 if data.len() >= 2 => {
        //     // Example: throttle position, battery charge state flag, etc.
        //     // Publish as a new MQTT topic — display nodes opt-in by subscribing.
        // }

        // ── OBD2 positive response (secondary path) ───────────────────────────
        // Only arrives when obd2_requester() has sent a service 0x01 request.
        // data layout: [PCI, 0x41, PID, byte_A, byte_B?, ...]
        // Formulae are SAE J1979 service 0x01 standard.
        0x7E8 if data.len() >= 4 && data[1] == 0x41 => {
            let a = data[3];
            match data[2] {
                // ── Engine performance ────────────────────────────────────
                0x0C if data.len() >= 5 => {
                    // RPM = (A * 256 + B) / 4
                    telem.engine_rpm = (((a as u16) << 8) | data[4] as u16) / 4;
                }
                0x0D => {
                    // Speed = A km/h
                    telem.vehicle_speed_kph = a;
                }
                0x04 => {
                    // Engine load = A * 100 / 255 %
                    telem.engine_load_pct = (a as u16 * 100 / 255) as u8;
                }
                0x43 if data.len() >= 5 => {
                    // Absolute load = (A*256+B) * 100 / 255 %  (saturate at 100)
                    let raw = (a as u32 * 256 + data[4] as u32) * 100 / 255;
                    telem.abs_load_pct = raw.min(100) as u8;
                }
                0x1F if data.len() >= 5 => {
                    // Engine runtime = A * 256 + B  seconds
                    telem.engine_runtime_s = ((a as u16) << 8) | data[4] as u16;
                }
                0x10 if data.len() >= 5 => {
                    // MAF raw = A * 256 + B  (units: 0.01 g/s)
                    telem.maf_rate_raw = ((a as u16) << 8) | data[4] as u16;
                }
                0x5E if data.len() >= 5 => {
                    // Fuel rate raw = A * 256 + B  (units: 0.05 L/h)
                    telem.fuel_rate_raw = ((a as u16) << 8) | data[4] as u16;
                }
                0x42 if data.len() >= 5 => {
                    // Module voltage = A * 256 + B  millivolts
                    telem.module_voltage_mv = ((a as u16) << 8) | data[4] as u16;
                }

                // ── Throttle / pedal ──────────────────────────────────────
                0x11 => {
                    telem.throttle_pct = (a as u16 * 100 / 255) as u8;
                }
                0x45 => {
                    telem.throttle_rel_pct = (a as u16 * 100 / 255) as u8;
                }
                0x4C => {
                    telem.throttle_cmd_pct = (a as u16 * 100 / 255) as u8;
                }
                0x49 => {
                    telem.accel_pedal_d_pct = (a as u16 * 100 / 255) as u8;
                }
                0x4A => {
                    telem.accel_pedal_e_pct = (a as u16 * 100 / 255) as u8;
                }

                // ── Temperature ───────────────────────────────────────────
                0x05 => {
                    // Coolant temp = A - 40 °C
                    telem.coolant_temp_c = (a as i16 - 40) as i8;
                }
                0x0F => {
                    // Intake air temp = A - 40 °C
                    telem.intake_air_temp_c = (a as i16 - 40) as i8;
                }
                0x46 => {
                    // Ambient temp = A - 40 °C
                    telem.ambient_air_temp_c = (a as i16 - 40) as i8;
                }
                0x5C => {
                    // Oil temp = A - 40 °C
                    telem.oil_temp_c = (a as i16 - 40) as i8;
                }

                // ── Air / fuel ────────────────────────────────────────────
                0x0B => {
                    // Intake MAP = A kPa
                    telem.intake_map_kpa = a;
                }
                0x33 => {
                    // Barometric pressure = A kPa
                    telem.baro_kpa = a;
                }
                0x0E => {
                    // Timing advance = A/2 - 64  degrees BTDC
                    telem.timing_advance_deg = ((a / 2) as i16 - 64) as i8;
                }
                0x06 => {
                    // STFT bank 1 = (A - 128) * 100 / 128  %
                    telem.fuel_trim_short_b1 = ((a as i16 - 128) * 100 / 128) as i8;
                }
                0x07 => {
                    telem.fuel_trim_long_b1 = ((a as i16 - 128) * 100 / 128) as i8;
                }
                0x08 => {
                    telem.fuel_trim_short_b2 = ((a as i16 - 128) * 100 / 128) as i8;
                }
                0x09 => {
                    telem.fuel_trim_long_b2 = ((a as i16 - 128) * 100 / 128) as i8;
                }
                0x2F => {
                    // Fuel tank = A * 100 / 255 %
                    telem.fuel_tank_pct = (a as u16 * 100 / 255) as u8;
                }
                0x0A => {
                    // Fuel pressure = A * 3 kPa gauge  → store /3 to fit u8
                    telem.fuel_pressure_kpa_div3 = a;
                }

                _ => {} // Unsupported PID — ignore
            }
        }

        _ => {} // All other frames: ignore
    }
}

// ── Optional OBD2 requester ───────────────────────────────────────────────────
// Call this from a separate low-priority thread only for PIDs that are NOT
// available as native broadcast frames. Runs at ≈ 1 Hz to avoid bus congestion.
// Delete this function entirely once native broadcast decoding covers all your
// data needs.
//
// To enable: spawn a thread in main() that calls obd2_requester() in a loop,
// sleeping 1 s between rounds.
#[allow(dead_code)]
fn obd2_requester(twai: &mut impl esp_idf_hal::twai::Transmit) {
    // The PIDs to request — only those not covered by native broadcasts.
    // Remove each PID below once you have confirmed a native broadcast that
    // covers it. Start with the most critical (RPM, speed, coolant) and work
    // down the list. A 1 Hz request rate for 20 PIDs adds < 1 % bus load.
    const FALLBACK_PIDS: &[u8] = &[
        // Engine performance
        0x0C, // RPM
        0x0D, // Speed
        0x04, // Engine load
        0x43, // Absolute load
        0x1F, // Engine runtime
        0x10, // MAF rate
        0x5E, // Fuel rate
        0x42, // Module voltage
        // Throttle / pedal
        0x11, // Throttle pos
        0x45, // Rel throttle
        0x4C, // Cmd throttle
        0x49, // Accel pedal D
        0x4A, // Accel pedal E
        // Temperature
        0x05, // Coolant temp
        0x0F, // Intake air temp
        0x46, // Ambient temp
        0x5C, // Oil temp
        // Air / fuel
        0x0B, // Intake MAP
        0x33, // Baro pressure
        0x0E, // Timing advance
        0x06, // STFT bank 1
        0x07, // LTFT bank 1
        0x08, // STFT bank 2
        0x09, // LTFT bank 2
        0x2F, // Fuel tank level
        0x0A, // Fuel pressure
    ];

    for &pid in FALLBACK_PIDS {
        // ISO 15765-4 single-frame service 0x01 request to functional address 0x7DF.
        // The ECU replies on 0x7E8 and decode_frame() handles it.
        use esp_idf_hal::twai::TwaiFrame;
        if let Ok(frame) = TwaiFrame::new_data(
            0x7DF,
            &[0x02, 0x01, pid, 0x00, 0x00, 0x00, 0x00, 0x00],
        ) {
            let _ = twai.transmit(&frame);
        }
        // Brief gap between requests — avoids flooding the bus.
        thread::sleep(Duration::from_millis(10));
    }
}

fn main() -> anyhow::Result<()> {
    // Prerequisites (handled in peripherals_init.rs before reaching here):
    //   • TWAI driver initialised in listen-only mode at 500 kbps
    //     (listen-only = no ACK bits driven, safe for sniffing a live network)
    //   • Wi-Fi connected (station mode) to GL.iNet router

    const MQTT_TOPIC_PREFIX: &str = "vehicle";

    // ── Shared telemetry state ────────────────────────────────────────────────
    // decode_frame() writes fields as frames arrive.
    // The publish loop clones the whole struct — the lock is held for < 1 µs.
    let telemetry: Arc<Mutex<CarTelemetry>> =
        Arc::new(Mutex::new(CarTelemetry::default()));

    // ── CAN receive task (primary data path) ─────────────────────────────────
    // Tight loop — processes frames as fast as the bus produces them.
    // No request phase; no artificial timing. On a busy automotive bus this
    // may dispatch thousands of frames per second.
    let telem_can = Arc::clone(&telemetry);

    thread::Builder::new()
        .name("can_rx".into())
        .stack_size(4096)
        .spawn(move || -> anyhow::Result<()> {
            // TODO: accept the TwaiDriver from peripherals_init.rs.
            // The driver should be configured for listen-only mode so the
            // hub never disturbs the live bus with ACK bits.
            // let mut twai = twai_driver;

            loop {
                // TODO: replace with actual TWAI receive call, e.g.:
                // match twai.receive() {
                //     Ok(frame) => {
                //         // Software allowlist — drop uninteresting IDs before
                //         // any decode work. is_relevant() is #[inline(always)]
                //         // and compiles down to a small set of comparisons.
                //         let id = frame.identifier();
                //         if !is_relevant(id) { continue; }
                //
                //         let mut t = telem_can.lock().unwrap();
                //         decode_frame(id, frame.data(), &mut t);
                //     }
                //     Err(esp_idf_hal::twai::TwaiError::NoMessage) => {
                //         // Nothing on the bus right now — yield without sleeping
                //         // so we stay responsive when traffic resumes.
                //     }
                //     Err(e) => log::warn!("TWAI receive error: {:?}", e),
                // }
                thread::sleep(Duration::from_millis(1)); // placeholder until wired up
            }
        })?;

    // ── MQTT client ───────────────────────────────────────────────────────────
    let mqtt_config = MqttClientConfiguration {
        client_id: Some("canbus_hub"),
        ..Default::default() // port 1883, no TLS — local network only
    };

    let mut client = EspMqttClient::new(
        "mqtt://192.168.8.1:1883",
        &mqtt_config,
        move |_event| {},
    )?;

    // ── Publish loop (10 Hz) ─────────────────────────────────────────────────
    // Completely decoupled from the CAN bus rate. The snapshot below costs one
    // Mutex lock and a struct copy — typically < 1 µs — so it never delays the
    // receive path. Change the sleep duration to adjust the publish rate
    // without touching any other module.
    loop {
        let snapshot = telemetry.lock().unwrap().clone();

        let mut pub_topic = |suffix: &str, val: String| -> anyhow::Result<()> {
            client.publish(
                &format!("{MQTT_TOPIC_PREFIX}/{suffix}"),
                QoS::AtMostOnce,
                false,
                val.as_bytes(),
            )?;
            Ok(())
        };

        // ── Engine performance ────────────────────────────────────────────
        pub_topic("engine/rpm",              format!("{}",     snapshot.engine_rpm))?;
        pub_topic("engine/speed_kph",        format!("{}",     snapshot.vehicle_speed_kph))?;
        pub_topic("engine/load_pct",         format!("{}",     snapshot.engine_load_pct))?;
        pub_topic("engine/abs_load_pct",     format!("{}",     snapshot.abs_load_pct))?;
        pub_topic("engine/runtime_s",        format!("{}",     snapshot.engine_runtime_s))?;
        // MAF raw counts are 0.01 g/s units — publish as g/s float string
        pub_topic("engine/maf_g_s",          format!("{:.2}",  snapshot.maf_rate_raw as f32 / 100.0))?;
        // Fuel rate raw counts are 0.05 L/h units — publish as L/h float string
        pub_topic("engine/fuel_rate_l_h",    format!("{:.2}",  snapshot.fuel_rate_raw as f32 / 20.0))?;
        pub_topic("engine/module_voltage_mv",format!("{}",     snapshot.module_voltage_mv))?;

        // ── Throttle / pedal ─────────────────────────────────────────────
        pub_topic("engine/throttle_pct",     format!("{}",     snapshot.throttle_pct))?;
        pub_topic("engine/throttle_rel_pct", format!("{}",     snapshot.throttle_rel_pct))?;
        pub_topic("engine/throttle_cmd_pct", format!("{}",     snapshot.throttle_cmd_pct))?;
        pub_topic("engine/accel_pedal_d_pct",format!("{}",     snapshot.accel_pedal_d_pct))?;
        pub_topic("engine/accel_pedal_e_pct",format!("{}",     snapshot.accel_pedal_e_pct))?;

        // ── Temperature ──────────────────────────────────────────────────
        pub_topic("engine/coolant_temp_c",   format!("{}",     snapshot.coolant_temp_c))?;
        pub_topic("engine/intake_air_temp_c",format!("{}",     snapshot.intake_air_temp_c))?;
        pub_topic("engine/ambient_temp_c",   format!("{}",     snapshot.ambient_air_temp_c))?;
        pub_topic("engine/oil_temp_c",       format!("{}",     snapshot.oil_temp_c))?;

        // ── Air / fuel ───────────────────────────────────────────────────
        pub_topic("engine/intake_map_kpa",   format!("{}",     snapshot.intake_map_kpa))?;
        pub_topic("engine/baro_kpa",         format!("{}",     snapshot.baro_kpa))?;
        pub_topic("engine/timing_advance_deg",format!("{}",    snapshot.timing_advance_deg))?;
        pub_topic("engine/fuel_trim_short_b1",format!("{}",   snapshot.fuel_trim_short_b1))?;
        pub_topic("engine/fuel_trim_long_b1", format!("{}",   snapshot.fuel_trim_long_b1))?;
        pub_topic("engine/fuel_trim_short_b2",format!("{}",   snapshot.fuel_trim_short_b2))?;
        pub_topic("engine/fuel_trim_long_b2", format!("{}",   snapshot.fuel_trim_long_b2))?;
        pub_topic("engine/fuel_tank_pct",    format!("{}",     snapshot.fuel_tank_pct))?;
        // Fuel pressure stored /3 to fit u8 — multiply back for kPa
        pub_topic("engine/fuel_pressure_kpa",format!("{}",    snapshot.fuel_pressure_kpa_div3 as u16 * 3))?;

        // ── Auxiliary battery (I2C sensor) ───────────────────────────────
        pub_topic("battery/aux_voltage_mv",  format!("{}",     snapshot.aux_battery_mv))?;
        pub_topic("battery/aux_current_ma",  format!("{}",     snapshot.aux_current_ma))?;

        // Vehicle-specific extras go here as native broadcast decoding
        // is filled in above — display nodes subscribe only to what they want.

        thread::sleep(Duration::from_millis(100)); // 10 Hz
    }
}
