import Clutter from 'gi://Clutter';
import Cogl from 'gi://Cogl';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import St from 'gi://St';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as PanelMenu from 'resource:///org/gnome/shell/ui/panelMenu.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';

const BUS = 'io.github.bepcyc.MarginBridge';
const PATH = '/io/github/bepcyc/MarginBridge';
const IFACE = 'io.github.bepcyc.MarginBridge';
const XML = `<node><interface name="${IFACE}">
  <method name="Register"><arg type="u" direction="in"/><arg type="ay" direction="in"/><arg type="s" direction="in"/><arg type="t" direction="in"/><arg type="b" direction="out"/></method>
  <method name="Update"><arg type="u" direction="in"/><arg type="ay" direction="in"/><arg type="s" direction="in"/><arg type="t" direction="in"/><arg type="b" direction="out"/></method>
  <method name="Reposition"><arg type="b" direction="out"/></method>
  <signal name="Action"><arg type="s"/></signal>
</interface></node>`;

function validIcon(size, pixels, tooltip, revision) {
    return size > 0 && size <= 128 && pixels.length === size * size * 4 &&
        tooltip.length <= 512 && Number.isSafeInteger(revision) && revision >= 0;
}

export default class MarginBridge extends Extension {
    enable() {
        this.active = true;
        this.connection = Gio.DBus.session;
        this.owner = null;
        this.pending = null;
        this.pid = 0;
        this.revision = 0;
        this.button = null;
        this.window = null;
        this.windowSignals = [];
        this.mapSignal = global.window_manager.connect('map', (_manager, actor) => {
            if (actor.meta_window === this.window) this.positionWindow();
        });
        this.windowCreatedSignal = global.display.connect('window-created', (_display, window) => {
            if (this.pid && window.get_pid() === this.pid) this.trackWindow();
        });
        this.windowVisibilitySignal = global.display.connect('window-visibility-updated', () => {
            if (!this.pid || this.window) return;
            const window = this.trackWindow();
            if (window && !window.is_hidden() && window.showing_on_its_workspace()) this.positionWindow();
        });
        this.object = Gio.DBusExportedObject.wrapJSObject(XML, this);
        this.object.export(this.connection, PATH);
        this.nameId = Gio.bus_own_name_on_connection(this.connection, BUS, Gio.BusNameOwnerFlags.NONE, null, null);
        this.ownerSignal = this.connection.signal_subscribe('org.freedesktop.DBus', 'org.freedesktop.DBus', 'NameOwnerChanged', '/org/freedesktop/DBus', null, Gio.DBusSignalFlags.NONE,
            (_connection, _sender, _path, _interface, _signal, parameters) => {
                const [name, oldOwner, newOwner] = parameters.deep_unpack();
                if (name === oldOwner && newOwner === '' && (name === this.owner || name === this.pending)) this.reset();
            });
    }

    RegisterAsync(parameters, invocation) {
        const [size, pixels, tooltip, revision] = parameters;
        const sender = invocation.get_sender();
        if (!sender.startsWith(':') || this.owner || this.pending || !validIcon(size, pixels, tooltip, revision)) {
            invocation.return_value(new GLib.Variant('(b)', [false]));
            return;
        }
        this.pending = sender;
        this.connection.call('org.freedesktop.DBus', '/org/freedesktop/DBus', 'org.freedesktop.DBus', 'GetConnectionUnixProcessID',
            new GLib.Variant('(s)', [sender]), new GLib.VariantType('(u)'), Gio.DBusCallFlags.NONE, 2000, null,
            (connection, result) => {
                try {
                    const [pid] = connection.call_finish(result).deep_unpack();
                    if (!this.active || !pid || this.pending !== sender) throw new Error('Disconnected sender');
                    this.pending = null;
                    this.owner = sender;
                    this.pid = pid;
                    this.revision = revision;
                    this.createIndicator();
                    this.setIcon(size, pixels, tooltip);
                    this.trackWindow();
                    invocation.return_value(new GLib.Variant('(b)', [true]));
                } catch (_error) {
                    logError(_error);
                    if (this.pending === sender || this.owner === sender) this.reset();
                    invocation.return_value(new GLib.Variant('(b)', [false]));
                }
            });
    }

    UpdateAsync(parameters, invocation) {
        const [size, pixels, tooltip, revision] = parameters;
        if (invocation.get_sender() !== this.owner || !this.button || !validIcon(size, pixels, tooltip, revision)) {
            invocation.return_value(new GLib.Variant('(b)', [false]));
            return;
        }
        if (revision <= this.revision) {
            invocation.return_value(new GLib.Variant('(b)', [true]));
            return;
        }
        try {
            this.setIcon(size, pixels, tooltip);
            this.revision = revision;
            invocation.return_value(new GLib.Variant('(b)', [true]));
        } catch (_error) {
            invocation.return_value(new GLib.Variant('(b)', [false]));
        }
    }

    RepositionAsync(_parameters, invocation) {
        invocation.return_value(new GLib.Variant('(b)', [invocation.get_sender() === this.owner && this.positionWindow()]));
    }

    setIcon(size, pixels, tooltip) {
        const image = new Clutter.Image();
        image.set_data(pixels, Cogl.PixelFormat.RGBA_8888, size, size, size * 4);
        this.icon.content = image;
        this.button.set_accessible_name(tooltip);
        this.actionButton.set_accessible_name(tooltip);
    }

    sendAction(action) {
        if (this.owner) this.connection.emit_signal(this.owner, PATH, IFACE, 'Action', new GLib.Variant('(s)', [action]));
    }

    createIndicator() {
        this.button = new PanelMenu.Button(0.0, 'Margin', false);
        this.icon = new St.Widget({width: 16, height: 16, style_class: 'system-status-icon'});
        this.actionButton = new St.Button({child: this.icon, reactive: true, can_focus: true});
        this.button.add_child(this.actionButton);
        for (const [label, action] of [['Open Margin', 'open'], ['Refresh usage', 'refresh'], ['Settings…', 'settings'], ['Quit Margin', 'quit']]) {
            const item = new PopupMenu.PopupMenuItem(label);
            item.connect('activate', () => this.sendAction(action));
            this.button.menu.addMenuItem(item);
        }
        this.actionButton.connect('button-press-event', (_actor, event) => {
            if (event.get_button() === 1) this.sendAction('toggle');
            else if (event.get_button() === 3) this.button.menu.toggle();
            else return Clutter.EVENT_PROPAGATE;
            return Clutter.EVENT_STOP;
        });
        const handleKey = (_actor, event) => {
            const key = event.get_key_symbol();
            if ([Clutter.KEY_Return, Clutter.KEY_KP_Enter, Clutter.KEY_space].includes(key)) this.sendAction('toggle');
            else if (key === Clutter.KEY_Menu || (key === Clutter.KEY_F10 && (event.get_state() & Clutter.ModifierType.SHIFT_MASK))) this.button.menu.toggle();
            else return Clutter.EVENT_PROPAGATE;
            return Clutter.EVENT_STOP;
        };
        this.actionButton.connect('key-press-event', handleKey);
        this.button.connect('captured-event', (actor, event) =>
            event.type() === Clutter.EventType.KEY_PRESS && global.stage.get_key_focus() === actor
                ? handleKey(actor, event) : Clutter.EVENT_PROPAGATE);
        this.actionButton.connect('clicked', () => this.sendAction('toggle'));
        this.button.connect('notify::allocation', () => this.positionWindow());
        Main.panel.addToStatusArea(this.uuid, this.button);
    }

    clearWindow() {
        if (this.window) for (const signal of this.windowSignals) this.window.disconnect(signal);
        this.window = null;
        this.windowSignals = [];
    }

    trackWindow() {
        if (!this.pid) return null;
        const windows = global.display.list_all_windows().filter(window => window.get_pid() === this.pid && window.get_wm_class() === 'Margin' && !window.get_transient_for());
        if (windows.length !== 1) {
            this.clearWindow();
            return null;
        }
        const window = windows[0];
        if (this.window !== window) {
            this.clearWindow();
            this.window = window;
            this.windowSignals = [
                window.connect('shown', () => this.positionWindow()),
                window.connect('size-changed', () => this.positionWindow()),
                window.connect('unmanaged', () => this.clearWindow()),
            ];
        }
        return window;
    }

    positionWindow() {
        if (!this.button || !this.window) return false;
        const window = this.window;
        const monitor = Main.layoutManager.findMonitorForActor(this.button);
        if (!monitor) return false;
        const work = window.get_work_area_for_monitor(monitor.index);
        const frame = window.get_frame_rect();
        const [ax, ay] = this.button.get_transformed_position();
        const [aw, ah] = this.button.get_transformed_size();
        const x = Math.round(Math.max(work.x, Math.min(work.x + work.width - frame.width, ax + aw / 2 - frame.width / 2)));
        const y = Math.round(Math.max(work.y, Math.min(work.y + work.height - frame.height, ay + ah)));
        if (frame.x !== x || frame.y !== y) window.move_frame(true, x, y);
        return true;
    }

    reset() {
        this.clearWindow();
        this.button?.destroy();
        this.button = null;
        this.icon = null;
        this.actionButton = null;
        this.owner = null;
        this.pending = null;
        this.pid = 0;
        this.revision = 0;
    }

    disable() {
        this.active = false;
        this.connection.signal_unsubscribe(this.ownerSignal);
        global.window_manager.disconnect(this.mapSignal);
        global.display.disconnect(this.windowCreatedSignal);
        global.display.disconnect(this.windowVisibilitySignal);
        this.reset();
        Gio.bus_unown_name(this.nameId);
        this.object.unexport();
    }
}
