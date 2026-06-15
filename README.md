# gnome-vram-booster

> **Experimental.** This is a local desktop tool under active development. It writes to privileged cgroup files and is not production-hardened. Test on a non-critical setup first.

Dynamic VRAM prioritization for GNOME via Linux dmem cgroups. Keeps the focused app's GPU memory protected from TTM eviction on 8 GB and under GPUs. GNOME equivalent of KDE's `plasma-foreground-booster`.

## Requirements

- Kernel 6.12+ with `dmem` cgroup controller
- [`dmemcg-booster`](https://pixelcluster.github.io/VRAM-Mgmt-fixed/) — both the **system** service (propagates dmem into user session cgroups) and the **user** service (propagates dmem into app scopes) must be active
- GNOME Shell 45–50 (Wayland session)
- AMD GPU (`amdgpu` driver)
- [`just`](https://github.com/casey/just) and a Rust toolchain to build/install

## Hardware support

| Category | Status |
|---|---|
| AMD GPU (`amdgpu`), Mesa/RADV | Tested, supported |
| NVIDIA proprietary driver | Untested, likely unsupported (no dmemcg) |
| Intel GPU | Untested |
| GNOME Wayland | Primary target |
| X11 session | Untested, may work |
| Non-GNOME desktops | Untested |

## Install

```
just install
```

See [docs/install.md](docs/install.md) for full instructions and [docs/usage.md](docs/usage.md) for usage and troubleshooting.

## Measurements

The mechanism (`dmem.low`) is a real kernel-level cgroup parameter. Measurable effects include:

- **VRAM allocation under desktop idle** — GNOME compositor + background apps should see reduced dmem.low when focus shifts to a workload
- **Frametime stability under VRAM pressure** — compare 1% and 0.1% lows with booster disabled vs enabled on 4–8 GB GPUs
- **Eviction avoidance** — foreground app should experience fewer TTM evictions during compositor GPU activity (browser compositing, animations)
- **Compositor behavior** — GNOME Shell may show reduced VRAM allocation when a game or GPU workload is focused

Suggested test cases:

- 4 GB GPU: GNOME Wayland + Firefox (several tabs) + Steam Proton game
- 8 GB GPU: GNOME Wayland + browser + Electron/Discord + VRAM-heavy game

## How it works

1. GNOME Shell extension detects focused window PID
2. Rust daemon resolves PID to systemd cgroup under `app.slice`
3. Daemon writes `dmem.low = VRAM_total * boost_ratio` to the focused app's cgroup
4. Previous boosted cgroup is reverted to 0
5. On daemon exit (SIGTERM/SIGINT), boosted cgroup is reset

**Idle / ClearFocus.** When focus moves to something that cannot be boosted — no
focused window, a non-normal window, an invalid/shell PID, an excluded WM class,
or a process with no `app.slice` scope — the extension calls `ClearFocus` and the
daemon reverts the previously boosted cgroup to 0. This prevents stale priority
lingering on the app you switched away from.

**Startup cleanup.** If the daemon was killed (`SIGKILL`, crash) without running
its exit cleanup, a stale `dmem.low` boost can persist. On startup the daemon
scans `app.slice` scopes and clears only `dmem.low` entries whose value for the
selected GPU equals its own boost value; unrelated values are left untouched. The
number cleared is logged.

Boost ratio defaults to 0.90 (90% of VRAM), which is **aggressive** — it leaves
little headroom for the compositor and other GPU consumers. Lower it (e.g. 0.80)
if you see compositor stutter or eviction of background apps. Override via
`VRAM_BOOST_RATIO`:

```
VRAM_BOOST_RATIO=0.80 gnome-vram-booster
```

Query daemon status:

```
gnome-vram-boosterctl
```

## License

GPL-3.0-or-later
