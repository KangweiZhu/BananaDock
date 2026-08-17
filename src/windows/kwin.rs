//! Window tracking on KWin, over D-Bus.
//!
//! KWin implements no foreign-toplevel Wayland protocol at all: not
//! `wlr-foreign-toplevel-management-v1`, not `ext-foreign-toplevel-list-v1`,
//! and its own `org_kde_plasma_window_management` global is never advertised to
//! ordinary clients -- a Qt/libtaskmanager build of this dock sees exactly the
//! same 68 globals as this one does, and none of them carry windows. So on KWin
//! the list has to come from inside the compositor.
//!
//! A small KWin script (`assets/kwin-windows.js`) is loaded through
//! `org.kde.kwin.Scripting`, and pushes the window list to a D-Bus service the
//! dock exposes. Activation goes back the other way through KWin's
//! `WindowsRunner`, which already accepts a window id and raises it.

use std::collections::HashMap;

use super::{Capabilities, Toplevel, ToplevelId, WindowSource};

const KWIN_SERVICE: &str = "org.kde.KWin";
const SCRIPTING_PATH: &str = "/Scripting";
const SCRIPTING_IFACE: &str = "org.kde.kwin.Scripting";
const RUNNER_PATH: &str = "/WindowsRunner";
const RUNNER_IFACE: &str = "org.kde.krunner1";

/// Name the dock claims on the session bus for the script to push into.
const OUR_SERVICE: &str = "org.kde.kdock";
const OUR_PATH: &str = "/Windows";

/// Identifies our script to KWin, for loading and unloading.
const PLUGIN_NAME: &str = "kdock-windows";

const FS: char = '\x1f';
const RS: char = '\x1e';

/// The script, compiled in so there is no install step and no chance of the
/// binary and the script drifting apart.
const SCRIPT: &str = include_str!("../../assets/kwin-windows.js");

/// Windows as last reported by the KWin script.
pub struct KwinWindows {
    toplevels: Vec<Toplevel>,
    /// Our synthetic ids are what the rest of the dock passes around, so they
    /// have to stay put across snapshots; KWin's own ids are UUID strings.
    ids: Ids,
    connection: Option<zbus::blocking::Connection>,
}

#[derive(Default)]
struct Ids {
    by_uuid: HashMap<String, ToplevelId>,
    next: ToplevelId,
}

impl Ids {
    fn get(&mut self, uuid: &str) -> ToplevelId {
        if let Some(id) = self.by_uuid.get(uuid) {
            return *id;
        }
        // Start at 1: zero reads as "no window" elsewhere.
        self.next += 1;
        self.by_uuid.insert(uuid.to_owned(), self.next);
        self.next
    }

    /// Forgets windows that are gone, so a long session does not accumulate
    /// dead entries.
    fn retain(&mut self, live: &[String]) {
        self.by_uuid.retain(|uuid, _| live.contains(uuid));
    }
}

impl KwinWindows {
    pub fn new(connection: Option<zbus::blocking::Connection>) -> Self {
        Self {
            toplevels: Vec::new(),
            ids: Ids::default(),
            connection,
        }
    }

    /// Replaces the window list from a snapshot pushed by the script.
    pub fn apply(&mut self, snapshot: &str) {
        let rows = parse_snapshot(snapshot);
        let uuids: Vec<String> = rows.iter().map(|r| r.uuid.clone()).collect();

        self.toplevels = rows
            .into_iter()
            .map(|r| Toplevel {
                id: self.ids.get(&r.uuid),
                app_id: r.app_id,
                title: r.title,
                active: r.active,
                minimized: r.minimized,
                // KWin's script API exposes transientFor, but the dock groups by
                // application anyway, so nothing needs it yet.
                parent: None,
            })
            .collect();

        self.ids.retain(&uuids);
    }

    /// The bus connection, for cleanup on the way out.
    pub fn connection(&self) -> Option<&zbus::blocking::Connection> {
        self.connection.as_ref()
    }

    fn uuid_of(&self, id: ToplevelId) -> Option<&str> {
        self.ids
            .by_uuid
            .iter()
            .find(|(_, v)| **v == id)
            .map(|(k, _)| k.as_str())
    }

    /// Asks KWin's window runner to raise a window.
    ///
    /// Fired onto a thread rather than awaited: a D-Bus round trip is short but
    /// not instant, and this runs from the pointer handler, where a stall shows
    /// up as the dock skipping a frame.
    fn run_window(&self, runner_id: String) {
        let Some(conn) = self.connection.clone() else {
            return;
        };
        std::thread::spawn(move || {
            if let Err(e) = conn.call_method(
                Some(KWIN_SERVICE),
                RUNNER_PATH,
                Some(RUNNER_IFACE),
                "Run",
                &(runner_id.as_str(), ""),
            ) {
                eprintln!("kdock: could not raise window: {e}");
            }
        });
    }
}

impl WindowSource for KwinWindows {
    fn toplevels(&self) -> &[Toplevel] {
        &self.toplevels
    }

    fn capabilities(&self) -> Capabilities {
        // KWin's runner only raises. A script could do more, but nothing can
        // call into a running KWin script, so there is no route back in.
        Capabilities {
            minimize: false,
            close: false,
        }
    }

    fn activate(&self, id: ToplevelId) {
        if let Some(uuid) = self.uuid_of(id) {
            // The runner keys windows as "0_" plus KWin's internal id, braces
            // and all.
            self.run_window(format!("0_{uuid}"));
        }
    }

    fn set_minimized(&self, _id: ToplevelId, _minimized: bool) {
        // KWin offers no D-Bus route to minimise a specific window: the runner
        // only raises, and a script cannot be called into. Restoring happens
        // implicitly, since raising a minimised window unminimises it.
    }

    fn close(&self, _id: ToplevelId) {
        // Same gap as `set_minimized`.
    }

    fn set_icon_rect(
        &self,
        _id: ToplevelId,
        _surface: &wayland_client::protocol::wl_surface::WlSurface,
        _rect: (i32, i32, i32, i32),
    ) {
        // Wayland-protocol-only; KWin's own minimise animation does not take a
        // hint over this route.
    }
}

/// One window as the script reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    uuid: String,
    app_id: String,
    title: String,
    active: bool,
    minimized: bool,
}

/// Parses the script's snapshot: records split by RS, fields by FS.
///
/// Both separators are ASCII control characters that cannot appear in a window
/// title or class, which is why the format needs no escaping. Malformed records
/// are skipped rather than failing the whole snapshot -- one odd window should
/// not empty the dock.
fn parse_snapshot(snapshot: &str) -> Vec<Row> {
    snapshot
        .split(RS)
        .filter(|r| !r.is_empty())
        .filter_map(|record| {
            let mut f = record.split(FS);
            let uuid = f.next()?.trim();
            if uuid.is_empty() {
                return None;
            }
            Some(Row {
                uuid: uuid.to_owned(),
                app_id: f.next().unwrap_or_default().to_owned(),
                title: f.next().unwrap_or_default().to_owned(),
                active: f.next() == Some("1"),
                minimized: f.next() == Some("1"),
            })
        })
        .collect()
}

/// How a snapshot gets from the D-Bus thread back to the event loop.
///
/// A callback rather than a channel so this module owes nothing to whatever
/// the main loop happens to use.
pub type Push = Box<dyn Fn(String) + Send + Sync + 'static>;

/// The dock's own D-Bus interface, which the KWin script pushes into.
struct WindowsService {
    push: Push,
}

#[zbus::interface(name = "org.kde.kdock.Windows")]
impl WindowsService {
    /// Called by the KWin script whenever the window list changes.
    fn update(&self, snapshot: String) {
        (self.push)(snapshot);
    }
}

/// Whether this session has a KWin to talk to.
pub fn is_available(conn: &zbus::blocking::Connection) -> bool {
    conn.call_method(
        Some(KWIN_SERVICE),
        SCRIPTING_PATH,
        Some("org.freedesktop.DBus.Peer"),
        "Ping",
        &(),
    )
    .is_ok()
}

/// Brings up the D-Bus service and gets the KWin script running.
///
/// Returns the connection, which doubles as the channel for activation calls.
pub fn start(push: Push) -> Result<zbus::blocking::Connection, Box<dyn std::error::Error>> {
    // The service has to answer before the script runs, or the script's first
    // push lands on nothing and the dock starts out empty.
    let conn = zbus::blocking::connection::Builder::session()?
        .name(OUR_SERVICE)?
        .serve_at(OUR_PATH, WindowsService { push })?
        .build()?;

    if !is_available(&conn) {
        return Err("KWin is not on this session bus".into());
    }

    load_script(&conn)?;
    Ok(conn)
}

/// Writes the script out and asks KWin to load and run it.
fn load_script(conn: &zbus::blocking::Connection) -> Result<(), Box<dyn std::error::Error>> {
    // A leftover copy from a previous run would double every push.
    unload_script(conn);

    let path = script_path()?;
    std::fs::write(&path, SCRIPT)?;

    let reply = conn.call_method(
        Some(KWIN_SERVICE),
        SCRIPTING_PATH,
        Some(SCRIPTING_IFACE),
        "loadScript",
        &(path.to_string_lossy().as_ref(), PLUGIN_NAME),
    )?;
    let id: i32 = reply.body().deserialize()?;

    // Scripts loaded this way are not started automatically.
    conn.call_method(
        Some(KWIN_SERVICE),
        format!("{SCRIPTING_PATH}/Script{id}").as_str(),
        Some("org.kde.kwin.Script"),
        "run",
        &(),
    )?;

    Ok(())
}

/// Takes the script back out of KWin.
///
/// Worth doing on the way out: a script left loaded keeps pushing to a bus name
/// that no longer answers.
pub fn unload_script(conn: &zbus::blocking::Connection) {
    if let Err(e) = conn.call_method(
        Some(KWIN_SERVICE),
        SCRIPTING_PATH,
        Some(SCRIPTING_IFACE),
        "unloadScript",
        &(PLUGIN_NAME,),
    ) {
        eprintln!("kdock: could not unload the KWin script: {e}");
    }
}

fn script_path() -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    Ok(dir.join("kdock-kwin-windows.js"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(uuid: &str, class: &str, title: &str, active: &str, min: &str) -> String {
        [uuid, class, title, active, min].join(&FS.to_string())
    }

    #[test]
    fn parses_a_two_window_snapshot() {
        let snap = [
            record("{aaa}", "konsole", "zsh", "1", "0"),
            record("{bbb}", "firefox", "News", "0", "1"),
        ]
        .join(&RS.to_string());

        let rows = parse_snapshot(&snap);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].app_id, "konsole");
        assert!(rows[0].active);
        assert!(!rows[0].minimized);
        assert!(rows[1].minimized);
    }

    #[test]
    fn an_empty_snapshot_means_no_windows() {
        assert!(parse_snapshot("").is_empty());
    }

    /// Titles routinely contain the characters that would break a naive
    /// delimiter choice; the control separators must survive them.
    #[test]
    fn titles_with_awkward_characters_survive() {
        let snap = record("{aaa}", "code", "a|b,c\"d — Code", "1", "0");
        let rows = parse_snapshot(&snap);
        assert_eq!(rows[0].title, "a|b,c\"d — Code");
    }

    /// A truncated record must not take the rest of the snapshot with it.
    #[test]
    fn short_records_fall_back_to_empty_fields() {
        let snap = format!("{{aaa}}{FS}konsole{RS}{}", record("{bbb}", "f", "t", "1", "0"));
        let rows = parse_snapshot(&snap);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].title, "");
        assert!(!rows[0].active);
        assert_eq!(rows[1].app_id, "f");
    }

    #[test]
    fn records_without_a_uuid_are_dropped() {
        let snap = format!("{FS}konsole{FS}zsh{FS}1{FS}0");
        assert!(parse_snapshot(&snap).is_empty());
    }

    /// The rest of the dock holds on to ids across updates, so the same window
    /// must keep the same one.
    #[test]
    fn ids_are_stable_across_snapshots() {
        let mut w = KwinWindows::new(None);
        let first = [
            record("{aaa}", "konsole", "zsh", "1", "0"),
            record("{bbb}", "firefox", "News", "0", "0"),
        ]
        .join(&RS.to_string());
        w.apply(&first);
        let konsole = w.toplevels()[0].id;
        let firefox = w.toplevels()[1].id;

        // Reordered, and one window retitled.
        let second = [
            record("{bbb}", "firefox", "Other", "1", "0"),
            record("{aaa}", "konsole", "zsh", "0", "0"),
        ]
        .join(&RS.to_string());
        w.apply(&second);

        assert_eq!(w.toplevels()[0].id, firefox);
        assert_eq!(w.toplevels()[1].id, konsole);
    }

    #[test]
    fn closed_windows_stop_taking_up_ids() {
        let mut w = KwinWindows::new(None);
        w.apply(&[
            record("{aaa}", "a", "a", "0", "0"),
            record("{bbb}", "b", "b", "0", "0"),
        ]
        .join(&RS.to_string()));
        assert_eq!(w.ids.by_uuid.len(), 2);

        w.apply(&record("{aaa}", "a", "a", "0", "0"));
        assert_eq!(w.ids.by_uuid.len(), 1);
        assert_eq!(w.toplevels().len(), 1);
    }

    #[test]
    fn ids_never_collide_with_the_no_window_sentinel() {
        let mut ids = Ids::default();
        assert_ne!(ids.get("{aaa}"), 0);
    }
}
