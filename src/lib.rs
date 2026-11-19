use std::env;
use std::ffi::{c_char, c_int, c_uint, c_void};
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
static IMPORT_CALLS: AtomicU64 = AtomicU64::new(0);
static IMPORT_MODIFIER_CALLS: AtomicU64 = AtomicU64::new(0);
static IMPORT_MODIFIER_SUCCESSES: AtomicU64 = AtomicU64::new(0);
static IMPORT_MODIFIER_FAILURES: AtomicU64 = AtomicU64::new(0);
static LEGACY_IMPORT_FALLBACK_CALLS: AtomicU64 = AtomicU64::new(0);
static LEGACY_IMPORT_FALLBACK_SUCCESSES: AtomicU64 = AtomicU64::new(0);

unsafe extern "C" {
    fn write(fd: i32, buf: *const u8, count: usize) -> isize;
    fn __errno_location() -> *mut c_int;
    fn dup(fd: c_int) -> c_int;
}

fn raw_stderr(message: &'static [u8]) {
    unsafe { let _ = write(2, message.as_ptr(), message.len()); }
}

extern "C" fn bridge_loaded() {
    raw_stderr(b"[igpu-linear-bridge] loaded\n");
}

#[used]
#[link_section = ".init_array"]
static BRIDGE_INIT: extern "C" fn() = bridge_loaded;

static FIRST_HOOK_REPORTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

fn report_first_hook() {
    if !FIRST_HOOK_REPORTED.swap(true, Ordering::Relaxed) {
        raw_stderr(b"[igpu-linear-bridge] first GBM hook entered\n");
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
        "[igpu-linear-bridge] stats: hooks total={total} create={create} modifiers={modifiers} modifiers2={modifiers2}; off_mode={}; linear attempts={} success={} failed={}; fallback attempts={} success={}; import_fd_modifier calls={} success={} failed={}",
        OFF_CALLS.load(Ordering::Relaxed),
        LINEAR_ATTEMPTS.load(Ordering::Relaxed),
        LINEAR_SUCCESSES.load(Ordering::Relaxed),
        LINEAR_FAILURES.load(Ordering::Relaxed),
        FALLBACK_ATTEMPTS.load(Ordering::Relaxed),
        FALLBACK_SUCCESSES.load(Ordering::Relaxed),
        IMPORT_MODIFIER_CALLS.load(Ordering::Relaxed),
        IMPORT_MODIFIER_SUCCESSES.load(Ordering::Relaxed),
        IMPORT_MODIFIER_FAILURES.load(Ordering::Relaxed),
    );
    eprintln!(
        "[igpu-linear-bridge] legacy import fallback: calls={} success={}",
        LEGACY_IMPORT_FALLBACK_CALLS.load(Ordering::Relaxed),
        LEGACY_IMPORT_FALLBACK_SUCCESSES.load(Ordering::Relaxed),
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
type Import = unsafe extern "C" fn(*mut GbmDevice, c_uint, *mut c_void, c_uint) -> *mut GbmBo;

#[repr(C)]
struct GbmImportFdData {
    fd: c_int,
    width: c_uint,
    height: c_uint,
    stride: c_uint,
    format: c_uint,
}

#[repr(C)]
struct GbmImportFdModifierData {
    width: c_uint,
    height: c_uint,
    format: c_uint,
    num_fds: c_uint,
    fds: [i32; 4],
    strides: [i32; 4],
    offsets: [i32; 4],
    modifier: u64,
}

const GBM_BO_IMPORT_FD: c_uint = 0x5503;
const GBM_BO_IMPORT_FD_MODIFIER: c_uint = 0x5504;
static LEGACY_IMPORT_DIAG_RAN: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

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
pub unsafe extern "C" fn gbm_bo_import(
    device: *mut GbmDevice,
    import_type: c_uint,
    buffer: *mut c_void,
    flags: c_uint,
) -> *mut GbmBo {
    let symbol = unsafe { next(b"gbm_bo_import\0") };
    if symbol.is_null() {
        log("ERROR: real gbm_bo_import symbol not found");
        return ptr::null_mut();
    }
    let real: Import = unsafe { std::mem::transmute(symbol) };

    // Record the numeric import type before interpreting the payload. GBM
    // headers in the wild have used both 0x5504 and 0x5505 for modifier
    // imports; 0x5504 is also used by older headers for FD_PLANAR.
    let import_call = IMPORT_CALLS.fetch_add(1, Ordering::Relaxed) + 1;
    if import_call <= 64 && log_enabled() {
        eprintln!(
            "[igpu-linear-bridge] gbm_bo_import call #{import_call} type=0x{import_type:04x} flags=0x{flags:08x} buffer_null={}",
            buffer.is_null()
        );
    }

    if import_type != GBM_BO_IMPORT_FD_MODIFIER || buffer.is_null() {
        return unsafe { real(device, import_type, buffer, flags) };
    }

    let data = unsafe { &*(buffer.cast::<GbmImportFdModifierData>()) };
    let call = IMPORT_MODIFIER_CALLS.fetch_add(1, Ordering::Relaxed) + 1;
    let bo = unsafe { real(device, import_type, buffer, flags) };
    let errno = unsafe { *__errno_location() };
    if bo.is_null() {
        IMPORT_MODIFIER_FAILURES.fetch_add(1, Ordering::Relaxed);
    } else {
        IMPORT_MODIFIER_SUCCESSES.fetch_add(1, Ordering::Relaxed);
    }

    // Keep diagnostics bounded: Chromium may perform this probe in a tight loop.
    if call <= 32 && log_enabled() {
        let outcome = if bo.is_null() { "FAILED" } else { "OK" };
        eprintln!(
            "[igpu-linear-bridge] import_fd_modifier #{call} {}x{} format=0x{:08x} num_fds={} fd0={} stride0={} offset0={} modifier=0x{:016x} flags=0x{flags:08x} errno={errno} {outcome}",
            data.width,
            data.height,
            data.format,
            data.num_fds,
            if data.num_fds > 0 && data.num_fds <= 4 { data.fds[0] } else { -1 },
            if data.num_fds > 0 && data.num_fds <= 4 { data.strides[0] } else { -1 },
            if data.num_fds > 0 && data.num_fds <= 4 { data.offsets[0] } else { -1 },
            data.modifier
        );
    }
    if bo.is_null()
        && data.num_fds == 1
        && data.modifier == LINEAR
        && data.offsets[0] == 0
        && env::var("IGPU_LINEAR_BRIDGE_LINEAR_IMPORT_FALLBACK").as_deref() == Ok("1")
    {
        let fallback_call = LEGACY_IMPORT_FALLBACK_CALLS.fetch_add(1, Ordering::Relaxed) + 1;
        let mut legacy = GbmImportFdData {
            fd: data.fds[0],
            width: data.width,
            height: data.height,
            stride: data.strides[0] as c_uint,
            format: data.format,
        };
        let legacy_bo = unsafe {
            real(device, GBM_BO_IMPORT_FD, (&mut legacy as *mut GbmImportFdData).cast(), flags)
        };
        let legacy_errno = unsafe { *__errno_location() };
        if !legacy_bo.is_null() {
            let imported_modifier = unsafe { actual_modifier(legacy_bo) };
            if imported_modifier == Some(LINEAR) {
                LEGACY_IMPORT_FALLBACK_SUCCESSES.fetch_add(1, Ordering::Relaxed);
                if fallback_call <= 32 {
                    eprintln!(
                        "[igpu-linear-bridge] linear_import_fallback #{fallback_call} {}x{} format=0x{:08x} stride={} flags=0x{flags:08x} legacy_modifier=0x0000000000000000 OK",
                        data.width, data.height, data.format, data.strides[0]
                    );
                }
                return legacy_bo;
            }

            // A legacy FD import may infer the real NVIDIA layout (e.g. a
            // tiled modifier) even though the caller supplied modifier=0.
            // Never return such a BO to a caller that requested Linear.
            let destroy_symbol = unsafe { next(b"gbm_bo_destroy\0") };
            if !destroy_symbol.is_null() {
                let destroy: unsafe extern "C" fn(*mut GbmBo) =
                    unsafe { std::mem::transmute(destroy_symbol) };
                unsafe { destroy(legacy_bo) };
            }
            if fallback_call <= 32 {
                eprintln!(
                    "[igpu-linear-bridge] linear_import_fallback #{fallback_call} {}x{} format=0x{:08x} stride={} flags=0x{flags:08x} legacy_modifier={} REJECTED",
                    data.width,
                    data.height,
                    data.format,
                    data.strides[0],
                    imported_modifier.map(|v| format!("0x{v:016x}")).unwrap_or_else(|| "<unavailable>".to_string())
                );
            }
        }
        if fallback_call <= 32 {
            eprintln!(
                "[igpu-linear-bridge] linear_import_fallback #{fallback_call} {}x{} format=0x{:08x} stride={} flags=0x{flags:08x} errno={} FAILED",
                data.width, data.height, data.format, data.strides[0], legacy_errno
            );
        }
    }
    if bo.is_null()
        && data.width == 1
        && data.height == 1
        && data.num_fds == 1
        && data.modifier == LINEAR
        && env::var("IGPU_LINEAR_BRIDGE_DIAG_LEGACY_IMPORT").as_deref() == Ok("1")
        && !LEGACY_IMPORT_DIAG_RAN.swap(true, Ordering::Relaxed)
    {
        // Diagnostic only: duplicate the exported dma-buf so the original FD
        // remains untouched. Keep the duplicate open because GBM backend FD
        // ownership is backend-specific and undocumented by the public API.
        for usage in [flags, 0] {
            let probe_fd = unsafe { dup(data.fds[0]) };
            if probe_fd < 0 {
                eprintln!("[igpu-linear-bridge] legacy_import_diag dup failed errno={}", unsafe { *__errno_location() });
                break;
            }
            let mut legacy = GbmImportFdData {
                fd: probe_fd,
                width: data.width,
                height: data.height,
                stride: data.strides[0] as c_uint,
                format: data.format,
            };
            let legacy_bo = unsafe {
                real(device, GBM_BO_IMPORT_FD, (&mut legacy as *mut GbmImportFdData).cast(), usage)
            };
            let legacy_errno = unsafe { *__errno_location() };
            eprintln!(
                "[igpu-linear-bridge] legacy_import_diag {}x{} format=0x{:08x} fd={} stride={} flags=0x{usage:08x} errno={} {}",
                legacy.width,
                legacy.height,
                legacy.format,
                legacy.fd,
                legacy.stride,
                legacy_errno,
                if legacy_bo.is_null() { "FAILED" } else { "OK" }
            );
            if !legacy_bo.is_null() {
                let destroy_symbol = unsafe { next(b"gbm_bo_destroy\0") };
                if !destroy_symbol.is_null() {
                    let destroy: unsafe extern "C" fn(*mut GbmBo) =
                        unsafe { std::mem::transmute(destroy_symbol) };
                    unsafe { destroy(legacy_bo) };
                }
                break;
            }
        }
    }
    bo
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
