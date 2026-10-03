//! Shared raw evdev helpers: the kernel's input_event record and EVIOCGABS.
//!
//! The record is 24 bytes on a 64-bit kernel (Paper Pro) but 16 bytes on a
//! 32-bit one (reMarkable 1/2): `struct timeval` is two native longs. The
//! type/code/value tail is the same 8 bytes on both.

use std::os::fd::RawFd;

pub const EVENT_SIZE: usize = if cfg!(target_pointer_width = "64") { 24 } else { 16 };

/// Split one input_event record into (type, code, value).
pub fn decode(chunk: &[u8]) -> (u16, u16, i32) {
    let t = EVENT_SIZE - 8;
    (
        u16::from_le_bytes(chunk[t..t + 2].try_into().unwrap()),
        u16::from_le_bytes(chunk[t + 2..t + 4].try_into().unwrap()),
        i32::from_le_bytes(chunk[t + 4..t + 8].try_into().unwrap()),
    )
}

/// struct input_absinfo: value, minimum, maximum, fuzz, flat, resolution.
#[repr(C)]
#[derive(Default)]
struct AbsInfo {
    value: i32,
    minimum: i32,
    maximum: i32,
    fuzz: i32,
    flat: i32,
    resolution: i32,
}

/// EVIOCGABS(code) = _IOR('E', 0x40 + code, struct input_absinfo).
fn eviocgabs(code: u16) -> libc::c_ulong {
    (2 << 30) | ((std::mem::size_of::<AbsInfo>() as libc::c_ulong) << 16) | (0x45 << 8) | (0x40 + code as libc::c_ulong)
}

/// The axis range the driver reports, or None if the ioctl fails.
pub fn abs_range(fd: RawFd, code: u16) -> Option<(i32, i32)> {
    let mut info = AbsInfo::default();
    let rc = unsafe { libc::ioctl(fd, eviocgabs(code) as _, &mut info as *mut AbsInfo) };
    (rc == 0 && info.maximum > info.minimum).then_some((info.minimum, info.maximum))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eviocgabs_matches_kernel_encoding() {
        // _IOR('E', 0x40, 24 bytes) from <linux/input.h>.
        assert_eq!(eviocgabs(0), 0x8018_4540);
        assert_eq!(eviocgabs(24), 0x8018_4558);
    }

    #[test]
    fn decode_reads_the_tail() {
        let mut rec = vec![0u8; EVENT_SIZE];
        let t = EVENT_SIZE - 8;
        rec[t..t + 2].copy_from_slice(&3u16.to_le_bytes());
        rec[t + 2..t + 4].copy_from_slice(&1u16.to_le_bytes());
        rec[t + 4..t + 8].copy_from_slice(&(-7i32).to_le_bytes());
        assert_eq!(decode(&rec), (3, 1, -7));
    }
}
