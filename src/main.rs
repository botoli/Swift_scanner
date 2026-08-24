#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui::Color32;

const INK: Color32 = Color32::from_rgb(220, 229, 242);
const MUTED: Color32 = Color32::from_rgb(125, 141, 164);
const BLUE: Color32 = Color32::from_rgb(79, 141, 255);
const CANVAS: Color32 = Color32::from_rgb(9, 15, 23);
const SURFACE: Color32 = Color32::from_rgb(17, 26, 38);
const SURFACE_ALT: Color32 = Color32::from_rgb(22, 33, 47);
const LINE: Color32 = Color32::from_rgb(35, 49, 67);
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
