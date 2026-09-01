#!/usr/bin/env bash
#
# Build the working tree and put it on the running desktop, in one step.
#
#     dist/deploy.sh                 build, install under ~/.local, restart
#     dist/deploy.sh --package       install through pacman instead (needs root)
#     dist/deploy.sh --no-restart    install but leave the running dock alone
#     dist/deploy.sh --dry-run       say what would happen, change nothing
#
# The default route needs no root, which is what makes it usable from a script
# that cannot answer a password prompt. --package is the supported install and
# goes through makepkg, so pacman owns the files -- but pacman needs root, and
# that is a prompt someone has to be present for.
#
# Two things happen before the running dock is touched, in this order, because
# a dock that will not start leaves the desktop without one:
#
#   * the tests run
#   * the new binary renders a frame offscreen, and has to actually write it
#
# Either failing stops the deploy with the old dock still running.

set -euo pipefail

readonly REPO="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
readonly NAME=bananadock
readonly OLD_NAME=kdock

MODE=user
RESTART=yes
DRY_RUN=no

for arg in "$@"; do
    case "$arg" in
        --package) MODE=package ;;
        --no-restart) RESTART=no ;;
        --dry-run) DRY_RUN=yes ;;
        -h | --help)
            sed -n '3,20p' "${BASH_SOURCE[0]}" | sed 's/^# \?//'
            exit 0
            ;;
        *)
            echo "deploy.sh: unknown option $arg" >&2
            exit 2
            ;;
    esac
done

say() { printf '\033[1m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m==> %s\033[0m\n' "$*" >&2; }
run() {
    if [[ $DRY_RUN == yes ]]; then
        printf '    would run: %s\n' "$*"
    else
        "$@"
    fi
}

# --- where things go ----------------------------------------------------

# ~/.local/bin is not an XDG_DATA_HOME subdirectory -- the XDG base directory
# spec has nothing to say about executables, and $HOME/.local/bin is the
# convention systemd and the shell profile both expect. Only the other three
# follow their variables.
readonly USER_BIN="$HOME/.local/bin"
readonly APPS_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
readonly UNIT_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
readonly CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}"

# --- 1. leftovers from the old name -------------------------------------

migrate_from_kdock() {
    local moved=no

    if [[ -d $CONFIG_DIR/$OLD_NAME && ! -d $CONFIG_DIR/$NAME ]]; then
        say "moving $CONFIG_DIR/$OLD_NAME to $CONFIG_DIR/$NAME"
        run mv "$CONFIG_DIR/$OLD_NAME" "$CONFIG_DIR/$NAME"
        moved=yes
    elif [[ -d $CONFIG_DIR/$OLD_NAME && -d $CONFIG_DIR/$NAME ]]; then
        warn "both $CONFIG_DIR/$OLD_NAME and $CONFIG_DIR/$NAME exist; leaving both alone"
        warn "the dock reads $CONFIG_DIR/$NAME -- delete the other when you are sure"
    fi

    if systemctl --user list-unit-files "$OLD_NAME.service" &>/dev/null &&
        [[ -f $UNIT_DIR/$OLD_NAME.service ]]; then
        say "removing the old $OLD_NAME user service"
        run systemctl --user disable --now "$OLD_NAME.service" || true
        run rm -f "$UNIT_DIR/$OLD_NAME.service"
        run systemctl --user daemon-reload
        moved=yes
    fi

    local stale=()
    [[ -e $USER_BIN/$OLD_NAME ]] && stale+=("$USER_BIN/$OLD_NAME")
    for f in "$APPS_DIR/$OLD_NAME.desktop" "$APPS_DIR/$OLD_NAME-settings.desktop"; do
        [[ -e $f ]] && stale+=("$f")
    done
    if ((${#stale[@]})); then
        say "removing ${#stale[@]} file(s) left by $OLD_NAME"
        run rm -f "${stale[@]}"
        moved=yes
    fi

    [[ $moved == yes ]] && say "migration from $OLD_NAME done"
    return 0
}

# --- 2. build and prove it works ----------------------------------------

build_and_check() {
    say "building the working tree"
    run cargo build --release --manifest-path "$REPO/Cargo.toml"

    say "running the tests"
    run cargo test --manifest-path "$REPO/Cargo.toml" --all-targets --quiet

    [[ $DRY_RUN == yes ]] && return 0

    # Offscreen, so a broken build is caught before the running dock is
    # replaced -- and without synthesising a single pointer event on a desktop
    # somebody is using.
    local png
    png="$(mktemp --suffix=.png)"
    say "rendering a frame offscreen to prove the binary runs"
    if ! "$REPO/target/release/$NAME" --dump-frame "$png" 900 firefox >/dev/null 2>&1 ||
        [[ ! -s $png ]]; then
        rm -f "$png"
        echo "deploy.sh: the new binary did not render a frame; not touching the running dock" >&2
        exit 1
    fi
    say "rendered $(stat -c %s "$png") bytes; the binary starts and draws"
    rm -f "$png"
}

# --- 3. install ---------------------------------------------------------

install_user() {
    local target="$USER_BIN/$NAME"

    if [[ -e /usr/bin/$NAME ]]; then
        warn "/usr/bin/$NAME also exists, from the pacman package."
        warn "Two installs means two desktop entries; the one whose Exec= matches"
        warn "the binary that actually runs is the one KWin honours. Consider"
        warn "'sudo pacman -R ${NAME}-git' if you are staying with this route."
    fi

    say "installing the binary to $target"
    run install -Dm755 "$REPO/target/release/$NAME" "$target"

    # Exec= MUST be the absolute path of the binary that actually runs: KWin
    # traces the process back to a desktop entry through it and grants the
    # window-management and screenshot interfaces only on a match. The shipped
    # entries name /usr/bin, so both have to be rewritten here -- the settings
    # entry included, which the README's manual instructions used to miss.
    say "installing both desktop entries, with Exec= pointed at $target"
    if [[ $DRY_RUN == no ]]; then
        mkdir -p "$APPS_DIR"
        sed "s|^Exec=/usr/bin/$NAME|Exec=$target|" \
            "$REPO/dist/$NAME.desktop" > "$APPS_DIR/$NAME.desktop"
        sed "s|^Exec=/usr/bin/$NAME|Exec=$target|" \
            "$REPO/dist/$NAME-settings.desktop" > "$APPS_DIR/$NAME-settings.desktop"
    else
        printf '    would write: %s\n' "$APPS_DIR/$NAME.desktop" \
            "$APPS_DIR/$NAME-settings.desktop"
    fi

    # The shipped unit already starts %h/.local/bin/bananadock, which is where
    # this route puts it, so it installs unedited.
    say "installing the user service"
    run install -Dm644 "$REPO/dist/$NAME.service" "$UNIT_DIR/$NAME.service"
    run systemctl --user daemon-reload
}

install_package() {
    say "building the package with makepkg (pacman will ask for a password)"
    if [[ $DRY_RUN == yes ]]; then
        printf '    would run: makepkg -si in %s\n' "$REPO/dist"
        return 0
    fi
    if [[ -n $(git -C "$REPO" status --porcelain) ]]; then
        warn "the working tree is dirty, and the package is built from a git clone"
        warn "of the committed branch -- uncommitted changes will NOT be included."
    fi
    (cd "$REPO/dist" && makepkg -si)
}

# --- 4. put it on the desktop -------------------------------------------

restart_dock() {
    # An instance started by hand, or by an autostart entry, is not something
    # systemctl knows about -- and two docks would fight over the same layer
    # surface and the same D-Bus name.
    # -x against the process name, never -f against the whole command line:
    # this repository lives in a directory called kdock, so any shell whose
    # command line happens to end in that path would match a -f pattern and be
    # killed. The name is the only safe thing to match on.
    local found main strays
    found="$(
        pgrep -x "$NAME" || true
        pgrep -x "$OLD_NAME" || true
    )"

    # The service's own process must not be killed here. The unit is
    # Restart=always, so killing it makes systemd respawn it immediately and
    # the restart below is then the *second* time the dock goes away and comes
    # back. `systemctl restart` is the only thing that should stop that one.
    main="$(systemctl --user show "$NAME.service" -p MainPID --value 2>/dev/null || echo 0)"
    strays="$(printf '%s\n' "$found" | grep -vx -e '' -e "${main:-0}" || true)"
    if [[ -n $strays ]]; then
        say "stopping $(wc -l <<< "$strays") dock instance(s) not run by systemd"
        # shellcheck disable=SC2086
        run kill $strays || true
        [[ $DRY_RUN == no ]] && sleep 1
    fi

    # `enable` without --now, then `restart`: restart starts a unit that is not
    # running, so this is one start. `enable --now` followed by a restart is
    # two, and the dock visibly goes away and comes back twice.
    say "starting $NAME.service"
    run systemctl --user enable "$NAME.service"
    run systemctl --user restart "$NAME.service"

    [[ $DRY_RUN == yes ]] && return 0

    sleep 1
    if systemctl --user is-active --quiet "$NAME.service"; then
        say "the dock is running"
    else
        echo "deploy.sh: the service did not come up. Recent log:" >&2
        journalctl --user -u "$NAME.service" -n 20 --no-pager >&2
        exit 1
    fi
}

# --- go -----------------------------------------------------------------

[[ $DRY_RUN == yes ]] && say "dry run: nothing will be changed"

migrate_from_kdock
build_and_check

case $MODE in
    user) install_user ;;
    package) install_package ;;
esac

if [[ $RESTART == yes ]]; then
    restart_dock
else
    say "not restarting; run 'systemctl --user restart $NAME.service' when ready"
fi

say "done"
