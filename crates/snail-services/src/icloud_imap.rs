//! The small authenticated-discovery slice of iCloud IMAP needed by Settings (E14.3).
//! Full CONDSTORE/QRESYNC mail synchronization remains in E4; setup only proves the app password,
//! re-issues CAPABILITY after login, and lists the real mailbox hierarchy before the first pull.

use anyhow::{Context, Result, anyhow};
use futures::TryStreamExt;

use crate::auth::{AuthError, imap_username};

const HOST: &str = "imap.mail.me.com";
const PORT: u16 = 993;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveredMailbox {
    pub name: String,
    pub kind: String,
}

/// Authenticate and list every selectable mailbox. This synchronous facade is deliberately used
/// only from Snail's background executor; the protocol implementation itself is Tokio-based.
pub fn discover_mailboxes(address: &str, password: &str) -> Result<Vec<DiscoveredMailbox>> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("start iCloud IMAP runtime")?;
    runtime.block_on(async {
        tokio::time::timeout(
            std::time::Duration::from_secs(45),
            discover_mailboxes_async(address, password),
        )
        .await
        .map_err(|_| anyhow!("iCloud IMAP discovery timed out"))?
    })
}

async fn discover_mailboxes_async(address: &str, password: &str) -> Result<Vec<DiscoveredMailbox>> {
    let tcp = tokio::net::TcpStream::connect((HOST, PORT))
        .await
        .context("connect to iCloud IMAP")?;
    let tls = async_native_tls::TlsConnector::new()
        .connect(HOST, tcp)
        .await
        .context("secure the iCloud IMAP connection")?;
    let mut client = async_imap::Client::new(tls);
    client
        .read_response()
        .await
        .context("read iCloud IMAP greeting")?
        .ok_or_else(|| anyhow!("iCloud IMAP closed before its greeting"))?;

    // Apple accepts the full address for some accounts and only the name part for others. A
    // failed LOGIN returns the client, so the documented fallback does not reconnect needlessly.
    let mut session = match client.login(address, password).await {
        Ok(session) => session,
        Err((_first_error, client)) => {
            let local = imap_username(address);
            client
                .login(local, password)
                .await
                .map_err(|(_error, _client)| AuthError::WrongAppPassword)?
        }
    };

    // iCloud's useful capabilities appear only after authentication.
    let _post_auth_capabilities = session
        .capabilities()
        .await
        .context("read post-auth iCloud IMAP capabilities")?;
    let names = {
        let stream = session
            .list(None, Some("*"))
            .await
            .context("list iCloud mailboxes")?;
        stream
            .try_collect::<Vec<_>>()
            .await
            .context("read iCloud mailbox list")?
    };
    let mut mailboxes = names
        .into_iter()
        .map(|name| {
            let name = name.name().to_string();
            DiscoveredMailbox {
                kind: mailbox_kind(&name).into(),
                name,
            }
        })
        .collect::<Vec<_>>();
    mailboxes.sort_by(|left, right| {
        mailbox_rank(&left.kind)
            .cmp(&mailbox_rank(&right.kind))
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    mailboxes.dedup_by(|left, right| left.name.eq_ignore_ascii_case(&right.name));
    let _ = session.logout().await;
    Ok(mailboxes)
}

fn mailbox_kind(name: &str) -> &'static str {
    let leaf = name
        .rsplit(['/', '.'])
        .next()
        .unwrap_or(name)
        .to_ascii_lowercase();
    match leaf.as_str() {
        "inbox" => "inbox",
        "sent" | "sent messages" => "sent",
        "draft" | "drafts" => "drafts",
        "archive" | "all mail" => "archive",
        "trash" | "deleted" | "deleted messages" => "trash",
        _ => "other",
    }
}

fn mailbox_rank(kind: &str) -> u8 {
    match kind {
        "inbox" => 0,
        "sent" => 1,
        "drafts" => 2,
        "archive" => 3,
        "trash" => 4,
        _ => 5,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovered_names_map_to_stable_mailbox_kinds() {
        assert_eq!(mailbox_kind("INBOX"), "inbox");
        assert_eq!(mailbox_kind("Sent Messages"), "sent");
        assert_eq!(mailbox_kind("Archive.Receipts"), "other");
        assert_eq!(mailbox_kind("Deleted Messages"), "trash");
        assert_eq!(mailbox_kind("Newsletters"), "other");
    }
}
