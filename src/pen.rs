//! Raw evdev pen input: the full digitizer, bypassing Qt's filtered view.
//! Gives us 0-4096 pressure, tilt, hover, and the eraser tip (BTN_TOOL_RUBBER),
//! at the hardware event rate.
//!
//! The device is grabbed (EVIOCGRAB) while the diary is open so xochitl
//! doesn't also react to the pen; released automatically on close/exit.

use std::io;
use std::os::fd::RawFd;

use crate::fb::{screen_h, screen_w};

use crate::evdev;

// Fallback digitizer axis ranges, used only if EVIOCGABS fails.
// Paper Pro ("Elan marker input"): axes already match the screen.
const PP_MAX_X: i32 = 11180;
const PP_MAX_Y: i32 = 15340;
// reMarkable 1/2 ("Wacom I2C Digitizer"): mounted rotated 90 degrees, so raw X
// runs down the screen (inverted) and raw Y runs across it.
const WACOM_MAX_X: i32 = 20967;
const WACOM_MAX_Y: i32 = 15725;
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
    /// Wacom digitizer (rM1/rM2): swap axes and invert raw X.
    rotated: bool,
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
        let (path, wacom) = find_marker_device()?;
        let cpath = std::ffi::CString::new(path.clone()).unwrap();
        let fd = unsafe { libc::open(cpath.as_ptr(), libc::O_RDONLY | libc::O_NONBLOCK) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let (fx, fy) = if wacom { (WACOM_MAX_X, WACOM_MAX_Y) } else { (PP_MAX_X, PP_MAX_Y) };
        let (raw_min_x, raw_max_x) = evdev::abs_range(fd, ABS_X).unwrap_or((0, fx));
        let (raw_min_y, raw_max_y) = evdev::abs_range(fd, ABS_Y).unwrap_or((0, fy));
        let (_, raw_max_pressure) =
            evdev::abs_range(fd, ABS_PRESSURE).unwrap_or((0, MAX_PRESSURE));
        // RIDDLE_PEN_ROTATE=0/1 overrides the per-digitizer default.
        let rotated = match std::env::var("RIDDLE_PEN_ROTATE").as_deref() {
            Ok("1") => true,
            Ok("0") => false,
            _ => wacom,
        };
        eprintln!(
            "riddle: pen x {raw_min_x}..{raw_max_x} y {raw_min_y}..{raw_max_y} \
             pressure 0..{raw_max_pressure} rotated {rotated}"
        );
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
            rotated,
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
        let mut buf = [0u8; evdev::EVENT_SIZE * 64];
        loop {
            let n =
                unsafe { libc::read(self.fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
            if n <= 0 {
                break;
            }
            for chunk in buf[..n as usize].chunks_exact(evdev::EVENT_SIZE) {
                let (etype, code, value) = evdev::decode(chunk);
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
                            let (x, y) = self.to_screen(screen_w() as i32, screen_h() as i32);
                            let pr = self.raw_max_pressure.max(1);
                            out.push(PenSample {
                                x,
                                y,
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

    /// Map the current raw position to screen pixels.
    fn to_screen(&self, sw: i32, sh: i32) -> (i32, i32) {
        let xr = (self.raw_max_x - self.raw_min_x).max(1);
        let yr = (self.raw_max_y - self.raw_min_y).max(1);
        let (x, y) = if self.rotated {
            (
                (self.raw_y - self.raw_min_y) * (sw - 1) / yr,
                (self.raw_max_x - self.raw_x) * (sh - 1) / xr,
            )
        } else {
            (
                (self.raw_x - self.raw_min_x) * (sw - 1) / xr,
                (self.raw_y - self.raw_min_y) * (sh - 1) / yr,
            )
        };
        (x.clamp(0, sw - 1), y.clamp(0, sh - 1))
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

/// Find the pen digitizer. Returns its path and whether it is the rM1/rM2
/// Wacom panel (which needs the rotated mapping).
fn find_marker_device() -> io::Result<(String, bool)> {
    for i in 0..8 {
        let name_path = format!("/sys/class/input/event{i}/device/name");
        if let Ok(name) = std::fs::read_to_string(&name_path) {
            let name = name.to_lowercase();
            let wacom = name.contains("wacom");
            if name.contains("marker") || wacom {
                return Ok((format!("/dev/input/event{i}"), wacom));
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "no pen digitizer (marker/wacom) input device found",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(rotated: bool, max_x: i32, max_y: i32, raw_x: i32, raw_y: i32) -> PenDevice {
        PenDevice {
            fd: -1,
            raw_min_x: 0,
            raw_max_x: max_x,
            raw_min_y: 0,
            raw_max_y: max_y,
            raw_max_pressure: MAX_PRESSURE,
            rotated,
            raw_x,
            raw_y,
            pressure: 0,
            tool: Tool::Pen,
            touching: false,
            pen_in_range: false,
            rubber_in_range: false,
            proximity: false,
            dirty: false,
        }
    }

    #[test]
    fn paper_pro_maps_straight() {
        let d = dev(false, PP_MAX_X, PP_MAX_Y, PP_MAX_X, 0);
        assert_eq!(d.to_screen(1620, 2160), (1619, 0));
        std::mem::forget(d);
    }

    #[test]
    fn wacom_swaps_axes_and_inverts_raw_x() {
        // Raw X max is the top of the screen; raw Y max is the right edge.
        let d = dev(true, WACOM_MAX_X, WACOM_MAX_Y, WACOM_MAX_X, WACOM_MAX_Y);
        assert_eq!(d.to_screen(1404, 1872), (1403, 0));
        std::mem::forget(d);
        let d = dev(true, WACOM_MAX_X, WACOM_MAX_Y, 0, 0);
        assert_eq!(d.to_screen(1404, 1872), (0, 1871));
        std::mem::forget(d);
    }
}
