# Usage

## How it works

The focused window receives full VRAM priority (`dmem.low` set to total VRAM capacity). All other apps are set to zero. When focus switches, the previous app is reverted and the new one is boosted. Only apps with a systemd scope under `app.slice` can be boosted — see below for CLI-launched apps.

## Debug indicator

Open the Extensions app, click the gear icon next to **GNOME VRAM Booster**, and enable **Show active app in panel**. A label appears in the top bar showing which app currently holds VRAM priority, or `VRAM: idle` when the focused window is not under `app.slice`.

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

## Apps launched from a terminal or custom launcher

Apps started from a terminal do not get an `app.slice` scope and are skipped by the daemon. Wrap the launch command with `systemd-run` to create a proper scope:

```
systemd-run --user --scope --slice=app.slice leyen run {game_id}
```

To make this permanent, add a shell alias:

```bash
alias leyen='systemd-run --user --scope --slice=app.slice leyen'
```

**Alternative: create a `.desktop` file**

Apps launched through GNOME (app grid, `gnome-shell` search) automatically get an `app.slice` scope — no `systemd-run` needed. Create `~/.local/share/applications/leyen.desktop`:

```ini
[Desktop Entry]
Type=Application
Name=Leyen
Exec=leyen run %u
Icon=application-x-executable
Categories=Game;
```

Then launch from the app grid or Activities search instead of a terminal.

**Single-instance apps (Zed, VS Code, etc.)**

Some editors use a daemon model — the first launch starts a background process; all subsequent launches attach to it. If that daemon was ever started from a terminal, it lives outside `app.slice` and will be skipped even when you click the app in the GNOME menu.

Check which cgroup the process is in:

```
cat /proc/$(pgrep -f zed | head -1)/cgroup
```

If the path does not contain `/app.slice/`, kill the daemon and relaunch from the app grid:

```
pkill -f zed
```

The new daemon will inherit the `app.slice` scope from GNOME and will be boosted correctly.

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
