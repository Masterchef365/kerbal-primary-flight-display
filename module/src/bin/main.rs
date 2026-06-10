#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;
use display_interface_spi::SPIInterface;
use embedded_graphics::framebuffer::buffer_size;
use embedded_graphics::framebuffer::Framebuffer;
use embedded_graphics::image::GetPixel;
use embedded_graphics::image::ImageRawLE;
use embedded_graphics::mono_font::ascii::FONT_6X10;
use embedded_graphics::mono_font::ascii::FONT_9X18_BOLD;
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::pixelcolor::raw::LittleEndian;
use embedded_graphics::pixelcolor::raw::RawU16;
use embedded_graphics::primitives::Rectangle;
use embedded_graphics::primitives::StyledDrawable;
use embedded_graphics::primitives::Triangle;
use embedded_graphics::text::Alignment;
use embedded_graphics::text::Text;
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
use esp_hal::usb_serial_jtag::UsbSerialJtag;
use ili9341::Ili9341;
use ili9341::Orientation;
use micromath::F32Ext;

/// Degrees of FOV in the Y direction
const FOV_Y: f32 = 30.0;
/// Degrees of FOV in the X direction
const FOV_X: f32 = FOV_Y * WIDTH as f32 / HEIGHT as f32;

/// Screen width
const WIDTH: usize = 320;
/// Screen height
const HEIGHT: usize = 240;

/// TFT Display update tile size
const TILE_SIZE: usize = 10;

// Colors
const COLOR_SKY: u16 = 0b00000_111010_11111;
const COLOR_GROUND: u16 = 0b10001_010010_00000;

use alloc::boxed::Box;

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[main]
fn main() -> ! {
    // ESP-Hal setup
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    let mut delay = esp_hal::delay::Delay::new();

    esp_alloc::psram_allocator!(peripherals.PSRAM, esp_hal::psram);

    // TFT Display setup
    let config = OutputConfig::default();

    let mosi = Output::new(peripherals.GPIO11, Level::Low, config);
    let miso = Input::new(peripherals.GPIO12, InputConfig::default());
    let sck = Output::new(peripherals.GPIO13, Level::Low, config);

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

    let dc = Output::new(peripherals.GPIO17, Level::Low, config);
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

    // Show red on screen if there's an error during setup
    display.clear(Rgb565::RED).unwrap();

    let mut front_buffer = Box::new(Framebuffer::<
        Rgb565,
        _,
        LittleEndian,
        WIDTH,
        HEIGHT,
        { buffer_size::<Rgb565>(WIDTH, HEIGHT) },
    >::new());

    let mut back_buffer = Framebuffer::<
        Rgb565,
        _,
        LittleEndian,
        WIDTH,
        HEIGHT,
        { buffer_size::<Rgb565>(WIDTH, HEIGHT) },
    >::new();

    let size = Size::new(WIDTH as _, HEIGHT as _);
    let rect = Rectangle::new(Point::zero(), size);

    // USB serial
    let mut usb = UsbSerialJtag::new(peripherals.USB_DEVICE).into_async();
    usb.listen_rx_packet_recv_interrupt();

    // Runtime variables
    let mut frame_num = 0;
    let mut parser = MessageStreamParser::new(4 * 5);

    let mut current_state: DisplayState = DisplayState::default();

    let small_style = MonoTextStyle::new(&FONT_6X10, Rgb565::WHITE);
    let large_style = MonoTextStyle::new(&FONT_9X18_BOLD, Rgb565::WHITE);

    // Clear black to match what the front buffer thinks it is
    display.clear(Rgb565::BLACK).unwrap();

    loop {
        let frame_start = Instant::now();

        // Parse USB messages
        while let Ok(byte) = usb.read_byte() {
            if let Some(s) = parser.step(byte) {
                let start = Instant::now();
                match DisplayState::parse(s) {
                    Err(e) => esp_println::println!("Parsing error"),
                    Ok(state) => {
                        //esp_println::println!("Deser time: {} us", start.elapsed().as_micros());
                        current_state = state
                    }
                }
            }
        }

        // Draw background
        let (roll_sin, roll_cos) = current_state.roll.to_radians().sin_cos();

        let iter = (0..HEIGHT)
            .map(|y| {
                (0..WIDTH)
                    .map(move |x| background_fill(x, y, current_state.pitch, roll_sin, roll_cos))
            })
            .flatten();
        back_buffer.fill_contiguous(&rect, iter);

        // Draw pitch lines
        for pitch_line in (-85_i32..=85).step_by(5) {
            let width = if pitch_line == 0 {
                30.0
            } else {
                if pitch_line % 20 == 0 {
                    15.0
                } else {
                    if pitch_line % 10 == 0 {
                        10.0
                    } else {
                        7.0
                    }
                }
            };

            let line = project_line(
                pitch_line as f32 + current_state.pitch,
                width,
                current_state.roll,
            );

            if pitch_line % 10 == 0 {
                let text = pitch_line.abs().to_string();

                Text::with_alignment(&text, line.start, small_style, Alignment::Right)
                    .draw(&mut back_buffer)
                    .unwrap();

                Text::with_alignment(&text, line.end, small_style, Alignment::Left)
                    .draw(&mut back_buffer);
            }

            line.into_styled(PrimitiveStyle::with_stroke(Rgb565::WHITE, 2))
                .draw(&mut back_buffer)
                .unwrap();
        }

        // Draw chevron
        let center = Point::new(WIDTH as i32 / 2, HEIGHT as i32 / 2);
        let chev_bottom = Point::new(center.x, center.y + 10);
        let chev_left = Point::new(center.x - 30, center.y + 20);
        let chev_right = Point::new(center.x + 30, center.y + 20);

        Triangle::new(center, chev_bottom, chev_left)
            .into_styled(PrimitiveStyle::with_fill(Rgb565::YELLOW))
            .draw(&mut back_buffer)
            .unwrap();

        Triangle::new(center, chev_bottom, chev_right)
            .into_styled(PrimitiveStyle::with_fill(Rgb565::YELLOW))
            .draw(&mut back_buffer)
            .unwrap();

        // Draw 'wings'
        Line::new(
            Point::new(center.x - 30, center.y),
            Point::new(center.x - 50, center.y),
        )
        .into_styled(PrimitiveStyle::with_stroke(Rgb565::YELLOW, 2))
        .draw(&mut back_buffer)
        .unwrap();

        Line::new(
            Point::new(center.x + 30, center.y),
            Point::new(center.x + 50, center.y),
        )
        .into_styled(PrimitiveStyle::with_stroke(Rgb565::YELLOW, 2))
        .draw(&mut back_buffer)
        .unwrap();


        // Speed tape background
        let margin = 5;
        let speed_tape_sz = Size::new(WIDTH as u32 / 6, 5 * HEIGHT as u32 / 6);
        let speed_tape_tl = Point::new(
            center.x - WIDTH as i32 / 3 - speed_tape_sz.width as i32 / 2,
            center.y,
        );

        let dark_rect = Rectangle::new(
            Point::new(
                speed_tape_tl.x - speed_tape_sz.width as i32 / 2,
                speed_tape_tl.y - speed_tape_sz.height as i32 / 2,
            ),
            speed_tape_sz,
        );
        for p in dark_rect.points() {
            if let Some(c) = back_buffer.pixel(p) {
                back_buffer.set_pixel(p, Rgb565::new(c.r() / 2, c.g() / 2, c.b() / 2));
            }
        }

        // Altitude tape numbers
        let (_, quant) = quant_bignumber(current_state.speed);
        let unit = quant * 100.0;
        let rounded_speed = (current_state.speed / unit).floor() * unit;
        let remainder_speed = current_state.speed - rounded_speed;

        let n: i32 = 2;
        let total = n * 2 + 1;
        let px_per_unit: f32 = speed_tape_sz.height as f32 / (total + 1) as f32 / unit;

        for i in -n..=n {
            let offset = i as f32 * unit;
            let speed = offset + rounded_speed;

            if speed < 0.0 {
                continue;
            }

            let offset_y = ((offset - remainder_speed) * px_per_unit) as i32;
            let text_pos = Point::new(speed_tape_tl.x, center.y - offset_y);
            let text = format_bignumber(speed, true);
            Text::with_alignment(&text, text_pos, large_style, Alignment::Center)
                .draw(&mut back_buffer)
                .unwrap();
        }


        // Speed background
        let triangle_width = 5;
        let speed_bkg_sz = Size::new(speed_tape_sz.width - triangle_width, 22);
        let black_rect = Rectangle::new(
            Point::new(
                speed_tape_tl.x - speed_bkg_sz.width as i32 / 2,
                speed_tape_tl.y - speed_bkg_sz.height as i32 / 2,
            ),
            speed_bkg_sz,
        );
        black_rect
            .draw_styled(&PrimitiveStyle::with_fill(Rgb565::BLACK), &mut back_buffer)
            .unwrap();

        let br = black_rect.bottom_right().unwrap();
        let tr = Point::new(br.x, black_rect.top_left.y);
        let middle = Point::new(br.x + triangle_width as i32, (br.y + tr.y) / 2);

        Triangle::new(br, tr, middle)
            .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
            .draw(&mut back_buffer)
            .unwrap();

        // Draw speed
        let speed_text = format_bignumber(current_state.speed, false);
        Text::with_alignment(&speed_text, speed_tape_tl, large_style, Alignment::Center)
            .draw(&mut back_buffer)
            .unwrap();

        // Altitude tape background
        let alt_tape_sz = Size::new(WIDTH as u32 / 6, 5 * HEIGHT as u32 / 6);
        let alt_tape_tl = Point::new(
            center.x + WIDTH as i32 / 3 + alt_tape_sz.width as i32 / 2,
            center.y,
        );

        let dark_rect = Rectangle::new(
            Point::new(
                alt_tape_tl.x - alt_tape_sz.width as i32 / 2,
                alt_tape_tl.y - alt_tape_sz.height as i32 / 2,
            ),
            alt_tape_sz,
        );
        for p in dark_rect.points() {
            if let Some(c) = back_buffer.pixel(p) {
                back_buffer.set_pixel(p, Rgb565::new(c.r() / 2, c.g() / 2, c.b() / 2));
            }
        }

        // Altitude tape numbers
        let (_, quant) = quant_bignumber(current_state.altitude);
        let unit = quant * 100.0;
        let rounded_alt = (current_state.altitude / unit).floor() * unit;
        let remainder_alt = current_state.altitude - rounded_alt;

        let n: i32 = 2;
        let total = n * 2 + 1;
        let px_per_unit: f32 = alt_tape_sz.height as f32 / (total + 1) as f32 / unit;

        for i in -n..=n {
            let offset = i as f32 * unit;
            let altitude = offset + rounded_alt;

            if altitude < 0.0 {
                continue;
            }

            let offset_y = ((offset - remainder_alt) * px_per_unit) as i32;
            let text_pos = Point::new(alt_tape_tl.x, center.y - offset_y);
            let text = format_bignumber(altitude, true);
            Text::with_alignment(&text, text_pos, large_style, Alignment::Center)
                .draw(&mut back_buffer)
                .unwrap();
        }

        // Altitude background
        let alt_bkg_sz = Size::new(alt_tape_sz.width - triangle_width, 22);
        let black_rect = Rectangle::new(
            Point::new(
                alt_tape_tl.x - alt_bkg_sz.width as i32 / 2 + triangle_width as i32,
                alt_tape_tl.y - alt_bkg_sz.height as i32 / 2,
            ),
            alt_bkg_sz,
        );
        black_rect
            .draw_styled(&PrimitiveStyle::with_fill(Rgb565::BLACK), &mut back_buffer)
            .unwrap();

        let tl = black_rect.top_left;
        let bl = Point::new(tl.x, black_rect.bottom_right().unwrap().y);
        let middle = Point::new(tl.x - triangle_width as i32, (bl.y + tl.y) / 2);

        Triangle::new(bl, tl, middle)
            .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
            .draw(&mut back_buffer)
            .unwrap();

        // Draw altitude
        let alt_text = format_bignumber(current_state.altitude, false);
        Text::with_alignment(&alt_text, alt_tape_tl, large_style, Alignment::Center)
            .draw(&mut back_buffer)
            .unwrap();

        // Draw only those tiles which changed
        let s = TILE_SIZE;
        for yi in (0..HEIGHT).step_by(s) {
            for xi in (0..WIDTH).step_by(s) {
                // Check if the tile matches
                let ymax = (yi + s).min(HEIGHT);
                let xmax = (xi + s).min(WIDTH);
                {
                    let back_buffer = &back_buffer;
                    let front_buffer = &front_buffer;
                    if (yi..ymax).all(|y| {
                        (xi..xmax).all(move |x| {
                            let back_px = back_buffer.pixel(Point::new(x as _, y as _)).unwrap();
                            let front_px = front_buffer.pixel(Point::new(x as _, y as _)).unwrap();
                            front_px == back_px
                        })
                    }) {
                        continue;
                    }
                }

                // Otherwise draw it from the back buffer
                {
                    let back_buffer = &back_buffer;
                    let iter = (yi..ymax)
                        .map(|y| {
                            (xi..xmax).map(move |x| {
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
                        .draw_raw_iter(xi as _, yi as _, (xmax - 1) as _, (ymax - 1) as _, iter)
                        .unwrap();

                    // Remember the change
                    for y in yi..ymax {
                        for x in xi..xmax {
                            let c = back_buffer.pixel(Point::new(x as _, y as _)).unwrap();
                            front_buffer.set_pixel(Point::new(x as _, y as _), c);
                        }
                    }
                }
            }
        }

        frame_num += 1;

        //let elap = frame_start.elapsed();
        //let ms = elap.as_millis();
        //let hz = 1000.0 / ms as f32;
        //esp_println::println!("{ms} ms = {hz} Hz");

        //display.clear(Rgb565::RED).unwrap();
    }
}

fn background_fill(x: usize, y: usize, pitch: f32, roll_sin: f32, roll_cos: f32) -> Rgb565 {
    let x = x as f32 - WIDTH as f32 / 2.0;
    let y = y as f32 - HEIGHT as f32 / 2.0;

    let px_per_degree = HEIGHT as f32 / FOV_Y / 2.0;
    let xe = -x * roll_sin;
    let ye = y * roll_cos;
    let pitch_component = pitch * px_per_degree;

    let outside = x * x + y * y > (WIDTH.min(HEIGHT) / 2).pow(2) as f32;

    let shade = if outside {
        xe > ye
    } else {
        xe > ye - pitch_component
    };

    if shade {
        Rgb565::from(RawU16::from(COLOR_SKY))
    } else {
        Rgb565::from(RawU16::from(COLOR_GROUND))
    }
}

struct MessageStreamParser {
    current_message: Vec<u8>,
    seek: bool,
    msg_len: usize,
}

impl MessageStreamParser {
    pub fn new(msg_len: usize) -> Self {
        Self {
            current_message: Vec::new(),
            seek: true,
            msg_len,
        }
    }

    pub fn step(&mut self, byte: u8) -> Option<Vec<u8>> {
        self.current_message.push(byte);

        if self.seek {
            if self.current_message.ends_with(&[0x00, 0x00, 0x00, 0xff]) {
                self.seek = false;
                self.current_message.clear();
            }
            None
        } else {
            if self.current_message.len() == self.msg_len {
                self.seek = true;
                Some(core::mem::take(&mut self.current_message))
            } else {
                None
            }
        }
    }
}

#[derive(Copy, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct DisplayState {
    /// Pitch (degrees)
    pub pitch: f32,
    /// Roll (degrees)
    pub roll: f32,
    /// Altitude (meters)
    pub altitude: f32,
    /// Velocity (meters per second)
    pub speed: f32,
    /// Heading (degrees)
    pub heading: f32,
    // /// Target pitch difference (degrees)
    // pub target_pitch: f32,
    // /// Target yaw difference (degrees)
    // pub target_yaw: f32,
}

impl DisplayState {
    pub fn parse(bytes: Vec<u8>) -> Result<Self, ()> {
        Ok(Self {
            pitch: read_f32(&bytes[0..4]),
            roll: read_f32(&bytes[4..8]),
            altitude: read_f32(&bytes[8..12]),
            speed: read_f32(&bytes[12..16]),
            heading: read_f32(&bytes[16..20]),
        })
    }
}

fn read_f32(values: &[u8]) -> f32 {
    f32::from_le_bytes([values[0], values[1], values[2], values[3]])
}

fn project_relative_point(rel_pitch: f32, rel_yaw: f32, roll: f32) -> Point {
    let px_per_degree = HEIGHT as f32 / FOV_Y / 2.0;
    let dy = rel_pitch * px_per_degree;
    let dx = rel_yaw * px_per_degree;
    let (sin, cos) = roll.to_radians().sin_cos();
    Point::new(
        (dy * sin + dx * cos) as i32 + WIDTH as i32 / 2,
        (dy * cos - dx * sin) as i32 + HEIGHT as i32 / 2,
    )
}

fn project_line(rel_pitch: f32, size: f32, roll: f32) -> Line {
    Line::new(
        project_relative_point(rel_pitch, -size, roll),
        project_relative_point(rel_pitch, size, roll),
    )
}

fn quant_bignumber(value: f32) -> (&'static str, f32) {
    if value > 1e2 * 1e9 {
        ("G", 1e9)
    } else if value > 1e2 * 1e6 {
        ("M", 1e6)
    } else if value > 1e2 * 1e3 {
        ("k", 1e3)
    } else {
        ("", 1.0)
    }
}

fn format_bignumber(mut value: f32, round_100: bool) -> String {
    let (unit, quant) = quant_bignumber(value);
    let mut value = value / quant;

    if round_100 {
        value = (value / 100.0).floor() * 100.0;
    } else {
        value = value.floor();
    }
    format!("{:5}{unit}", value.floor())
}
