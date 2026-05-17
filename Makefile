BINARY      := gnome-vram-booster
BIN_SRC     := daemon/target/release/$(BINARY)
BIN_DEST    := /usr/local/bin/$(BINARY)
SERVICE     := daemon/$(BINARY).service
SERVICE_DIR := /etc/systemd/system
DBUS_CONF   := daemon/org.gnome.VramBooster.conf
DBUS_DIR    := /etc/dbus-1/system.d
EXT_UUID    := vram-booster@local
EXT_SRC     := extension
EXT_DEST    := $(HOME)/.local/share/gnome-shell/extensions/$(EXT_UUID)

.PHONY: all build install uninstall reload logs check-deps

all: build

build:
	cd daemon && cargo build --release

check-deps:
	@systemctl is-active --quiet dmemcg-booster.service || \
		{ echo "ERROR: dmemcg-booster.service is not running. Install and start it first."; exit 1; }
	@echo "dmemcg-booster.service OK"

install: check-deps build
	sudo install -Dm755 $(BIN_SRC) $(BIN_DEST)
	sudo install -Dm644 $(SERVICE) $(SERVICE_DIR)/$(BINARY).service
	sudo install -Dm644 $(DBUS_CONF) $(DBUS_DIR)/org.gnome.VramBooster.conf
	sudo systemctl daemon-reload
	sudo systemctl enable --now $(BINARY).service
	mkdir -p $(EXT_DEST)
	cp -r $(EXT_SRC)/. $(EXT_DEST)/
	gnome-extensions enable $(EXT_UUID) || true
	@echo "Installed. Reload GNOME Shell (Alt+F2 r) to activate extension."

uninstall:
	-sudo systemctl disable --now $(BINARY).service
	-sudo rm -f $(BIN_DEST) $(SERVICE_DIR)/$(BINARY).service $(DBUS_DIR)/org.gnome.VramBooster.conf
	-sudo systemctl daemon-reload
	-gnome-extensions disable $(EXT_UUID) || true
	-rm -rf $(EXT_DEST)
	@echo "Uninstalled."

reload: build
	sudo install -Dm755 $(BIN_SRC) $(BIN_DEST)
	cp -r $(EXT_SRC)/. $(EXT_DEST)/
	sudo systemctl restart $(BINARY).service
	@echo "Reloaded daemon. Reload GNOME Shell (Alt+F2 r) to pick up extension changes."

logs:
	journalctl -u $(BINARY).service -f --no-pager
