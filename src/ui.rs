use crate::{CANVAS, LINE, MUTED, SURFACE, SURFACE_ALT};
use chrono::{DateTime, Local};
use eframe::egui::{self, Color32, FontId, Stroke, Vec2};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, SystemTime};

pub(crate) fn configure_style(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.visuals = egui::Visuals::dark();
    style.visuals.panel_fill = CANVAS;
    style.visuals.window_fill = SURFACE;
    style.visuals.extreme_bg_color = CANVAS;
    style.visuals.faint_bg_color = SURFACE_ALT;
    style.visuals.selection.bg_fill = Color32::from_rgb(27, 62, 105);
    style.visuals.selection.stroke = Stroke::new(1.0_f32, crate::BLUE);
    style.visuals.widgets.inactive.bg_fill = SURFACE_ALT;
    style.visuals.widgets.inactive.weak_bg_fill = SURFACE_ALT;
    style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(28, 43, 61);
    style.visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(28, 43, 61);
    style.visuals.widgets.active.bg_fill = Color32::from_rgb(31, 55, 86);
    style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, LINE);
    style.spacing.item_spacing = Vec2::new(8.0, 7.0);
    style.spacing.button_padding = Vec2::new(10.0, 6.0);
    style.spacing.interact_size.y = 32.0;
    style.visuals.window_corner_radius = 7.0.into();
    style
        .text_styles
        .insert(egui::TextStyle::Body, FontId::proportional(13.0));
    ctx.set_style(style);
}

pub(crate) fn assessment_label(score: u8) -> (&'static str, Color32) {
    match score {
        80..=u8::MAX => ("Удалить", Color32::from_rgb(91, 201, 151)),
        55..=79 => ("Очистка", Color32::from_rgb(244, 174, 91)),
        30..=54 => ("Проверить", Color32::from_rgb(92, 159, 255)),
        _ => ("Обычный", MUTED),
    }
}

pub(crate) fn file_extension(path: &Path) -> String {
    path.extension()
        .and_then(|value| value.to_str())
        .filter(|v| !v.is_empty())
        .unwrap_or("FILE")
        .to_uppercase()
}

pub(crate) fn format_modified(modified: Option<SystemTime>) -> String {
    modified
        .map(DateTime::<Local>::from)
        .map(|date| date.format("%d.%m.%Y %H:%M").to_string())
        .unwrap_or_else(|| "—".to_owned())
}

pub(crate) fn format_file_age(modified: Option<SystemTime>) -> String {
    let Some(age) = modified.and_then(|time| SystemTime::now().duration_since(time).ok()) else {
        return "возраст неизвестен".to_owned();
    };
    let days = age.as_secs() / 86_400;
    match days {
        0 => "сегодня".to_owned(),
        1 => "1 день назад".to_owned(),
        2..=4 => format!("{days} дня назад"),
        5..=30 => format!("{days} дней назад"),
        31..=364 => format!("{} мес. назад", days / 30),
        _ => format!("{} г. назад", days / 365),
    }
}

pub(crate) fn parse_megabytes(value: &str) -> Option<u64> {
    let megabytes: f64 = value.trim().replace(',', ".").parse().ok()?;
    (megabytes.is_finite() && megabytes >= 0.0).then_some((megabytes * 1024.0 * 1024.0) as u64)
}

pub(crate) fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["Б", "КБ", "МБ", "ГБ", "ТБ"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

pub(crate) fn format_count(value: u64) -> String {
    value
        .to_string()
        .as_bytes()
        .rchunks(3)
        .rev()
        .map(|chunk| std::str::from_utf8(chunk).unwrap_or(""))
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn format_duration(duration: Duration) -> String {
    if duration.as_secs() >= 60 {
        format!(
            "{} мин {} с",
            duration.as_secs() / 60,
            duration.as_secs() % 60
        )
    } else {
        format!("{:.1} с", duration.as_secs_f32())
    }
}

pub(crate) fn short_path(path: &Path) -> String {
    let value = path.to_string_lossy();
    if value.chars().count() <= 56 {
        value.into_owned()
    } else {
        format!(
            "…{}",
            value
                .chars()
                .rev()
                .take(55)
                .collect::<String>()
                .chars()
                .rev()
                .collect::<String>()
        )
    }
}

pub(crate) fn reveal_in_file_manager(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Err("Файл больше не существует".to_owned());
    }
    #[cfg(target_os = "windows")]
    Command::new("explorer")
        .arg(format!("/select,{}", path.display()))
        .spawn()
        .map_err(|error| format!("Не удалось открыть Проводник: {error}"))?;
    #[cfg(target_os = "macos")]
    Command::new("open")
        .arg("-R")
        .arg(path)
        .spawn()
        .map_err(|error| format!("Не удалось открыть Finder: {error}"))?;
    #[cfg(target_os = "linux")]
    if let Some(parent) = path.parent() {
        Command::new("xdg-open")
            .arg(parent)
            .spawn()
            .map_err(|error| format!("Не удалось открыть файловый менеджер: {error}"))?;
    }
    Ok(())
}

pub(crate) fn open_path(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Err("Файл больше не существует".to_owned());
    }
    #[cfg(target_os = "windows")]
    Command::new("explorer")
        .arg(path)
        .spawn()
        .map_err(|error| format!("Не удалось открыть файл: {error}"))?;
    #[cfg(target_os = "macos")]
    Command::new("open")
        .arg(path)
        .spawn()
        .map_err(|error| format!("Не удалось открыть файл: {error}"))?;
    #[cfg(target_os = "linux")]
    Command::new("xdg-open")
        .arg(path)
        .spawn()
        .map_err(|error| format!("Не удалось открыть файл: {error}"))?;
    Ok(())
}
