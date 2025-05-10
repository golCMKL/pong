#![feature(sync_unsafe_cell)]
#![feature(abi_x86_interrupt)]
#![no_std]
#![no_main]

extern crate alloc;

mod screen;
mod allocator;
mod frame_allocator;
mod interrupts;
mod gdt;

use alloc::boxed::Box;
use core::fmt::Write;
use core::slice;
use bootloader_api::{entry_point, BootInfo, BootloaderConfig};
use bootloader_api::config::Mapping::Dynamic;
use bootloader_api::info::MemoryRegionKind;
use kernel::{HandlerTable, serial};
use pc_keyboard::DecodedKey;
use x86_64::registers::control::Cr3;
use x86_64::VirtAddr;
use crate::frame_allocator::BootInfoFrameAllocator;
use crate::screen::{Writer, screenwriter};

const BOOTLOADER_CONFIG: BootloaderConfig = {
    let mut config = BootloaderConfig::new_default();
    config.mappings.physical_memory = Some(Dynamic);
    config.kernel_stack_size = 256 * 1024;
    config
};
entry_point!(kernel_main, config = &BOOTLOADER_CONFIG);

#[derive(PartialEq, Eq)]
pub enum GameMode {
    Menu,
    OnePlayer,
    TwoPlayer,
    GameOver,
}

pub struct Pong {
    pub game_mode: GameMode,
    pub ball_x: usize,
    pub ball_y: usize,
    pub ball_dx: isize,
    pub ball_dy: isize,
    pub player1_y: usize,
    pub player2_y: usize,
    pub player1_score: u32,
    pub player2_score: u32,
    pub width: usize,
    pub height: usize,
    pub paddle_height: usize,
}

impl Pong {
    pub const fn new(width: usize, height: usize) -> Self {
        Self {
            game_mode: GameMode::Menu,
            ball_x: width / 2,
            ball_y: height / 2,
            ball_dx: 1,
            ball_dy: 1,
            player1_y: height / 2,
            player2_y: height / 2,
            player1_score: 0,
            player2_score: 0,
            width,
            height,
            paddle_height: 50,
        }
    }

    pub fn reset(&mut self) {
        self.ball_x = self.width / 2;
        self.ball_y = self.height / 2;
        self.ball_dx = if fast_rand() % 2 == 0 { 1 } else { -1 };
        self.ball_dy = if fast_rand() % 2 == 0 { 1 } else { -1 };
        self.player1_y = self.height / 2;
        self.player2_y = self.height / 2;
    }

    pub fn draw(&self) {
        screenwriter().clear();

        match self.game_mode {
            GameMode::Menu => {
                // Centered title
                screenwriter().draw_string_centered(100, "PONG GAME", 0xFF, 0xFF, 0xFF);
                
                // Centered menu options
                screenwriter().draw_string_centered(130, "Press 1: 1 Player", 0xAA, 0xFF, 0xAA);
                screenwriter().draw_string_centered(150, "Press 2: 2 Player", 0xAA, 0xAA, 0xFF);
                
                // Controls information
                screenwriter().draw_string_centered(180, "Controls:", 0xFF, 0xFF, 0xFF);
                screenwriter().draw_string_centered(200, "Player 1: W/S to move", 0xAA, 0xFF, 0xAA);
                screenwriter().draw_string_centered(220, "Player 2: I/K to move", 0xAA, 0xAA, 0xFF);
            }
            GameMode::GameOver => {
                let winner = if self.player1_score > self.player2_score {
                    "Player 1 Wins!"
                } else {
                    "Player 2 Wins!"
                };
                screenwriter().draw_string_centered(100, winner, 0xFF, 0xFF, 0xFF);
                screenwriter().draw_string_centered(130, "Press P to play again", 0xFF, 0xFF, 0xFF);
                screenwriter().draw_string_centered(150, "Press R to return to menu", 0xFF, 0xFF, 0xFF);
            }
            _ => {
                self.draw_game();
            }
        }
    }

    pub fn draw_game(&self) {
        // Draw paddles
        for y in 0..self.paddle_height {
            screenwriter().draw_pixel(10, self.player1_y + y, 0xFF, 0xFF, 0xFF);
            screenwriter().draw_pixel(self.width - 10, self.player2_y + y, 0xFF, 0xFF, 0xFF);
        }

        // Draw ball (larger for better visibility)
        let ball_size = 6; // Radius of 2 pixels (total 5x5)
        for dy in -ball_size..=ball_size {
            for dx in -ball_size..=ball_size {
                screenwriter().draw_pixel(
                    (self.ball_x as isize + dx) as usize,
                    (self.ball_y as isize + dy) as usize,
                    0xFF, 0xFF, 0xFF
                );
            }
        }

        // Draw scores
        let score_text = alloc::format!("{} - {}", self.player1_score, self.player2_score);
        screenwriter().draw_string_centered(20, &score_text, 0xFF, 0xFF, 0xFF);
    }

    pub fn update(&mut self) {
        if self.game_mode != GameMode::OnePlayer && self.game_mode != GameMode::TwoPlayer {
            return;
        }

        // Increase ball speed
        self.ball_x = (self.ball_x as isize + self.ball_dx * 36) as usize;
        self.ball_y = (self.ball_y as isize + self.ball_dy * 36) as usize;

        // Ball collision with top/bottom
        if self.ball_y <= 1 || self.ball_y >= self.height - 2 {
            self.ball_dy = -self.ball_dy;
        }

        // Ball collision with paddles - with explicit type annotations
        let paddle_hit = |paddle_x: usize, paddle_y: usize| -> bool {
            self.ball_x >= paddle_x.saturating_sub(3) &&
            self.ball_x <= paddle_x + 3 &&
            self.ball_y >= paddle_y &&
            self.ball_y <= paddle_y + self.paddle_height
        };

        // Player 1 paddle (left)
        if paddle_hit(10, self.player1_y) {
            self.ball_dx = self.ball_dx.abs(); // Ensure ball moves right
        }
        
        // Player 2 paddle (right)
        if paddle_hit(self.width - 10, self.player2_y) {
            self.ball_dx = -self.ball_dx.abs(); // Ensure ball moves left
        }

        // Scoring
        if self.ball_x <= 0 {
            self.player2_score += 1;
            self.reset();
        } else if self.ball_x >= self.width {
            self.player1_score += 1;
            self.reset();
        }

        // Game over condition
        if self.player1_score >= 1 || self.player2_score >= 1 {
            self.game_mode = GameMode::GameOver;
        }

        // Improved AI for single player
        if self.game_mode == GameMode::OnePlayer {
            let target_y = self.ball_y.saturating_sub(self.paddle_height / 2);
            let ai_paddle_center = self.player2_y + self.paddle_height / 2;
            
            if ai_paddle_center < target_y {
                self.move_paddle(false, false);
            } else if ai_paddle_center > target_y {
                self.move_paddle(false, true);
            }
        }
    }

    pub fn move_paddle(&mut self, is_player1: bool, up: bool) {
        let paddle_y = if is_player1 {
            &mut self.player1_y
        } else {
            &mut self.player2_y
        };

        // Increase paddle movement speed
        let step = 25;
        
        if up {
            *paddle_y = paddle_y.saturating_sub(step);
        } else {
            *paddle_y = (*paddle_y + step).min(self.height - self.paddle_height);
        }
    }
}

// Simple pseudo-random number generator
fn fast_rand() -> u32 {
    use core::sync::atomic::{AtomicU32, Ordering};
    static SEED: AtomicU32 = AtomicU32::new(123456789);
    let mut x = SEED.load(Ordering::Relaxed);
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    SEED.store(x, Ordering::Relaxed);
    x
}

static PONG: spin::Mutex<Pong> = spin::Mutex::new(Pong::new(0, 0));

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    writeln!(serial(), "Entered kernel with boot info: {boot_info:?}").unwrap();
    writeln!(serial(), "Frame Buffer: {:p}", boot_info.framebuffer.as_ref().unwrap().buffer()).unwrap();

    let frame_info = boot_info.framebuffer.as_ref().unwrap().info();
    let framebuffer = boot_info.framebuffer.as_mut().unwrap();
    screen::init(framebuffer);
    
    // Initialize Pong game with screen dimensions
    {
        let mut pong = PONG.lock();
        pong.width = frame_info.width as usize;
        pong.height = frame_info.height as usize;
    }

    for x in 0..frame_info.width {
        screenwriter().draw_pixel(x, frame_info.height-15, 0xff, 0, 0);
        screenwriter().draw_pixel(x, frame_info.height-10, 0, 0xff, 0);
        screenwriter().draw_pixel(x, frame_info.height-5, 0, 0, 0xff);
    }

    for r in boot_info.memory_regions.iter() {
        writeln!(serial(), "{:?} {:?} {:?} {}", r, r.start as *mut u8, r.end as *mut usize, r.end-r.start).unwrap();
    }

    let usable_region = boot_info.memory_regions.iter().filter(|x|x.kind == MemoryRegionKind::Usable).last().unwrap();
    writeln!(serial(), "{usable_region:?}").unwrap();

    let physical_offset = boot_info.physical_memory_offset.take().expect("Failed to find physical memory offset");
    let ptr = (physical_offset + usable_region.start) as *mut u8;
    writeln!(serial(), "Physical memory offset: {:X}; usable range: {:p}", physical_offset, ptr).unwrap();

    let vault = unsafe { slice::from_raw_parts_mut(ptr, 100) };
    vault[0] = 65;
    vault[1] = 66;
    writeln!(Writer, "{} {}", vault[0] as char, vault[1] as char).unwrap();

    let cr3 = Cr3::read().0.start_address().as_u64();
    writeln!(serial(), "CR3 read: {:#x}", cr3).unwrap();

    let cr3_page = unsafe { slice::from_raw_parts_mut((cr3 + physical_offset) as *mut usize, 6) };
    writeln!(serial(), "CR3 Page table virtual address {cr3_page:#p}").unwrap();

    allocator::init_heap((physical_offset + usable_region.start) as usize);

    let rsdp = boot_info.rsdp_addr.take();
    let mut mapper = frame_allocator::init(VirtAddr::new(physical_offset));
    let mut frame_allocator = BootInfoFrameAllocator::new(&boot_info.memory_regions);
    
    gdt::init();

    let x = Box::new(42);
    let y = Box::new(24);
    writeln!(Writer, "x + y = {}", *x + *y).unwrap();
    writeln!(Writer, "{x:#p} {:?}", *x).unwrap();
    writeln!(Writer, "{y:#p} {:?}", *y).unwrap();
    
    writeln!(serial(), "Starting kernel...").unwrap();

    let lapic_ptr = interrupts::init_apic(rsdp.expect("Failed to get RSDP address") as usize, physical_offset, &mut mapper, &mut frame_allocator);
    HandlerTable::new()
        .keyboard(key)
        .timer(tick)
        .startup(start)
        .start(lapic_ptr)
}

fn start() {
    writeln!(Writer, "Hello, world!").unwrap();
    PONG.lock().draw();
}

fn tick() {
    let mut pong = PONG.lock();
    pong.update();
    pong.draw();
}

fn key(key: DecodedKey) {
    let mut pong = PONG.lock();
    
    match key {
        DecodedKey::Unicode('1') if pong.game_mode == GameMode::Menu => {
            pong.reset();
            pong.game_mode = GameMode::OnePlayer;
        }
        DecodedKey::Unicode('2') if pong.game_mode == GameMode::Menu => {
            pong.reset();
            pong.game_mode = GameMode::TwoPlayer;
        }
        DecodedKey::Unicode('r') if pong.game_mode == GameMode::GameOver => {
            pong.player1_score = 0;
            pong.player2_score = 0;
            pong.game_mode = GameMode::Menu;
        }

        DecodedKey::Unicode('p') if pong.game_mode == GameMode::GameOver => {
        // Keep current game mode
        let last_mode = if pong.player1_score >= 1 {
            GameMode::OnePlayer
        } else {
            GameMode::TwoPlayer
        };
    pong.reset();
    pong.player1_score = 0;
    pong.player2_score = 0;
    pong.game_mode = last_mode;
}
        // Faster paddle movement (larger steps)
        DecodedKey::Unicode('w') => pong.move_paddle(true, true),
        DecodedKey::Unicode('s') => pong.move_paddle(true, false),
        DecodedKey::Unicode('i') if pong.game_mode == GameMode::TwoPlayer => pong.move_paddle(false, true),
        DecodedKey::Unicode('k') if pong.game_mode == GameMode::TwoPlayer => pong.move_paddle(false, false),
        _ => {}
    }
    
    pong.draw();
}