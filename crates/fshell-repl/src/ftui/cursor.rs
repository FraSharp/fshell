// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_core::theme::Theme;
use ratatui::style::{Color, Modifier, Style};
use std::time::Instant;

use crate::theme_ext::ThemeColorRatatui;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Coord {
    pub x: u16,
    pub y: u16,
}

impl Coord {
    pub fn new(x: u16, y: u16) -> Self {
        Self { x, y }
    }

    pub fn interpolate(&self, target: &Self, t: f32) -> Self {
        let x = self.x as f32 + (target.x as f32 - self.x as f32) * t;
        let y = self.y as f32 + (target.y as f32 - self.y as f32) * t;
        Self::new(x.round() as u16, y.round() as u16)
    }

    pub fn abs_diff(&self, other: &Self) -> u16 {
        self.x.abs_diff(other.x).max(self.y.abs_diff(other.y))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorEasing {
    #[default]
    Linear,
    InOutQuad,
    OutQuad,
    InOutSine,
    OutElastic,
}

impl CursorEasing {
    pub fn apply(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            CursorEasing::Linear => t,
            CursorEasing::InOutQuad => {
                if t < 0.5 {
                    2.0 * t * t
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(2) * 0.5
                }
            }
            CursorEasing::OutQuad => 1.0 - (1.0 - t) * (1.0 - t),
            CursorEasing::InOutSine => -((std::f32::consts::PI * t).cos() - 1.0) * 0.5,
            CursorEasing::OutElastic => {
                if t == 0.0 || t == 1.0 {
                    t
                } else {
                    let c4 = (2.0 * std::f32::consts::PI) / 3.0;
                    (2.0f32.powf(-10.0 * t) * ((t * 10.0 - 0.75) * c4).sin()) + 1.0
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorEffect {
    #[default]
    Fade,
    Blink,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorStyleConfig {
    #[default]
    Default, // Classic grey block
    Reverse, // Invert background/foreground
}

#[derive(Debug, Clone)]
pub struct CursorConfig {
    pub interpolate: Option<f32>, // Speed, e.g., Some(12.0). None to disable
    pub interpolate_easing: CursorEasing,
    pub style: CursorStyleConfig,
    pub effect: CursorEffect,
    pub effect_speed: f32,
    pub effect_easing: CursorEasing,
}

impl Default for CursorConfig {
    fn default() -> Self {
        Self {
            interpolate: None,
            interpolate_easing: CursorEasing::default(),
            style: CursorStyleConfig::Default,
            effect: CursorEffect::None,
            effect_speed: 1.0,
            effect_easing: CursorEasing::default(),
        }
    }
}

pub struct CursorState {
    pub target_pos: Coord,
    pub prev_pos: Coord,
    pub time_of_change: Instant,
}

impl Default for CursorState {
    fn default() -> Self {
        Self::new()
    }
}

impl CursorState {
    pub fn new() -> Self {
        Self {
            target_pos: Coord::new(0, 0),
            prev_pos: Coord::new(0, 0),
            time_of_change: Instant::now(),
        }
    }

    pub fn update_logical_pos(&mut self, new_pos: Coord, config: &CursorConfig) {
        if new_pos != self.target_pos {
            self.prev_pos = self.get_render_pos(config);
            self.time_of_change = Instant::now();
            self.target_pos = new_pos;
        }
    }

    pub fn get_render_pos(&self, config: &CursorConfig) -> Coord {
        match config.interpolate {
            None => self.target_pos,
            Some(speed) => {
                let elapsed = self.time_of_change.elapsed().as_secs_f32();
                let factor = elapsed * speed;

                // If movement is very small, jump instantly to avoid micro-jitters
                if self.prev_pos.abs_diff(&self.target_pos) <= 1 {
                    return self.target_pos;
                }

                let t = factor.min(1.0);
                let eased_t = config.interpolate_easing.apply(t);
                self.prev_pos.interpolate(&self.target_pos, eased_t)
            }
        }
    }

    pub fn get_style(&self, focused: bool, config: &CursorConfig, theme: &Theme) -> Option<Style> {
        if config.interpolate.is_none() && config.effect == CursorEffect::None {
            return None;
        }
        let intensity = self.compute_intensity(focused, config)?;
        Some(self.build_style(intensity, config.style, theme))
    }

    fn compute_intensity(&self, focused: bool, config: &CursorConfig) -> Option<f32> {
        if !focused {
            return Some(0.3); // Steady dim when unfocused
        }

        match config.effect {
            CursorEffect::None => Some(1.0),
            CursorEffect::Fade => {
                let elapsed = self.time_of_change.elapsed().as_secs_f32();
                let raw = (elapsed * 3.5 * config.effect_speed).sin() * 0.5 + 0.5;
                let eased = config.effect_easing.apply(raw);
                Some(eased * 0.7 + 0.3) // Never fully black
            }
            CursorEffect::Blink => {
                let elapsed = self.time_of_change.elapsed().as_secs_f32();
                let phase = (elapsed * config.effect_speed * 2.0).fract();
                if phase < 0.5 { Some(1.0) } else { None }
            }
        }
    }

    fn build_style(&self, intensity: f32, style: CursorStyleConfig, theme: &Theme) -> Style {
        match style {
            CursorStyleConfig::Default => {
                let base = theme.prompt.cursor_color.to_ratatui_color();
                // Apply intensity by blending with background
                let (r, g, b) = match base {
                    Color::Rgb(r, g, b) => (r, g, b),
                    _ => (128, 128, 128),
                };
                let factor = intensity;
                let bg = theme.prompt.cursor_unfocused.to_ratatui_color();
                let (br, bg_g, bb) = match bg {
                    Color::Rgb(r, g, b) => (r, g, b),
                    _ => (40, 40, 40),
                };
                let v_r = (br as f32 + (r as f32 - br as f32) * factor) as u8;
                let v_g = (bg_g as f32 + (g as f32 - bg_g as f32) * factor) as u8;
                let v_b = (bb as f32 + (b as f32 - bb as f32) * factor) as u8;
                Style::default().bg(Color::Rgb(v_r, v_g, v_b))
            }
            CursorStyleConfig::Reverse => Style::default().add_modifier(Modifier::REVERSED),
        }
    }
}
