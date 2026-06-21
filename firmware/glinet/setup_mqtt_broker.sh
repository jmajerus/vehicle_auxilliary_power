#!/bin/sh
# GL.iNet OpenWrt — Mosquitto MQTT broker setup
#
# Run this script once via SSH on the GL.iNet router to install and configure
# the Mosquitto broker. After this, the broker starts automatically on boot.
#
# Usage (from your workstation):
#   ssh root@192.168.8.1 'ash -s' < setup_mqtt_broker.sh
#
# Or copy the script first:
#   scp setup_mqtt_broker.sh root@192.168.8.1:/tmp/
#   ssh root@192.168.8.1 ash /tmp/setup_mqtt_broker.sh
#
# Default GL.iNet credentials: root / (password set on first login)
# Default GL.iNet LAN IP:      192.168.8.1
#
# Tested on: GL.iNet OpenWrt 21.x / 23.x

set -e  # exit immediately on any error

echo "==> Updating package list..."
opkg update

echo "==> Installing Mosquitto (no SSL build — smaller, sufficient for LAN use)..."
opkg install mosquitto-nossl mosquitto-client-nossl

echo "==> Deploying mosquitto.conf..."
# Back up the default config if it exists
if [ -f /etc/mosquitto/mosquitto.conf ]; then
    cp /etc/mosquitto/mosquitto.conf /etc/mosquitto/mosquitto.conf.bak
    echo "    (original config backed up to mosquitto.conf.bak)"
fi

# Write the project config directly
cat > /etc/mosquitto/mosquitto.conf << 'EOF'
listener 1883
allow_anonymous true
persistence false
log_dest syslog
log_type error
log_type warning
log_type notice
max_inflight_messages 10
max_queued_messages 50
EOF

echo "==> Enabling Mosquitto to start on boot..."
/etc/init.d/mosquitto enable

echo "==> Starting Mosquitto now..."
/etc/init.d/mosquitto start

echo ""
echo "==> Verifying broker is listening on port 1883..."
# Give it a moment to start
sleep 2
if netstat -tlnp 2>/dev/null | grep -q ':1883'; then
    echo "    OK — Mosquitto is running on port 1883"
else
    echo "    WARNING — port 1883 not detected; check: logread | grep mosquitto"
fi

echo ""
echo "==> Setup complete."
echo "    Broker address for devices: mqtt://192.168.8.1:1883"
echo ""
echo "    To monitor live MQTT traffic from the router:"
echo "      mosquitto_sub -h 192.168.8.1 -t '#' -v"
echo ""
echo "    To check broker logs:"
echo "      logread | grep mosquitto"
