//! Whether the desktop is in its light or dark appearance, and staying in step
//! with it.
//!
//! macOS puts this one setting -- Light, Dark or Auto -- in System Settings and
//! the Dock simply follows it: the panel's material, the running dots, the
//! menus and the hover label all flip together. There is nothing dock-specific
//! to configure, and a dock that ignored it would be the only dark thing on a
//! light desktop.
//!
//! The desktop's own answer comes from the XDG settings portal, which every
//! desktop implements and which reports changes as they happen. KDE's own
//! `kdeglobals` would work on this machine and nowhere else.

use zbus::blocking::Connection;

const PORTAL_SERVICE: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const SETTINGS_IFACE: &str = "org.freedesktop.portal.Settings";
const NAMESPACE: &str = "org.freedesktop.appearance";
const KEY: &str = "color-scheme";

/// Which way round the palette goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Appearance {
    #[default]
    Dark,
    Light,
}

impl Appearance {
    /// The portal's `color-scheme`: 1 asks for dark, 2 for light, and 0 means
    /// the desktop has no opinion -- which is not the same as asking for
    /// light, so it keeps the dark dock the reference is drawn from.
    fn from_scheme(scheme: u32) -> Self {
        match scheme {
            2 => Appearance::Light,
            _ => Appearance::Dark,
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "light" => Some(Appearance::Light),
            "dark" => Some(Appearance::Dark),
            _ => None,
        }
    }
}

/// Asks the desktop which appearance it is in.
///
/// A desktop with no portal, or one that has never been asked, leaves this at
/// `None` -- the caller keeps whatever it had rather than flipping the dock on
/// a failed lookup.
pub fn detect(connection: Option<&Connection>) -> Option<Appearance> {
    let reply = connection?
        .call_method(
            Some(PORTAL_SERVICE),
            PORTAL_PATH,
            Some(SETTINGS_IFACE),
            "Read",
            &(NAMESPACE, KEY),
        )
        .ok()?;

    // `Read` answers a variant wrapping the number -- wrapping a second
    // variant, on some implementations.
    let value: zbus::zvariant::OwnedValue = reply.body().deserialize().ok()?;
    scheme_of(&value).map(Appearance::from_scheme)
}

/// Digs the number out of however many layers of variant it arrived in.
fn scheme_of(value: &zbus::zvariant::Value<'_>) -> Option<u32> {
    match value {
        zbus::zvariant::Value::U32(n) => Some(*n),
        zbus::zvariant::Value::Value(inner) => scheme_of(inner),
        _ => None,
    }
}

/// Calls `on_change` whenever the desktop's appearance changes.
///
/// Runs on its own thread: the signal arrives whenever the user flips the
/// setting -- or, with a scheduled Auto, at dusk -- and the dock's event loop
/// must not sit blocked waiting for something that may never come.
pub fn watch<F>(connection: Option<Connection>, on_change: F)
where
    F: Fn(Appearance) + Send + 'static,
{
    let Some(connection) = connection else {
        return;
    };

    std::thread::spawn(move || {
        let rule = match zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface(SETTINGS_IFACE)
            .and_then(|b| b.member("SettingChanged"))
        {
            Ok(builder) => builder.build(),
            Err(_) => return,
        };
        let Ok(stream) = zbus::blocking::MessageIterator::for_match_rule(rule, &connection, None)
        else {
            return;
        };

        for message in stream.flatten() {
            let Ok((namespace, key, value)) =
                message
                    .body()
                    .deserialize::<(String, String, zbus::zvariant::OwnedValue)>()
            else {
                continue;
            };
            if namespace != NAMESPACE || key != KEY {
                continue;
            }
            if let Some(scheme) = scheme_of(&value) {
                on_change(Appearance::from_scheme(scheme));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The portal's numbers, including the one that means "no opinion" -- which
    /// must not be read as a request for light.
    #[test]
    fn the_portals_numbers_map_to_an_appearance() {
        assert_eq!(Appearance::from_scheme(1), Appearance::Dark);
        assert_eq!(Appearance::from_scheme(2), Appearance::Light);
        assert_eq!(Appearance::from_scheme(0), Appearance::Dark);
        assert_eq!(Appearance::from_scheme(99), Appearance::Dark);
    }

    /// The number arrives wrapped in a variant, sometimes twice over.
    #[test]
    fn the_number_is_found_however_it_is_wrapped() {
        use zbus::zvariant::Value;
        // Bare, wrapped once, and wrapped twice -- all three shapes have been
        // seen in the wild, depending on which portal answers.
        let once = Value::Value(Box::new(Value::U32(1)));
        let twice = Value::Value(Box::new(once.try_clone().unwrap()));
        assert_eq!(scheme_of(&Value::U32(2)), Some(2));
        assert_eq!(scheme_of(&once), Some(1));
        assert_eq!(scheme_of(&twice), Some(1));
        assert_eq!(scheme_of(&Value::Str("light".into())), None);
    }

    #[test]
    fn appearances_round_trip_through_their_names() {
        assert_eq!(Appearance::parse("light"), Some(Appearance::Light));
        assert_eq!(Appearance::parse("DARK"), Some(Appearance::Dark));
        assert_eq!(Appearance::parse("system"), None);
    }
}
