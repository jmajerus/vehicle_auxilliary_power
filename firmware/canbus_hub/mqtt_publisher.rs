// MQTT publisher loop for the Waveshare CAN hub
//
// Architecture: "headless hub" — stream-based CAN receive
// ────────────────────────────────────────────────────────
//  Waveshare ESP32-S3 (CAN hub)
//    └─ Wi-Fi client (station mode) ──► GL.iNet router (OpenWrt)
//                                           └─ Mosquitto MQTT broker
//                                                ├─ Display node A  (OLED)
//                                                ├─ Display node B  (TFT gauge)
//                                                └─ Laptop / phone  (MQTT Explorer)
//
// The hub never polls PIDs one-at-a-time. Instead it runs two decoupled tasks:
//
//   can_rx task  — responds to the CAN bus stream as frames arrive.
//                  For OBD2 PIDs it scatters all requests upfront, then drains
//                  the receive buffer; the ECU responds asynchronously so no
//                  blocking wait-for-one-response-before-asking-next is needed.
//                  For native broadcast frames the ECU transmits them without
//                  being asked at all — the task just receives and decodes.
//
//   publish loop — reads a snapshot of the shared CarTelemetry state every
//                  100 ms and publishes all MQTT topics. The publish rate is
//                  completely decoupled from the CAN bus rate. Adding new
//                  display nodes or changing the publish interval never
//                  requires recompiling or reflashing this hub firmware.
//
// GL.iNet broker setup (one-time, via SSH):
//   opkg update && opkg install mosquitto-nossl
//   # Edit /etc/mosquitto/mosquitto.conf — listener 1883, allow_anonymous true
//   /etc/init.d/mosquitto enable && /etc/init.d/mosquitto start
//
// MQTT topic layout (prefix is configurable — change MQTT_TOPIC_PREFIX below):
//   vehicle/engine/rpm              — u16, raw RPM
//   vehicle/engine/coolant_temp     — i8,  degrees C
//   vehicle/engine/speed_kph        — u8,  km/h
//   vehicle/battery/aux_voltage_mv  — u16, millivolts (e.g. 13400 = 13.4 V)
//   vehicle/battery/aux_current_ma  — i32, milliamps  (negative = discharging)
//
// Cargo.toml dependencies:
//   esp-idf-hal = "0.43"
//   esp-idf-svc = "0.48"
//   anyhow      = "1"

use crate::telemetry::CarTelemetry;   // see telemetry.rs — add #[derive(Clone, Default)]
use esp_idf_svc::mqtt::client::{EspMqttClient, MqttClientConfiguration, QoS};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use std::thread;

// ── OBD2 PID constants (SAE J1979 service 0x01) ──────────────────────────────
// Add further PIDs here as you extend the telemetry struct.
const OBD2_PID_RPM:          u8 = 0x0C; // (A*256 + B) / 4  → RPM
const OBD2_PID_COOLANT_TEMP: u8 = 0x05; // A - 40            → °C
const OBD2_PID_SPEED:        u8 = 0x0D; // A                 → km/h

// ── CAN frame decoder ─────────────────────────────────────────────────────────
// Maps an incoming CAN frame to the relevant field(s) in CarTelemetry.
//
// Two frame sources are handled:
//
//   OBD2 responses (ID 0x7E8, service byte 0x41):
//     Sent by the ECU in reply to our service 0x01 requests (see
//     request_obd2_pid below). Layout per ISO 15765-4 single-frame response:
//       data[0] = PCI / length byte (0x03 for a 3-byte payload)
//       data[1] = 0x41  (positive response to service 0x01)
//       data[2] = PID
//       data[3..] = value bytes (PID-specific formula)
//
//   Native broadcast frames (vehicle-specific IDs):
//     Many ECUs transmit parameters continuously without being asked. If you
//     capture IDs via a CAN sniffer (e.g. SavvyCAN) while the engine runs,
//     add them to the match arm below and you get the data at the native ECU
//     broadcast rate (often 10–100 Hz) with zero request overhead.
fn decode_frame(id: u32, data: &[u8], telem: &mut CarTelemetry) {
    match id {
        // ── OBD2 positive response from the ECM ──────────────────────────
        0x7E8 if data.len() >= 4 && data[1] == 0x41 => {
            match data[2] {
                OBD2_PID_RPM if data.len() >= 5 => {
                    // (A * 256 + B) / 4
                    telem.engine_rpm =
                        (((data[3] as u16) << 8) | data[4] as u16) / 4;
                }
                OBD2_PID_COOLANT_TEMP => {
                    // A - 40
                    telem.coolant_temp_c = (data[3] as i16 - 40) as i8;
                }
                OBD2_PID_SPEED => {
                    // A
                    telem.vehicle_speed_kph = data[3];
                }
                _ => {} // Unknown PID in the response — ignore
            }
        }

        // ── Native broadcast frames (2008 Cobalt — TODO) ─────────────────
        // Replace the placeholder IDs and byte extractions below once you
        // have sniffed the bus. Use SavvyCAN or a USB-CAN adapter at idle
        // to record which frame IDs contain RPM, speed, and coolant data.
        // Example pattern (byte positions are illustrative):
        //
        // 0x0C9 if data.len() >= 4 => {
        //     telem.engine_rpm = ((data[2] as u16) << 8 | data[3] as u16) / 4;
        // }

        _ => {} // Unrecognised or irrelevant frame — discard
    }
}

// ── OBD2 request helper ───────────────────────────────────────────────────────
// Transmits a standard ISO 15765-4 single-frame service 0x01 PID request to
// the OBD2 functional broadcast address (0x7DF). The ECU responds on 0x7E8.
// Call this for each PID you want, without waiting for the response in between
// — the ECU replies asynchronously and decode_frame() handles them.
fn request_obd2_pid(
    twai: &mut impl esp_idf_hal::twai::Transmit,
    pid: u8,
) -> anyhow::Result<()> {
    use esp_idf_hal::twai::TwaiFrame;
    // PCI byte 0x02 = 2 payload bytes follow; service 0x01; the requested PID;
    // remaining bytes padded to the 8-byte CAN DLC.
    let frame = TwaiFrame::new_data(
        0x7DF,
        &[0x02, 0x01, pid, 0x00, 0x00, 0x00, 0x00, 0x00],
    )?;
    twai.transmit(&frame)?;
    Ok(())
}

fn main() -> anyhow::Result<()> {
    // Prerequisites:
    //   • Wi-Fi connected (station mode) to GL.iNet router — see peripherals_init.rs
    //   • TWAI driver initialised at 500 kbps              — see peripherals_init.rs
    // Both are passed in here once the project is wired together end-to-end.
    // For now the TWAI handle is a placeholder (marked TODO below).

    const MQTT_TOPIC_PREFIX: &str = "vehicle";

    // ── Shared telemetry state ────────────────────────────────────────────────
    // The can_rx task writes individual fields; the publish loop reads a clone.
    // CarTelemetry must derive Clone + Default (add to telemetry.rs).
    let telemetry: Arc<Mutex<CarTelemetry>> =
        Arc::new(Mutex::new(CarTelemetry::default()));

    // ── CAN receive task ──────────────────────────────────────────────────────
    // Owns the TWAI driver. Runs as fast as the bus produces frames.
    // The MQTT publish rate is completely independent of this thread.
    let telem_can = Arc::clone(&telemetry);

    thread::Builder::new()
        .name("can_rx".into())
        .stack_size(4096)
        .spawn(move || -> anyhow::Result<()> {
            // TODO: accept the initialised TwaiDriver from peripherals_init.rs.
            // let mut twai = twai_driver;
            loop {
                // ── Scatter: fire all OBD2 requests in a burst ────────────
                // The ECU responds to each asynchronously — no blocking wait
                // between requests needed. Responses arrive within ~5 ms each.
                // (Skip this block if relying solely on native broadcast frames.)
                // let _ = request_obd2_pid(&mut twai, OBD2_PID_RPM);
                // let _ = request_obd2_pid(&mut twai, OBD2_PID_COOLANT_TEMP);
                // let _ = request_obd2_pid(&mut twai, OBD2_PID_SPEED);

                // ── Gather: drain the receive buffer for up to 50 ms ─────
                // Catches OBD2 responses AND any native broadcast frames.
                // 50 ms gather window + 50 ms sleep below ≈ 10 Hz request cadence,
                // matching the MQTT publish rate without flooding the bus.
                let deadline = Instant::now() + Duration::from_millis(50);
                while Instant::now() < deadline {
                    // TODO: replace with actual TWAI receive call:
                    // match twai.receive() {
                    //     Ok(frame) => {
                    //         let mut t = telem_can.lock().unwrap();
                    //         decode_frame(frame.identifier(), frame.data(), &mut t);
                    //     }
                    //     Err(_) => thread::sleep(Duration::from_millis(1)),
                    // }
                    thread::sleep(Duration::from_millis(1)); // placeholder
                }

                thread::sleep(Duration::from_millis(50));
            }
        })?;

    // ── MQTT client ───────────────────────────────────────────────────────────
    // Broker is Mosquitto on the GL.iNet router (LAN IP 192.168.8.1 by default).
    // The hub connects as a plain Wi-Fi station — it does NOT host the broker.
    let mqtt_config = MqttClientConfiguration {
        client_id: Some("canbus_hub"),
        ..Default::default() // port 1883, no TLS — local network only
    };

    let mut client = EspMqttClient::new(
        "mqtt://192.168.8.1:1883",
        &mqtt_config,
        move |_event| {
            // Handle broker acks / disconnects here as the project matures.
        },
    )?;

    // ── Publish loop (10 Hz) ─────────────────────────────────────────────────
    // Takes a cheap Clone snapshot of the shared state so the lock is held for
    // the minimum possible time. The publish rate is independent of the CAN bus
    // rate — change it here without touching any other module.
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

        pub_topic("engine/rpm",             format!("{}", snapshot.engine_rpm))?;
        pub_topic("engine/coolant_temp",    format!("{}", snapshot.coolant_temp_c))?;
        pub_topic("engine/speed_kph",       format!("{}", snapshot.vehicle_speed_kph))?;
        pub_topic("battery/aux_voltage_mv", format!("{}", snapshot.aux_battery_mv))?;
        pub_topic("battery/aux_current_ma", format!("{}", snapshot.aux_current_ma))?;

        // 100 ms = 10 Hz — smooth for dashboard gauges, light on the broker.
        thread::sleep(Duration::from_millis(100));
    }
}
