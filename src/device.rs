// SPDX-License-Identifier: MIT
use crate::Result;
use std::ffi::{c_char, c_void, CString};
#[repr(C)]
#[derive(Default, Clone, Debug)]
pub struct Info {
    pub driver: [u8; 16],
    pub card: [u8; 32],
    pub bus: [u8; 32],
    pub caps: u32,
    pub output: u32,
}
#[repr(C)]
#[derive(Default, Copy, Clone, Debug)]
pub struct Mode {
    pub width: u32,
    pub height: u32,
    pub interlaced: u32,
    pub total_lines: u32,
    pub num: u64,
    pub den: u64,
    pub index: u32,
    pub reduced: u32,
}
impl Mode {
    pub fn fps(self) -> f64 {
        self.num as f64 / self.den as f64
    }
    pub fn name(self) -> String {
        format!(
            "{}x{}{}{:.3}",
            self.width,
            self.height,
            if self.interlaced != 0 { 'i' } else { 'p' },
            self.fps() * if self.interlaced != 0 { 2.0 } else { 1.0 }
        )
    }
}
#[repr(C)]
#[derive(Default, Copy, Clone, Debug)]
pub struct Layout {
    pub width: u32,
    pub height: u32,
    pub fourcc: u32,
    pub stride: u32,
    pub sizes: [u32; 5],
}
#[repr(C)]
pub struct Frame {
    pub index: u32,
    pub sequence: u32,
    pub flags: u32,
    pub events: u32,
    pub timestamp: u64,
    pub data: [*mut u8; 5],
    pub len: [u32; 5],
}
impl Default for Frame {
    fn default() -> Self {
        Self {
            index: 0,
            sequence: 0,
            flags: 0,
            events: 0,
            timestamp: 0,
            data: [std::ptr::null_mut(); 5],
            len: [0; 5],
        }
    }
}
impl Frame {
    pub fn planes(&self) -> [&[u8]; 5] {
        std::array::from_fn(|p| {
            if self.len[p] == 0 {
                &[]
            } else {
                unsafe { std::slice::from_raw_parts(self.data[p], self.len[p] as usize) }
            }
        })
    }
    pub fn planes_mut(&mut self) -> [&mut [u8]; 5] {
        std::array::from_fn(|p| {
            if self.len[p] == 0 {
                &mut []
            } else {
                unsafe { std::slice::from_raw_parts_mut(self.data[p], self.len[p] as usize) }
            }
        })
    }
}
#[cfg(target_os = "linux")]
extern "C" {
    fn vv_contract(path: *const c_char, output: u32) -> i32;
    fn vv_probe(path: *const c_char, i: *mut Info) -> i32;
    fn vv_mode(path: *const c_char, index: u32, m: *mut Mode) -> i32;
    fn vv_format(path: *const c_char, index: u32, output: u32, f: *mut u32) -> i32;
    fn vv_open(
        path: *const c_char,
        output: u32,
        index: i32,
        reduced: u32,
        fourcc: u32,
        userptr: u32,
        d: *mut *mut c_void,
        l: *mut Layout,
        m: *mut Mode,
    ) -> i32;
    fn vv_close(d: *mut c_void);
    fn vv_buffer(d: *mut c_void, index: u32, f: *mut Frame) -> i32;
    fn vv_queue(d: *mut c_void, f: *const Frame) -> i32;
    fn vv_start(d: *mut c_void) -> i32;
    fn vv_next(d: *mut c_void, timeout: u32, f: *mut Frame) -> i32;
}
// Non-Linux builds support software validation and unit tests only.
#[cfg(not(target_os = "linux"))]
mod stub {
    use super::*;
    pub unsafe fn vv_contract(_: *const c_char, _: u32) -> i32 {
        -38
    }
    pub unsafe fn vv_probe(_: *const c_char, _: *mut Info) -> i32 {
        -38
    }
    pub unsafe fn vv_mode(_: *const c_char, _: u32, _: *mut Mode) -> i32 {
        -38
    }
    pub unsafe fn vv_format(_: *const c_char, _: u32, _: u32, _: *mut u32) -> i32 {
        -38
    }
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn vv_open(
        _: *const c_char,
        _: u32,
        _: i32,
        _: u32,
        _: u32,
        _: u32,
        _: *mut *mut c_void,
        _: *mut Layout,
        _: *mut Mode,
    ) -> i32 {
        -38
    }
    pub unsafe fn vv_close(_: *mut c_void) {}
    pub unsafe fn vv_buffer(_: *mut c_void, _: u32, _: *mut Frame) -> i32 {
        -38
    }
    pub unsafe fn vv_queue(_: *mut c_void, _: *const Frame) -> i32 {
        -38
    }
    pub unsafe fn vv_start(_: *mut c_void) -> i32 {
        -38
    }
    pub unsafe fn vv_next(_: *mut c_void, _: u32, _: *mut Frame) -> i32 {
        -38
    }
}
#[cfg(not(target_os = "linux"))]
use stub::*;
fn path(s: &str) -> Result<CString> {
    CString::new(s).map_err(|e| e.to_string())
}
pub fn check(r: i32, op: &str) -> Result<i32> {
    if r < 0 {
        Err(format!(
            "{op}: {} (errno {})",
            std::io::Error::from_raw_os_error(-r),
            -r
        ))
    } else {
        Ok(r)
    }
}
pub fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes.split(|b| *b == 0).next().unwrap_or(bytes)).into_owned()
}
pub fn probe(p: &str) -> Result<Info> {
    let mut i = Info::default();
    check(unsafe { vv_probe(path(p)?.as_ptr(), &mut i) }, "QUERYCAP")?;
    Ok(i)
}
pub fn modes(p: &str) -> Result<Vec<Mode>> {
    let p = path(p)?;
    let mut out = vec![];
    for index in 0..4096 {
        let mut m = Mode::default();
        let r = unsafe { vv_mode(p.as_ptr(), index, &mut m) };
        if r == -22 || r == -25 {
            break;
        }
        check(r, "ENUM_DV_TIMINGS")?;
        if m.num == 0 || m.den == 0 {
            return Err("invalid timing rational".into());
        }
        out.push(m);
        if m.reduced != 0 {
            m.num *= 1000;
            m.den *= 1001;
            m.reduced = 2;
            out.push(m);
        }
    }
    Ok(out)
}
pub fn formats(p: &str, output: bool) -> Result<Vec<u32>> {
    let p = path(p)?;
    let mut out = vec![];
    for index in 0..256 {
        let mut f = 0;
        let r = unsafe { vv_format(p.as_ptr(), index, output as u32, &mut f) };
        if r == -22 || r == -25 {
            break;
        }
        check(r, "ENUM_FMT")?;
        out.push(f);
    }
    Ok(out)
}
pub struct Device {
    raw: *mut c_void,
    pub layout: Layout,
    pub mode: Mode,
    pub count: u32,
}
impl Device {
    pub fn open(
        p: &str,
        output: bool,
        mode: Option<Mode>,
        fourcc: u32,
        memory: u32,
    ) -> Result<Self> {
        let mut d = Self {
            raw: std::ptr::null_mut(),
            layout: Layout::default(),
            mode: Mode::default(),
            count: 0,
        };
        d.count = check(
            unsafe {
                vv_open(
                    path(p)?.as_ptr(),
                    output as u32,
                    mode.map_or(-1, |m| m.index as i32),
                    mode.map_or(0, |m| (m.reduced == 2) as u32),
                    fourcc,
                    memory,
                    &mut d.raw,
                    &mut d.layout,
                    &mut d.mode,
                )
            },
            "configure/allocate",
        )? as u32;
        Ok(d)
    }
    pub fn buffer(&self, index: u32) -> Result<Frame> {
        let mut f = Frame::default();
        check(unsafe { vv_buffer(self.raw, index, &mut f) }, "buffer")?;
        Ok(f)
    }
    pub fn queue(&self, f: &Frame) -> Result<()> {
        check(unsafe { vv_queue(self.raw, f) }, "QBUF")?;
        Ok(())
    }
    pub fn start(&self) -> Result<()> {
        check(unsafe { vv_start(self.raw) }, "STREAMON")?;
        Ok(())
    }
    pub fn next(&self, timeout: u32) -> Result<Frame> {
        let mut f = Frame::default();
        check(unsafe { vv_next(self.raw, timeout, &mut f) }, "poll/DQBUF")?;
        Ok(f)
    }
}
impl Drop for Device {
    fn drop(&mut self) {
        if !self.raw.is_null() {
            unsafe { vv_close(self.raw) }
        }
    }
}

pub fn contract(p: &str, output: bool) -> Result<()> {
    check(
        unsafe { vv_contract(path(p)?.as_ptr(), output as u32) },
        "five-plane ABI",
    )?;
    Ok(())
}
