//! Session-bus idle inhibition, including GTK's portal fallback.
#[cfg(feature = "dbus")]
mod implementation {
    use calloop::channel::Sender;
    use futures_util::StreamExt;
    use std::collections::{HashMap, hash_map::Entry};
    use std::sync::{Arc, Mutex};
    use zbus::names::{OwnedUniqueName, UniqueName};
    use zbus::zvariant::NoneValue;
    use zbus::{fdo, interface, message::Header};

    const BUS_NAME: &str = "org.freedesktop.ScreenSaver";
    const MAX_INHIBITORS: usize = 4096;

    #[derive(Default)]
    struct Cookies {
        owners: HashMap<u32, OwnedUniqueName>,
        next: u32,
        broken: bool,
    }

    #[derive(Clone)]
    struct ScreenSaver {
        cookies: Arc<Mutex<Cookies>>,
        changed: Sender<()>,
    }

    impl ScreenSaver {
        fn changed(&self) {
            let _ = self.changed.send(());
        }
    }

    #[interface(name = "org.freedesktop.ScreenSaver")]
    impl ScreenSaver {
        fn inhibit(
            &self,
            #[zbus(header)] header: Header<'_>,
            application_name: &str,
            reason_for_inhibit: &str,
        ) -> fdo::Result<u32> {
            let _ = (application_name, reason_for_inhibit);
            let owner = header
                .sender()
                .ok_or_else(|| fdo::Error::Failed("missing sender".into()))?
                .to_owned();
            let mut state = self.cookies.lock().unwrap();
            if state.broken {
                return Err(fdo::Error::Failed(
                    "client disconnect monitor unavailable".into(),
                ));
            }
            if state.owners.len() >= MAX_INHIBITORS {
                return Err(fdo::Error::LimitsExceeded(
                    "too many idle inhibitors".into(),
                ));
            }
            let cookie = loop {
                state.next = state.next.wrapping_add(1).max(1);
                let candidate = state.next;
                if let Entry::Vacant(entry) = state.owners.entry(candidate) {
                    entry.insert(OwnedUniqueName::from(owner));
                    break candidate;
                }
            };
            drop(state);
            self.changed();
            Ok(cookie)
        }

        fn un_inhibit(&self, #[zbus(header)] header: Header<'_>, cookie: u32) -> fdo::Result<()> {
            let sender = header
                .sender()
                .ok_or_else(|| fdo::Error::Failed("missing sender".into()))?;
            let mut state = self.cookies.lock().unwrap();
            match state.owners.get(&cookie) {
                Some(owner) if owner.as_str() == sender.as_str() => {
                    state.owners.remove(&cookie);
                }
                Some(_) => {
                    return Err(fdo::Error::AccessDenied(
                        "idle inhibitor belongs to another client".into(),
                    ));
                }
                None => return Err(fdo::Error::Failed("invalid idle inhibitor cookie".into())),
            }
            drop(state);
            self.changed();
            Ok(())
        }
    }

    pub struct Service {
        // Drop the monitor task before the connection so its retained clone
        // cannot keep this session's bus name alive after shutdown.
        _monitor: zbus::Task<()>,
        _connection: zbus::blocking::Connection,
        cookies: Arc<Mutex<Cookies>>,
    }

    impl Service {
        pub fn start(changed: Sender<()>) -> Result<Self, Box<dyn std::error::Error>> {
            Self::with_connection(zbus::blocking::Connection::session()?, changed)
        }

        pub fn with_connection(
            connection: zbus::blocking::Connection,
            changed: Sender<()>,
        ) -> Result<Self, Box<dyn std::error::Error>> {
            let cookies = Arc::new(Mutex::new(Cookies::default()));
            let interface = ScreenSaver {
                cookies: cookies.clone(),
                changed,
            };
            // Subscribe before accepting inhibitors. Otherwise a short-lived
            // caller can disconnect between Inhibit and monitor startup.
            let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
            let conn = connection.inner().clone();
            let monitor_interface = interface.clone();
            let monitor = connection.inner().executor().spawn(
                async move {
                    let result = async {
                        let proxy = fdo::DBusProxy::new(&conn).await?;
                        let mut changes = proxy
                            .receive_name_owner_changed_with_args(&[(2, UniqueName::null_value())])
                            .await?;
                        let _ = ready_tx.send(Ok::<(), String>(()));
                        while let Some(signal) = changes.next().await {
                            let args = signal.args()?;
                            if args.new_owner().is_none()
                                && let Some(owner) = &**args.old_owner()
                            {
                                let mut state = monitor_interface.cookies.lock().unwrap();
                                let before = state.owners.len();
                                state
                                    .owners
                                    .retain(|_, client| client.as_str() != owner.as_str());
                                let removed = state.owners.len() != before;
                                drop(state);
                                if removed {
                                    monitor_interface.changed();
                                }
                            }
                        }
                        Ok::<(), zbus::Error>(())
                    }
                    .await;
                    let error = result
                        .err()
                        .map(|e| e.to_string())
                        .unwrap_or_else(|| "session bus disconnected".into());
                    let _ = ready_tx.send(Err(error.clone()));
                    let mut state = monitor_interface.cookies.lock().unwrap();
                    state.broken = true;
                    state.owners.clear();
                    drop(state);
                    monitor_interface.changed();
                    eventline::warn!("idle inhibition: client monitor stopped: {error}");
                },
                "halley idle inhibitor cleanup",
            );
            ready_rx
                .recv_timeout(std::time::Duration::from_secs(2))?
                .map_err(std::io::Error::other)?;
            connection
                .object_server()
                .at("/org/freedesktop/ScreenSaver", interface.clone())?;
            connection.object_server().at("/ScreenSaver", interface)?;
            // Do not steal a user service's name, and never queue an obsolete
            // compositor behind the current session's owner.
            connection
                .request_name_with_flags(BUS_NAME, fdo::RequestNameFlags::DoNotQueue.into())?;
            Ok(Self {
                _monitor: monitor,
                _connection: connection,
                cookies,
            })
        }

        pub fn is_inhibited(&self) -> bool {
            !self.cookies.lock().unwrap().owners.is_empty()
        }
    }
}

#[cfg(feature = "dbus")]
pub use implementation::Service;

#[cfg(not(feature = "dbus"))]
pub struct Service;
#[cfg(not(feature = "dbus"))]
impl Service {
    pub fn is_inhibited(&self) -> bool {
        false
    }
}
