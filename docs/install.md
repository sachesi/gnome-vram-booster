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

## Verifying

Watch `dmem.low` values update as you switch focus between apps:

```
find /sys/fs/cgroup/user.slice -name "dmem.low" -path "*/app.slice/*" \
  | xargs grep -v " 0$" 2>/dev/null
```

The focused app scope should show the full VRAM capacity. All others should be zero.

Follow the daemon log:

```
make logs
```

## Troubleshooting

**Extension does not appear in Extensions app after relogin**

Check that the extension directory was created:

```
ls /usr/share/gnome-shell/extensions/vram-booster@local/
```

**Daemon fails to start**

```
sudo journalctl -u gnome-vram-booster -b --no-pager
```

Common cause: `dmemcg-booster` is not running or `dmem` is not in `cgroup.controllers`.

**dmem.low files missing under app scopes**

`dmemcg-booster` has not propagated the controller. Check its status:

```
systemctl status dmemcg-booster.service
```

**Boost not happening for a specific app**

The app must be launched from the GNOME app launcher (not from a terminal) to get its own systemd scope under `app.slice`. Check the daemon log — it will skip processes not under `app.slice`.
