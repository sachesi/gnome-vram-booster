# Installation

## Prerequisites

**1. Kernel with dmem cgroup controller**

Verify support:

```
cat /sys/fs/cgroup/cgroup.controllers
```

The output must include `dmem`. The controller came in kernel 6.14, and amdgpu uses it from 6.15; for what changes with 7.3, see the [README](../README.md#requirements). Distributions known to ship it: CachyOS, Nobara, Bazzite.

**2. dmemcg-booster**

This daemon propagates the dmem controller into user session cgroups. Without it, `dmem.low` files will not exist under app scopes and the booster cannot write to them.

Install from your distribution's repository or build from source:
https://pixelcluster.github.io/VRAM-Mgmt-fixed/

Enable and start both the system service (propagates dmem into user session cgroups) and the user service (propagates dmem into app scopes):

```
sudo systemctl enable --now dmemcg-booster.service
systemctl --user enable --now dmemcg-booster.service
```

**3. Rust toolchain**, only where you build

```
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

## Build and install

```
just build
just install
```

`just build` runs `cargo build --release` and needs the Rust toolchain. `just install` never builds: it only checks that `daemon/target/release/` holds the binaries, so a machine without Rust, such as the host when you build in a container, can install binaries built elsewhere. Run it as your user, not with `sudo`: it calls `sudo` itself, and under `sudo` its check of the user `dmemcg-booster.service` would ask root's service manager instead of yours. `just install` will:

- Install the binaries to `/usr/bin/gnome-vram-booster` and `/usr/bin/gnome-vram-boosterctl`
- Install the systemd service to `/usr/lib/systemd/system/`
- Install the D-Bus policy to `/usr/share/dbus-1/system.d/`
- Copy the GNOME Shell extension to `/usr/share/gnome-shell/extensions/`
- Enable and start the daemon

**After installation**, log out and back in. Then open the Extensions app and enable **GNOME VRAM Booster**.

## Uninstall

```
just uninstall
```

Log out and back in to deactivate the extension.

## Updating after code changes

```
just build
just reload
```

Reinstalls the binaries from `daemon/target/release/` and the extension, and restarts the daemon.
