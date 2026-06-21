// MQTT publisher loop for the Waveshare CAN hub
//
// The hub connects to a local in-car Wi-Fi network (either its own AP or a
// Raspberry Pi running Mosquitto) and publishes telemetry to structured topics.
//
// MQTT topic layout:
//   cobalt/engine/rpm
//   cobalt/engine/coolant_temp
//   cobalt/engine/speed_kph
//   cobalt/battery/aux_voltage_mv
//   cobalt/battery/aux_current_ma
//
// Subscribers (display nodes, MQTT Explorer on a laptop, Home Assistant, etc.)
// receive these topics and format/render data however they choose.
// The hub firmware never needs to change when the display changes.
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

    // Broker address options:
    //   "mqtt://192.168.4.1:1883"  — Raspberry Pi / external router
    //   "mqtt://127.0.0.1:1883"    — hub running its own embedded broker
    let broker_url = "mqtt://192.168.4.1:1883";

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
