// CarTelemetry — fixed data payload struct for the headless hub protocol
//
// Vehicle-agnostic: covers the full SAE J1979 service 0x01 parameter set —
// on par with apps like Torque Pro. Vehicle-specific broadcast fields can be
// appended as they are reverse-engineered; display nodes subscribe only to
// the topics they want.
//
// Transmission wire format: raw bytes via I2C slave response OR MQTT topics.
// #[repr(C)] guarantees a stable, cross-platform memory layout so any outboard
// board (Arduino, Pico, etc.) can deserialize the byte array if needed.

#[repr(C)]
#[derive(Clone, Default)]
pub struct CarTelemetry {

    // ── Engine performance ────────────────────────────────────────────────────

    /// OBD2 PID 0x0C — Engine RPM (0–16 383)
    pub engine_rpm: u16,

    /// OBD2 PID 0x1F — Engine run time since start (0–65 535 s)
    pub engine_runtime_s: u16,

    /// OBD2 PID 0x10 — Mass air flow rate, raw counts (divide by 100 for g/s)
    pub maf_rate_raw: u16,

    /// OBD2 PID 0x5E — Engine fuel rate, raw counts (divide by 20 for L/h)
    pub fuel_rate_raw: u16,

    /// OBD2 PID 0x42 — ECM supply voltage in millivolts
    pub module_voltage_mv: u16,

    /// OBD2 PID 0x04 — Calculated engine load (0–100 %)
    pub engine_load_pct: u8,

    /// OBD2 PID 0x43 — Absolute load value (0–100 %, saturates for NA engines)
    pub abs_load_pct: u8,

    /// OBD2 PID 0x0D — Vehicle speed (0–255 km/h)
    pub vehicle_speed_kph: u8,

    // ── Throttle / pedal ─────────────────────────────────────────────────────

    /// OBD2 PID 0x11 — Absolute throttle position (0–100 %)
    pub throttle_pct: u8,

    /// OBD2 PID 0x45 — Relative throttle position (0–100 %)
    pub throttle_rel_pct: u8,

    /// OBD2 PID 0x4C — Commanded throttle actuator (0–100 %)
    pub throttle_cmd_pct: u8,

    /// OBD2 PID 0x49 — Accelerator pedal position D (0–100 %)
    pub accel_pedal_d_pct: u8,

    /// OBD2 PID 0x4A — Accelerator pedal position E (0–100 %)
    pub accel_pedal_e_pct: u8,

    // ── Temperature ──────────────────────────────────────────────────────────

    /// OBD2 PID 0x05 — Engine coolant temperature, °C (practical -40 to 127)
    pub coolant_temp_c: i8,

    /// OBD2 PID 0x0F — Intake air temperature, °C
    pub intake_air_temp_c: i8,

    /// OBD2 PID 0x46 — Ambient air temperature, °C
    pub ambient_air_temp_c: i8,

    /// OBD2 PID 0x5C — Engine oil temperature, °C
    pub oil_temp_c: i8,

    // ── Air / fuel ───────────────────────────────────────────────────────────

    /// OBD2 PID 0x0B — Intake manifold absolute pressure (0–255 kPa)
    pub intake_map_kpa: u8,

    /// OBD2 PID 0x33 — Barometric pressure (0–255 kPa)
    pub baro_kpa: u8,

    /// OBD2 PID 0x0E — Ignition timing advance (-64 to +63 ° BTDC)
    pub timing_advance_deg: i8,

    /// OBD2 PID 0x06 — Short-term fuel trim bank 1 (-100 to +99 %)
    pub fuel_trim_short_b1: i8,

    /// OBD2 PID 0x07 — Long-term fuel trim bank 1 (-100 to +99 %)
    pub fuel_trim_long_b1: i8,

    /// OBD2 PID 0x08 — Short-term fuel trim bank 2 (-100 to +99 %)
    pub fuel_trim_short_b2: i8,

    /// OBD2 PID 0x09 — Long-term fuel trim bank 2 (-100 to +99 %)
    pub fuel_trim_long_b2: i8,

    /// OBD2 PID 0x2F — Fuel tank level (0–100 %)
    pub fuel_tank_pct: u8,

    /// OBD2 PID 0x0A — Fuel pressure (0–765 kPa gauge; stored /3 to fit u8)
    pub fuel_pressure_kpa_div3: u8,

    // ── Auxiliary battery (I2C INA219/226 sensor, not OBD2) ──────────────────

    /// Auxiliary battery voltage in millivolts (e.g. 13_400 = 13.4 V)
    pub aux_battery_mv: u16,

    /// Auxiliary battery current in milliamps (negative = discharging)
    pub aux_current_ma: i32,
}

impl CarTelemetry {
    /// Serialize the struct to a fixed-size byte slice for I2C transmission.
    ///
    /// # Safety
    /// Safe because `#[repr(C)]` guarantees layout stability and all fields
    /// are plain-old-data types with no interior pointers.
    pub fn as_bytes(&self) -> &[u8] {
        let ptr = self as *const Self as *const u8;
        unsafe { std::slice::from_raw_parts(ptr, std::mem::size_of::<Self>()) }
    }
}
