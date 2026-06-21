#!/usr/bin/env python3
"""
simulate_telemetry.py — Publish synthetic vehicle telemetry to an MQTT broker.

Simulates a vehicle warming up, accelerating, and decelerating over time.
Use this during UI development to test display nodes without physical hardware.

Requirements:
    pip install -r requirements.txt

Usage:
    # Against a local Mosquitto instance (default):
    python3 simulate_telemetry.py

    # Against the GL.iNet in-car broker:
    python3 simulate_telemetry.py --host 192.168.8.1

    # Custom prefix or update rate:
    python3 simulate_telemetry.py --prefix cobalt --hz 20
"""

import argparse
import math
import random
import time

import paho.mqtt.client as mqtt


def main() -> None:
    parser = argparse.ArgumentParser(description="Simulate vehicle telemetry via MQTT")
    parser.add_argument("--host",   default="127.0.0.1", help="Broker hostname or IP (default: 127.0.0.1)")
    parser.add_argument("--port",   default=1883, type=int, help="Broker TCP port (default: 1883)")
    parser.add_argument("--prefix", default="vehicle",    help="MQTT topic prefix (default: vehicle)")
    parser.add_argument("--hz",     default=10.0, type=float, help="Publish rate in Hz (default: 10)")
    args = parser.parse_args()

    client = mqtt.Client(client_id="telemetry_sim", protocol=mqtt.MQTTv311)
    client.connect(args.host, args.port, keepalive=60)
    client.loop_start()

    print(f"Simulating → mqtt://{args.host}:{args.port}/{args.prefix}/...  at {args.hz} Hz")
    print("Press Ctrl+C to stop.\n")

    interval = 1.0 / args.hz
    t = 0.0
    coolant = 20.0          # °C — starts at ambient, warms asymptotically to ~92°C

    try:
        while True:
            t += interval

            # ── Engine RPM ───────────────────────────────────────────────────
            # Gentle sine wave simulating acceleration/deceleration cycles.
            rpm = int(800 + 2200 * abs(math.sin(t * 0.18)) + random.gauss(0, 40))
            rpm = max(650, min(6500, rpm))

            # ── Vehicle speed ─────────────────────────────────────────────────
            # Loosely correlated with RPM.
            speed = max(0, int((rpm - 700) / 20 + random.gauss(0, 1)))
            speed = min(200, speed)

            # ── Coolant temperature ───────────────────────────────────────────
            # Asymptotic warmup toward 92°C (typical thermostat open temp).
            coolant += (92.0 - coolant) * 0.002 + random.gauss(0, 0.05)
            coolant = max(18.0, min(115.0, coolant))

            # ── Auxiliary battery ─────────────────────────────────────────────
            # ~13.8 V when engine running (alternator charging), ~12.6 V at rest.
            charging = rpm > 900
            target_mv = 13_800 if charging else 12_600
            aux_mv = int(target_mv + random.gauss(0, 60))
            aux_mv = max(10_000, min(16_000, aux_mv))

            # Positive = charging into aux bank, negative = aux bank discharging.
            aux_ma = int((-900 if charging else 700) + random.gauss(0, 150))

            # ── Publish ───────────────────────────────────────────────────────
            p = args.prefix
            client.publish(f"{p}/engine/rpm",           str(rpm),              qos=0)
            client.publish(f"{p}/engine/speed_kph",     str(speed),            qos=0)
            client.publish(f"{p}/engine/coolant_temp",  f"{coolant:.1f}",      qos=0)
            client.publish(f"{p}/battery/aux_voltage_mv", str(aux_mv),         qos=0)
            client.publish(f"{p}/battery/aux_current_ma", str(aux_ma),         qos=0)

            print(
                f"\r  RPM:{rpm:5d}  Speed:{speed:3d} km/h  "
                f"Coolant:{coolant:5.1f}°C  "
                f"Batt:{aux_mv/1000:.2f} V  "
                f"Current:{aux_ma/1000:+.2f} A   ",
                end="", flush=True,
            )

            time.sleep(interval)

    except KeyboardInterrupt:
        print("\nStopped.")
    finally:
        client.loop_stop()
        client.disconnect()


if __name__ == "__main__":
    main()
