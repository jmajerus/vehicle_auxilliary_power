// Waveshare Industrial ESP32-S3 — TWAI (CAN bus) + I2C initialization
//
// Target board : Waveshare ESP32-S3-RS485-CAN
// CAN network  : 2008 Chevy Cobalt, ISO 15765-4 (11-bit ID, 500 kbaud)
// I2C bus      : GPIO1 = SDA, GPIO2 = SCL  (JST SH1.0 bottom connector)
// Toolchain    : esp-idf-hal (std / esp-idf-sys path)
//
// Add to Cargo.toml:
//   [dependencies]
//   esp-idf-hal = "0.43"
//   anyhow      = "1"

use esp_idf_hal::peripherals::Peripherals;
use esp_idf_hal::twai::{TwaiDriver, Configuration};   // TWAI = ESP32's CAN controller
use esp_idf_hal::i2c::{I2cDriver, I2cConfig};

fn main() -> anyhow::Result<()> {
    // 1. Take ownership of the chip's physical peripherals
    let peripherals = Peripherals::take()?;

    // 2. Initialize the internal CAN bus (TWAI) at 500 kbps for the Cobalt
    //    The TX/RX pins are pre-wired internally on the Waveshare board.
    let twai_config = Configuration::new_500kbps();
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
