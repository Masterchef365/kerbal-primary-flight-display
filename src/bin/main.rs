#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use display_interface_spi::SPIInterface;
use embedded_graphics::framebuffer::buffer_size;
use embedded_graphics::framebuffer::Framebuffer;
use embedded_graphics::image::GetPixel;
use embedded_graphics::image::ImageRawLE;
use embedded_graphics::pixelcolor::raw::LittleEndian;
use embedded_graphics::pixelcolor::raw::RawU16;
use embedded_graphics::primitives::Rectangle;
use embedded_graphics::{
    prelude::*,
    primitives::{Line, PrimitiveStyle},
};
use embedded_graphics_core::draw_target::DrawTarget;
use embedded_graphics_core::pixelcolor::Rgb565;
use embedded_graphics_core::pixelcolor::RgbColor;
use embedded_hal_bus::spi::ExclusiveDevice;
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::gpio::Pin;
use esp_hal::gpio::{Input, InputConfig, Level, Output, OutputConfig};
use esp_hal::main;
use esp_hal::spi::{
    master::{Config, Spi},
    Mode,
};
use esp_hal::time::Rate;
use esp_hal::time::{Duration, Instant};
use ili9341::Ili9341;
use ili9341::Orientation;
use micromath::F32Ext;

const WIDTH: usize = 320;
const HEIGHT: usize = 240;
const TILE_SIZE: usize = 20;

/*
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    esp_println::println!("{info}");
    esp_println::println!("{}", esp_alloc::HEAP.stats());
    loop {}
}
*/

extern crate alloc;

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[main]
fn main() -> ! {
    // generator version: 1.3.0
    // generator parameters: --chip esp32 -o esp32-wroom-32 -o alloc -o neovim -o esp

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    // The following pins are used to bootstrap the chip. They are available
    // for use, but check the datasheet of the module for more information on them.
    // - GPIO0
    // - GPIO2
    // - GPIO5
    // - GPIO12
    // - GPIO15
    // These GPIO pins are in use by some feature of the module and should not be used.
    let _ = peripherals.GPIO6;
    let _ = peripherals.GPIO7;
    let _ = peripherals.GPIO8;
    let _ = peripherals.GPIO9;
    let _ = peripherals.GPIO10;
    let _ = peripherals.GPIO11;
    let _ = peripherals.GPIO16;
    let _ = peripherals.GPIO20;

    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 98768);

    let config = OutputConfig::default();

    let mosi = Output::new(peripherals.GPIO23, Level::Low, config);
    let miso = Input::new(peripherals.GPIO19, InputConfig::default());
    let sck = Output::new(peripherals.GPIO18, Level::Low, config);

    let mut spi = Spi::new(
        peripherals.SPI2,
        Config::default()
            .with_frequency(Rate::from_khz(40000))
            .with_mode(Mode::_0),
    )
    .unwrap()
    .with_sck(sck)
    .with_mosi(mosi)
    .with_miso(miso);

    let mut delay = esp_hal::delay::Delay::new();

    let dc = Output::new(peripherals.GPIO21, Level::Low, config);
    let cs = Output::new(peripherals.GPIO16, Level::Low, config);

    let reset_gpio = Output::new(peripherals.GPIO5, Level::Low, config); // Unused

    let device = ExclusiveDevice::new(spi, cs, delay).unwrap();

    let iface = SPIInterface::new(device, dc);

    let mut display = Ili9341::new(
        iface,
        reset_gpio,
        &mut delay,
        Orientation::LandscapeFlipped,
        ili9341::DisplaySize240x320,
    )
    .unwrap();

    display.clear(Rgb565::RED).unwrap();

    let mut front_buffer = Framebuffer::<
        Rgb565,
        _,
        LittleEndian,
        WIDTH,
        HEIGHT,
        { buffer_size::<Rgb565>(WIDTH, HEIGHT) },
    >::new();

    let mut back_buffer = Framebuffer::<
        Rgb565,
        _,
        LittleEndian,
        WIDTH,
        HEIGHT,
        { buffer_size::<Rgb565>(WIDTH, HEIGHT) },
    >::new();

    let mut time = 0;

    let size = Size::new(WIDTH as _, HEIGHT as _);
    let rect = Rectangle::new(Point::zero(), size);

    loop {
        let iter = (0..HEIGHT)
            .map(|y| (0..HEIGHT).map(move |x| fragment(x, y, time)))
            .flatten();
        back_buffer.fill_contiguous(&rect, iter);

        let s = TILE_SIZE;
        for yi in (0..HEIGHT).step_by(s) {
            for xi in (0..WIDTH).step_by(s) {
                {
                    let back_buffer = &back_buffer;
                    let front_buffer = &front_buffer;
                    if (yi..yi + s).all(|y| {
                        (xi..xi + s).all(move |x| {
                            back_buffer.pixel(Point::new(x as _, y as _))
                                == front_buffer.pixel(Point::new(x as _, y as _))
                        })
                    }) {
                        continue;
                    }
                }

                {
                    let back_buffer = &back_buffer;
                    let iter = (yi..yi + s)
                        .map(|y| {
                            (xi..xi + s).map(move |x| {
                                let px: RawU16 = back_buffer
                                    .pixel(Point::new(x as _, y as _))
                                    .unwrap()
                                    .into();
                                let px: u16 = px.into_inner();
                                px
                            })
                        })
                        .flatten();

                    display
                        .draw_raw_iter(xi as _, yi as _, TILE_SIZE as _, TILE_SIZE as _, iter)
                        .unwrap();
                }
            }
        }

        time += 1;

        core::mem::swap(&mut front_buffer, &mut back_buffer);
    }
}

fn fragment(x: usize, y: usize, time: usize) -> Rgb565 {
    let x = x as f32 - 320.0 / 2.0;
    let y = y as f32 - 240.0 / 2.0;
    let t = time as f32 / 50.0;

    if x * t.cos() < y * t.sin() {
        Rgb565::from(RawU16::from(0b00000_111010_11111))
    } else {
        Rgb565::from(RawU16::from(0b10001_010010_00000))
    }
}
