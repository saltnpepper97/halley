use std::error::Error;

use calloop::LoopHandle;
#[cfg(feature = "dbus")]
use calloop::channel::{Event, sync_channel};

const PORTAL_DESTINATION: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const SETTINGS_INTERFACE: &str = "org.freedesktop.portal.Settings";
const APPEARANCE_NAMESPACE: &str = "org.freedesktop.appearance";
const COLOR_SCHEME_KEY: &str = "color-scheme";

fn scheme_from_portal_value(value: u32) -> halley_config::SystemColorScheme {
    match value {
        1 => halley_config::SystemColorScheme::PreferDark,
        2 => halley_config::SystemColorScheme::PreferLight,
        _ => halley_config::SystemColorScheme::NoPreference,
    }
}

#[cfg(feature = "dbus")]
fn read_color_scheme() -> Result<halley_config::SystemColorScheme, Box<dyn Error>> {
    let connection = zbus::blocking::Connection::session()?;
    let proxy = zbus::blocking::Proxy::new(
        &connection,
        PORTAL_DESTINATION,
        PORTAL_PATH,
        SETTINGS_INTERFACE,
    )?;
    let value: zbus::zvariant::OwnedValue =
        proxy.call("Read", &(APPEARANCE_NAMESPACE, COLOR_SCHEME_KEY))?;
    Ok(scheme_from_portal_value(u32::try_from(value)?))
}

pub fn current_color_scheme() -> halley_config::SystemColorScheme {
    #[cfg(feature = "dbus")]
    match read_color_scheme() {
        Ok(scheme) => scheme,
        Err(error) => {
            eventline::debug!(
                "appearance: could not read org.freedesktop.appearance color-scheme: {error}"
            );
            halley_config::SystemColorScheme::NoPreference
        }
    }

    #[cfg(not(feature = "dbus"))]
    halley_config::SystemColorScheme::NoPreference
}

/// Watches the XDG Settings portal on a worker thread and delivers changes on
/// the compositor event loop. The worker owns all blocking D-Bus work.
pub fn watch<App: 'static>(
    loop_handle: &LoopHandle<'_, App>,
    mut notify: impl FnMut(&mut App, halley_config::SystemColorScheme) + 'static,
) -> Result<(), Box<dyn Error>> {
    #[cfg(feature = "dbus")]
    {
        let (sender, receiver) = sync_channel(4);
        std::thread::Builder::new()
            .name("halley appearance watcher".to_string())
            .spawn(move || {
                let result = (|| -> Result<(), Box<dyn Error>> {
                    let connection = zbus::blocking::Connection::session()?;
                    let proxy = zbus::blocking::Proxy::new(
                        &connection,
                        PORTAL_DESTINATION,
                        PORTAL_PATH,
                        SETTINGS_INTERFACE,
                    )?;
                    let signals = proxy.receive_signal_with_args(
                        "SettingChanged",
                        &[(0, APPEARANCE_NAMESPACE), (1, COLOR_SCHEME_KEY)],
                    )?;
                    // Read on this worker after the session environment is
                    // published. GTK may need the compositor to dispatch
                    // Wayland requests while the portal starts.
                    match proxy.call::<_, _, zbus::zvariant::OwnedValue>(
                        "Read",
                        &(APPEARANCE_NAMESPACE, COLOR_SCHEME_KEY),
                    ) {
                        Ok(initial) => {
                            if let Ok(value) = u32::try_from(initial)
                                && sender.send(scheme_from_portal_value(value)).is_err()
                            {
                                return Ok(());
                            }
                        }
                        Err(error) => eventline::debug!(
                            "appearance: initial colour scheme unavailable: {error}"
                        ),
                    }
                    for message in signals {
                        let (_, _, value): (String, String, zbus::zvariant::OwnedValue) =
                            message.body().deserialize()?;
                        let Ok(value) = u32::try_from(value) else {
                            continue;
                        };
                        if sender.send(scheme_from_portal_value(value)).is_err() {
                            break;
                        }
                    }
                    Ok(())
                })();
                if let Err(error) = result {
                    eventline::debug!("appearance: system colour watcher stopped: {error}");
                }
            })?;

        loop_handle.insert_source(receiver, move |event, _, app| {
            if let Event::Msg(scheme) = event {
                notify(app, scheme);
            }
        })?;
    }

    #[cfg(not(feature = "dbus"))]
    let _ = (loop_handle, &mut notify);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portal_values_follow_the_xdg_appearance_contract() {
        assert_eq!(
            scheme_from_portal_value(0),
            halley_config::SystemColorScheme::NoPreference
        );
        assert_eq!(
            scheme_from_portal_value(1),
            halley_config::SystemColorScheme::PreferDark
        );
        assert_eq!(
            scheme_from_portal_value(2),
            halley_config::SystemColorScheme::PreferLight
        );
        assert_eq!(
            scheme_from_portal_value(99),
            halley_config::SystemColorScheme::NoPreference
        );
    }
}
