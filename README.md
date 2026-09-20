# nibari

An ultra-minimalist bar for niri wm, inspired by awesomewm. On the left are workspaces with labels `一 二 三 四 五 六 七 八 九 零` (I can't be bothered to move this to the config)

## Build and run

```sh
cargo build --release
./target/release/nibari
```

For autostart, specify the compiled binary in your niri config.

Run `cargo test --locked` for unit and protocol integration tests. The application
menu integration tests require `dbus-daemon`; they start isolated buses and do not
change the desktop session bus.

## Configuration

By default, it reads from:

```text
$XDG_CONFIG_HOME/nibari/config.toml
```

If `XDG_CONFIG_HOME` is not set, it uses
`~/.config/nibari/config.toml`. On first startup, a missing default config file
and its directory are created from the bundled example.

At startup, missing settings are automatically added with their default values
and comments. Existing values, comments, and formatting are preserved. A complete
config is left untouched; updates use an atomic replacement and preserve file
permissions and symbolic links. Invalid configs are never updated. If writing is
not possible, nibari logs a warning and still uses the valid config with built-in
defaults for missing settings. This runs once at startup and adds no file watcher
or periodic work. A missing file explicitly passed with `--config` remains an error.

To get a full example:

```sh
nibari --print-default-config
```

A minimal config for a specific clock format:

```toml
clock_format = "%a %b %d, %H:%M"
```

You can specify a different file:

```sh
nibari --config /path/to/config.toml
```

`clock_format` uses the `strftime` syntax from `chrono`. For example,
`"%d.%m.%Y %H:%M:%S"` will show the date and time with seconds. Other available
parameters are listed in [`config.example.toml`](config.example.toml).

To make the background transparent and hide the list of windows on the active workspace:

```toml
background = "#28282800"
show_tasks = false
```

Colors accept `#RRGGBB` (opaque) or `#RRGGBBAA`, where the last two hexadecimal
digits control opacity: `00` is fully transparent, `80` is approximately 50%,
and `FF` is fully opaque. For example, `background = "#28282880"` gives a
semi-transparent background while keeping text and icons at their configured opacity.
Workspace highlights and task buttons have their own background colors; set their
`*_background` colors to an alpha of `00` too if you want those transparent.
Restart nibari after editing the config.

The tasklist shows application windows on the active workspace of each monitor.
Enable it with `show_tasks = true` and `clock_position = "center"`; its appearance
is configured via `task_*` parameters. Tasks are hidden by default.
`show_tasks = true` with a right-hand clock is a configuration error. Hiding tasks
skips application icon and title preparation.
The icon is searched for first via the application's desktop file, then in the selected icon
theme; if it is missing, a stable colored placeholder is drawn.

The active workspace on each monitor uses a solid dot (`●`) instead of its label.
An underline marks any workspace with open windows, including the active one.
Set both `workspace_focused_background` and `workspace_active_background` to
`"#00000000"` for a dot without a background highlight.

Left-click a workspace label or dot to switch to that workspace on its monitor. Left-click
anywhere on a task's icon or title button to focus that window.

### Workspace applications and media

```toml
clock_position = "center"
show_tasks = true
media_enabled = true
```

The centered layout is `workspaces | workspace applications | date/time | media | tray`.
Applications belong to the active workspace on that monitor. The clock stays at
its actual screen center; the optional weather icon remains beside it, and the
keyboard/network indicators remain in the right status group. Long content is
clipped within its own side, preserving the clock and tray.

Media comes directly from [MPRIS](https://specifications.freedesktop.org/mpris/latest/Player_Interface.html)
on the session D-Bus. It shows the artist/channel, a title limited to 32
characters, elapsed/total time, percentage and playback status, for example:

`PHARAOH - Мой кайф - 00:49 / 02:40 (31%) [Paused]`

Playing players take precedence over paused and stopped players; within the
same state, the latest active player wins. Without a track, the media area is
empty. Streams without a known duration show `--:--` and omit the percentage.
Position advances locally once per second while playing and freezes when paused;
metadata, seeking, player appearance and disappearance are handled through D-Bus
signals. There is no periodic D-Bus polling. The media line is centered in the
space available between the clock and the right status group.

Set `media_enabled = false` to disable the media worker and display. With a
right-aligned clock, media uses the available space after workspaces and before
the status group; `show_tasks` still requires a centered clock.

Application-menu integration has been removed from the main version. The old
`appmenu_enabled` key is ignored so existing configuration files keep loading.

### Date, time, and calendar

`clock_format` controls the entire date/time string using chrono/strftime syntax.
Set `clock_position = "center"` to center it on the full monitor width, independently
of workspaces, windows, keyboard layout, and tray size. The default is `"right"`.
In center mode, tasks use the space between workspaces and the clock. Media uses
the space after the clock (and optional weather icon), before the tray/status group.
Side content is clipped if necessary so it cannot cover the date/time.

Click the date/time to open the month calendar. Use its arrows, the mouse wheel,
or Left/Right (Page Up/Page Down) to change months. Click the month heading or
press Home to return to the current month. Up/Down or Tab selects a header
control; Enter activates it. Escape, right-click, or a click outside closes it.
The week starts on Monday; today's date is highlighted.

Set `calendar_enabled = false` to disable the popup. Its palette is independent
of the bar: `calendar_background`, `calendar_foreground`, `calendar_header`,
`calendar_weekday`, `calendar_weekend`, `calendar_muted` (adjacent months),
`calendar_today_background`, `calendar_today_foreground`, and `calendar_border`.
All accept `#RRGGBB` or `#RRGGBBAA`. Restart nibari after configuration changes.

### Weather

Set `weather_enabled = true` for a weather icon immediately after the date/time.
The date/time itself remains exactly centered with `clock_position = "center"`.
Click the icon for current temperature, feels-like temperature, wind, humidity,
and the next three days in the weather location's time zone. The small arrow
refreshes the data; Escape, right-click, or an outside click closes the card.

```toml
weather_enabled = true
weather_location = ""             # Automatic location from the public IP
weather_units = "metric"          # "metric" (°C, km/h) or "imperial" (°F, mph)
weather_refresh_minutes = 15
```

Automatic location uses [ipwho.is](https://ipwhois.io/documentation) through your
normal connection, so a VPN can make it select the VPN endpoint's city. Set
`weather_location = "Helsinki"` to search for a fixed city instead. For an exact
location, set both coordinates; these take priority over city lookup:

```toml
weather_location = "Helsinki"     # Display name when coordinates are set
weather_latitude = 60.1695
weather_longitude = 24.9354
```

To return to automatic mode, empty `weather_location` and remove/comment out
both coordinate settings. City search chooses the first geocoding result; use
coordinates if the name is ambiguous. Restart nibari after editing the config.

Weather data and city search come from [Open-Meteo](https://open-meteo.com/)
([CC BY 4.0](https://creativecommons.org/licenses/by/4.0/)), also used by the
[Omarchy weather panel](https://github.com/omacom/omarchy/tree/quattro/shell/plugins/panels/weather).
No API key is needed. The system `curl` command performs bounded HTTPS requests
on a sleeping background worker. Automatic refresh defaults to 15 minutes
(configurable from 5 to 1440); errors retry after a minute. Opening the card or
clicking refresh requests an update, limited to once a minute. Failed updates
keep the last successful report with a visible stale-data notice. Nothing is
polled every frame or every second.

The independent `weather_background`, `weather_foreground`, `weather_muted`,
`weather_accent`, and `weather_border` colors support `#RRGGBB` and `#RRGGBBAA`.

### Network

Set `network_enabled = true` to show a connection icon immediately to the right
of the keyboard layout. Click it for connection type, ping, packet loss, current
receive/send rates, traffic totals, IP address, and gateway. The refresh button
updates measurements; Escape, right-click, or clicking outside closes the card.

`network_interface = ""` selects a live default-route interface automatically;
set an interface name to monitor it explicitly. Traffic totals are Linux interface
counters since the interface started. Rates update once per second while the card
is open. Ping uses the system `ping` command every two seconds against
`network_ping_target` (a numeric IP, default `1.1.1.1`) through normal system routing.
Loss covers up to the last 20 probes in the current viewing session; unavailable
measurements appear as a dash. The closed card stops periodic measurements and
the icon follows Linux connection events. NetworkManager is not required.

The card palette is configurable with `network_background`, `network_foreground`,
`network_muted`, `network_accent`, and `network_border`; all accept `#RRGGBBAA`
opacity. Restart nibari to apply configuration changes.

### Bar visibility

Control the running bar on all monitors:

```sh
nibari --hide
nibari --show
nibari --toggle
```

Hiding removes the bar and its reserved space, letting windows expand upwards.
Showing it restores the bar and its reserved space. Repeating `--hide` or `--show`
keeps the requested state. The bar stays hidden across workspace switches and
monitor changes until you show it again. No separate daemon is needed: these
commands signal the running bar using `pkill` from procps. They do not start a new
bar or load its configuration. If the bar is not running, they report an error.

Use the binary's full path if `nibari` is not installed in your `PATH`.
The original `pkill -USR1 -x nibari` toggle also remains available.

For example, add this binding inside `binds` in your niri config:

```kdl
Mod+Shift+B { spawn "nibari" "--toggle"; }
```

A left click on a tray icon triggers the main action, a middle click triggers the secondary
action, and a right click opens the application's context menu.

To hide tray icons behind a chevron that expands on hover, enable the optional drawer:

```toml
tray_drawer = true
tray_drawer_duration_ms = 600
```

The icons slide out to the left of a fixed chevron. The right-side order is drawer,
keyboard layout, optional network indicator, then clock (when positioned right). The drawer stays
open while the pointer is over the chevron, icons, or gaps, and while a nibari tray
menu is open. It collapses when the pointer leaves. Each monitor has its own drawer;
an empty tray has no chevron. The chevron uses the foreground color and requires no
icon font. Set `tray_drawer_duration_ms = 0` for instant transitions. The default is
`tray_drawer = false`, which keeps all icons visible. Restart nibari to apply changes.
Animation uses Wayland frame callbacks and adds no periodic timer while idle.

For applications that export a DBusMenu, nibari draws a rounded popup with a soft shadow,
hover highlighting, compact separators, checkboxes and radio buttons. It follows the
configured bar colors and font family, with a minimum menu font size of 13 for readability.
Long labels are shortened with an ellipsis; tall menus scroll to fit the output.

Left-click a menu item to activate it or open its submenu. Submenus have a back header;
right-click also goes back (or closes the root menu). Use Up/Down to select, Enter to
activate, Right/Left to enter/leave a submenu, and Escape or a click outside to dismiss.
The mouse wheel scrolls long menus; keyboard navigation keeps the selection visible.
Applications without an exported DBusMenu retain their own context-menu fallback.
