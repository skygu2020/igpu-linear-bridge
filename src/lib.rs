use std::env;
use std::ffi::{c_char, c_uint, c_void};
use std::fmt::Write as _;
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};

static CREATE_CALLS: AtomicU64 = AtomicU64::new(0);
static CREATE_MODIFIERS_CALLS: AtomicU64 = AtomicU64::new(0);
static CREATE_MODIFIERS2_CALLS: AtomicU64 = AtomicU64::new(0);
static OFF_CALLS: AtomicU64 = AtomicU64::new(0);
static LINEAR_ATTEMPTS: AtomicU64 = AtomicU64::new(0);
static LINEAR_SUCCESSES: AtomicU64 = AtomicU64::new(0);
static LINEAR_FAILURES: AtomicU64 = AtomicU64::new(0);
static FALLBACK_ATTEMPTS: AtomicU64 = AtomicU64::new(0);
static FALLBACK_SUCCESSES: AtomicU64 = AtomicU64::new(0);

unsafe extern "C" {
    fn write(fd: i32, buf: *const u8, count: usize) -> isize;
}

fn raw_stderr(message: &'static [u8]) {
    unsafe { let _ = write(2, message.as_ptr(), message.len()); }
}

extern "C" fn bridge_loaded() {
    raw_stderr(b"[igpu-linear-bridge] loaded\\n");
}

#[used]
#[link_section = ".init_array"]
static BRIDGE_INIT: extern "C" fn() = bridge_loaded;

static FIRST_HOOK_REPORTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

fn report_first_hook() {
    if !FIRST_HOOK_REPORTED.swap(true, Ordering::Relaxed) {
        raw_stderr(b"[igpu-linear-bridge] first GBM hook entered\\n");
    }
}

extern "C" fn report_stats() {
    let create = CREATE_CALLS.load(Ordering::Relaxed);
    let modifiers = CREATE_MODIFIERS_CALLS.load(Ordering::Relaxed);
    let modifiers2 = CREATE_MODIFIERS2_CALLS.load(Ordering::Relaxed);
    let total = create + modifiers + modifiers2;
    if total == 0 {
        return;
    }

    eprintln!(
        "[igpu-linear-bridge] stats: hooks total={total} create={create} modifiers={modifiers} modifiers2={modifiers2}; off_mode={}; linear attempts={} success={} failed={}; fallback attempts={} success={}",
        OFF_CALLS.load(Ordering::Relaxed),
        LINEAR_ATTEMPTS.load(Ordering::Relaxed),
        LINEAR_SUCCESSES.load(Ordering::Relaxed),
        LINEAR_FAILURES.load(Ordering::Relaxed),
        FALLBACK_ATTEMPTS.load(Ordering::Relaxed),
        FALLBACK_SUCCESSES.load(Ordering::Relaxed),
    );
}

#[used]
#[link_section = ".fini_array"]
static REPORT_STATS: extern "C" fn() = report_stats;

#[repr(C)]
struct GbmDevice { _private: [u8; 0] }
#[repr(C)]
struct GbmBo { _private: [u8; 0] }

type CreateBasic = unsafe extern "C" fn(
    *mut GbmDevice, c_uint, c_uint, c_uint, c_uint
) -> *mut GbmBo;

type Create = unsafe extern "C" fn(
    *mut GbmDevice, c_uint, c_uint, c_uint, *const u64, c_uint
) -> *mut GbmBo;

type Create2 = unsafe extern "C" fn(
    *mut GbmDevice, c_uint, c_uint, c_uint, *const u64, c_uint, c_uint
) -> *mut GbmBo;

type GetModifier = unsafe extern "C" fn(*mut GbmBo) -> u64;

const LINEAR: u64 = 0;
const GBM_BO_USE_LINEAR: c_uint = 1 << 4;
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

// By default suppress tiny GBM capability probes (commonly 1x1).
// Set IGPU_LINEAR_BRIDGE_MIN_SIZE=1 to include them.
fn min_log_size() -> u32 {
    env::var("IGPU_LINEAR_BRIDGE_MIN_SIZE")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(2)
}

fn should_log(width: c_uint, height: c_uint) -> bool {
    log_enabled() && width >= min_log_size() && height >= min_log_size()
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

fn log_allocation(
    api: &str,
    width: c_uint,
    height: c_uint,
    format: c_uint,
    flags: Option<c_uint>,
    requested: String,
    path: &str,
    bo: *mut GbmBo,
) {
    if !should_log(width, height) { return; }

    let flag_text = flags
        .map(|v| format!(" flags=0x{v:08x}"))
        .unwrap_or_default();
    let outcome = if bo.is_null() {
        "FAILED".to_string()
    } else {
        match unsafe { actual_modifier(bo) } {
            Some(value) => format!("OK actual_modifier=0x{value:016x}"),
            None => "OK actual_modifier=<query unavailable>".to_string(),
        }
    };
    eprintln!(
        "[igpu-linear-bridge] {api} {width}x{height} format=0x{format:08x}{flag_text} path={path} {outcome}; requested {requested}"
    );
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
    report_first_hook();
    CREATE_MODIFIERS_CALLS.fetch_add(1, Ordering::Relaxed);
    let symbol = unsafe { next(b"gbm_bo_create_with_modifiers\0") };
    if symbol.is_null() {
        log("ERROR: real gbm_bo_create_with_modifiers symbol not found");
        return ptr::null_mut();
    }
    let real: Create = unsafe { std::mem::transmute(symbol) };
    let selected = mode();
    if selected == Mode::Off {
        OFF_CALLS.fetch_add(1, Ordering::Relaxed);
        return unsafe { real(device, width, height, format, modifiers, count) };
    }

    let linear = [LINEAR];
    LINEAR_ATTEMPTS.fetch_add(1, Ordering::Relaxed);
    let bo = unsafe { real(device, width, height, format, linear.as_ptr(), 1) };
    if !bo.is_null() {
        LINEAR_SUCCESSES.fetch_add(1, Ordering::Relaxed);
        log_allocation("create_with_modifiers", width, height, format, None,
            modifier_list(modifiers, count), "linear", bo);
        return bo;
    }

    LINEAR_FAILURES.fetch_add(1, Ordering::Relaxed);
    if selected == Mode::Prefer {
        FALLBACK_ATTEMPTS.fetch_add(1, Ordering::Relaxed);
        let fallback = unsafe { real(device, width, height, format, modifiers, count) };
        if !fallback.is_null() { FALLBACK_SUCCESSES.fetch_add(1, Ordering::Relaxed); }
        log_allocation("create_with_modifiers", width, height, format, None,
            modifier_list(modifiers, count), "original-list-fallback", fallback);
        return fallback;
    }

    log_allocation("create_with_modifiers", width, height, format, None,
        modifier_list(modifiers, count), "linear", ptr::null_mut());
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
    report_first_hook();
    CREATE_MODIFIERS2_CALLS.fetch_add(1, Ordering::Relaxed);
    let symbol = unsafe { next(b"gbm_bo_create_with_modifiers2\0") };
    if symbol.is_null() {
        log("ERROR: real gbm_bo_create_with_modifiers2 symbol not found");
        return ptr::null_mut();
    }
    let real: Create2 = unsafe { std::mem::transmute(symbol) };
    let selected = mode();
    if selected == Mode::Off {
        OFF_CALLS.fetch_add(1, Ordering::Relaxed);
        return unsafe { real(device, width, height, format, modifiers, count, flags) };
    }

    let linear = [LINEAR];
    LINEAR_ATTEMPTS.fetch_add(1, Ordering::Relaxed);
    let bo = unsafe { real(device, width, height, format, linear.as_ptr(), 1, flags) };
    if !bo.is_null() {
        LINEAR_SUCCESSES.fetch_add(1, Ordering::Relaxed);
        log_allocation("create_with_modifiers2", width, height, format, Some(flags),
            modifier_list(modifiers, count), "linear", bo);
        return bo;
    }

    LINEAR_FAILURES.fetch_add(1, Ordering::Relaxed);
    if selected == Mode::Prefer {
        FALLBACK_ATTEMPTS.fetch_add(1, Ordering::Relaxed);
        let fallback = unsafe { real(device, width, height, format, modifiers, count, flags) };
        if !fallback.is_null() { FALLBACK_SUCCESSES.fetch_add(1, Ordering::Relaxed); }
        log_allocation("create_with_modifiers2", width, height, format, Some(flags),
            modifier_list(modifiers, count), "original-list-fallback", fallback);
        return fallback;
    }

    log_allocation("create_with_modifiers2", width, height, format, Some(flags),
        modifier_list(modifiers, count), "linear", ptr::null_mut());
    ptr::null_mut()
}

#[no_mangle]
pub unsafe extern "C" fn gbm_bo_create(
    device: *mut GbmDevice,
    width: c_uint,
    height: c_uint,
    format: c_uint,
    flags: c_uint,
) -> *mut GbmBo {
    report_first_hook();
    CREATE_CALLS.fetch_add(1, Ordering::Relaxed);
    let symbol = unsafe { next(b"gbm_bo_create\0") };
    if symbol.is_null() {
        log("ERROR: real gbm_bo_create symbol not found");
        return ptr::null_mut();
    }
    let real: CreateBasic = unsafe { std::mem::transmute(symbol) };
    let selected = mode();
    if selected == Mode::Off {
        OFF_CALLS.fetch_add(1, Ordering::Relaxed);
        return unsafe { real(device, width, height, format, flags) };
    }

    // Preserve all caller requirements and additionally request a linear BO.
    let linear_flags = flags | GBM_BO_USE_LINEAR;
    LINEAR_ATTEMPTS.fetch_add(1, Ordering::Relaxed);
    let bo = unsafe { real(device, width, height, format, linear_flags) };
    if !bo.is_null() {
        LINEAR_SUCCESSES.fetch_add(1, Ordering::Relaxed);
        log_allocation(
            "create",
            width,
            height,
            format,
            Some(linear_flags),
            format!("original_flags=0x{flags:08x}"),
            "linear-flag",
            bo,
        );
        return bo;
    }

    LINEAR_FAILURES.fetch_add(1, Ordering::Relaxed);
    if selected == Mode::Prefer {
        FALLBACK_ATTEMPTS.fetch_add(1, Ordering::Relaxed);
        let fallback = unsafe { real(device, width, height, format, flags) };
        if !fallback.is_null() { FALLBACK_SUCCESSES.fetch_add(1, Ordering::Relaxed); }
        log_allocation(
            "create",
            width,
            height,
            format,
            Some(flags),
            format!("original_flags=0x{flags:08x}"),
            "original-flags-fallback",
            fallback,
        );
        return fallback;
    }

    log_allocation(
        "create",
        width,
        height,
        format,
        Some(linear_flags),
        format!("original_flags=0x{flags:08x}"),
        "linear-flag",
        ptr::null_mut(),
    );
    ptr::null_mut()
}
