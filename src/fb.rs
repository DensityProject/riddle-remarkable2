//! Geometry helpers. Drawing lives in surface.rs.

use std::sync::atomic::{AtomicUsize, Ordering};

pub const SCREEN_W: usize = 1620;
pub const SCREEN_H: usize = 2160;

static SCREEN_W_RUNTIME: AtomicUsize = AtomicUsize::new(SCREEN_W);
static SCREEN_H_RUNTIME: AtomicUsize = AtomicUsize::new(SCREEN_H);

pub fn set_screen_size(w: usize, h: usize) {
    SCREEN_W_RUNTIME.store(w.max(1), Ordering::Relaxed);
    SCREEN_H_RUNTIME.store(h.max(1), Ordering::Relaxed);
}

pub fn screen_w() -> usize {
    SCREEN_W_RUNTIME.load(Ordering::Relaxed)
}

pub fn screen_h() -> usize {
    SCREEN_H_RUNTIME.load(Ordering::Relaxed)
}

/// Grow-only pixel bounding box, used to build update/dissolve regions.
#[derive(Clone, Copy, Debug)]
pub struct BBox {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl BBox {
    pub fn empty() -> Self {
        Self {
            x0: i32::MAX,
            y0: i32::MAX,
            x1: i32::MIN,
            y1: i32::MIN,
        }
    }
    pub fn is_empty(&self) -> bool {
        self.x0 > self.x1
    }
    pub fn add(&mut self, x: i32, y: i32, margin: i32) {
        let sw = screen_w() as i32;
        let sh = screen_h() as i32;
        self.x0 = self.x0.min(x - margin).max(0);
        self.y0 = self.y0.min(y - margin).max(0);
        self.x1 = self.x1.max(x + margin).min(sw - 1);
        self.y1 = self.y1.max(y + margin).min(sh - 1);
    }
    pub fn rect(&self) -> (i32, i32, i32, i32) {
        (
            self.x0,
            self.y0,
            self.x1 - self.x0 + 1,
            self.y1 - self.y0 + 1,
        )
    }
}
