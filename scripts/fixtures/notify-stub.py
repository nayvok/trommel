#!/usr/bin/python3
"""Minimal `org.freedesktop.Notifications` service for the e2e private bus.

The e2e harness runs the daemon on an isolated session bus, so the daemon has
no real notification service to talk to. This stub owns the well-known name,
records every `Notify` summary/body in a log file and touches a ready file once
the name is acquired. It is only a test fixture; it never shows a notification.

    notify-stub.py LOG READY
"""

import sys

from gi.repository import Gio, GLib

INTROSPECTION = """
<node>
  <interface name="org.freedesktop.Notifications">
    <method name="Notify">
      <arg type="s" direction="in"/>
      <arg type="u" direction="in"/>
      <arg type="s" direction="in"/>
      <arg type="s" direction="in"/>
      <arg type="s" direction="in"/>
      <arg type="as" direction="in"/>
      <arg type="a{sv}" direction="in"/>
      <arg type="i" direction="in"/>
      <arg type="u" direction="out"/>
    </method>
    <method name="GetCapabilities">
      <arg type="as" direction="out"/>
    </method>
    <method name="GetServerInformation">
      <arg type="s" direction="out"/>
      <arg type="s" direction="out"/>
      <arg type="s" direction="out"/>
      <arg type="s" direction="out"/>
    </method>
  </interface>
</node>
"""


def main() -> int:
    if len(sys.argv) != 3:
        print("usage: notify-stub.py LOG READY", file=sys.stderr)
        return 2
    log_path, ready_path = sys.argv[1], sys.argv[2]

    connection = Gio.bus_get_sync(Gio.BusType.SESSION, None)
    node = Gio.DBusNodeInfo.new_for_xml(INTROSPECTION).interfaces[0]

    def on_call(_conn, _sender, _path, _iface, method, params, invocation):
        if method == "Notify":
            args = params.unpack()
            with open(log_path, "a", encoding="utf-8") as log:
                log.write(f"{args[3]}\t{args[4]}\n")
            invocation.return_value(GLib.Variant("(u)", (1,)))
        elif method == "GetCapabilities":
            invocation.return_value(GLib.Variant("(as)", (["body"],)))
        else:
            invocation.return_value(
                GLib.Variant("(ssss)", ("trommel-e2e", "trommel", "1", "1.2"))
            )

    connection.register_object(
        "/org/freedesktop/Notifications", node, on_call, None, None
    )

    def on_acquired(_conn, _name):
        with open(ready_path, "w", encoding="utf-8") as ready:
            ready.write("ready\n")

    Gio.bus_own_name_on_connection(
        connection,
        "org.freedesktop.Notifications",
        Gio.BusNameOwnerFlags.NONE,
        on_acquired,
        None,
    )
    GLib.MainLoop().run()
    return 0


if __name__ == "__main__":
    sys.exit(main())
