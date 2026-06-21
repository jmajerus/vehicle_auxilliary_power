// Waveshare Industrial ESP32-S3 — TWAI (CAN bus) + I2C initialization
//
// Target board : Waveshare ESP32-S3-RS485-CAN
// CAN network  : Any OBD2 vehicle (ISO 15765-4, 11-bit ID, 500 kbaud typical)
//                Some vehicles use 250 kbaud — check your service manual.
// I2C bus      : GPIO1 = SDA, GPIO2 = SCL  (JST SH1.0 bottom connector)
// Toolchain    : esp-idf-hal (std / esp-idf-sys path)
//
// Add to Cargo.toml:
//   [dependencies]
//   esp-idf-hal = "0.43"
//   anyhow      = "1"

use esp_idf_hal::peripherals::Peripherals;
use esp_idf_hal::twai::{TwaiDriver, TwaiConfig, TwaiFilter, BitTiming};  // TWAI = ESP32's CAN controller
use esp_idf_hal::i2c::{I2cDriver, I2cConfig};

fn main() -> anyhow::Result<()> {
    // 1. Take ownership of the chip's physical peripherals
    let peripherals = Peripherals::take()?;

    // 2. Configure hardware acceptance filter (first line of defence)
    //
    //    The TWAI peripheral's hardware filter is a code/mask register pair.
    //    Only frames whose 11-bit ID satisfies (id & mask) == (code & mask)
    //    are passed to the receive buffer; all others are silently dropped
    //    in silicon — zero CPU cost.
    //
    //    Strategy: admit the ranges we know we need, block everything else.
    //
    //    2008 Cobalt GMLAN high-speed bus — known useful ranges:
    //      0x000–0x0FF  OBD2 / diagnostic  (0x7DF requests, 0x7E8 responses)
    //      0x100–0x4FF  ECM, ABS, BCM, HVAC broadcast frames (vehicle-specific)
    //
    //    Frames we want to block entirely (examples — verify with sniffer):
    //      0x500–0x5FF  Infotainment / XM radio
    //      0x600–0x6FF  Climate control UI events not useful for telemetry
    //      0x700–0x77F  Network management / wake frames
    //
    //    Hardware filter is a single code+mask pair, so it admits one
    //    contiguous aligned range. The broadest safe choice is to admit
    //    0x000–0x7FF (all 11-bit IDs) here and rely on the software
    //    allowlist in mqtt_publisher.rs to drop the unwanted ones.
    //    Once you have sniffed the bus and know the exact IDs you care
    //    about you can tighten this to a narrower aligned range.
    //
    //    TwaiFilter::new(code, mask):
    //      code = the bit pattern to match
    //      mask = 1 bits are "must match code", 0 bits are "don't care"
    //
    //    Accept all 11-bit IDs (open filter — software list does the real work):
    let hw_filter = TwaiFilter::new(0x000, 0x000); // code 0, mask 0 → accept all
    //
    //    Tighter example once IDs are known — accept only 0x000–0x4FF:
    //    Aligned mask for IDs < 0x500:  mask = 0x600 (bits 10–9 must be 0)
    //    let hw_filter = TwaiFilter::new(0x000, 0x600);
    //
    //    Exact single-ID example (admit only 0x0C9):
    //    let hw_filter = TwaiFilter::new(0x0C9, 0x7FF);

    // 3. Initialise the TWAI driver in listen-only mode at 500 kbps
    //    Listen-only: the controller never drives ACK bits, so it cannot
    //    disturb the live vehicle bus — safe to leave connected at all times.
    //    (Change BitTiming::baud_500k() to baud_250k() for 250 kbaud vehicles.)
    let twai_config = TwaiConfig::new(BitTiming::baud_500k())
        .listen_only(true)
        .filter(hw_filter);

    let mut _twai = TwaiDriver::new(
        peripherals.twai,
        peripherals.pins.gpio_can_tx,  // internal — check Waveshare schematic
        peripherals.pins.gpio_can_rx,  // internal — check Waveshare schematic
        &twai_config,
    )?;

    // 3. Initialize the shared I2C bus on the external JST header pins
    //    Devices on this bus: LCD display, INA219/226 voltage sensor,
    //    outboard Arduino slave nodes (one per unique I2C address).
    let i2c_config = I2cConfig::new().baudrate(100_000.into());
    let mut _i2c = I2cDriver::new(
        peripherals.i2c0,
        peripherals.pins.gpio1,  // SDA — JST pin 3
        peripherals.pins.gpio2,  // SCL — JST pin 4
        &i2c_config,
    )?;

    // Your multi-threaded loop code goes here (see mqtt_publisher.rs for an example)
    Ok(())
}
