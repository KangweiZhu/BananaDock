// Reports the window list to kdock, from inside KWin.
//
// KWin implements no foreign-toplevel Wayland protocol -- not the wlroots one,
// not the ext one, and its own org_kde_plasma_window_management global is never
// advertised to ordinary clients. A KWin script is the only place a third-party
// dock can see the window list at all, so this runs inside KWin and pushes the
// state out over D-Bus.
//
// Loaded and unloaded by kdock itself through org.kde.kwin.Scripting; it is not
// meant to be installed by hand.

var SERVICE = "org.kde.kdock";
var PATH = "/Windows";
var IFACE = "org.kde.kdock.Windows";

// Field and record separators. ASCII unit/record separators cannot occur in a
// window title or class, so no escaping is needed.
var FS = "\x1f";
var RS = "\x1e";

function windowList() {
    // `windowList()` is the Plasma 6 spelling; older builds expose `windows`.
    return workspace.windowList ? workspace.windowList() : workspace.windows;
}

function isTask(w) {
    // Match what a task bar shows: real application windows only, minus the
    // ones that explicitly opt out.
    return w && w.normalWindow && !w.skipTaskbar && !w.deleted;
}

function snapshot() {
    var wins = windowList();
    var rows = [];
    for (var i = 0; i < wins.length; ++i) {
        var w = wins[i];
        if (!isTask(w)) {
            continue;
        }
        rows.push(
            [
                String(w.internalId),
                String(w.resourceClass || ""),
                String(w.caption || ""),
                w.active ? "1" : "0",
                w.minimized ? "1" : "0",
            ].join(FS)
        );
    }
    return rows.join(RS);
}

// Several signals fire for one user action (activating a window also changes
// the previous window's active flag), so this runs more often than strictly
// needed. The payload is small and the receiver is idempotent, so the extra
// calls are cheaper than the machinery to coalesce them.
function push() {
    callDBus(SERVICE, PATH, IFACE, "Update", snapshot());
}

// Per-window signals: the workspace-level ones do not fire when a window is
// merely minimised or retitled.
function watch(w) {
    if (!w) {
        return;
    }
    if (w.minimizedChanged) {
        w.minimizedChanged.connect(push);
    }
    if (w.captionChanged) {
        w.captionChanged.connect(push);
    }
    if (w.skipTaskbarChanged) {
        w.skipTaskbarChanged.connect(push);
    }
}

var initial = windowList();
for (var i = 0; i < initial.length; ++i) {
    watch(initial[i]);
}

workspace.windowAdded.connect(function (w) {
    watch(w);
    push();
});
workspace.windowRemoved.connect(push);
workspace.windowActivated.connect(push);

push();
