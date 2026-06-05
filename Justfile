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

default: build

build:
    cd daemon && cargo build --release

check-deps:
    @systemctl is-active --quiet dmemcg-booster.service || \
        { echo "ERROR: dmemcg-booster.service is not running. Install and start it first."; exit 1; }
    @echo "dmemcg-booster.service OK"

install: check-deps build
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

uninstall:
    -sudo systemctl disable --now {{binary}}.service
    -sudo rm -f {{bin_dest}} {{ctl_dest}} {{service_dir}}/{{binary}}.service {{dbus_dir}}/org.gnome.VramBooster.conf
    -sudo systemctl daemon-reload
    -sudo rm -rf {{ext_dest}}
    @echo ""
    @echo "Uninstalled. Log out and back in to deactivate the extension."

reload: build
    sudo install -Dm755 {{bin_src}} {{bin_dest}}
    sudo cp -r {{ext_src}}/. {{ext_dest}}/
    sudo chmod -R a+rX {{ext_dest}}
    sudo glib-compile-schemas {{ext_dest}}/schemas/
    sudo systemctl restart {{binary}}.service
    @echo "Daemon restarted. Log out and back in to reload the extension."

logs:
    sudo journalctl -u {{binary}}.service -f --no-pager
