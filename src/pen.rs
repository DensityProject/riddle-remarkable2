//! Raw evdev pen input: the full digitizer, bypassing Qt's filtered view.
//! Gives us 0-4096 pressure, tilt, hover, and the eraser tip (BTN_TOOL_RUBBER),
//! at the hardware event rate.
//!
//! The device is grabbed (EVIOCGRAB) while the diary is open so xochitl
//! doesn't also react to the pen; released automatically on close/exit.

use std::io;
use std::os::fd::RawFd;

use crate::fb::{screen_h, screen_w};

// Digitizer axis ranges on the Paper Pro ("Elan marker input").
const DIGI_MIN_X: i32 = 0;
const DIGI_MAX_X: i32 = 11180;
const DIGI_MIN_Y: i32 = 0;
const DIGI_MAX_Y: i32 = 15340;
pub const MAX_PRESSURE: i32 = 4096;

const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;
const EV_ABS: u16 = 3;
const SYN_REPORT: u16 = 0;
const ABS_X: u16 = 0;
const ABS_Y: u16 = 1;
const ABS_PRESSURE: u16 = 24;
const BTN_TOOL_PEN: u16 = 320;
const BTN_TOOL_RUBBER: u16 = 321;
const BTN_TOUCH: u16 = 330;

const EVIOCGRAB: libc::c_ulong = 0x40044590;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Pen,
    Eraser,
}

#[derive(Debug, Clone, Copy)]
pub struct PenSample {
    /// Screen coordinates.
    pub x: i32,
    pub y: i32,
    /// 0..4096
    pub pressure: i32,
    pub tool: Tool,
    pub touching: bool,
    /// True from tool-in-range until the pen leaves the digitizer.
    pub proximity: bool,
}

pub struct PenDevice {
    fd: RawFd,
    raw_min_x: i32,
    raw_max_x: i32,
    raw_min_y: i32,
    raw_max_y: i32,
    raw_max_pressure: i32,
    // Accumulated state between SYN_REPORTs.
    raw_x: i32,
    raw_y: i32,
    pressure: i32,
    tool: Tool,
    touching: bool,
    pen_in_range: bool,
    rubber_in_range: bool,
    proximity: bool,
    dirty: bool,
}

impl PenDevice {
    /// Find and grab the marker input device.
    pub fn open() -> io::Result<Self> {
        let (path, event_i) = find_marker_device()?;
        let (raw_min_x, raw_max_x) =
            read_abs_min_max(event_i, ABS_X).unwrap_or((DIGI_MIN_X, DIGI_MAX_X));
        let (raw_min_y, raw_max_y) =
            read_abs_min_max(event_i, ABS_Y).unwrap_or((DIGI_MIN_Y, DIGI_MAX_Y));
        let (_, raw_max_pressure) =
            read_abs_min_max(event_i, ABS_PRESSURE).unwrap_or((0, MAX_PRESSURE));
        let cpath = std::ffi::CString::new(path.clone()).unwrap();
        let fd = unsafe { libc::open(cpath.as_ptr(), libc::O_RDONLY | libc::O_NONBLOCK) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let grab = unsafe { libc::ioctl(fd, EVIOCGRAB, 1i32) };
        if grab != 0 {
            eprintln!(
                "riddle: warning: EVIOCGRAB failed ({}) — xochitl will also see the pen",
                io::Error::last_os_error()
            );
        }
        eprintln!("riddle: pen device {path} opened (grabbed: {})", grab == 0);
        Ok(Self {
            fd,
            raw_min_x,
            raw_max_x,
            raw_min_y,
            raw_max_y,
            raw_max_pressure,
            raw_x: 0,
            raw_y: 0,
            pressure: 0,
            tool: Tool::Pen,
            touching: false,
            pen_in_range: false,
            rubber_in_range: false,
            proximity: false,
            dirty: false,
        })
    }

    pub fn raw_fd(&self) -> RawFd {
        self.fd
    }

    /// Drain all pending events; returns one sample per SYN_REPORT frame
    /// that changed state.
    pub fn drain(&mut self) -> Vec<PenSample> {
        let mut out = Vec::new();
        // input_event on 64-bit: struct timeval (16) + type u16 + code u16 + value i32.
        let mut buf = [0u8; 24 * 64];
        loop {
            let n =
                unsafe { libc::read(self.fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
            if n <= 0 {
                break;
            }
            for chunk in buf[..n as usize].chunks_exact(24) {
                let etype = u16::from_le_bytes(chunk[16..18].try_into().unwrap());
                let code = u16::from_le_bytes(chunk[18..20].try_into().unwrap());
                let value = i32::from_le_bytes(chunk[20..24].try_into().unwrap());
                match (etype, code) {
                    (EV_ABS, ABS_X) => {
                        self.raw_x = value;
                        self.dirty = true;
                    }
                    (EV_ABS, ABS_Y) => {
                        self.raw_y = value;
                        self.dirty = true;
                    }
                    (EV_ABS, ABS_PRESSURE) => {
                        self.pressure = value;
                        self.dirty = true;
                    }
                    (EV_KEY, BTN_TOOL_PEN) => {
                        self.pen_in_range = value == 1;
                        if self.pen_in_range {
                            self.tool = Tool::Pen;
                        }
                        self.proximity = self.pen_in_range || self.rubber_in_range;
                        self.dirty = true;
                    }
                    (EV_KEY, BTN_TOOL_RUBBER) => {
                        self.rubber_in_range = value == 1;
                        if self.rubber_in_range {
                            self.tool = Tool::Eraser;
                        }
                        self.proximity = self.pen_in_range || self.rubber_in_range;
                        self.dirty = true;
                    }
                    (EV_KEY, BTN_TOUCH) => {
                        self.touching = value == 1;
                        self.dirty = true;
                    }
                    (EV_SYN, SYN_REPORT) => {
                        if self.dirty {
                            self.dirty = false;
                            let sw = screen_w() as i32;
                            let sh = screen_h() as i32;
                            let xr = (self.raw_max_x - self.raw_min_x).max(1);
                            let yr = (self.raw_max_y - self.raw_min_y).max(1);
                            let pr = self.raw_max_pressure.max(1);
                            out.push(PenSample {
                                x: ((self.raw_x - self.raw_min_x) * (sw - 1) / xr).clamp(0, sw - 1),
                                y: ((self.raw_y - self.raw_min_y) * (sh - 1) / yr).clamp(0, sh - 1),
                                pressure: (self.pressure * MAX_PRESSURE / pr)
                                    .clamp(0, MAX_PRESSURE),
                                tool: self.tool,
                                touching: self.touching,
                                proximity: self.proximity,
                            });
                        }
                    }
                    _ => {}
                }
            }
        }
        out
    }
}

impl Drop for PenDevice {
    fn drop(&mut self) {
        unsafe {
            libc::ioctl(self.fd, EVIOCGRAB, 0i32);
            libc::close(self.fd);
        }
    }
}

fn find_marker_device() -> io::Result<(String, usize)> {
    for i in 0..8 {
        let name_path = format!("/sys/class/input/event{i}/device/name");
        if let Ok(name) = std::fs::read_to_string(&name_path) {
            if name.to_lowercase().contains("marker") {
                return Ok((format!("/dev/input/event{i}"), i));
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "no marker input device found",
    ))
}

fn read_abs_min_max(event_i: usize, code: u16) -> Option<(i32, i32)> {
    let path = format!("/sys/class/input/event{event_i}/device/abs/abs{code}");
    let raw = std::fs::read_to_string(path).ok()?;
    let mut parts = raw.split_whitespace();
    let min = parts.next()?.parse().ok()?;
    let max = parts.next()?.parse().ok()?;
    Some((min, max))
}
