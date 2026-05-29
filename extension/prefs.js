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

        const excludeGroup = new Adw.PreferencesGroup({title: 'Exclusions'});
        const excludeRow = new Adw.EntryRow({
            title: 'Excluded WM classes',
            show_apply_button: true,
        });
        excludeRow.set_text(settings.get_strv('excluded-wm-classes').join(', '));
        excludeRow.connect('apply', () => {
            const val = excludeRow.get_text()
                .split(',')
                .map(s => s.trim().toLowerCase())
                .filter(s => s.length > 0);
            settings.set_strv('excluded-wm-classes', val);
        });
        excludeGroup.add(excludeRow);
        page.add(excludeGroup);

        window.add(page);
    }
}
