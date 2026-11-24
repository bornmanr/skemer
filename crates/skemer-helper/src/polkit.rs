use std::collections::HashMap;

use anyhow::{Context, Result};
use zbus::zvariant::Value;
use zbus::Connection;

pub const APPLY_ACTION: &str = "org.veldmuislinux.skemer.apply";
pub const MANAGE_PACKAGES_ACTION: &str = "org.veldmuislinux.skemer.manage-packages";
pub const MANAGE_REPOSITORIES_ACTION: &str = "org.veldmuislinux.skemer.manage-repositories";

const AUTHORITY: &str = "org.freedesktop.PolicyKit1";
const AUTHORITY_PATH: &str = "/org/freedesktop/PolicyKit1/Authority";
const AUTHORITY_INTERFACE: &str = "org.freedesktop.PolicyKit1.Authority";

/// Ask polkit to authorize `sender` for `action`. The action uses
/// `auth_admin_keep`, so a recent authentication is reused for the next apply.
pub async fn authorize(connection: &Connection, sender: &str, action: &str) -> Result<()> {
    if check(connection, sender, action, true).await? {
        Ok(())
    } else {
        anyhow::bail!("authentication was not granted");
    }
}

/// Run `CheckAuthorization`. `allow_interaction` controls whether polkit may
/// show an authentication dialog.
pub async fn check(
    connection: &Connection,
    sender: &str,
    action: &str,
    allow_interaction: bool,
) -> Result<bool> {
    let proxy = zbus::Proxy::new(connection, AUTHORITY, AUTHORITY_PATH, AUTHORITY_INTERFACE)
        .await
        .context("connect to the polkit authority")?;

    let subject_details: HashMap<&str, Value<'_>> = HashMap::from([("name", Value::from(sender))]);
    let subject = ("system-bus-name", subject_details);
    let details: HashMap<&str, &str> = HashMap::new();
    let flags = u32::from(allow_interaction);

    let (authorized, _challenge, _result_details): (bool, bool, HashMap<String, String>) = proxy
        .call("CheckAuthorization", &(subject, action, details, flags, ""))
        .await
        .context("call polkit CheckAuthorization")?;

    Ok(authorized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "needs the system bus and a running polkit authority"]
    async fn check_authorization_round_trips() {
        let connection = zbus::Connection::system().await.expect("system bus");
        let sender = connection.unique_name().expect("unique name").to_string();
        // Use a built-in action so the test does not need the Skemer policy
        // installed. This only validates the call signature and return type.
        let authorized = check(
            &connection,
            &sender,
            "org.freedesktop.login1.power-off",
            false,
        )
        .await
        .expect("polkit call");
        eprintln!("polkit authorized: {authorized}");
    }
}
