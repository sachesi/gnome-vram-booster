# Usage

## How it works

The focused window receives VRAM priority (`dmem.low` set to VRAM × boost_ratio, default 90%). All other apps are set to zero. When focus switches, the previous app is reverted and the new one is boosted. Only apps with a systemd scope under `app.slice` can be boosted — see below for CLI-launched apps.

The extension sends the focused window's PID to the daemon over D-Bus, and the daemon boosts the unit under `app.slice` that the process, or one of its children, runs in. When focus moves to something that cannot be boosted (no focused window, a non-normal window, an invalid PID or GNOME Shell's own, an excluded WM class, or a process with no `app.slice` unit), the daemon takes the boost back, so the app you switched away from keeps no priority. A caller only reaches its own user's units: a PID whose unit is in another user's `user-<uid>.slice` cannot be boosted, and a session takes back only a boost it was given, so a session that loses focus does not drop the boost another user's session holds. On SIGTERM or SIGINT the daemon drops the boost, takes its settings off the slices and exits.

The daemon also protects `session.slice`, and can put a ceiling on `app.slice` as a whole; see [below](#protecting-the-compositor).

The GPU is the largest `drm/` entry in `/sys/fs/cgroup/dmem.capacity`; set `DRM_KEY` in the unit to pick another one.

If the daemon is killed without cleaning up (`SIGKILL`, a crash), a boost can be left behind. At startup it clears the `dmem.low` of every unit under `app.slice` that holds exactly its own boost value for the selected GPU, and logs how many; any other value, and any slice, is left alone.

A ratio of 0.90 is aggressive: it leaves little headroom for the compositor and other GPU users. Lower it (0.80, say) if the compositor stutters or background apps are evicted. Override it in the systemd service, not by running a second copy of the daemon by hand: the running instance owns the bus name.

```
sudo systemctl edit gnome-vram-booster.service
```

Add:

```
[Service]
Environment=VRAM_BOOST_RATIO=0.85
```

Then `sudo systemctl restart gnome-vram-booster.service`.

## Protecting the compositor

`dmemcg-booster` sets `dmem.low` on `app.slice` to the whole of VRAM, and nothing on its sibling `session.slice`, where GNOME Shell runs. The kernel weighs protection between siblings, so next to `app.slice`, everything in `session.slice` is unprotected: once VRAM is full, any app's buffers, background apps' included, can push out the compositor's. The daemon therefore sets `dmem.low` on the `session.slice` of every user whose app it boosts to the whole of VRAM as well, which puts GNOME Shell on a par with `app.slice`: the focused app evicts background apps' buffers first, and the compositor's only when nothing unprotected is left and a buffer moves back into VRAM. The rest of `session.slice` (portals, gsd daemons, Xwayland) is covered too; it holds little VRAM. `VRAM_PROTECT_SESSION=0` turns it off:

```
[Service]
Environment=VRAM_PROTECT_SESSION=0
```

The daemon writes it only where `session.slice`'s `dmem.low` is 0, leaves a value someone else set alone, and puts 0 back at exit where the value is still its own, like the ceiling below.

## The ceiling on `app.slice`

Off by default, and mostly not needed with `session.slice` protected. It keeps VRAM for everything outside `app.slice` in a harder way: `VRAM_RESERVE_MIB` sets `dmem.max` on the `app.slice` of every user whose app the daemon boosts to VRAM less that many MiB, which then stay with the rest:

```
[Service]
Environment=VRAM_RESERVE_MIB=256
```

It costs the focused app. A new buffer that would take `app.slice` past the ceiling goes straight to system memory (GTT) without evicting anything, even while background apps hold VRAM the focused app could otherwise take from them: from Linux 7.3 the kernel evicts for a protected allocation when VRAM itself is full, but not when a cgroup limit is hit. Only a buffer moved back into VRAM later evicts, inside `app.slice`, where the focused app's `dmem.low` still protects it. Up to 7.2 new buffers never evict, and the ceiling makes `app.slice`'s share of VRAM smaller by the reserve. Turn it on if GNOME Shell stutters when VRAM runs out, and compare with it off.

A reserve as large as the VRAM stops the daemon from starting. Nothing outside `app.slice` is limited.

The daemon checks the ceiling at every focus change, since `app.slice` is made anew when a user manager restarts. Up to Linux 7.2, the kernel keeps the old limit without an error if `app.slice` already uses more than the ceiling; the daemon notices and tries again at the next focus change. From 7.3 it applies at once. The daemon never evicts to make room: the write is non-blocking.

A `dmem.max` on `app.slice` that the daemon did not write is left alone, with a warning. On exit it restores `max` where the ceiling is still its own. A daemon killed outright leaves its ceiling behind; the next start takes it over, unless `VRAM_RESERVE_MIB` changed in between, in which case logging out and in clears it.

```
cat /sys/fs/cgroup/user.slice/user-$(id -u).slice/user@$(id -u).service/app.slice/dmem.max
```

## Daemon status

Query the daemon's current state:

```
gnome-vram-boosterctl
```

Example output:

```
=== GNOME VRAM Booster Status ===
Daemon:           running
DRM key:          drm/0000:2d:00.0/vram
VRAM total:       8573157376 (8176 MiB, 7.98 GiB)
Boost ratio:      90%
Boosted bytes:    7715841638 (7358 MiB, 7.19 GiB) (90% of total)
Session low:      8573157376 (8176 MiB, 7.98 GiB)
App ceiling:      off
Current unit:     app-org.example.Game.scope
Boosted cgroup:   /sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service/app.slice/app-org.example.Game.scope
```

## Debug indicator

Open the Extensions app, click the gear icon next to **GNOME VRAM Booster**, and enable **Show active app in panel**. A label appears in the top bar showing which app currently holds VRAM priority, `idle` when the focused window is not under `app.slice`, or `offline` while the daemon is not running.

## Verifying

Watch `dmem.low` values update as you switch focus between apps:

```
find /sys/fs/cgroup/user.slice -name "dmem.low" -path "*/app.slice/*" \
  | xargs grep -v " 0$" 2>/dev/null
```

The focused app scope should show 90% of VRAM capacity (boosted value). All others should be zero.

Follow the daemon log:

```
just logs
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

Common cause: `dmemcg-booster` is not running or `dmem` is not in `cgroup.controllers`. The journal names the reason; a `VRAM_BOOST_RATIO` that is not a number above 0 and at most 1, a `VRAM_PROTECT_SESSION` other than 0 or 1, a `VRAM_RESERVE_MIB` that is not a whole number below the VRAM size, or a `DRM_KEY` that `dmem.capacity` does not list, stops it too.

**dmem.low files missing under app scopes**

`dmemcg-booster` has not propagated the controller. Check its status:

```
systemctl status dmemcg-booster.service
```

**Boost not happening for a specific app**

The app must be launched from the GNOME app launcher (not from a terminal) to get its own systemd scope under `app.slice`. Check the daemon log — it will skip processes not under `app.slice`.
