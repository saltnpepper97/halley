//! D-Bus caller authorization used by accessibility services.

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
    authorized_name_owner(sender, owner, denial)
}

#[cfg(test)]
mod tests {
    use super::authorized_name_owner;
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
}
