import Clutter from 'gi://Clutter';
import GLib from 'gi://GLib';
import Gio from 'gi://Gio';
import Meta from 'gi://Meta';
import St from 'gi://St';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as PanelMenu from 'resource:///org/gnome/shell/ui/panelMenu.js';

const VRAM_BOOSTER_IFACE = `
<node>
  <interface name="org.gnome.VramBooster">
    <method name="FocusChanged">
      <arg type="u" direction="in" name="pid"/>
      <arg type="b" direction="out" name="boosted"/>
    </method>
  </interface>
</node>`;

const DEBOUNCE_MS = 300;

const PORTAL_WM_CLASSES = new Set([
    'nautilus',
    'org.gnome.nautilus',
    'xdg-desktop-portal-gnome',
]);

export default class VramBoosterExtension extends Extension {
    _readSelfPid() {
        try {
            const [, bytes] = GLib.file_get_contents('/proc/self/stat');
            return parseInt(new TextDecoder().decode(bytes));
        } catch {
            return 0;
        }
    }

    _getAppName(win) {
        const cls = win.get_wm_class() ?? '';
        if (cls) {
            const parts = cls.split('.');
            const name = parts[parts.length - 1] || cls;
            return name.charAt(0).toUpperCase() + name.slice(1);
        }
        return win.get_title() ?? 'App';
    }

    _syncIndicator() {
        const show = this._settings.get_boolean('debug-show-active');
        if (show && !this._indicator) {
            this._indicator = new PanelMenu.Button(0.0, 'VRAM Booster', true);
            this._indicatorLabel = new St.Label({
                text: 'VRAM: idle',
                y_align: Clutter.ActorAlign.CENTER,
                style: 'margin: 0 4px;',
            });
            this._indicator.add_child(this._indicatorLabel);
            Main.panel.addToStatusArea('vram-booster', this._indicator);
            if (this._currentApp)
                this._indicatorLabel.set_text(`VRAM: ${this._currentApp}`);
        } else if (!show && this._indicator) {
            this._indicator.destroy();
            this._indicator = null;
            this._indicatorLabel = null;
        }
    }

    _updateIndicatorText(appName) {
        this._currentApp = appName;
        if (this._indicatorLabel)
            this._indicatorLabel.set_text(appName ? `VRAM: ${appName}` : 'VRAM: idle');
    }

    enable() {
        this._shellPid = this._readSelfPid();
        this._debounceId = null;
        this._proxy = null;
        this._daemonWatchId = 0;
        this._indicator = null;
        this._indicatorLabel = null;
        this._currentApp = null;

        this._settings = this.getSettings();
        this._settingsSig = this._settings.connect('changed::debug-show-active', () => {
            this._syncIndicator();
        });
        this._syncIndicator();

        this._daemonWatchId = Gio.DBus.system.watch_name(
            'org.gnome.VramBooster',
            Gio.BusNameWatcherFlags.NONE,
            () => this._onDaemonAppeared(),
            () => {
                this._proxy = null;
                this._updateIndicatorText(null);
            }
        );

        this._focusSig = global.display.connect(
            'notify::focus-window',
            () => this._onFocusChanged()
        );
    }

    disable() {
        if (this._focusSig) {
            global.display.disconnect(this._focusSig);
            this._focusSig = null;
        }
        if (this._debounceId) {
            GLib.source_remove(this._debounceId);
            this._debounceId = null;
        }
        if (this._daemonWatchId) {
            Gio.DBus.system.unwatch_name(this._daemonWatchId);
            this._daemonWatchId = 0;
        }
        if (this._settingsSig) {
            this._settings.disconnect(this._settingsSig);
            this._settingsSig = null;
        }
        if (this._indicator) {
            this._indicator.destroy();
            this._indicator = null;
            this._indicatorLabel = null;
        }
        this._proxy = null;
        this._settings = null;
        this._currentApp = null;
    }

    _onDaemonAppeared() {
        const VramBoosterProxy = Gio.DBusProxy.makeProxyWrapper(VRAM_BOOSTER_IFACE);
        try {
            this._proxy = new VramBoosterProxy(
                Gio.DBus.system,
                'org.gnome.VramBooster',
                '/org/gnome/VramBooster',
                null
            );
        } catch (e) {
            console.error('[vram-booster] proxy creation failed:', e.message);
            this._proxy = null;
        }
    }

    _onFocusChanged() {
        const win = global.display.focus_window;
        if (!win)
            return;
        if (win.get_window_type() !== Meta.WindowType.NORMAL)
            return;

        const pid = win.get_pid();
        if (!pid || pid <= 0 || pid === this._shellPid)
            return;

        const wmClass = (win.get_wm_class() ?? '').toLowerCase();
        if (PORTAL_WM_CLASSES.has(wmClass))
            return;

        const appName = this._getAppName(win);

        if (this._debounceId) {
            GLib.source_remove(this._debounceId);
            this._debounceId = null;
        }

        this._debounceId = GLib.timeout_add(GLib.PRIORITY_DEFAULT, DEBOUNCE_MS, () => {
            this._debounceId = null;
            if (this._proxy) {
                try {
                    this._proxy.FocusChangedRemote(pid, (result, error) => {
                        if (error) {
                            console.error('[vram-booster] D-Bus call failed:', error.message);
                            return;
                        }
                        const boosted = result && result[0];
                        this._updateIndicatorText(boosted ? appName : null);
                    });
                } catch (e) {
                    console.error('[vram-booster] D-Bus call failed:', e.message);
                }
            }
            return GLib.SOURCE_REMOVE;
        });
    }
}
