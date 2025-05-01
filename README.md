# gnome-vram-booster

Dynamic VRAM prioritization for GNOME via Linux dmem cgroups. Keeps the focused app's GPU memory protected from TTM eviction on 8 GB and under GPUs.

GNOME equivalent of KDE's `plasma-foreground-booster`.

## Requirements

- Kernel 6.12+ with `dmem` cgroup controller
- [`dmemcg-booster`](https://pixelcluster.github.io/VRAM-Mgmt-fixed/) running
- GNOME Shell 45–50
- AMD GPU

## Install

```
make install
```

See [docs/install.md](docs/install.md) for full instructions.

## License

GPL-3.0-or-later
