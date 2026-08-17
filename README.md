# kdock

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

| Compositor | Position at edge | Window list | Blur | Verdict |
|---|---|---|---|---|
| niri | ✅ | ✅ | ✅ | Fully working |
| Hyprland, sway, Wayfire, river, cosmic | ✅ | ✅ | own mechanism | Expected to work; **not yet tested** |
| KWin (Plasma) | ✅ | ✅ via D-Bus | ✅ | Fully working, and the primary target |
| GNOME (Mutter) | ❌ | ❌ | ❌ | **Cannot work.** See below |

### How KWin works, and what it still cannot do

KWin 6.7 implements no foreign-toplevel protocol a third-party client can use.
Checked by enumerating its globals: neither `zwlr_foreign_toplevel_manager_v1`
nor `ext_foreign_toplevel_list_v1` is advertised, and `org_kde_plasma_window_management`
— which KWin does implement — is not offered to ordinary clients. A Qt build of
this dock using `libtaskmanager` sees exactly the same 68 globals, so this is a
KWin limitation rather than anything to do with the toolkit.

The window list therefore comes from inside KWin instead. On startup the dock
loads a small script (`assets/kwin-windows.js`, compiled into the binary)
through `org.kde.kwin.Scripting`. The script watches the workspace and pushes
the window list — application class, title, active and minimised state — to a
D-Bus service the dock exposes. Clicking an icon raises the window through
KWin's own `WindowsRunner`. The script is unloaded when the dock exits, and
re-loaded cleanly if a previous copy was left behind by a hard kill.

Minimising and closing travel the other way. Nothing outside KWin can call into
a running script, so the script asks instead: it polls the dock a few times a
second for queued commands and carries them out. That is the whole reason the
context menu's Hide and Quit work on KWin at all.

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

The binary lands at `target/release/kdock`.

## Running

```bash
./target/release/kdock
```

To start it with the session, install the user service:

```bash
install -Dm644 dist/kdock.service ~/.config/systemd/user/kdock.service
systemctl --user enable --now kdock.service
```

## Configuration

`~/.config/kdock/config.toml`, re-read the moment it changes — no restart. See
[dist/config.example.toml](dist/config.example.toml) for every setting with its
default.

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
kdock --dump-frame /tmp/dock.png 900 org.kde.dolphin firefox code
kdock --dump-menu /tmp/menu.png
```

`KDOCK_CURSOR=<x>` places a virtual pointer for `--dump-frame` so the
magnification curve can be inspected, `KDOCK_SCALE=<n>` renders at an output
scale, and `KDOCK_TRASH=full|empty` previews the Trash tile. `KDOCK_DEBUG=1`
logs the window list as it changes.

## Status

Working: magnification, macOS click semantics, launch bounce, running
indicators, pinned launchers, the Trash, a right-click menu, compositor blur,
auto-hide, drag-to-reorder, and drops from other applications.

Not done: the `[measure]`-tagged values in `src/metrics.rs` are still estimates
awaiting calibration against a real reference screenshot, and moving the dock
between screens at runtime requires a restart.
