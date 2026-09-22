# gnome-vram-booster

Keeps the focused window's VRAM from being evicted on GNOME. A GNOME Shell extension reports the focused window's PID to a system daemon, which, through the Linux dmem cgroup controller, protects most of the VRAM for that app's scope, taking the protection back from the app focused before, and protects `session.slice`, where GNOME Shell runs, from background apps. It matters most on GPUs with 8 GB or less, where whatever runs in the background can otherwise push the foreground app's buffers out to system memory.

It is the GNOME counterpart of KDE's `plasma-foreground-booster`, and experimental: it writes cgroup files as root, so try it on a setup you can afford to reboot.

## Requirements

- Linux 6.15 or newer, the first where amdgpu reports VRAM to the `dmem` cgroup controller (the controller came in 6.14). Up to 7.2, protection only decides what is evicted when a buffer moves back into VRAM, while a new buffer that finds VRAM full still goes to system memory; from 7.3, or with the patches CachyOS ships, a protected app's new buffer evicts unprotected ones instead, which is where most of the gain is.
- [dmemcg-booster](https://pixelcluster.github.io/VRAM-Mgmt-fixed/), both its system and its user service.
- GNOME Shell 45 to 50, on Wayland. X11 is untested.
- An AMD GPU on `amdgpu`. Intel is untested; NVIDIA's proprietary driver is untested and likely lacks dmem support.
- Apps launched into a scope of their own under `app.slice`, as the app grid does; see [docs/usage.md](docs/usage.md#apps-launched-from-a-terminal-or-custom-launcher).

## Building and installing

```
just build      # needs a Rust toolchain
just install    # run as your user: it calls sudo itself
```

`just install` never builds, so it can run on a machine without a Rust toolchain, such as the host when you build in a container. Log out and back in afterwards, then enable **GNOME VRAM Booster** in the Extensions app. Details and removal are in [docs/install.md](docs/install.md).

On Arch Linux, install the AUR package `gnome-vram-booster`, built from [packaging/aur/PKGBUILD](packaging/aur/PKGBUILD), which each release tag updates.

## Usage

```
gnome-vram-boosterctl
```

prints the GPU, the boost size and the unit that holds the boost. The boost is 90% of VRAM; lower it if GNOME Shell stutters:

```
sudo systemctl edit gnome-vram-booster.service   # [Service] Environment=VRAM_BOOST_RATIO=0.80
```

`VRAM_RESERVE_MIB` caps `app.slice` that far below the VRAM size, keeping it for GNOME Shell at a cost to the focused app; it is off by default. See [docs/usage.md](docs/usage.md#the-ceiling-on-appslice).

## Documentation

- [Installing](docs/install.md)
- [Usage](docs/usage.md), including apps launched from a terminal, and troubleshooting

GPL-3.0-or-later.
