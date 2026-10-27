// Настройки расширения Pearl Miner.
//
// Здесь только быстрые действия: сами настройки (лимит питания, частоты,
// пул, кошелёк) живут в приложении на GTK4/libadwaita — чтобы панель не
// превращалась в форму и оставалась «живой карточкой».

import Adw from 'gi://Adw';
import GLib from 'gi://GLib';
import Gtk from 'gi://Gtk';
import Gio from 'gi://Gio';

import {ExtensionPreferences} from 'resource:///org/gnome/Shell/Extensions/js/extensions/prefs.js';

function launchApp() {
    const info = Gio.DesktopAppInfo.new('org.pearl.Miner.desktop');
    if (info) {
        info.launch([], null);
        return;
    }
    try {
        Gio.Subprocess.new(['pearl-miner', 'gui'], Gio.SubprocessFlags.NONE);
    } catch (e) {
        logError(e, 'pearl-miner: не удалось открыть приложение');
    }
}

function openPath(path) {
    try {
        Gio.AppInfo.launch_default_for_uri(GLib.filename_to_uri(path, null), null);
    } catch (e) {
        logError(e, 'pearl-miner: не удалось открыть путь');
    }
}

function makeActionRow(title, subtitle, buttonLabel, callback) {
    const row = new Adw.ActionRow({ title, subtitle });
    const button = new Gtk.Button({
        label: buttonLabel,
        valign: Gtk.Align.CENTER,
    });
    button.connect('clicked', callback);
    row.add_suffix(button);
    row.activatable_widget = button;
    return row;
}

export default class PearlMinerPreferences extends ExtensionPreferences {
    fillPreferencesWindow(window) {
        const page = new Adw.PreferencesPage({
            title: 'Pearl Miner',
            icon_name: 'utilities-system-monitor-symbolic',
        });

        const appGroup = new Adw.PreferencesGroup({
            title: 'Приложение',
            description: 'Все настройки — питание, частоты, пул, кошелёк, сравнение майнеров — в приложении.',
        });

        const openRow = makeActionRow(
            'Открыть Pearl Miner',
            'Дашборд, настройки GPU и сравнение майнеров',
            'Открыть',
            () => launchApp(),
        );
        appGroup.add(openRow);

        appGroup.add(makeActionRow(
            'Лог майнера',
            'Последние строки WildRig Multi',
            'Открыть',
            () => openPath(`${GLib.get_home_dir()}/.local/state/pearl-miner/miner.log`),
        ));

        appGroup.add(makeActionRow(
            'Файл настроек',
            'config.json рядом с приложением',
            'Папка',
            () => openPath(`${GLib.get_home_dir()}/.config/pearl-miner`),
        ));

        page.add(appGroup);

        const infoGroup = new Adw.PreferencesGroup({
            title: 'О расширении',
            description: 'Цифры в панели приходят от фонового сервиса org.pearl.Miner.',
        });
        infoGroup.add(new Adw.ActionRow({
            title: 'Версия расширения',
            subtitle: `${this.metadata['version-name'] ?? this.metadata.version}`,
        }));
        infoGroup.add(new Adw.ActionRow({
            title: 'Если панель показывает «сервис не запущен»',
            subtitle: 'systemctl --user start pearl-minerd',
        }));
        page.add(infoGroup);

        window.add(page);
    }
}
