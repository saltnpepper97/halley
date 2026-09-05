use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use futures_util::StreamExt;
use smithay::backend::input::Keycode;
use smithay::input::keyboard::Keysym;
use zbus::blocking::Connection;
use zbus::fdo::{self, RequestNameFlags};
use zbus::interface;
use zbus::message::Header;
use zbus::names::{BusName, OwnedUniqueName, UniqueName};
use zbus::zvariant::NoneValue;

const BUS_NAME: &str = "org.freedesktop.a11y.Manager";
const OBJECT_PATH: &str = "/org/freedesktop/a11y/Manager";
const INTERFACE: &str = "org.freedesktop.a11y.KeyboardMonitor";
const AUTHORIZED_MONITOR: &str = "org.gnome.Orca.KeyboardMonitor";
const REPEAT_DELAY: Duration = Duration::from_millis(200);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum KeyboardDisposition {
    Pass,
    Intercept,
}

#[derive(Clone, Copy, Debug)]
pub struct KeyboardEvent {
    pub time: Duration,
    pub keycode: Keycode,
    pub released: bool,
    pub modifiers: u32,
    pub keysym: Keysym,
    pub unicode: u32,
}

#[derive(Debug, Default)]
struct Client {
    watched: bool,
    grabbed: bool,
    modifiers: HashSet<Keysym>,
    keystrokes: Vec<(Keysym, u32)>,
    pressed_modifiers: HashSet<Keysym>,
    last_modifier_press: HashMap<Keysym, Duration>,
    forwarded_modifiers: HashSet<Keysym>,
    intercepted_keys: HashSet<Keycode>,
}

#[derive(Debug, Default)]
struct MonitorData {
    clients: HashMap<OwnedUniqueName, Client>,
}

#[derive(Debug)]
struct ClientRoute {
    emit: bool,
    disposition: KeyboardDisposition,
}

#[derive(Debug)]
struct MonitorRoute {
    recipients: Vec<OwnedUniqueName>,
    disposition: KeyboardDisposition,
}

impl Client {
    /// Replaces declarative grabs without dropping press ownership. Clients
    /// may update grabs from inside a press signal, so transient state must
    /// live until the matching physical release.
    fn replace_grabs(&mut self, modifiers: Vec<u32>, keystrokes: Vec<(u32, u32)>) {
        self.modifiers = modifiers.into_iter().map(Keysym::new).collect();
        self.keystrokes = keystrokes
            .into_iter()
            .map(|(keysym, modifiers)| (Keysym::new(keysym), modifiers))
            .collect();
    }

    fn route(&mut self, event: KeyboardEvent, repeat_delay: Duration) -> ClientRoute {
        if self.intercepted_keys.contains(&event.keycode) {
            if event.released {
                self.intercepted_keys.remove(&event.keycode);
                return ClientRoute {
                    emit: true,
                    disposition: KeyboardDisposition::Intercept,
                };
            }
            return ClientRoute {
                emit: false,
                disposition: KeyboardDisposition::Intercept,
            };
        }

        if self.modifiers.contains(&event.keysym)
            || self.pressed_modifiers.contains(&event.keysym)
            || self.forwarded_modifiers.contains(&event.keysym)
        {
            return self.route_modifier(event, repeat_delay);
        }

        let grabbed = self.grabbed
            || !self.pressed_modifiers.is_empty()
            || self.keystrokes.iter().any(|(keysym, modifiers)| {
                *keysym == event.keysym && *modifiers == event.modifiers
            });
        if grabbed {
            if !event.released {
                self.intercepted_keys.insert(event.keycode);
            }
            return ClientRoute {
                emit: true,
                disposition: KeyboardDisposition::Intercept,
            };
        }

        ClientRoute {
            emit: self.watched,
            disposition: KeyboardDisposition::Pass,
        }
    }

    fn route_modifier(&mut self, event: KeyboardEvent, repeat_delay: Duration) -> ClientRoute {
        if event.released && self.forwarded_modifiers.remove(&event.keysym) {
            return ClientRoute {
                emit: false,
                disposition: KeyboardDisposition::Pass,
            };
        }

        if !event.released {
            if self.pressed_modifiers.contains(&event.keysym) {
                return ClientRoute {
                    emit: false,
                    disposition: KeyboardDisposition::Intercept,
                };
            }
            let repeated_soon = self
                .last_modifier_press
                .get(&event.keysym)
                .is_some_and(|last| event.time < last.saturating_add(repeat_delay));
            if repeated_soon {
                self.forwarded_modifiers.insert(event.keysym);
                return ClientRoute {
                    emit: false,
                    disposition: KeyboardDisposition::Pass,
                };
            }
            self.last_modifier_press.insert(event.keysym, event.time);
            self.pressed_modifiers.insert(event.keysym);
        } else if !self.pressed_modifiers.remove(&event.keysym) {
            return ClientRoute {
                emit: false,
                disposition: KeyboardDisposition::Pass,
            };
        }

        ClientRoute {
            emit: true,
            disposition: KeyboardDisposition::Intercept,
        }
    }
}

impl MonitorData {
    fn route(&mut self, event: KeyboardEvent, repeat_delay: Duration) -> MonitorRoute {
        let mut recipients = Vec::new();
        let mut disposition = KeyboardDisposition::Pass;
        for (name, client) in &mut self.clients {
            let route = client.route(event, repeat_delay);
            if route.emit {
                recipients.push(name.clone());
            }
            disposition = disposition.max(route.disposition);
        }
        MonitorRoute {
            recipients,
            disposition,
        }
    }

    fn disconnected(&mut self, name: &OwnedUniqueName) {
        self.clients.remove(name);
    }
}

#[derive(Clone)]
struct KeyboardMonitor {
    data: Arc<Mutex<MonitorData>>,
    connection: Arc<OnceLock<Connection>>,
}

impl KeyboardMonitor {
    fn new() -> Self {
        Self {
            data: Arc::new(Mutex::new(MonitorData::default())),
            connection: Arc::new(OnceLock::new()),
        }
    }

    async fn authorized_sender(&self, header: Header<'_>) -> fdo::Result<OwnedUniqueName> {
        let connection = self
            .connection
            .get()
            .ok_or_else(|| fdo::Error::Failed("keyboard monitor is not started".to_owned()))?;
        super::dbus::require_name_owner(
            connection.inner(),
            header,
            AUTHORIZED_MONITOR,
            "only the active assistive-technology keyboard monitor is authorized",
        )
        .await
    }

    fn process_key(&self, event: KeyboardEvent) -> KeyboardDisposition {
        let route = self
            .data
            .lock()
            .expect("keyboard monitor state lock poisoned")
            .route(event, REPEAT_DELAY);
        let Some(connection) = self.connection.get() else {
            return KeyboardDisposition::Pass;
        };
        let keycode = match u16::try_from(event.keycode.raw()) {
            Ok(keycode) => keycode,
            Err(_) => {
                eventline::warn!(
                    "accessibility: XKB keycode {} does not fit the D-Bus interface",
                    event.keycode.raw()
                );
                return KeyboardDisposition::Pass;
            }
        };
        for recipient in route.recipients {
            if let Err(err) = connection.emit_signal(
                Some(BusName::Unique(recipient.as_ref())),
                OBJECT_PATH,
                INTERFACE,
                "KeyEvent",
                &(
                    event.released,
                    event.modifiers,
                    event.keysym.raw(),
                    event.unicode,
                    keycode,
                ),
            ) {
                eventline::warn!("accessibility: failed to emit keyboard event: {err}");
            }
        }
        route.disposition
    }
}

#[interface(name = "org.freedesktop.a11y.KeyboardMonitor")]
impl KeyboardMonitor {
    async fn grab_keyboard(&self, #[zbus(header)] header: Header<'_>) -> fdo::Result<()> {
        let sender = self.authorized_sender(header).await?;
        self.data
            .lock()
            .map_err(|_| fdo::Error::Failed("keyboard monitor state lock poisoned".to_owned()))?
            .clients
            .entry(sender)
            .or_default()
            .grabbed = true;
        Ok(())
    }

    async fn ungrab_keyboard(&self, #[zbus(header)] header: Header<'_>) -> fdo::Result<()> {
        let sender = self.authorized_sender(header).await?;
        if let Some(client) = self
            .data
            .lock()
            .map_err(|_| fdo::Error::Failed("keyboard monitor state lock poisoned".to_owned()))?
            .clients
            .get_mut(&sender)
        {
            client.grabbed = false;
        }
        Ok(())
    }

    async fn watch_keyboard(&self, #[zbus(header)] header: Header<'_>) -> fdo::Result<()> {
        let sender = self.authorized_sender(header).await?;
        self.data
            .lock()
            .map_err(|_| fdo::Error::Failed("keyboard monitor state lock poisoned".to_owned()))?
            .clients
            .entry(sender)
            .or_default()
            .watched = true;
        Ok(())
    }

    async fn unwatch_keyboard(&self, #[zbus(header)] header: Header<'_>) -> fdo::Result<()> {
        let sender = self.authorized_sender(header).await?;
        if let Some(client) = self
            .data
            .lock()
            .map_err(|_| fdo::Error::Failed("keyboard monitor state lock poisoned".to_owned()))?
            .clients
            .get_mut(&sender)
        {
            client.watched = false;
        }
        Ok(())
    }

    async fn set_key_grabs(
        &self,
        #[zbus(header)] header: Header<'_>,
        modifiers: Vec<u32>,
        keystrokes: Vec<(u32, u32)>,
    ) -> fdo::Result<()> {
        let sender = self.authorized_sender(header).await?;
        let mut data = self
            .data
            .lock()
            .map_err(|_| fdo::Error::Failed("keyboard monitor state lock poisoned".to_owned()))?;
        data.clients
            .entry(sender)
            .or_default()
            .replace_grabs(modifiers, keystrokes);
        Ok(())
    }

    #[zbus(signal)]
    async fn key_event(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        released: bool,
        state: u32,
        keysym: u32,
        unichar: u32,
        keycode: u16,
    ) -> zbus::Result<()>;
}

pub struct KeyboardMonitorService {
    monitor: KeyboardMonitor,
    _connection: Connection,
}

impl KeyboardMonitorService {
    pub fn start() -> Result<Self, Box<dyn std::error::Error>> {
        let monitor = KeyboardMonitor::new();
        let connection = Connection::session()?;
        monitor
            .connection
            .set(connection.clone())
            .map_err(|_| "keyboard monitor connection was already initialized")?;
        connection
            .object_server()
            .at(OBJECT_PATH, monitor.clone())?;
        connection.request_name_with_flags(
            BUS_NAME,
            RequestNameFlags::AllowReplacement
                | RequestNameFlags::ReplaceExisting
                | RequestNameFlags::DoNotQueue,
        )?;
        start_disconnect_monitor(&connection, monitor.data.clone());
        eventline::info!("accessibility: keyboard monitor ready");
        Ok(Self {
            monitor,
            _connection: connection,
        })
    }

    pub fn process_key(&self, event: KeyboardEvent) -> KeyboardDisposition {
        self.monitor.process_key(event)
    }
}

fn start_disconnect_monitor(connection: &Connection, data: Arc<Mutex<MonitorData>>) {
    let connection = connection.inner().clone();
    let task = connection.clone().executor().spawn(
        async move {
            let proxy = match fdo::DBusProxy::new(&connection).await {
                Ok(proxy) => proxy,
                Err(err) => {
                    eventline::warn!("accessibility: cannot monitor D-Bus clients: {err}");
                    return;
                }
            };
            let mut stream = match proxy
                .receive_name_owner_changed_with_args(&[(2, UniqueName::null_value())])
                .await
            {
                Ok(stream) => stream,
                Err(err) => {
                    eventline::warn!("accessibility: cannot watch client disconnects: {err}");
                    return;
                }
            };
            while let Some(signal) = stream.next().await {
                let Ok(args) = signal.args() else {
                    continue;
                };
                let Some(name) = &**args.old_owner() else {
                    continue;
                };
                if args.new_owner().is_some() {
                    continue;
                }
                data.lock()
                    .expect("keyboard monitor state lock poisoned")
                    .disconnected(&OwnedUniqueName::from(name.to_owned()));
            }
        },
        "halley accessibility client cleanup",
    );
    task.detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(
        time_ms: u64,
        keycode: u32,
        released: bool,
        modifiers: u32,
        keysym: Keysym,
        unicode: u32,
    ) -> KeyboardEvent {
        KeyboardEvent {
            time: Duration::from_millis(time_ms),
            keycode: Keycode::new(keycode),
            released,
            modifiers,
            keysym,
            unicode,
        }
    }

    fn client() -> (OwnedUniqueName, MonitorData) {
        let name = OwnedUniqueName::try_from(":1.42").unwrap();
        let mut data = MonitorData::default();
        data.clients.insert(name.clone(), Client::default());
        (name, data)
    }

    #[test]
    fn exact_keystroke_pairs_press_and_release_across_modifier_changes() {
        let (name, mut data) = client();
        data.clients
            .get_mut(&name)
            .unwrap()
            .replace_grabs(Vec::new(), vec![(Keysym::space.raw(), 4)]);
        let press = data.route(
            event(1, 65, false, 4, Keysym::space, ' ' as u32),
            REPEAT_DELAY,
        );
        data.clients
            .get_mut(&name)
            .unwrap()
            .replace_grabs(Vec::new(), Vec::new());
        let repeat = data.route(
            event(2, 65, false, 4, Keysym::space, ' ' as u32),
            REPEAT_DELAY,
        );
        let release = data.route(
            event(3, 65, true, 0, Keysym::space, ' ' as u32),
            REPEAT_DELAY,
        );
        assert_eq!(press.recipients.as_slice(), std::slice::from_ref(&name));
        assert!(repeat.recipients.is_empty());
        assert_eq!(release.recipients.as_slice(), std::slice::from_ref(&name));
        assert_eq!(press.disposition, KeyboardDisposition::Intercept);
        assert_eq!(repeat.disposition, KeyboardDisposition::Intercept);
        assert_eq!(release.disposition, KeyboardDisposition::Intercept);
    }

    #[test]
    fn modifier_release_survives_grab_replacement() {
        let (name, mut data) = client();
        data.clients
            .get_mut(&name)
            .unwrap()
            .replace_grabs(vec![Keysym::Insert.raw()], Vec::new());

        let press = data.route(event(1, 118, false, 0, Keysym::Insert, 0), REPEAT_DELAY);
        data.clients
            .get_mut(&name)
            .unwrap()
            .replace_grabs(Vec::new(), Vec::new());
        let release = data.route(event(2, 118, true, 0, Keysym::Insert, 0), REPEAT_DELAY);
        let unrelated = data.route(event(3, 38, false, 0, Keysym::a, 'a' as u32), REPEAT_DELAY);

        assert_eq!(press.recipients.as_slice(), std::slice::from_ref(&name));
        assert_eq!(release.recipients.as_slice(), std::slice::from_ref(&name));
        assert_eq!(press.disposition, KeyboardDisposition::Intercept);
        assert_eq!(release.disposition, KeyboardDisposition::Intercept);
        assert_eq!(unrelated.disposition, KeyboardDisposition::Pass);
    }

    #[test]
    fn held_key_repeat_is_blocked_without_duplicate_events() {
        let (name, mut data) = client();
        data.clients
            .get_mut(&name)
            .unwrap()
            .keystrokes
            .push((Keysym::space, 4));
        let first = data.route(event(1, 65, false, 4, Keysym::space, 32), REPEAT_DELAY);
        let repeat = data.route(event(2, 65, false, 4, Keysym::space, 32), REPEAT_DELAY);
        assert_eq!(first.recipients, [name]);
        assert!(repeat.recipients.is_empty());
        assert_eq!(repeat.disposition, KeyboardDisposition::Intercept);
    }

    #[test]
    fn exact_keystroke_does_not_capture_other_chords() {
        let (name, mut data) = client();
        data.clients
            .get_mut(&name)
            .unwrap()
            .keystrokes
            .push((Keysym::space, 4));

        for unrelated in [
            event(1, 65, false, 5, Keysym::space, ' ' as u32),
            event(2, 38, false, 4, Keysym::a, 'a' as u32),
            event(3, 65, false, 0, Keysym::space, ' ' as u32),
        ] {
            let route = data.route(unrelated, REPEAT_DELAY);
            assert!(route.recipients.is_empty());
            assert_eq!(route.disposition, KeyboardDisposition::Pass);
        }
    }

    #[test]
    fn watching_and_grabbing_emit_only_one_event_per_client() {
        let (name, mut data) = client();
        let client = data.clients.get_mut(&name).unwrap();
        client.watched = true;
        client.grabbed = true;
        client.keystrokes.push((Keysym::space, 4));
        let route = data.route(event(1, 65, false, 4, Keysym::space, 32), REPEAT_DELAY);
        assert_eq!(route.recipients, [name]);
    }

    #[test]
    fn modifier_grab_captures_chord_and_double_press_escapes() {
        let (name, mut data) = client();
        data.clients
            .get_mut(&name)
            .unwrap()
            .modifiers
            .insert(Keysym::Insert);
        let first = data.route(event(500, 118, false, 0, Keysym::Insert, 0), REPEAT_DELAY);
        let chord = data.route(
            event(510, 38, false, 0, Keysym::a, 'a' as u32),
            REPEAT_DELAY,
        );
        let chord_release =
            data.route(event(520, 38, true, 0, Keysym::a, 'a' as u32), REPEAT_DELAY);
        let release = data.route(event(530, 118, true, 0, Keysym::Insert, 0), REPEAT_DELAY);
        let second = data.route(event(600, 118, false, 0, Keysym::Insert, 0), REPEAT_DELAY);
        let second_release = data.route(event(610, 118, true, 0, Keysym::Insert, 0), REPEAT_DELAY);
        let third = data.route(event(750, 118, false, 0, Keysym::Insert, 0), REPEAT_DELAY);

        assert_eq!(first.disposition, KeyboardDisposition::Intercept);
        assert_eq!(chord.recipients.as_slice(), std::slice::from_ref(&name));
        assert_eq!(
            chord_release.recipients.as_slice(),
            std::slice::from_ref(&name)
        );
        assert_eq!(release.disposition, KeyboardDisposition::Intercept);
        assert_eq!(second.disposition, KeyboardDisposition::Pass);
        assert_eq!(second_release.disposition, KeyboardDisposition::Pass);
        assert_eq!(third.disposition, KeyboardDisposition::Intercept);
    }

    #[test]
    fn xkb_keycodes_and_event_values_are_preserved() {
        let event = event(1, 57 + 8, false, 4, Keysym::space, ' ' as u32);
        assert_eq!(u16::try_from(event.keycode.raw()), Ok(65));
        assert_eq!(event.keysym.raw(), 0x20);
        assert_eq!(event.unicode, 0x20);
    }

    #[test]
    fn disconnect_removes_every_client_capability() {
        let (name, mut data) = client();
        data.clients.get_mut(&name).unwrap().grabbed = true;
        data.disconnected(&name);
        let route = data.route(event(1, 65, false, 0, Keysym::space, 32), REPEAT_DELAY);
        assert!(route.recipients.is_empty());
        assert_eq!(route.disposition, KeyboardDisposition::Pass);
    }
}
