# gnome-vram-booster

Dynamic VRAM prioritization for GNOME desktop via Linux dmem cgroups.

When VRAM is under pressure on low-memory GPUs (8 GB and under), the kernel's TTM memory manager evicts allocations to system RAM over the PCIe bus, causing stuttering. This tool tracks the focused window and sets `dmem.low` on its systemd scope so the kernel prefers evicting background apps instead.

Equivalent to KDE's `plasma-foreground-booster`, implemented for GNOME without Qt dependencies.

## How it works

Two components:

- **GNOME Shell extension** — hooks `notify::focus-window`, debounces rapid switches, sends the focused PID to the daemon over D-Bus
- **System daemon** (`gnome-vram-booster`) — reads the PID's cgroup path from `/proc`, writes `dmem.low` to that cgroup, reverts the previous one

The daemon runs as root and writes directly to `/sys/fs/cgroup`. It does not use `SetUnitProperties` because systemd does not yet expose dmem as a native unit property.

## Requirements

- Linux kernel with `dmem` cgroup controller (kernel 6.12+, check `cat /sys/fs/cgroup/cgroup.controllers`)
- [`dmemcg-booster`](https://pixelcluster.github.io/VRAM-Mgmt-fixed/) running — propagates the dmem controller into user session slices
- GNOME Shell 45–50
- AMD GPU (dmem controller currently only wired for AMDGPU/RadeonSI/RADV)
- Rust toolchain for building

## Build and install

```
make install
```

This builds the daemon, installs the systemd service and D-Bus policy, and copies the extension to `~/.local/share/gnome-shell/extensions/`. Reload GNOME Shell after (`Alt+F2`, type `r`).

Other targets:

```
make uninstall    # remove daemon, service, extension
make reload       # rebuild and restart without full reinstall
make logs         # follow daemon logs
make check-deps   # verify dmemcg-booster is running
```

## Verifying it works

Watch dmem.low values change as you switch focus between apps:

```
find /sys/fs/cgroup/user.slice -name "dmem.low" -path "*/app.slice/*" \
  | xargs grep -v " 0$" 2>/dev/null
```

The currently focused app scope should hold the full VRAM capacity value. Background scopes should be zero.

## Caveats

- Only boosts apps that have their own systemd scope under `app.slice` (apps launched from the GNOME app launcher). Terminal subprocesses and portal-backed apps inherit the scope of their parent.
- dmem.low is a soft limit — the kernel uses it as a hint under memory pressure, not a hard reservation.
- Transient scopes (short-lived apps) may be gone by the time the daemon tries to revert them; this logs a warning and is harmless.

## License

GPL-3.0-or-later
