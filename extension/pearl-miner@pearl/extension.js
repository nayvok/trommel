// Pearl Miner — «живая карточка» майнинга в верхней панели GNOME Shell.
//
// Источник данных: фоновый сервис org.pearl.Miner.Daemon (pearl-minerd). Расширение
// ничего не опрашивает само: статус приходит сигналом StatusChanged, поэтому
// цифры в панели и в поповере всегда свежие и не грузят shell.
//
// Оформление — только на цветах темы Shell (`-st-accent-color`, `inherit`,
// `opacity`), чтобы одинаково работать в светлой и тёмной теме.

import Clutter from 'gi://Clutter';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import St from 'gi://St';

import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as PanelMenu from 'resource:///org/gnome/shell/ui/panelMenu.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

const BUS_NAME = 'org.pearl.Miner.Daemon';
const OBJECT_PATH = '/org/pearl/Miner';
const IFACE = 'org.pearl.Miner';
const SPARK_POINTS = 40;
const DEFAULT_TEMP_LIMIT = 90;

// ---------------------------------------------------------------- утилиты

function fmtHashrate(hs) {
    if (!hs || hs <= 0)
        return '—';
    const units = [
        [1e12, 'TH/s'],
        [1e9, 'GH/s'],
        [1e6, 'MH/s'],
        [1e3, 'kH/s'],
        [1, 'H/s'],
    ];
    for (const [scale, name] of units) {
        if (hs >= scale)
            return `${(hs / scale).toFixed(hs / scale >= 100 ? 1 : 2)} ${name}`;
    }
    return '—';
}

function fmtHashrateShort(hs) {
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
    if (hs >= 1e3)
        return `${(hs / 1e3).toFixed(1)}k`;
    return `${Math.round(hs)}`;
}

function fmtDuration(sec) {
    const total = Math.max(0, Math.floor(sec ?? 0));
    const days = Math.floor(total / 86400);
    const h = Math.floor((total % 86400) / 3600);
    const m = Math.floor((total % 3600) / 60);
    const s = total % 60;
    if (days > 0)
        return `${days} д ${h} ч`;
    if (h > 0)
        return `${h}:${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}`;
    return `${m}:${String(s).padStart(2, '0')}`;
}

/** Склонение: 1 секунда, 2 секунды, 5 секунд. */
function plural(value, one, few, many) {
    const n = Math.abs(Math.floor(value)) % 100;
    const n1 = n % 10;
    if (n > 10 && n < 20)
        return many;
    if (n1 === 1)
        return one;
    if (n1 >= 2 && n1 <= 4)
        return few;
    return many;
}

function clamp01(value) {
    if (!Number.isFinite(value))
        return 0;
    return Math.min(1, Math.max(0, value));
}

function makeLabel(text, styleClass, extra = {}) {
    return new St.Label({
        text,
        style_class: styleClass,
        y_align: Clutter.ActorAlign.CENTER,
        ...extra,
    });
}

/** Полоса прогресса: трек + заливка, ширина заливки считается от ширины трека. */
class Bar {
    constructor(fillClass = 'pearl-fill') {
        this.frac = 0;
        this.fill = new St.Widget({ style_class: fillClass });
        this.track = new St.BoxLayout({
            style_class: 'pearl-track',
            x_expand: true,
            orientation: Clutter.Orientation.HORIZONTAL,
        });
        this.track.add_child(this.fill);
        this.track.connect('notify::width', () => this._applyWidth());
        this._applyWidth();
    }

    set(fraction, fillClass) {
        this.frac = clamp01(fraction);
        if (fillClass && fillClass !== this.fill.style_class)
            this.fill.style_class = fillClass;
        this._applyWidth();
    }

    _applyWidth() {
        const width = this.track.width;
        if (!width || width <= 0)
            return;
        this.fill.set_width(Math.round(width * this.frac));
    }
}

/** Цвет полосы по температуре (пороги те же, что в приложении). */
function tempClass(celsius, limit = DEFAULT_TEMP_LIMIT) {
    if (!Number.isFinite(celsius))
        return 'pearl-fill';
    if (celsius >= Math.min(limit, 85))
        return 'pearl-fill pearl-fill-red';
    if (celsius >= 75)
        return 'pearl-fill pearl-fill-yellow';
    return 'pearl-fill pearl-fill-green';
}

const PearlIndicator = class extends PanelMenu.Button {
    constructor(extensionPath) {
        super(0.5, 'Pearl Miner', false);

        this._path = extensionPath;
        this._proxy = null;
        this._signalIds = [];
        this._ownerSignalId = 0;
        this._status = null;
        this._updatedAt = 0;
        this._history = [];
        this._sparkMode = 'hashrate';
        this._serviceMissing = false;

        // Иконка лежит внутри расширения, а не в icon-theme: иначе без
        // отдельной установки в панели появляется пустое место.
        this._iconFile = GLib.build_filenamev([this._path, 'icons', 'pearl-symbolic.svg']);
        this._gicon = GLib.file_test(this._iconFile, GLib.FileTest.EXISTS)
            ? Gio.icon_new_for_string(this._iconFile)
            : null;

        this._buildPanel();
        this._buildMenu();

        this._ageTimer = GLib.timeout_add_seconds(GLib.PRIORITY_DEFAULT, 1, () => {
            this._updateAge();
            return GLib.SOURCE_CONTINUE;
        });

        this.menu.connect('open-state-changed', (menu, open) => {
            if (open) {
                this._requestRefresh();
                this._syncServiceState();
            }
        });

        this._connectProxy();
    }

    // ------------------------------------------------------------ панель

    _buildPanel() {
        this._panelBox = new St.BoxLayout({
            style_class: 'panel-status-indicators-box',
            y_align: Clutter.ActorAlign.CENTER,
        });
        this._icon = new St.Icon({
            gicon: this._gicon,
            icon_name: this._gicon ? null : 'utilities-system-monitor-symbolic',
            style_class: 'system-status-icon pearl-panel-icon',
        });
        this._panelLabel = makeLabel('', 'pearl-panel-label');
        this._panelLabel.visible = false;
        this._panelBox.add_child(this._icon);
        this._panelBox.add_child(this._panelLabel);
        this.add_child(this._panelBox);
    }

    // ---------------------------------------------------------- поповер

    _menuItem(child, { reactive = false, activate = null } = {}) {
        const item = new PopupMenu.PopupBaseMenuItem({ reactive, can_focus: reactive });
        item.add_style_class_name('pearl-item');
        if (child)
            item.add_child(child);
        if (activate)
            item.connect('activate', activate);
        return item;
    }

    _card(horizontal = false) {
        return new St.BoxLayout({
            style_class: 'pearl-card',
            x_expand: true,
            orientation: horizontal
                ? Clutter.Orientation.HORIZONTAL
                : Clutter.Orientation.VERTICAL,
        });
    }

    _buildMenu() {
        this.menu.box.add_style_class_name('pearl-menu');

        // --- шапка -----------------------------------------------------
        const header = new St.BoxLayout({ x_expand: true, y_align: Clutter.ActorAlign.CENTER });
        this._titleLabel = makeLabel('Pearl Miner', 'pearl-header-title');
        this._titleLabel.x_expand = true;
        this._pill = makeLabel('Проверяю…', 'pearl-pill pearl-pill-neutral');
        header.add_child(this._titleLabel);
        header.add_child(this._pill);
        this.menu.addMenuItem(this._menuItem(header));

        const subtitle = new St.BoxLayout({ x_expand: true });
        this._subtitleLabel = makeLabel('', 'pearl-caption');
        this._subtitleLabel.x_expand = true;
        this._uptimeLabel = makeLabel('', 'pearl-muted pearl-mono');
        subtitle.add_child(this._subtitleLabel);
        subtitle.add_child(this._uptimeLabel);
        this.menu.addMenuItem(this._menuItem(subtitle));

        // --- большая кнопка -------------------------------------------
        this._actionButton = new St.Button({
            style_class: 'pearl-action',
            x_expand: true,
            can_focus: true,
        });
        const actionBox = new St.BoxLayout({
            orientation: Clutter.Orientation.HORIZONTAL,
            x_expand: true,
        });
        this._actionIcon = new St.Icon({
            icon_name: 'media-playback-start-symbolic',
            style_class: 'popup-menu-icon',
        });
        this._actionLabel = makeLabel('Запустить майнинг', 'pearl-action-label', { x_expand: true });
        actionBox.add_child(this._actionIcon);
        actionBox.add_child(this._actionLabel);
        this._actionButton.set_child(actionBox);
        this._actionButton.connect('clicked', () => this._toggle());
        this.menu.addMenuItem(this._menuItem(this._actionButton));

        // --- карточка «Майнинг» ----------------------------------------
        const mining = this._card();
        const hrRow = new St.BoxLayout({ x_expand: true });
        this._hashrate = makeLabel('—', 'pearl-value');
        this._hashrate.x_expand = true;
        this._hashrateSrc = makeLabel('', 'pearl-caption');
        hrRow.add_child(this._hashrate);
        hrRow.add_child(this._hashrateSrc);
        mining.add_child(hrRow);

        this._hashrateSub = makeLabel('10s · 60s · 15m', 'pearl-caption pearl-mono');
        mining.add_child(this._hashrateSub);

        const loadBox = new St.BoxLayout({ x_expand: true });
        this._loadCaption = makeLabel('от ожидаемых —', 'pearl-label');
        this._loadCaption.x_expand = true;
        loadBox.add_child(this._loadCaption);
        mining.add_child(loadBox);
        this._loadBar = new Bar();
        this._loadBarWrap = new St.BoxLayout({ x_expand: true });
        this._loadBarWrap.add_child(this._loadBar.track);
        mining.add_child(this._loadBarWrap);

        const tempRow = new St.BoxLayout({ x_expand: true });
        this._tempCaption = makeLabel('Температура', 'pearl-label');
        this._tempCaption.x_expand = true;
        this._tempValue = makeLabel('—', 'pearl-label pearl-mono');
        tempRow.add_child(this._tempCaption);
        tempRow.add_child(this._tempValue);
        mining.add_child(tempRow);
        this._tempBar = new Bar();
        const tempWrap = new St.BoxLayout({ x_expand: true });
        tempWrap.add_child(this._tempBar.track);
        mining.add_child(tempWrap);

        const powerRow = new St.BoxLayout({ x_expand: true });
        this._powerCaption = makeLabel('Мощность', 'pearl-label');
        this._powerCaption.x_expand = true;
        this._powerValue = makeLabel('—', 'pearl-label pearl-mono');
        powerRow.add_child(this._powerCaption);
        powerRow.add_child(this._powerValue);
        mining.add_child(powerRow);
        this._powerBar = new Bar();
        const powerWrap = new St.BoxLayout({ x_expand: true });
        powerWrap.add_child(this._powerBar.track);
        mining.add_child(powerWrap);

        this._gpuLine = makeLabel('', 'pearl-caption pearl-mono');
        mining.add_child(this._gpuLine);

        this.menu.addMenuItem(this._menuItem(mining));

        // --- спарклайн -------------------------------------------------
        const spark = this._card();
        const sparkHead = new St.BoxLayout({ x_expand: true });
        this._sparkTitle = makeLabel('Хешрейт', 'pearl-label');
        this._sparkTitle.x_expand = true;
        this._sparkHint = makeLabel('нажми, чтобы переключить', 'pearl-caption');
        sparkHead.add_child(this._sparkTitle);
        sparkHead.add_child(this._sparkHint);
        spark.add_child(sparkHead);

        this._sparkArea = new St.BoxLayout({
            style_class: 'pearl-spark',
            x_expand: true,
            orientation: Clutter.Orientation.HORIZONTAL,
        });
        this._sparkBars = [];
        for (let i = 0; i < SPARK_POINTS; i++) {
            const bar = new St.Widget({
                style_class: 'pearl-spark-bar',
                width: 4,
                height: 2,
                y_align: Clutter.ActorAlign.END,
                x_align: Clutter.ActorAlign.CENTER,
                x_expand: true,
            });
            this._sparkBars.push(bar);
            this._sparkArea.add_child(bar);
        }
        spark.add_child(this._sparkArea);
        this.menu.addMenuItem(this._menuItem(spark, {
            reactive: true,
            activate: () => {
                this._sparkMode = this._sparkMode === 'hashrate' ? 'temp' : 'hashrate';
                this._renderSpark();
            },
        }));

        // --- карточка «Пул» --------------------------------------------
        const pool = this._card();
        const poolHead = new St.BoxLayout({ x_expand: true });
        this._sharesLabel = makeLabel('Шары', 'pearl-label');
        this._sharesLabel.x_expand = true;
        this._sharesValue = makeLabel('—', 'pearl-label pearl-mono');
        poolHead.add_child(this._sharesLabel);
        poolHead.add_child(this._sharesValue);
        pool.add_child(poolHead);

        const balanceHead = new St.BoxLayout({ x_expand: true });
        this._balanceLabel = makeLabel('Баланс пула', 'pearl-label');
        this._balanceLabel.x_expand = true;
        this._balanceValue = makeLabel('—', 'pearl-label pearl-mono');
        balanceHead.add_child(this._balanceLabel);
        balanceHead.add_child(this._balanceValue);
        pool.add_child(balanceHead);

        this._payoutBar = new Bar('pearl-fill pearl-fill-green');
        const payoutWrap = new St.BoxLayout({ x_expand: true });
        payoutWrap.add_child(this._payoutBar.track);
        pool.add_child(payoutWrap);

        this._poolLine = makeLabel('', 'pearl-caption pearl-mono');
        pool.add_child(this._poolLine);
        this.menu.addMenuItem(this._menuItem(pool));

        // --- футер -----------------------------------------------------
        const footer = new St.BoxLayout({
            x_expand: true,
            style_class: 'pearl-footer',
            y_align: Clutter.ActorAlign.CENTER,
        });
        this._ageLabel = makeLabel('✓ Обновлено только что', 'pearl-muted');
        this._ageLabel.x_expand = true;

        this._refreshButton = new St.Button({
            style_class: 'pearl-action-flat',
            can_focus: true,
            child: new St.Icon({
                icon_name: 'view-refresh-symbolic',
                style_class: 'popup-menu-icon',
            }),
        });
        this._refreshButton.connect('clicked', () => this._requestRefresh());

        this._settingsButton = new St.Button({
            style_class: 'pearl-action-flat',
            can_focus: true,
            child: new St.Icon({
                icon_name: 'preferences-system-symbolic',
                style_class: 'popup-menu-icon',
            }),
        });
        this._settingsButton.connect('clicked', () => this._openApp());

        footer.add_child(this._ageLabel);
        footer.add_child(this._refreshButton);
        footer.add_child(this._settingsButton);
        this.menu.addMenuItem(this._menuItem(footer));

        this._setStateIdle();
    }

    // ------------------------------------------------------- D-Bus связь

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

                this._signalIds.push(proxy.connectSignal('StatusChanged',
                    (p, sender, params) => this._onStatus(params[0])));
                this._signalIds.push(proxy.connectSignal('Notification',
                    (p, sender, params) => Main.notify(params[0], params[1])));
                this._ownerSignalId = proxy.connect('notify::g-name-owner',
                    () => this._syncServiceState());

                this._syncServiceState();
                this._requestRefresh();
            });
    }

    _syncServiceState() {
        const owner = this._proxy ? this._proxy.get_name_owner() : null;
        if (owner) {
            if (this._serviceMissing) {
                this._serviceMissing = false;
                this._titleLabel.text = 'Pearl Miner';
                this._icon.remove_style_class_name('pearl-panel-offline');
            }
            return;
        }
        this._serviceMissing = true;
        // Поднимаем сервис через D-Bus activation (файл org.pearl.Miner.Daemon.service).
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
            (conn, res) => {
                try {
                    conn.call_finish(res);
                } catch (e) {
                    this._setServiceMissing(e.message);
                }
            });
    }

    _setServiceMissing(detail) {
        this._serviceMissing = true;
        this._titleLabel.text = 'Pearl Miner: сервис не запущен';
        this._subtitleLabel.text = 'systemctl --user start pearl-minerd';
        this._pill.text = 'нет связи';
        this._pill.style_class = 'pearl-pill pearl-pill-error';
        this._uptimeLabel.text = '';
        this._panelLabel.visible = false;
        this._icon.add_style_class_name('pearl-panel-offline');
        this._actionButton.reactive = false;
        if (detail)
            logError(new Error(detail), 'pearl-miner: D-Bus');
    }

    _call(method, args, callback) {
        if (!this._proxy || !this._proxy.get_name_owner()) {
            if (callback)
                callback(new Error('сервис не запущен'));
            return;
        }
        this._proxy.call(method, args ?? null, Gio.DBusCallFlags.NONE, 15000, null,
            (proxy, res) => {
                let value = null;
                let error = null;
                try {
                    const variant = proxy.call_finish(res);
                    value = variant ? variant.deep_unpack() : null;
                } catch (e) {
                    error = e;
                }
                if (callback)
                    callback(error, value);
            });
    }

    _requestRefresh() {
        this._call('Refresh', null, (error, value) => {
            if (error) {
                if (this._proxy && !this._proxy.get_name_owner())
                    this._setServiceMissing(error.message);
                return;
            }
            if (typeof value === 'string')
                this._onStatus(value);
        });
    }

    _toggle() {
        this._actionButton.reactive = false;
        this._actionLabel.text = 'Переключаю…';
        this._call('Toggle', null, (error) => {
            this._actionButton.reactive = true;
            if (error) {
                Main.notify('Pearl Miner', `Не удалось переключить: ${error.message}`);
                this._setStateIdle();
                return;
            }
            this._requestRefresh();
        });
    }

    _openApp() {
        this.menu.close();
        this._call('OpenApp', null, (error) => {
            if (!error)
                return;
            try {
                const info = Gio.DesktopAppInfo.new('org.pearl.Miner.desktop');
                if (info)
                    info.launch([], null);
                else
                    Gio.Subprocess.new(['pearl-miner', 'gui'], Gio.SubprocessFlags.NONE);
            } catch (e) {
                Main.notify('Pearl Miner', `Не удалось открыть приложение: ${e.message}`);
            }
        });
    }

    // ------------------------------------------------------- обновление

    _onStatus(json) {
        let status;
        try {
            status = JSON.parse(json);
        } catch (e) {
            logError(e, 'pearl-miner: не разобрал статус');
            return;
        }
        this._status = status;
        this._updatedAt = GLib.get_monotonic_time() / 1e6;
        this._apply(status);
    }

    _apply(status) {
        const running = status?.state?.running ?? false;
        const gpu = status.gpu ?? null;
        const miner = status.miner ?? {};
        const pool = status.pool ?? null;
        const expected = status.expected_hashrate || 0;
        const primary = miner.hashrate_10s ?? miner.hashrate_60s ?? null;

        // Панель: иконка не прыгает, подпись только при майнинге.
        if (running && primary) {
            const temp = gpu?.temp_c;
            this._panelLabel.text = `${fmtHashrateShort(primary)}${temp ? ` · ${temp}°` : ''}`;
            this._panelLabel.visible = true;
            this._panelLabel.remove_style_class_name('pearl-panel-label-dim');
            this._icon.add_style_class_name('pearl-panel-running');
        } else if (running) {
            this._panelLabel.text = 'прогрев';
            this._panelLabel.visible = true;
            this._panelLabel.add_style_class_name('pearl-panel-label-dim');
            this._icon.add_style_class_name('pearl-panel-running');
        } else {
            this._panelLabel.visible = false;
            this._icon.remove_style_class_name('pearl-panel-running');
        }

        // Шапка
        if (running) {
            if (status.state.external) {
                this._pill.text = 'запущен вручную';
                this._pill.style_class = 'pearl-pill pearl-pill-warn';
            } else if (status.state.last_error) {
                this._pill.text = 'работает с ошибкой';
                this._pill.style_class = 'pearl-pill pearl-pill-warn';
            } else {
                this._pill.text = 'майнится';
                this._pill.style_class = 'pearl-pill';
            }
        } else {
            this._pill.text = 'остановлен';
            this._pill.style_class = 'pearl-pill pearl-pill-neutral';
        }
        this._uptimeLabel.text = running ? fmtDuration(status.state?.uptime_s) : '';
        this._subtitleLabel.text = this._subtitleText(status);

        // Кнопка
        if (running) {
            this._actionLabel.text = 'Остановить майнинг';
            this._actionIcon.icon_name = 'media-playback-stop-symbolic';
            this._actionButton.style_class = 'pearl-action pearl-action-stop';
        } else {
            this._actionLabel.text = 'Запустить майнинг';
            this._actionIcon.icon_name = 'media-playback-start-symbolic';
            this._actionButton.style_class = 'pearl-action';
        }
        this._actionButton.reactive = !this._serviceMissing;

        // Хешрейт
        this._hashrate.text = primary ? fmtHashrate(primary) : (running ? 'прогрев…' : '—');
        this._hashrateSrc.text = miner.source ? `${miner.source}` : '';
        const h10 = miner.hashrate_10s ? fmtHashrate(miner.hashrate_10s) : '—';
        const h60 = miner.hashrate_60s ? fmtHashrate(miner.hashrate_60s) : '—';
        const h15 = miner.hashrate_15m ? fmtHashrate(miner.hashrate_15m) : '—';
        this._hashrateSub.text = `10s ${h10} · 60s ${h60} · 15m ${h15}`;

        const load = expected > 0 && primary ? clamp01(primary / expected) : 0;
        this._loadCaption.text = expected > 0
            ? `от ожидаемых ${fmtHashrate(expected)}`
            : 'от ожидаемых —';
        this._loadBar.set(load);

        // Температура: порог из настроек, а не константа
        const temp = gpu?.temp_c;
        const limit = status.temp_limit ?? DEFAULT_TEMP_LIMIT;
        this._tempValue.text = temp != null ? `${temp} / ${limit} °C` : '—';
        this._tempBar.set(temp != null ? temp / limit : 0, tempClass(temp ?? 0, limit));

        // Мощность
        const power = gpu?.power_w;
        const plimit = gpu?.power_limit_w;
        this._powerValue.text = power != null
            ? `${Math.round(power)}${plimit ? ` / ${Math.round(plimit)} W` : ' W'}`
            : '—';
        this._powerBar.set(power != null && plimit ? clamp01(power / plimit) : 0);

        // Вентилятор и частоты
        const parts = [];
        if (gpu?.fan_pct != null)
            parts.push(`вентилятор ${gpu.fan_pct}%`);
        if (gpu?.core_mhz != null)
            parts.push(`${gpu.core_mhz} MHz`);
        if (gpu?.mem_mhz != null)
            parts.push(`память ${gpu.mem_mhz} MHz`);
        if (!gpu && status.gpu_error)
            parts.push('GPU недоступен');
        this._gpuLine.text = parts.join(' · ');

        // Шары и пул
        const accepted = miner.accepted;
        const rejected = miner.rejected;
        if (accepted != null) {
            const total = accepted + (rejected ?? 0);
            const pct = total > 0 ? ((accepted / total) * 100).toFixed(1) : '—';
            this._sharesValue.text = `${accepted} · ${rejected ?? 0} откл (${pct}%)`;
        } else {
            this._sharesValue.text = '—';
        }

        if (pool && pool.balance != null) {
            this._balanceValue.text = `${pool.balance.toFixed(3)} PRL`;
            const target = pool.min_payout_prl > 0 ? pool.min_payout_prl : 1.0;
            this._payoutBar.set(clamp01(pool.balance / target), 'pearl-fill pearl-fill-green');
            const bits = [];
            if (pool.fee_pct != null)
                bits.push(`комиссия пула ${pool.fee_pct}%`);
            if (pool.workers_online != null)
                bits.push(`воркеров ${pool.workers_online}`);
            if (pool.paid != null)
                bits.push(`выплачено ${pool.paid.toFixed(3)} PRL`);
            this._poolLine.text = bits.join(' · ');
        } else {
            this._balanceValue.text = running ? 'нет данных пула' : '—';
            this._payoutBar.set(0, 'pearl-fill pearl-fill-green');
            this._poolLine.text = '';
        }

        // История для спарклайна
        this._history.push({
            hashrate: primary ?? 0,
            temp: temp ?? null,
        });
        while (this._history.length > SPARK_POINTS)
            this._history.shift();
        this._renderSpark();
        this._updateAge();
    }

    _subtitleText(status) {
        if (status?.state?.last_error)
            return status.state.last_error;
        if (status?.warnings?.length)
            return status.warnings[0];
        if (status?.state?.restarts)
            return `перезапусков: ${status.state.restarts}`;
        const fee = status?.total_fee_pct;
        return Number.isFinite(fee)
            ? `Pearl (PRL) · комиссия ${fee.toFixed(2)}%`
            : 'Pearl (PRL)';
    }

    _renderSpark() {
        const values = this._history.map(entry =>
            this._sparkMode === 'temp' ? entry.temp : entry.hashrate);
        const numeric = values.filter(v => Number.isFinite(v) && v > 0);
        const max = numeric.length ? Math.max(...numeric) : 1;
        const min = this._sparkMode === 'temp' && numeric.length ? Math.min(...numeric) : 0;
        const maxHeight = 30;

        this._sparkTitle.text = this._sparkMode === 'temp' ? 'Температура' : 'Хешрейт';
        const barClass = this._sparkMode === 'temp' ? 'pearl-spark-bar pearl-spark-bar-temp' : 'pearl-spark-bar';

        for (let i = 0; i < this._sparkBars.length; i++) {
            const bar = this._sparkBars[i];
            const value = values[values.length - this._sparkBars.length + i];
            if (!Number.isFinite(value) || value <= 0) {
                bar.set_height(2);
                continue;
            }
            const span = Math.max(max - min, 1e-9);
            const fraction = clamp01((value - min) / span);
            bar.set_height(Math.max(2, Math.round(2 + fraction * maxHeight)));
            if (bar.style_class !== barClass)
                bar.style_class = barClass;
        }
    }

    _updateAge() {
        if (!this._updatedAt) {
            this._ageLabel.text = '✓ Ожидаю данные';
            return;
        }
        const age = Math.max(0, Math.round(GLib.get_monotonic_time() / 1e6 - this._updatedAt));
        const suffix = plural(age, 'секунду', 'секунды', 'секунд');
        this._ageLabel.text = age < 3
            ? '✓ Обновлено только что'
            : `✓ Обновлено ${age} ${suffix} назад`;
        if (this._status?.state?.running)
            this._uptimeLabel.text = fmtDuration((this._status.state.uptime_s ?? 0) + age);
    }

    _setStateIdle() {
        this._actionLabel.text = 'Запустить майнинг';
        this._actionIcon.icon_name = 'media-playback-start-symbolic';
        this._actionButton.style_class = 'pearl-action';
        this._actionButton.reactive = true;
    }

    destroy() {
        if (this._ageTimer) {
            GLib.source_remove(this._ageTimer);
            this._ageTimer = 0;
        }
        if (this._proxy) {
            for (const id of this._signalIds)
                this._proxy.disconnectSignal(id);
            this._signalIds = [];
            if (this._ownerSignalId) {
                this._proxy.disconnect(this._ownerSignalId);
                this._ownerSignalId = 0;
            }
            this._proxy = null;
        }
        super.destroy();
    }
};

export default class PearlMinerExtension extends Extension {
    enable() {
        this._indicator = new PearlIndicator(this.path);
        Main.panel.addToStatusArea(this.uuid, this._indicator, 0, 'right');
    }

    disable() {
        this._indicator?.destroy();
        this._indicator = null;
    }
}
