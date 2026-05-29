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

export default class VramBoosterExtension extends Extension {
    _readSelfPid() {
        try {
            const [, bytes] = GLib.file_get_contents('/proc/self/stat');
            const text = new TextDecoder().decode(bytes);
            const pid = parseInt(text.split(' ')[0], 10);
            return Number.isFinite(pid) ? pid : 0;
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

            this._indicatorBox = new St.BoxLayout({
                vertical: false,
                style_class: 'panel-status-indicators-box',
            });

            this._indicatorIcon = new St.Icon({
                icon_name: 'video-display-symbolic',
                style_class: 'system-status-icon',
                y_align: Clutter.ActorAlign.CENTER,
            });

            this._indicatorLabel = new St.Label({
                text: 'idle',
                y_align: Clutter.ActorAlign.CENTER,
                style: 'margin-left: 4px;',
            });

            this._indicatorBox.add_child(this._indicatorIcon);
            this._indicatorBox.add_child(this._indicatorLabel);
            this._indicator.add_child(this._indicatorBox);

            Main.panel.addToStatusArea('vram-booster', this._indicator);
            this._updateIndicatorText(this._currentApp);
        } else if (!show && this._indicator) {
            this._indicator.destroy();
            this._indicator = null;
            this._indicatorBox = null;
            this._indicatorIcon = null;
            this._indicatorLabel = null;
        }
    }

    _updateIndicatorText(appName) {
        this._currentApp = appName;
        if (this._indicatorLabel) {
            if (!this._daemonOnline)
                this._indicatorLabel.set_text('offline');
            else
                this._indicatorLabel.set_text(appName ? appName : 'idle');
        }
    }

    enable() {
        this._shellPid = this._readSelfPid();
        this._debounceId = null;
        this._lastPid = 0;
        this._proxy = null;
        this._daemonWatchId = 0;
        this._indicator = null;
        this._indicatorLabel = null;
        this._currentApp = null;
        this._daemonOnline = false;
        this._enabled = true;

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
                this._daemonOnline = false;
                this._lastPid = 0;
                this._updateIndicatorText(null);
            }
        );

        this._focusSig = global.display.connect(
            'notify::focus-window',
            () => this._onFocusChanged()
        );
    }

    disable() {
        this._enabled = false;
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
            this._indicatorBox = null;
            this._indicatorIcon = null;
            this._indicatorLabel = null;
        }
        this._proxy = null;
        this._daemonOnline = false;
        this._settings = null;
        this._currentApp = null;
        this._lastPid = 0;
    }

    _onDaemonAppeared() {
        this._daemonOnline = true;
        const VramBoosterProxy = Gio.DBusProxy.makeProxyWrapper(VRAM_BOOSTER_IFACE);
        try {
            this._proxy = new VramBoosterProxy(
                Gio.DBus.system,
                'org.gnome.VramBooster',
                '/org/gnome/VramBooster',
                null
            );
            this._lastPid = 0;
            this._onFocusChanged();
        } catch (e) {
            console.error('[vram-booster] Failed to connect to daemon D-Bus interface:', e.message);
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

        if (pid === this._lastPid)
            return;

        const wmClass = (win.get_wm_class() ?? '').toLowerCase();
        const excluded = this._settings.get_strv('excluded-wm-classes');
        if (excluded.some(c => c === wmClass))
            return;

        const appName = this._getAppName(win);

        if (this._debounceId) {
            console.log('[vram-booster] debounce reset, pid', pid, 'app', appName);
            GLib.source_remove(this._debounceId);
            this._debounceId = null;
        }

        this._debounceId = GLib.timeout_add(GLib.PRIORITY_DEFAULT, DEBOUNCE_MS, () => {
            this._debounceId = null;
            if (!this._enabled)
                return GLib.SOURCE_REMOVE;
            this._lastPid = pid;
            if (this._proxy) {
                try {
                    this._proxy.FocusChangedRemote(pid, (result, error) => {
                        if (error) {
                            console.error(`[vram-booster] Failed to notify daemon of focus change for PID ${pid}:`, error.message);
                            return;
                        }
                        const boosted = result && result[0];
                        this._updateIndicatorText(boosted ? appName : null);
                    });
                } catch (e) {
                    console.error(`[vram-booster] Failed to invoke FocusChangedRemote for PID ${pid}:`, e.message);
                }
            }
            return GLib.SOURCE_REMOVE;
        });
    }
}
