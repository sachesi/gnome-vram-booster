BINARY      := gnome-vram-booster
CTL         := gnome-vram-boosterctl
BIN_SRC     := daemon/target/release/$(BINARY)
CTL_SRC     := daemon/target/release/$(CTL)
BIN_DEST    := /usr/bin/$(BINARY)
CTL_DEST    := /usr/bin/$(CTL)
SERVICE     := packaging/usr/lib/systemd/system/$(BINARY).service
SERVICE_DIR := /usr/lib/systemd/system
DBUS_CONF   := packaging/usr/share/dbus-1/system.d/org.gnome.VramBooster.conf
DBUS_DIR    := /usr/share/dbus-1/system.d
EXT_UUID    := vram-booster@local
EXT_SRC     := extension
EXT_DEST    := /usr/share/gnome-shell/extensions/$(EXT_UUID)

.PHONY: all install uninstall reload logs check-deps

all: build

build: $(BIN_SRC)

$(BIN_SRC): daemon/src/*.rs daemon/Cargo.toml daemon/Cargo.lock
	cd daemon && cargo build --release

check-deps:
	@systemctl is-active --quiet dmemcg-booster.service || \
		{ echo "ERROR: dmemcg-booster.service is not running. Install and start it first."; exit 1; }
	@echo "dmemcg-booster.service OK"

install: check-deps build
	sudo install -Dm755 $(BIN_SRC) $(BIN_DEST)
	sudo install -Dm755 $(CTL_SRC) $(CTL_DEST)
	sudo install -Dm644 $(SERVICE) $(SERVICE_DIR)/$(BINARY).service
	sudo install -Dm644 $(DBUS_CONF) $(DBUS_DIR)/org.gnome.VramBooster.conf
	sudo systemctl daemon-reload
	sudo systemctl enable --now $(BINARY).service
	sudo mkdir -p $(EXT_DEST)
	sudo cp -r $(EXT_SRC)/. $(EXT_DEST)/
	sudo chmod -R a+rX $(EXT_DEST)
	sudo glib-compile-schemas $(EXT_DEST)/schemas/
	@echo ""
	@echo "Installed. Log out and back in, then enable 'GNOME VRAM Booster' in the Extensions app."

uninstall:
	-sudo systemctl disable --now $(BINARY).service
	-sudo rm -f $(BIN_DEST) $(CTL_DEST) $(SERVICE_DIR)/$(BINARY).service $(DBUS_DIR)/org.gnome.VramBooster.conf
	-sudo systemctl daemon-reload
	-sudo rm -rf $(EXT_DEST)
	@echo ""
	@echo "Uninstalled. Log out and back in to deactivate the extension."

reload: build
	sudo install -Dm755 $(BIN_SRC) $(BIN_DEST)
	sudo cp -r $(EXT_SRC)/. $(EXT_DEST)/
	sudo chmod -R a+rX $(EXT_DEST)
	sudo glib-compile-schemas $(EXT_DEST)/schemas/
	sudo systemctl restart $(BINARY).service
	@echo "Daemon restarted. Log out and back in to reload the extension."

logs:
	sudo journalctl -u $(BINARY).service -f --no-pager
