use std::env;
use std::ffi::{c_char, c_uint, c_void};
use std::fmt::Write as _;
use std::ptr;

#[repr(C)]
struct GbmDevice { _private: [u8; 0] }
#[repr(C)]
struct GbmBo { _private: [u8; 0] }

type Create = unsafe extern "C" fn(
    *mut GbmDevice, c_uint, c_uint, c_uint, *const u64, c_uint
) -> *mut GbmBo;

type Create2 = unsafe extern "C" fn(
    *mut GbmDevice, c_uint, c_uint, c_uint, *const u64, c_uint, c_uint
) -> *mut GbmBo;

type GetModifier = unsafe extern "C" fn(*mut GbmBo) -> u64;

const LINEAR: u64 = 0;
const RTLD_NEXT: *mut c_void = -1isize as *mut c_void;

#[link(name = "dl")]
extern "C" {
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode { Off, Force, Prefer }

fn mode() -> Mode {
    match env::var("IGPU_LINEAR_BRIDGE_MODE")
        .unwrap_or_default().to_ascii_lowercase().as_str()
    {
        "force" | "1" | "true" => Mode::Force,
        "prefer" => Mode::Prefer,
        _ => Mode::Off,
    }
}

fn log_enabled() -> bool {
    env::var("IGPU_LINEAR_BRIDGE_LOG")
        .map(|v| v != "0" && !v.is_empty())
        .unwrap_or(true)
}

fn log(message: &str) {
    if log_enabled() {
        eprintln!("[igpu-linear-bridge] {message}");
    }
}

unsafe fn next(name: &'static [u8]) -> *mut c_void {
    unsafe { dlsym(RTLD_NEXT, name.as_ptr().cast()) }
}

unsafe fn actual_modifier(bo: *mut GbmBo) -> Option<u64> {
    let symbol = unsafe { next(b"gbm_bo_get_modifier\0") };
    if symbol.is_null() { return None; }
    let get: GetModifier = unsafe { std::mem::transmute(symbol) };
    Some(unsafe { get(bo) })
}

fn modifier_list(ptr: *const u64, count: c_uint) -> String {
    if ptr.is_null() || count == 0 {
        return format!("count={count}, list=<empty>");
    }
    let mut list = String::new();
    for i in 0..count.min(16) {
        let value = unsafe { *ptr.add(i as usize) };
        if i != 0 { list.push(','); }
        let _ = write!(list, "0x{value:016x}");
    }
    if count > 16 { let _ = write!(list, ",...(+{})", count - 16); }
    format!("count={count}, list=[{list}]")
}

fn log_result(label: &str, bo: *mut GbmBo) {
    if bo.is_null() {
        log(&format!("{label}: allocation failed"));
    } else {
        let modifier = unsafe { actual_modifier(bo) };
        log(&format!(
            "{label}: allocation succeeded; actual modifier={}",
            modifier.map(|v| format!("0x{v:016x}"))
                .unwrap_or_else(|| "<query unavailable>".to_string())
        ));
    }
}

#[no_mangle]
pub unsafe extern "C" fn gbm_bo_create_with_modifiers(
    device: *mut GbmDevice,
    width: c_uint,
    height: c_uint,
    format: c_uint,
    modifiers: *const u64,
    count: c_uint,
) -> *mut GbmBo {
    let symbol = unsafe { next(b"gbm_bo_create_with_modifiers\0") };
    if symbol.is_null() {
        log("ERROR: real gbm_bo_create_with_modifiers symbol not found");
        return ptr::null_mut();
    }
    let real: Create = unsafe { std::mem::transmute(symbol) };
    let selected = mode();
    if selected == Mode::Off {
        return unsafe { real(device, width, height, format, modifiers, count) };
    }

    log(&format!(
        "create_with_modifiers {width}x{height} format=0x{format:08x}; requested {}",
        modifier_list(modifiers, count)
    ));
    let linear = [LINEAR];
    let bo = unsafe { real(device, width, height, format, linear.as_ptr(), 1) };
    if !bo.is_null() {
        log_result("LINEAR", bo);
        return bo;
    }
    log("LINEAR allocation failed");

    if selected == Mode::Prefer {
        log("prefer mode: retrying original modifier list");
        let fallback = unsafe { real(device, width, height, format, modifiers, count) };
        log_result("original-list fallback", fallback);
        return fallback;
    }
    ptr::null_mut()
}

#[no_mangle]
pub unsafe extern "C" fn gbm_bo_create_with_modifiers2(
    device: *mut GbmDevice,
    width: c_uint,
    height: c_uint,
    format: c_uint,
    modifiers: *const u64,
    count: c_uint,
    flags: c_uint,
) -> *mut GbmBo {
    let symbol = unsafe { next(b"gbm_bo_create_with_modifiers2\0") };
    if symbol.is_null() {
        log("ERROR: real gbm_bo_create_with_modifiers2 symbol not found");
        return ptr::null_mut();
    }
    let real: Create2 = unsafe { std::mem::transmute(symbol) };
    let selected = mode();
    if selected == Mode::Off {
        return unsafe { real(device, width, height, format, modifiers, count, flags) };
    }

    log(&format!(
        "create_with_modifiers2 {width}x{height} format=0x{format:08x} flags=0x{flags:08x}; requested {}",
        modifier_list(modifiers, count)
    ));
    let linear = [LINEAR];
    let bo = unsafe { real(device, width, height, format, linear.as_ptr(), 1, flags) };
    if !bo.is_null() {
        log_result("LINEAR2", bo);
        return bo;
    }
    log("LINEAR allocation2 failed");

    if selected == Mode::Prefer {
        log("prefer mode: retrying original modifier list");
        let fallback = unsafe { real(device, width, height, format, modifiers, count, flags) };
        log_result("original-list fallback2", fallback);
        return fallback;
    }
    ptr::null_mut()
}
