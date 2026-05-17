# Installation

## Prerequisites

**1. Kernel with dmem cgroup controller**

Verify support:

```
cat /sys/fs/cgroup/cgroup.controllers
```

The output must include `dmem`. Requires kernel 6.12 or newer. Distributions known to ship it: CachyOS, Nobara, Bazzite.

**2. dmemcg-booster**

This daemon propagates the dmem controller into user session cgroups. Without it, `dmem.low` files will not exist under app scopes and the booster cannot write to them.

Install from your distribution's repository or build from source:
https://pixelcluster.github.io/VRAM-Mgmt-fixed/

Enable and start:

```
sudo systemctl enable --now dmemcg-booster.service
```

**3. Rust toolchain**

```
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

## Build and install

```
make install
```

This will:

- Build the daemon with `cargo build --release`
- Install the binary to `/usr/bin/gnome-vram-booster`
- Install the systemd service to `/usr/lib/systemd/system/`
- Install the D-Bus policy to `/usr/share/dbus-1/system.d/`
- Copy the GNOME Shell extension to `/usr/share/gnome-shell/extensions/`
- Enable and start the daemon

**After installation**, log out and back in. Then open the Extensions app and enable **GNOME VRAM Booster**.

## Uninstall

```
make uninstall
```

Log out and back in to deactivate the extension.
