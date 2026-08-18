#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui::Color32;

const INK: Color32 = Color32::from_rgb(234, 239, 247);
const MUTED: Color32 = Color32::from_rgb(137, 151, 172);
const BLUE: Color32 = Color32::from_rgb(92, 159, 255);
const CANVAS: Color32 = Color32::from_rgb(12, 16, 22);
const SURFACE: Color32 = Color32::from_rgb(21, 27, 37);
const SURFACE_ALT: Color32 = Color32::from_rgb(27, 35, 47);
const LINE: Color32 = Color32::from_rgb(43, 53, 68);
const PAGE_SIZE: usize = 50;
const CLEANUP_SCORE_THRESHOLD: u8 = 55;

mod app;
mod cleanup;
mod filtering;
mod games;
mod index;
mod model;
mod scanner;
mod ui;

#[cfg(test)]
mod tests;

fn main() -> eframe::Result<()> {
    app::run()
}
