# igpu-linear-bridge (Wayland GBM presenter fallback shim)

This replacement is a small C `LD_PRELOAD` shim. It no longer rewrites GBM
modifier lists and does not import, export, map, clear, blit, or read back any
DMA-BUF.

Its diagnostic target is Chromium's Wayland GBM-device creation. When explicitly
enabled, it matches only a Chromium process whose command line contains both
`--ozone-platform=wayland` and the configured `--render-node-override`, and
only an FD whose character-device number exactly matches the configured DRM
node. If the target path cannot be resolved, the shim fails closed and does not
intercept. In `deny` mode it makes the
interposed `gbm_create_device(fd)` return `NULL` with `errno=ENODEV`. This is
intended to make Chromium's Wayland GBM Presenter unavailable so its existing
EGL Wayland window-surface fallback can be selected.

## Build

Requirements: a C compiler, GNU libc development files, and `make`.

```sh
make
```

Output: `libigpu_linear_bridge.so`.

## First run: observe only

This confirms whether `gbm_create_device()` is dynamically interposed and
whether the FD matches the NVIDIA render node. It does not change the GBM call.

```sh
IGPU_GBM_FALLBACK_ENABLE=1 \
IGPU_GBM_FALLBACK_TARGET=/dev/dri/renderD129 \
IGPU_GBM_FALLBACK_MODE=observe \
LD_PRELOAD="$PWD/libigpu_linear_bridge.so" \
/usr/lib/chromium/chromium \
  --enable-gpu-rasterization \
  --ozone-platform=wayland \
  --render-node-override=/dev/dri/renderD129
```

Expected diagnostic form:

```text
[igpu-linear-bridge] gbm_create_device #1 fd=... target=/dev/dri/renderD129 caller=... -> passthrough (mode=observe)
```

If no matching log appears, the hook was not reached, the process command line
did not match, or the FD did not identify the configured node. Do not switch to
`deny` until the observe log confirms the intended call.

## Enable the fallback experiment

```sh
IGPU_GBM_FALLBACK_ENABLE=1 \
IGPU_GBM_FALLBACK_TARGET=/dev/dri/renderD129 \
IGPU_GBM_FALLBACK_MODE=deny \
LD_PRELOAD="$PWD/libigpu_linear_bridge.so" \
/usr/lib/chromium/chromium \
  --enable-gpu-rasterization \
  --ozone-platform=wayland \
  --render-node-override=/dev/dri/renderD129
```

`deny` is deliberately opt-in. The default mode is `observe`, and the shim is
inactive unless `IGPU_GBM_FALLBACK_ENABLE=1` is set.

## Scope and limitations

- This hook only intercepts the dynamically resolved `gbm_create_device` symbol.
  It cannot intercept a statically linked, hidden, or direct internal call.
- The match is constrained to Chromium, native Wayland, the exact render-node
  override, and the configured DRM node.
- Returning `NULL` makes the GBM device unavailable to every Chromium Wayland
  code path sharing that device initialization, not just the output Presenter.
  Other GBM-backed resources may consequently fail. This is a diagnostic
  fallback experiment, not a guaranteed production fix.
- The shim does not force the EGL implementation to NVIDIA. Check
  `GL_VENDOR`/`GL_RENDERER` at runtime; the render-node override and this hook
  do not independently select the EGL vendor.
- EGL Wayland window surfaces still exchange buffers with the compositor via
  the EGL/Wayland driver path. This shim does not bypass compositor import.
- Do not preload it into the whole desktop session. Use a separate Chromium
  profile for testing.
- No GitHub Actions, CI, or remote repository changes are performed by this
  replacement bundle.

## Safety boundary

This implementation performs no GPU rendering, DMA-BUF writes, DMA-BUF reads,
format conversion, or cross-GPU synchronization. It only observes or denies
`gbm_create_device()` according to the explicit mode above.

## License

MIT
