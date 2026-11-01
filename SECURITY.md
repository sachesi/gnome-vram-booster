# Security

Please report a vulnerability privately rather than in a public issue: through
[a private advisory](https://github.com/sachesi/gnome-vram-booster/security/advisories/new) on GitHub,
or by mail to sachesi <xsachesi@pm.me>. Say what you found, how to reproduce it and which
version you ran; a fix is worked out with you before anything is published.

Only the latest release gets fixes.

## What counts

A daemon runs as root on the system bus for every user, and writes the `dmem` files of
cgroups. The extension tells it which window has focus. Any local user can call
`FocusChanged` and `ClearFocus`, so what the daemon takes from those calls is where a
mistake matters most:

- A caller only boosts, and takes back, what lies in its own `user-<uid>.slice`. A way to
  change the boost of another user's programs, or to take away another user's boost, is a
  vulnerability.
- The daemon writes `dmem` files under `/sys/fs/cgroup` and nothing else. A PID, a cgroup
  name or anything else a caller controls that makes it write another file or another
  value is a vulnerability. So is a way to get past the limits of its systemd unit, which
  holds it to that directory and `CAP_DAC_OVERRIDE`.
- A call that crashes the daemon or keeps it from answering is a vulnerability, since it
  affects every user of the computer.
- The bus policy. A method beyond the two above and the read-only properties that a user
  other than root can reach is a vulnerability.

A user boosting their own focused app is what the project does, and it is not a
vulnerability that this leaves less VRAM to other programs, including other users'. The
stutter or eviction a large boost can cause is a matter of tuning; please file it as an
ordinary issue. A flaw in the kernel's `dmem` controller or in dmemcg-booster belongs with
those projects.
