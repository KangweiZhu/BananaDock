# BananaDock

A macOS-style dock for Wayland.

Icons magnify by relaying out the whole row rather than scaling in place, so
neighbours part around the pointer the way they do on macOS. The panel is a
capsule with compositor-composited frosted glass behind it, and its proportions
are derived from a macOS Tahoe reference rather than guessed.

## Compositor support

The dock needs two things from a compositor: a way to sit at a screen edge and
reserve space (`wlr-layer-shell`), and a way to list and activate other
applications' windows. The second has no portable answer, so there are two
implementations of it -- `wlr-foreign-toplevel-management` where it exists, and
a D-Bus route on KWin, which has no such protocol. Everything else degrades
quietly when missing.

| Compositor | Position at edge | Window list | Blur | Thumbnails | Verdict |
|---|---|---|---|---|---|
| niri | ✅ | ✅ | ✅ | ❌ icon instead | Fully working |
| Hyprland, sway, Wayfire, river, cosmic | ✅ | ✅ | own mechanism | ❌ icon instead | Expected to work; **not yet tested** |
| KWin (Plasma) | ✅ | ✅ via D-Bus | ✅ | ✅ via D-Bus | Fully working, and the primary target |
| GNOME (Mutter) | ❌ | ❌ | ❌ | ❌ | **Cannot work.** See below |

GNOME's own dock is not a counter-example: the Dash, and extensions like Dash
to Dock, run *inside* GNOME Shell as JavaScript rather than as Wayland clients.
They draw on the shell's own stage instead of asking for a layer surface, read
`Meta.Window` instead of a foreign-toplevel protocol, and clone a window's
actor instead of copying pixels out of it. None of those routes exist for a
separate process, which is what this dock is.

Thumbnails are the picture of the window itself on a minimised tile. Only the
KWin route is implemented, through `org.kde.KWin.ScreenShot2`; everywhere else
a minimised window still gets its own tile and shows the application's icon.

That is a gap in this dock rather than a wall. `ext-image-copy-capture-v1`,
with a source from `ext_foreign_toplevel_image_capture_source_manager_v1`, is
the portable equivalent, and Hyprland also has its own
`hyprland-toplevel-export-v1`. Two things would need checking before either is
worth writing: whether the compositor implements it at all, and whether a
*minimised* window still has a buffer to copy. The lazy capture here works
because KWin keeps a minimised window's last frame; a compositor that drops it
would force the picture to be taken before the window is hidden.

### How KWin works

KWin implements no *portable* foreign-toplevel protocol: neither
`zwlr_foreign_toplevel_manager_v1` nor `ext_foreign_toplevel_list_v1` is
advertised. It does implement its own `org_kde_plasma_window_management`, but
withholds it from clients that have not asked for it by name.

Asking is done in the desktop entry:

```ini
X-KDE-Wayland-Interfaces=org_kde_plasma_window_management
```

KWin traces the connecting process back to a desktop entry and checks that
list. Three details decide whether it works, and every one of them fails
silently — the global simply never appears:

* **`Exec=` must be the absolute path** to the binary that actually runs. A
  bare command name does not resolve back to the entry.
* **Entries are comma-separated.** KWin reads the key as a KConfig list, not as
  the semicolon-separated list the desktop-entry spec defines. A trailing
  semicolon makes the whole value unmatchable.
* The entry has to be installed somewhere KWin will find it, such as
  `~/.local/share/applications`. `NoDisplay` does not affect the grant.

With the grant in place the dock uses the protocol directly: event-driven
updates, activate, minimise, close, and `set_minimized_geometry`, which points
KWin's minimise animation at the right slot in the dock.

A second entry earns the thumbnails on the minimised tiles:

```ini
X-KDE-DBUS-Restricted-Interfaces=org.kde.KWin.ScreenShot2
```

`org.kde.KWin.ScreenShot2.CaptureWindow` writes a window's raw pixels into a
file descriptor the caller passes in, so a thumbnail costs one D-Bus call and a
read — no PipeWire, no stream to negotiate, no GPU buffer to import. KWin keeps
a minimised window's last frame, so the capture can happen lazily, when a tile
first needs a picture. Without the grant the tiles show their application's icon
instead.

### The fallback, if the grant is missing

Without it the dock loads a small script (`assets/kwin-windows.js`, compiled
into the binary) through `org.kde.kwin.Scripting`. The script pushes the window
list to a D-Bus service the dock exposes, and polls that same service a few
times a second for commands to carry out. Clicking an icon raises the window
through KWin's `WindowsRunner`.

It works, but it is strictly worse — polled rather than event-driven, and it
injects a script into the compositor — so it only runs when the native protocol
was not granted.

### Why GNOME cannot work at all

Mutter does not implement `wlr-layer-shell` and has consistently declined to.
Without it there is no way for a normal application to place itself at a screen
edge or reserve space, so the dock cannot be positioned at all. Its
`ext-foreign-toplevel-list-v1` support does not help either: that protocol can
enumerate windows but has no request to activate, minimise or close one, so
clicking an icon could not do anything.

A dock on GNOME has to be a GNOME Shell extension running inside the shell
process. That is a separate program in a different language; nothing here can be
reused beyond the design values.

## Building

Needs a Rust toolchain and the development files for Wayland, xkbcommon and
fontconfig.

```bash
cargo build --release
```

The binary lands at `target/release/bananadock`.

## Installing on Arch

[dist/PKGBUILD](dist/PKGBUILD) builds and installs the dock through pacman:

```bash
cd dist && makepkg -si
```

It packages the current default branch rather than a release, because there are
no tagged releases yet; the version it reports (`0.1.0.rN.g<hash>`) counts
commits so that upgrades still order correctly. The package installs the binary
to `/usr/bin/bananadock`, both desktop entries to `/usr/share/applications`, and
the user unit to `/usr/lib/systemd/user`, so the manual install below is not
needed — start it with:

```bash
systemctl --user enable --now bananadock.service
```

The desktop entries matter more here than they look: their `Exec=` has to be
the absolute path of the binary that actually runs or KWin withholds the window
list, and `/usr/bin/bananadock` is exactly where the package puts it. That is
one fewer thing to get wrong than in the manual install.

To publish it to the AUR as `bananadock-git`, the PKGBUILD needs only its
generated companion:

```bash
cd dist && makepkg --printsrcinfo > .SRCINFO
```

`.SRCINFO` is deliberately not committed here: it is a build artefact that goes
stale the moment anything above it changes, and it belongs in the AUR
repository rather than in this one.

## Deploying a change

[dist/deploy.sh](dist/deploy.sh) takes the working tree to a running dock in
one step — build, test, install, restart:

```bash
dist/deploy.sh              # install under ~/.local, restart the dock
dist/deploy.sh --package    # install through pacman instead (asks for root)
dist/deploy.sh --no-restart # install, leave the running dock alone
dist/deploy.sh --dry-run    # say what would happen, change nothing
```

Two things have to pass before the running dock is touched, because a dock
that will not start leaves the desktop without one: the test suite, and a
frame the new binary renders offscreen and must actually write. Either failing
stops the deploy with the old dock still up.

It also migrates an install left by the old `kdock` name — the configuration
directory, the user service, the desktop entries and the binary — and takes
down an instance started by hand, which `systemctl` does not know about and
which would otherwise fight the new one for the same layer surface and D-Bus
name.

The default route installs under `~/.local` and needs no root, which is what
makes it usable unattended. `--package` is the supported install and goes
through `makepkg`, so pacman owns the files; pick one route and stay on it,
since two installs mean two desktop entries with different `Exec=` paths.

## Running

```bash
./target/release/bananadock
```

To start it with the session, install the user service:

```bash
# The desktop entry is what earns the dock its window list on KWin, and its
# Exec= has to name the installed binary by absolute path.
install -Dm755 target/release/bananadock ~/.local/bin/bananadock
sed "s|^Exec=.*|Exec=$HOME/.local/bin/bananadock|" dist/bananadock.desktop \
    > ~/.local/share/applications/bananadock.desktop

install -Dm644 dist/bananadock.service ~/.config/systemd/user/bananadock.service
systemctl --user enable --now bananadock.service
```

The unit uses `Restart=always` rather than `on-failure` on purpose: the dock
exits *zero* when the compositor goes away, so `on-failure` would leave it gone
for the rest of the session after every compositor restart.

## Quitting, and the dock's own menu

Right-click the dock -- the separator, or any part of the panel that is not an
icon -- for magnification and auto-hide toggles, the settings window, and
**Quit BananaDock**. The dock has no window and no tray icon, so this is the
way out short of `systemctl --user stop bananadock.service`.

## Configuration

A settings window covers the common options:

```sh
bananadock --settings
```

It writes straight to the configuration file below -- there is no separate
store and nothing to apply. The dock is already watching that file, so a change
takes effect as it is made. Options the window does not cover (the icon theme,
which output to sit on, the pinned list) are edited in the file, or in the dock
itself by dragging.


`~/.config/bananadock/config.toml`, re-read the moment it changes — no
restart. See [dist/config.example.toml](dist/config.example.toml) for every
setting with its default.

Only preferences live in the config. The proportions measured off the reference
— how large the artwork is relative to its tile, the panel height, the capsule
radius — are design constants in `src/metrics.rs`, so that the dock cannot be
configured into something that is no longer the thing being replicated.

## Development

```bash
cargo test
cargo clippy --all-targets
```

Two offline rendering modes make it possible to check pixels without a
compositor, which is also how the layout is compared against a reference
screenshot:

```bash
bananadock --dump-frame /tmp/dock.png 900 org.kde.dolphin firefox code
bananadock --dump-menu /tmp/menu.png
```

`BANANADOCK_CURSOR=<x>` places a virtual pointer for `--dump-frame` so the
magnification curve can be inspected, `BANANADOCK_SCALE=<n>` renders at an
output scale, and `BANANADOCK_TRASH=full|empty` previews the Trash tile.
`BANANADOCK_APPEARANCE=light|dark` picks the palette, which the dumps cannot
ask the desktop for — without it the light colours cannot be looked at at all.
`BANANADOCK_DEBUG=1` logs the window list as it changes.

## Status

Working: magnification, macOS click semantics, launch bounce, running
indicators, pinned launchers, the Trash, a right-click menu, compositor blur,
auto-hide, drag-to-reorder, and drops from other applications.

Not done: the `[measure]`-tagged values in `src/metrics.rs` are still estimates
awaiting calibration against a real reference screenshot, and moving the dock
between screens at runtime requires a restart.

## License

Copyright (C) 2026 Kangwei Zhu.

GPL-3.0-or-later. [LICENSE](LICENSE) is the licence text as published by the
Free Software Foundation; `Cargo.toml` and the Arch package declare the same
SPDX identifier.
