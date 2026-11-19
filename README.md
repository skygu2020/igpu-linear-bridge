# igpu-linear-bridge

An experimental Linux LD_PRELOAD shim for GBM modifier-based allocations.
It is intended for PRIME render-offload systems where an NVIDIA GPU renders
buffers and an Intel iGPU compositor (such as labwc/wlroots) imports them.

## What it does

When enabled, it interposes the following GBM entry points:

- gbm_bo_create_with_modifiers()
- gbm_bo_create_with_modifiers2()

It replaces the requested modifier list with the single DRM_FORMAT_MOD_LINEAR
modifier (numeric value 0) at allocation time. The second entry point preserves
the caller's GBM flags.

It does not rewrite a modifier after allocation, alter DMA-BUF metadata, or
fake the value returned by gbm_bo_get_modifier(). It currently does not
interpose implicit-modifier gbm_bo_create(), EGL, Vulkan, or Wayland protocol
messages.

## Build

Install Rust and run:

    cargo build --release

The shared library is:

    target/release/libigpu_linear_bridge.so

No third-party Rust crates are required.

## Run

    IGPU_LINEAR_BRIDGE_MODE=force \
    IGPU_LINEAR_BRIDGE_LOG=1 \
    LD_PRELOAD="$PWD/target/release/libigpu_linear_bridge.so" \
    /usr/lib/chromium/chromium \
      --enable-gpu-rasterization \
      --ozone-platform=wayland \
      --render-node-override=/dev/dri/renderD129

The NVIDIA render-node override is retained. LD_PRELOAD is inherited by
Chromium child processes; only processes that call these GBM symbols are
affected.

## Modes

- Unset or off: pass through without changing allocations.
- force: request LINEAR only. If that allocation fails, return failure rather
  than silently returning a tiled buffer.
- prefer: try LINEAR first, then retry the original modifier list if LINEAR
  allocation fails. This mode may still return a non-LINEAR allocation.

Logging is enabled by default when the bridge is active. By default, allocations
smaller than 2x2 are omitted from the log (they are still intercepted and
handled normally). Set IGPU_LINEAR_BRIDGE_MIN_SIZE=1 to include 1x1 probes,
or choose another minimum dimension. Set IGPU_LINEAR_BRIDGE_LOG=0 to suppress
all allocation logs.

## Validation

With force enabled, look for log messages showing the requested modifier list
and an actual modifier of 0x0000000000000000. Then inspect WAYLAND_DEBUG output
to confirm that the compositor receives modifier 0, and inspect labwc debug
logs to confirm the corresponding eglCreateImageKHR calls no longer fail
with EGL_BAD_MATCH.

A successful GBM allocation does not prove that Chromium uses these two
functions for every buffer path. If no bridge log appears, Chromium may be
using another allocation path or the symbols may not be dynamically
interposed in that process.

## Limitations and safety

- Test with a separate Chromium profile first.
- Do not preload this into the entire desktop session.
- LINEAR allocations may have different performance and memory requirements.
- A LINEAR allocation can fail for a particular format/usage combination.
- This is an experimental compatibility shim, not a general-purpose PRIME
  buffer conversion implementation.

## License

MIT
