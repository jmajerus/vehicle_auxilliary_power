// CarTelemetry — fixed data payload struct for the headless hub protocol
//
// Vehicle-agnostic: all fields use standard OBD2 PIDs available on any
// OBD2-compliant vehicle (US market 1996+). The hub populates this struct
// and broadcasts it; display modules decide how to render the values.
// The hub firmware never needs updating when the UI changes.
//
// Transmission wire format: raw bytes via I2C slave response  OR  MQTT JSON/binary.
// Use `#[repr(C)]` to guarantee a stable, cross-platform memory layout so that
// any outboard board (Arduino, Pico, etc.) can deserialize the byte array.

#[repr(C)]
#[derive(Clone, Default)]
pub struct CarTelemetry {
    /// Engine RPM from OBD2 PID 0x0C (range 0–16383)
    pub engine_rpm: u16,

    /// Engine coolant temperature in °C from OBD2 PID 0x05 (range -40 to 215)
    pub coolant_temp_c: i8,

    /// Vehicle speed in km/h from OBD2 PID 0x0D (range 0–255)
    pub vehicle_speed_kph: u8,

    /// Auxiliary battery voltage in millivolts (e.g. 13_400 = 13.4 V).
    /// Stored as millivolts to avoid floating-point across the wire.
    pub aux_battery_mv: u16,

    /// Auxiliary battery current in milliamperes (negative = discharging).
    /// Measured by outboard INA219/226 I2C sensor.
    pub aux_current_ma: i32,
}

impl CarTelemetry {
    /// Serialize the struct to a fixed-size byte slice for transmission.
    ///
    /// # Safety
    /// Safe because `#[repr(C)]` guarantees layout stability and all fields
    /// are plain-old-data types with no padding surprises at these sizes.
    pub fn as_bytes(&self) -> &[u8] {
        let ptr = self as *const Self as *const u8;
        unsafe { std::slice::from_raw_parts(ptr, std::mem::size_of::<Self>()) }
    }
}
