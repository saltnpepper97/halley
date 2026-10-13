//! Opt-in grants for desktop-wide protocols, evaluated at connection admission.
use smithay::reexports::wayland_server::Client;
use std::os::fd::AsRawFd;
use std::os::unix::{fs::MetadataExt, net::UnixStream};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[derive(Clone, Copy)]
pub enum Capability {
    Capture = 0,
    Clipboard = 1,
    VirtualKeyboard = 2,
    InputMethod = 3,
    OutputPower = 4,
}

const VARIABLES: [&str; 5] = [
    "HALLEY_ALLOW_SCREEN_CAPTURE",
    "HALLEY_ALLOW_CLIPBOARD_CONTROL",
    "HALLEY_ALLOW_VIRTUAL_KEYBOARD",
    "HALLEY_ALLOW_INPUT_METHOD",
    "HALLEY_ALLOW_OUTPUT_POWER",
];

#[derive(Default)]
pub struct Permissions([bool; 5]);

impl Permissions {
    pub fn for_socket(stream: &UnixStream) -> Self {
        static POLICY: OnceLock<[Vec<PathBuf>; 5]> = OnceLock::new();
        let policy = POLICY.get_or_init(|| {
            VARIABLES.map(|name| {
                std::env::var_os(name)
                    .map(|value| {
                        std::env::split_paths(&value)
                            .filter(|path| path.is_absolute())
                            .collect()
                    })
                    .unwrap_or_default()
            })
        });
        let mut credentials: libc::ucred = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of_val(&credentials) as libc::socklen_t;
        if unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&mut credentials as *mut libc::ucred).cast(),
                &mut len,
            )
        } != 0
            || credentials.pid <= 0
            || credentials.uid != unsafe { libc::geteuid() }
        {
            return Self::default();
        }
        let executable = PathBuf::from(format!("/proc/{}/exe", credentials.pid));
        Self(std::array::from_fn(|index| {
            policy[index]
                .iter()
                .any(|allowed| same_executable(&executable, allowed))
        }))
    }
}

fn same_executable(executable: &Path, allowed: &Path) -> bool {
    if !allowed.is_absolute() {
        return false;
    }
    let (Ok(actual), Ok(approved)) = (std::fs::metadata(executable), std::fs::metadata(allowed))
    else {
        return false;
    };
    actual.is_file()
        && approved.is_file()
        && actual.dev() == approved.dev()
        && actual.ino() == approved.ino()
}

pub fn allowed(client: &Client, capability: Capability) -> bool {
    client
        .get_data::<super::ClientState>()
        .is_some_and(|state| state.permissions.0[capability as usize])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ordinary_connections_have_no_desktop_capabilities() {
        assert_eq!(Permissions::default().0, [false; 5]);
    }
    #[test]
    fn grants_match_executable_identity_not_a_claimed_name() {
        let current = std::env::current_exe().unwrap();
        assert!(same_executable(Path::new("/proc/self/exe"), &current));
        assert!(!same_executable(
            Path::new("/proc/self/exe"),
            Path::new("/bin/sh")
        ));
        assert!(!same_executable(
            Path::new("/proc/self/exe"),
            Path::new("*")
        ));
        assert!(!same_executable(
            Path::new("/proc/self/exe"),
            Path::new("/missing/approved")
        ));
    }
    #[test]
    fn grants_are_per_client_and_per_capability() {
        use smithay::reexports::wayland_server::Display;
        use std::sync::Arc;
        let display = Display::<()>::new().unwrap();
        let mut handle = display.handle();
        let (socket, _peer) = UnixStream::pair().unwrap();
        let ordinary = handle
            .insert_client(socket, Arc::new(super::super::ClientState::default()))
            .unwrap();
        for capability in [
            Capability::Capture,
            Capability::Clipboard,
            Capability::VirtualKeyboard,
            Capability::InputMethod,
            Capability::OutputPower,
        ] {
            assert!(!allowed(&ordinary, capability));
        }
        let (socket, _peer2) = UnixStream::pair().unwrap();
        let ime = handle
            .insert_client(
                socket,
                Arc::new(super::super::ClientState {
                    permissions: Permissions([false, false, false, true, false]),
                    ..Default::default()
                }),
            )
            .unwrap();
        assert!(allowed(&ime, Capability::InputMethod));
        assert!(!allowed(&ime, Capability::Capture));
        assert!(!allowed(&ime, Capability::Clipboard));
        assert!(!allowed(&ime, Capability::VirtualKeyboard));
        assert!(!allowed(&ime, Capability::OutputPower));
        let (socket, _peer3) = UnixStream::pair().unwrap();
        let power = handle
            .insert_client(
                socket,
                Arc::new(super::super::ClientState {
                    permissions: Permissions([false, false, false, false, true]),
                    ..Default::default()
                }),
            )
            .unwrap();
        assert!(allowed(&power, Capability::OutputPower));
        assert!(!allowed(&power, Capability::Capture));
        assert!(!allowed(&power, Capability::InputMethod));
    }
}
