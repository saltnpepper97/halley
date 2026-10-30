//! D-Bus caller authorization used by accessibility services.

use std::process::Child;
use std::sync::{Arc, Mutex};
use zbus::fdo;
use zbus::message::Header;
use zbus::names::{BusName, OwnedUniqueName, UniqueName, WellKnownName};

fn authorized_name_owner(
    sender: &UniqueName<'_>,
    owner: OwnedUniqueName,
    denial: &str,
) -> fdo::Result<OwnedUniqueName> {
    if sender != &owner.as_ref() {
        return Err(fdo::Error::AccessDenied(denial.to_owned()));
    }
    Ok(owner)
}

pub(super) async fn require_name_owner(
    connection: &zbus::Connection,
    header: Header<'_>,
    authorized_name: &str,
    denial: &str,
    supervised_orca: &Arc<Mutex<Option<Child>>>,
) -> fdo::Result<OwnedUniqueName> {
    let sender = header
        .sender()
        .ok_or_else(|| fdo::Error::AccessDenied("missing D-Bus sender".to_owned()))?;
    let proxy = fdo::DBusProxy::new(connection)
        .await
        .map_err(|err| fdo::Error::Failed(err.to_string()))?;
    let authorized_name = WellKnownName::try_from(authorized_name)
        .map_err(|err| fdo::Error::Failed(err.to_string()))?;
    let owner = proxy
        .get_name_owner(BusName::WellKnown(authorized_name))
        .await
        .map_err(|_| fdo::Error::AccessDenied(denial.to_owned()))?;
    let owner = authorized_name_owner(sender, owner, denial)?;
    // A well-known name is freely claimable. Require either the explicitly
    // pinned connection or the exact live child Halley was asked to launch.
    let approved = std::env::var("HALLEY_ACCESSIBILITY_MONITOR").ok();
    if require_approved_connection(&owner, approved.as_deref()).is_err() {
        let pid = proxy
            .get_connection_unix_process_id(BusName::Unique(owner.as_ref()))
            .await
            .map_err(|_| fdo::Error::AccessDenied(denial.to_owned()))?;
        let mut child = supervised_orca
            .lock()
            .map_err(|_| fdo::Error::Failed("screen reader state lock poisoned".into()))?;
        let trusted = supervised_child_matches(child.as_mut(), pid);
        if !trusted {
            return Err(fdo::Error::AccessDenied(denial.to_owned()));
        }
    }
    Ok(owner)
}

fn supervised_child_matches(child: Option<&mut Child>, pid: u32) -> bool {
    child.is_some_and(|child| child.id() == pid && matches!(child.try_wait(), Ok(None)))
}

fn require_approved_connection(owner: &OwnedUniqueName, approved: Option<&str>) -> fdo::Result<()> {
    if approved.is_some_and(|name| name.starts_with(':') && name == owner.as_str()) {
        Ok(())
    } else {
        Err(fdo::Error::AccessDenied("keyboard monitoring requires an explicitly approved unique D-Bus connection in HALLEY_ACCESSIBILITY_MONITOR".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::{authorized_name_owner, require_approved_connection, supervised_child_matches};
    use zbus::{fdo, names::OwnedUniqueName};

    #[test]
    fn name_owner_authorization_accepts_the_owner() {
        let owner = OwnedUniqueName::try_from(":1.42").unwrap();
        assert_eq!(
            authorized_name_owner(&owner.as_ref(), owner.clone(), "denied").unwrap(),
            owner
        );
    }

    #[test]
    fn name_owner_authorization_rejects_an_untrusted_bus_client() {
        let owner = OwnedUniqueName::try_from(":1.42").unwrap();
        let untrusted = OwnedUniqueName::try_from(":1.43").unwrap();
        assert!(matches!(
            authorized_name_owner(&untrusted.as_ref(), owner, "denied"),
            Err(fdo::Error::AccessDenied(message)) if message == "denied"
        ));
    }
    #[test]
    fn claiming_orca_name_does_not_grant_keyboard_access() {
        let caller = OwnedUniqueName::try_from(":1.42").unwrap();
        for approval in [
            None,
            Some("org.gnome.Orca.KeyboardMonitor"),
            Some(":1.41"),
            Some("*"),
        ] {
            assert!(require_approved_connection(&caller, approval).is_err());
        }
        assert!(require_approved_connection(&caller, Some(":1.42")).is_ok());
    }

    #[test]
    fn supervisor_authorizes_only_its_live_child() {
        let mut child = std::process::Command::new("/usr/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let pid = child.id();
        assert!(!supervised_child_matches(None, pid));
        assert!(!supervised_child_matches(
            Some(&mut child),
            pid.wrapping_add(1)
        ));
        assert!(supervised_child_matches(Some(&mut child), pid));
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(!supervised_child_matches(Some(&mut child), pid));
    }
}
