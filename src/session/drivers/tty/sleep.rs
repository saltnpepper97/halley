#[cfg(feature = "dbus")]
use std::thread;

use calloop::LoopHandle;
#[cfg(feature = "dbus")]
use calloop::channel::{Event, channel};

#[cfg(feature = "dbus")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SleepEvent {
    Preparing,
    Resumed,
}

/// Bridges logind's system-sleep lifecycle onto the compositor event loop.
///
/// libseat stays active across ordinary S3 suspend, so its VT notifier cannot
/// be used to invalidate pre-suspend DRM buffers.
#[cfg(feature = "dbus")]
pub(super) fn install(
    loop_handle: &LoopHandle<'_, super::TtyApp>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (sender, receiver) = channel();
    thread::Builder::new()
        .name("halley logind sleep monitor".into())
        .spawn(move || {
            let connection = match zbus::blocking::Connection::system() {
                Ok(connection) => connection,
                Err(err) => {
                    eventline::warn!("system sleep: could not connect to system bus: {err}");
                    return;
                }
            };
            let proxy = match zbus::blocking::Proxy::new(
                &connection,
                "org.freedesktop.login1",
                "/org/freedesktop/login1",
                "org.freedesktop.login1.Manager",
            ) {
                Ok(proxy) => proxy,
                Err(err) => {
                    eventline::warn!("system sleep: login1 proxy unavailable: {err}");
                    return;
                }
            };
            let signals = match proxy.receive_signal("PrepareForSleep") {
                Ok(signals) => signals,
                Err(err) => {
                    eventline::warn!("system sleep: could not subscribe to logind: {err}");
                    return;
                }
            };
            for message in signals {
                let preparing: bool = match message.body().deserialize() {
                    Ok(preparing) => preparing,
                    Err(err) => {
                        eventline::warn!("system sleep: invalid PrepareForSleep signal: {err}");
                        continue;
                    }
                };
                let event = if preparing {
                    SleepEvent::Preparing
                } else {
                    SleepEvent::Resumed
                };
                if sender.send(event).is_err() {
                    break;
                }
            }
        })?;

    loop_handle.insert_source(receiver, |event, _, app| {
        if let Event::Msg(event) = event {
            super::handle_system_sleep(app, event == SleepEvent::Preparing);
        }
    })?;
    Ok(())
}

#[cfg(not(feature = "dbus"))]
pub(super) fn install(
    _loop_handle: &LoopHandle<'_, super::TtyApp>,
) -> Result<(), Box<dyn std::error::Error>> {
    Ok(())
}
