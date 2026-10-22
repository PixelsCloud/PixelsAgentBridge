//! QUIC path counters, independent of the currently selected application path.
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DirectTraffic {
    pub uploaded_bytes: u64,
    pub downloaded_bytes: u64,
    pub incomplete: bool,
}

#[derive(Default)]
pub(crate) struct PathTraffic {
    // Keep closed paths too: a delayed close event or snapshot must never count
    // the same path twice. Bound bookkeeping for very long-lived connections.
    paths: BTreeMap<String, (u64, u64)>,
    incomplete: bool,
}

impl PathTraffic {
    pub fn observe(&mut self, id: String, sent: u64, received: u64) {
        if self.paths.len() >= 4096 && !self.paths.contains_key(&id) {
            self.incomplete = true;
            return;
        }
        let previous = self.paths.entry(id).or_default();
        previous.0 = previous.0.max(sent);
        previous.1 = previous.1.max(received);
    }

    pub fn mark_incomplete(&mut self) {
        self.incomplete = true;
    }

    pub fn total(&self) -> DirectTraffic {
        let mut total = DirectTraffic {
            incomplete: self.incomplete,
            ..Default::default()
        };
        for &(sent, received) in self.paths.values() {
            match (
                total.uploaded_bytes.checked_add(sent),
                total.downloaded_bytes.checked_add(received),
            ) {
                (Some(upload), Some(download)) => {
                    total.uploaded_bytes = upload;
                    total.downloaded_bytes = download;
                }
                _ => {
                    total.uploaded_bytes = total.uploaded_bytes.saturating_add(sent);
                    total.downloaded_bytes = total.downloaded_bytes.saturating_add(received);
                    total.incomplete = true;
                }
            }
        }
        total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_changes_and_delayed_close_do_not_duplicate_or_reduce_usage() {
        let mut paths = PathTraffic::default();
        paths.observe("direct-a".into(), 100, 200);
        paths.observe("direct-b".into(), 20, 30);
        paths.observe("direct-a".into(), 120, 250); // final close counters
        paths.observe("direct-a".into(), 100, 200); // earlier snapshot
        assert_eq!(
            paths.total(),
            DirectTraffic {
                uploaded_bytes: 140,
                downloaded_bytes: 280,
                incomplete: false
            }
        );
        paths.mark_incomplete();
        assert!(paths.total().incomplete);
    }
}
