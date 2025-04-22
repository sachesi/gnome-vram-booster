import GLib from 'gi://GLib';
import Gio from 'gi://Gio';
import Meta from 'gi://Meta';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

const VRAM_BOOSTER_IFACE = `
<node>
  <interface name="org.gnome.VramBooster">
    <method name="FocusChanged">
      <arg type="u" direction="in" name="pid"/>
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

    enable() {
        this._shellPid = this._readSelfPid();
        this._debounceId = null;
        this._proxy = null;
        this._daemonWatchId = 0;

        this._daemonWatchId = Gio.DBus.system.watch_name(
            'org.gnome.VramBooster',
            Gio.BusNameWatcherFlags.NONE,
            () => this._onDaemonAppeared(),
            () => { this._proxy = null; }
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
        this._proxy = null;
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

        if (this._debounceId) {
            GLib.source_remove(this._debounceId);
            this._debounceId = null;
        }

        this._debounceId = GLib.timeout_add(GLib.PRIORITY_DEFAULT, DEBOUNCE_MS, () => {
            this._debounceId = null;
            if (this._proxy) {
                try {
                    this._proxy.FocusChangedRemote(pid, null);
                } catch (e) {
                    console.error('[vram-booster] D-Bus call failed:', e.message);
                }
            }
            return GLib.SOURCE_REMOVE;
        });
    }
}
