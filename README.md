# nibari

An ultra-minimalist bar for niri wm, inspired by awesomewm. On the left are workspaces with labels `一 二 三 四 五 六 七 八 九 零` (I can't be bothered to move this to the config)

## Build and run

```sh
cargo build --release
./target/release/nibari
```

For autostart, specify the compiled binary in your niri config.

## Configuration

By default, it reads from:

```text
$XDG_CONFIG_HOME/nibari/config.toml
```

If `XDG_CONFIG_HOME` is not set, it uses
`~/.config/nibari/config.toml`. A missing config file is not an error:
built-in values will be applied.

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

The appearance of the central tasklist is configured via `task_*` parameters.
The icon is searched for first via the application's desktop file, then in the selected icon
theme; if it is missing, a stable colored placeholder is drawn.

Left-click a workspace label to switch to that workspace on its monitor. Left-click
anywhere on a task's icon or title button to focus that window.

A left click on a tray icon triggers the main action, a middle click triggers the secondary
action, and a right click opens the application's context menu.

For applications that export a DBusMenu, nibari draws a rounded popup with a soft shadow,
hover highlighting, compact separators, checkboxes and radio buttons. It follows the
configured bar colors and font family, with a minimum menu font size of 13 for readability.
Long labels are shortened with an ellipsis; tall menus scroll to fit the output.

Left-click a menu item to activate it or open its submenu. Submenus have a back header;
right-click also goes back (or closes the root menu). Use Up/Down to select, Enter to
activate, Right/Left to enter/leave a submenu, and Escape or a click outside to dismiss.
The mouse wheel scrolls long menus; keyboard navigation keeps the selection visible.
Applications without an exported DBusMenu retain their own context-menu fallback.
