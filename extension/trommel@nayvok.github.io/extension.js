// SPDX-License-Identifier: GPL-3.0-or-later
// Trommel — GNOME Shell 50 panel indicator.
//
// The extension is a thin client: it never runs nvidia-smi and never applies
// overclocking. All state comes from trommeld over D-Bus.

import Clutter from 'gi://Clutter';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import GObject from 'gi://GObject';
import St from 'gi://St';

import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as PanelMenu from 'resource:///org/gnome/shell/ui/panelMenu.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';
import * as QuickSettings from 'resource:///org/gnome/shell/ui/quickSettings.js';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

const BUS_NAME = 'io.github.nayvok.Trommel.Daemon';
const OBJECT_PATH = '/io/github/nayvok/Trommel';
const IFACE = 'io.github.nayvok.Trommel';

const RU = GLib.get_language_names().some(name => name.startsWith('ru'));

function tr(en, ru) {
    return RU ? ru : en;
}

function fmtHashrate(hs) {
    if (!hs || hs <= 0)
        return '—';
    if (hs >= 1e15)
        return `${(hs / 1e15).toFixed(2)} PH/s`;
    if (hs >= 1e12)
        return `${(hs / 1e12).toFixed(2)} TH/s`;
    if (hs >= 1e9)
        return `${(hs / 1e9).toFixed(2)} GH/s`;
    if (hs >= 1e6)
        return `${(hs / 1e6).toFixed(2)} MH/s`;
    return `${Math.round(hs)} H/s`;
}

function fmtShort(hs) {
    if (!hs || hs <= 0)
        return '—';
    if (hs >= 1e15)
        return `${(hs / 1e15).toFixed(2)}P`;
    if (hs >= 1e12)
        return `${(hs / 1e12).toFixed(1)}T`;
    if (hs >= 1e9)
        return `${(hs / 1e9).toFixed(1)}G`;
    if (hs >= 1e6)
        return `${(hs / 1e6).toFixed(1)}M`;
    return `${Math.round(hs)}`;
}

function fmtDuration(sec) {
    const total = Math.max(0, Math.floor(sec ?? 0));
    const days = Math.floor(total / 86400);
    const h = Math.floor((total % 86400) / 3600);
    const m = Math.floor((total % 3600) / 60);
    const s = total % 60;
    if (days > 0)
        return `${days}d ${h}h`;
    if (h > 0)
        return `${h}:${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}`;
    return `${m}:${String(s).padStart(2, '0')}`;
}

function label(text, styleClass) {
    return new St.Label({
        text,
        style_class: styleClass,
        y_align: Clutter.ActorAlign.CENTER,
    });
}

function item(child, reactive = false) {
    const row = new PopupMenu.PopupBaseMenuItem({reactive, can_focus: reactive});
    row.add_style_class_name('trommel-card');
    if (child)
        row.add_child(child);
    return row;
}

class TrommelToggle extends QuickSettings.QuickToggle {
    static {
        GObject.registerClass(this);
    }

    constructor(onToggle) {
        super({
            title: 'Trommel',
            subtitle: tr('Mining', 'Майнинг'),
            iconName: 'utilities-system-monitor-symbolic',
            toggleMode: true,
        });
        this._onToggle = onToggle;
        this.connect('clicked', () => this._onToggle());
    }

    setRunning(running) {
        if (this.checked !== running)
            this.checked = running;
        this.subtitle = running ? tr('Mining', 'Майнинг') : tr('Stopped', 'Остановлен');
    }
}

export default class TrommelExtension extends Extension {
    enable() {
        this._settings = this.getSettings();
        this._proxy = null;
        this._status = null;
        this._serviceMissing = false;
        this._signalIds = [];
        this._ownerId = 0;

        this._path = this.path;
        const iconFile = GLib.build_filenamev([this._path, 'icons', 'trommel-symbolic.svg']);
        this._gicon = GLib.file_test(iconFile, GLib.FileTest.EXISTS)
            ? Gio.icon_new_for_string(iconFile)
            : null;

        this._indicator = new PanelMenu.Button(0.5, 'Trommel', false);
        this._buildPanel();
        this._buildMenu();
        Main.panel.addToStatusArea(this.uuid, this._indicator, 0, 'right');

        this._settingsIds = [];
        for (const key of ['panel-mode', 'live-activity-enabled', 'quick-toggle-enabled']) {
            this._settingsIds.push(this._settings.connect(`changed::${key}`, () => {
                this._applyPanelMode();
                this._applyLiveActivity();
                this._syncQuickToggle();
            }));
        }

        this._syncQuickToggle();
        this._connectProxy();
    }

    disable() {
        for (const id of this._settingsIds ?? [])
            this._settings?.disconnect(id);
        this._settingsIds = [];
        if (this._proxy) {
            for (const id of this._signalIds ?? [])
                this._proxy.disconnectSignal(id);
            if (this._ownerId)
                this._proxy.disconnect(this._ownerId);
        }
        this._signalIds = [];
        this._ownerId = 0;
        this._destroyQuickToggle();
        this._indicator?.destroy();
        this._indicator = null;
        this._proxy = null;
        this._status = null;
        this._settings = null;
    }

    _buildPanel() {
        const box = new St.BoxLayout({
            style_class: 'panel-status-indicators-box',
            y_align: Clutter.ActorAlign.CENTER,
        });
        this._icon = new St.Icon({
            gicon: this._gicon,
            icon_name: this._gicon ? null : 'utilities-system-monitor-symbolic',
            style_class: 'system-status-icon',
        });
        this._panelLabel = label('', 'trommel-panel-label');
        this._panelLabel.visible = false;
        box.add_child(this._icon);
        box.add_child(this._panelLabel);
        this._indicator.add_child(box);
    }

    _buildMenu() {
        this._indicator.menu.box.add_style_class_name('trommel-menu');

        const header = new St.BoxLayout({x_expand: true});
        this._title = label('Trommel', 'trommel-title');
        this._title.x_expand = true;
        this._pill = label(tr('checking…', 'проверяю…'), 'trommel-pill');
        header.add_child(this._title);
        header.add_child(this._pill);
        this._indicator.menu.addMenuItem(item(header));

        this._subtitle = label('', 'trommel-caption');
        this._indicator.menu.addMenuItem(item(this._subtitle));

        this._action = new St.Button({
            style_class: 'button',
            x_expand: true,
            can_focus: true,
        });
        this._actionLabel = label(tr('Start mining', 'Запустить майнинг'), '');
        this._action.set_child(this._actionLabel);
        this._action.connect('clicked', () => this._toggle());
        this._indicator.menu.addMenuItem(item(this._action));

        const card = new St.BoxLayout({
            vertical: true,
            x_expand: true,
            style_class: 'trommel-card',
        });
        this._hashrate = label('—', 'trommel-value');
        this._hashrate.x_expand = true;
        this._liveCard = new St.BoxLayout({vertical: true, x_expand: true});
        this._liveCard.add_child(this._hashrate);
        this._liveMeta = label('', 'trommel-caption trommel-mono');
        this._liveCard.add_child(this._liveMeta);
        this._gpuLine = label('', 'trommel-caption trommel-mono');
        this._liveCard.add_child(this._gpuLine);
        card.add_child(this._liveCard);
        this._indicator.menu.addMenuItem(item(card));
        this._minerLine = label('', 'trommel-caption');
        this._indicator.menu.addMenuItem(item(this._minerLine));

        this._sharesLine = label('', 'trommel-caption trommel-mono');
        this._indicator.menu.addMenuItem(item(this._sharesLine));

        this._poolLine = label('', 'trommel-caption trommel-mono');
        this._indicator.menu.addMenuItem(item(this._poolLine));

        const footer = new St.BoxLayout({x_expand: true, y_align: Clutter.ActorAlign.CENTER});
        this._open = new St.Button({
            style_class: 'button',
            can_focus: true,
            label: tr('Open Trommel', 'Открыть Trommel'),
        });
        this._open.connect('clicked', () => this._openApp());
        footer.add_child(this._open);
        this._indicator.menu.addMenuItem(item(footer));

        this._applyPanelMode();
        this._applyLiveActivity();
        this._setIdle();
    }

    _connectProxy() {
        Gio.DBusProxy.new(
            Gio.DBus.session,
            Gio.DBusProxyFlags.NONE,
            null,
            BUS_NAME,
            OBJECT_PATH,
            IFACE,
            null,
            (source, res) => {
                let proxy;
                try {
                    proxy = Gio.DBusProxy.new_finish(res);
                } catch (e) {
                    this._setServiceMissing(e.message);
                    return;
                }
                this._proxy = proxy;
                this._signalIds = [];
                this._signalIds.push(proxy.connectSignal('StatusChanged',
                    (p, sender, params) => this._onStatus(params[0])));
                this._signalIds.push(proxy.connectSignal('Notification',
                    (p, sender, params) => {
                        if (this._settings.get_boolean('notifications-enabled'))
                            Main.notify(params[0], params[1]);
                    }));
                this._ownerId = proxy.connect('notify::g-name-owner', () => this._syncService());
                this._syncService();
                this._refresh();
            });
    }

    _syncService() {
        const owner = this._proxy?.get_name_owner();
        if (owner) {
            this._serviceMissing = false;
            this._title.text = 'Trommel';
            return;
        }
        this._serviceMissing = true;
        this._title.text = tr('Trommel: service is not running', 'Trommel: сервис не запущен');
        this._pill.text = tr('offline', 'нет связи');
        this._pill.style_class = 'trommel-pill trommel-pill-error';
        this._open.set_sensitive(false);
        Gio.DBus.session.call(
            'org.freedesktop.DBus',
            '/org/freedesktop/DBus',
            'org.freedesktop.DBus',
            'StartServiceByName',
            new GLib.Variant('(su)', [BUS_NAME, 0]),
            null,
            Gio.DBusCallFlags.NONE,
            3000,
            null,
            () => {});
    }

    _setServiceMissing(message) {
        this._serviceMissing = true;
        this._title.text = tr('Trommel: service is not running', 'Trommel: сервис не запущен');
        this._subtitle.text = message ?? '';
        this._pill.text = tr('offline', 'нет связи');
        this._pill.style_class = 'trommel-pill trommel-pill-error';
        this._open.set_sensitive(false);
    }

    _call(method, callback) {
        if (!this._proxy || !this._proxy.get_name_owner()) {
            callback?.(new Error('service is not running'));
            return;
        }
        this._proxy.call(method, null, Gio.DBusCallFlags.NONE, 15000, null,
            (proxy, res) => {
                let error = null;
                let value = null;
                try {
                    const variant = proxy.call_finish(res);
                    value = variant ? variant.deep_unpack() : null;
                } catch (e) {
                    error = e;
                }
                callback?.(error, value);
            });
    }

    _refresh() {
        this._call('Refresh', (error, value) => {
            if (error) {
                this._setServiceMissing(error.message);
                return;
            }
            if (typeof value === 'string')
                this._onStatus(value);
        });
    }

    _toggle() {
        this._action.reactive = false;
        this._actionLabel.text = tr('Switching…', 'Переключаю…');
        this._call('Toggle', error => {
            this._action.reactive = true;
            if (error) {
                Main.notify('Trommel', error.message);
                this._setIdle();
                return;
            }
            this._refresh();
        });
    }

    _openApp() {
        this._indicator.menu.close();
        this._call('OpenApp', error => {
            if (!error)
                return;
            try {
                const info = Gio.DesktopAppInfo.new('io.github.nayvok.Trommel.desktop');
                if (info)
                    info.launch([], null);
            } catch (e) {
                logError(e, 'trommel: cannot open app');
            }
        });
    }

    _onStatus(json) {
        try {
            this._status = JSON.parse(json);
        } catch (e) {
            logError(e, 'trommel: cannot parse status');
            return;
        }
        this._apply(this._status);
    }

    _apply(status) {
        const running = status.state?.running ?? false;
        const miner = status.miner ?? {};
        const total = status.total ?? {};
        const info = status.miner_info ?? {};
        const primary = total.hashrate ?? miner.hashrate_10s ?? miner.hashrate_60s;
        const temp = total.temp_max_c ?? status.gpu?.temp_c;

        this._panelLabel.text = '';
        const mode = this._settings.get_string('panel-mode');
        if (running && mode !== 'icon') {
            if (mode === 'hashrate')
                this._panelLabel.text = fmtShort(primary);
            else if (mode === 'temperature')
                this._panelLabel.text = temp != null ? `${temp}°` : '—';
            else
                this._panelLabel.text = `${fmtShort(primary)}${temp != null ? ` · ${temp}°` : ''}`;
            this._panelLabel.visible = true;
        } else {
            this._panelLabel.visible = false;
        }

        this._pill.text = running
            ? (status.state.external ? tr('external', 'вручную') : tr('mining', 'майнится'))
            : tr('stopped', 'остановлен');
        this._pill.style_class = running
            ? 'trommel-pill trommel-pill-ok'
            : 'trommel-pill';
        this._actionLabel.text = running ? tr('Stop mining', 'Остановить майнинг') : tr('Start mining', 'Запустить майнинг');
        this._action.reactive = !this._serviceMissing;
        this._open.set_sensitive(true);

        this._hashrate.text = primary ? fmtHashrate(primary) : (running ? tr('warming up…', 'прогрев…') : '—');
        this._liveMeta.text = [
            total.power_w != null ? `${Math.round(total.power_w)} W` : null,
            temp != null ? `${temp} / ${status.temp_limit ?? 90} °C` : null,
            total.efficiency_j_per_th != null ? `${total.efficiency_j_per_th.toFixed(2)} J/TH` : null,
            running ? fmtDuration(status.state.uptime_s) : null,
        ].filter(Boolean).join(' · ');
        this._minerLine.text = [
            info.name,
            info.version,
            info.total_fee_pct != null
                ? `${info.miner_fee_pct.toFixed(2)}% + ${info.pool_fee_pct.toFixed(2)}% = ${info.total_fee_pct.toFixed(2)}%`
                : null,
        ].filter(Boolean).join(' · ');
        this._sharesLine.text = `${miner.accepted ?? 0} ${tr('accepted', 'принято')} · ${miner.rejected ?? 0} ${tr('rejected', 'отклонено')}`;

        const gpus = status.gpus ?? [];
        if (gpus.length) {
            const visible = gpus.slice(0, 3).map(gpu =>
                `#${gpu.index} ${gpu.temp_c != null ? `${gpu.temp_c}°` : '—'} ${gpu.power_w != null ? `${Math.round(gpu.power_w)}W` : ''}`.trim());
            if (gpus.length > 3)
                visible.push(`+${gpus.length - 3}`);
            this._gpuLine.text = visible.join(' · ');
        } else {
            this._gpuLine.text = status.gpu_error ?? '';
        }

        if (status.pool?.balance != null) {
            this._poolLine.text = `${tr('Pool', 'Пул')}: ${status.pool.balance.toFixed(3)} PRL`;
        } else {
            this._poolLine.text = '';
        }

        this._subtitle.text = status.state?.preset_name ?? '';
        this._quickToggle?.setRunning(running);
    }

    _setIdle() {
        this._actionLabel.text = tr('Start mining', 'Запустить майнинг');
        this._action.reactive = true;
    }

    _applyPanelMode() {
        if (this._status)
            this._apply(this._status);
        else
            this._panelLabel.visible = false;
    }

    _applyLiveActivity() {
        const show = this._settings.get_boolean('live-activity-enabled');
        this._liveCard.visible = show;
    }

    _syncQuickToggle() {
        const enabled = this._settings.get_boolean('quick-toggle-enabled');
        if (enabled && !this._quickToggle)
            this._createQuickToggle();
        else if (!enabled && this._quickToggle)
            this._destroyQuickToggle();
    }

    _createQuickToggle() {
        this._quickIndicator = new QuickSettings.SystemIndicator();
        this._quickToggle = new TrommelToggle(() => this._toggle());
        this._quickIndicator.quickSettingsItems.push(this._quickToggle);
        Main.panel.statusArea.quickSettings.addExternalIndicator(this._quickIndicator);
        if (this._status)
            this._quickToggle.setRunning(this._status.state?.running ?? false);
    }

    _destroyQuickToggle() {
        this._quickIndicator?.destroy();
        this._quickIndicator = null;
        this._quickToggle = null;
    }
}
