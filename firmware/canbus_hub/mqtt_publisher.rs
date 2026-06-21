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
        0x7E8 if data.len() >= 4 && data[1] == 0x41 => {
            match data[2] {
                0x0C if data.len() >= 5 => {
                    // RPM = (A * 256 + B) / 4
                    telem.engine_rpm =
                        (((data[3] as u16) << 8) | data[4] as u16) / 4;
                }
                0x05 => {
                    // Coolant temp = A - 40 °C
                    telem.coolant_temp_c = (data[3] as i16 - 40) as i8;
                }
                0x0D => {
                    // Speed = A km/h
                    telem.vehicle_speed_kph = data[3];
                }
                _ => {}
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
    const FALLBACK_PIDS: &[u8] = &[
        0x0C, // RPM          — remove once native broadcast is decoded
        0x05, // Coolant temp — remove once native broadcast is decoded
        0x0D, // Speed        — remove once native broadcast is decoded
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

        pub_topic("engine/rpm",             format!("{}", snapshot.engine_rpm))?;
        pub_topic("engine/coolant_temp",    format!("{}", snapshot.coolant_temp_c))?;
        pub_topic("engine/speed_kph",       format!("{}", snapshot.vehicle_speed_kph))?;
        pub_topic("battery/aux_voltage_mv", format!("{}", snapshot.aux_battery_mv))?;
        pub_topic("battery/aux_current_ma", format!("{}", snapshot.aux_current_ma))?;

        // Vehicle-specific extras added here as native broadcast decoding
        // is filled in above — display nodes subscribe only to what they want.

        thread::sleep(Duration::from_millis(100)); // 10 Hz
    }
}
