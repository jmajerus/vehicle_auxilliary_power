// MQTT publisher loop for the Waveshare CAN hub
//
// Architecture: "headless hub" pattern
// ─────────────────────────────────────
//  Waveshare ESP32-S3 (CAN hub)
//    └─ Wi-Fi client (station mode) ──► GL.iNet router (OpenWrt)
//                                           └─ Mosquitto MQTT broker
//                                                ├─ Display node A  (OLED, subscribes to rpm + voltage)
//                                                ├─ Display node B  (TFT gauge, subscribes to all topics)
//                                                └─ Laptop / phone  (MQTT Explorer, diagnostics)
//
// The hub connects to the GL.iNet as a plain Wi-Fi client and publishes raw
// telemetry. It has no knowledge of how many displays exist or how they render
// data. New display peripherals can be added or reconfigured without touching
// or recompiling this firmware.
//
// GL.iNet broker setup (one-time, via SSH):
//   opkg update && opkg install mosquitto-nossl
//   # Edit /etc/mosquitto/mosquitto.conf — set listener 1883, allow_anonymous true
//   /etc/init.d/mosquitto enable && /etc/init.d/mosquitto start
//
// MQTT topic layout:
//   cobalt/engine/rpm              — u16, raw RPM
//   cobalt/engine/coolant_temp     — i8,  degrees C
//   cobalt/engine/speed_kph        — u8,  km/h
//   cobalt/battery/aux_voltage_mv  — u16, millivolts (e.g. 13400 = 13.4 V)
//   cobalt/battery/aux_current_ma  — i32, milliamps (negative = discharging)
//
// Cargo.toml dependencies:
//   esp-idf-svc = "0.48"
//   anyhow      = "1"

use esp_idf_svc::mqtt::client::{EspMqttClient, MqttClientConfiguration, QoS};
use std::time::Duration;
use std::thread;

// Replace with your actual CAN-read function (uses the TWAI driver)
fn read_can_bus_rpm() -> u16 {
    todo!("Implement TWAI OBD2 PID 0x0C request/response")
}

fn main() -> anyhow::Result<()> {
    // Prerequisite: Wi-Fi must already be initialised and connected before
    // reaching this point. The hub can act as an AP (SSID "Cobalt-Net") or
    // connect to an existing in-car router.

    let mqtt_config = MqttClientConfiguration {
        client_id: Some("cobalt_hub"),
        ..Default::default() // Standard port 1883, no TLS for local network
    };

    // Broker is Mosquitto running on the GL.iNet router.
    // The GL.iNet's LAN IP is typically 192.168.8.1 (check GL.iNet admin panel).
    // The hub connects in station (client) mode — it does NOT run its own AP or broker.
    let broker_url = "mqtt://192.168.8.1:1883";

    let mut client = EspMqttClient::new(broker_url, &mqtt_config, move |_event| {
        // Handle incoming broker acknowledgements / subscriptions here if needed
    })?;

    loop {
        let rpm = read_can_bus_rpm();

        // Publish each telemetry value as a simple UTF-8 string payload.
        // QoS::AtMostOnce (fire-and-forget) is appropriate for high-frequency
        // sensor data where occasional dropped packets are acceptable.
        client.publish(
            "cobalt/engine/rpm",
            QoS::AtMostOnce,
            false,
            format!("{}", rpm).as_bytes(),
        )?;

        // TODO: add coolant_temp, speed, aux_battery_mv, aux_current_ma topics

        // 100 ms = 10 Hz update rate — smooth for dashboard gauges,
        // low enough to avoid flooding the broker.
        thread::sleep(Duration::from_millis(100));
    }
}
