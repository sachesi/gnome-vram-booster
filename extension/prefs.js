import Adw from 'gi://Adw';
import Gio from 'gi://Gio';
import {ExtensionPreferences} from 'resource:///org/gnome/Shell/Extensions/js/extensions/prefs.js';

export default class VramBoosterPrefs extends ExtensionPreferences {
    fillPreferencesWindow(window) {
        const settings = this.getSettings();

        const page = new Adw.PreferencesPage();
        const group = new Adw.PreferencesGroup({title: 'Debug'});

        const row = new Adw.SwitchRow({
            title: 'Show active app in panel',
            subtitle: 'Display which app currently holds VRAM priority',
        });
        settings.bind(
            'debug-show-active',
            row,
            'active',
            Gio.SettingsBindFlags.DEFAULT
        );

        group.add(row);
        page.add(group);
        window.add(page);
    }
}
