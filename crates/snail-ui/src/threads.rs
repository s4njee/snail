//! Message threading (plan.md E8.1), framework-free and unit-tested.
//!
//! A simplified JWZ algorithm: link each message to the last of its `References`/`In-Reply-To` that
//! we actually have, then reconcile with the server's own thread id and a subject fallback, so the
//! two never disagree on which thread a message belongs to.

use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadMessage {
    pub id: i64,
    pub message_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    pub subject: Option<String>,
    /// Gmail's `threadId`, or a CalDAV/provider-side grouping, when there is one.
    pub server_thread: Option<String>,
}

/// One canonical thread id per message. The id is the smallest message id in the thread, so it is
/// stable and independent of arrival order.
pub fn assign_threads(messages: &[ThreadMessage]) -> HashMap<i64, String> {
    let mut union = UnionFind::default();
    let mut by_message_id: HashMap<&str, i64> = HashMap::new();
    for message in messages {
        union.insert(message.id);
        if let Some(id) = message.message_id.as_deref() {
            by_message_id.entry(id).or_insert(message.id);
        }
    }

    // References win over In-Reply-To, and the *last* reference we hold is the parent (JWZ).
    for message in messages {
        let parent = message
            .references
            .iter()
            .rev()
            .chain(message.in_reply_to.iter())
            .find_map(|reference| by_message_id.get(reference.as_str()).copied());
        if let Some(parent) = parent {
            union.union(message.id, parent);
        }
    }

    // The server's own thread id must not disagree with ours: union same-server groups.
    let mut by_server: HashMap<&str, i64> = HashMap::new();
    for message in messages {
        if let Some(server) = message.server_thread.as_deref() {
            if let Some(existing) = by_server.get(server).copied() {
                union.union(message.id, existing);
            } else {
                by_server.insert(server, message.id);
            }
        }
    }

    // Subject fallback: rootless messages that share a normalized subject are one thread.
    let mut by_subject: HashMap<String, i64> = HashMap::new();
    for message in messages {
        let has_parent = message
            .references
            .iter()
            .chain(message.in_reply_to.iter())
            .any(|reference| by_message_id.contains_key(reference.as_str()));
        let Some(subject) = message.subject.as_deref().map(normalize_subject) else {
            continue;
        };
        if subject.is_empty() {
            continue;
        }
        if has_parent {
            continue;
        }
        if let Some(existing) = by_subject.get(&subject).copied() {
            union.union(message.id, existing);
        } else {
            by_subject.insert(subject, message.id);
        }
    }

    messages
        .iter()
        .map(|message| (message.id, union.canonical(message.id).to_string()))
        .collect()
}

/// Group message ids by their thread, ready for the list query (E8.3).
pub fn group_threads(threads: &HashMap<i64, String>) -> HashMap<String, Vec<i64>> {
    let mut grouped: HashMap<String, Vec<i64>> = HashMap::new();
    for (id, thread) in threads {
        grouped.entry(thread.clone()).or_default().push(*id);
    }
    for ids in grouped.values_mut() {
        ids.sort_unstable();
    }
    grouped
}

/// Strip any `Re:`/`Fwd:`/`Fw:` prefixes and lowercase, for the subject fallback.
pub fn normalize_subject(subject: &str) -> String {
    let mut text = subject.trim().to_ascii_lowercase();
    loop {
        let trimmed = text.trim_start();
        let lower = trimmed;
        if let Some(rest) = lower.strip_prefix("re:") {
            text = rest.trim().to_string();
        } else if let Some(rest) = lower.strip_prefix("fwd:") {
            text = rest.trim().to_string();
        } else if let Some(rest) = lower.strip_prefix("fw:") {
            text = rest.trim().to_string();
        } else {
            text = trimmed.to_string();
            break;
        }
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Default)]
struct UnionFind {
    parent: HashMap<i64, i64>,
}

impl UnionFind {
    fn insert(&mut self, id: i64) {
        self.parent.entry(id).or_insert(id);
    }

    fn find(&mut self, id: i64) -> i64 {
        let parent = *self.parent.get(&id).unwrap_or(&id);
        if parent == id {
            id
        } else {
            let root = self.find(parent);
            self.parent.insert(id, root);
            root
        }
    }

    fn union(&mut self, a: i64, b: i64) {
        let (root_a, root_b) = (self.find(a), self.find(b));
        if root_a != root_b {
            // Keep the smaller id as the root so the canonical id is the minimum.
            let (low, high) = if root_a <= root_b {
                (root_a, root_b)
            } else {
                (root_b, root_a)
            };
            self.parent.insert(high, low);
        }
    }

    fn canonical(&mut self, id: i64) -> i64 {
        self.find(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(id: i64, message_id: &str) -> ThreadMessage {
        ThreadMessage {
            id,
            message_id: Some(message_id.into()),
            in_reply_to: None,
            references: Vec::new(),
            subject: None,
            server_thread: None,
        }
    }

    #[test]
    fn a_reply_joins_its_parent_via_references() {
        let root = message(1, "a@x");
        let mut reply = message(2, "b@x");
        reply.references = vec!["a@x".into()];
        let threads = assign_threads(&[root, reply]);
        assert_eq!(threads[&1], threads[&2]);
    }

    #[test]
    fn a_chain_is_one_thread() {
        let root = message(1, "a@x");
        let mut second = message(2, "b@x");
        second.references = vec!["a@x".into()];
        let mut third = message(3, "c@x");
        third.references = vec!["a@x".into(), "b@x".into()];
        let threads = assign_threads(&[root, second, third]);
        assert_eq!(threads[&1], threads[&2]);
        assert_eq!(threads[&2], threads[&3]);
    }

    #[test]
    fn the_last_reference_we_hold_is_the_parent() {
        let root = message(1, "a@x");
        // `middle` is a separate thread (it does not reference a@x).
        let middle = message(2, "b@x");
        // The reply references both a@x and b@x, plus a missing c@x: it joins the last one we hold.
        let mut reply = message(3, "d@x");
        reply.references = vec!["a@x".into(), "b@x".into(), "c@x".into()];
        let threads = assign_threads(&[root, middle, reply]);
        assert_eq!(threads[&3], threads[&2], "joins the last held reference");
        assert_ne!(threads[&3], threads[&1], "not the earlier one");
    }

    #[test]
    fn a_server_thread_id_merges_messages_with_no_references() {
        let mut a = message(1, "a@x");
        let mut b = message(2, "b@x");
        a.server_thread = Some("gmail-thread-9".into());
        b.server_thread = Some("gmail-thread-9".into());
        let threads = assign_threads(&[a, b]);
        assert_eq!(threads[&1], threads[&2], "server id must not disagree with us");
    }

    #[test]
    fn subject_fallback_groups_rootless_messages() {
        let mut a = message(1, "a@x");
        let mut b = message(2, "b@x");
        a.subject = Some("Almanac geometry".into());
        b.subject = Some("Re: Almanac geometry".into());
        let threads = assign_threads(&[a, b]);
        assert_eq!(threads[&1], threads[&2]);
    }

    #[test]
    fn the_thread_id_is_the_smallest_message_id() {
        let mut reply = message(7, "b@x");
        reply.references = vec!["a@x".into()];
        let root = message(3, "a@x");
        let threads = assign_threads(&[reply, root]);
        assert_eq!(threads[&7], "3");
        assert_eq!(threads[&3], "3");
    }

    #[test]
    fn normalize_subject_strips_repeated_prefixes() {
        assert_eq!(normalize_subject("Re: Fwd:  Hello  World "), "hello world");
        assert_eq!(normalize_subject("RE: Hello"), "hello");
    }
}
