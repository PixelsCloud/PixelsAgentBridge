use crate::TransferSnapshot;

impl TransferSnapshot {
    /// Validate facts independently of message arrival order. Destination
    /// attributes can only be confirmed after publishing the complete file.
    pub fn validate_publication_facts(&self) -> Result<(), &'static str> {
        if self.offset > self.size {
            return Err("transfer receipt offset exceeds size");
        }
        if self.published == Some(true)
            && (self.direction != "receive" || self.offset != self.size)
        {
            return Err("publication requires a complete receiver receipt");
        }
        if let Some(result) = &self.metadata_result {
            result.validate()?;
            if self.published != Some(true) {
                return Err("attribute results require confirmed publication");
            }
        }
        Ok(())
    }

    /// True for a compatible new observation, false for an older in-flight
    /// observation. Contradictory durable facts are errors, never stale updates.
    pub fn supersedes(&self, previous: &Self) -> Result<bool, &'static str> {
        self.validate_publication_facts()?;
        if self.request_id != previous.request_id
            || self.initiated_by != previous.initiated_by
            || self.initiating_user != previous.initiating_user
            || self.path != previous.path
            || self.direction != previous.direction
            || self.execution_context != previous.execution_context
            || self.sha256.as_ref().zip(previous.sha256.as_ref()).is_some_and(|(a, b)| a != b)
        {
            return Err("transfer receipt identity changed");
        }
        if self.metadata_result.as_ref().zip(previous.metadata_result.as_ref())
            .is_some_and(|(a, b)| a != b)
        {
            return Err("confirmed attribute result changed");
        }
        let regressed = self.offset < previous.offset
            || self.size < previous.size
            || previous.sha256.is_some() && self.sha256.is_none()
            || previous.published == Some(true) && self.published != Some(true)
            || previous.metadata_result.is_some() && self.metadata_result.is_none()
            || matches!(previous.state.as_str(), "stopping" | "committing") && self.state == "running";
        if self.finished_at_unix_ms.is_none()
            && (previous.finished_at_unix_ms.is_some() || regressed)
        {
            return Ok(false);
        }
        if regressed || previous.finished_at_unix_ms.is_some() && self != previous {
            return Err("transfer receipt contradicts a durable fact");
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EndpointKey, FileTransferMetadata, FileTransferMetadataResult, OperatorRef, RequestId, UserId};

    fn receipt() -> TransferSnapshot {
        TransferSnapshot {
            request_id: RequestId::new(), initiated_by: OperatorRef::account(UserId::from_u128(1), EndpointKey::new([1; 32])),
            initiating_user: None, execution_context: None, direction: "receive".into(), path: "/file".into(),
            state: "running".into(), offset: 100, size: 100, sha256: Some("a".repeat(64)),
            finished_at_unix_ms: None, message: None, published: None, metadata_result: None,
        }
    }

    #[test]
    fn delayed_receipts_cannot_erase_publication_or_attributes() {
        let running = receipt();
        let mut published = running.clone();
        published.state = "committing".into(); published.published = Some(true);
        assert_eq!(published.supersedes(&running), Ok(true));
        assert_eq!(running.supersedes(&published), Ok(false));
        let mut attributed = published.clone();
        let metadata = FileTransferMetadata { modified_at_unix_ms: Some(123), unix_mode: None };
        attributed.metadata_result = Some(FileTransferMetadataResult { requested: metadata.clone(), actual: metadata, warnings: vec![] });
        assert_eq!(attributed.supersedes(&published), Ok(true));
        assert_eq!(published.supersedes(&attributed), Ok(false));
        let mut changed = attributed.clone();
        changed.metadata_result.as_mut().unwrap().actual.unix_mode = Some(0o600);
        assert!(changed.supersedes(&attributed).is_err());
        let mut done = attributed.clone(); done.state = "completed".into(); done.finished_at_unix_ms = Some(456);
        assert_eq!(done.supersedes(&attributed), Ok(true));
        assert_eq!(done.supersedes(&done), Ok(true));
        assert_eq!(running.supersedes(&done), Ok(false));
        let mut conflicting = done.clone(); conflicting.metadata_result = None;
        assert!(conflicting.supersedes(&attributed).is_err());
        conflicting = done.clone(); conflicting.finished_at_unix_ms = Some(457);
        assert!(conflicting.supersedes(&done).is_err());
        conflicting = running; conflicting.finished_at_unix_ms = Some(458); conflicting.state = "cancelled".into();
        assert!(conflicting.supersedes(&published).is_err());
    }

    #[test]
    fn malformed_or_foreign_receipts_are_not_treated_as_delayed_progress() {
        let original = receipt();
        let mut changed = original.clone(); changed.request_id = RequestId::new(); changed.offset = 0;
        assert!(changed.supersedes(&original).is_err());
        changed = original.clone(); changed.published = Some(true); changed.offset = 99;
        assert!(changed.validate_publication_facts().is_err());
        changed = original.clone(); changed.direction = "send".into(); changed.published = Some(true);
        assert!(changed.validate_publication_facts().is_err());
        changed = original.clone(); changed.offset = 101;
        assert!(changed.validate_publication_facts().is_err());
        changed = original.clone(); changed.metadata_result = Some(FileTransferMetadataResult { requested: Default::default(), actual: Default::default(), warnings: vec![] });
        assert!(changed.validate_publication_facts().is_err());
        changed = original.clone(); changed.sha256 = Some("b".repeat(64)); changed.offset = 0;
        assert!(changed.supersedes(&original).is_err());
        changed = original.clone(); changed.state = "stopping".into();
        assert_eq!(original.supersedes(&changed), Ok(false));
    }
}
