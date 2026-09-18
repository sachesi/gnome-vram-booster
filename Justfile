# gnome-vram-booster build and install tasks.
#
# `build` needs a Rust toolchain; `install` only copies what is already in
# daemon/target/release, so the two can run on different machines. Run
# `install` as your user, not under sudo: it calls sudo itself.
#
#   just build
#   just install

set shell := ["bash", "-euo", "pipefail", "-c"]

binary      := "gnome-vram-booster"
ctl         := "gnome-vram-boosterctl"
bin_src     := "daemon/target/release/" + binary
ctl_src     := "daemon/target/release/" + ctl
bin_dest    := "/usr/bin/" + binary
ctl_dest    := "/usr/bin/" + ctl
service     := "packaging/usr/lib/systemd/system/" + binary + ".service"
service_dir := "/usr/lib/systemd/system"
dbus_conf   := "packaging/usr/share/dbus-1/system.d/org.gnome.VramBooster.conf"
dbus_dir    := "/usr/share/dbus-1/system.d"
ext_uuid    := "vram-booster@local"
ext_src     := "extension"
ext_dest    := "/usr/share/gnome-shell/extensions/" + ext_uuid

default:
    @just --list

# Release build of the daemon and the ctl.
build:
    cd daemon && cargo build --release

# Lints, unit tests and the extension's schema.
check:
    cd daemon && cargo fmt --check
    cd daemon && cargo clippy -- -D warnings
    cd daemon && cargo test
    glib-compile-schemas --strict extension/schemas

# Binaries come from `just build` (or any other checkout/toolchain); install
# never builds, so a host without a Rust toolchain can still install.
[private]
check-bins:
    @test -x {{bin_src}} && test -x {{ctl_src}} || \
        { echo "error: {{bin_src}} or {{ctl_src}} missing; run 'just build' first (needs a Rust toolchain)" >&2; exit 1; }

# Under sudo, `systemctl --user` reaches root's user manager, not yours.
[private]
not-root:
    @test "$(id -u)" -ne 0 || \
        { echo "error: run 'just install' as your user, not under sudo; it calls sudo itself" >&2; exit 1; }

# Check the kernel and dmemcg-booster this machine runs.
check-deps: not-root
    @grep -qw dmem /sys/fs/cgroup/cgroup.controllers || \
        { echo "ERROR: 'dmem' controller missing from /sys/fs/cgroup/cgroup.controllers. Need kernel 6.14+ with dmem cgroup support, 6.15+ for amdgpu."; exit 1; }
    @systemctl is-active --quiet dmemcg-booster.service || \
        { echo "ERROR: system dmemcg-booster.service is not active. Run: sudo systemctl enable --now dmemcg-booster.service"; exit 1; }
    @systemctl --user is-active --quiet dmemcg-booster.service || \
        { echo "ERROR: user dmemcg-booster.service is not active. Run: systemctl --user enable --now dmemcg-booster.service"; exit 1; }
    @echo "deps OK: dmem controller present, system + user dmemcg-booster active"

# Install the release build, the system service and the extension. Does not build: run `just build` first.
install: check-deps check-bins
    sudo install -Dm755 {{bin_src}} {{bin_dest}}
    sudo install -Dm755 {{ctl_src}} {{ctl_dest}}
    sudo install -Dm644 {{service}} {{service_dir}}/{{binary}}.service
    sudo install -Dm644 {{dbus_conf}} {{dbus_dir}}/org.gnome.VramBooster.conf
    sudo systemctl daemon-reload
    sudo systemctl enable --now {{binary}}.service
    sudo mkdir -p {{ext_dest}}
    sudo cp -r {{ext_src}}/. {{ext_dest}}/
    sudo chmod -R a+rX {{ext_dest}}
    sudo glib-compile-schemas {{ext_dest}}/schemas/
    @echo ""
    @echo "Installed. Log out and back in, then enable 'GNOME VRAM Booster' in the Extensions app."

# Disable the service and remove what install put in place.
uninstall:
    -sudo systemctl disable --now {{binary}}.service
    -sudo rm -f {{bin_dest}} {{ctl_dest}} {{service_dir}}/{{binary}}.service {{dbus_dir}}/org.gnome.VramBooster.conf
    -sudo systemctl daemon-reload
    -sudo rm -rf {{ext_dest}}
    @echo ""
    @echo "Uninstalled. Log out and back in to deactivate the extension."

# Reinstall the built binaries and the extension, and restart the daemon.
reload: check-bins
    sudo install -Dm755 {{bin_src}} {{bin_dest}}
    sudo install -Dm755 {{ctl_src}} {{ctl_dest}}
    sudo cp -r {{ext_src}}/. {{ext_dest}}/
    sudo chmod -R a+rX {{ext_dest}}
    sudo glib-compile-schemas {{ext_dest}}/schemas/
    sudo systemctl restart {{binary}}.service
    @echo "Daemon restarted. Log out and back in to reload the extension."

# Follow the daemon's journal.
logs:
    sudo journalctl -u {{binary}}.service -f --no-pager
